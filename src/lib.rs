// Infrastructure (no prefix)
pub mod cli;
pub mod db;
pub mod metrics;
pub mod schema;
pub mod status;

// S1 Operations — capture, store, retrieve
pub mod s1_extract;
pub mod s1_git;
pub mod s1_lsp;
pub mod s1_mark;
pub mod s1_pin;
pub mod s1_record;
pub mod s1_search;
pub mod s1_serve;

// S2 Coordination — classification, dedup
pub mod s2_classify;
pub mod s2_inference;
pub mod s2_locus;
pub mod s2_novelty;
pub mod s2_scope;

// S1's S4 — session intelligence (VSM recursion within S1)
pub mod s1_4_summarize;
pub mod s1_4_transcript;

// S3 Control — retention, compaction, integrity
pub mod s3_learn;
pub mod s3_maintain;
pub mod s3_purge;
pub mod s3_sweep;

// S4 Intelligence — context injection, task dispatch, cross-session patterns, episodic memory
pub mod s4_context;
pub mod s4_dispatch;
pub mod s4_memory;

// S5 Policy — config, boundaries, identity
pub mod s5_config;
pub mod s5_filter;
pub mod s5_project;

// Backward-compat aliases — external code (main.rs, tests) can use old names
pub use s4_context as context;
pub use s1_extract as extract;
pub use s1_mark as mark;
pub use s1_pin as pin;
pub use s1_record as record;
pub use s1_search as search;
pub use s1_serve as serve;
pub use s1_4_summarize as summarize;
pub use s1_4_transcript as transcript;
pub use s3_learn as learn;
pub use s3_maintain as maintain;
pub use s3_purge as purge;
pub use s3_sweep as sweep;
pub use s4_dispatch as dispatch;
pub use s4_memory as memory;
pub use s5_config as config;
pub use s5_filter as filter;
pub use s5_project as project;

#[derive(Debug)]
pub enum NmemError {
    Database(rusqlite::Error),
    Io(std::io::Error),
    Json(serde_json::Error),
    Config(String),
}

impl std::fmt::Display for NmemError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NmemError::Database(e) => write!(f, "database: {e}"),
            NmemError::Io(e) => write!(f, "io: {e}"),
            NmemError::Json(e) => write!(f, "json: {e}"),
            NmemError::Config(msg) => write!(f, "config: {msg}"),
        }
    }
}

impl From<rusqlite::Error> for NmemError {
    fn from(e: rusqlite::Error) -> Self {
        NmemError::Database(e)
    }
}

impl From<std::io::Error> for NmemError {
    fn from(e: std::io::Error) -> Self {
        NmemError::Io(e)
    }
}

impl From<serde_json::Error> for NmemError {
    fn from(e: serde_json::Error) -> Self {
        NmemError::Json(e)
    }
}

impl From<rusqlite_migration::Error> for NmemError {
    fn from(e: rusqlite_migration::Error) -> Self {
        match e {
            rusqlite_migration::Error::RusqliteError { query: _, err } => NmemError::Database(err),
            other => NmemError::Config(format!("migration: {other}")),
        }
    }
}

pub fn schema_migrations() -> &'static rusqlite_migration::Migrations<'static> {
    &schema::MIGRATIONS
}

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
        w.contains('-') || w.contains(':') || w.contains('.') || w.contains('/')
            || w.contains('\\') || w.contains('(') || w.contains(')')
            || w.contains('{') || w.contains('}') || w.contains('[') || w.contains(']')
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

    if terms.is_empty() { None } else { Some(terms.join(" ")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_plain_word() {
        assert_eq!(sanitize_fts_query("cargo"), Some("cargo".into()));
    }

    #[test]
    fn sanitize_hyphenated_term() {
        // OPS-1234 must not become OPS NOT 1234
        assert_eq!(sanitize_fts_query("OPS-1234"), Some("\"OPS-1234\"".into()));
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
        // AND, OR, NOT should pass through as FTS5 operators
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
}
