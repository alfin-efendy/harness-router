use std::collections::BTreeMap;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_yaml::Value;

use crate::agents::personality::AgentPersonality;

pub type AgentId = String;
pub const AGENT_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq)]
pub struct AgentIndex {
    pub schema_version: u32,
    pub order: Vec<AgentId>,
    pub default_agent_id: AgentId,
    pub extensions: IndexMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentModel {
    Concrete {
        name: String,
        effort: Option<String>,
    },
    Route {
        route: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentAvatar {
    pub color: String,
    /// Bundled (`apps/cockpit/public/pets/<slug>`) or downloaded
    /// (`state_dir()/pets/<slug>`) pet slug shown alongside the avatar
    /// color; `None` when no pet is configured. Free-form — no catalog
    /// check against the petdex manifest happens on write.
    pub pet: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionDecision {
    Allow,
    Deny,
    Ask,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeToolDecision {
    Allow,
    Ask,
    Off,
}

impl NativeToolDecision {
    pub fn enabled(self) -> bool {
        !matches!(self, Self::Off)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionRule {
    pub id: String,
    pub tool: String,
    pub decision: PermissionDecision,
    pub command_prefix: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentPermissions {
    /// Per-tool native decision, keyed by the tool's registry id. An absent
    /// entry means [`NativeToolDecision::Ask`].
    pub native: BTreeMap<String, NativeToolDecision>,
    pub rules: Vec<PermissionRule>,
}

impl AgentPermissions {
    pub fn native_decision(&self, tool: &str) -> NativeToolDecision {
        self.native
            .get(tool)
            .copied()
            .unwrap_or(NativeToolDecision::Ask)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTools {
    pub plugins: Vec<String>,
    pub apps: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentProfile {
    pub schema_version: u32,
    pub id: AgentId,
    pub name: String,
    pub description: String,
    pub avatar: AgentAvatar,
    pub model: AgentModel,
    pub personality: AgentPersonality,
    pub permissions: AgentPermissions,
    pub skills: Vec<String>,
    pub tools: AgentTools,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentConfig {
    pub schema_version: u32,
    pub model: AgentModel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentValidationIssue {
    pub field: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentSnapshot {
    pub profile: AgentProfile,
    pub executable: bool,
    pub validation: Vec<AgentValidationIssue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRecoveryNotice {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentRegistrySnapshot {
    pub agents: Vec<AgentSnapshot>,
    pub default_agent_id: AgentId,
    pub recovery: Vec<AgentRecoveryNotice>,
    pub subagent_model: AgentModel,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentMutationInput {
    pub name: String,
    pub description: String,
    pub avatar: AgentAvatar,
    pub model: AgentModel,
    pub personality: AgentPersonality,
    pub permissions: AgentPermissions,
    pub skills: Vec<String>,
    pub tools: AgentTools,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RegistryDiskImage {
    pub index_yaml: String,
    pub subagents_yaml: String,
    pub agents: IndexMap<AgentId, String>,
    pub deleted_agent_ids: Vec<AgentId>,
}
