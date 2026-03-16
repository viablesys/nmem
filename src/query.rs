//! FTS5 query infrastructure — sanitization, tiered rewriting, stopwords.
//!
//! Used by CLI search (s1_search), MCP search (s1_serve), purge (s3_purge),
//! and fleet beacon (s4_beacon).

/// Stopwords stripped before FTS5 tier construction.
const STOPWORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "been", "being", "but", "by",
    "can", "could", "did", "do", "does", "for", "from", "had", "has", "have",
    "in", "is", "it", "its", "may", "might", "not", "of", "on", "or", "shall",
    "should", "that", "the", "this", "to", "was", "were", "what", "which",
    "who", "will", "with", "would",
];

/// Sanitize user input for FTS5 MATCH queries.
///
/// FTS5 treats `-` as NOT and `*` as prefix wildcard. This function quotes
/// individual terms that contain characters that would be misinterpreted
/// (hyphens, colons, dots, slashes), while preserving valid FTS5 operators
/// (AND, OR, NOT, NEAR) and already-quoted phrases.
///
/// Returns `None` if the input produces no usable tokens.
pub fn sanitize_fts_query(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }

    // If the entire input is already a quoted phrase, pass through
    if trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() > 1 {
        return Some(trimmed.to_string());
    }

    let fts_operators = ["AND", "OR", "NOT", "NEAR"];
    let needs_quoting = |w: &str| -> bool {
        w.contains('-')
            || w.contains(':')
            || w.contains('.')
            || w.contains('/')
            || w.contains('\\')
            || w.contains('(')
            || w.contains(')')
            || w.contains('{')
            || w.contains('}')
            || w.contains('[')
            || w.contains(']')
    };

    let words: Vec<&str> = trimmed.split_whitespace().collect();

    // If there are no non-operator terms, operators can't be preserved
    let has_operands = words.iter().any(|w| !fts_operators.contains(w));

    let terms: Vec<String> = words
        .into_iter()
        .map(|w| {
            // Already quoted — pass through
            if w.starts_with('"') && w.ends_with('"') && w.len() > 1 {
                return w.to_string();
            }
            // FTS5 operators — only pass through if there are actual operands
            if fts_operators.contains(&w) {
                if has_operands {
                    return w.to_string();
                }
                // No operands — quote the operator so it's treated as literal
                return format!("\"{}\"", w);
            }
            // Terms with problematic characters — quote them
            if needs_quoting(w) {
                return format!("\"{}\"", w.replace('"', ""));
            }
            // Plain words — pass through
            w.to_string()
        })
        .filter(|t| !t.is_empty())
        .collect();

    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" "))
    }
}

/// Generate tiered FTS5 query variants for first-shot accuracy.
///
/// Returns queries ordered from highest precision to lowest:
/// 1. Exact phrase: `"term1 term2 term3"`
/// 2. AND (all terms): `term1 term2 term3`
/// 3. OR (any term): `term1 OR term2 OR term3`
/// 4. Prefix OR: `term1* OR term2* OR term3*`
///
/// Stopwords are stripped before tier construction. Returns empty Vec
/// if no usable terms remain. Callers iterate tiers, stopping at the
/// first that returns results.
pub fn rewrite_query(input: &str) -> Vec<String> {
    let terms: Vec<String> = input
        .split_whitespace()
        .filter(|w| {
            let lower = w.to_lowercase();
            !STOPWORDS.contains(&lower.as_str()) && w.len() >= 2
        })
        .map(|w| {
            // Quote terms with special chars (consistent with sanitize_fts_query)
            if w.contains(['-', ':', '.', '/', '\\', '(', ')', '{', '}', '[', ']']) {
                format!("\"{}\"", w.replace('"', ""))
            } else {
                w.to_string()
            }
        })
        .collect();

    if terms.is_empty() {
        return Vec::new();
    }

    if terms.len() == 1 {
        // Single term: skip redundant phrase and AND tiers
        let prefix = if terms[0].starts_with('"') {
            terms[0].clone() // already quoted, can't prefix
        } else {
            format!("{}*", terms[0])
        };
        return vec![terms[0].clone(), prefix];
    }

    let phrase = format!("\"{}\"", terms.join(" "));
    let and_query = terms.join(" ");
    let or_query = terms.join(" OR ");
    let prefix_query = terms
        .iter()
        .map(|t| {
            if t.starts_with('"') {
                t.clone()
            } else {
                format!("{t}*")
            }
        })
        .collect::<Vec<_>>()
        .join(" OR ");

    vec![phrase, and_query, or_query, prefix_query]
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- sanitize_fts_query tests (moved from lib.rs) ---

    #[test]
    fn sanitize_plain_word() {
        assert_eq!(sanitize_fts_query("cargo"), Some("cargo".into()));
    }

    #[test]
    fn sanitize_hyphenated_term() {
        assert_eq!(
            sanitize_fts_query("OPS-1234"),
            Some("\"OPS-1234\"".into())
        );
    }

    #[test]
    fn sanitize_multiple_words() {
        assert_eq!(
            sanitize_fts_query("cargo test"),
            Some("cargo test".into())
        );
    }

    #[test]
    fn sanitize_preserves_fts_operators() {
        assert_eq!(
            sanitize_fts_query("auth OR cargo"),
            Some("auth OR cargo".into())
        );
    }

    #[test]
    fn sanitize_quoted_phrase_passthrough() {
        assert_eq!(
            sanitize_fts_query("\"already quoted\""),
            Some("\"already quoted\"".into())
        );
    }

    #[test]
    fn sanitize_empty_input() {
        assert_eq!(sanitize_fts_query(""), None);
        assert_eq!(sanitize_fts_query("   "), None);
    }

    #[test]
    fn sanitize_path_with_slashes() {
        assert_eq!(
            sanitize_fts_query("/src/auth.rs"),
            Some("\"/src/auth.rs\"".into())
        );
    }

    #[test]
    fn sanitize_mixed_plain_and_special() {
        assert_eq!(
            sanitize_fts_query("fix OPS-1234 bug"),
            Some("fix \"OPS-1234\" bug".into())
        );
    }

    // --- rewrite_query tests ---

    #[test]
    fn rewrite_single_term() {
        let tiers = rewrite_query("cargo");
        assert_eq!(tiers.len(), 2);
        assert_eq!(tiers[0], "cargo");
        assert_eq!(tiers[1], "cargo*");
    }

    #[test]
    fn rewrite_multi_term() {
        let tiers = rewrite_query("session summarization backfill");
        assert_eq!(tiers.len(), 4);
        assert_eq!(tiers[0], "\"session summarization backfill\"");
        assert_eq!(tiers[1], "session summarization backfill");
        assert_eq!(tiers[2], "session OR summarization OR backfill");
        assert_eq!(tiers[3], "session* OR summarization* OR backfill*");
    }

    #[test]
    fn rewrite_strips_stopwords() {
        let tiers = rewrite_query("the session and summary");
        // "the" and "and" stripped → ["session", "summary"]
        assert_eq!(tiers.len(), 4);
        assert_eq!(tiers[0], "\"session summary\"");
        assert_eq!(tiers[1], "session summary");
    }

    #[test]
    fn rewrite_all_stopwords_returns_empty() {
        let tiers = rewrite_query("the and or is a");
        assert!(tiers.is_empty());
    }

    #[test]
    fn rewrite_empty_input() {
        assert!(rewrite_query("").is_empty());
        assert!(rewrite_query("   ").is_empty());
    }

    #[test]
    fn rewrite_special_chars_quoted() {
        let tiers = rewrite_query("fix OPS-1234 bug");
        // OPS-1234 gets quoted, fix and bug pass through
        assert_eq!(tiers.len(), 4);
        assert!(tiers[0].contains("\"OPS-1234\""));
        assert!(tiers[1].contains("\"OPS-1234\""));
    }

    #[test]
    fn rewrite_short_words_filtered() {
        // Single-char words filtered (len < 2)
        let tiers = rewrite_query("a b session");
        assert_eq!(tiers.len(), 2); // single term after filtering
        assert_eq!(tiers[0], "session");
    }
}
