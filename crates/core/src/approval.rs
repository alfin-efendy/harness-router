use crate::domain::{ApprovalKind, ApprovalResponse, PendingApprovalRow, Principal};
use crate::store::Store;
use std::collections::HashMap;
use std::sync::Mutex;
use tokio::sync::oneshot;

/// Identifies a pending approval within its owning durable agent run. Tool call
/// identifiers are only unique inside a provider turn, so `request_id` alone
/// is insufficient once delegated runs execute concurrently.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ApprovalKey {
    pub run_id: String,
    pub request_id: String,
}

impl ApprovalKey {
    pub fn new(run_id: impl Into<String>, request_id: impl Into<String>) -> Self {
        Self {
            run_id: run_id.into(),
            request_id: request_id.into(),
        }
    }
}

/// One parked approval: the reply channel plus (for native-runtime prompts)
/// the owning session, so a session-wide stop can deny everything it parked.
struct Pending {
    session_pk: Option<String>,
    tx: oneshot::Sender<ApprovalResponse>,
}

/// Shared registry of pending tool-permission requests. The native runtime's
/// permission gate (see `harness::native::permission`) registers a run-scoped
/// request key when it prompts the user; the UI resolves it via
/// [`ApprovalHub::resolve`].
pub struct ApprovalHub {
    pending: Mutex<HashMap<ApprovalKey, Pending>>,
}

impl ApprovalHub {
    pub fn new() -> ApprovalHub {
        ApprovalHub {
            pending: Mutex::new(HashMap::new()),
        }
    }

    pub fn register(&self, key: ApprovalKey) -> oneshot::Receiver<ApprovalResponse> {
        self.register_inner(None, key)
    }

    /// Register a pending approval tagged with its owning session, so
    /// [`ApprovalHub::resolve_session`] can deny it on a session-wide stop.
    pub fn register_for_session(
        &self,
        session_pk: &str,
        key: ApprovalKey,
    ) -> oneshot::Receiver<ApprovalResponse> {
        self.register_inner(Some(session_pk.to_string()), key)
    }

    fn register_inner(
        &self,
        session_pk: Option<String>,
        key: ApprovalKey,
    ) -> oneshot::Receiver<ApprovalResponse> {
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .unwrap()
            .insert(key, Pending { session_pk, tx });
        rx
    }

    /// Returns true if a pending request with this run-scoped key existed.
    pub fn resolve(&self, key: &ApprovalKey, response: ApprovalResponse) -> bool {
        if let Some(p) = self.pending.lock().unwrap().remove(key) {
            let _ = p.tx.send(response);
            true
        } else {
            false
        }
    }

    /// Binary convenience for callers that only know allow/deny (CLI y/N,
    /// gateway fan-out, cancellation cleanup).
    pub fn resolve_bool(&self, key: &ApprovalKey, allow: bool) -> bool {
        self.resolve(key, ApprovalResponse::once(allow))
    }

    /// Resolve every pending approval registered for `session_pk` (see
    /// [`ApprovalHub::register_for_session`]); unscoped registrations are
    /// never touched. Returns how many were resolved.
    pub fn resolve_session(&self, session_pk: &str, allow: bool) -> usize {
        let mut pending = self.pending.lock().unwrap();
        let keys: Vec<ApprovalKey> = pending
            .iter()
            .filter(|(_, p)| p.session_pk.as_deref() == Some(session_pk))
            .map(|(key, _)| key.clone())
            .collect();
        for key in &keys {
            if let Some(p) = pending.remove(key) {
                let _ = p.tx.send(ApprovalResponse::once(allow));
            }
        }
        keys.len()
    }

    /// Returns `true` if the hub currently has any unresolved registrations.
    /// Useful in tests to assert that the bridge never registered a request
    /// (i.e. auto-allow short-circuited before the hub).
    pub fn has_pending(&self) -> bool {
        !self.pending.lock().unwrap().is_empty()
    }
}

impl Default for ApprovalHub {
    fn default() -> Self {
        Self::new()
    }
}

/// Stable on-disk spelling of an approval kind. Explicit rather than reusing
/// serde so the DB text can never drift with a serde attribute change.
fn kind_to_db(kind: ApprovalKind) -> &'static str {
    match kind {
        ApprovalKind::Tool => "tool",
        ApprovalKind::Plan => "plan",
        ApprovalKind::Question => "question",
    }
}

fn kind_from_db(s: &str) -> ApprovalKind {
    match s {
        "plan" => ApprovalKind::Plan,
        "question" => ApprovalKind::Question,
        // Unknown/legacy text falls back to the most restrictive kind, which is
        // also the overwhelmingly common one.
        _ => ApprovalKind::Tool,
    }
}

/// Record a parked approval so a reconnecting surface can re-list it.
/// Best-effort by contract: the caller logs and continues on error — a failed
/// write must never change the permission verdict.
pub async fn persist_pending(store: &Store, row: PendingApprovalRow) -> anyhow::Result<()> {
    let input_json = serde_json::to_string(&row.input).unwrap_or_else(|_| "null".to_string());
    let principal_json = row
        .principal
        .as_ref()
        .and_then(|p| serde_json::to_string(p).ok());
    let kind = kind_to_db(row.approval_kind).to_string();
    store
        .with_conn(move |c| {
            c.execute(
                "INSERT INTO pending_approvals(run_id,request_id,session_pk,requesting_agent_id,\
                 requesting_agent_name,tool,summary,approval_kind,input_json,principal_json,created_at) \
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11) \
                 ON CONFLICT(run_id,request_id) DO NOTHING",
                rusqlite::params![
                    row.run_id,
                    row.request_id,
                    row.session_pk,
                    row.requesting_agent_id,
                    row.requesting_agent_name,
                    row.tool,
                    row.summary,
                    kind,
                    input_json,
                    principal_json,
                    row.created_at,
                ],
            )
            .map(|_| ())
        })
        .await
}

/// Forget a parked approval. Called by the parking call site once its
/// `tokio::select!` returns, whatever ended the park.
pub async fn delete_pending(store: &Store, run_id: &str, request_id: &str) -> anyhow::Result<()> {
    let run_id = run_id.to_string();
    let request_id = request_id.to_string();
    store
        .with_conn(move |c| {
            c.execute(
                "DELETE FROM pending_approvals WHERE run_id=?1 AND request_id=?2",
                rusqlite::params![run_id, request_id],
            )
            .map(|_| ())
        })
        .await
}

/// Every still-parked approval, oldest first.
pub async fn list_pending(store: &Store) -> anyhow::Result<Vec<PendingApprovalRow>> {
    store
        .with_conn(|c| {
            let mut stmt = c.prepare(
                "SELECT session_pk,run_id,request_id,requesting_agent_id,requesting_agent_name,\
                 tool,summary,approval_kind,input_json,principal_json,created_at \
                 FROM pending_approvals ORDER BY created_at ASC, run_id ASC, request_id ASC",
            )?;
            let rows = stmt
                .query_map([], |r| {
                    let kind: String = r.get(7)?;
                    let input: String = r.get(8)?;
                    let principal: Option<String> = r.get(9)?;
                    Ok(PendingApprovalRow {
                        session_pk: r.get(0)?,
                        run_id: r.get(1)?,
                        request_id: r.get(2)?,
                        requesting_agent_id: r.get(3)?,
                        requesting_agent_name: r.get(4)?,
                        tool: r.get(5)?,
                        summary: r.get(6)?,
                        approval_kind: kind_from_db(&kind),
                        input: serde_json::from_str(&input).unwrap_or(serde_json::Value::Null),
                        principal: principal
                            .as_deref()
                            .and_then(|p| serde_json::from_str::<Principal>(p).ok()),
                        created_at: r.get(10)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .await
}

/// Drop every persisted parked approval. Called once by
/// `daemon::build_daemon`: a reply channel cannot survive a process restart, so
/// rows from a previous boot are unanswerable and must not resurface as
/// un-actionable cards. Returns how many rows were removed.
pub async fn clear_all_pending(store: &Store) -> anyhow::Result<usize> {
    store
        .with_conn(|c| c.execute("DELETE FROM pending_approvals", []))
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_store() -> Store {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("t.db")).await.unwrap();
        // The tempdir must outlive the store in the caller; leak it for the
        // duration of the test process instead of threading a guard around.
        std::mem::forget(dir);
        store
    }

    fn row(run_id: &str, request_id: &str) -> PendingApprovalRow {
        PendingApprovalRow {
            session_pk: "s1".into(),
            run_id: run_id.into(),
            request_id: request_id.into(),
            requesting_agent_id: "agent-1".into(),
            requesting_agent_name: "Agent One".into(),
            tool: "bash".into(),
            summary: "Bash: rm".into(),
            approval_kind: ApprovalKind::Tool,
            input: serde_json::json!({"command": "rm -rf ./x"}),
            principal: None,
            created_at: 1,
        }
    }

    #[tokio::test]
    async fn persisted_pending_approval_round_trips_and_deletes() {
        let store = test_store().await;
        persist_pending(&store, row("run-1", "req-1"))
            .await
            .unwrap();
        let listed = list_pending(&store).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].run_id, "run-1");
        assert_eq!(listed[0].approval_kind, ApprovalKind::Tool);
        assert_eq!(listed[0].input["command"], "rm -rf ./x");

        delete_pending(&store, "run-1", "req-1").await.unwrap();
        assert!(list_pending(&store).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn persist_is_idempotent_and_delete_is_key_scoped() {
        let store = test_store().await;
        persist_pending(&store, row("run-a", "req-1"))
            .await
            .unwrap();
        persist_pending(&store, row("run-a", "req-1"))
            .await
            .unwrap();
        persist_pending(&store, row("run-b", "req-1"))
            .await
            .unwrap();
        assert_eq!(list_pending(&store).await.unwrap().len(), 2);

        delete_pending(&store, "run-a", "req-1").await.unwrap();
        let listed = list_pending(&store).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].run_id, "run-b");
    }

    #[tokio::test]
    async fn clear_all_pending_empties_the_table() {
        let store = test_store().await;
        persist_pending(&store, row("run-1", "req-1"))
            .await
            .unwrap();
        persist_pending(&store, row("run-2", "req-2"))
            .await
            .unwrap();
        assert_eq!(clear_all_pending(&store).await.unwrap(), 2);
        assert!(list_pending(&store).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn principal_and_non_tool_kind_survive_the_round_trip() {
        let store = test_store().await;
        let mut r = row("run-p", "req-p");
        r.approval_kind = ApprovalKind::Question;
        r.principal = Some(Principal {
            plugin_id: "acme-connector".into(),
            plugin_name: "Acme Connector".into(),
        });
        persist_pending(&store, r).await.unwrap();
        let listed = list_pending(&store).await.unwrap();
        assert_eq!(listed[0].approval_kind, ApprovalKind::Question);
        assert_eq!(
            listed[0].principal.as_ref().unwrap().plugin_id,
            "acme-connector"
        );
    }

    #[tokio::test]
    async fn register_then_resolve_completes_the_receiver() {
        let hub = ApprovalHub::new();
        let key = ApprovalKey::new("run-1", "req-1");
        let rx = hub.register(key.clone());
        assert!(hub.resolve_bool(&key, true));
        assert!(rx.await.unwrap().allowed());
        assert!(!hub.resolve_bool(&ApprovalKey::new("run-1", "nope"), true));
    }

    #[tokio::test]
    async fn resolve_requires_the_owning_run_identity() {
        let hub = ApprovalHub::new();
        let first = ApprovalKey::new("run-a", "request-1");
        let second = ApprovalKey::new("run-b", "request-1");
        let rx_first = hub.register(first.clone());
        let rx_second = hub.register(second.clone());

        assert!(hub.resolve_bool(&first, true));
        assert!(rx_first.await.unwrap().allowed());
        assert!(hub.has_pending());
        assert!(hub.resolve_bool(&second, false));
        assert!(!rx_second.await.unwrap().allowed());
    }

    #[tokio::test]
    async fn resolve_session_denies_only_that_sessions_pending_requests() {
        let hub = ApprovalHub::new();
        let rx_a = hub.register_for_session("sess-a", ApprovalKey::new("run-a", "req-1"));
        let rx_b = hub.register_for_session("sess-a", ApprovalKey::new("run-b", "req-2"));
        let rx_c = hub.register_for_session("sess-b", ApprovalKey::new("run-c", "req-3"));
        let plain = ApprovalKey::new("run-d", "req-4");
        let rx_plain = hub.register(plain.clone());

        assert_eq!(hub.resolve_session("sess-a", false), 2);
        assert!(!rx_a.await.unwrap().allowed());
        assert!(!rx_b.await.unwrap().allowed());
        assert!(hub.resolve_bool(&ApprovalKey::new("run-c", "req-3"), true));
        assert!(rx_c.await.unwrap().allowed());
        assert!(hub.resolve_bool(&plain, true));
        assert!(rx_plain.await.unwrap().allowed());
        assert_eq!(hub.resolve_session("sess-a", false), 0);
    }

    #[tokio::test]
    async fn resolve_carries_a_structured_response() {
        use crate::domain::{ApprovalDecision, ApprovalResponse, ApprovalScope};
        let hub = ApprovalHub::new();
        let key = ApprovalKey::new("run-s", "req-s");
        let rx = hub.register(key.clone());
        assert!(hub.resolve(
            &key,
            ApprovalResponse {
                decision: ApprovalDecision::AllowAlways,
                scope: Some(ApprovalScope::Project),
                payload: Some(serde_json::json!({"mode": "acceptEdits"})),
            },
        ));
        let got = rx.await.unwrap();
        assert_eq!(got.decision, ApprovalDecision::AllowAlways);
        assert_eq!(got.scope, Some(ApprovalScope::Project));
        assert!(got.allowed());
    }

    #[tokio::test]
    async fn resolve_bool_maps_to_once_decisions() {
        use crate::domain::ApprovalDecision;
        let hub = ApprovalHub::new();
        let key = ApprovalKey::new("run-b", "req-b");
        let rx = hub.register(key.clone());
        assert!(hub.resolve_bool(&key, false));
        let got = rx.await.unwrap();
        assert_eq!(got.decision, ApprovalDecision::RejectOnce);
        assert!(!got.allowed());
    }
}
