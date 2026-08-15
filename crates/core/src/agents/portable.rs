//! The portable agent bundle: one UTF-8 JSON envelope carrying a single
//! agent's profile YAML plus its per-agent OKF knowledge, so an agent can be
//! moved between installs.
//!
//! Everything in this module is pure and synchronous — no filesystem, no
//! registry, no `ApiState`, and deliberately **no network**. The bundle is
//! credential-free by construction: `AgentProfile` carries no secret fields,
//! and nothing here reaches into connections, tokens, or SQLite.

/// The bundle envelope version. A reader rejects anything else on purpose —
/// that rejection message is a user-visible compatibility contract.
pub const AGENT_BUNDLE_VERSION: u32 = 1;

/// One OKF Markdown document carried by a bundle. `relative_path` is always
/// bundle-relative to the agent's knowledge root and always uses `/`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct KnowledgeFile {
    pub relative_path: String,
    pub markdown: String,
}

/// One exported agent: the profile YAML exactly as it lives at
/// `agents/<id>/agent.yaml`, plus its exportable knowledge documents.
///
/// Deliberately absent, and not to be added: credentials or tokens of any
/// kind, `agents/index.yaml`, `agents/subagents.yaml`, session history, run
/// rows, stats, the learning queue, and the internal `.knowledge-transactions`
/// / `.learning-events` directories.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AgentBundle {
    pub bundle_version: u32,
    pub schema_version: u32,
    pub exported_at: String,
    pub source_agent_id: String,
    pub source_agent_name: String,
    pub profile_yaml: String,
    #[serde(default)]
    pub knowledge: Vec<KnowledgeFile>,
}

/// Parses a bundle, rejecting an envelope version this build cannot read.
pub fn parse_bundle(data: &str) -> anyhow::Result<AgentBundle> {
    let bundle: AgentBundle = serde_json::from_str(data)?;
    if bundle.bundle_version != AGENT_BUNDLE_VERSION {
        anyhow::bail!(
            "unsupported agent bundle version {} (this build reads version {AGENT_BUNDLE_VERSION})",
            bundle.bundle_version
        );
    }
    Ok(bundle)
}

/// Renders a bundle as the pretty JSON the user saves to disk.
pub fn render_bundle(bundle: &AgentBundle) -> anyhow::Result<String> {
    Ok(serde_json::to_string_pretty(bundle)?)
}

/// Reference/environment issues are tolerated on import: the agent is
/// committed and shows up flagged for repair. Everything else is
/// structural and rejects the import.
///
/// Note the deliberate asymmetry — `permissions.rules` (exact) is the
/// catalog-level "this rule targets a tool you do not have" issue and is
/// tolerated, while `permissions.rules[0].id` / `permissions.rules[0].tool`
/// are blank/duplicate-id defects in the bundle itself and are structural.
pub fn issue_is_tolerable_on_import(field: &str) -> bool {
    field.starts_with("model.")
        || field == "skills"
        || field.starts_with("skills[")
        || field == "permissions.native"
        || field.starts_with("tools.")
        || field == "permissions.rules"
}

/// The suggested save-dialog file name for an agent's bundle.
pub fn bundle_file_name(agent_name: &str) -> String {
    let mut slug = String::new();
    for character in agent_name.to_ascii_lowercase().chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character);
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches('-');
    let slug = if slug.len() > 48 { &slug[..48] } else { slug };
    let slug = slug.trim_end_matches('-');
    let slug = if slug.is_empty() { "agent" } else { slug };
    format!("{slug}.ryuzi-agent.json")
}

/// True for a knowledge path under the per-project memory tree. Those
/// directories are keyed by a machine-local project id that will not exist on
/// the importing machine, so they are never exported and never imported.
pub fn is_project_memory_path(relative_path: &str) -> bool {
    relative_path.starts_with(&format!("{}/", crate::agents::okf::PROJECT_MEMORY_PARENT))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> AgentBundle {
        AgentBundle {
            bundle_version: AGENT_BUNDLE_VERSION,
            schema_version: 2,
            exported_at: "2026-08-15T10:00:00Z".into(),
            source_agent_id: "agent-01j".into(),
            source_agent_name: "Reviewer".into(),
            profile_yaml: "schema_version: 2\nid: agent-01j\nname: Reviewer\n".into(),
            knowledge: vec![KnowledgeFile {
                relative_path: "memory/global/store.md".into(),
                markdown: "---\ntype: memory\n---\nbody\n".into(),
            }],
        }
    }

    #[test]
    fn render_then_parse_round_trips_a_bundle_with_knowledge() {
        let bundle = fixture();
        let rendered = render_bundle(&bundle).unwrap();
        assert!(rendered.contains('\n'), "bundles are pretty-printed");
        assert_eq!(parse_bundle(&rendered).unwrap(), bundle);
    }

    #[test]
    fn parse_bundle_rejects_a_future_envelope_version() {
        let mut bundle = fixture();
        bundle.bundle_version = 2;
        let raw = serde_json::to_string(&bundle).unwrap();
        let error = parse_bundle(&raw).unwrap_err();
        assert!(
            format!("{error:#}").contains("unsupported agent bundle version"),
            "{error:#}"
        );
    }

    #[test]
    fn parse_bundle_rejects_non_json() {
        assert!(parse_bundle("not json").is_err());
    }

    #[test]
    fn reference_and_environment_issues_are_tolerable() {
        for field in [
            "model.name",
            "model.route",
            "model.effort",
            "skills",
            "skills[0]",
            "permissions.native",
            "tools.plugins",
            "tools.plugins[0]",
            "tools.apps",
            "permissions.rules",
        ] {
            assert!(issue_is_tolerable_on_import(field), "{field}");
        }
    }

    #[test]
    fn structural_issues_are_not_tolerable() {
        for field in [
            "id",
            "name",
            "description",
            "avatar.color",
            "schema_version",
            "profile",
            "permissions.rules[0].id",
            "permissions.rules[0].tool",
            "index.default_agent_id",
        ] {
            assert!(!issue_is_tolerable_on_import(field), "{field}");
        }
    }

    #[test]
    fn bundle_file_name_slugs_and_falls_back() {
        assert_eq!(
            bundle_file_name("Code Reviewer"),
            "code-reviewer.ryuzi-agent.json"
        );
        assert_eq!(bundle_file_name("  ***  "), "agent.ryuzi-agent.json");
        assert_eq!(
            bundle_file_name("Ryuzi / Ops #2"),
            "ryuzi-ops-2.ryuzi-agent.json"
        );
        assert_eq!(
            bundle_file_name(&"x".repeat(80)),
            format!("{}.ryuzi-agent.json", "x".repeat(48))
        );
    }

    #[test]
    fn project_memory_paths_are_detected_without_prefix_near_misses() {
        assert!(is_project_memory_path("memory/projects/p1/x.md"));
        assert!(!is_project_memory_path("memory/global/x.md"));
        assert!(!is_project_memory_path("memory/projectsish/x.md"));
    }
}
