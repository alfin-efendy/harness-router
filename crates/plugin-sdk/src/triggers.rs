//! Trigger vocabulary for declarative `[[hooks]]`: the SDK's string-level
//! copy of `ryuzi_core::automation::TriggerKind` (the SDK cannot depend on
//! ryuzi-core), plus the Claude Code alias spellings accepted at input
//! boundaries. **Keep `CANONICAL_TRIGGERS` in sync with `TriggerKind`** —
//! same discipline as the old `KNOWN_HOOK_EVENTS`.

/// Canonical dotted trigger names, exactly `TriggerKind`'s serde strings.
pub const CANONICAL_TRIGGERS: &[&str] = &[
    "session.start",
    "tool.before",
    "tool.after",
    "session.end",
    "scheduler.run.success",
    "scheduler.run.failed",
    "gateway.status.changed",
    "webhook.inbound",
];

/// Claude Code alias → canonical. `Stop` and `SessionEnd` both map to
/// `session.end` (Claude fires Stop at turn end; ryuzi's nearest event).
/// ORDER MATTERS: when inverting for display (`claude_alias_for`, Task 12),
/// the FIRST alias for a canonical name wins — `Stop` is listed before
/// `SessionEnd` deliberately, matching the UI's display choice (Task 16).
pub const CLAUDE_ALIASES: &[(&str, &str)] = &[
    ("PreToolUse", "tool.before"),
    ("PostToolUse", "tool.after"),
    ("SessionStart", "session.start"),
    ("Stop", "session.end"),
    ("SessionEnd", "session.end"),
];

/// Resolve a trigger spelling (canonical or Claude alias) to its canonical
/// dotted name. `None` for an unknown spelling.
pub fn canonical_trigger(input: &str) -> Option<&'static str> {
    if let Some(found) = CANONICAL_TRIGGERS.iter().find(|t| **t == input) {
        return Some(found);
    }
    CLAUDE_ALIASES
        .iter()
        .find(|(alias, _)| *alias == input)
        .map(|(_, canonical)| *canonical)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_names_resolve_to_themselves() {
        for t in CANONICAL_TRIGGERS {
            assert_eq!(canonical_trigger(t), Some(*t));
        }
    }

    #[test]
    fn claude_aliases_resolve_to_canonical() {
        assert_eq!(canonical_trigger("PreToolUse"), Some("tool.before"));
        assert_eq!(canonical_trigger("PostToolUse"), Some("tool.after"));
        assert_eq!(canonical_trigger("SessionStart"), Some("session.start"));
        assert_eq!(canonical_trigger("Stop"), Some("session.end"));
        assert_eq!(canonical_trigger("SessionEnd"), Some("session.end"));
    }

    #[test]
    fn unknown_spellings_are_rejected() {
        assert_eq!(canonical_trigger("UserPromptSubmit"), None); // follow-up, not v2
        assert_eq!(canonical_trigger("tool_before"), None);
        assert_eq!(canonical_trigger(""), None);
    }
}
