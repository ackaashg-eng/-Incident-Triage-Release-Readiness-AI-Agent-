//! Structured errors returned to MCP clients.
//!
//! Every failure a tool can hit is turned into a [`ToolError`] and sent back as
//! a normal tool result with `isError: true` and this JSON body:
//!
//! ```json
//! {"error": {"code": "UNKNOWN_SERVICE", "message": "...", "hint": "...",
//!            "valid_values": ["auth-service", "..."], "field_errors": [...]}}
//! ```
//!
//! A tool result (rather than a JSON-RPC protocol error) is deliberate: the MCP
//! spec says errors the caller should see and react to belong in the result,
//! so the AI agent can read the message and correct its next call.

use serde::Serialize;
use serde_json::{Value, json};

/// Machine-readable error categories (serialised as `SCREAMING_SNAKE_CASE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    /// The service name is not one of the known services.
    UnknownService,
    /// A severity value is not allowed.
    InvalidSeverity,
    /// `limit` is out of range.
    InvalidLimit,
    /// `window_hours` is out of range.
    InvalidTimeWindow,
    /// One or more ticket fields failed validation.
    InvalidTicketFields,
    /// A data file is missing or unreadable.
    DataUnavailable,
    /// A data file exists but its content is malformed.
    DataCorrupt,
    /// The ticket database could not be opened or written.
    DatabaseError,
    /// The resource URI does not exist.
    UnknownResource,
}

/// A problem with one named input field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FieldError {
    pub field: &'static str,
    pub problem: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, thiserror::Error)]
#[error("{code:?}: {message}")]
pub struct ToolError {
    pub code: ErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valid_values: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub field_errors: Vec<FieldError>,
}

impl ToolError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            hint: None,
            valid_values: None,
            field_errors: Vec::new(),
        }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn with_valid_values<S: ToString>(mut self, values: impl IntoIterator<Item = S>) -> Self {
        self.valid_values = Some(values.into_iter().map(|v| v.to_string()).collect());
        self
    }

    pub fn with_field_errors(mut self, errors: Vec<FieldError>) -> Self {
        self.field_errors = errors;
        self
    }

    /// The JSON body sent to the client: `{"error": {...}}`.
    pub fn to_json(&self) -> Value {
        json!({ "error": self })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialises_to_the_documented_shape() {
        let err = ToolError::new(ErrorCode::UnknownService, "no such service")
            .with_hint("pick one of the valid values")
            .with_valid_values(["a", "b"]);
        assert_eq!(
            err.to_json(),
            json!({"error": {
                "code": "UNKNOWN_SERVICE",
                "message": "no such service",
                "hint": "pick one of the valid values",
                "valid_values": ["a", "b"]
            }})
        );
    }

    #[test]
    fn empty_optional_parts_are_omitted() {
        let body = ToolError::new(ErrorCode::DatabaseError, "boom").to_json();
        assert_eq!(
            body,
            json!({"error": {"code": "DATABASE_ERROR", "message": "boom"}})
        );
    }
}
