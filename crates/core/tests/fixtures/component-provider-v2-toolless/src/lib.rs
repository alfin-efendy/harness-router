// A component fixture exporting ONLY `ryuzi:provider/provider@0.2.0` while
// honestly reporting `tools: false` — the exact shape the review's Critical
// finding is about: `mimo`'s free-tier component exports only 0.2.0 (no
// 0.1.0 fallback at all) and its `capabilities()` reports no proven tool
// support. Before the fix, the router keyed its ABI choice on
// `capabilities().tools` instead of the export, so a component shaped like
// this one took the flat 0.1.0 `complete` path — which it does not export —
// and every turn failed outright. This fixture exists so a real compiled
// component (not just the `FakeWasmProvider` test double) proves the fixed
// dispatch: `complete_v2` must succeed and return a plain, tool-call-free
// completion, and `complete` must fail because the export genuinely isn't
// there.
//
//   - `capabilities` declares `tools: false`.
//   - `list-models` returns a single static model, mirroring the tool-capable
//     v2 fixture.
//   - `complete` always answers with plain finished text and NO tool calls,
//     regardless of whether the request carries any `tools` (a real toolless
//     component has nothing to call them with) — proving a completion
//     through `complete_v2` succeeds and carries no tool calls even when
//     driven with a tools-bearing request.

wit_bindgen::generate!({
    path: "wit",
    world: "provider-v2-toolless-fixture",
    generate_all,
});

use exports::ryuzi::provider0_2_0::provider::{
    CompletionChunk, CompletionRequest, Guest, ModelInfo, ProviderCapabilities, ProviderError,
    StopReason,
};

struct Fixture;

impl Guest for Fixture {
    fn capabilities() -> ProviderCapabilities {
        ProviderCapabilities {
            tools: false,
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

    /// Always answers with plain finished text and no tool calls — this
    /// component genuinely cannot call tools, so nothing it returns ever
    /// carries a `tool-calls` entry, regardless of what the request asked for.
    fn complete(_request: CompletionRequest) -> Result<Vec<CompletionChunk>, ProviderError> {
        Ok(vec![CompletionChunk {
            text: "no tools".to_string(),
            tool_calls: vec![],
            finished: true,
            stop_reason: Some(StopReason::EndTurn),
            usage: None,
        }])
    }
}

export!(Fixture);
