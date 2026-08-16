//! Scheduler domain: persisted jobs with cron schedules that really run —
//! a background loop starts an agent session with the job's prompt when a
//! schedule fires, and the run row closes when that session's turn completes.

use crate::automation::{AutomationEnvelope, AutomationSource, TriggerKind};
use crate::control::ControlPlane;
use crate::domain::CoreEvent;
use crate::store::Store;
use chrono::{DateTime, Local, TimeZone};
use croner::Cron;
use rusqlite::{params, OptionalExtension};
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq)]
pub struct JobRow {
    pub id: String,
    pub name: String,
    pub cron: String,
    /// natural | cron
    pub mode: String,
    pub natural_text: String,
    pub project_id: String,
    pub branch: String,
    pub gateway: String,
    pub enabled: bool,
    pub prompt: String,
    pub notify_success: bool,
    pub notify_fail: bool,
    /// Optional wake-gate command run before the agent wakes: empty stdout,
    /// non-zero exit, or timeout skips the fire; stdout is otherwise appended
    /// to the prompt as context.
    pub pre_check: String,
    /// Model id this job's session should start with, overriding the
    /// project's/agent's default. `None` keeps the ordinary resolution.
    pub model_override: Option<String>,
    /// The plugin that installed this job, if any (slot-4 origin column).
    /// `None` for a user-created job. Written only by
    /// `plugins::automation_sync` — the Scheduler screen's create/update
    /// commands never set this.
    pub plugin_id: Option<String>,
    /// The chat this job reports into: a `sessions.session_pk`, or `None` for
    /// a job that reports nowhere (the default). Written ONLY by the Cockpit
    /// job editor via `api::scheduler_api::update_job` — never by an agent
    /// (`control::app_control::create_job` always writes `None`) and never by
    /// a plugin manifest, because each delivery spends a real agent turn in
    /// that chat and the user has to have asked for it. Cleared automatically
    /// when the chat turns out to be gone (see `deliver_to_home_chat`).
    pub home_session_pk: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunRow {
    pub id: String,
    pub job_id: String,
    /// running | success | failed
    pub status: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub session_pk: Option<String>,
    pub error: Option<String>,
    pub add_lines: Option<i64>,
    pub del_lines: Option<i64>,
    pub note: Option<String>,
    pub log: Option<String>,
}

// ---------------------------------------------------------------------------
// Cron / natural language
// ---------------------------------------------------------------------------

/// Next occurrence of `cron_expr` strictly after `after` (epoch ms), in the
/// local timezone. None when the expression is invalid.
pub fn next_run_after(cron_expr: &str, after_ms: i64) -> Option<i64> {
    let cron = Cron::new(cron_expr).parse().ok()?;
    let after: DateTime<Local> = Local.timestamp_millis_opt(after_ms).single()?;
    let next = cron.find_next_occurrence(&after, false).ok()?;
    Some(next.timestamp_millis())
}

/// Rule-based English → cron for the patterns the UI offers. Returns None for
/// anything it can't parse confidently (the UI then asks for cron mode).
pub fn natural_to_cron(text: &str) -> Option<String> {
    let t = text.trim().to_lowercase();
    let t = t.strip_prefix("every ").unwrap_or(&t).trim().to_string();

    const DAYS: [(&str, u8); 7] = [
        ("sunday", 0),
        ("monday", 1),
        ("tuesday", 2),
        ("wednesday", 3),
        ("thursday", 4),
        ("friday", 5),
        ("saturday", 6),
    ];

    // "N minutes" / "minute"
    if t == "minute" {
        return Some("* * * * *".into());
    }
    if let Some(rest) = t
        .strip_suffix(" minutes")
        .or_else(|| t.strip_suffix(" mins"))
    {
        let n: u32 = rest.trim().parse().ok()?;
        if (1..60).contains(&n) {
            return Some(format!("*/{n} * * * *"));
        }
        return None;
    }
    // "N hours" / "hour"
    if t == "hour" {
        return Some("0 * * * *".into());
    }
    if let Some(rest) = t.strip_suffix(" hours") {
        let n: u32 = rest.trim().parse().ok()?;
        if (1..24).contains(&n) {
            return Some(format!("0 */{n} * * *"));
        }
        return None;
    }

    // "<scope> at <time>" where scope ∈ day | weekday name | weekdays
    let (scope, time) = match t.split_once(" at ") {
        Some((s, time)) => (s.trim(), time.trim()),
        None => return None,
    };
    let (hour, minute) = parse_time(time)?;
    if scope == "day" {
        return Some(format!("{minute} {hour} * * *"));
    }
    if scope == "weekday" || scope == "weekdays" {
        return Some(format!("{minute} {hour} * * 1-5"));
    }
    for (name, num) in DAYS {
        if scope == name || scope == name.trim_end_matches("day") {
            return Some(format!("{minute} {hour} * * {num}"));
        }
    }
    None
}

/// "2am", "9pm", "14:30", "9:15am", "12am" (midnight), "12pm" (noon).
fn parse_time(t: &str) -> Option<(u32, u32)> {
    let t = t.trim();
    let (body, pm) = if let Some(b) = t.strip_suffix("pm") {
        (b.trim(), Some(true))
    } else if let Some(b) = t.strip_suffix("am") {
        (b.trim(), Some(false))
    } else {
        (t, None)
    };
    let (h, m) = match body.split_once(':') {
        Some((h, m)) => (h.trim().parse::<u32>().ok()?, m.trim().parse::<u32>().ok()?),
        None => (body.trim().parse::<u32>().ok()?, 0),
    };
    if m >= 60 {
        return None;
    }
    let hour = match pm {
        Some(true) => {
            if h == 12 {
                12
            } else if h < 12 {
                h + 12
            } else {
                return None;
            }
        }
        Some(false) => {
            if h == 12 {
                0
            } else if h < 12 {
                h
            } else {
                return None;
            }
        }
        None => {
            if h < 24 {
                h
            } else {
                return None;
            }
        }
    };
    Some((hour, m))
}

// ---------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------

const JOB_COLS: &str =
    "id,name,cron,mode,natural_text,project_id,branch,gateway,enabled,prompt,notify_success,notify_fail,pre_check,model_override,plugin_id,home_session_pk";

fn job_from(r: &rusqlite::Row) -> rusqlite::Result<JobRow> {
    Ok(JobRow {
        id: r.get(0)?,
        name: r.get(1)?,
        cron: r.get(2)?,
        mode: r.get(3)?,
        natural_text: r.get(4)?,
        project_id: r.get(5)?,
        branch: r.get(6)?,
        gateway: r.get(7)?,
        enabled: r.get::<_, i64>(8)? != 0,
        prompt: r.get(9)?,
        notify_success: r.get::<_, i64>(10)? != 0,
        notify_fail: r.get::<_, i64>(11)? != 0,
        pre_check: r.get(12)?,
        model_override: r.get(13)?,
        plugin_id: r.get(14)?,
        home_session_pk: r.get(15)?,
    })
}

pub async fn list_jobs(store: &Store) -> anyhow::Result<Vec<JobRow>> {
    store
        .with_conn(|c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {JOB_COLS} FROM jobs ORDER BY created_at DESC"
            ))?;
            let rows = stmt
                .query_map([], job_from)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .await
}

pub async fn get_job(store: &Store, id: &str) -> anyhow::Result<Option<JobRow>> {
    let id = id.to_string();
    store
        .with_conn(move |c| {
            c.query_row(
                &format!("SELECT {JOB_COLS} FROM jobs WHERE id=?1"),
                params![id],
                job_from,
            )
            .optional()
        })
        .await
}

pub async fn upsert_job(store: &Store, job: JobRow) -> anyhow::Result<()> {
    let now = crate::paths::now_ms();
    store
        .with_conn(move |c| {
            c.execute(
                "INSERT INTO jobs(id,name,cron,mode,natural_text,project_id,branch,gateway,enabled,prompt,notify_success,notify_fail,pre_check,model_override,plugin_id,home_session_pk,created_at) \
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17) \
                 ON CONFLICT(id) DO UPDATE SET \
                   name=excluded.name, cron=excluded.cron, mode=excluded.mode, \
                   natural_text=excluded.natural_text, project_id=excluded.project_id, \
                   branch=excluded.branch, gateway=excluded.gateway, \
                   enabled=excluded.enabled, prompt=excluded.prompt, \
                   notify_success=excluded.notify_success, notify_fail=excluded.notify_fail, \
                   pre_check=excluded.pre_check, model_override=excluded.model_override, \
                   plugin_id=excluded.plugin_id, home_session_pk=excluded.home_session_pk",
                params![
                    job.id, job.name, job.cron, job.mode, job.natural_text, job.project_id,
                    job.branch, job.gateway, job.enabled as i64, job.prompt,
                    job.notify_success as i64, job.notify_fail as i64, job.pre_check,
                    job.model_override, job.plugin_id, job.home_session_pk, now
                ],
            )
            .map(|_| ())
        })
        .await
}

/// Delete every job `plugin_id` owns, and their run history — the uninstall
/// counterpart of `plugins::automation_sync::sync_plugin_automations`. Called
/// only from `plugins::automation_sync::remove_plugin_automations`.
pub async fn delete_jobs_and_runs_for_plugin(store: &Store, plugin_id: &str) -> anyhow::Result<()> {
    let plugin_id = plugin_id.to_string();
    store
        .with_conn(move |c| {
            c.execute(
                "DELETE FROM job_runs WHERE job_id IN (SELECT id FROM jobs WHERE plugin_id=?1)",
                params![plugin_id],
            )?;
            c.execute("DELETE FROM jobs WHERE plugin_id=?1", params![plugin_id])?;
            Ok(())
        })
        .await
}

/// Delete every job `plugin_id` owns whose `id` is NOT in `keep_ids`, and
/// their run history — the orphan-pruning counterpart of
/// [`delete_jobs_and_runs_for_plugin`], used when a plugin update's
/// manifest no longer declares a job it previously synced (F3: without
/// this, a removed job kept firing forever with its stale config). Scoped
/// to `plugin_id` only — a row with a different `plugin_id` (including a
/// user's own jobs, where `plugin_id IS NULL`) is never touched regardless
/// of an id collision. Returns the number of rows pruned.
pub async fn prune_jobs_and_runs_for_plugin(
    store: &Store,
    plugin_id: &str,
    keep_ids: &[String],
) -> anyhow::Result<usize> {
    let plugin_id = plugin_id.to_string();
    let keep_ids = keep_ids.to_vec();
    store
        .with_conn(move |c| {
            let mut stmt = c.prepare("SELECT id FROM jobs WHERE plugin_id=?1")?;
            let ids = stmt
                .query_map(params![plugin_id], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let mut pruned = 0usize;
            for id in ids {
                if keep_ids.iter().any(|kept| kept == &id) {
                    continue;
                }
                c.execute("DELETE FROM job_runs WHERE job_id=?1", params![id])?;
                c.execute("DELETE FROM jobs WHERE id=?1", params![id])?;
                pruned += 1;
            }
            Ok(pruned)
        })
        .await
}

/// Flip a job's `enabled` flag. Enabling is refused with a clear,
/// user-facing message when the job has no `project_id` yet — a
/// plugin-installed job lands exactly in this state on first sync (no
/// target project a plugin could ever guess), and flipping it on blind
/// would try to run an agent nowhere.
pub async fn toggle(store: &Store, id: &str, enabled: bool) -> anyhow::Result<()> {
    let mut job = get_job(store, id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("unknown job: {id}"))?;
    if enabled && job.project_id.trim().is_empty() {
        anyhow::bail!("pick a project first — this job has no project to run in");
    }
    job.enabled = enabled;
    upsert_job(store, job).await
}

pub async fn delete_job(store: &Store, id: &str) -> anyhow::Result<()> {
    let id = id.to_string();
    store
        .with_conn(move |c| {
            c.execute("DELETE FROM job_runs WHERE job_id=?1", params![id])?;
            c.execute("DELETE FROM jobs WHERE id=?1", params![id])
                .map(|_| ())
        })
        .await
}

/// Clear a job's report-to-chat binding. Called by the run watcher when the
/// bound chat turns out to be gone — an undeliverable rail row can never be
/// claimed (`Store::claim_deliverable_background_event` joins `sessions` and
/// requires `status='idle'`), so leaving the binding in place would queue one
/// permanently-pending row per run, forever.
pub async fn clear_home_session(store: &Store, job_id: &str) -> anyhow::Result<()> {
    let job_id = job_id.to_string();
    store
        .with_conn(move |c| {
            c.execute(
                "UPDATE jobs SET home_session_pk=NULL WHERE id=?1",
                params![job_id],
            )
            .map(|_| ())
        })
        .await
}

const RUN_COLS: &str =
    "id,job_id,status,started_at,finished_at,session_pk,error,add_lines,del_lines,note,log";

fn run_from(r: &rusqlite::Row) -> rusqlite::Result<RunRow> {
    Ok(RunRow {
        id: r.get(0)?,
        job_id: r.get(1)?,
        status: r.get(2)?,
        started_at: r.get(3)?,
        finished_at: r.get(4)?,
        session_pk: r.get(5)?,
        error: r.get(6)?,
        add_lines: r.get(7)?,
        del_lines: r.get(8)?,
        note: r.get(9)?,
        log: r.get(10)?,
    })
}

pub async fn list_runs(store: &Store, job_id: &str, limit: u32) -> anyhow::Result<Vec<RunRow>> {
    let job_id = job_id.to_string();
    store
        .with_conn(move |c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {RUN_COLS} FROM job_runs WHERE job_id=?1 ORDER BY started_at DESC LIMIT ?2"
            ))?;
            let rows = stmt
                .query_map(params![job_id, limit], run_from)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .await
}

pub async fn insert_run(store: &Store, run: RunRow) -> anyhow::Result<()> {
    store
        .with_conn(move |c| {
            c.execute(
                &format!(
                    "INSERT INTO job_runs({RUN_COLS}) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)"
                ),
                params![
                    run.id,
                    run.job_id,
                    run.status,
                    run.started_at,
                    run.finished_at,
                    run.session_pk,
                    run.error,
                    run.add_lines,
                    run.del_lines,
                    run.note,
                    run.log
                ],
            )
            .map(|_| ())
        })
        .await
}

#[allow(clippy::too_many_arguments)]
pub async fn finalize_run(
    store: &Store,
    run_id: &str,
    status: &str,
    finished_at: i64,
    session_pk: Option<String>,
    error: Option<String>,
    add_lines: Option<i64>,
    del_lines: Option<i64>,
    note: Option<String>,
) -> anyhow::Result<()> {
    let run_id = run_id.to_string();
    let status = status.to_string();
    store
        .with_conn(move |c| {
            c.execute(
                "UPDATE job_runs SET status=?2, finished_at=?3, session_pk=COALESCE(?4, session_pk), \
                 error=?5, add_lines=?6, del_lines=?7, note=?8 WHERE id=?1",
                params![run_id, status, finished_at, session_pk, error, add_lines, del_lines, note],
            )
            .map(|_| ())
        })
        .await
}

/// Whether the job has a run still marked running (guards double-fires).
pub async fn has_running_run(store: &Store, job_id: &str) -> anyhow::Result<bool> {
    let job_id = job_id.to_string();
    store
        .with_conn(move |c| {
            c.query_row(
                "SELECT COUNT(*) FROM job_runs WHERE job_id=?1 AND status='running'",
                params![job_id],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n > 0)
        })
        .await
}

/// On boot: close every `job_runs` row a dead process left mid-flight. The
/// scheduler-side twin of [`crate::automation::fail_incomplete_runs_on_restart`].
///
/// A row stuck at `running` is exactly the condition [`has_running_run`]
/// reports, and that guard is what both the scheduler tick and the "Run now"
/// path consult before firing a job — so a crash mid-run would otherwise wedge
/// that job forever, with no in-product recovery. Unlike the automation twin
/// there is no `queued` state to sweep here (`job_runs.status` is only
/// `running` | `success` | `failed`), so this clears precisely the wedging
/// condition and nothing else. Returns the number of rows closed.
pub async fn fail_incomplete_runs_on_restart(store: &Store) -> anyhow::Result<u64> {
    let now = crate::paths::now_ms();
    store
        .with_conn(move |c| {
            c.execute(
                "UPDATE job_runs
                 SET status='failed', error='restart interrupted', finished_at=?1
                 WHERE status='running'",
                params![now],
            )
            .map(|changed| changed as u64)
        })
        .await
}

// ---------------------------------------------------------------------------
// Silence + wake gates (hermes-agent cron conventions)
// ---------------------------------------------------------------------------

/// Prompt header teaching scheduled sessions the silence convention.
pub const SCHED_HEADER: &str = "[Scheduled run] If, after checking, there is nothing worth \
reporting or doing, reply with a single line starting with [SILENT] - the run is still \
recorded but no notification is delivered.";

/// Whether a scheduled run's final reply opts out of delivery.
pub(crate) fn is_silent(text: &str) -> bool {
    text.trim_start().starts_with("[SILENT]")
}

/// The (notify, note) decision for a finished run's final assistant text.
pub(crate) fn run_note_for(final_text: Option<&str>) -> (bool, Option<String>) {
    match final_text {
        Some(t) if is_silent(t) => (false, Some("[SILENT] suppressed".to_string())),
        _ => (true, None),
    }
}

/// Whether a finished run should raise a user-facing notification.
///
/// Composes the job's own switches with the run's `[SILENT]` opt-out:
/// - `success` needs `notify_success` AND a reply that did not open with
///   `[SILENT]` (`not_silenced` is the first element of [`run_note_for`]).
/// - `failed` is gated on `notify_fail` alone — the `[SILENT]` convention can
///   never apply, because a failed run has no final assistant text to read
///   (`final_text` is `None` on that path), so `run_note_for` always reports
///   `true` there and silencing a failure would be a lie.
/// - Anything non-terminal (`running`) never notifies.
///
/// This is only about the USER-FACING notification. The gateway activity log
/// (`crate::gateways::add_event`) is deliberately NOT gated on it — a failure
/// must stay in the diagnostic record whatever the user's preference.
pub(crate) fn should_notify_terminal(
    status: &str,
    notify_success: bool,
    notify_fail: bool,
    not_silenced: bool,
) -> bool {
    match status {
        "success" => notify_success && not_silenced,
        "failed" => notify_fail,
        _ => false,
    }
}

/// The final assistant message of a session: the trailing run of assistant
/// text rows (they are persisted delta-shaped), concatenated in order.
pub(crate) async fn final_assistant_text(store: &Store, session_pk: &str) -> Option<String> {
    let msgs = store.list_messages(session_pk).await.ok()?;
    let mut parts: Vec<String> = Vec::new();
    for m in msgs.iter().rev() {
        if m.role == "assistant" && m.block_type == "text" {
            if let Some(t) = m.payload.get("text").and_then(|t| t.as_str()) {
                parts.push(t.to_string());
            }
        } else if m.role == "assistant" && m.block_type == "thought" {
            continue;
        } else {
            break;
        }
    }
    if parts.is_empty() {
        return None;
    }
    parts.reverse();
    Some(parts.concat())
}

/// Outcome of a job's wake-gate pre-check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreCheckOutcome {
    /// Nothing to do - skip this fire entirely (reason for the log).
    Skip(String),
    /// Wake the agent; stdout is appended to the job prompt.
    Wake(String),
}

/// Run a job's `pre_check` command (60s cap; `cmd /C` on Windows, `sh -c`
/// elsewhere) in `workdir` (the job's project checkout) so repo-relative
/// checks evaluate against the right tree. Empty stdout, non-zero exit,
/// spawn failure, or timeout skips the fire.
pub async fn run_pre_check(cmd: &str, workdir: Option<&str>) -> PreCheckOutcome {
    let mut c = if cfg!(windows) {
        let mut c = tokio::process::Command::new("cmd");
        c.args(["/C", cmd]);
        c
    } else {
        let mut c = tokio::process::Command::new("sh");
        c.args(["-c", cmd]);
        c
    };
    if let Some(dir) = workdir {
        c.current_dir(dir);
    }
    // A timed-out future is dropped: without kill_on_drop the child would
    // keep running detached (the spawn convention everywhere else in core).
    c.kill_on_drop(true);
    crate::process_util::no_window(&mut c);
    match tokio::time::timeout(Duration::from_secs(60), c.output()).await {
        Err(_) => PreCheckOutcome::Skip("pre-check timed out after 60s".into()),
        Ok(Err(e)) => PreCheckOutcome::Skip(format!("pre-check failed to spawn: {e}")),
        Ok(Ok(o)) if !o.status.success() => {
            PreCheckOutcome::Skip(format!("pre-check exited with {}", o.status))
        }
        Ok(Ok(o)) => {
            let stdout = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if stdout.is_empty() {
                PreCheckOutcome::Skip("pre-check produced no output".into())
            } else {
                PreCheckOutcome::Wake(stdout)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Runner
// ---------------------------------------------------------------------------

/// Sum of `git diff --numstat HEAD` in `workdir` → (added, deleted).
pub async fn diff_totals(workdir: &str) -> Option<(i64, i64)> {
    let mut cmd = tokio::process::Command::new("git");
    cmd.args(["-C", workdir, "diff", "--numstat", "HEAD"]);
    crate::process_util::no_window(&mut cmd);
    let out = cmd.output().await.ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut add = 0i64;
    let mut del = 0i64;
    for line in text.lines() {
        let mut cols = line.split_whitespace();
        add += cols.next().and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
        del += cols.next().and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
    }
    Some((add, del))
}

/// Execute `job` now (a MANUAL run: no scheduled-run header, so the agent is
/// not taught the [SILENT] convention and a user-triggered run always
/// notifies): create the run row, start the agent session, and close the run
/// when the session's first turn completes. Returns the run id.
pub async fn execute_job(cp: &Arc<ControlPlane>, job: &JobRow) -> anyhow::Result<String> {
    run_job(cp, job, job.prompt.clone()).await
}

/// Execute a SCHEDULED fire: the prompt gains the [`SCHED_HEADER`] silence
/// convention plus any wake-gate pre-check output.
pub async fn execute_job_scheduled(
    cp: &Arc<ControlPlane>,
    job: &JobRow,
    pre_check_output: Option<String>,
) -> anyhow::Result<String> {
    let mut prompt = format!("{SCHED_HEADER}\n\n{}", job.prompt);
    if let Some(out) = &pre_check_output {
        prompt.push_str(&format!("\n\nPre-check output:\n{out}"));
    }
    run_job(cp, job, prompt).await
}

async fn emit_scheduler_terminal_automation(
    cp: &Arc<ControlPlane>,
    job_id: &str,
    run_id: &str,
    session_pk: Option<&str>,
    status: &str,
    error: Option<&str>,
) {
    let trigger = match status {
        "success" => TriggerKind::SchedulerRunSuccess,
        "failed" => TriggerKind::SchedulerRunFailed,
        _ => {
            tracing::warn!(run_id, status, "scheduler run has a non-terminal status");
            return;
        }
    };
    cp.dispatch_automation_event(
        AutomationEnvelope::new(
            trigger,
            chrono::Utc::now().to_rfc3339(),
            AutomationSource::new("scheduler.run", run_id),
            serde_json::json!({
                "jobId": job_id,
                "runId": run_id,
                "sessionPk": session_pk,
                "status": status,
                "error": error,
            }),
        ),
        None,
    )
    .await;
}

async fn run_job(cp: &Arc<ControlPlane>, job: &JobRow, prompt: String) -> anyhow::Result<String> {
    let store = cp.store().clone();
    let run_id = format!("r-{}", &crate::paths::new_id()[..8]);
    let started = crate::paths::now_ms();
    insert_run(
        &store,
        RunRow {
            id: run_id.clone(),
            job_id: job.id.clone(),
            status: "running".into(),
            started_at: started,
            finished_at: None,
            session_pk: None,
            error: None,
            add_lines: None,
            del_lines: None,
            note: None,
            log: None,
        },
    )
    .await?;
    let _ = crate::gateways::add_event(
        &store,
        &job.gateway,
        "info",
        &format!("job {} run {run_id} started", job.name),
    )
    .await;

    // Subscribe BEFORE starting so a fast turn can't slip past the listener.
    let mut rx = cp.subscribe();
    let session = match cp
        .start_session_with_prompt(
            &job.project_id,
            crate::harness::TurnPrompt::text(prompt.clone(), prompt.clone()),
            "scheduler",
            &[],
            None,
            None,
            job.model_override.clone(),
            None,
        )
        .await
    {
        Ok(s) => s,
        Err(e) => {
            let now = crate::paths::now_ms();
            finalize_run(
                &store,
                &run_id,
                "failed",
                now,
                None,
                Some(e.to_string()),
                None,
                None,
                None,
            )
            .await?;
            let _ = crate::gateways::add_event(
                &store,
                &job.gateway,
                "error",
                &format!("job {} run {run_id} failed to start: {e}", job.name),
            )
            .await;
            emit_scheduler_terminal_automation(
                cp,
                &job.id,
                &run_id,
                None,
                "failed",
                Some(&e.to_string()),
            )
            .await;
            let _ = cp.send_event(CoreEvent::JobRunChanged {
                job_id: job.id.clone(),
                run_id: run_id.clone(),
                status: "failed".into(),
                job_name: job.name.clone(),
                // The session never started, so there is no reply and no
                // `[SILENT]` decision to compose — this is purely the job's
                // own failure switch.
                notify: job.notify_fail,
                detail: Some(e.to_string()),
            });
            return Ok(run_id);
        }
    };

    let session_pk = session.session_pk.clone();
    let job_id = job.id.clone();
    let job_name = job.name.clone();
    let gateway = job.gateway.clone();
    let notify_success = job.notify_success;
    let notify_fail = job.notify_fail;
    let cp2 = Arc::clone(cp);
    let run_id2 = run_id.clone();
    tokio::spawn(async move {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2 * 60 * 60);
        let mut outcome: (&str, Option<String>) = ("failed", Some("run watcher stopped".into()));
        loop {
            let ev = tokio::time::timeout_at(deadline, rx.recv()).await;
            match ev {
                Ok(Ok(CoreEvent::Result { session_pk: pk })) if pk == session_pk => {
                    outcome = ("success", None);
                    break;
                }
                Ok(Ok(CoreEvent::Error {
                    session_pk: pk,
                    message,
                })) if pk == session_pk => {
                    outcome = ("failed", Some(message));
                    break;
                }
                Ok(Ok(_)) => continue,
                Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
                Ok(Err(_)) => break,
                Err(_) => {
                    outcome = ("failed", Some("timed out after 2h".into()));
                    break;
                }
            }
        }
        let (status, error) = outcome;
        // Session-first start returns a provisional row (worktree_path: None,
        // backfilled during background startup) — re-read the stored row so
        // diff stats see the real worktree path. By the time the terminal
        // Result lands, the backfill is long done: it precedes harness start,
        // which precedes the first turn.
        let worktree = cp2
            .store()
            .get_session(&session_pk)
            .await
            .ok()
            .flatten()
            .and_then(|s| s.worktree_path);
        let (add, del) = match &worktree {
            Some(wt) if status == "success" => diff_totals(wt).await.unwrap_or((0, 0)),
            _ => (0, 0),
        };
        let now = crate::paths::now_ms();
        let final_text = if status == "success" {
            final_assistant_text(cp2.store(), &session_pk).await
        } else {
            None
        };
        let (notify, silent_note) = run_note_for(final_text.as_deref());
        let note = silent_note.or_else(|| {
            if status == "success" && add == 0 && del == 0 {
                Some("No changes produced".to_string())
            } else {
                None
            }
        });
        let _ = finalize_run(
            cp2.store(),
            &run_id2,
            status,
            now,
            Some(session_pk.clone()),
            error.clone(),
            Some(add),
            Some(del),
            note,
        )
        .await;
        // Deliver a successful, non-`[SILENT]` run's report into the chat the
        // user bound this job to. The background rail is the ONLY delivery
        // path: no direct/in-memory hand-off to that session. Absent a binding
        // this is a no-op and the `add_event`/`JobRunChanged` notifications
        // below are unchanged.
        //
        // Three gates, because the rail replays this block through
        // `ControlPlane::continue_session_with_prompt` — one delivery is one
        // full agent turn in that chat, with real tokens and real tools:
        //   1. the run succeeded (a failure belongs in the run history and the
        //      notification path, not in a turn that says "it broke");
        //   2. the reply did not open with `[SILENT]` (`notify`), which is the
        //      whole point of the `SCHED_HEADER` convention;
        //   3. the user explicitly nominated a chat.
        if status == "success" && notify {
            if let Some(text) = &final_text {
                deliver_to_home_chat(&cp2, &job_id, &job_name, &gateway, text).await;
            }
        }
        let notify_user = should_notify_terminal(status, notify_success, notify_fail, notify);
        let detail = match &error {
            Some(e) => Some(e.clone()),
            None if status == "success" => Some(format!("+{add} −{del}")),
            None => None,
        };
        if status != "success" || notify {
            let level = if status == "success" {
                "success"
            } else {
                "error"
            };
            let text = match &error {
                Some(e) => format!("job {job_name} run {run_id2} failed — {e}"),
                None => format!("job {job_name} run {run_id2} finished — +{add} −{del}"),
            };
            let _ = crate::gateways::add_event(cp2.store(), &gateway, level, &text).await;
        }
        emit_scheduler_terminal_automation(
            &cp2,
            &job_id,
            &run_id2,
            Some(&session_pk),
            status,
            error.as_deref(),
        )
        .await;
        let _ = cp2.send_event(CoreEvent::JobRunChanged {
            job_id,
            run_id: run_id2,
            status: status.into(),
            job_name,
            notify: notify_user,
            detail,
        });
    });

    // Record the session on the run row right away so the UI can link to it.
    finalize_partial_session(&store, &run_id, &session.session_pk).await?;
    Ok(run_id)
}

async fn finalize_partial_session(
    store: &Store,
    run_id: &str,
    session_pk: &str,
) -> anyhow::Result<()> {
    let run_id = run_id.to_string();
    let session_pk = session_pk.to_string();
    store
        .with_conn(move |c| {
            c.execute(
                "UPDATE job_runs SET session_pk=?2 WHERE id=?1",
                params![run_id, session_pk],
            )
            .map(|_| ())
        })
        .await
}

/// The framing every scheduled-job report carries into its home chat.
///
/// The rail replays a report as a USER turn, so it needs framing: without it,
/// a run whose output reads "TODO: fix the flaky test" reads to the home
/// chat's agent as an instruction, and it goes and does it unattended. Pure
/// and separate so the delivered shape is unit-testable and named once.
pub(crate) fn scheduled_report_header(job_name: &str) -> String {
    format!(
        "[SCHEDULED JOB — {job_name}]\nThis is a finished scheduled run reporting in. \
         Relay what matters to me and stop; do not start new work unless I ask.\n\n"
    )
}

/// Enqueue a finished run's report onto the background rail, addressed to the
/// chat `job_id` is bound to. A no-op when the job has no binding.
///
/// Self-heals a dangling binding: when the bound chat's row is gone, or its
/// status is `Ended`, the binding is cleared and one line lands in the gateway
/// log. That is not politeness — `Store::claim_deliverable_background_event`
/// joins `sessions` and requires `status='idle'`, so a row aimed at such a
/// session can NEVER be claimed and would sit pending forever, one per run. An
/// ARCHIVED chat is still a live idle session and still receives reports;
/// archiving is list hygiene, not a lifecycle end.
async fn deliver_to_home_chat(
    cp: &Arc<ControlPlane>,
    job_id: &str,
    job_name: &str,
    gateway: &str,
    text: &str,
) {
    // Re-read the job instead of capturing the binding when the run started,
    // so a chat the user picked (or cleared) during a long run wins.
    let Ok(Some(job)) = get_job(cp.store(), job_id).await else {
        return;
    };
    let Some(home_pk) = job.home_session_pk.filter(|pk| !pk.trim().is_empty()) else {
        return;
    };
    // A store error is NOT evidence the chat is gone, so it must not unbind:
    // that would let one transient read failure throw away a setting only the
    // user can restore. Skip this one report and try again next run.
    let Ok(target) = cp.store().get_session(&home_pk).await else {
        return;
    };
    let deliverable = matches!(&target, Some(s) if s.status != crate::domain::SessionStatus::Ended);
    if !deliverable {
        let _ = clear_home_session(cp.store(), job_id).await;
        let _ = crate::gateways::add_event(
            cp.store(),
            gateway,
            "error",
            &format!(
                "job {job_name}: the chat it reported to no longer exists — unbound; \
                 pick another chat in the job's detail view"
            ),
        )
        .await;
        return;
    }
    let block = format!("{}{text}", scheduled_report_header(job_name));
    let _ = cp
        .store()
        .enqueue_background_event(&home_pk, "job", &block)
        .await;
}

/// Background loop: every 30s, fire enabled jobs whose next occurrence (after
/// the last fire) has passed. `last fired` persists in settings KV so app
/// restarts don't re-fire missed-by-restart schedules more than once.
///
/// Returned as a future (not self-spawned) so hosts can run it on their own
/// runtime — Tauri's setup hook has no ambient tokio context.
pub fn spawn_runner(cp: Arc<ControlPlane>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(run_loop(cp))
}

pub async fn run_loop(cp: Arc<ControlPlane>) {
    loop {
        tokio::time::sleep(Duration::from_secs(30)).await;
        tick(&cp).await;
    }
}

/// One scheduler pass: record liveness, then fire any due jobs (through their
/// wake-gate pre-checks). Factored out of [`run_loop`] so tests can drive it
/// without sleeping.
pub async fn tick(cp: &Arc<ControlPlane>) {
    let store = cp.store().clone();
    let now = crate::paths::now_ms();
    // Cheap staleness probe for health surfaces.
    let _ = store
        .set_setting(
            crate::domain::WriteOrigin::User,
            "scheduler_last_tick",
            &now.to_string(),
        )
        .await;
    let jobs = match list_jobs(&store).await {
        Ok(j) => j,
        Err(_) => return,
    };
    for job in jobs.into_iter().filter(|j| j.enabled) {
        // I1 fix: `job.enabled` alone used to gate firing, so disabling a
        // plugin left its `[[jobs]]` presets still due-checked and firing —
        // the scheduler-side twin of `automation::list_enabled_hooks`'s bug.
        // A job with no `plugin_id` (user-created) is unaffected; a
        // plugin-owned job is skipped unless its owner reads explicitly
        // enabled, matching every other plugin-enablement read's
        // default-to-disabled convention.
        if let Some(plugin_id) = &job.plugin_id {
            let key = crate::plugins::host::qualified_setting_key(plugin_id, "enabled");
            let plugin_enabled =
                store.get_setting_raw(&key).await.ok().flatten().as_deref() == Some("true");
            if !plugin_enabled {
                continue;
            }
        }
        let key = format!("job_last_fired.{}", job.id);
        let last_fired: i64 = store
            .get_setting(&key)
            .await
            .ok()
            .flatten()
            .and_then(|v| v.parse().ok())
            // First sighting: anchor at now so we fire on the NEXT occurrence.
            .unwrap_or(now);
        if last_fired == now {
            let _ = store
                .set_setting(crate::domain::WriteOrigin::User, &key, &now.to_string())
                .await;
            continue;
        }
        let Some(next) = next_run_after(&job.cron, last_fired) else {
            continue;
        };
        if next > now {
            continue;
        }
        if has_running_run(&store, &job.id).await.unwrap_or(true) {
            continue;
        }
        let _ = store
            .set_setting(crate::domain::WriteOrigin::User, &key, &now.to_string())
            .await;
        // Fire on a detached task: a slow/hung pre-check (up to 60s) must not
        // stall the other due jobs or the next liveness stamp. The anchor is
        // already advanced, so this fire cannot double-run.
        let cp2 = cp.clone();
        tokio::spawn(async move {
            // Wake gate: a configured pre-check must produce output, or the
            // fire is skipped entirely (no session, no run row).
            let pre = if job.pre_check.trim().is_empty() {
                None
            } else {
                let workdir = cp2
                    .store()
                    .get_project(&job.project_id)
                    .await
                    .ok()
                    .flatten()
                    .map(|p| p.workdir);
                match run_pre_check(&job.pre_check, workdir.as_deref()).await {
                    PreCheckOutcome::Skip(reason) => {
                        tracing::debug!("scheduler: job {} skipped ({reason})", job.id);
                        return;
                    }
                    PreCheckOutcome::Wake(out) => Some(out),
                }
            };
            let _ = execute_job_scheduled(&cp2, &job, pre).await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn prepare_test_agent_persistence(store: &std::sync::Arc<Store>) {
        crate::llm_router::connections::add_connection(
            store,
            crate::llm_router::connections::ConnectionRow {
                id: "test-anthropic".into(),
                provider: "anthropic".into(),
                auth_type: "api_key".into(),
                label: "Test Anthropic".into(),
                priority: 0,
                enabled: true,
                data: crate::llm_router::connections::ConnectionData {
                    api_key: Some("test-key".into()),
                    models_override: Some(vec!["claude-opus-4-8".into()]),
                    ..Default::default()
                },
                created_at: 0,
                updated_at: 0,
            },
        )
        .await
        .unwrap();
        crate::agents::bootstrap::ensure_default_routes(store)
            .await
            .unwrap();
    }

    #[test]
    fn natural_phrases_map_to_cron() {
        assert_eq!(
            natural_to_cron("every day at 2am").as_deref(),
            Some("0 2 * * *")
        );
        assert_eq!(
            natural_to_cron("every day at 14:30").as_deref(),
            Some("30 14 * * *")
        );
        assert_eq!(
            natural_to_cron("every monday at 9am").as_deref(),
            Some("0 9 * * 1")
        );
        assert_eq!(
            natural_to_cron("weekdays at 9:15am").as_deref(),
            Some("15 9 * * 1-5")
        );
        assert_eq!(
            natural_to_cron("every 6 hours").as_deref(),
            Some("0 */6 * * *")
        );
        assert_eq!(
            natural_to_cron("every 15 minutes").as_deref(),
            Some("*/15 * * * *")
        );
        assert_eq!(
            natural_to_cron("every day at 12am").as_deref(),
            Some("0 0 * * *")
        );
        assert_eq!(
            natural_to_cron("every day at 12pm").as_deref(),
            Some("0 12 * * *")
        );
        assert_eq!(natural_to_cron("whenever I feel like it"), None);
        assert_eq!(natural_to_cron("every day at 25:00"), None);
    }

    #[test]
    fn next_run_is_strictly_after_anchor() {
        // Daily at 02:00 — anchor at some fixed time; next must be within 24h and after.
        let now = crate::paths::now_ms();
        let next = next_run_after("0 2 * * *", now).expect("valid cron");
        assert!(next > now);
        assert!(next - now <= 24 * 60 * 60 * 1000 + 60_000);
        assert!(next_run_after("not a cron", now).is_none());
    }

    #[test]
    fn silent_prefix_detection_and_note() {
        assert!(is_silent("[SILENT] nothing to do"));
        assert!(is_silent("  [SILENT]"));
        assert!(!is_silent("done: [SILENT] not a prefix"));
        assert!(!is_silent("all good"));
        assert_eq!(
            run_note_for(Some("[SILENT] ok")),
            (false, Some("[SILENT] suppressed".to_string()))
        );
        assert_eq!(run_note_for(Some("did things")), (true, None));
        assert_eq!(run_note_for(None), (true, None));
    }

    #[test]
    fn terminal_notification_gate_composes_job_switches_with_silent() {
        // success needs the switch AND a non-`[SILENT]` reply
        assert!(should_notify_terminal("success", true, false, true));
        assert!(!should_notify_terminal("success", true, false, false));
        assert!(!should_notify_terminal("success", false, true, true));
        // failure is gated on `notify_fail` alone — the `[SILENT]` convention
        // never reaches it (a failed run has no final assistant text)
        assert!(should_notify_terminal("failed", false, true, true));
        assert!(should_notify_terminal("failed", false, true, false));
        assert!(!should_notify_terminal("failed", false, false, true));
        // a non-terminal status never notifies
        assert!(!should_notify_terminal("running", true, true, true));
    }

    #[tokio::test]
    async fn pre_check_gates_on_output_and_exit() {
        assert_eq!(
            run_pre_check("echo hi", None).await,
            PreCheckOutcome::Wake("hi".into())
        );
        assert!(matches!(
            run_pre_check("exit 1", None).await,
            PreCheckOutcome::Skip(_)
        ));
        // Succeeds but prints nothing: still a skip.
        let quiet = if cfg!(windows) { "rem quiet" } else { "true" };
        assert!(matches!(
            run_pre_check(quiet, None).await,
            PreCheckOutcome::Skip(_)
        ));
        // The command runs in the given workdir.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("flag.txt"), "x").unwrap();
        let list = if cfg!(windows) {
            "dir /b flag.txt"
        } else {
            "ls flag.txt"
        };
        assert_eq!(
            run_pre_check(list, Some(&dir.path().to_string_lossy())).await,
            PreCheckOutcome::Wake("flag.txt".into())
        );
    }

    #[tokio::test]
    async fn tick_records_scheduler_liveness() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = std::sync::Arc::new(Store::open(tmp.path()).await.unwrap());
        prepare_test_agent_persistence(&store).await;
        let cp = {
            let persistence = crate::agents::bootstrap::AgentPersistence::temporary(store.clone())
                .await
                .unwrap();
            crate::control::ControlPlane::new(store, crate::plugins::Registries::new(), persistence)
                .await
        };
        tick(&cp).await;
        let val = cp
            .store()
            .get_setting("scheduler_last_tick")
            .await
            .unwrap()
            .expect("liveness recorded");
        assert!(val.parse::<i64>().unwrap() > 0);
    }

    // I1: a plugin-owned job that's DUE must not fire while its owning
    // plugin is disabled — the scheduler-side twin of
    // `automation::list_enabled_hooks_excludes_a_disabled_plugins_hook_but_
    // keeps_user_hooks`. Bypasses `tick()`'s own first-sighting anchor (a
    // brand-new job never fires on its first tick — it just anchors) by
    // seeding `job_last_fired` far in the past, so the job reads as
    // genuinely due on the very first `tick()` call this test makes.
    #[tokio::test]
    async fn tick_skips_a_due_jobs_disabled_plugin_but_fires_once_enabled() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = std::sync::Arc::new(Store::open(tmp.path()).await.unwrap());
        prepare_test_agent_persistence(&store).await;
        let cp = {
            let persistence = crate::agents::bootstrap::AgentPersistence::temporary(store.clone())
                .await
                .unwrap();
            crate::control::ControlPlane::new(
                store.clone(),
                crate::plugins::Registries::new(),
                persistence,
            )
            .await
        };

        let mut job = sample_job("acme/nightly");
        job.plugin_id = Some("acme".into());
        upsert_job(&store, job.clone()).await.unwrap();

        let key = format!("job_last_fired.{}", job.id);
        let long_ago = crate::paths::now_ms() - 10 * 24 * 60 * 60 * 1000;
        store
            .set_setting(
                crate::domain::WriteOrigin::User,
                &key,
                &long_ago.to_string(),
            )
            .await
            .unwrap();

        // No `plugin.acme.enabled` setting yet (default-to-disabled) — the
        // fire anchor must not advance at all.
        tick(&cp).await;
        assert_eq!(
            store.get_setting(&key).await.unwrap(),
            Some(long_ago.to_string()),
            "a disabled plugin's due job must not fire"
        );

        // Enable the plugin — the SAME due job must now attempt to fire
        // (the anchor advances to "now").
        store
            .set_setting_raw("plugin.acme.enabled", "true")
            .await
            .unwrap();
        tick(&cp).await;
        let after: i64 = store
            .get_setting(&key)
            .await
            .unwrap()
            .and_then(|v| v.parse().ok())
            .unwrap();
        assert!(
            after > long_ago,
            "an enabled plugin's due job must attempt to fire (anchor must advance)"
        );

        // A user-created job (no plugin_id) is unaffected either way.
        let mut user_job = sample_job("user/nightly");
        user_job.plugin_id = None;
        upsert_job(&store, user_job.clone()).await.unwrap();
        let user_key = format!("job_last_fired.{}", user_job.id);
        store
            .set_setting(
                crate::domain::WriteOrigin::User,
                &user_key,
                &long_ago.to_string(),
            )
            .await
            .unwrap();
        tick(&cp).await;
        let user_after: i64 = store
            .get_setting(&user_key)
            .await
            .unwrap()
            .and_then(|v| v.parse().ok())
            .unwrap();
        assert!(
            user_after > long_ago,
            "a user-created job must never be gated on any plugin's enablement"
        );
    }

    #[tokio::test]
    async fn job_and_run_crud_roundtrip() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = std::sync::Arc::new(Store::open(tmp.path()).await.unwrap());

        let job = JobRow {
            id: "j1".into(),
            name: "Nightly audit".into(),
            cron: "0 2 * * *".into(),
            mode: "natural".into(),
            natural_text: "every day at 2am".into(),
            project_id: "p1".into(),
            branch: "main".into(),
            gateway: "local".into(),
            enabled: true,
            prompt: "Run npm audit".into(),
            notify_success: false,
            notify_fail: true,
            pre_check: "git status --short".into(),
            model_override: None,
            plugin_id: None,
            home_session_pk: None,
        };
        upsert_job(&store, job.clone()).await.unwrap();
        assert_eq!(get_job(&store, "j1").await.unwrap().unwrap(), job);

        insert_run(
            &store,
            RunRow {
                id: "r1".into(),
                job_id: "j1".into(),
                status: "running".into(),
                started_at: 1000,
                finished_at: None,
                session_pk: None,
                error: None,
                add_lines: None,
                del_lines: None,
                note: None,
                log: None,
            },
        )
        .await
        .unwrap();
        assert!(has_running_run(&store, "j1").await.unwrap());

        finalize_run(
            &store,
            "r1",
            "success",
            2000,
            Some("s-1".into()),
            None,
            Some(12),
            Some(4),
            None,
        )
        .await
        .unwrap();
        let runs = list_runs(&store, "j1", 10).await.unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, "success");
        assert_eq!(runs[0].add_lines, Some(12));
        assert_eq!(runs[0].session_pk.as_deref(), Some("s-1"));
        assert!(!has_running_run(&store, "j1").await.unwrap());

        delete_job(&store, "j1").await.unwrap();
        assert!(get_job(&store, "j1").await.unwrap().is_none());
        assert!(list_runs(&store, "j1", 10).await.unwrap().is_empty());
    }

    // A daemon restart mid-run used to leave `job_runs.status='running'`
    // forever, which `has_running_run` reads as "still executing" — so the
    // scheduler tick skipped the job and "Run now" refused. Reconciliation
    // must close exactly those rows and leave terminal ones alone.
    #[tokio::test]
    async fn restart_closes_running_job_runs_and_unblocks_the_job() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = std::sync::Arc::new(Store::open(tmp.path()).await.unwrap());
        upsert_job(&store, sample_job("j1")).await.unwrap();

        insert_run(
            &store,
            RunRow {
                id: "r-stale".into(),
                job_id: "j1".into(),
                status: "running".into(),
                started_at: 1000,
                finished_at: None,
                session_pk: None,
                error: None,
                add_lines: None,
                del_lines: None,
                note: None,
                log: None,
            },
        )
        .await
        .unwrap();
        insert_run(
            &store,
            RunRow {
                id: "r-done".into(),
                job_id: "j1".into(),
                status: "success".into(),
                started_at: 500,
                finished_at: Some(900),
                session_pk: Some("s-1".into()),
                error: None,
                add_lines: Some(3),
                del_lines: Some(1),
                note: None,
                log: None,
            },
        )
        .await
        .unwrap();

        let done_before = list_runs(&store, "j1", 10)
            .await
            .unwrap()
            .into_iter()
            .find(|run| run.id == "r-done")
            .unwrap();
        assert!(has_running_run(&store, "j1").await.unwrap());

        assert_eq!(fail_incomplete_runs_on_restart(&store).await.unwrap(), 1);

        let runs = list_runs(&store, "j1", 10).await.unwrap();
        let stale = runs.iter().find(|run| run.id == "r-stale").unwrap();
        assert_eq!(stale.status, "failed");
        assert_eq!(stale.error.as_deref(), Some("restart interrupted"));
        assert!(stale.finished_at.is_some());
        let done_after = runs.into_iter().find(|run| run.id == "r-done").unwrap();
        assert_eq!(done_after, done_before);

        // The job is eligible to fire again — this is the whole point.
        assert!(!has_running_run(&store, "j1").await.unwrap());
        // Idempotent: a second boot closes nothing.
        assert_eq!(fail_incomplete_runs_on_restart(&store).await.unwrap(), 0);
    }

    /// A minimal, otherwise-boring job row — tests that only care about one
    /// field (like `model_override`) mutate the field they need instead of
    /// repeating every field inline.
    fn sample_job(id: &str) -> JobRow {
        JobRow {
            id: id.into(),
            name: "test job".into(),
            cron: "0 2 * * *".into(),
            mode: "cron".into(),
            natural_text: String::new(),
            project_id: "p1".into(),
            branch: "main".into(),
            gateway: "local".into(),
            enabled: true,
            prompt: "do the thing".into(),
            notify_success: false,
            notify_fail: false,
            pre_check: String::new(),
            model_override: None,
            plugin_id: None,
            home_session_pk: None,
        }
    }

    #[tokio::test]
    async fn scheduler_model_override_is_session_scoped_without_mutating_primary_profile() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = std::sync::Arc::new(Store::open(tmp.path()).await.unwrap());
        prepare_test_agent_persistence(&store).await;
        let cp = {
            let persistence = crate::agents::bootstrap::AgentPersistence::temporary(store.clone())
                .await
                .unwrap();
            crate::control::ControlPlane::new(
                store.clone(),
                crate::plugins::Registries::new(),
                persistence,
            )
            .await
        };
        store
            .insert_project(crate::domain::Project {
                project_id: "p-override".into(),
                name: "override".into(),
                workdir: std::env::temp_dir().to_string_lossy().into_owned(),
                source: None,
                model: None,
                effort: None,
                perm_mode: crate::domain::PermMode::Default,
                created_at: Some(crate::paths::now_ms()),
                is_git: false,
            })
            .await
            .unwrap();
        let primary_id = cp.registry().default_agent_id().await;
        let profile_before = cp.registry().resolved_snapshot(&primary_id).await.unwrap();
        let session = cp
            .start_session_with_prompt(
                "p-override",
                crate::harness::TurnPrompt::text("run", "run"),
                "scheduler",
                &[],
                None,
                None,
                Some("scheduled/model".into()),
                None,
            )
            .await
            .unwrap();

        assert_eq!(
            store
                .get_session_runtime_settings(&session.session_pk)
                .await
                .unwrap()
                .and_then(|settings| settings.model),
            Some("scheduled/model".into())
        );
        assert_eq!(
            cp.registry()
                .resolved_snapshot(&primary_id)
                .await
                .unwrap()
                .profile,
            profile_before.profile
        );
    }
    #[tokio::test]
    async fn job_model_override_roundtrips() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = std::sync::Arc::new(Store::open(tmp.path()).await.unwrap());
        let mut job = sample_job("j-mo");
        job.model_override = Some("cheap/haiku".into());
        upsert_job(&store, job.clone()).await.unwrap();
        let got = get_job(&store, "j-mo").await.unwrap().unwrap();
        assert_eq!(got.model_override.as_deref(), Some("cheap/haiku"));

        // Clearing it back to None round-trips too (ON CONFLICT overwrite).
        job.model_override = None;
        upsert_job(&store, job.clone()).await.unwrap();
        let got = get_job(&store, "j-mo").await.unwrap().unwrap();
        assert_eq!(got.model_override, None);
    }

    // The binding is a first-class job column, not a `session_surfaces` row:
    // it must survive `upsert_job`'s ON CONFLICT overwrite in both directions,
    // and `clear_home_session` must be able to drop it without touching
    // anything else on the row.
    #[tokio::test]
    async fn home_session_binding_roundtrips_and_clears() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = std::sync::Arc::new(Store::open(tmp.path()).await.unwrap());
        let mut job = sample_job("j-home");
        assert_eq!(job.home_session_pk, None, "a new job reports nowhere");

        job.home_session_pk = Some("chat-1".into());
        upsert_job(&store, job.clone()).await.unwrap();
        assert_eq!(
            get_job(&store, "j-home")
                .await
                .unwrap()
                .unwrap()
                .home_session_pk
                .as_deref(),
            Some("chat-1")
        );

        clear_home_session(&store, "j-home").await.unwrap();
        let cleared = get_job(&store, "j-home").await.unwrap().unwrap();
        assert_eq!(cleared.home_session_pk, None);
        // Clearing the binding must not disturb the rest of the row.
        assert_eq!(
            JobRow {
                home_session_pk: Some("chat-1".into()),
                ..cleared
            },
            job
        );
    }

    // A run whose reply opts out with `[SILENT]` must not spend an agent turn
    // in the home chat saying nothing. `run_note_for`'s first element is that
    // decision and the delivery gate now ANDs it in.
    #[test]
    fn silent_runs_are_not_worth_a_turn_in_the_home_chat() {
        let (notify_silent, _) = run_note_for(Some("[SILENT] nothing to report"));
        let (notify_real, _) = run_note_for(Some("found 3 new issues"));
        assert!(!notify_silent, "a [SILENT] run must not deliver");
        assert!(notify_real, "a run with something to say must deliver");
    }

    /// The framing is what stops the home chat's agent from treating a report
    /// like an instruction, so pin its shape rather than just its prefix.
    #[test]
    fn a_report_is_framed_as_a_report_not_a_task() {
        let header = scheduled_report_header("nightly audit");
        assert!(
            header.starts_with("[SCHEDULED JOB — nightly audit]\n"),
            "{header:?}"
        );
        assert!(
            header.contains("do not start new work unless I ask"),
            "the framing must tell the home agent not to act on the report: {header:?}"
        );
        assert!(
            header.ends_with("\n\n"),
            "the run's own text follows on its own paragraph: {header:?}"
        );
    }

    // -----------------------------------------------------------------
    // Rail-delivery test fixtures. Mirrors the fake-harness pattern each
    // test module keeps privately (see `control::tests`, `background_rail`,
    // `harness::native::runner`) rather than reaching into another module's
    // private `#[cfg(test)] mod tests`.
    // -----------------------------------------------------------------

    /// Redirects `dirs::data_dir()`/HOME into a tempdir for the duration of a
    /// test, so a chat session's scratch dir never touches the real state
    /// dir. Process-global env — every test using it must be `#[serial]`.
    struct SchedulerStateDirGuard {
        _dir: tempfile::TempDir,
    }
    impl SchedulerStateDirGuard {
        fn new() -> Self {
            let dir = tempfile::tempdir().expect("tempdir");
            std::env::set_var("XDG_DATA_HOME", dir.path().join("data"));
            std::env::set_var("HOME", dir.path());
            SchedulerStateDirGuard { _dir: dir }
        }
    }

    /// A fake `HarnessSession` that persists a deterministic assistant reply
    /// so `final_assistant_text` (and thus the rail delivery block) has
    /// something real to read, without touching a live LLM. The reply is
    /// configurable so a test can drive the `[SILENT]` opt-out through the
    /// real watcher instead of only unit-testing `run_note_for`.
    struct FakeJobSession {
        store: std::sync::Arc<Store>,
        events: tokio::sync::broadcast::Sender<CoreEvent>,
        session_pk: String,
        reply: String,
    }

    #[async_trait::async_trait]
    impl crate::harness::HarnessSession for FakeJobSession {
        async fn send_prompt(&self, prompt: crate::harness::TurnPrompt) -> anyhow::Result<()> {
            let _ = self
                .store
                .insert_message(crate::domain::NewMessage::block(
                    &self.session_pk,
                    "user",
                    "text",
                    serde_json::json!({ "text": prompt.display }),
                ))
                .await;
            if let Ok(seq) = self
                .store
                .insert_message(crate::domain::NewMessage::block(
                    &self.session_pk,
                    "assistant",
                    "text",
                    serde_json::json!({ "text": self.reply }),
                ))
                .await
            {
                let _ = self.events.send(CoreEvent::Message {
                    session_pk: self.session_pk.clone(),
                    seq,
                    run_id: None,
                    role: "assistant".into(),
                    block_type: "text".into(),
                    payload: serde_json::json!({ "text": self.reply }),
                    tool_call_id: None,
                    status: None,
                    tool_kind: None,
                    speaker: None,
                });
            }
            Ok(())
        }
        async fn cancel(&self) -> anyhow::Result<()> {
            Ok(())
        }
        async fn end(&self) -> anyhow::Result<()> {
            Ok(())
        }
        fn agent_session_id(&self) -> Option<String> {
            Some("agent-fake".into())
        }
    }

    struct FakeJobHarness {
        reply: String,
    }

    #[async_trait::async_trait]
    impl crate::harness::Harness for FakeJobHarness {
        async fn start_session(
            &self,
            ctx: crate::harness::SessionCtx,
        ) -> anyhow::Result<Box<dyn crate::harness::HarnessSession>> {
            Ok(Box::new(FakeJobSession {
                store: ctx.store.clone(),
                events: ctx.events.clone(),
                session_pk: ctx.session_pk.clone(),
                reply: self.reply.clone(),
            }))
        }
    }

    struct FakeJobHarnessFactory {
        reply: String,
    }

    impl FakeJobHarnessFactory {
        /// The ordinary "the run had something to say" harness.
        fn new() -> Self {
            FakeJobHarnessFactory {
                reply: "done".to_string(),
            }
        }
        /// A harness whose every session replies with `reply` — used to drive
        /// the `[SILENT]` opt-out through the real run watcher.
        fn replying(reply: &str) -> Self {
            FakeJobHarnessFactory {
                reply: reply.to_string(),
            }
        }
    }

    impl crate::harness::HarnessFactory for FakeJobHarnessFactory {
        fn create(&self) -> anyhow::Result<std::sync::Arc<dyn crate::harness::Harness>> {
            Ok(std::sync::Arc::new(FakeJobHarness {
                reply: self.reply.clone(),
            }))
        }
    }

    /// Poll the rail (bounded) until a `kind='job'` row lands, claiming (and
    /// thus returning) it — mirrors `harness::native::runner`'s
    /// `wait_for_rail_row`.
    async fn wait_for_job_rail_row(store: &Store) -> crate::domain::BackgroundEvent {
        for _ in 0..200 {
            if let Some(row) = store
                .claim_deliverable_background_event("test-poll")
                .await
                .unwrap()
            {
                return row;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("no rail row appeared within the poll window");
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn completed_job_delivers_to_its_home_session_via_rail() {
        let _guard = SchedulerStateDirGuard::new();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = std::sync::Arc::new(Store::open(tmp.path()).await.unwrap());
        prepare_test_agent_persistence(&store).await;
        let mut regs = crate::plugins::Registries::new();
        regs.harness = std::sync::Arc::new(FakeJobHarnessFactory::new());
        let cp = {
            let persistence = crate::agents::bootstrap::AgentPersistence::temporary(store.clone())
                .await
                .unwrap();
            ControlPlane::new(store, regs, persistence).await
        };

        // A non-git project the job runs against — the fake harness needs no
        // real repo, so any workdir will do.
        cp.store()
            .insert_project(crate::domain::Project {
                project_id: "p-deliver".into(),
                name: "demo".into(),
                workdir: std::env::temp_dir().to_string_lossy().into_owned(),
                source: None,
                model: None,
                effort: None,
                perm_mode: crate::domain::PermMode::Default,
                created_at: Some(crate::paths::now_ms()),
                is_git: false,
            })
            .await
            .unwrap();

        // The chat the user bound this job to.
        let home = cp
            .start_chat_session(
                crate::harness::TurnPrompt::text("home", "home"),
                "test",
                &[],
            )
            .await
            .unwrap();
        // Let the home session's own startup + first turn settle to idle
        // before the job fires, or the rail claim (idle-only) would never
        // see it as a deliverable target.
        for _ in 0..400 {
            if cp
                .store()
                .get_session(&home.session_pk)
                .await
                .unwrap()
                .map(|s| s.status == crate::domain::SessionStatus::Idle)
                .unwrap_or(false)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        let mut job = sample_job("j-deliver");
        job.project_id = "p-deliver".into();
        job.gateway = "local".into();
        job.home_session_pk = Some(home.session_pk.clone());
        upsert_job(cp.store(), job.clone()).await.unwrap();

        execute_job(&cp, &job).await.unwrap();

        let row = wait_for_job_rail_row(cp.store()).await;
        assert_eq!(row.kind, "job");
        assert_eq!(row.target_session_pk, home.session_pk);
        assert!(
            row.payload
                .starts_with(&scheduled_report_header("test job")),
            "the report must carry the relay framing, got: {}",
            row.payload
        );
        assert!(
            row.payload.contains("done"),
            "expected the job's final assistant text in the rail payload, got: {}",
            row.payload
        );
    }

    // The regression this plan fixes, driven through the real watcher rather
    // than through `run_note_for` alone: the old delivery block gated on
    // `status == "success"` only, so a run that correctly answered "[SILENT]
    // nothing to report" still burned a full agent turn in the user's chat to
    // say nothing — on an hourly job, every hour. The binding must survive
    // (this is a silent run, not a broken one) and the rail must stay empty.
    #[tokio::test]
    #[serial_test::serial]
    async fn a_silent_run_delivers_nothing_and_keeps_its_binding() {
        let _guard = SchedulerStateDirGuard::new();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = std::sync::Arc::new(Store::open(tmp.path()).await.unwrap());
        prepare_test_agent_persistence(&store).await;
        let mut regs = crate::plugins::Registries::new();
        regs.harness = std::sync::Arc::new(FakeJobHarnessFactory::replying(
            "[SILENT] nothing to report",
        ));
        let cp = {
            let persistence = crate::agents::bootstrap::AgentPersistence::temporary(store.clone())
                .await
                .unwrap();
            ControlPlane::new(store, regs, persistence).await
        };
        cp.store()
            .insert_project(crate::domain::Project {
                project_id: "p-silent".into(),
                name: "demo".into(),
                workdir: std::env::temp_dir().to_string_lossy().into_owned(),
                source: None,
                model: None,
                effort: None,
                perm_mode: crate::domain::PermMode::Default,
                created_at: Some(crate::paths::now_ms()),
                is_git: false,
            })
            .await
            .unwrap();

        // A real, live, idle chat — so the ONLY thing that can suppress the
        // delivery is the `[SILENT]` gate, not an undeliverable target.
        let home = cp
            .start_chat_session(
                crate::harness::TurnPrompt::text("home", "home"),
                "test",
                &[],
            )
            .await
            .unwrap();
        for _ in 0..400 {
            if cp
                .store()
                .get_session(&home.session_pk)
                .await
                .unwrap()
                .map(|s| s.status == crate::domain::SessionStatus::Idle)
                .unwrap_or(false)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        let mut job = sample_job("j-silent");
        job.project_id = "p-silent".into();
        job.gateway = "local".into();
        job.home_session_pk = Some(home.session_pk.clone());
        upsert_job(cp.store(), job.clone()).await.unwrap();

        execute_job(&cp, &job).await.unwrap();

        // Wait for the watcher to actually finish the run, so "no rail row"
        // means "the gate suppressed it", not "the run hadn't closed yet".
        let mut closed = false;
        for _ in 0..400 {
            let runs = list_runs(cp.store(), "j-silent", 5).await.unwrap();
            if runs.iter().any(|r| r.status == "success") {
                closed = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(closed, "the silent run should still have closed as success");

        assert!(
            cp.store()
                .claim_deliverable_background_event("test-poll")
                .await
                .unwrap()
                .is_none(),
            "a [SILENT] run must not spend an agent turn in the home chat"
        );
        assert_eq!(
            get_job(cp.store(), "j-silent")
                .await
                .unwrap()
                .unwrap()
                .home_session_pk
                .as_deref(),
            Some(home.session_pk.as_str()),
            "a silent run is not a broken binding — it must stay bound"
        );
    }

    // A chat that was deleted (or ended) can never receive a rail row —
    // `claim_deliverable_background_event` joins `sessions` and requires
    // `status='idle'` — so a report aimed at one would sit pending forever,
    // once per run. The watcher must clear the binding instead of enqueuing.
    #[tokio::test]
    #[serial_test::serial]
    async fn a_report_to_a_vanished_chat_unbinds_instead_of_queueing_forever() {
        let _guard = SchedulerStateDirGuard::new();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = std::sync::Arc::new(Store::open(tmp.path()).await.unwrap());
        prepare_test_agent_persistence(&store).await;
        let mut regs = crate::plugins::Registries::new();
        regs.harness = std::sync::Arc::new(FakeJobHarnessFactory::new());
        let cp = {
            let persistence = crate::agents::bootstrap::AgentPersistence::temporary(store.clone())
                .await
                .unwrap();
            ControlPlane::new(store, regs, persistence).await
        };
        cp.store()
            .insert_project(crate::domain::Project {
                project_id: "p-ghost".into(),
                name: "demo".into(),
                workdir: std::env::temp_dir().to_string_lossy().into_owned(),
                source: None,
                model: None,
                effort: None,
                perm_mode: crate::domain::PermMode::Default,
                created_at: Some(crate::paths::now_ms()),
                is_git: false,
            })
            .await
            .unwrap();

        let mut job = sample_job("j-ghost");
        job.project_id = "p-ghost".into();
        job.home_session_pk = Some("chat-that-never-existed".into());
        upsert_job(cp.store(), job.clone()).await.unwrap();

        execute_job(&cp, &job).await.unwrap();

        // The watcher runs on a spawned task; poll (bounded) for the unbind.
        let mut cleared = false;
        for _ in 0..400 {
            if get_job(cp.store(), "j-ghost")
                .await
                .unwrap()
                .unwrap()
                .home_session_pk
                .is_none()
            {
                cleared = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(cleared, "a binding to a vanished chat must be cleared");
        assert!(
            cp.store()
                .claim_deliverable_background_event("test-poll")
                .await
                .unwrap()
                .is_none(),
            "nothing may be queued for a chat that cannot receive it"
        );
    }
}
