//! Bridge a WASM component's `ryuzi:connector/connector` export (in-process
//! tools) into the native session's tool registry.
//!
//! # Why this is NOT the Rust `Connector` trait
//! The Rust [`crate::connector::Connector`] trait only yields
//! [`McpServerSpec`](crate::domain::McpServerSpec)s — pointers to *external*
//! subprocess/HTTP MCP servers — and structurally cannot represent a tool
//! whose implementation runs IN-PROCESS. The WIT `connector` interface
//! (`list-tools` + `invoke`) is exactly such an in-process tool surface, so
//! this module owns just that in-process activation: instantiate-per-call,
//! `list-tools`/`invoke` dispatch, and WIT<->JSON conversion.
//!
//! [`crate::plugins::mcp_component::ComponentMcpServer`] is the seam that
//! turns one [`WasmActivation`] into an in-process MCP server (Task 6): it
//! enumerates `connector.list-tools` via [`WasmActivation::connector_list_tools`],
//! validates each definition with [`parse_tool_def`], and dispatches calls
//! through [`WasmActivation::connector_invoke`] — the SAME registry
//! `mcp__<server>__<tool>` tools from an external stdio server flow through,
//! via `harness::native::tools::mcp::McpTool`.
//!
//! Every value is validated before registering it (a missing/blank tool name
//! skips that tool with a warning, never a crash — an installed component is
//! untrusted code) and a component trap/timeout during `list-tools`/`invoke`
//! is caught and turned into a skipped tool / tool error, never a daemon
//! crash.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::domain::Principal;
use crate::plugins::capabilities::wit_bindings::exports::ryuzi::connector::connector as wit;
use crate::plugins::capabilities::PluginCapabilityContext;
use crate::plugins::runtime::{CompiledComponent, ComponentInstance, PluginRuntimeError};

/// One enabled component bundle, compiled once and ready to instantiate a
/// fresh, isolated instance per operation. Shared by the connector adapter
/// (this module) and the hook adapter ([`crate::plugins::wasm_hooks`]); both
/// re-instantiate per call so concurrent sessions never share mutable Wasm
/// state.
pub struct WasmActivation {
    compiled: Arc<CompiledComponent>,
    ctx: Arc<PluginCapabilityContext>,
    component_id: String,
    pub(crate) principal: Principal,
}

impl WasmActivation {
    /// Build an activation for one enabled bundle. `compiled` is the validated
    /// component (see [`crate::plugins::runtime::ComponentRuntime::compile`]);
    /// `ctx` carries the shared settings/store/telemetry backends; `principal`
    /// attributes this plugin's tool calls in approval prompts.
    pub fn new(
        compiled: Arc<CompiledComponent>,
        ctx: Arc<PluginCapabilityContext>,
        component_id: String,
        principal: Principal,
    ) -> Self {
        WasmActivation {
            compiled,
            ctx,
            component_id,
            principal,
        }
    }

    pub(crate) fn component_id(&self) -> &str {
        &self.component_id
    }

    /// Whether this component exports `ryuzi:connector/connector` (IMP-2).
    pub(crate) fn exports_connector(&self) -> bool {
        self.compiled.exports_connector()
    }

    /// Instantiate a fresh, isolated instance of this bundle's component,
    /// running `start` under the fuel/epoch budget.
    pub(crate) async fn instantiate(&self) -> Result<ComponentInstance, PluginRuntimeError> {
        self.compiled.instantiate(self.ctx.clone()).await
    }

    /// Enumerate this component's connector tool definitions. A component with
    /// no connector export (e.g. a hooks-only plugin) surfaces as an `Err`,
    /// which the caller treats as "contributes no tools".
    pub(crate) async fn connector_list_tools(&self) -> Result<Vec<wit::ToolDefinition>, String> {
        let mut instance = self.instantiate().await.map_err(|e| e.to_string())?;
        let result = instance
            .call(|inst, store| {
                let pre = inst.instance_pre(&*store);
                let guest = wit::GuestIndices::new(&pre)?.load(&mut *store, &inst)?;
                guest.call_list_tools(&mut *store)
            })
            .await
            .map_err(|e| e.to_string())?;
        result.map_err(|connector_error| describe_connector_error(&connector_error))
    }

    /// Invoke one connector tool by its bare WIT name with the harness's JSON
    /// `input`, returning the converted output value. Any guest
    /// `connector-error`, or any host-side trap/timeout, becomes an `Err` (the
    /// caller renders it as a tool error) — never a panic or a hung daemon.
    pub(crate) async fn connector_invoke(
        &self,
        tool_name: &str,
        input: Value,
    ) -> anyhow::Result<Value> {
        let call = wit::ToolCall {
            call_id: crate::paths::new_id(),
            name: tool_name.to_string(),
            arguments: json_object_to_arguments(input),
        };
        let mut instance = self
            .instantiate()
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let result = instance
            .call(move |inst, store| {
                let pre = inst.instance_pre(&*store);
                let guest = wit::GuestIndices::new(&pre)?.load(&mut *store, &inst)?;
                guest.call_invoke(&mut *store, &call)
            })
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        match result {
            Ok(tool_result) => Ok(tool_result_to_value(tool_result.values)),
            Err(connector_error) => {
                Err(anyhow::anyhow!(describe_connector_error(&connector_error)))
            }
        }
    }
}

/// A typed, validated connector tool definition, with the JSON input schema
/// synthesized from the WIT `tool-parameter` list.
#[derive(Debug, Clone, PartialEq)]
pub struct WasmToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// Validate one WIT tool definition into a [`WasmToolDef`]. Only a non-empty
/// (trimmed) `name` is required — the one field a native `Tool` cannot
/// function without (both naming and `invoke` dispatch key off it). A blank
/// name skips the whole def (`None`); everything else is coerced into a valid,
/// if permissive, shape. The JSON input schema is synthesized from the
/// parameter list (see [`synthesize_input_schema`]).
pub(crate) fn parse_tool_def(def: &wit::ToolDefinition) -> Option<WasmToolDef> {
    let name = def.name.trim();
    if name.is_empty() {
        return None;
    }
    Some(WasmToolDef {
        name: name.to_string(),
        description: def.description.clone(),
        input_schema: synthesize_input_schema(&def.parameters),
    })
}

/// Build a JSON-Schema object from the WIT `tool-parameter` list: each named
/// parameter becomes a property whose type is mapped from its `value-type`
/// string; parameters flagged `required` populate the schema's `required`
/// array. A parameter with a blank name is skipped (it cannot be addressed).
pub(crate) fn synthesize_input_schema(parameters: &[wit::ToolParameter]) -> Value {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    for parameter in parameters {
        let name = parameter.name.trim();
        if name.is_empty() {
            continue;
        }
        properties.insert(
            name.to_string(),
            parameter_type_schema(&parameter.value_type),
        );
        if parameter.required {
            required.push(Value::String(name.to_string()));
        }
    }
    let mut schema = serde_json::Map::new();
    schema.insert("type".to_string(), Value::String("object".to_string()));
    schema.insert("properties".to_string(), Value::Object(properties));
    if !required.is_empty() {
        schema.insert("required".to_string(), Value::Array(required));
    }
    Value::Object(schema)
}

/// Map a WIT `tool-parameter.value-type` string to a JSON-Schema type. An
/// unrecognized type yields a permissive `{}` (no type constraint) rather than
/// guessing — a thin schema is merely imprecise, never unsafe.
fn parameter_type_schema(value_type: &str) -> Value {
    match value_type.trim().to_ascii_lowercase().as_str() {
        "string" | "text" => json!({ "type": "string" }),
        "integer" | "int" | "s64" | "i64" => json!({ "type": "integer" }),
        "number" | "decimal" | "float" | "f64" | "double" => json!({ "type": "number" }),
        "boolean" | "bool" => json!({ "type": "boolean" }),
        _ => json!({}),
    }
}

/// Convert the harness's JSON tool input (expected to be a JSON object) into
/// the WIT `tool-argument` list `invoke` expects. A non-object input
/// contributes no arguments.
pub(crate) fn json_object_to_arguments(input: Value) -> Vec<wit::ToolArgument> {
    match input {
        Value::Object(map) => map
            .into_iter()
            .map(|(name, value)| wit::ToolArgument {
                name,
                value: json_to_tool_value(value),
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Convert a JSON value to the WIT `tool-value` variant. The connector ABI has
/// only four scalar variants, so a null/array/object is preserved losslessly
/// as its JSON `text` rather than dropped.
fn json_to_tool_value(value: Value) -> wit::ToolValue {
    match value {
        Value::String(s) => wit::ToolValue::Text(s),
        Value::Bool(b) => wit::ToolValue::Boolean(b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                wit::ToolValue::Integer(i)
            } else if let Some(f) = n.as_f64() {
                wit::ToolValue::Decimal(f)
            } else {
                wit::ToolValue::Text(n.to_string())
            }
        }
        other => wit::ToolValue::Text(other.to_string()),
    }
}

/// Convert a WIT `tool-result`'s value list into a single JSON value: no
/// values → `null`, one value → that value, many → an array.
pub(crate) fn tool_result_to_value(values: Vec<wit::ToolValue>) -> Value {
    let mut converted: Vec<Value> = values.into_iter().map(tool_value_to_json).collect();
    match converted.len() {
        0 => Value::Null,
        1 => converted.pop().unwrap(),
        _ => Value::Array(converted),
    }
}

/// Convert a WIT `tool-value` back to JSON. A non-finite decimal (which JSON
/// cannot represent) degrades to its string form rather than being dropped.
fn tool_value_to_json(value: wit::ToolValue) -> Value {
    match value {
        wit::ToolValue::Text(s) => Value::String(s),
        wit::ToolValue::Integer(i) => Value::from(i),
        wit::ToolValue::Boolean(b) => Value::Bool(b),
        wit::ToolValue::Decimal(f) => serde_json::Number::from_f64(f)
            .map(Value::Number)
            .unwrap_or_else(|| Value::String(f.to_string())),
    }
}

/// A human-readable, secret-free rendering of a WIT `connector-error`.
fn describe_connector_error(error: &wit::ConnectorError) -> String {
    match error {
        wit::ConnectorError::NotFound => "connector tool not found".to_string(),
        wit::ConnectorError::InvalidCall(message) => format!("invalid connector call: {message}"),
        wit::ConnectorError::Unavailable => "connector unavailable".to_string(),
        wit::ConnectorError::Failed(message) => format!("connector failed: {message}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- pure conversion helpers (no Wasm) ----------

    #[test]
    fn synthesize_input_schema_maps_types_and_required() {
        let params = vec![
            wit::ToolParameter {
                name: "message".to_string(),
                value_type: "string".to_string(),
                required: true,
            },
            wit::ToolParameter {
                name: "count".to_string(),
                value_type: "integer".to_string(),
                required: false,
            },
        ];
        assert_eq!(
            synthesize_input_schema(&params),
            json!({
                "type": "object",
                "properties": {
                    "message": { "type": "string" },
                    "count": { "type": "integer" }
                },
                "required": ["message"]
            })
        );
    }

    #[test]
    fn synthesize_input_schema_omits_required_when_none_and_skips_blank_names() {
        let params = vec![
            wit::ToolParameter {
                name: "  ".to_string(),
                value_type: "string".to_string(),
                required: true,
            },
            wit::ToolParameter {
                name: "flag".to_string(),
                value_type: "boolean".to_string(),
                required: false,
            },
        ];
        assert_eq!(
            synthesize_input_schema(&params),
            json!({ "type": "object", "properties": { "flag": { "type": "boolean" } } })
        );
    }

    #[test]
    fn unknown_value_type_is_permissive() {
        assert_eq!(parameter_type_schema("mystery"), json!({}));
        assert_eq!(
            parameter_type_schema("DECIMAL"),
            json!({ "type": "number" })
        );
    }

    #[test]
    fn parse_tool_def_rejects_a_blank_name() {
        let def = wit::ToolDefinition {
            name: "   ".to_string(),
            description: "x".to_string(),
            parameters: vec![],
        };
        assert!(parse_tool_def(&def).is_none());
    }

    #[test]
    fn json_arguments_round_trip_scalar_types() {
        let mut args = json_object_to_arguments(json!({
            "s": "hi",
            "i": 7,
            "f": 1.5,
            "b": true
        }));
        args.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(args.len(), 4);
        assert!(
            matches!(&args[3], wit::ToolArgument { name, value: wit::ToolValue::Text(t) } if name == "s" && t == "hi")
        );
        assert!(
            matches!(&args[2], wit::ToolArgument { name, value: wit::ToolValue::Integer(7) } if name == "i")
        );
        assert!(
            matches!(&args[0], wit::ToolArgument { name, value: wit::ToolValue::Boolean(true) } if name == "b")
        );
    }

    #[test]
    fn tool_result_collapses_by_arity() {
        assert_eq!(tool_result_to_value(vec![]), Value::Null);
        assert_eq!(
            tool_result_to_value(vec![wit::ToolValue::Text("only".to_string())]),
            json!("only")
        );
        assert_eq!(
            tool_result_to_value(vec![
                wit::ToolValue::Integer(1),
                wit::ToolValue::Boolean(false)
            ]),
            json!([1, false])
        );
    }

    // ---------- fixture-backed integration (real connector component) ----------

    use crate::plugins::bundle::InstalledBundle;
    use crate::plugins::runtime::{ComponentRuntime, HostPolicy};
    use crate::settings::SettingsStore;
    use crate::store::ComponentPluginReleaseRecord;
    use crate::telemetry::NoopTelemetry;
    use ryuzi_plugin_sdk::{
        ComponentSpec, PluginLifecycle, PluginManifest, PluginPermissions, PluginRelease,
    };
    use std::path::PathBuf;
    use std::time::Duration;

    use crate::plugins::build_fixture_components_once as build_fixtures;

    fn connector_artifact() -> PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/component-connector/target/wasm32-wasip2/release")
            .join("ryuzi_component_connector_fixture.wasm")
    }

    fn hooks_artifact() -> PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/component-hooks/target/wasm32-wasip2/release")
            .join("ryuzi_component_hooks_fixture.wasm")
    }

    /// Build one `WasmActivation` from a prebuilt fixture artifact under an
    /// arbitrary policy + plugin id.
    async fn build_activation(
        component_path: PathBuf,
        plugin_id: &str,
        policy: HostPolicy,
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
        let compiled = Arc::new(runtime.compile(&bundle, policy).unwrap());
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

    async fn test_activation(timeout: Duration) -> (Arc<WasmActivation>, tempfile::NamedTempFile) {
        let mut policy = HostPolicy::deny_all();
        policy.limits.timeout = timeout;
        build_activation(connector_artifact(), "acme-tools", policy).await
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn connector_list_tools_enumerates_and_parse_tool_def_synthesizes_schema() {
        build_fixtures();
        let (activation, _tmp) = test_activation(Duration::from_secs(10)).await;
        let defs = activation.connector_list_tools().await.unwrap();
        let mut parsed: Vec<WasmToolDef> = defs.iter().filter_map(parse_tool_def).collect();
        parsed.sort_by(|a, b| a.name.cmp(&b.name));
        let names: Vec<&str> = parsed.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["echo", "explode", "slow"]);

        let echo = parsed.iter().find(|d| d.name == "echo").unwrap();
        assert_eq!(
            echo.input_schema,
            json!({
                "type": "object",
                "properties": { "message": { "type": "string" } },
                "required": ["message"]
            })
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn invoke_echoes_the_message_argument() {
        build_fixtures();
        let (activation, _tmp) = test_activation(Duration::from_secs(10)).await;
        let output = activation
            .connector_invoke("echo", json!({ "message": "hello wasm" }))
            .await
            .expect("echo must succeed");
        assert_eq!(output, json!("hello wasm"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn invoke_surfaces_a_connector_error_without_crashing() {
        build_fixtures();
        let (activation, _tmp) = test_activation(Duration::from_secs(10)).await;
        let error = activation
            .connector_invoke("explode", json!({}))
            .await
            .expect_err("explode must return a connector error");
        assert!(
            error.to_string().contains("intentional connector failure"),
            "unexpected error: {error}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn invoke_isolates_a_nonterminating_tool_via_timeout() {
        build_fixtures();
        let (activation, _tmp) = test_activation(Duration::from_millis(200)).await;
        let started = std::time::Instant::now();
        let error = activation
            .connector_invoke("slow", json!({}))
            .await
            .expect_err("a looping invoke must be caught, not hang the host");
        let elapsed = started.elapsed();
        assert!(
            error.to_string().contains("timeout") || error.to_string().contains("budget"),
            "expected a timeout error, got: {error}"
        );
        assert!(
            elapsed < Duration::from_secs(5),
            "timeout must fire promptly: {elapsed:?}"
        );
    }

    // ---------- IMP-2: a hooks-only component exports no connector ----------
    //
    // The full "never even instantiated" guard now lives one layer up, in
    // `plugins::mcp_component::ComponentMcpServer::discover` (see its
    // `non_connector_component_yields_none` test) — it gates on this exact
    // `exports_connector()` check before ever calling `connector_list_tools`.

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn exports_connector_is_false_for_a_hooks_only_component() {
        build_fixtures();
        let (hooks, _h) =
            build_activation(hooks_artifact(), "acme-hooks", HostPolicy::deny_all()).await;
        assert!(
            !hooks.exports_connector(),
            "a hooks-only component must not export the connector interface"
        );
    }
}
