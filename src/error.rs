//! Structured errors. Every failure carries a stable `code` plus optional
//! `details`, so an agent can branch on the code rather than parse a message.

use serde_json::{json, Value};
use std::fmt;

#[derive(Debug)]
pub struct NaviError {
    pub code: String,
    pub message: String,
    pub details: Value,
}

impl NaviError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        NaviError {
            code: code.to_string(),
            message: message.into(),
            details: Value::Null,
        }
    }

    pub fn with_details(mut self, details: Value) -> Self {
        self.details = details;
        self
    }

    pub fn as_json(&self) -> Value {
        json!({
            "code": self.code,
            "message": self.message,
            "details": self.details,
        })
    }
}

impl fmt::Display for NaviError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for NaviError {}

impl From<std::io::Error> for NaviError {
    fn from(e: std::io::Error) -> Self {
        NaviError::new("io_error", e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, NaviError>;
