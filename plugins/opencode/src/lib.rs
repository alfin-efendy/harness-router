//! First-party OpenCode Zen free-tier provider component.
//!
//! Exports `ryuzi:provider/provider@0.2.0` — a full transcript and bound tools
//! in, tool-carrying completion chunks out — and ports OpenCode's free-tier
//! wire contract (`llm_router::registry`'s `opencode-free` descriptor +
//! `client::apply_provider_request_headers`): base `https://opencode.ai/zen/v1`,
//! `Authorization: Bearer public` + `x-opencode-client: desktop`, and NO
//! bootstrap step. All network I/O goes through the host `ryuzi:http/http`
//! capability; the static bearer is forwarded because this is a VERIFIED
//! first-party bundle (see `capabilities::http` self-auth).
//!
//! OpenCode Zen is an OpenAI-compatible `/chat/completions` endpoint, so the
//! request/response mapping is delegated to the shared `ryuzi_openai_format`
//! crate rather than reimplemented — see [`logic`] for what stays local
//! (the static credential, the client-tag header, the component's own URLs,
//! `/models` parsing, and error classification) versus what is a thin
//! delegation. `capabilities()` reports `tools: true`: a live probe of
//! `big-pickle` on 2026-08-09 returned `finish_reason: "tool_calls"` with a
//! well-formed `tool_calls` array.
//!
//! Unlike the host-injected-credential OpenAI-format components (which use
//! `ryuzi_openai_format::provider_component!`), this component's world imports
//! `ryuzi:http` directly and sends its own static bearer, so it cannot use
//! that macro — the wasm-gated `guest` module is hand-rolled effect/mapping
//! glue that follows the same mapper shapes. No storage capability is needed:
//! there is nothing to cache (no minted token, no device identity).

pub mod logic;

#[cfg(target_arch = "wasm32")]
mod guest;
