//! THROWAWAY probe (Task C5, Step 1): does MiMo's free-tier gate accept a
//! `tools` array on `/api/free-ai/openai/chat`?
//!
//! MiMo requires a bootstrap JWT, so this cannot be a plain curl. This test
//! performs the exact bootstrap dance `logic.rs` implements, then POSTs a
//! one-tool chat request built in the same OpenAI function-calling shape
//! `ryuzi_openai_format::OpenAiFormat::build_chat_body` emits, and prints the
//! status + response body so the result can be recorded by hand.
//!
//! `#[ignore]`d because it hits the live network on every run. To reproduce:
//!
//! ```sh
//! cd plugins/mimo
//! cargo test --test probe_tools_gate -- --ignored --nocapture
//! ```
//!
//! Result recorded 2026-08-09 (see `.superpowers/sdd/task-C5-report.md` for
//! the full transcript and the corroborating upstream evidence): BOTH the
//! baseline (no-tools) request and the one-tool request came back identical —
//! HTTP 400 `{"error":{"code":"400","message":"Unsupported model mimo-auto"}}`.
//! Since the toolless baseline fails exactly the same way, the rejection is
//! not about `tools` at all: the entire `mimo-auto` free channel is gone.
//! `XiaomiMiMo/MiMo-Code`'s own `packages/opencode/src/util/free-api-sunset.ts`
//! sets `FREE_API_SUNSET_AT = 2026-07-26T10:00:00Z`, already past by the time
//! of this probe — so there is no live signal available to prove or disprove
//! tools support, and none is claimed. See `capabilities()` in `guest.rs`.

use ryuzi_plugin_mimo::logic;

#[test]
#[ignore = "hits the live MiMo free-tier gate; run manually, see module docs"]
fn probe_whether_the_free_gate_accepts_a_tools_array() {
    let http = reqwest::blocking::Client::new();

    // 1. Bootstrap a JWT exactly as `logic::bootstrap_body` shapes it.
    let seed = format!("probe-c5-{}", std::process::id());
    let fingerprint = logic::fingerprint_from_seed(seed.as_bytes());
    let session_affinity = logic::session_affinity_from_seed(seed.as_bytes());

    let bootstrap_resp = http
        .post(logic::BOOTSTRAP_URL)
        .header("user-agent", logic::CHROME_UA)
        .header("content-type", "application/json")
        .body(logic::bootstrap_body(&fingerprint))
        .send()
        .expect("bootstrap request should reach the network");
    let bootstrap_status = bootstrap_resp.status().as_u16();
    let bootstrap_body = bootstrap_resp.bytes().expect("bootstrap body readable");
    eprintln!(
        "bootstrap: HTTP {bootstrap_status} body={}",
        String::from_utf8_lossy(&bootstrap_body)
    );
    assert!(
        (200..300).contains(&bootstrap_status),
        "bootstrap must succeed for the probe to mean anything"
    );
    let jwt = logic::parse_bootstrap_jwt(&bootstrap_body).expect("bootstrap must carry a jwt");

    // 2a. Baseline: a plain, toolless request (the shape `logic::build_chat_body`
    //     already sends in production), to disambiguate a "the gate rejects
    //     tools" verdict from "the gate rejects this model/request entirely
    //     right now" — an unrelated, environment-specific failure would
    //     otherwise be misread as a tools rejection.
    let baseline_messages = vec![ryuzi_openai_format::MessageIn {
        role: ryuzi_openai_format::RoleIn::User,
        content: vec![ryuzi_openai_format::BlockIn::Text(
            "What is 2 plus 2?".to_string(),
        )],
    }];
    let baseline_body = logic::build_chat_body(ryuzi_openai_format::ChatRequest {
        model: logic::MODEL_ID,
        messages: &baseline_messages,
        tools: &[],
        tool_choice: ryuzi_openai_format::ToolChoiceIn::Auto,
        max_tokens: None,
        temperature: None,
        leading_system: Some(logic::SYSTEM_MARKER),
    });
    let headers = logic::chat_headers(&jwt, &session_affinity);
    let mut baseline_req = http.post(logic::CHAT_URL);
    for (name, value) in &headers {
        baseline_req = baseline_req.header(name, value);
    }
    let baseline_resp = baseline_req
        .body(baseline_body)
        .send()
        .expect("baseline chat request should reach the network");
    let baseline_status = baseline_resp.status().as_u16();
    let baseline_body_bytes = baseline_resp.bytes().expect("baseline body readable");
    eprintln!(
        "baseline (no tools): HTTP {baseline_status} body={}",
        String::from_utf8_lossy(&baseline_body_bytes)
    );

    // 2b. One chat request carrying a single tool, in the exact OpenAI
    //    function-calling shape `ryuzi_openai_format` builds — the marker
    //    stays messages[0] as the gate requires.
    let body = serde_json::json!({
        "model": logic::MODEL_ID,
        "stream": false,
        "messages": [
            {"role": "system", "content": logic::SYSTEM_MARKER},
            {"role": "user", "content": "What is 2 plus 2? Use the calculator tool to compute it."}
        ],
        "tools": [{
            "type": "function",
            "function": {
                "name": "calculator",
                "description": "Adds two numbers.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "a": {"type": "number"},
                        "b": {"type": "number"}
                    },
                    "required": ["a", "b"]
                }
            }
        }],
        "tool_choice": "auto"
    });

    let mut req = http.post(logic::CHAT_URL);
    for (name, value) in &headers {
        req = req.header(name, value);
    }
    let chat_resp = req
        .json(&body)
        .send()
        .expect("chat request should reach the network");
    let chat_status = chat_resp.status().as_u16();
    let chat_body = chat_resp.bytes().expect("chat body readable");
    eprintln!(
        "chat: HTTP {chat_status} body={}",
        String::from_utf8_lossy(&chat_body)
    );

    // No assertion on the verdict itself — this test exists to PRINT the
    // evidence for a human (or a follow-up agent) to record in the task
    // report, not to encode a pass/fail expectation about an external
    // service's behaviour.
}
