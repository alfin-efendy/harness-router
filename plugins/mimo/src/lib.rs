//! First-party Xiaomi MiMo free-tier provider component.
//!
//! Exports `ryuzi:provider/provider@0.2.0` (`list-models` + `complete` over a
//! full transcript, plus `capabilities`) and ports the MiMo free-tier wire
//! protocol from `ryuzi-core`'s `llm_router::mimo` faithfully, behind the
//! host-mediated `ryuzi:http/http` capability. See [`logic`] for the exact
//! strings and the rationale behind the anti-abuse gate (bootstrap JWT,
//! Chrome-like headers, `x-session-affinity`, and the MiMoCode system
//! marker). `capabilities()` reports `tools: false` — see [`logic`]'s module
//! docs and the comment on `logic::classify_chat_error` for why: a live probe
//! found no evidence the gate accepts a `tools` array (the whole free channel
//! is sunset upstream), so no tools support is claimed.
//!
//! # Architecture: pure `logic` vs. wasm `guest`
//! Every piece of behaviour that does not need a live host — request/response
//! shaping, JWT parsing, transient-block classification — lives in the [`logic`]
//! module as pure functions over plain Rust types, so it is exercised by native
//! `cargo test` without a wasm host. The `guest` module (compiled only for
//! `wasm32`) is thin glue: it wires `logic` to the `ryuzi:http`/`ryuzi:storage`
//! host imports and maps the plain types to/from the generated WIT types.
//!
//! # Why storage-backed JWT caching
//! The Task 10 provider adapter RE-INSTANTIATES the component per `complete()`
//! call (`WasmProviderTransport` compiles once, instantiates per call), so an
//! in-component `static` JWT cache would be wiped every call. The bootstrap JWT
//! (with its expiry), the stable device fingerprint, and the session-affinity
//! id are therefore persisted in the host `ryuzi:storage` capability (scoped to
//! this plugin by the host), so they survive re-instantiation. Storage is
//! granted to every component bundle by the host policy; if it is ever denied,
//! the guest degrades to bootstrapping per call rather than failing.

pub mod logic;

#[cfg(target_arch = "wasm32")]
mod guest;
