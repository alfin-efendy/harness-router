//! Bridge one enabled WASM component into the native runtime's MCP tool
//! registry — the merge point of Task 6: component connector tools stop
//! being a bespoke `wasm__<id>__<tool>` path and become ordinary
//! `mcp__<id>__<tool>` [`McpTool`](crate::harness::native::tools::mcp::McpTool)s,
//! governed by the exact same permission model as every other MCP server.
//!
//! [`ComponentMcpServer`] is an in-process [`McpCaller`]: no stdio, no JSON-RPC
//! framing — `call` goes straight to the component's
//! `ryuzi:connector/connector` export via [`WasmActivation::connector_invoke`].
//! The `mcp__<server>__<tool>` wire name itself is assembled by the shared
//! [`McpTool::new`](crate::harness::native::tools::mcp::McpTool::new)
//! constructor at the call site (`harness::native::mod`'s session-tool
//! assembly), keyed off [`ComponentMcpServer::server_id`] — this module never
//! formats that name itself.

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::domain::Principal;
use crate::harness::native::mcp_client::{McpCaller, McpToolDef};
use crate::plugins::wasm_connector::{parse_tool_def, WasmActivation};

/// One enabled WASM component exposed as an in-process MCP server: no
/// protocol serialization, `McpCaller` straight over the component's
/// `ryuzi:connector/connector` export. Tool names become
/// `mcp__<component-id>__<tool>` via the shared `McpTool` wrapper.
pub struct ComponentMcpServer {
    activation: Arc<WasmActivation>,
    pub server_id: String,
    pub tools: Vec<McpToolDef>,
    pub principal: Principal,
}

impl ComponentMcpServer {
    /// List the component's tools and wrap it. `None` when the component
    /// exports no connector (a hooks/provider/gateway-only plugin — never
    /// instantiated to find that out, mirroring the old `WasmToolSet`'s
    /// IMP-2 guard), when enumerating those tools fails (a component trap,
    /// timeout, or declared `connector-error` — logged and treated as
    /// "contributes no tools", never a session-start failure), or when every
    /// listed definition is malformed/duplicate and none survive.
    pub async fn discover(activation: Arc<WasmActivation>) -> Option<ComponentMcpServer> {
        if !activation.exports_connector() {
            return None;
        }
        let defs = match activation.connector_list_tools().await {
            Ok(defs) => defs,
            Err(reason) => {
                tracing::warn!(
                    component = %activation.component_id(),
                    "skipping component connector tools: {reason}"
                );
                return None;
            }
        };
        // A component could declare the same tool name twice in its own
        // `list-tools`; drop a duplicate deterministically (first wins)
        // rather than silently shadowing it later in the tool registry,
        // mirroring the old `WasmToolSet::session_tools`'s own dedup.
        let mut seen: HashSet<String> = HashSet::new();
        let mut tools = Vec::new();
        for raw in &defs {
            let Some(def) = parse_tool_def(raw) else {
                tracing::warn!(
                    component = %activation.component_id(),
                    "skipping malformed connector tool definition (missing/blank name)"
                );
                continue;
            };
            if !seen.insert(def.name.clone()) {
                tracing::warn!(
                    component = %activation.component_id(),
                    tool = %def.name,
                    "skipping duplicate connector tool: name already claimed"
                );
                continue;
            }
            tools.push(McpToolDef {
                name: def.name,
                description: def.description,
                input_schema: def.input_schema,
            });
        }
        if tools.is_empty() {
            return None;
        }
        let server_id = activation.component_id().to_string();
        let principal = activation.principal.clone();
        Some(ComponentMcpServer {
            activation,
            server_id,
            tools,
            principal,
        })
    }
}

#[async_trait]
impl McpCaller for ComponentMcpServer {
    async fn call(&self, tool: &str, arguments: Value) -> anyhow::Result<Value> {
        let value = self.activation.connector_invoke(tool, arguments).await?;
        // Mirror the old `WasmTool::execute`'s rendering exactly: a plain
        // string result is raw text, anything else is compact JSON — then
        // wrap it into the MCP `{"content":[{"type":"text","text":...}]}`
        // shape `render_tool_result` (mcp_client.rs) expects, so a component
        // result renders identically to a real MCP server's.
        let text = match value {
            Value::String(text) => text,
            other => serde_json::to_string(&other).unwrap_or_default(),
        };
        Ok(json!({ "content": [{ "type": "text", "text": text }] }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::build_fixture_components_once as build_fixtures;
    use crate::plugins::bundle::InstalledBundle;
    use crate::plugins::capabilities::PluginCapabilityContext;
    use crate::plugins::runtime::{ComponentRuntime, HostPolicy};
    use crate::settings::SettingsStore;
    use crate::store::ComponentPluginReleaseRecord;
    use crate::telemetry::NoopTelemetry;
    use ryuzi_plugin_sdk::{
        ComponentSpec, PluginLifecycle, PluginManifest, PluginPermissions, PluginRelease,
    };
    use std::path::PathBuf;

    fn connector_artifact() -> PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/component-connector/target/wasm32-wasip2/release")
            .join("ryuzi_component_connector_fixture.wasm")
    }

    fn noop_artifact() -> PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/component-noop/target/wasm32-wasip2/release")
            .join("ryuzi_component_noop_fixture.wasm")
    }

    /// Build one `WasmActivation` from a prebuilt fixture artifact, mirroring
    /// `wasm_connector::tests::build_activation` (private to that module, so
    /// duplicated here rather than exposed cross-module for a test-only need).
    async fn build_activation(
        component_path: PathBuf,
        plugin_id: &str,
    ) -> (Arc<WasmActivation>, tempfile::NamedTempFile) {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = Arc::new(crate::store::Store::open(tmp.path()).await.unwrap());
        let ctx = Arc::new(PluginCapabilityContext {
            plugin_id: plugin_id.to_string(),
            version: "0.1.0".to_string(),
            settings: SettingsStore::new(store.clone()),
            store,
            telemetry: Arc::new(NoopTelemetry),
            network_allowlist: vec![],
            oauth_profile_ids: vec![],
            provider_ids: vec![],
        });
        let bundle = InstalledBundle {
            manifest: PluginManifest {
                contract: ryuzi_plugin_sdk::CONTRACT_VERSION,
                id: plugin_id.to_string(),
                name: plugin_id.to_string(),
                version: "0.1.0".to_string(),
                publisher: String::new(),
                description: String::new(),
                homepage: None,
                icon: None,
                categories: vec![],
                slot: None,
                verified: false,
                experimental: false,
                auth: None,
                settings: vec![],
                component: Some(ComponentSpec {
                    file: "plugin.wasm".to_string(),
                    wit_api: "^0.1.0".to_string(),
                    lifecycle: PluginLifecycle::Singleton,
                }),
                permissions: PluginPermissions { network: vec![] },
                oauth: vec![],
                provider: None,
                tools: vec![],
                mcp: vec![],
                hooks: vec![],
                jobs: vec![],
                gateway: false,
            },
            release: PluginRelease {
                id: plugin_id.to_string(),
                version: "0.1.0".to_string(),
                wit_api: "0.1.0".to_string(),
                component_url: "https://example.invalid/x.wasm".to_string(),
                component_sha256: "0".repeat(64),
                size_bytes: None,
                published_at: None,
            },
            release_record: ComponentPluginReleaseRecord {
                plugin_id: plugin_id.to_string(),
                version: "0.1.0".to_string(),
                source_url: "https://example.invalid/x.wasm".to_string(),
                sha256: "0".repeat(64),
                signing_key_id: "test".to_string(),
                installed_at: 0,
                active: true,
                revoked: false,
                revocation_reason: None,
            },
            root: component_path.parent().unwrap().to_path_buf(),
            component_path,
        };
        let runtime = ComponentRuntime::new().unwrap();
        let compiled = Arc::new(runtime.compile(&bundle, HostPolicy::deny_all()).unwrap());
        let activation = Arc::new(WasmActivation::new(
            compiled,
            ctx,
            plugin_id.to_string(),
            Principal {
                plugin_id: plugin_id.to_string(),
                plugin_name: plugin_id.to_string(),
            },
        ));
        (activation, tmp)
    }

    async fn connector_fixture_activation() -> Arc<WasmActivation> {
        build_fixtures();
        build_activation(connector_artifact(), "acme-tools").await.0
    }

    async fn noop_fixture_activation() -> Arc<WasmActivation> {
        build_fixtures();
        build_activation(noop_artifact(), "acme-noop").await.0
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn discover_lists_tools_and_call_invokes() {
        let activation = connector_fixture_activation().await;
        let server = ComponentMcpServer::discover(activation)
            .await
            .expect("connector fixture exports tools");
        assert!(!server.tools.is_empty());
        assert_eq!(server.server_id, "acme-tools");
        let echo = server
            .tools
            .iter()
            .find(|t| t.name == "echo")
            .expect("connector fixture exports an echo tool");
        let result = server
            .call(&echo.name, json!({ "message": "hi" }))
            .await
            .unwrap();
        assert!(result.is_object());
        assert_eq!(result["content"][0]["type"], "text");
        assert_eq!(result["content"][0]["text"], "hi");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn non_connector_component_yields_none() {
        let activation = noop_fixture_activation().await;
        assert!(
            !activation.exports_connector(),
            "sanity: noop fixture exports no connector"
        );
        assert!(ComponentMcpServer::discover(activation).await.is_none());
    }
}
