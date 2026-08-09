//! Pure mapping between an Anthropic-Messages request body and the structured
//! `ryuzi:provider/provider@0.2.0` records, plus the reverse mapping from a
//! returned chunk into the synthetic OpenAI streaming delta that
//! `translate::OpenAiToAnthropicStream` already knows how to turn into
//! Anthropic events.
//!
//! Deliberately separate from `llm_router::client` (~4000 lines) and free of
//! any wasmtime dependency, so every mapping rule is unit-testable without
//! compiling or instantiating a component.

use serde_json::Value;

use crate::plugins::wasm_provider::{
    WasmCompletionRequestV2, WasmContentBlock, WasmMessage, WasmRole, WasmToolCall, WasmToolChoice,
    WasmToolDef, WasmToolResult,
};

/// Map an Anthropic-Messages body onto the structured 0.2.0 request.
///
/// `system` (string or content-block array) becomes a leading `System` message
/// so a component sees it as an ordinary turn — the WIT `role` enum carries it,
/// so nothing is lost the way `flatten_anthropic_prompt` loses it. An absent or
/// empty `system` adds no message at all.
#[allow(dead_code)] // Consumed by the transport wiring in Task 4.
pub(crate) fn request_from_anthropic_body(body: &Value, model: &str) -> WasmCompletionRequestV2 {
    let mut messages = Vec::new();
    let system = join_text_blocks(&body["system"]);
    if !system.is_empty() {
        messages.push(WasmMessage {
            role: WasmRole::System,
            content: vec![WasmContentBlock::Text(system)],
        });
    }
    for message in body["messages"].as_array().into_iter().flatten() {
        messages.push(WasmMessage {
            role: role_from_str(message["role"].as_str().unwrap_or("user")),
            content: content_blocks(&message["content"]),
        });
    }
    WasmCompletionRequestV2 {
        model: model.to_string(),
        messages,
        tools: body["tools"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(tool_def)
            .collect(),
        tool_choice: tool_choice(&body["tool_choice"]),
        max_tokens: body["max_tokens"]
            .as_u64()
            .and_then(|v| u32::try_from(v).ok()),
        temperature: body["temperature"].as_f64().map(|v| v as f32),
    }
}

fn role_from_str(role: &str) -> WasmRole {
    match role {
        "system" => WasmRole::System,
        "assistant" => WasmRole::Assistant,
        _ => WasmRole::User,
    }
}

/// Every `text` field of a string-or-array content value, newline-joined.
/// Used for `system` and for a structured `tool_result.content`.
fn join_text_blocks(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|block| block["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn content_blocks(content: &Value) -> Vec<WasmContentBlock> {
    match content {
        Value::String(text) => vec![WasmContentBlock::Text(text.clone())],
        Value::Array(blocks) => blocks.iter().filter_map(content_block).collect(),
        _ => Vec::new(),
    }
}

fn content_block(block: &Value) -> Option<WasmContentBlock> {
    match block["type"].as_str() {
        Some("text") => Some(WasmContentBlock::Text(
            block["text"].as_str().unwrap_or_default().to_string(),
        )),
        Some("tool_use") => Some(WasmContentBlock::ToolUse(WasmToolCall {
            id: block["id"].as_str().unwrap_or_default().to_string(),
            name: block["name"].as_str().unwrap_or_default().to_string(),
            arguments: block
                .get("input")
                .map(|input| input.to_string())
                .unwrap_or_else(|| "{}".to_string()),
        })),
        Some("tool_result") => Some(WasmContentBlock::ToolResult(WasmToolResult {
            tool_call_id: block["tool_use_id"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            content: match &block["content"] {
                Value::String(text) => text.clone(),
                other => join_text_blocks(other),
            },
            is_error: block["is_error"].as_bool().unwrap_or(false),
        })),
        // `thinking`, `image` and any future block type have no representation
        // in this ABI; dropping them is lossy but keeps the transcript valid.
        _ => None,
    }
}

fn tool_def(tool: &Value) -> Option<WasmToolDef> {
    let name = tool["name"].as_str()?.to_string();
    Some(WasmToolDef {
        name,
        description: tool["description"].as_str().unwrap_or_default().to_string(),
        input_schema: tool
            .get("input_schema")
            .map(|schema| schema.to_string())
            .unwrap_or_else(|| "{}".to_string()),
    })
}

/// Anthropic's `tool_choice` shapes: `auto` / `none` / `any` / a named `tool`.
/// `any` and a named tool both mean "you must call something", which is the
/// closest this ABI expresses — a component cannot force one specific tool.
fn tool_choice(value: &Value) -> WasmToolChoice {
    match value["type"].as_str() {
        Some("none") => WasmToolChoice::None,
        Some("any" | "tool") => WasmToolChoice::Required,
        _ => WasmToolChoice::Auto,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::wasm_provider::{WasmContentBlock, WasmRole, WasmToolChoice};
    use serde_json::json;

    #[test]
    fn system_becomes_a_leading_system_message_and_roles_are_preserved() {
        let body = json!({
            "system": "You are Ryuzi.",
            "messages": [
                {"role": "user", "content": "hi"},
                {"role": "assistant", "content": [{"type": "text", "text": "hello"}]}
            ]
        });

        let request = request_from_anthropic_body(&body, "big-pickle");

        assert_eq!(request.model, "big-pickle");
        assert_eq!(request.messages.len(), 3);
        assert!(matches!(request.messages[0].role, WasmRole::System));
        assert!(matches!(
            &request.messages[0].content[0],
            WasmContentBlock::Text(t) if t == "You are Ryuzi."
        ));
        assert!(matches!(request.messages[1].role, WasmRole::User));
        assert!(matches!(request.messages[2].role, WasmRole::Assistant));
    }

    #[test]
    fn a_system_array_is_joined_into_one_block_and_absent_system_adds_no_message() {
        let arrayed = request_from_anthropic_body(
            &json!({
                "system": [{"type": "text", "text": "a"}, {"type": "text", "text": "b"}],
                "messages": [{"role": "user", "content": "hi"}]
            }),
            "m",
        );
        assert_eq!(arrayed.messages.len(), 2);
        assert!(matches!(
            &arrayed.messages[0].content[0],
            WasmContentBlock::Text(t) if t == "a\nb"
        ));

        let bare = request_from_anthropic_body(
            &json!({"messages": [{"role": "user", "content": "hi"}]}),
            "m",
        );
        assert_eq!(bare.messages.len(), 1);
        assert!(matches!(bare.messages[0].role, WasmRole::User));
    }

    #[test]
    fn tool_use_and_tool_result_blocks_survive_the_round_trip() {
        let body = json!({
            "messages": [
                {"role": "assistant", "content": [
                    {"type": "tool_use", "id": "toolu_1", "name": "jira_search",
                     "input": {"jql": "project = SFM"}}
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "toolu_1",
                     "content": "3 issues", "is_error": false}
                ]}
            ]
        });

        let request = request_from_anthropic_body(&body, "m");

        let WasmContentBlock::ToolUse(call) = &request.messages[0].content[0] else {
            panic!(
                "expected a tool_use block, got {:?}",
                request.messages[0].content[0]
            );
        };
        assert_eq!(call.id, "toolu_1");
        assert_eq!(call.name, "jira_search");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&call.arguments).unwrap(),
            json!({"jql": "project = SFM"}),
            "input must serialize to a JSON object string"
        );

        let WasmContentBlock::ToolResult(result) = &request.messages[1].content[0] else {
            panic!("expected a tool_result block");
        };
        assert_eq!(result.tool_call_id, "toolu_1");
        assert_eq!(result.content, "3 issues");
        assert!(!result.is_error);
    }

    #[test]
    fn a_structured_tool_result_content_array_is_flattened_to_text() {
        let body = json!({
            "messages": [{"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "t1",
                 "content": [{"type": "text", "text": "line one"},
                             {"type": "text", "text": "line two"}],
                 "is_error": true}
            ]}]
        });

        let request = request_from_anthropic_body(&body, "m");

        let WasmContentBlock::ToolResult(result) = &request.messages[0].content[0] else {
            panic!("expected a tool_result block");
        };
        assert_eq!(result.content, "line one\nline two");
        assert!(result.is_error);
    }

    #[test]
    fn tools_map_to_tool_defs_with_a_serialized_schema() {
        let body = json!({
            "messages": [],
            "tools": [{
                "name": "get_weather",
                "description": "Get current weather",
                "input_schema": {"type": "object", "properties": {"city": {"type": "string"}}}
            }]
        });

        let request = request_from_anthropic_body(&body, "m");

        assert_eq!(request.tools.len(), 1);
        assert_eq!(request.tools[0].name, "get_weather");
        assert_eq!(request.tools[0].description, "Get current weather");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&request.tools[0].input_schema).unwrap()
                ["properties"]["city"]["type"],
            "string"
        );
    }

    #[test]
    fn tool_choice_maps_every_anthropic_form_and_defaults_to_auto() {
        let with = |choice: serde_json::Value| {
            request_from_anthropic_body(&json!({"messages": [], "tool_choice": choice}), "m")
                .tool_choice
        };
        assert!(matches!(
            with(json!({"type": "auto"})),
            WasmToolChoice::Auto
        ));
        assert!(matches!(
            with(json!({"type": "any"})),
            WasmToolChoice::Required
        ));
        assert!(matches!(
            with(json!({"type": "tool", "name": "x"})),
            WasmToolChoice::Required
        ));
        assert!(matches!(
            with(json!({"type": "none"})),
            WasmToolChoice::None
        ));
        assert!(matches!(
            request_from_anthropic_body(&json!({"messages": []}), "m").tool_choice,
            WasmToolChoice::Auto
        ));
    }

    #[test]
    fn max_tokens_and_temperature_carry_over() {
        let request = request_from_anthropic_body(
            &json!({"messages": [], "max_tokens": 512, "temperature": 0.25}),
            "m",
        );
        assert_eq!(request.max_tokens, Some(512));
        assert_eq!(request.temperature, Some(0.25));
    }
}
