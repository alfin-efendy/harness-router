//! The unified "/" catalog: slash commands plus user-invocable skills.
//!
//! One merge layer answers both "what does '/' autocomplete list?" (per
//! surface, per agent binding) and "what does '/name args' run?". Commands
//! win name clashes; the losing skill stays reachable through the `skill`
//! tool, so nothing registers twice for the model.

use super::commands::{Command, CommandOrigin, CommandRegistry, CommandSurfaces, ResolvedCommand};
use super::skills::{Skill, SkillOrigin, SkillRegistry};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Maximum nesting for `/command` lines inside expanded templates.
const NESTED_EXPANSION_DEPTH: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlashKind {
    Command,
    Skill,
}

#[derive(Debug, Clone)]
pub struct SlashEntry {
    pub name: String,
    pub description: String,
    pub kind: SlashKind,
    pub origin: CommandOrigin,
    pub surfaces: CommandSurfaces,
    pub requires_project: bool,
    pub effective: bool,
    pub shadows_global: bool,
    pub agent: Option<String>,
    pub model: Option<String>,
    pub subtask: bool,
}

pub struct SlashCatalog {
    commands: CommandRegistry,
    skills: SkillRegistry,
    allowed_skills: Option<Vec<String>>,
}

impl SlashCatalog {
    pub fn load(project_dir: Option<&Path>, allowed_skills: Option<&[String]>) -> SlashCatalog {
        Self::load_with_plugins(project_dir, allowed_skills, &[], &[])
    }

    /// Like [`Self::load`], plus every ENABLED, installed plugin's
    /// `commands/` and `skills/` directories (Tasks 8/9) —
    /// `plugin_command_roots`/`plugin_skill_roots` are each `(plugin_id,
    /// <install_dir>/{commands,skills})`, provided by the control plane
    /// (`crate::control::ControlPlane::enabled_plugin_content_roots`). No
    /// merge-logic change here: commands still win name clashes against
    /// skills, exactly as [`Self::entries`]/[`Self::resolve`] already do.
    pub fn load_with_plugins(
        project_dir: Option<&Path>,
        allowed_skills: Option<&[String]>,
        plugin_command_roots: &[(String, PathBuf)],
        plugin_skill_roots: &[(String, PathBuf)],
    ) -> SlashCatalog {
        let (commands, skills) = match project_dir {
            Some(dir) => (
                CommandRegistry::load_with_plugins(dir, plugin_command_roots),
                SkillRegistry::load_with_plugin_roots(dir, plugin_skill_roots),
            ),
            None => (
                CommandRegistry::load_without_project_with_plugins(plugin_command_roots),
                SkillRegistry::load_global_with_plugin_roots(plugin_skill_roots),
            ),
        };
        SlashCatalog {
            commands,
            skills,
            allowed_skills: allowed_skills.map(<[String]>::to_vec),
        }
    }

    fn global_skill_listed(name: &str, allowed: Option<&[String]>) -> bool {
        allowed.is_some_and(|list| list.iter().any(|s| s == name))
    }

    /// Whether `skill` surfaces in "/" autocomplete. Project skills always
    /// list; a Global OR Plugin-origin skill lists only when bound to the
    /// agent (`allowed_skills`) — plugin skills behave exactly like global
    /// ones here (Task 9): both stay reachable through the `skill` tool's
    /// index regardless of this gate.
    fn skill_listed(&self, skill: &Skill) -> bool {
        match skill.origin {
            SkillOrigin::Project => true,
            SkillOrigin::Global | SkillOrigin::Plugin => {
                Self::global_skill_listed(&skill.name, self.allowed_skills.as_deref())
            }
        }
    }

    /// Merged autocomplete entries. Command sources come through unchanged
    /// (including shadowed ones, for the Automation tab) — builtins are
    /// always listed, project or not; it's the callers that decide what's
    /// home-visible without a project (Home's client-side filters combine
    /// surface + requiresProject, see `matchSlashEntries`), not this API.
    /// Listed skills are appended unless a command already owns the name;
    /// the collision set spans the FULL registry (builtins included) —
    /// `resolve()` always consults the full registry via `self.commands.get`,
    /// so a name a builtin owns must never list as a skill either.
    pub fn entries(&self) -> Vec<SlashEntry> {
        let command_names: BTreeSet<String> = self
            .commands
            .catalog()
            .into_iter()
            .map(|entry| entry.command.name)
            .collect();
        let mut entries: Vec<SlashEntry> = self
            .commands
            .catalog()
            .into_iter()
            .map(|entry| {
                command_entry(
                    &entry.command,
                    entry.origin,
                    entry.effective,
                    entry.shadows_global,
                )
            })
            .collect();
        for skill in self.skills.all() {
            if !self.skill_listed(&skill) || command_names.contains(&skill.name) {
                continue;
            }
            entries.push(SlashEntry {
                name: skill.name.clone(),
                description: skill.description.clone(),
                kind: SlashKind::Skill,
                origin: match skill.origin {
                    SkillOrigin::Project => CommandOrigin::Project,
                    SkillOrigin::Global => CommandOrigin::Global,
                    SkillOrigin::Plugin => CommandOrigin::Plugin,
                },
                surfaces: CommandSurfaces::default(),
                requires_project: false,
                effective: true,
                shadows_global: false,
                agent: None,
                model: None,
                subtask: false,
            });
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        entries
    }

    /// Resolve `/name args…` to a runnable prompt: command (with nested
    /// expansion) or listed-skill invocation. `None` for anything else.
    pub fn resolve(&self, input: &str) -> Option<ResolvedCommand> {
        let trimmed = input.trim_start();
        let rest = trimmed.strip_prefix('/')?;
        let (name, args) = match rest.split_once(char::is_whitespace) {
            Some((n, a)) => (n, a),
            None => (rest, ""),
        };
        if let Some(cmd) = self.commands.get(name) {
            let mut visited = BTreeSet::from([name.to_string()]);
            let prompt = self.expand_lines(&cmd.expand(args), 1, &mut visited);
            return Some(ResolvedCommand {
                prompt,
                agent: cmd.agent,
                model: cmd.model,
                subtask: cmd.subtask,
            });
        }
        let skill = self.skills.get(name).filter(|s| self.skill_listed(s))?;
        Some(ResolvedCommand {
            prompt: skill_invocation_prompt(&skill, args),
            agent: None,
            model: None,
            subtask: false,
        })
    }

    /// Expand `/name args` lines inside a prompt. Depth-limited, cycle-guarded;
    /// unknown or revisited names stay literal so a turn never hard-fails.
    fn expand_lines(&self, prompt: &str, depth: usize, visited: &mut BTreeSet<String>) -> String {
        if depth > NESTED_EXPANSION_DEPTH {
            return prompt.to_string();
        }
        let lines: Vec<String> = prompt
            .lines()
            .map(|line| {
                let Some(rest) = line.trim_start().strip_prefix('/') else {
                    return line.to_string();
                };
                let (name, args) = match rest.split_once(char::is_whitespace) {
                    Some((n, a)) => (n, a),
                    None => (rest, ""),
                };
                if visited.contains(name) {
                    return line.to_string();
                }
                if let Some(cmd) = self.commands.get(name) {
                    visited.insert(name.to_string());
                    return self.expand_lines(&cmd.expand(args), depth + 1, visited);
                }
                if let Some(skill) = self.skills.get(name).filter(|s| self.skill_listed(s)) {
                    visited.insert(name.to_string());
                    return skill_invocation_prompt(&skill, args);
                }
                line.to_string()
            })
            .collect();
        lines.join("\n")
    }
}

fn command_entry(
    command: &Command,
    origin: CommandOrigin,
    effective: bool,
    shadows_global: bool,
) -> SlashEntry {
    SlashEntry {
        name: command.name.clone(),
        description: command.description.clone(),
        kind: SlashKind::Command,
        origin,
        surfaces: command.surfaces,
        requires_project: command.requires_project,
        effective,
        shadows_global,
        agent: command.agent.clone(),
        model: command.model.clone(),
        subtask: command.subtask,
    }
}

/// The same body the `skill` tool serves, plus the user's arguments — a
/// user-invoked skill and a model-invoked one read identically.
fn skill_invocation_prompt(skill: &Skill, args: &str) -> String {
    let mut prompt = format!("# Skill: {}\n\n{}", skill.name, skill.body);
    let args = args.trim();
    if !args.is_empty() {
        prompt.push_str(&format!("\n\nArguments: {args}"));
    }
    prompt
}

/// First `@name` token naming a known agent, for template-driven routing.
pub fn first_agent_mention<'a>(prompt: &str, known: &'a [String]) -> Option<&'a str> {
    for token in prompt.split_whitespace() {
        let Some(raw) = token.strip_prefix('@') else {
            continue;
        };
        let name =
            raw.trim_end_matches(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'));
        if let Some(hit) = known.iter().find(|k| k.as_str() == name) {
            return Some(hit.as_str());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project_with(commands: &[(&str, &str)], skills: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".ryuzi/commands")).unwrap();
        for (name, body) in commands {
            std::fs::write(
                dir.path().join(format!(".ryuzi/commands/{name}.md")),
                format!("---\ndescription: {name}\n---\n{body}"),
            )
            .unwrap();
        }
        for (name, body) in skills {
            let sd = dir.path().join(format!(".agents/skills/{name}"));
            std::fs::create_dir_all(&sd).unwrap();
            std::fs::write(
                sd.join("SKILL.md"),
                format!("---\nname: {name}\ndescription: d\n---\n{body}"),
            )
            .unwrap();
        }
        dir
    }

    #[test]
    fn entries_merge_commands_and_project_skills_commands_win_name_clashes() {
        let dir = project_with(
            &[("ship", "Ship $ARGUMENTS")],
            &[("ship", "skill body"), ("pdf", "pdf body")],
        );
        let catalog = SlashCatalog::load(Some(dir.path()), None);
        let entries = catalog.entries();
        let ship: Vec<_> = entries
            .iter()
            .filter(|e| e.name == "ship" && e.effective)
            .collect();
        assert_eq!(ship.len(), 1);
        assert_eq!(ship[0].kind, SlashKind::Command);
        assert!(entries
            .iter()
            .any(|e| e.name == "pdf" && e.kind == SlashKind::Skill));
    }

    #[test]
    fn global_skills_require_agent_binding() {
        // Project skills always list; a global-origin skill lists only when
        // allowed_skills contains it. Simulated via a project skill (origin
        // Project) vs. the eligibility fn on a Global-origin skill.
        let dir = project_with(&[], &[("local", "body")]);
        let unbound = SlashCatalog::load(Some(dir.path()), None);
        assert!(unbound.entries().iter().any(|e| e.name == "local"));
        // Global filtering is unit-tested through eligible_global():
        assert!(!SlashCatalog::global_skill_listed("triage", None));
        assert!(!SlashCatalog::global_skill_listed(
            "triage",
            Some(&["other".into()])
        ));
        assert!(SlashCatalog::global_skill_listed(
            "triage",
            Some(&["triage".into()])
        ));
    }

    #[test]
    fn resolves_skill_invocation_with_arguments() {
        let dir = project_with(&[], &[("deploy", "Run make deploy.")]);
        let catalog = SlashCatalog::load(Some(dir.path()), None);
        let resolved = catalog.resolve("/deploy to staging").unwrap();
        assert!(resolved.prompt.contains("# Skill: deploy"));
        assert!(resolved.prompt.contains("Run make deploy."));
        assert!(resolved.prompt.contains("Arguments: to staging"));
        assert_eq!(resolved.agent, None);
        assert!(!resolved.subtask);
    }

    #[test]
    fn expands_nested_command_lines_with_args_depth_and_cycles_guarded() {
        let dir = project_with(
            &[
                ("outer", "Start\n/inner run fast\nEnd"),
                ("inner", "Inner does $ARGUMENTS"),
                ("loop-a", "/loop-b"),
                ("loop-b", "/loop-a"),
            ],
            &[],
        );
        let catalog = SlashCatalog::load(Some(dir.path()), None);
        let resolved = catalog.resolve("/outer").unwrap();
        assert!(resolved.prompt.contains("Inner does run fast"));
        assert!(!resolved.prompt.contains("/inner"));
        // Cycles never hang; the revisited token stays literal. Traced:
        // loop-a -> "/loop-b" -> loop-b -> "/loop-a" -> loop-a already
        // visited, so the innermost line stays the literal "/loop-a".
        let looped = catalog.resolve("/loop-a").unwrap();
        assert_eq!(looped.prompt, "/loop-a");
        // Unknown names stay literal.
        let dir2 = project_with(&[("solo", "Line\n/does-not-exist x")], &[]);
        let catalog2 = SlashCatalog::load(Some(dir2.path()), None);
        assert!(catalog2
            .resolve("/solo")
            .unwrap()
            .prompt
            .contains("/does-not-exist x"));
    }

    #[test]
    fn first_agent_mention_finds_known_agents_only() {
        let known = vec!["plan".to_string(), "build-bot".to_string()];
        assert_eq!(
            first_agent_mention("route to @plan please", &known),
            Some("plan")
        );
        assert_eq!(
            first_agent_mention("@build-bot, go", &known),
            Some("build-bot")
        );
        assert_eq!(first_agent_mention("mail @nobody", &known), None);
        assert_eq!(first_agent_mention("no mentions", &known), None);
    }

    #[test]
    fn no_project_load_lists_global_and_builtin_commands() {
        // Builtins are always listed, project or not — Home's client-side
        // filters (surface + requiresProject) are what hide them with no
        // project attached, so the API itself doesn't need to.
        let catalog = SlashCatalog::load(None, None);
        let entries = catalog.entries();
        let builtin = |name: &str| {
            entries
                .iter()
                .find(|e| e.name == name && e.kind == SlashKind::Command)
                .unwrap_or_else(|| panic!("missing builtin command entry: {name}"))
        };
        let init = builtin("init");
        assert_eq!(init.origin, CommandOrigin::Builtin);
        assert!(init.surfaces.home && init.surfaces.session && init.requires_project);
        let review = builtin("review");
        assert_eq!(review.origin, CommandOrigin::Builtin);
        assert!(!review.surfaces.home && review.surfaces.session);
        let compact = builtin("compact");
        assert_eq!(compact.origin, CommandOrigin::Builtin);
        assert!(!compact.surfaces.home && compact.surfaces.session);
        // Every non-builtin command entry has nowhere to come from but the
        // global command directory (no project => no Project origin).
        assert!(entries
            .iter()
            .filter(|e| e.kind == SlashKind::Command && e.origin != CommandOrigin::Builtin)
            .all(|e| e.origin == CommandOrigin::Global));
    }

    /// Hermetic version of the test above: rather than relying on whatever
    /// this machine's real `~/.config/ryuzi/{commands,skills}` happen to
    /// contain, build the catalog directly from its private fields with a
    /// controlled `SkillRegistry` (an empty project dir + an explicit extra
    /// skills root, both `SkillOrigin::Global` per Task 2). Covers both the
    /// "bound global skills" half of the name above (a skill lists once
    /// bound) AND the Issue-1 regression (a bound global skill whose name
    /// collides with a BUILTIN command — present in the full registry
    /// `resolve()` uses, and now also listed as a Command entry — must be
    /// dropped from the SKILL listing, and the builtin must still win at
    /// resolve time).
    #[test]
    fn no_project_load_binds_global_skills_and_drops_ones_colliding_with_builtins() {
        let empty_project = tempfile::tempdir().unwrap();
        let extra_root = tempfile::tempdir().unwrap();
        for name in ["triage", "init"] {
            let dir = extra_root.path().join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("SKILL.md"),
                format!("---\nname: {name}\ndescription: d\n---\nbody"),
            )
            .unwrap();
        }
        let skills =
            SkillRegistry::load_with(empty_project.path(), &[extra_root.path().to_path_buf()]);
        let catalog = SlashCatalog {
            commands: CommandRegistry::load_without_project(),
            skills,
            allowed_skills: Some(vec!["triage".into(), "init".into()]),
        };
        let entries = catalog.entries();
        assert!(entries.iter().any(|e| e.name == "triage"
            && e.kind == SlashKind::Skill
            && e.origin == CommandOrigin::Global));
        // "init" collides with the builtin command: even though it's bound
        // via allowed_skills, it must NOT appear as a Skill entry.
        assert!(!entries
            .iter()
            .any(|e| e.name == "init" && e.kind == SlashKind::Skill));
        // The builtin still resolves and wins — the skill never shadows it.
        let resolved = catalog.resolve("/init").unwrap();
        assert!(resolved.prompt.contains("Analyze this codebase"));
        // "init" now also lists as a builtin Command entry (Issue 1: builtins
        // are no longer hidden without a project) — every other entry still
        // has nowhere to come from but the global command/skill sources.
        assert!(entries.iter().any(|e| e.name == "init"
            && e.kind == SlashKind::Command
            && e.origin == CommandOrigin::Builtin));
        assert!(entries
            .iter()
            .filter(|e| e.origin != CommandOrigin::Builtin)
            .all(|e| e.origin == CommandOrigin::Global));
    }

    // ---------- Task 9: plugin skills gate exactly like global ones ----------

    /// Mirrors [`no_project_load_binds_global_skills_and_drops_ones_colliding_with_builtins`]
    /// but with `SkillOrigin::Plugin` sources (built via
    /// `SkillRegistry::load_with_plugin_roots`) — asserts identical gating:
    /// an unbound plugin skill never lists, a bound one lists (unless a
    /// builtin command owns its name), and origin surfaces as `"plugin"`.
    #[test]
    fn no_project_load_binds_plugin_skills_and_drops_ones_colliding_with_builtins() {
        let empty_project = tempfile::tempdir().unwrap();
        let plugin_root = tempfile::tempdir().unwrap();
        for name in ["triage", "init"] {
            let dir = plugin_root.path().join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("SKILL.md"),
                format!("---\nname: {name}\ndescription: d\n---\nbody"),
            )
            .unwrap();
        }
        let plugin_roots = [("acme".to_string(), plugin_root.path().to_path_buf())];

        // Unbound: neither plugin skill lists, mirroring an unbound global skill.
        let unbound = SlashCatalog {
            commands: CommandRegistry::load_without_project(),
            skills: SkillRegistry::load_with_plugin_roots(empty_project.path(), &plugin_roots),
            allowed_skills: None,
        };
        assert!(!unbound
            .entries()
            .iter()
            .any(|e| e.name == "triage" && e.kind == SlashKind::Skill));

        // Bound: "triage" lists as Plugin-origin; "init" is dropped because a
        // builtin command owns that name, and the builtin still wins resolve.
        let bound = SlashCatalog {
            commands: CommandRegistry::load_without_project(),
            skills: SkillRegistry::load_with_plugin_roots(empty_project.path(), &plugin_roots),
            allowed_skills: Some(vec!["triage".into(), "init".into()]),
        };
        let entries = bound.entries();
        assert!(entries.iter().any(|e| e.name == "triage"
            && e.kind == SlashKind::Skill
            && e.origin == CommandOrigin::Plugin));
        assert_eq!(CommandOrigin::Plugin.as_str(), "plugin");
        assert!(!entries
            .iter()
            .any(|e| e.name == "init" && e.kind == SlashKind::Skill));
        let resolved = bound.resolve("/init").unwrap();
        assert!(resolved.prompt.contains("Analyze this codebase"));
    }

    #[test]
    fn load_with_plugins_threads_command_and_skill_roots_with_lowest_precedence() {
        let dir = project_with(&[], &[]);
        let plugin_root = tempfile::tempdir().unwrap();
        let plugin_cmds = plugin_root.path().join("commands");
        std::fs::create_dir_all(&plugin_cmds).unwrap();
        std::fs::write(
            plugin_cmds.join("sync.md"),
            "---\ndescription: Sync\n---\nSync $ARGUMENTS",
        )
        .unwrap();
        let plugin_skills = plugin_root.path().join("skills/triage");
        std::fs::create_dir_all(&plugin_skills).unwrap();
        std::fs::write(
            plugin_skills.join("SKILL.md"),
            "---\nname: triage\ndescription: Triage\n---\nBody",
        )
        .unwrap();

        let catalog = SlashCatalog::load_with_plugins(
            Some(dir.path()),
            Some(&["triage".to_string()]),
            &[("acme".to_string(), plugin_cmds)],
            &[("acme".to_string(), plugin_root.path().join("skills"))],
        );
        let entries = catalog.entries();
        assert!(entries.iter().any(|e| e.name == "sync"
            && e.kind == SlashKind::Command
            && e.origin == CommandOrigin::Plugin));
        assert!(entries.iter().any(|e| e.name == "triage"
            && e.kind == SlashKind::Skill
            && e.origin == CommandOrigin::Plugin));
        let resolved = catalog.resolve("/sync now").unwrap();
        assert!(resolved.prompt.contains("Sync now"));
    }
}
