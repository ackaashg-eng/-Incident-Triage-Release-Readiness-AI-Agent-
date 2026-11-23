//! Validates Claude's tool arguments against the tool's original MCP input
//! schema **before** anything is sent to a server.
//!
//! The servers validate too (defence in depth), but checking here means:
//! * a malformed call never reaches a server (important for write tools);
//! * Claude gets one clear, complete error message to fix its next call.
//!
//! Supported keywords (what the two servers actually use): `type` (string or
//! list), `enum`, `const`, `required`, `properties`, `additionalProperties:
//! false`, `minimum`/`maximum`/`exclusiveMinimum`/`exclusiveMaximum`,
//! `minLength`/`maxLength`, `items`, `minItems`/`maxItems`, `anyOf`/`oneOf`.
//! Unknown keywords are ignored (the server still enforces them).

use serde_json::Value;

/// Returns every problem found (empty = valid).
pub fn validate(input: &Value, schema: &Value) -> Vec<String> {
    let mut errors = Vec::new();
    check(input, schema, "arguments", &mut errors);
    errors
}

fn check(value: &Value, schema: &Value, path: &str, errors: &mut Vec<String>) {
    let Some(s) = schema.as_object() else { return };

    for key in ["anyOf", "oneOf"] {
        if let Some(Value::Array(options)) = s.get(key)
            && !options
                .iter()
                .any(|o| validate_at(value, o, path).is_empty())
        {
            errors.push(format!("{path}: does not match any allowed form"));
            return;
        }
    }

    if let Some(t) = s.get("type") {
        let allowed: Vec<&str> = match t {
            Value::String(t) => vec![t.as_str()],
            Value::Array(ts) => ts.iter().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        };
        if !allowed.is_empty() && !allowed.iter().any(|t| has_type(value, t)) {
            errors.push(format!(
                "{path}: expected {}, got {}",
                allowed.join(" or "),
                type_name(value)
            ));
            return;
        }
    }

    if let Some(Value::Array(options)) = s.get("enum")
        && !options.contains(value)
    {
        let shown: Vec<String> = options.iter().map(Value::to_string).collect();
        errors.push(format!(
            "{path}: {value} is not one of {}",
            shown.join(", ")
        ));
    }
    if let Some(c) = s.get("const")
        && c != value
    {
        errors.push(format!("{path}: must be {c}"));
    }

    if let Some(n) = value.as_f64() {
        let bound = |k: &str| s.get(k).and_then(Value::as_f64);
        if let Some(min) = bound("minimum")
            && n < min
        {
            errors.push(format!("{path}: {value} is below the minimum {min}"));
        }
        if let Some(max) = bound("maximum")
            && n > max
        {
            errors.push(format!("{path}: {value} is above the maximum {max}"));
        }
        if let Some(min) = bound("exclusiveMinimum")
            && n <= min
        {
            errors.push(format!("{path}: {value} must be greater than {min}"));
        }
        if let Some(max) = bound("exclusiveMaximum")
            && n >= max
        {
            errors.push(format!("{path}: {value} must be less than {max}"));
        }
    }

    if let Some(text) = value.as_str() {
        let len = text.chars().count() as u64;
        if let Some(min) = s.get("minLength").and_then(Value::as_u64)
            && len < min
        {
            errors.push(format!(
                "{path}: must be at least {min} characters (got {len})"
            ));
        }
        if let Some(max) = s.get("maxLength").and_then(Value::as_u64)
            && len > max
        {
            errors.push(format!(
                "{path}: must be at most {max} characters (got {len})"
            ));
        }
    }

    if let Some(items) = value.as_array() {
        let n = items.len() as u64;
        if let Some(min) = s.get("minItems").and_then(Value::as_u64)
            && n < min
        {
            errors.push(format!("{path}: needs at least {min} items"));
        }
        if let Some(max) = s.get("maxItems").and_then(Value::as_u64)
            && n > max
        {
            errors.push(format!("{path}: allows at most {max} items"));
        }
        if let Some(item_schema) = s.get("items") {
            for (i, item) in items.iter().enumerate() {
                check(item, item_schema, &format!("{path}[{i}]"), errors);
            }
        }
    }

    if let Some(obj) = value.as_object() {
        let props = s.get("properties").and_then(Value::as_object);
        if let Some(Value::Array(required)) = s.get("required") {
            for name in required.iter().filter_map(Value::as_str) {
                if !obj.contains_key(name) {
                    errors.push(format!("{path}: missing required field `{name}`"));
                }
            }
        }
        for (key, v) in obj {
            match props.and_then(|p| p.get(key)) {
                Some(prop_schema) => check(v, prop_schema, &format!("{path}.{key}"), errors),
                None => {
                    if s.get("additionalProperties") == Some(&Value::Bool(false)) {
                        let known: Vec<&str> = props
                            .map(|p| p.keys().map(String::as_str).collect())
                            .unwrap_or_default();
                        errors.push(format!(
                            "{path}: unknown field `{key}` (allowed: {})",
                            known.join(", ")
                        ));
                    }
                }
            }
        }
    }
}

fn validate_at(value: &Value, schema: &Value, path: &str) -> Vec<String> {
    let mut errors = Vec::new();
    check(value, schema, path, &mut errors);
    errors
}

fn has_type(value: &Value, t: &str) -> bool {
    match t {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        "number" => value.is_number(),
        "integer" => {
            value.is_i64() || value.is_u64() || value.as_f64().is_some_and(|f| f.fract() == 0.0)
        }
        _ => true,
    }
}

fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The real `search_logs` schema shape published by ops-telemetry-mcp.
    fn search_logs() -> Value {
        json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["service_name"],
            "properties": {
                "service_name": {"type": "string", "minLength": 1, "maxLength": 64},
                "severity": {"type": ["string", "null"], "enum": ["INFO", "WARN", "ERROR", "FATAL", null]},
                "limit": {"type": ["integer", "null"], "format": "int64", "minimum": 1, "maximum": 100}
            }
        })
    }

    #[test]
    fn valid_arguments_pass() {
        assert!(validate(&json!({"service_name": "auth-service"}), &search_logs()).is_empty());
        assert!(
            validate(
                &json!({"service_name": "a", "severity": "ERROR", "limit": 5}),
                &search_logs()
            )
            .is_empty()
        );
        assert!(
            validate(
                &json!({"service_name": "a", "severity": null, "limit": null}),
                &search_logs()
            )
            .is_empty()
        );
    }

    #[test]
    fn every_problem_is_reported() {
        let errors = validate(
            &json!({"severity": "CRITICAL", "limit": 100000, "path": "../.env"}),
            &search_logs(),
        );
        let joined = errors.join(" | ");
        assert!(
            joined.contains("missing required field `service_name`"),
            "{joined}"
        );
        assert!(joined.contains("\"CRITICAL\" is not one of"), "{joined}");
        assert!(joined.contains("above the maximum 100"), "{joined}");
        assert!(joined.contains("unknown field `path`"), "{joined}");
        assert_eq!(errors.len(), 4, "{joined}");
    }

    #[test]
    fn types_are_checked() {
        for (bad, expect) in [
            (json!({"service_name": 42}), "expected string, got integer"),
            (
                json!({"service_name": "a", "limit": "ten"}),
                "expected integer or null, got string",
            ),
            (
                json!({"service_name": "a", "limit": 2.5}),
                "expected integer or null, got number",
            ),
            (json!({"service_name": ""}), "at least 1 characters"),
            (json!("just a string"), "expected object, got string"),
            (Value::Null, "expected object, got null"),
        ] {
            let joined = validate(&bad, &search_logs()).join(" | ");
            assert!(joined.contains(expect), "{bad}: {joined}");
        }
    }

    #[test]
    fn nested_arrays_and_any_of() {
        let schema = json!({"type": "object", "properties": {
            "paths": {"type": "array", "items": {"type": "string"}, "minItems": 1},
            "mode": {"anyOf": [{"const": "a"}, {"const": "b"}]}
        }});
        assert!(validate(&json!({"paths": ["x"], "mode": "a"}), &schema).is_empty());
        let joined = validate(&json!({"paths": [1], "mode": "c"}), &schema).join(" | ");
        assert!(
            joined.contains("arguments.paths[0]: expected string"),
            "{joined}"
        );
        assert!(
            joined.contains("arguments.mode: does not match"),
            "{joined}"
        );
        assert!(!validate(&json!({"paths": []}), &schema).is_empty());
    }
}
