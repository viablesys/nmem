use regex::{Regex, RegexSet};
use std::sync::LazyLock;

/// Parameters controlling filter behavior (entropy thresholds, extra patterns).
pub struct FilterParams {
    pub extra_patterns: Vec<String>,
    pub entropy_threshold: f64,
    pub entropy_min_length: usize,
    pub entropy_enabled: bool,
}

impl Default for FilterParams {
    fn default() -> Self {
        Self {
            extra_patterns: Vec::new(),
            entropy_threshold: 4.0,
            entropy_min_length: 20,
            entropy_enabled: true,
        }
    }
}

pub struct SecretFilter {
    set: RegexSet,
    patterns: Vec<Regex>,
    placeholder: &'static str,
    entropy_threshold: f64,
    entropy_min_length: usize,
    entropy_enabled: bool,
}

const BUILTIN_PATTERNS: &[&str] = &[
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

impl SecretFilter {
    fn new() -> Self {
        Self::with_params(FilterParams::default())
    }

    /// Create a filter with custom parameters (extra patterns, entropy overrides).
    pub fn with_params(params: FilterParams) -> Self {
        let mut all_patterns: Vec<String> =
            BUILTIN_PATTERNS.iter().map(|p| (*p).to_string()).collect();
        all_patterns.extend(params.extra_patterns);

        let set = RegexSet::new(&all_patterns).unwrap();
        let patterns: Vec<Regex> = all_patterns
            .iter()
            .map(|p| Regex::new(p).unwrap())
            .collect();

        Self {
            set,
            patterns,
            placeholder: "[REDACTED]",
            entropy_threshold: params.entropy_threshold,
            entropy_min_length: params.entropy_min_length,
            entropy_enabled: params.entropy_enabled,
        }
    }

    /// Redact secrets from input. Returns (output, was_redacted).
    pub fn redact(&self, input: &str) -> (String, bool) {
        let mut output = input.to_string();
        let mut redacted = false;

        // Phase 1: regex-based redaction
        if self.set.is_match(&output) {
            let matches = self.set.matches(&output);
            for idx in matches.into_iter() {
                if let std::borrow::Cow::Owned(new) =
                    self.patterns[idx].replace_all(&output, self.placeholder)
                {
                    output = new;
                    redacted = true;
                }
            }
        }

        // Phase 2: entropy-based redaction
        if self.entropy_enabled {
            let (entropy_output, entropy_hit) = self.check_entropy(&output);
            if entropy_hit {
                output = entropy_output;
                redacted = true;
            }
        }

        (output, redacted)
    }

    /// Scan tokens for high-entropy strings. Returns (output, was_redacted).
    fn check_entropy(&self, input: &str) -> (String, bool) {
        let tokens = tokenize_for_entropy(input);
        if tokens.is_empty() {
            return (input.to_string(), false);
        }

        let mut output = input.to_string();
        let mut redacted = false;

        // Process in reverse order to preserve byte offsets during replacement
        for &(offset, token) in tokens.iter().rev() {
            if token.len() < self.entropy_min_length {
                continue;
            }
            if is_entropy_allowlisted(token) {
                continue;
            }
            if shannon_entropy(token) >= self.entropy_threshold {
                output.replace_range(offset..offset + token.len(), self.placeholder);
                redacted = true;
            }
        }

        (output, redacted)
    }
}

/// Shannon entropy in bits per character.
pub fn shannon_entropy(s: &str) -> f64 {
    if s.is_empty() {
        return 0.0;
    }
    let mut freq = [0u32; 256];
    let len = s.len() as f64;
    for &b in s.as_bytes() {
        freq[b as usize] += 1;
    }
    let mut entropy = 0.0;
    for &count in &freq {
        if count > 0 {
            let p = count as f64 / len;
            entropy -= p * p.log2();
        }
    }
    entropy
}

/// Split input into (byte_offset, token) pairs on whitespace and delimiters.
/// Keeps `-_/.:=` within tokens (common in paths, URLs, assignments).
fn tokenize_for_entropy(input: &str) -> Vec<(usize, &str)> {
    let mut tokens = Vec::new();
    let mut start = None;

    for (i, ch) in input.char_indices() {
        let is_delimiter = ch.is_whitespace()
            || matches!(ch, '"' | '\'' | '(' | ')' | '[' | ']' | '{' | '}' | '`' | ',' | ';');

        if is_delimiter {
            if let Some(s) = start.take() {
                tokens.push((s, &input[s..i]));
            }
        } else if start.is_none() {
            start = Some(i);
        }
    }
    // Trailing token
    if let Some(s) = start {
        tokens.push((s, &input[s..]));
    }
    tokens
}

/// Returns true if a token is a known high-entropy non-secret.
fn is_entropy_allowlisted(token: &str) -> bool {
    let bytes = token.as_bytes();
    let len = token.len();

    // File paths
    if token.starts_with('/')
        || token.starts_with("./")
        || token.starts_with("~/")
    {
        return true;
    }

    // URLs
    if token.starts_with("http://")
        || token.starts_with("https://")
        || token.starts_with("file://")
    {
        return true;
    }

    // UUID: 8-4-4-4-12 hex with dashes (36 chars)
    if len == 36 && is_uuid(token) {
        return true;
    }

    // Git SHA: exactly 40 hex chars
    if len == 40 && bytes.iter().all(|b| b.is_ascii_hexdigit()) {
        return true;
    }

    // Short git SHA: 7-12 hex chars
    if (7..=12).contains(&len) && bytes.iter().all(|b| b.is_ascii_hexdigit()) {
        return true;
    }

    // Already redacted
    if token == "[REDACTED]" {
        return true;
    }

    false
}

fn is_uuid(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 5 {
        return false;
    }
    let expected_lens = [8, 4, 4, 4, 12];
    parts.iter().zip(expected_lens.iter()).all(|(part, &expected_len)| {
        part.len() == expected_len && part.bytes().all(|b| b.is_ascii_hexdigit())
    })
}

/// Singleton filter — compiled once at process startup.
pub static FILTER: LazyLock<SecretFilter> = LazyLock::new(SecretFilter::new);

/// Recursively redact secrets in a serde_json::Value using the given filter.
pub fn redact_json_value_with(value: &mut serde_json::Value, filter: &SecretFilter) -> bool {
    let mut any_redacted = false;
    match value {
        serde_json::Value::String(s) => {
            let (redacted, was_redacted) = filter.redact(s);
            if was_redacted {
                *s = redacted;
                any_redacted = true;
            }
        }
        serde_json::Value::Object(map) => {
            for v in map.values_mut() {
                any_redacted |= redact_json_value_with(v, filter);
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr.iter_mut() {
                any_redacted |= redact_json_value_with(v, filter);
            }
        }
        _ => {}
    }
    any_redacted
}

/// Recursively redact secrets in a serde_json::Value using the global filter.
pub fn redact_json_value(value: &mut serde_json::Value) -> bool {
    redact_json_value_with(value, &FILTER)
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

    // --- Entropy tests ---

    #[test]
    fn test_shannon_entropy() {
        // Single repeated char → 0 entropy
        assert!((shannon_entropy("aaaaaaa") - 0.0).abs() < 0.01);
        // Uniform distribution of 2 chars → 1.0 bit
        assert!((shannon_entropy("abababab") - 1.0).abs() < 0.01);
        // High-entropy random hex
        let hex64 = "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2";
        assert!(shannon_entropy(hex64) > 3.5);
    }

    #[test]
    fn test_entropy_catches_random_hex() {
        // Mixed-case hex — high entropy, no regex prefix, pure entropy catch
        let hex = "c8EB7Fa171ac826Ca6EfcEe4847BB8CdCcb74Af2134E5FdD2ccDeA8B0F3FB8Ea";
        let input = format!("export MY_KEY={hex}");
        let (output, redacted) = FILTER.redact(&input);
        assert!(redacted, "should catch random hex via entropy");
        assert!(output.contains("[REDACTED]"));
        assert!(!output.contains(hex));
    }

    #[test]
    fn test_entropy_catches_base64_blob() {
        // base64 blob with high entropy
        let b64 = "dGhlIHF1aWNrIGJyb3duIGZveCBqdW1wcyBvdmVy";
        let input = format!("data: {b64}");
        let (output, redacted) = FILTER.redact(&input);
        assert!(redacted, "should catch base64 via entropy");
        assert!(!output.contains(b64));
    }

    #[test]
    fn test_entropy_skips_git_sha() {
        let sha = "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0";
        assert_eq!(sha.len(), 40);
        let input = format!("commit {sha}");
        let (output, redacted) = FILTER.redact(&input);
        assert!(!redacted, "git SHA should be allowlisted");
        assert!(output.contains(sha));
    }

    #[test]
    fn test_entropy_skips_uuid() {
        let uuid = "550e8400-e29b-41d4-a716-446655440000";
        assert_eq!(uuid.len(), 36);
        let input = format!("id: {uuid}");
        let (output, redacted) = FILTER.redact(&input);
        assert!(!redacted, "UUID should be allowlisted");
        assert!(output.contains(uuid));
    }

    #[test]
    fn test_entropy_skips_short_tokens() {
        // Tokens under 20 chars should never trigger entropy check
        let input = "x8fZ9kL2m3 short tokens";
        let (_, redacted) = FILTER.redact(input);
        assert!(!redacted, "short tokens should be skipped");
    }

    #[test]
    fn test_entropy_skips_english_text() {
        let input = "The quick brown fox jumps over the lazy dog and runs away into the forest";
        let (_, redacted) = FILTER.redact(input);
        assert!(!redacted, "English text should not trigger entropy");
    }

    #[test]
    fn test_entropy_skips_file_paths() {
        let input = "/home/user/.config/some-very-long-path/with/many/segments/file.txt";
        let (output, redacted) = FILTER.redact(input);
        assert!(!redacted, "file paths should be allowlisted");
        assert_eq!(output, input);
    }

    #[test]
    fn test_entropy_in_context() {
        // Token embedded in surrounding text — mixed-case for high entropy
        let hex = "c8EB7Fa171ac826Ca6EfcEe4847BB8CdCcb74Af2134E5FdD2ccDeA8B0F3FB8Ea";
        let input = format!("Use key {hex} in the config file");
        let (output, redacted) = FILTER.redact(&input);
        assert!(redacted);
        assert!(output.starts_with("Use key "));
        assert!(output.ends_with(" in the config file"));
        assert!(!output.contains(hex));
    }

    #[test]
    fn test_with_params_extra_patterns() {
        let filter = SecretFilter::with_params(FilterParams {
            extra_patterns: vec![r"MYCO-[A-Za-z0-9]{32}".into()],
            ..Default::default()
        });
        let input = "key: MYCO-abcdefghijklmnopqrstuvwxyz012345";
        let (output, redacted) = filter.redact(input);
        assert!(redacted);
        assert!(output.contains("[REDACTED]"));
    }

    #[test]
    fn test_with_params_entropy_disabled() {
        let filter = SecretFilter::with_params(FilterParams {
            entropy_enabled: false,
            ..Default::default()
        });
        // Random hex that would normally be caught by entropy
        let hex = "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2";
        let input = format!("export MY_KEY={hex}");
        let (output, redacted) = filter.redact(&input);
        assert!(!redacted, "entropy disabled should not catch hex");
        assert!(output.contains(hex));
    }
}
