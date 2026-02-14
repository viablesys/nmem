use regex::{Regex, RegexSet};
use std::sync::LazyLock;

pub struct SecretFilter {
    set: RegexSet,
    patterns: Vec<Regex>,
    placeholder: &'static str,
}

impl SecretFilter {
    fn new() -> Self {
        // More-specific patterns before broader ones.
        let pattern_strings = vec![
            // AWS
            r"AKIA[0-9A-Z]{16}",
            r"(?i)aws[_\-]?secret[_\-]?access[_\-]?key\s*[=:]\s*\S+",
            // GitHub (longest prefix first)
            r"github_pat_[A-Za-z0-9_]{82}",
            r"ghp_[A-Za-z0-9]{36}",
            r"gho_[A-Za-z0-9]{36}",
            r"ghs_[A-Za-z0-9]{36}",
            // API keys (Anthropic before generic sk-)
            r"sk-ant-[A-Za-z0-9\-]{20,}",
            r"sk-[A-Za-z0-9\-]{20,}",
            // Bearer tokens
            r"(?i)bearer\s+[A-Za-z0-9_\-.~+/]{20,}=*",
            // Private keys
            r"-----BEGIN\s+(RSA|EC|DSA|OPENSSH|PGP)\s+PRIVATE\s+KEY-----",
            // Connection strings with credentials
            r"(postgres|mysql|mongodb|redis|amqp|https?)://[^:]+:[^@\s]+@",
            // Generic password/secret/token assignment
            r"(?i)(password|passwd|secret|token|api_key|apikey)\s*[=:]\s*\S+",
        ];

        let set = RegexSet::new(&pattern_strings).unwrap();
        let patterns: Vec<Regex> = pattern_strings
            .iter()
            .map(|p| Regex::new(p).unwrap())
            .collect();

        Self {
            set,
            patterns,
            placeholder: "[REDACTED]",
        }
    }

    /// Redact secrets from input. Returns (output, was_redacted).
    pub fn redact(&self, input: &str) -> (String, bool) {
        // Fast path: single-pass check across all patterns
        if !self.set.is_match(input) {
            return (input.to_string(), false);
        }

        // Slow path: replace only matched patterns
        let mut output = input.to_string();
        let mut redacted = false;
        let matches = self.set.matches(input);
        for idx in matches.into_iter() {
            if let std::borrow::Cow::Owned(new) = self.patterns[idx]
                .replace_all(&output, self.placeholder)
            {
                output = new;
                redacted = true;
            }
        }
        (output, redacted)
    }
}

/// Singleton filter — compiled once at process startup.
pub static FILTER: LazyLock<SecretFilter> = LazyLock::new(SecretFilter::new);

/// Recursively redact secrets in a serde_json::Value.
pub fn redact_json_value(value: &mut serde_json::Value) -> bool {
    let mut any_redacted = false;
    match value {
        serde_json::Value::String(s) => {
            let (redacted, was_redacted) = FILTER.redact(s);
            if was_redacted {
                *s = redacted;
                any_redacted = true;
            }
        }
        serde_json::Value::Object(map) => {
            for v in map.values_mut() {
                any_redacted |= redact_json_value(v);
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr.iter_mut() {
                any_redacted |= redact_json_value(v);
            }
        }
        _ => {}
    }
    any_redacted
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_true_positives() {
        let cases = vec![
            ("AKIAIOSFODNN7EXAMPLE", "AWS access key"),
            (
                "ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdef1234",
                "GitHub PAT",
            ),
            (
                "sk-ant-api03-abcdefghijklmnopqrstuvwxyz",
                "Anthropic key",
            ),
            (
                "sk-proj-abcdefghijklmnopqrstuvwxyz1234",
                "OpenAI key",
            ),
            (
                "Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.abc",
                "JWT bearer",
            ),
            (
                "-----BEGIN RSA PRIVATE KEY-----",
                "PEM header",
            ),
            (
                "postgres://admin:s3cret@db.host:5432/mydb",
                "Connection string",
            ),
            ("password=hunter2", "Generic assignment"),
        ];

        for (input, label) in cases {
            let (output, redacted) = FILTER.redact(input);
            assert!(redacted, "should redact {label}: {input}");
            assert!(
                output.contains("[REDACTED]"),
                "output should contain placeholder for {label}: got {output}"
            );
        }
    }

    #[test]
    fn test_false_positives_avoided() {
        let cases = vec![
            ("password_validator", "variable name"),
            ("git://github.com/user/repo", "git URL"),
            ("file:///tmp/token_cache/data", "file path"),
            ("sk-iplink", "short non-key"),
            ("the token count was 150", "natural language"),
        ];

        for (input, label) in cases {
            let (_, redacted) = FILTER.redact(input);
            assert!(!redacted, "should NOT redact {label}: {input}");
        }
    }

    #[test]
    fn test_partial_redaction_preserves_context() {
        let (output, redacted) = FILTER.redact(
            "Set key to sk-proj-abcdefghijklmnopqrstuvwxyz1234 in config",
        );
        assert!(redacted);
        assert!(output.starts_with("Set key to "));
        assert!(output.ends_with(" in config"));
        assert!(output.contains("[REDACTED]"));
    }

    #[test]
    fn test_no_secret_no_allocation_semantics() {
        let (output, redacted) = FILTER.redact("no secrets here");
        assert!(!redacted);
        assert_eq!(output, "no secrets here");
    }

    #[test]
    fn test_redact_json_value() {
        let mut val = json!({
            "command": "curl -H 'Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.abcdefghijklmnop' http://example.com",
            "nested": {
                "key": "password=hunter2"
            }
        });
        let redacted = redact_json_value(&mut val);
        assert!(redacted);
        assert!(val["command"].as_str().unwrap().contains("[REDACTED]"));
        assert!(val["nested"]["key"]
            .as_str()
            .unwrap()
            .contains("[REDACTED]"));
    }
}
