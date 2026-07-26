// Persistence scaffolding for the agent registry: several pub(crate)
// document types and renderers are only consumed by later Plan 2 tasks
// (registry state, disk writer). Until that wiring lands, suppress
// dead-code so the intermediate commits stay clippy-clean.
#![allow(dead_code)]

use std::collections::BTreeMap;

use anyhow::{bail, Context};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_yaml::{Mapping, Value};

use crate::agents::personality::{AgentPersonality, PersonalityPreset};

use super::types::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AgentIndexWire {
    schema_version: u32,
    order: Vec<String>,
    default_agent_id: String,
    #[serde(flatten)]
    extensions: IndexMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AvatarWire {
    color: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pet: Option<String>,
    #[serde(flatten)]
    extensions: IndexMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum AgentModelWire {
    Concrete(ConcreteModelWire),
    Route(RouteModelWire),
}

// `deny_unknown_fields` cannot be combined with `flatten` extension maps in
// serde, so union violations (both arms, or `effort` on a route) are rejected
// up front by `validate_model_union` before deserialization; the required
// `name`/`route` fields then discriminate the untagged arms deterministically.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ConcreteModelWire {
    name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    effort: Option<String>,
    #[serde(flatten)]
    extensions: IndexMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RouteModelWire {
    route: String,
    #[serde(flatten)]
    extensions: IndexMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PermissionRuleWire {
    id: String,
    tool: String,
    decision: PermissionDecision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    command_prefix: Option<String>,
}

/// A native tool's decision on the wire. `NativeToolDecision` covers the
/// current `allow`/`ask`/`off` vocabulary; a bare `off` scalar always lands
/// directly in this arm (this crate's `serde_yaml` follows the YAML 1.2
/// core schema, so unquoted `off`/`no`/`on`/`yes` are plain strings, never
/// booleans — only `true`/`false` resolve as booleans). `Legacy(bool)`
/// exists purely to tolerate a hand-edited literal boolean `false` (as
/// opposed to the string `"false"`), folding it to `Off` so a profile is
/// never bricked by that. `true` has no meaning here (there is no boolean
/// "on" decision) and is rejected at parse time.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum NativeToolDecisionWire {
    Decision(NativeToolDecision),
    Legacy(bool),
}

impl NativeToolDecisionWire {
    fn resolve(self, tool: &str) -> anyhow::Result<NativeToolDecision> {
        match self {
            Self::Decision(decision) => Ok(decision),
            Self::Legacy(false) => Ok(NativeToolDecision::Off),
            Self::Legacy(true) => bail!(
                "native tool `{tool}` cannot be set to `true`; use \"allow\" or remove the entry"
            ),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PermissionsWire {
    #[serde(default)]
    native: BTreeMap<String, NativeToolDecisionWire>,
    #[serde(default)]
    rules: Vec<PermissionRuleWire>,
    #[serde(flatten)]
    extensions: IndexMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SkillsWire {
    #[serde(default)]
    enabled: Vec<String>,
    #[serde(flatten)]
    extensions: IndexMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ToolsWire {
    #[serde(default)]
    plugins: Vec<String>,
    #[serde(default)]
    apps: Vec<String>,
    #[serde(flatten)]
    extensions: IndexMap<String, Value>,
}

fn default_personality_preset() -> PersonalityPreset {
    PersonalityPreset::Helpful
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersonalityWire {
    #[serde(default = "default_personality_preset")]
    preset: PersonalityPreset,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    custom: Option<String>,
    #[serde(flatten)]
    extensions: IndexMap<String, Value>,
}

impl Default for PersonalityWire {
    fn default() -> Self {
        Self {
            preset: default_personality_preset(),
            custom: None,
            extensions: IndexMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AgentProfileWire {
    schema_version: u32,
    id: String,
    name: String,
    description: String,
    avatar: AvatarWire,
    model: AgentModelWire,
    #[serde(default)]
    personality: PersonalityWire,
    permissions: PermissionsWire,
    skills: SkillsWire,
    tools: ToolsWire,
    #[serde(flatten)]
    extensions: IndexMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SubagentConfigWire {
    schema_version: u32,
    model: AgentModelWire,
    #[serde(flatten)]
    extensions: IndexMap<String, Value>,
}

#[derive(Debug, Clone)]
pub(crate) struct AgentProfileDocument {
    typed: AgentProfile,
    raw: Value,
    extensions: IndexMap<String, Value>,
}

impl AgentProfileDocument {
    pub(crate) fn typed(&self) -> &AgentProfile {
        &self.typed
    }

    pub(crate) fn extensions(&self) -> &IndexMap<String, Value> {
        &self.extensions
    }

    pub(crate) fn merge_typed(&mut self, profile: AgentProfile) {
        self.typed = profile;
    }
}

#[derive(Debug, Clone)]
pub(crate) struct AgentIndexDocument {
    typed: AgentIndex,
    raw: Value,
    extensions: IndexMap<String, Value>,
}

impl AgentIndexDocument {
    pub(crate) fn typed(&self) -> &AgentIndex {
        &self.typed
    }

    pub(crate) fn extensions(&self) -> &IndexMap<String, Value> {
        &self.extensions
    }

    pub(crate) fn merge_typed(&mut self, value: AgentIndex) {
        self.typed = value;
    }
}

#[derive(Debug, Clone)]
pub(crate) struct SubagentConfigDocument {
    typed: SubagentConfig,
    raw: Value,
    extensions: IndexMap<String, Value>,
}

impl SubagentConfigDocument {
    pub(crate) fn typed(&self) -> &SubagentConfig {
        &self.typed
    }

    pub(crate) fn extensions(&self) -> &IndexMap<String, Value> {
        &self.extensions
    }

    pub(crate) fn merge_typed(&mut self, value: SubagentConfig) {
        self.typed = value;
    }
}

pub fn parse_agent_index(raw: &str) -> anyhow::Result<AgentIndex> {
    Ok(parse_agent_index_document(raw)?.typed)
}

pub fn render_agent_index(value: &AgentIndex) -> anyhow::Result<String> {
    render_yaml(&index_to_wire(value))
}

pub fn parse_subagent_config(raw: &str) -> anyhow::Result<SubagentConfig> {
    Ok(parse_subagent_config_document(raw)?.typed)
}

pub fn render_subagent_config(value: &SubagentConfig) -> anyhow::Result<String> {
    render_yaml(&SubagentConfigWire {
        schema_version: value.schema_version,
        model: model_to_wire(&value.model, IndexMap::new()),
        extensions: IndexMap::new(),
    })
}

pub fn parse_agent_profile(raw: &str) -> anyhow::Result<AgentProfile> {
    Ok(parse_agent_profile_document(raw)?.typed().clone())
}

pub fn render_agent_profile(value: &AgentProfile) -> anyhow::Result<String> {
    render_yaml(&profile_to_wire(value, &IndexMap::new()))
}

pub(crate) fn parse_agent_profile_document(raw: &str) -> anyhow::Result<AgentProfileDocument> {
    let raw_value: Value = serde_yaml::from_str(raw).context("invalid agent profile YAML")?;
    validate_model_union(&raw_value)?;
    let wire: AgentProfileWire = serde_yaml::from_value(raw_value.clone())?;
    ensure_schema(wire.schema_version)?;
    let (typed, extensions) = profile_from_wire(wire)?;
    Ok(AgentProfileDocument {
        typed,
        raw: raw_value,
        extensions,
    })
}

pub(crate) fn render_agent_profile_document(
    value: &AgentProfileDocument,
) -> anyhow::Result<String> {
    let wire = profile_to_wire(&value.typed, &value.extensions);
    merge_and_render(&value.raw, &wire)
}

pub(crate) fn parse_agent_index_document(raw: &str) -> anyhow::Result<AgentIndexDocument> {
    let raw_value: Value = serde_yaml::from_str(raw).context("invalid agent index YAML")?;
    let wire: AgentIndexWire = serde_yaml::from_value(raw_value.clone())?;
    ensure_schema(wire.schema_version)?;
    let typed = index_from_wire(wire)?;
    Ok(AgentIndexDocument {
        extensions: typed.extensions.clone(),
        typed,
        raw: raw_value,
    })
}

pub(crate) fn render_agent_index_document(value: &AgentIndexDocument) -> anyhow::Result<String> {
    merge_and_render(&value.raw, &index_to_wire(&value.typed))
}

pub(crate) fn parse_subagent_config_document(raw: &str) -> anyhow::Result<SubagentConfigDocument> {
    let raw_value: Value = serde_yaml::from_str(raw).context("invalid subagent YAML")?;
    validate_model_union(&raw_value)?;
    let wire: SubagentConfigWire = serde_yaml::from_value(raw_value.clone())?;
    ensure_schema(wire.schema_version)?;
    let (model, model_extensions) = model_from_wire(wire.model)?;
    let mut extensions = wire.extensions;
    if !model_extensions.is_empty() {
        extensions.insert(
            "model".into(),
            Value::Mapping(map_from_index(model_extensions)),
        );
    }
    Ok(SubagentConfigDocument {
        typed: SubagentConfig {
            schema_version: AGENT_SCHEMA_VERSION,
            model,
        },
        raw: raw_value,
        extensions,
    })
}

pub(crate) fn render_subagent_config_document(
    value: &SubagentConfigDocument,
) -> anyhow::Result<String> {
    let model_extensions = nested_extensions(&value.extensions, "model");
    let wire = SubagentConfigWire {
        schema_version: value.typed.schema_version,
        model: model_to_wire(&value.typed.model, model_extensions),
        extensions: top_extensions(&value.extensions, &["model"]),
    };
    merge_and_render(&value.raw, &wire)
}

fn profile_from_wire(
    wire: AgentProfileWire,
) -> anyhow::Result<(AgentProfile, IndexMap<String, Value>)> {
    let (model, model_extensions) = model_from_wire(wire.model)?;
    let schema_version = wire.schema_version;
    let profile_id = wire.id.trim().to_owned();

    let mut permissions_extensions = wire.permissions.extensions;
    let mut tools_extensions = wire.tools.extensions;

    let rules: Vec<PermissionRule> = wire
        .permissions
        .rules
        .into_iter()
        .map(|rule| PermissionRule {
            id: rule.id.trim().to_owned(),
            tool: rule.tool.trim().to_owned(),
            decision: rule.decision,
            command_prefix: trim_option(rule.command_prefix),
        })
        .collect();
    let native: BTreeMap<String, NativeToolDecision> = wire
        .permissions
        .native
        .into_iter()
        .map(|(tool, decision)| {
            let resolved = decision.resolve(&tool)?;
            Ok((tool, resolved))
        })
        .collect::<anyhow::Result<_>>()?;

    // Schema 1 documents carry the retired `permissions.mode` and
    // `tools.native` keys as unrecognized extensions (the v2 wire structs no
    // longer declare those fields). Fold them into the v2 per-tool decision
    // map once, here, and drop the legacy keys so they do not linger forever
    // as phantom "vendor extensions" on every later render.
    let (native, rules) = if schema_version == 1 {
        let mode = permissions_extensions
            .shift_remove("mode")
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_else(|| "ask".to_owned());
        let old_native_raw = tools_extensions
            .shift_remove("native")
            .and_then(|value| value.as_sequence().cloned())
            .unwrap_or_default();
        let old_native: Vec<String> = old_native_raw
            .iter()
            .filter_map(|value| value.as_str().map(str::to_owned))
            .collect();
        let builtin = crate::harness::native::tools::ToolRegistry::builtin_ids();
        migrate_v1_permissions(&profile_id, &mode, &old_native, rules, &builtin)
    } else {
        (native, rules)
    };

    let mut extensions = wire.extensions;
    add_nested(&mut extensions, "avatar", wire.avatar.extensions);
    add_nested(&mut extensions, "model", model_extensions);
    add_nested(&mut extensions, "personality", wire.personality.extensions);
    add_nested(&mut extensions, "permissions", permissions_extensions);
    add_nested(&mut extensions, "skills", wire.skills.extensions);
    add_nested(&mut extensions, "tools", tools_extensions);

    let personality = AgentPersonality {
        preset: wire.personality.preset,
        custom: trim_option(wire.personality.custom),
    };
    personality.validate()?;

    let profile = AgentProfile {
        schema_version: AGENT_SCHEMA_VERSION,
        id: required(wire.id, "id")?,
        name: required(wire.name, "name")?,
        description: required(wire.description, "description")?,
        avatar: AgentAvatar {
            color: required(wire.avatar.color, "avatar.color")?,
            pet: trim_option(wire.avatar.pet).filter(|value| !value.is_empty()),
        },
        model,
        personality,
        permissions: AgentPermissions { native, rules },
        skills: trim_vec(wire.skills.enabled),
        tools: AgentTools {
            plugins: trim_vec(wire.tools.plugins),
            apps: trim_vec(wire.tools.apps),
        },
    };
    Ok((profile, extensions))
}

/// Upgrades a schema-1 agent's permission table to schema 2's per-tool
/// native decision map.
///
/// - The built-in `ryuzi` id always gets every builtin tool set to `Allow`,
///   overriding whatever the old mode/native list said (mirrors the old
///   bootstrap behavior where Ryuzi's native harness always ran unrestricted).
/// - A non-`ryuzi` profile with an empty native allow-list inherited its
///   effective permissions purely from `mode`: `full` allowed everything,
///   `accept_edits` allowed only the edit-class tools, `ask`/`plan` allowed
///   nothing (both collapse to the same "prompt for everything" behavior, so
///   an absent map entry — which now means `Ask` — is the correct migration
///   target and nothing needs inserting).
/// - A non-empty native allow-list was an explicit table: listed tools keep
///   their mode-derived decision (`Allow` under `full`, `Ask` otherwise —
///   `accept_edits` on a listed non-edit tool degrades to `Ask`, not `Off`,
///   since the old runtime still exposed it, just gated by prompt), and every
///   other builtin tool becomes `Off` since it was never exposed at all.
/// - Whole-tool permission rules (no `command_prefix`) folded into the same
///   base decision table (`deny`→`Off`, `allow`→`Allow`, `ask`→`Ask`) and are
///   dropped. Command-prefix-scoped rules survive as explicit rules, except
///   an `ask`-decision prefix rule: `Ask` was always a runtime no-op (rules
///   only ever resolved to allow/deny), so those are dropped too rather than
///   migrated forward as an unrenderable rule.
fn migrate_v1_permissions(
    profile_id: &str,
    mode: &str,
    old_native: &[String],
    old_rules: Vec<PermissionRule>,
    builtin: &[String],
) -> (BTreeMap<String, NativeToolDecision>, Vec<PermissionRule>) {
    use NativeToolDecision::*;
    // The lowercase policy.rs EDIT_TOOLS ids that actually exist as builtin
    // tool ids in this registry. `multiedit`/`notebookedit` are named in
    // policy.rs's EDIT_TOOLS but have no corresponding builtin tool.
    const EDIT_CLASS: &[&str] = &["edit", "write"];
    let mode_decision = |tool: &str| match mode {
        "full" => Allow,
        "accept_edits" if EDIT_CLASS.contains(&tool) => Allow,
        _ => Ask, // ask, plan, accept_edits non-edit
    };
    let mut map = BTreeMap::new();
    if profile_id == "ryuzi" {
        for id in builtin {
            map.insert(id.clone(), Allow);
        }
    } else if old_native.is_empty() {
        for id in builtin {
            if mode_decision(id) == Allow {
                map.insert(id.clone(), Allow);
            } // Ask stays absent
        }
    } else {
        for id in old_native {
            match mode_decision(id) {
                Allow => {
                    map.insert(id.clone(), Allow);
                }
                _ => {
                    map.insert(id.clone(), Ask);
                }
            }
        }
        for id in builtin {
            if !old_native.contains(id) {
                map.insert(id.clone(), Off);
            }
        }
    }
    let mut kept = Vec::new();
    for rule in old_rules {
        if rule.command_prefix.is_some() {
            // `Ask` was always a runtime no-op (the rules engine only ever
            // resolved a matching rule to allow/deny); carrying it forward
            // just leaves an unrenderable rule for the allow/deny-only
            // Permissions UI. Drop it here instead of migrating it.
            if rule.decision != PermissionDecision::Ask {
                kept.push(rule);
            }
            continue;
        }
        if profile_id == "ryuzi" {
            continue; // all-Allow override wins
        }
        match rule.decision {
            PermissionDecision::Deny => {
                map.insert(rule.tool, Off);
            }
            PermissionDecision::Allow => {
                map.insert(rule.tool, Allow);
            }
            PermissionDecision::Ask => {
                map.insert(rule.tool, Ask);
            }
        }
    }
    (map, kept)
}

fn profile_to_wire(value: &AgentProfile, extensions: &IndexMap<String, Value>) -> AgentProfileWire {
    AgentProfileWire {
        schema_version: value.schema_version,
        id: value.id.clone(),
        name: value.name.clone(),
        description: value.description.clone(),
        avatar: AvatarWire {
            color: value.avatar.color.clone(),
            pet: value.avatar.pet.clone(),
            extensions: nested_extensions(extensions, "avatar"),
        },
        model: model_to_wire(&value.model, nested_extensions(extensions, "model")),
        personality: PersonalityWire {
            preset: value.personality.preset,
            custom: value.personality.custom.clone(),
            extensions: nested_extensions(extensions, "personality"),
        },
        permissions: PermissionsWire {
            native: value
                .permissions
                .native
                .iter()
                .map(|(tool, decision)| (tool.clone(), NativeToolDecisionWire::Decision(*decision)))
                .collect(),
            rules: value
                .permissions
                .rules
                .iter()
                .map(|rule| PermissionRuleWire {
                    id: rule.id.clone(),
                    tool: rule.tool.clone(),
                    decision: rule.decision,
                    command_prefix: rule.command_prefix.clone(),
                })
                .collect(),
            extensions: nested_extensions(extensions, "permissions"),
        },
        skills: SkillsWire {
            enabled: value.skills.clone(),
            extensions: nested_extensions(extensions, "skills"),
        },
        tools: ToolsWire {
            plugins: value.tools.plugins.clone(),
            apps: value.tools.apps.clone(),
            extensions: nested_extensions(extensions, "tools"),
        },
        extensions: top_extensions(
            extensions,
            &[
                "avatar",
                "model",
                "personality",
                "permissions",
                "skills",
                "tools",
            ],
        ),
    }
}

fn model_from_wire(wire: AgentModelWire) -> anyhow::Result<(AgentModel, IndexMap<String, Value>)> {
    match wire {
        AgentModelWire::Concrete(wire) => Ok((
            AgentModel::Concrete {
                name: required(wire.name, "model.name")?,
                effort: trim_option(wire.effort),
            },
            wire.extensions,
        )),
        AgentModelWire::Route(wire) => Ok((
            AgentModel::Route {
                route: required(wire.route, "model.route")?,
            },
            wire.extensions,
        )),
    }
}

fn model_to_wire(value: &AgentModel, extensions: IndexMap<String, Value>) -> AgentModelWire {
    match value {
        AgentModel::Concrete { name, effort } => AgentModelWire::Concrete(ConcreteModelWire {
            name: name.clone(),
            effort: effort.clone(),
            extensions,
        }),
        AgentModel::Route { route } => AgentModelWire::Route(RouteModelWire {
            route: route.clone(),
            extensions,
        }),
    }
}

fn validate_model_union(value: &Value) -> anyhow::Result<()> {
    let model = value
        .as_mapping()
        .and_then(|map| map.get(Value::String("model".into())))
        .and_then(Value::as_mapping)
        .context("agent model must be a mapping")?;
    let has_name = model.contains_key(Value::String("name".into()));
    let has_route = model.contains_key(Value::String("route".into()));
    if has_name == has_route {
        bail!("agent model requires exactly one of 'name' or 'route'");
    }
    if has_route && model.contains_key(Value::String("effort".into())) {
        bail!("agent route model cannot contain 'effort'");
    }
    Ok(())
}

fn index_from_wire(wire: AgentIndexWire) -> anyhow::Result<AgentIndex> {
    Ok(AgentIndex {
        schema_version: AGENT_SCHEMA_VERSION,
        order: trim_vec(wire.order),
        default_agent_id: required(wire.default_agent_id, "default_agent_id")?,
        extensions: wire.extensions,
    })
}

fn index_to_wire(value: &AgentIndex) -> AgentIndexWire {
    AgentIndexWire {
        schema_version: value.schema_version,
        order: value.order.clone(),
        default_agent_id: value.default_agent_id.clone(),
        extensions: value.extensions.clone(),
    }
}

/// Accepts any schema version from 1 up to the current
/// [`AGENT_SCHEMA_VERSION`]. Version 1 profile documents are upgraded in
/// place by [`profile_from_wire`]; index/subagent documents have not changed
/// shape between 1 and 2, so an old-but-still-1 file loads unmodified and is
/// simply written back out at the current version on its next save (both
/// already always render `schema_version: AGENT_SCHEMA_VERSION`).
fn ensure_schema(version: u32) -> anyhow::Result<()> {
    if version == 0 || version > AGENT_SCHEMA_VERSION {
        bail!("unsupported agent schema version {version}");
    }
    Ok(())
}

fn required(value: String, field: &str) -> anyhow::Result<String> {
    let value = value.trim().to_owned();
    if value.is_empty() {
        bail!("{field} cannot be empty");
    }
    Ok(value)
}

fn trim_option(value: Option<String>) -> Option<String> {
    value.map(|value| value.trim().to_owned())
}

fn trim_vec(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .map(|value| value.trim().to_owned())
        .collect()
}

fn add_nested(target: &mut IndexMap<String, Value>, key: &str, values: IndexMap<String, Value>) {
    if !values.is_empty() {
        target.insert(key.into(), Value::Mapping(map_from_index(values)));
    }
}

fn nested_extensions(values: &IndexMap<String, Value>, key: &str) -> IndexMap<String, Value> {
    values
        .get(key)
        .and_then(Value::as_mapping)
        .map(index_from_map)
        .unwrap_or_default()
}

fn top_extensions(values: &IndexMap<String, Value>, nested: &[&str]) -> IndexMap<String, Value> {
    values
        .iter()
        .filter(|(key, _)| !nested.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn map_from_index(values: IndexMap<String, Value>) -> Mapping {
    values
        .into_iter()
        .map(|(key, value)| (Value::String(key), value))
        .collect()
}

fn index_from_map(values: &Mapping) -> IndexMap<String, Value> {
    values
        .iter()
        .filter_map(|(key, value)| Some((key.as_str()?.to_owned(), value.clone())))
        .collect()
}

fn merge_and_render<T: Serialize>(raw: &Value, typed: &T) -> anyhow::Result<String> {
    let mut merged = raw.clone();
    let replacement = serde_yaml::to_value(typed)?;
    remove_stale_model_keys(&mut merged, &replacement);
    remove_stale_personality_keys(&mut merged, &replacement);
    remove_stale_v1_permission_keys(&mut merged);
    merge_value(&mut merged, replacement);
    render_yaml(&merged)
}

/// `permissions.mode` and `tools.native` are retired schema-1 keys with no
/// v2 replacement field (the migration folds them into
/// `permissions.native` once, in [`profile_from_wire`]). `merge_value` only
/// overwrites/inserts keys present in the replacement and never deletes a
/// target-only key, so re-rendering a still schema-1-shaped `raw` document
/// (a profile edited before its first clean re-render) would otherwise carry
/// these two dead keys forward forever as phantom "vendor extensions".
/// Dropping them unconditionally here is safe: neither key is ever a
/// legitimate v2 field.
fn remove_stale_v1_permission_keys(target: &mut Value) {
    let permissions_key = Value::String("permissions".into());
    let mode_key = Value::String("mode".into());
    if let Some(permissions) = target
        .as_mapping_mut()
        .and_then(|mapping| mapping.get_mut(&permissions_key))
        .and_then(Value::as_mapping_mut)
    {
        permissions.remove(&mode_key);
    }
    let tools_key = Value::String("tools".into());
    let native_key = Value::String("native".into());
    if let Some(tools) = target
        .as_mapping_mut()
        .and_then(|mapping| mapping.get_mut(&tools_key))
        .and_then(Value::as_mapping_mut)
    {
        tools.remove(&native_key);
    }
}

fn remove_stale_model_keys(target: &mut Value, replacement: &Value) {
    let model_key = Value::String("model".into());
    let Some(replacement_model) = replacement
        .as_mapping()
        .and_then(|mapping| mapping.get(&model_key))
        .and_then(Value::as_mapping)
    else {
        return;
    };
    let Some(target_model) = target
        .as_mapping_mut()
        .and_then(|mapping| mapping.get_mut(&model_key))
        .and_then(Value::as_mapping_mut)
    else {
        return;
    };

    for key in ["name", "route", "effort"] {
        let key = Value::String(key.into());
        if !replacement_model.contains_key(&key) {
            target_model.remove(&key);
        }
    }
}

fn remove_stale_personality_keys(target: &mut Value, replacement: &Value) {
    let personality_key = Value::String("personality".into());
    let Some(replacement_personality) = replacement
        .as_mapping()
        .and_then(|mapping| mapping.get(&personality_key))
        .and_then(Value::as_mapping)
    else {
        return;
    };
    let Some(target_personality) = target
        .as_mapping_mut()
        .and_then(|mapping| mapping.get_mut(&personality_key))
        .and_then(Value::as_mapping_mut)
    else {
        return;
    };

    let custom_key = Value::String("custom".into());
    if !replacement_personality.contains_key(&custom_key) {
        target_personality.remove(&custom_key);
    }
}

fn merge_value(target: &mut Value, replacement: Value) {
    match (target, replacement) {
        (Value::Mapping(target), Value::Mapping(replacement)) => {
            for (key, value) in replacement {
                match target.get_mut(&key) {
                    Some(existing) => merge_value(existing, value),
                    None => {
                        target.insert(key, value);
                    }
                }
            }
        }
        (target, replacement) => *target = replacement,
    }
}

/// No post-processing of the rendered text happens here on purpose: an
/// earlier version of this function used to rewrite every line ending in
/// `: off` to `: "off"`, on the theory that `serde_yaml`'s YAML 1.1-style
/// resolver would otherwise read an unquoted `off` back as the boolean
/// `false`. That rewrite was both unnecessary and unsafe — unnecessary
/// because this crate's `serde_yaml` (0.9) follows the YAML 1.2 core
/// schema, under which only bareword `true`/`false` resolve as booleans;
/// `off`/`no`/`on`/`yes` always parse back as plain strings (see
/// [`NativeToolDecision::Off`] and `NativeToolDecisionWire`), so a bare
/// `off` scalar in `permissions.native` already round-trips correctly with
/// no quoting at all. It was unsafe because the line-based rewrite matched
/// *any* line in the document ending in `: off`, including interior
/// content lines of a multi-line literal block scalar (e.g. an agent
/// description containing a sentence like "turn logging: off") — silently
/// corrupting user text on every save. Emit exactly what `serde_yaml`
/// produces.
fn render_yaml<T: Serialize>(value: &T) -> anyhow::Result<String> {
    let rendered = serde_yaml::to_string(value)?;
    Ok(format!("{}\n", rendered.trim_end()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_roundtrip_preserves_unknown_fields_and_model_union() {
        let raw = r#"schema_version: 1
id: reviewer
name: Reviewer
description: Reviews code.
avatar: { color: violet, x_icon: owl }
model: { name: anthropic/claude-opus-4-8, effort: high, x_model: keep }
permissions: { mode: ask, rules: [], x_policy: keep }
skills: { enabled: [systematic-debugging] }
tools: { native: [read], plugins: [], apps: [] }
loop: { max_turns: 50, max_tool_rounds: 100 }
x_vendor: { enabled: true }
"#;
        let profile = parse_agent_profile_document(raw).unwrap();
        assert!(matches!(profile.typed().model, AgentModel::Concrete { .. }));
        let reparsed =
            parse_agent_profile_document(&render_agent_profile_document(&profile).unwrap())
                .unwrap();
        assert_eq!(reparsed.extensions()["x_vendor"]["enabled"], true);
        assert_eq!(reparsed.extensions()["avatar"]["x_icon"], "owl");
        assert_eq!(reparsed.extensions()["model"]["x_model"], "keep");
        assert_eq!(reparsed.extensions()["permissions"]["x_policy"], "keep");
    }

    #[test]
    fn avatar_pet_round_trips_when_present() {
        let raw = r#"schema_version: 1
id: reviewer
name: Reviewer
description: Reviews code.
avatar: { color: violet, pet: paperclip }
model: { name: anthropic/claude-opus-4-8, effort: high }
permissions: { mode: ask, rules: [] }
skills: { enabled: [] }
tools: { native: [], plugins: [], apps: [] }
loop: { max_turns: 50, max_tool_rounds: 100 }
"#;
        let parsed = parse_agent_profile_document(raw).unwrap();
        assert_eq!(parsed.typed().avatar.pet.as_deref(), Some("paperclip"));

        let rendered = render_agent_profile_document(&parsed).unwrap();
        assert!(rendered.contains("pet: paperclip"));
        let reparsed = parse_agent_profile_document(&rendered).unwrap();
        assert_eq!(reparsed.typed().avatar.pet.as_deref(), Some("paperclip"));
    }

    #[test]
    fn avatar_pet_absent_parses_to_none_and_stays_absent_on_render() {
        // Old doc without the `pet` key at all — back-compat parse.
        let doc = parse_agent_profile_document(LEGACY_AGENT_YAML).unwrap();
        assert_eq!(doc.typed().avatar.pet, None);
        let rendered = render_agent_profile_document(&doc).unwrap();
        assert!(!rendered.contains("pet:"));
    }

    #[test]
    fn empty_avatar_pet_normalizes_to_none() {
        let raw = r#"schema_version: 1
id: reviewer
name: Reviewer
description: Reviews code.
avatar: { color: violet, pet: "" }
model: { name: anthropic/claude-opus-4-8, effort: high }
permissions: { mode: ask, rules: [] }
skills: { enabled: [] }
tools: { native: [], plugins: [], apps: [] }
loop: { max_turns: 50, max_tool_rounds: 100 }
"#;
        let parsed = parse_agent_profile_document(raw).unwrap();
        assert_eq!(parsed.typed().avatar.pet, None);
    }

    const LEGACY_AGENT_YAML: &str = r#"schema_version: 1
id: reviewer
name: Reviewer
description: Reviews code.
avatar: { color: violet }
model: { name: anthropic/claude-opus-4-8, effort: high }
permissions: { mode: ask, rules: [] }
skills: { enabled: [] }
tools: { native: [], plugins: [], apps: [] }
loop: { max_turns: 50, max_tool_rounds: 100 }
"#;

    const CUSTOM_PERSONALITY_YAML: &str = r#"schema_version: 1
id: reviewer
name: Reviewer
description: Reviews code.
avatar: { color: violet }
model: { name: anthropic/claude-opus-4-8, effort: high }
personality: { preset: custom, custom: "Speak like a strict code reviewer.\nBe terse and cite line numbers.", x-user-extension: keep }
permissions: { mode: ask, rules: [] }
skills: { enabled: [] }
tools: { native: [], plugins: [], apps: [] }
loop: { max_turns: 50, max_tool_rounds: 100 }
"#;

    #[test]
    fn missing_legacy_personality_defaults_to_helpful() {
        let doc = parse_agent_profile_document(LEGACY_AGENT_YAML).unwrap();
        assert_eq!(doc.typed().personality.preset, PersonalityPreset::Helpful);
        assert_eq!(doc.typed().personality.custom, None);
    }

    #[test]
    fn custom_personality_round_trips_and_extensions_survive() {
        let parsed = parse_agent_profile_document(CUSTOM_PERSONALITY_YAML).unwrap();
        assert_eq!(parsed.typed().personality.preset, PersonalityPreset::Custom);
        assert_eq!(
            parsed.typed().personality.custom.as_deref(),
            Some("Speak like a strict code reviewer.\nBe terse and cite line numbers.")
        );
        let rendered = render_agent_profile_document(&parsed).unwrap();
        assert!(rendered.contains("preset: custom"));
        assert!(rendered.contains("custom: |-"));
        assert!(rendered.contains("x-user-extension:"));

        let reparsed = parse_agent_profile_document(&rendered).unwrap();
        assert_eq!(reparsed.typed().personality, parsed.typed().personality);
        assert_eq!(
            reparsed.extensions()["personality"]["x-user-extension"],
            "keep"
        );
    }

    #[test]
    fn invalid_personality_combination_is_rejected() {
        let raw = r#"schema_version: 1
id: reviewer
name: Reviewer
description: Reviews code.
avatar: { color: violet }
model: { name: anthropic/claude-opus-4-8, effort: high }
personality: { preset: custom }
permissions: { mode: ask, rules: [] }
skills: { enabled: [] }
tools: { native: [], plugins: [], apps: [] }
loop: { max_turns: 50, max_tool_rounds: 100 }
"#;
        assert!(parse_agent_profile_document(raw).is_err());

        let raw = r#"schema_version: 1
id: reviewer
name: Reviewer
description: Reviews code.
avatar: { color: violet }
model: { name: anthropic/claude-opus-4-8, effort: high }
personality: { preset: technical, custom: "extra text" }
permissions: { mode: ask, rules: [] }
skills: { enabled: [] }
tools: { native: [], plugins: [], apps: [] }
loop: { max_turns: 50, max_tool_rounds: 100 }
"#;
        assert!(parse_agent_profile_document(raw).is_err());
    }

    #[test]
    fn route_model_rejects_effort_and_both_union_arms() {
        for raw in [
            "schema_version: 1\nmodel: { route: free, effort: high }\n",
            "schema_version: 1\nmodel: { route: free, name: openai/gpt-5 }\n",
        ] {
            assert!(parse_subagent_config(raw).is_err(), "accepted {raw}");
        }
    }

    #[test]
    fn merge_typed_model_arm_switch_renders_reparseable_yaml() {
        let raw = r#"schema_version: 1
id: reviewer
name: Reviewer
description: Reviews code.
avatar: { color: violet }
model: { name: anthropic/claude-opus-4-8, effort: high, x_model: keep }
permissions: { mode: ask, rules: [] }
skills: { enabled: [] }
tools: { native: [], plugins: [], apps: [] }
loop: { max_turns: 50, max_tool_rounds: 100 }
"#;
        let mut doc = parse_agent_profile_document(raw).unwrap();
        let mut typed = doc.typed().clone();
        typed.model = AgentModel::Route {
            route: "free".into(),
        };
        doc.merge_typed(typed);
        let rendered = render_agent_profile_document(&doc).unwrap();
        let reparsed = parse_agent_profile_document(&rendered).unwrap();
        assert_eq!(
            reparsed.typed().model,
            AgentModel::Route {
                route: "free".into()
            }
        );
        assert_eq!(reparsed.extensions()["model"]["x_model"], "keep");

        // Dropping an explicit effort inside the concrete arm must also
        // remove the stale `effort` key from the rendered YAML.
        let mut doc = parse_agent_profile_document(raw).unwrap();
        let mut typed = doc.typed().clone();
        typed.model = AgentModel::Concrete {
            name: "anthropic/claude-opus-4-8".into(),
            effort: None,
        };
        doc.merge_typed(typed);
        let rendered = render_agent_profile_document(&doc).unwrap();
        assert!(!rendered.contains("effort"), "stale effort in:\n{rendered}");
        let reparsed = parse_agent_profile_document(&rendered).unwrap();
        assert_eq!(
            reparsed.typed().model,
            AgentModel::Concrete {
                name: "anthropic/claude-opus-4-8".into(),
                effort: None,
            }
        );
    }

    #[test]
    fn merge_typed_personality_preset_switch_renders_reparseable_yaml() {
        let mut doc = parse_agent_profile_document(CUSTOM_PERSONALITY_YAML).unwrap();
        let mut typed = doc.typed().clone();
        typed.personality = AgentPersonality {
            preset: PersonalityPreset::Technical,
            custom: None,
        };
        doc.merge_typed(typed);
        let rendered = render_agent_profile_document(&doc).unwrap();
        assert!(
            !rendered.contains("custom:"),
            "stale custom key in:\n{rendered}"
        );

        let reparsed = parse_agent_profile_document(&rendered).unwrap();
        assert_eq!(
            reparsed.typed().personality,
            AgentPersonality {
                preset: PersonalityPreset::Technical,
                custom: None,
            }
        );
    }

    #[test]
    fn merge_typed_subagent_route_to_concrete_renders_reparseable_yaml() {
        let mut doc =
            parse_subagent_config_document("schema_version: 1\nmodel: { route: free }\n").unwrap();
        let mut typed = doc.typed().clone();
        typed.model = AgentModel::Concrete {
            name: "anthropic/claude-opus-4-8".into(),
            effort: Some("high".into()),
        };
        doc.merge_typed(typed);
        let rendered = render_subagent_config_document(&doc).unwrap();
        let reparsed = parse_subagent_config(&rendered).unwrap();
        assert_eq!(
            reparsed.model,
            AgentModel::Concrete {
                name: "anthropic/claude-opus-4-8".into(),
                effort: Some("high".into()),
            }
        );
    }

    #[test]
    fn index_roundtrip_keeps_order_default_and_extensions() {
        let raw = "schema_version: 1\norder: [b, a]\ndefault_agent_id: b\nx_sync: manual\n";
        let index = parse_agent_index(raw).unwrap();
        assert_eq!(index.order, vec!["b", "a"]);
        assert_eq!(index.default_agent_id, "b");
        assert_eq!(
            parse_agent_index(&render_agent_index(&index).unwrap())
                .unwrap()
                .extensions["x_sync"],
            "manual"
        );
    }

    // --- v1 -> v2 permission migration -------------------------------------
    //
    // Shape copied from the pre-change `render_agent_profile` output (see
    // `LEGACY_AGENT_YAML` above): `permissions: { mode, rules }` and
    // `tools: { native, plugins, apps }`, with `skills: { enabled: [] }`
    // (the brief's draft used a bare `skills: []`, which does not match the
    // real `SkillsWire` mapping shape and was corrected here).
    fn v1_doc(mode: &str, native: &[&str], rules_yaml: &str) -> String {
        format!(
            "schema_version: 1\nid: tester\nname: Tester\ndescription: d\navatar:\n  color: blue\nmodel:\n  route: free\npersonality:\n  preset: helpful\npermissions:\n  mode: {mode}\n{rules_yaml}skills:\n  enabled: []\ntools:\n  native: [{}]\n  plugins: []\n  apps: []\n",
            native.join(", ")
        )
    }

    #[test]
    fn v1_full_empty_native_migrates_to_all_allow() {
        let profile = parse_agent_profile_document(&v1_doc("full", &[], "")).unwrap();
        assert_eq!(profile.typed().schema_version, 2);
        for id in crate::harness::native::tools::ToolRegistry::builtin_ids() {
            assert_eq!(
                profile.typed().permissions.native_decision(&id),
                NativeToolDecision::Allow
            );
        }
    }

    #[test]
    fn v1_ask_empty_native_migrates_to_empty_map() {
        // absent = Ask
        let profile = parse_agent_profile_document(&v1_doc("ask", &[], "")).unwrap();
        assert!(profile.typed().permissions.native.is_empty());
    }

    #[test]
    fn v1_accept_edits_empty_native_allows_edit_class_only() {
        let profile = parse_agent_profile_document(&v1_doc("accept_edits", &[], "")).unwrap();
        assert_eq!(
            profile.typed().permissions.native_decision("edit"),
            NativeToolDecision::Allow
        );
        assert_eq!(
            profile.typed().permissions.native_decision("write"),
            NativeToolDecision::Allow
        );
        assert_eq!(
            profile.typed().permissions.native_decision("bash"),
            NativeToolDecision::Ask
        );
    }

    #[test]
    fn v1_plan_migrates_like_ask() {
        let profile = parse_agent_profile_document(&v1_doc("plan", &[], "")).unwrap();
        assert!(profile.typed().permissions.native.is_empty());
    }

    #[test]
    fn v1_nonempty_native_list_maps_listed_and_offs_unlisted() {
        let profile = parse_agent_profile_document(&v1_doc("full", &["read", "bash"], "")).unwrap();
        assert_eq!(
            profile.typed().permissions.native_decision("read"),
            NativeToolDecision::Allow
        );
        assert_eq!(
            profile.typed().permissions.native_decision("bash"),
            NativeToolDecision::Allow
        );
        // every OTHER builtin id must be explicitly Off
        assert_eq!(
            profile.typed().permissions.native_decision("write"),
            NativeToolDecision::Off
        );
    }

    #[test]
    fn v1_whole_tool_rules_fold_into_base_decision() {
        let rules = "  rules:\n    - id: r1\n      tool: bash\n      decision: allow\n    - id: r2\n      tool: read\n      decision: deny\n    - id: r3\n      tool: bash\n      decision: allow\n      command_prefix: \"git \"\n    - id: r4\n      tool: bash\n      decision: ask\n      command_prefix: \"npm \"\n";
        let profile = parse_agent_profile_document(&v1_doc("ask", &[], rules)).unwrap();
        assert_eq!(
            profile.typed().permissions.native_decision("bash"),
            NativeToolDecision::Allow
        ); // allow rule folds
        assert_eq!(
            profile.typed().permissions.native_decision("read"),
            NativeToolDecision::Off
        ); // deny rule -> Off
        assert_eq!(
            profile.typed().permissions.rules.len(),
            1,
            "only the allow-decision prefix rule survives; the ask-decision prefix rule (r4) is dropped as a runtime no-op"
        );
        // `profile_from_wire` trims every parsed `command_prefix` (pre-existing
        // behavior, unrelated to this migration: `trim_option(rule.command_prefix)`
        // already ran on schema-1 profiles before this task). The brief's
        // draft literal was `Some("git ")` with a trailing space; adjusted to
        // the real trimmed value.
        assert_eq!(
            profile.typed().permissions.rules[0]
                .command_prefix
                .as_deref(),
            Some("git")
        );
    }

    #[test]
    fn v1_ryuzi_id_overrides_table_to_all_allow() {
        let doc = v1_doc("ask", &["read"], "").replace("id: tester", "id: ryuzi");
        let profile = parse_agent_profile_document(&doc).unwrap();
        for id in crate::harness::native::tools::ToolRegistry::builtin_ids() {
            assert_eq!(
                profile.typed().permissions.native_decision(&id),
                NativeToolDecision::Allow
            );
        }
    }

    #[test]
    fn v2_round_trips_and_quotes_off() {
        let mut profile = crate::agents::bootstrap::default_ryuzi_profile("t".into());
        profile
            .permissions
            .native
            .insert("write".into(), NativeToolDecision::Off);
        let rendered = render_agent_profile(&profile).unwrap();
        // The `off` scalar is never quoted (serde_yaml 0.9 follows the YAML
        // 1.2 core schema, where only `true`/`false` bareword scalars
        // resolve as booleans — `off` always round-trips as a plain
        // string). The requirement is that it parses back to `Off`, not
        // that it carries any particular quote style.
        let back = parse_agent_profile(&rendered).unwrap();
        assert_eq!(back.permissions, profile.permissions);
    }

    #[test]
    fn description_content_line_ending_in_colon_off_is_not_corrupted() {
        // Regression test: a multi-line description forces serde_yaml to
        // emit it as a literal block scalar. A content line that happens to
        // end in ": off" (e.g. a sentence about a logging toggle) must
        // survive render -> parse byte-for-byte; it must never be rewritten
        // into `...: "off"` the way a real `permissions.native` decision is.
        let mut profile = crate::agents::bootstrap::default_ryuzi_profile("t".into());
        profile.description =
            "Handles alerts.\nRuntime toggle for turn logging: off\nAlso handles greetings."
                .to_string();
        let rendered = render_agent_profile(&profile).unwrap();
        assert!(
            rendered.contains("Runtime toggle for turn logging: off"),
            "description content line must survive unquoted: {rendered}"
        );
        assert!(
            !rendered.contains("Runtime toggle for turn logging: \"off\""),
            "description content must not be quoted like a permission decision: {rendered}"
        );
        let back = parse_agent_profile(&rendered).unwrap();
        assert_eq!(back.description, profile.description);
    }

    #[test]
    fn bool_false_parses_as_off() {
        // A rendered `off` is already a bare, unquoted plain-string scalar
        // (see `render_yaml`'s doc comment), so replacing quote characters
        // around it is a no-op. To actually exercise the `Legacy(bool)`
        // hand-edit tolerance path (someone types a literal `false` instead
        // of `off`/`"off"`), rewrite the rendered scalar to `false` — that's
        // the only spelling `NativeToolDecisionWire::Legacy` ever sees.
        // Verified this discriminates: temporarily deleting the `Legacy`
        // variant (and its `resolve` arm) makes this test fail to compile /
        // fail to parse, since `false` no longer deserializes into
        // `NativeToolDecisionWire` at all.
        let mut profile = crate::agents::bootstrap::default_ryuzi_profile("t".into());
        profile
            .permissions
            .native
            .insert("write".into(), NativeToolDecision::Off);
        let rendered = render_agent_profile(&profile)
            .unwrap()
            .replace("write: \"off\"", "write: false")
            .replace("write: 'off'", "write: false")
            .replace("write: off", "write: false");
        assert!(
            rendered.contains("write: false"),
            "replace must have matched the rendered `write` line: {rendered}"
        );
        let back = parse_agent_profile(&rendered).unwrap();
        assert_eq!(
            back.permissions.native_decision("write"),
            NativeToolDecision::Off
        );
    }

    #[test]
    fn stale_v1_permission_keys_do_not_survive_a_merge_typed_render() {
        // A profile edited (via merge_typed + render_agent_profile_document)
        // before it has ever been cleanly re-rendered still has a schema-1
        // `raw` tree. The stale `permissions.mode`/`tools.native` keys must
        // not leak into the merged output as phantom extensions.
        let raw = v1_doc("full", &[], "");
        let mut doc = parse_agent_profile_document(&raw).unwrap();
        let typed = doc.typed().clone();
        doc.merge_typed(typed);
        let rendered = render_agent_profile_document(&doc).unwrap();
        // A plain `contains("mode:")` false-positives on the legitimate
        // builtin tool id `exitplanmode` (`    exitplanmode: allow`); anchor
        // on the 2-space `permissions:`-child indentation the retired
        // top-level `mode:` key would have rendered at.
        assert!(
            !rendered.contains("\n  mode:"),
            "stale mode key in:\n{rendered}"
        );
        let reparsed = parse_agent_profile_document(&rendered).unwrap();
        assert!(
            !reparsed.extensions().contains_key("permissions")
                || reparsed.extensions()["permissions"].get("mode").is_none()
        );
    }
}
