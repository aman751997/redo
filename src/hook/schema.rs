//! Hook-payload shape sanity check.
//!
//! redo's projection from `PostToolUse[Bash]` to `Output`, and from
//! `PostToolUse[Edit|Write|MultiEdit]` to `FileWrite`, depends on a handful
//! of fields in the hook stdin payload. Anthropic owns that payload shape; if
//! they change it, our projection silently degrades to "everything is a
//! `Marker`" again — exactly the failure mode the audits flagged.
//!
//! This module defines the fields we depend on per hook kind and exposes a
//! `validate` that checks an `Envelope` against them. On a mismatch we log a
//! `tracing::warn!` with a stable message that names the missing field and
//! the hook kind, and the recorder bumps a counter on `Meta`. Recording
//! continues — the point is loud surface visibility, not breakage.
//!
//! The pinned snapshot of the payload shapes lives in
//! `tests/fixtures/hooks/claude_code_v1.json`. When Anthropic ships a payload
//! change, update that fixture, bump the contract here, and ship the
//! migration in lockstep.

use serde_json::Value;

/// Documentation pointer surfaced in the warning message so a user hitting
/// drift has a place to start.
pub const PAYLOAD_DOC_URL: &str = "https://docs.claude.com/en/docs/claude-code/hooks";

/// Required top-level fields per hook kind. Empty list means "no hard
/// requirement" — the hook fires but redo doesn't currently project anything
/// out of it.
pub fn expected_fields(kind: &str) -> &'static [&'static str] {
    match kind {
        // PreToolUse fires before the tool runs. We need `tool_name` and
        // `tool_input` to surface the call in the marker.
        "PreToolUse" => &["tool_name", "tool_input"],
        // PostToolUse fires after the tool returns. We additionally need
        // `tool_response` for the `Output` and `FileWrite` projections.
        "PostToolUse" => &["tool_name", "tool_input", "tool_response"],
        // The remaining hook kinds carry their own shapes; we don't project
        // anything out of them yet, so no required fields.
        "UserPromptSubmit" | "Stop" | "Notification" | "SubagentStop" | "PreCompact" => &[],
        _ => &[],
    }
}

/// Outcome of a single envelope validation.
#[derive(Debug, Default, Clone)]
pub struct ValidationReport {
    /// Names of fields that were expected but missing. Empty on success.
    pub missing_fields: Vec<&'static str>,
}

impl ValidationReport {
    pub fn ok(&self) -> bool {
        self.missing_fields.is_empty()
    }
}

/// Validate that `payload` carries the fields redo depends on for `kind`.
/// Returns the report; the caller decides what to do with a non-ok result.
pub fn validate(kind: &str, payload: &Value) -> ValidationReport {
    let mut report = ValidationReport::default();
    let expected = expected_fields(kind);
    if expected.is_empty() {
        return report;
    }
    let obj = match payload.as_object() {
        Some(o) => o,
        None => {
            // Non-object payload for a kind we have expectations for is itself
            // a drift signal. Surface every required field as missing.
            report.missing_fields.extend(expected.iter().copied());
            return report;
        }
    };
    for field in expected {
        if !obj.contains_key(*field) {
            report.missing_fields.push(*field);
        }
    }
    report
}

/// Emit a `tracing::warn!` for a drift event. Pulled into a function so the
/// message text is stable and grep-able.
pub fn warn_drift(kind: &str, missing: &[&'static str]) {
    for field in missing {
        tracing::warn!(
            field = field,
            kind = kind,
            doc = PAYLOAD_DOC_URL,
            "redo expected field {field} for hook kind {kind}; payload shape may have drifted - see {PAYLOAD_DOC_URL}"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn pretooluse_requires_tool_name_and_input() {
        let payload = json!({"tool_name": "Bash", "tool_input": {}});
        assert!(validate("PreToolUse", &payload).ok());
    }

    #[test]
    fn pretooluse_missing_tool_input_reports_drift() {
        let payload = json!({"tool_name": "Bash"});
        let r = validate("PreToolUse", &payload);
        assert!(!r.ok());
        assert!(r.missing_fields.contains(&"tool_input"));
    }

    #[test]
    fn posttooluse_requires_tool_response() {
        let full = json!({
            "tool_name": "Bash",
            "tool_input": {},
            "tool_response": {"stdout": "x"}
        });
        assert!(validate("PostToolUse", &full).ok());
        let stripped = json!({"tool_name": "Bash", "tool_input": {}});
        let r = validate("PostToolUse", &stripped);
        assert_eq!(r.missing_fields, vec!["tool_response"]);
    }

    #[test]
    fn unknown_kind_is_unconstrained() {
        assert!(validate("MysteryHook", &json!({})).ok());
    }

    #[test]
    fn non_object_payload_for_constrained_kind_is_drift() {
        let r = validate("PreToolUse", &json!("not an object"));
        assert!(!r.ok());
        assert!(r.missing_fields.contains(&"tool_name"));
    }
}
