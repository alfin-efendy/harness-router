// A component fixture exporting `ryuzi:provider/provider@0.2.0` for the Task 4
// transport-capability-negotiation seam. It exercises the structured
// completion path (`complete_v2`):
//   - `capabilities` declares `tools: true`, so the host's cached
//     `WasmProviderTransport::capabilities()` reports tool support.
//   - `list-models` returns a single static model, mirroring the 0.1.0
//     fixture.
//   - `complete` answers a tools-bearing request with one `echo` tool call
//     (`stop-reason: tool-use`), and a toolless request with plain finished
//     text (`stop-reason: end-turn`), so the host test can assert the tool
//     channel end to end.

wit_bindgen::generate!({
    path: "wit",
    world: "provider-v2-fixture",
    generate_all,
});

use exports::ryuzi::provider0_2_0::provider::{
    CompletionChunk, CompletionRequest, Guest, ModelInfo, ProviderCapabilities, ProviderError,
    StopReason, ToolCall,
};

struct Fixture;

impl Guest for Fixture {
    fn capabilities() -> ProviderCapabilities {
        ProviderCapabilities {
            tools: true,
            parallel_tool_calls: false,
        }
    }

    fn list_models() -> Result<Vec<ModelInfo>, ProviderError> {
        Ok(vec![ModelInfo {
            id: "fixture-model".to_string(),
            display_name: "Fixture Model".to_string(),
            context_window: 8192,
        }])
    }

    /// A request carrying tools always answers with one `echo` call, so the
    /// host test can assert the tool channel end to end; a request with no
    /// tools answers with plain finished text.
    fn complete(request: CompletionRequest) -> Result<Vec<CompletionChunk>, ProviderError> {
        if request.tools.is_empty() {
            return Ok(vec![CompletionChunk {
                text: "no tools".to_string(),
                tool_calls: vec![],
                finished: true,
                stop_reason: Some(StopReason::EndTurn),
                usage: None,
            }]);
        }
        Ok(vec![CompletionChunk {
            text: String::new(),
            tool_calls: vec![ToolCall {
                id: "call_fixture_1".to_string(),
                name: "echo".to_string(),
                arguments: "{}".to_string(),
            }],
            finished: true,
            stop_reason: Some(StopReason::ToolUse),
            usage: None,
        }])
    }
}

export!(Fixture);
