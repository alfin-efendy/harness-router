//! DT5: dispatch a Track C [`HookEvent`] to every subscribed, running
//! extension subprocess — the second sink for the event dispatch point
//! Track C built (`harness::native::hooks::run` is the first sink, firing
//! the same event to on-disk scripts; `harness::native::hooks::fire_hook`
//! combines both — see that function's doc).
//!
//! # Gating vs. observational
//! - **Gating (`HookEvent::is_gating()`, i.e. `tool.before`)**: every
//!   subscribed extension is contacted CONCURRENTLY (not one at a time —
//!   `futures::future::join_all`) and awaited, each bounded by ITS OWN
//!   manifest `timeout_ms` ([`proc::dispatch_event`] enforces this per
//!   extension, so joining them concurrently bounds total wait to the
//!   slowest single extension's timeout, never their sum). ANY extension
//!   denying denies the call. A timeout or a transport failure (crash,
//!   closed pipe) is **fail-OPEN**: treated as "did not deny," plus a
//!   `tracing::warn!` — a broken/slow extension must NEVER deadlock or brick
//!   the agent. This mirrors `harness::native::hooks::run`'s own script
//!   contract (missing hook dir / spawn failure = allow), just with a
//!   network round trip instead of a process exit code.
//! - **Observational** (`session.start`/`tool.after`/`session.end`): never
//!   awaited on the caller's path at all. Each subscribed extension's send
//!   is handed to a detached `tokio::spawn` task, gated by
//!   [`proc::ExtensionHost::try_acquire_observational_permit`] so a burst of
//!   slow/misbehaving extensions can only ever have a bounded number of
//!   sends in flight — a send that can't get a permit is dropped (logged),
//!   never queued. [`ExtensionEvents::dispatch`] returns
//!   `HookResult::allow()` immediately in this branch, before any of those
//!   tasks could possibly have resolved.
//!
//! An extension NOT subscribed to the firing event (its confirmed
//! `events` from `extension/initialize` doesn't include it) is never
//! contacted at all — see [`proc::dispatch_event`]'s `Skipped` outcome.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::harness::native::hooks::{HookEvent, HookResult};

use super::proc::{DispatchHandle, EventDispatchOutcome, ExtensionHost};

/// Cap on a sanitized deny reason's length (characters, after secret-shaped
/// redaction below). A gating deny reason IS meant to be shown to the
/// user/agent — unlike an init-handshake failure
/// (`proc::sanitize_init_error`), which collapses to a canned per-stage
/// message — so this only truncates and screens, it never discards the
/// whole message.
const MAX_DENY_REASON_CHARS: usize = 300;

/// Case-insensitive substrings that mark a deny reason as "secret-shaped" —
/// deliberately broad and over-inclusive: a false positive just replaces a
/// harmless reason with a generic marker; a false negative could leak a
/// credential straight into a transcript/UI. An extension is less trusted
/// than a script the user wrote by hand (it is arbitrary vendor code), so
/// its reason gets this extra screening a script's stdout does not.
const SECRET_SHAPED_MARKERS: &[&str] = &[
    "token",
    "secret",
    "password",
    "passwd",
    "apikey",
    "api_key",
    "api-key",
    "authorization",
    "bearer",
    "credential",
];

/// Dispatch a lifecycle [`HookEvent`] to registered extension subprocesses.
/// Implemented for [`ExtensionHost`]; `harness::native::hooks::fire_hook`
/// (Track C's combine point) calls through a `SessionCtx.extension_events:
/// Option<Arc<dyn ExtensionEvents>>` so the hot fire sites never depend on
/// `ExtensionHost`'s concrete type.
#[async_trait]
pub trait ExtensionEvents: Send + Sync {
    /// Dispatch `event` to every subscribed+running extension.
    ///
    /// Gating events await each subscribed extension's response up to its
    /// own `timeout_ms`; a `{"deny": true, "reason": "..."}` denies the
    /// action. A timeout or a crashed/closed transport is fail-OPEN (allow)
    /// plus a warning.
    ///
    /// Observational events are fire-and-forget and this call returns
    /// `HookResult::allow()` immediately, without waiting on any extension.
    async fn dispatch(&self, event: HookEvent, payload: &Value) -> HookResult;
}

#[async_trait]
impl ExtensionEvents for ExtensionHost {
    async fn dispatch(&self, event: HookEvent, payload: &Value) -> HookResult {
        let handles = self.dispatch_handles().await;
        if handles.is_empty() {
            return HookResult::allow();
        }
        if event.is_gating() {
            dispatch_gating(handles, event, payload).await
        } else {
            self.dispatch_observational(handles, event, payload);
            HookResult::allow()
        }
    }
}

/// The gating half of [`ExtensionEvents::dispatch`] — see this module's doc.
async fn dispatch_gating(
    handles: Vec<DispatchHandle>,
    event: HookEvent,
    payload: &Value,
) -> HookResult {
    let calls = handles.into_iter().map(|handle| async move {
        let outcome = handle.dispatch(event, payload).await;
        (handle.name().to_string(), outcome)
    });
    for (name, outcome) in futures::future::join_all(calls).await {
        match outcome {
            EventDispatchOutcome::Denied(reason) => {
                return HookResult {
                    allowed: false,
                    message: Some(sanitize_deny_reason(&name, reason)),
                };
            }
            EventDispatchOutcome::Unreachable => {
                tracing::warn!(
                    extension = %name,
                    event = event.as_str(),
                    "extension timed out or its transport failed responding to a gating event \
                     — failing open (allow) so a broken extension can never brick the agent"
                );
            }
            EventDispatchOutcome::Allowed | EventDispatchOutcome::Skipped => {}
        }
    }
    HookResult::allow()
}

impl ExtensionHost {
    /// The observational half of [`ExtensionEvents::dispatch`] — see this
    /// module's doc. Never awaited by its caller: each send is a detached
    /// `tokio::spawn` task, bounded by
    /// [`ExtensionHost::try_acquire_observational_permit`].
    fn dispatch_observational(
        &self,
        handles: Vec<DispatchHandle>,
        event: HookEvent,
        payload: &Value,
    ) {
        let payload = Arc::new(payload.clone());
        for handle in handles {
            let Some(permit) = self.try_acquire_observational_permit() else {
                tracing::warn!(
                    extension = %handle.name(),
                    event = event.as_str(),
                    "dropped an observational event dispatch: too many sends already in flight"
                );
                continue;
            };
            let payload = payload.clone();
            tokio::spawn(async move {
                let _permit = permit; // held for this send's lifetime
                let outcome = handle.dispatch(event, &payload).await;
                if matches!(outcome, EventDispatchOutcome::Unreachable) {
                    tracing::debug!(
                        extension = %handle.name(),
                        event = event.as_str(),
                        "observational event dispatch to extension timed out or failed \
                         (ignored — fire-and-forget)"
                    );
                }
            });
        }
    }
}

/// Turn an extension's raw deny reason into something safe to surface in a
/// transcript/UI: `None`/empty becomes a generic marker; a reason that looks
/// like it contains a credential (see [`SECRET_SHAPED_MARKERS`]) is replaced
/// wholesale rather than surgically redacted (an extension controls its own
/// formatting, so a partial redaction is easy to get wrong); everything else
/// is capped at [`MAX_DENY_REASON_CHARS`]. Always prefixed with the
/// extension's name, mirroring a script hook's own denial message
/// (`harness::native::hooks::run`'s `"blocked by hook {path}"` fallback).
fn sanitize_deny_reason(name: &str, reason: Option<String>) -> String {
    let Some(raw) = reason
        .map(|r| r.trim().to_string())
        .filter(|r| !r.is_empty())
    else {
        return format!("{name}: denied (no reason given)");
    };
    let lower = raw.to_lowercase();
    let screened = if SECRET_SHAPED_MARKERS.iter().any(|m| lower.contains(m)) {
        "[reason withheld: it looked like it might contain a credential]".to_string()
    } else {
        raw
    };
    let capped = if screened.chars().count() > MAX_DENY_REASON_CHARS {
        let mut s: String = screened.chars().take(MAX_DENY_REASON_CHARS).collect();
        s.push('…');
        s
    } else {
        screened
    };
    format!("{name}: {capped}")
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- sanitize_deny_reason (pure, no I/O) ----------

    #[test]
    fn sanitize_deny_reason_passes_through_a_plain_reason_with_the_extension_name() {
        assert_eq!(
            sanitize_deny_reason("linter", Some("bash is not allowed here".to_string())),
            "linter: bash is not allowed here"
        );
    }

    #[test]
    fn sanitize_deny_reason_falls_back_when_no_reason_given() {
        assert_eq!(
            sanitize_deny_reason("linter", None),
            "linter: denied (no reason given)"
        );
        assert_eq!(
            sanitize_deny_reason("linter", Some("   ".to_string())),
            "linter: denied (no reason given)"
        );
    }

    #[test]
    fn sanitize_deny_reason_withholds_a_secret_shaped_reason() {
        let reason = sanitize_deny_reason(
            "linter",
            Some("denied: token=leaked-secret-token in the request".to_string()),
        );
        assert!(!reason.contains("leaked-secret-token"));
        assert!(reason.contains("withheld"));
    }

    #[test]
    fn sanitize_deny_reason_caps_length() {
        let long = "x".repeat(1000);
        let reason = sanitize_deny_reason("linter", Some(long));
        assert!(reason.chars().count() <= MAX_DENY_REASON_CHARS + "linter: ".len() + 1);
        assert!(reason.ends_with('…'));
    }

    // NOTE: the former "ExtensionEvents integration (real sh-based fake
    // extensions)" section was deleted here: all 5 tests
    // (`gating_dispatch_denies_when_a_subscribed_extension_denies`,
    // `observational_dispatch_returns_immediately_even_if_the_extension_is_slow`,
    // `gating_dispatch_fails_open_when_a_subscribed_extension_times_out`,
    // `gating_dispatch_fails_open_when_the_extension_crashes_mid_dispatch`,
    // `dispatch_does_not_contact_an_extension_not_subscribed_to_the_event`)
    // relied on `ExtensionHost::spawn_all` discovering an `ExtensionFactory`
    // via a `CorePlugin.extension`-driven `extension_only(...)` fixture to
    // get a real fake extension running before exercising `dispatch`.
    // `CorePlugin.extension` no longer exists (the v2 SDK manifest has no
    // `[[extension]]` surface), `spawn_all` is now a permanent no-op, and no
    // plugin can ever be discovered this way — that whole integration is
    // categorically impossible pending Task 3's full deletion of Track D
    // subprocess extensions.
}
