//! Secret redaction for everything that leaves a trusted boundary:
//! tool output going to Claude, trace lines, console echoes.
//!
//! Three mechanisms:
//! 1. **Exact values** we know are secret (the active provider's API key) are always
//!    replaced, wherever they appear.
//! 2. **Credential patterns**: Anthropic/OpenAI/GitHub/Slack tokens, AWS
//!    access key IDs, PEM private keys, `Bearer` tokens and `password=`-style
//!    assignments.
//! 3. **Sensitive JSON keys** (`password`, `token`, `api_key`, …): the value is
//!    replaced whatever it looks like.
//!
//! Vault references such as `${vault:secret/...}` are pointers, not secrets,
//! and are left alone.

use serde_json::Value;

pub const REDACTED: &str = "[REDACTED]";

/// Token prefixes followed by at least `min` token characters.
const TOKEN_PREFIXES: [(&str, usize); 10] = [
    ("sk-ant-", 10),
    ("sk-or-", 10),
    ("sk-proj-", 10),
    ("sk-live-", 10),
    ("ghp_", 20),
    ("gho_", 20),
    ("github_pat_", 20),
    ("xoxb-", 10),
    ("xoxp-", 10),
    ("xapp-", 10),
];

/// `name=value` / `name: value` assignments whose value is a secret.
const ASSIGNMENT_NAMES: [&str; 8] = [
    "password",
    "passwd",
    "pwd",
    "api_key",
    "apikey",
    "secret",
    "token",
    "access_key",
];

/// JSON object keys whose values are always redacted.
const SENSITIVE_KEYS: [&str; 10] = [
    "password",
    "passwd",
    "secret",
    "token",
    "api_key",
    "apikey",
    "x-api-key",
    "authorization",
    "credential",
    "private_key",
];

#[derive(Debug, Clone, Default)]
pub struct Redactor {
    exact: Vec<String>,
    /// Local paths shown as placeholders (e.g. the project folder → `<project>`),
    /// so machine and user names don't leak into Claude's context or traces.
    aliases: Vec<(String, String)>,
}

impl Redactor {
    /// `exact`: known secret values (values shorter than 8 characters are
    /// ignored, they would cause false matches).
    pub fn new(exact: impl IntoIterator<Item = String>) -> Self {
        Self {
            exact: exact.into_iter().filter(|s| s.len() >= 8).collect(),
            aliases: Vec::new(),
        }
    }

    /// Shows `path` as `placeholder`. Add longer paths first.
    pub fn with_alias(mut self, path: &str, placeholder: &str) -> Self {
        let path = path.trim_end_matches(['\\', '/']);
        if path.len() >= 3 {
            self.aliases
                .push((path.to_string(), placeholder.to_string()));
        }
        self
    }

    /// Redacts a string. Returns the cleaned text and how many secrets were removed.
    pub fn text(&self, input: &str) -> (String, usize) {
        let mut out = input.to_string();
        let mut count = 0;
        for secret in &self.exact {
            let n = out.matches(secret.as_str()).count();
            if n > 0 {
                out = out.replace(secret.as_str(), REDACTED);
                count += n;
            }
        }
        for (path, placeholder) in &self.aliases {
            out = out.replace(path.as_str(), placeholder);
            // The JSON-escaped form (C:\\Users\\...) as it appears inside JSON text.
            let escaped = path.replace('\\', "\\\\");
            if escaped != *path {
                out = out.replace(&escaped, placeholder);
            }
        }
        count += redact_pem(&mut out);
        for (prefix, min) in TOKEN_PREFIXES {
            count += redact_after(&mut out, prefix, min, true);
        }
        count += redact_aws_key_ids(&mut out);
        count += redact_after(&mut out, "bearer ", 16, false);
        for name in ASSIGNMENT_NAMES {
            count += redact_assignment(&mut out, name);
        }
        (out, count)
    }

    pub fn redact_str(&self, input: &str) -> String {
        self.text(input).0
    }

    /// Redacts a JSON value: sensitive keys entirely, every string by pattern.
    pub fn json(&self, value: &Value) -> Value {
        match value {
            Value::String(s) => Value::String(self.redact_str(s)),
            Value::Array(items) => Value::Array(items.iter().map(|v| self.json(v)).collect()),
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(k, v)| {
                        let lower = k.to_ascii_lowercase();
                        let sensitive = SENSITIVE_KEYS
                            .iter()
                            .any(|s| lower == *s || lower.ends_with(&format!("_{s}")));
                        let v = if sensitive && !v.is_null() {
                            Value::String(REDACTED.into())
                        } else {
                            self.json(v)
                        };
                        (k.clone(), v)
                    })
                    .collect(),
            ),
            other => other.clone(),
        }
    }
}

fn is_token_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'/' | b'+' | b'=')
}

/// Replaces `<prefix><token>` (case-insensitive prefix match). When
/// `include_prefix` is false the prefix (e.g. "Bearer ") is kept.
fn redact_after(text: &mut String, prefix: &str, min: usize, include_prefix: bool) -> usize {
    let mut count = 0;
    let mut from = 0;
    loop {
        let lower = text.to_ascii_lowercase();
        let Some(rel) = lower[from..].find(prefix) else {
            return count;
        };
        let start = from + rel;
        let token_start = start + prefix.len();
        let bytes = text.as_bytes();
        let mut end = token_start;
        while end < bytes.len() && is_token_char(bytes[end]) {
            end += 1;
        }
        if end - token_start >= min {
            let replace_from = if include_prefix { start } else { token_start };
            text.replace_range(replace_from..end, REDACTED);
            count += 1;
            from = replace_from + REDACTED.len();
        } else {
            from = token_start;
        }
    }
}

/// AWS access key IDs: `AKIA` or `ASIA` + 16 upper-case letters/digits.
fn redact_aws_key_ids(text: &mut String) -> usize {
    let mut count = 0;
    let mut i = 0;
    while i + 20 <= text.len() {
        let bytes = text.as_bytes();
        let candidate = &bytes[i..i + 20];
        let prefix_ok = candidate.starts_with(b"AKIA") || candidate.starts_with(b"ASIA");
        let body_ok = candidate[4..]
            .iter()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit());
        let boundary_ok = (i == 0 || !bytes[i - 1].is_ascii_alphanumeric())
            && (i + 20 == bytes.len() || !bytes[i + 20].is_ascii_alphanumeric());
        if prefix_ok && body_ok && boundary_ok {
            text.replace_range(i..i + 20, REDACTED);
            count += 1;
            i += REDACTED.len();
        } else {
            i += 1;
        }
    }
    count
}

/// `-----BEGIN ... PRIVATE KEY----- ... -----END ... PRIVATE KEY-----`
/// (certificates and public keys are not secret and are left alone).
fn redact_pem(text: &mut String) -> usize {
    let mut count = 0;
    let mut from = 0;
    while let Some(rel) = text[from..].find("-----BEGIN") {
        let start = from + rel;
        let Some(h) = text[start + 10..].find("-----") else {
            break;
        };
        let header_end = start + 10 + h + 5;
        if !text[start..header_end].contains("PRIVATE KEY") {
            from = header_end;
            continue;
        }
        let end = match text[header_end..].find("-----END") {
            Some(p) => {
                let e = header_end + p + 8;
                text[e..].find("-----").map_or(text.len(), |q| e + q + 5)
            }
            None => text.len(),
        };
        text.replace_range(start..end, REDACTED);
        count += 1;
        from = start + REDACTED.len();
    }
    count
}
/// `password=hunter2`, `api_key: abc123`, `"token": "xyz"`. Vault references are kept.
fn redact_assignment(text: &mut String, name: &str) -> usize {
    let mut count = 0;
    let mut from = 0;
    loop {
        let lower = text.to_ascii_lowercase();
        let Some(rel) = lower[from..].find(name) else {
            return count;
        };
        let name_start = from + rel;
        let mut i = name_start + name.len();
        let bytes = text.as_bytes();
        // The name must not be the middle of a longer word on the left.
        let left_ok = name_start == 0 || !bytes[name_start - 1].is_ascii_alphanumeric();
        // Allow a closing quote, spaces, then = or :, then spaces/quote.
        while i < bytes.len() && matches!(bytes[i], b'"' | b'\'') {
            i += 1;
        }
        while i < bytes.len() && bytes[i] == b' ' {
            i += 1;
        }
        if !left_ok || i >= bytes.len() || !matches!(bytes[i], b'=' | b':') {
            from = name_start + name.len();
            continue;
        }
        i += 1;
        while i < bytes.len() && matches!(bytes[i], b' ' | b'"' | b'\'') {
            i += 1;
        }
        let value_start = i;
        while i < bytes.len()
            && !matches!(
                bytes[i],
                b' ' | b'\n' | b'\r' | b'\t' | b'&' | b';' | b',' | b'"' | b'\'' | b'}'
            )
        {
            i += 1;
        }
        let value = &text[value_start..i];
        if value.len() >= 4 && !value.starts_with("${") && value != REDACTED {
            text.replace_range(value_start..i, REDACTED);
            count += 1;
            from = value_start + REDACTED.len();
        } else {
            from = i.max(name_start + name.len());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn r() -> Redactor {
        Redactor::new(["exact-secret-value-123".to_string()])
    }

    #[test]
    fn known_secret_formats_are_removed() {
        let cases = [
            (
                "key sk-ant-api03-AbCdEf0123456789xyz end",
                "key [REDACTED] end",
            ),
            (
                "or key sk-or-v1-0123456789abcdef0123 end",
                "or key [REDACTED] end",
            ),
            ("aws AKIAIOSFODNN7EXAMPLE here", "aws [REDACTED] here"),
            (
                "Authorization: Bearer abcdefghijklmnop1234",
                "Authorization: Bearer [REDACTED]",
            ),
            (
                "token ghp_abcdefghijklmnopqrstuvwxyz0123",
                "token [REDACTED]",
            ),
            (
                "db password=hunter2secret; next",
                "db password=[REDACTED]; next",
            ),
            (
                "{\"api_key\": \"abc123def456\"}",
                "{\"api_key\": \"[REDACTED]\"}",
            ),
            ("the exact-secret-value-123 leaked", "the [REDACTED] leaked"),
        ];
        for (input, expected) in cases {
            assert_eq!(r().redact_str(input), expected, "{input}");
        }
    }

    #[test]
    fn private_keys_are_removed_but_certificates_kept() {
        let pem =
            "a\n-----BEGIN RSA PRIVATE KEY-----\nMIIBOgIBAAJBAK\n-----END RSA PRIVATE KEY-----\nb";
        assert_eq!(r().redact_str(pem), "a\n[REDACTED]\nb");
        let cert = "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----";
        assert_eq!(r().redact_str(cert), cert);
    }

    #[test]
    fn harmless_text_and_vault_references_are_untouched() {
        for text in [
            "api_key: ${vault:secret/payments/payflow-api-key}",
            "password_reset email sent",
            "token issuance latency 41ms",
            "The secret of good logs is structure.",
            "AKIA is a prefix",
            "short password=abc",
        ] {
            assert_eq!(r().redact_str(text), text, "{text}");
        }
    }

    #[test]
    fn the_real_mock_data_has_nothing_to_redact() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("mock_data");
        for file in [
            "logs.json",
            "deployments.json",
            "workspace/RELEASE_NOTES.md",
            "workspace/service-config.yaml",
        ] {
            let text = std::fs::read_to_string(root.join(file)).unwrap();
            assert_eq!(r().text(&text).1, 0, "{file}");
        }
    }

    #[test]
    fn local_paths_become_placeholders() {
        let r = Redactor::default()
            .with_alias(r"C:\Users\Alice\proj", "<project>")
            .with_alias(r"C:\Users\Alice", "<home>");
        assert_eq!(
            r.redact_str(r"denied: C:\Users\Alice\proj\.env not in C:\Users\Alice\proj\mock_data, C:\Users\Alice\.ssh"),
            r"denied: <project>\.env not in <project>\mock_data, <home>\.ssh"
        );
        assert_eq!(
            r.redact_str(r#"{"p":"C:\\Users\\Alice\\proj\\x"}"#),
            r#"{"p":"<project>\\x"}"#
        );
        assert_eq!(
            r.text(r"C:\Users\Alice").1,
            0,
            "aliases are not counted as secrets"
        );
    }

    #[test]
    fn json_keys_and_nested_strings() {
        let v = json!({"title": "ok", "Authorization": "anything", "db_password": "x", "nested": [{"note": "sk-ant-api03-ZZZZZZZZZZZZZZ"}], "token": null});
        let out = r().json(&v);
        assert_eq!(out["title"], "ok");
        assert_eq!(out["Authorization"], REDACTED);
        assert_eq!(out["db_password"], REDACTED);
        assert_eq!(out["nested"][0]["note"], REDACTED);
        assert_eq!(out["token"], Value::Null);
    }
}
