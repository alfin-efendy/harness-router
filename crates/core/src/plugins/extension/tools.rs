//! Tool provision (Track D, DT6): an extension that declared `provides_tools`
//! returns tool defs at `extension/initialize` (opaque `Value`s, captured by
//! `ExtensionProc::tools`/`ExtensionSnapshot::tools`); this module gives them
//! a type ([`ExtensionToolDef`]) and gathers every currently-provided tool,
//! across every plugin, into an [`ExtensionToolBinding`] — everything
//! `harness::native::tools::extension::ExtensionTool` needs to wrap one as a
//! native `Tool` (naming, description/schema, the owning plugin's
//! [`Principal`] for approval attribution, and a caller to actually dispatch
//! `tool/call` — see `proc::ExtensionCaller`).
//!
//! [`ExtensionTools::session_tools`] is [`super::events::ExtensionEvents::dispatch`]'s
//! sibling accessor: `SessionCtx` threads a SECOND
//! `Option<Arc<dyn ExtensionTools>>` (`extension_tools`) alongside
//! `extension_events`, both resolved from the same daemon-global
//! [`super::ExtensionHost`] at session start
//! (`ControlPlane::start_harness_session`) — `None` in the common case (no
//! extensions spawned) and in every bare test `SessionCtx`, so a session with
//! no extensions pays zero extra cost building its tool registry, exactly
//! like every hook fire site already pays zero extra cost dispatching events.
//!
//! An extension that is not `Running`, or is running but never declared
//! `provides_tools`, or whose declared tool list is empty, contributes
//! NOTHING — see [`ExtensionTools::session_tools`]'s filtering. A malformed
//! tool def (missing/blank `name`) is skipped with a warning, never a crash —
//! an extension is untrusted, arbitrary vendor code (see `plugins::extension`'s
//! module doc), so its declared tool list gets the same "must not crash the
//! host" treatment DT3-DT5 already give every other extension response.

use async_trait::async_trait;
use serde_json::Value;

use crate::domain::Principal;

use super::proc::{ExtensionCaller, ExtensionHost};
use super::ExtensionStatus;

/// A typed extension-declared tool definition, parsed from
/// `extension/initialize`'s raw `tools` array by [`parse_tool_def`].
#[derive(Debug, Clone, PartialEq)]
pub struct ExtensionToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// Parse one raw tool def. Only `name` is required (non-empty after
/// trimming) — the one field a native `Tool` cannot function without, since
/// both wire naming (`ext__<extension>__<name>`) and the `tool/call` dispatch
/// itself key off it. `description` defaults to `""`; `input_schema` defaults
/// to a bare permissive `{"type":"object"}` when absent or not a JSON object
/// — an extension is untrusted input, but a thin/missing schema is merely
/// imprecise, not unsafe, so it does not disqualify the whole def the way a
/// missing name does. Returns `None` (never panics) for anything that isn't
/// even a JSON object, or whose `name` is missing, blank, or not a string —
/// `serde_json::Value::get` returns `None` for a non-object/array `raw`
/// (e.g. a bare string or number), so this falls through safely rather than
/// panicking on a malformed entry.
pub(crate) fn parse_tool_def(raw: &Value) -> Option<ExtensionToolDef> {
    let name = raw.get("name").and_then(Value::as_str)?.trim();
    if name.is_empty() {
        return None;
    }
    let description = raw
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let input_schema = raw
        .get("inputSchema")
        .or_else(|| raw.get("input_schema"))
        .filter(|v| v.is_object())
        .cloned()
        .unwrap_or_else(|| serde_json::json!({ "type": "object" }));
    Some(ExtensionToolDef {
        name: name.to_string(),
        description,
        input_schema,
    })
}

/// The wire tool name `harness::native::tools::extension::ExtensionTool`
/// exposes to the model — `ext__<extension>__<tool>`. Single source of truth
/// shared by [`ExtensionTools::session_tools`]'s collision dedup (below) and
/// `ExtensionTool::from_binding`'s own naming, so the two can never drift.
pub(crate) fn full_tool_name(extension_name: &str, tool_name: &str) -> String {
    format!("ext__{extension_name}__{tool_name}")
}

/// Everything `harness::native::tools::extension::ExtensionTool` needs to
/// wrap one extension-provided tool: the typed def, the owning extension's
/// name (for `ext__<extension>__<tool>` naming — kept separate from
/// `principal.plugin_id`/`.plugin_name`, since one plugin may declare more
/// than one `[[extension]]`), the owning plugin's [`Principal`] (resolved
/// once at spawn time from the `CorePlugin` binding — never string-parsed
/// from a name), and a caller to actually dispatch `tool/call`.
pub struct ExtensionToolBinding {
    pub def: ExtensionToolDef,
    pub extension_name: String,
    pub principal: Principal,
    pub(crate) caller: std::sync::Arc<dyn ExtensionCaller>,
}

/// Gather every currently-provided extension tool, across every plugin — the
/// `ExtensionEvents`-sibling accessor `harness::native`'s session-start tool
/// gathering (`connect_extension_tools`, mirroring `connect_mcp_tools`) calls
/// through `SessionCtx.extension_tools`. Implemented by [`ExtensionHost`];
/// `None`/no host, and a host with nothing spawned, are both true no-ops —
/// see this module's doc.
#[async_trait]
pub trait ExtensionTools: Send + Sync {
    async fn session_tools(&self) -> Vec<ExtensionToolBinding>;
}

#[async_trait]
impl ExtensionTools for ExtensionHost {
    async fn session_tools(&self) -> Vec<ExtensionToolBinding> {
        // `ExtensionSpec::name` is only unique WITHIN one plugin's own
        // manifest, not globally (see its own doc) — two different plugins
        // can each declare an `[[extension]]`/tool pair that formats to the
        // identical `ext__<extension>__<tool>` full name. Left unguarded,
        // `harness::native::tools::ToolRegistry::with_extra`'s plain
        // `BTreeMap::insert` would let the later one silently shadow the
        // earlier with no log, and — because `tool_provision_entries` used
        // to walk raw `HashMap` iteration order — WHICH one "later" meant
        // was randomly reseeded every process start.
        // `tool_provision_entries` now returns entries pre-sorted by
        // `(plugin_id, extension name)`, so iterating it in order here and
        // tracking already-emitted full names is enough to make the winner
        // of a collision deterministic and stable across restarts: the
        // first entry (by that sort) to claim a full name always wins,
        // mirroring `ControlPlane::attach_plugin_mcp_servers`'s own
        // first-registration-wins `HashSet` discipline for MCP server names.
        let mut seen_full_names: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        let mut out = Vec::new();
        for entry in self.tool_provision_entries().await {
            if !entry.provides_tools || !matches!(entry.status, ExtensionStatus::Running) {
                continue;
            }
            for raw in &entry.tools {
                match parse_tool_def(raw) {
                    Some(def) => {
                        let full_name = full_tool_name(&entry.name, &def.name);
                        if !seen_full_names.insert(full_name.clone()) {
                            tracing::warn!(
                                full_name = %full_name,
                                extension = %entry.name,
                                plugin = %entry.principal.plugin_id,
                                "skipping extension tool: full name already claimed by an earlier plugin's extension"
                            );
                            continue;
                        }
                        out.push(ExtensionToolBinding {
                            def,
                            extension_name: entry.name.clone(),
                            principal: entry.principal.clone(),
                            caller: entry.caller.clone(),
                        });
                    }
                    None => {
                        // `raw` is extension-controlled; cap it so a large or
                        // sensitive-looking tool def can't flood or leak into
                        // the daemon log wholesale. `chars().take` keeps the
                        // cut on a UTF-8 boundary (String::truncate would panic).
                        let preview: String = raw.to_string().chars().take(200).collect();
                        tracing::warn!(
                            extension = %entry.name,
                            tool_def_preview = %preview,
                            "skipping malformed tool def from extension/initialize"
                        );
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ---------- parse_tool_def (pure, no I/O) ----------

    #[test]
    fn parse_tool_def_accepts_a_full_valid_def() {
        let raw = json!({
            "name": "lint",
            "description": "Lint a file",
            "inputSchema": { "type": "object", "properties": {} }
        });
        let def = parse_tool_def(&raw).expect("a well-formed def must parse");
        assert_eq!(def.name, "lint");
        assert_eq!(def.description, "Lint a file");
        assert_eq!(
            def.input_schema,
            json!({ "type": "object", "properties": {} })
        );
    }

    #[test]
    fn parse_tool_def_defaults_missing_description_and_schema() {
        let raw = json!({ "name": "lint" });
        let def = parse_tool_def(&raw).unwrap();
        assert_eq!(def.description, "");
        assert_eq!(def.input_schema, json!({ "type": "object" }));
    }

    #[test]
    fn parse_tool_def_accepts_snake_case_input_schema_key() {
        let raw = json!({ "name": "lint", "input_schema": { "type": "object", "extra": true } });
        let def = parse_tool_def(&raw).unwrap();
        assert_eq!(def.input_schema, json!({ "type": "object", "extra": true }));
    }

    #[test]
    fn parse_tool_def_rejects_a_missing_name() {
        let raw = json!({ "description": "no name here" });
        assert!(parse_tool_def(&raw).is_none());
    }

    #[test]
    fn parse_tool_def_rejects_a_blank_name() {
        let raw = json!({ "name": "   " });
        assert!(parse_tool_def(&raw).is_none());
    }

    #[test]
    fn parse_tool_def_rejects_a_non_object_entry_without_panicking() {
        assert!(parse_tool_def(&json!("just a string")).is_none());
        assert!(parse_tool_def(&json!(42)).is_none());
        assert!(parse_tool_def(&json!(null)).is_none());
        assert!(parse_tool_def(&json!(["array", "entry"])).is_none());
    }

    #[test]
    fn parse_tool_def_ignores_a_non_object_input_schema() {
        let raw = json!({ "name": "lint", "inputSchema": "not an object" });
        let def = parse_tool_def(&raw).unwrap();
        assert_eq!(def.input_schema, json!({ "type": "object" }));
    }

    // NOTE: the former "ExtensionTools::session_tools (real sh-based fake
    // extensions)" and "ExtensionCaller dispatch (tool/call round trip)"
    // sections were deleted here: every test in both relied on
    // `ExtensionHost::spawn_all` discovering an `ExtensionFactory` via a
    // `CorePlugin.extension`-driven `extension_only(...)` fixture to get a
    // real fake extension running before exercising `session_tools`/`caller`.
    // `CorePlugin.extension` no longer exists (the v2 SDK manifest has no
    // `[[extension]]` surface), `spawn_all` is now a permanent no-op, and no
    // plugin can ever be discovered this way — that whole integration is
    // categorically impossible pending Task 3's full deletion of Track D
    // subprocess extensions. `session_tools_is_empty_when_nothing_was_ever_spawned`
    // below still holds (and needs no fixture): a host with nothing spawned
    // is trivially empty.

    #[tokio::test]
    async fn session_tools_is_empty_when_nothing_was_ever_spawned() {
        let ext_host = ExtensionHost::new();
        assert!(ext_host.session_tools().await.is_empty());
    }
}
