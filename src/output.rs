//! The response envelope every command emits on stdout. One shape for every
//! tool: agents parse the same outer keys regardless of which command ran.

use serde_json::{json, Value};

/// The structured result a command hands back to `main`, which wraps it in the
/// envelope and records telemetry from the same fields.
pub struct Outcome {
    pub result: Value,
    pub backend: Option<String>,
    /// Set when a preferred backend was missing and we fell back. Surfaced in
    /// both the envelope and telemetry so gaps are measurable.
    pub fallback_reason: Option<String>,
    pub returned: usize,
    pub elided: usize,
    pub truncated: bool,
}

impl Outcome {
    pub fn new(result: Value) -> Self {
        Outcome {
            result,
            backend: None,
            fallback_reason: None,
            returned: 1,
            elided: 0,
            truncated: false,
        }
    }

    pub fn backend(mut self, backend: impl Into<String>) -> Self {
        self.backend = Some(backend.into());
        self
    }

    pub fn fallback(mut self, reason: impl Into<String>) -> Self {
        self.fallback_reason = Some(reason.into());
        self
    }

    pub fn budget(mut self, returned: usize, elided: usize, truncated: bool) -> Self {
        self.returned = returned;
        self.elided = elided;
        self.truncated = truncated;
        self
    }
}

pub fn envelope_ok(tool: &str, o: &Outcome) -> Value {
    json!({
        "tool": tool,
        "ok": true,
        "backend": o.backend,
        "fallback_reason": o.fallback_reason,
        "budget": {
            "returned": o.returned,
            "elided": o.elided,
            "truncated": o.truncated,
        },
        "result": o.result,
        "error": Value::Null,
    })
}

pub fn envelope_err(tool: &str, error: Value) -> Value {
    json!({
        "tool": tool,
        "ok": false,
        "backend": Value::Null,
        "fallback_reason": Value::Null,
        "budget": { "returned": 0, "elided": 0, "truncated": false },
        "result": Value::Null,
        "error": error,
    })
}
