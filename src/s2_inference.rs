//! S2 Coordination — shared TF-IDF + LinearSVC inference engine.
//!
//! Pure Rust inference for binary text classifiers exported from
//! scikit-learn (TfidfVectorizer + LinearSVC). All classifier modules
//! (s2_classify, s2_scope, s2_locus, s2_novelty, s2_friction) delegate
//! to these shared types and functions.

use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

/// Classification result: label, confidence score, and model fingerprint.
#[derive(Debug, Clone)]
pub struct ClassificationResult {
    pub label: &'static str,
    pub confidence: f32,
    pub model_hash: &'static str,
}

/// Exported TF-IDF vectorizer weights for one feature set (word or char).
#[derive(Deserialize)]
pub(crate) struct VectorizerWeights {
    pub vocabulary: HashMap<String, usize>,
    pub idf: Vec<f64>,
    pub weights: Vec<f64>,
    pub ngram_range: [usize; 2],
    #[serde(default)]
    pub binary: bool,
    #[serde(default = "default_true")]
    pub sublinear_tf: bool,
}

fn default_true() -> bool {
    true
}

/// Full exported model (deserialized from JSON).
#[derive(Deserialize)]
struct ExportedModel {
    classes: Vec<String>,
    word: VectorizerWeights,
    char: VectorizerWeights,
    bias: f64,
}

/// Loaded model ready for inference.
pub(crate) struct Model {
    pub classes: Vec<String>,
    pub word: VectorizerWeights,
    pub char: VectorizerWeights,
    pub bias: f64,
    pub hash: String,
}

/// Compute a SipHash fingerprint of raw bytes, returned as 16-char hex string.
pub(crate) fn siphash_hex(data: &[u8]) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::hash::DefaultHasher::new();
    data.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn nmem_dir() -> Option<std::path::PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(|h| std::path::PathBuf::from(h).join(".nmem"))
}

/// Embedded model weights — compiled into the binary.
mod embedded {
    pub const THINK_ACT: &str = include_str!("../models/think-act.json");
    pub const CONVERGE_DIVERGE: &str = include_str!("../models/converge-diverge.json");
    pub const INTERNAL_EXTERNAL: &str = include_str!("../models/internal-external.json");
    pub const ROUTINE_NOVEL: &str = include_str!("../models/routine-novel.json");
}

/// Get embedded model data by filename.
fn embedded_model_data(filename: &str) -> Option<&'static str> {
    match filename {
        "think-act.json" => Some(embedded::THINK_ACT),
        "converge-diverge.json" => Some(embedded::CONVERGE_DIVERGE),
        "internal-external.json" => Some(embedded::INTERNAL_EXTERNAL),
        "routine-novel.json" => Some(embedded::ROUTINE_NOVEL),
        _ => None,
    }
}

/// Resolve model path by searching standard locations.
/// External files override embedded models (for development or model updates).
pub(crate) fn resolve_model_path(filename: &str) -> std::path::PathBuf {
    let candidates = [
        // ~/.nmem/models/ (user override)
        nmem_dir().map(|d| d.join("models").join(filename)),
        // Project root (development)
        Some(std::path::PathBuf::from("models").join(filename)),
    ];

    for candidate in candidates.into_iter().flatten() {
        if candidate.exists() {
            return candidate;
        }
    }

    // Fallback — will fail gracefully (embedded models handle this case)
    std::path::PathBuf::from("models").join(filename)
}

/// Load a model from raw JSON bytes.
fn parse_model(raw: &[u8]) -> Option<Model> {
    let hash = siphash_hex(raw);
    let exported: ExportedModel = serde_json::from_str(std::str::from_utf8(raw).ok()?).ok()?;

    Some(Model {
        classes: exported.classes,
        word: exported.word,
        char: exported.char,
        bias: exported.bias,
        hash,
    })
}

/// Load a model from a JSON file.
pub(crate) fn load_model_from(path: &Path) -> Option<Model> {
    let raw = std::fs::read(path).ok()?;
    parse_model(&raw)
}

/// Load or retrieve a cached model from a OnceLock.
/// Tries external file first (for overrides), falls back to embedded model.
pub(crate) fn load_or_get_model<'a>(
    lock: &'a OnceLock<Option<Model>>,
    filename: &str,
    log_name: &str,
) -> Option<&'a Model> {
    lock.get_or_init(|| {
        // Try external file first (allows overriding embedded models)
        let path = resolve_model_path(filename);
        if let Some(m) = load_model_from(&path) {
            eprintln!(
                "nmem: loaded {log_name} model from file ({} word + {} char features)",
                m.word.vocabulary.len(),
                m.char.vocabulary.len()
            );
            return Some(m);
        }

        // Fall back to embedded model
        if let Some(data) = embedded_model_data(filename) {
            if let Some(m) = parse_model(data.as_bytes()) {
                eprintln!(
                    "nmem: loaded {log_name} model (embedded, {} word + {} char features)",
                    m.word.vocabulary.len(),
                    m.char.vocabulary.len()
                );
                return Some(m);
            }
        }

        None
    })
    .as_ref()
}

/// Tokenize text into lowercase word tokens.
pub(crate) fn word_tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();

    for ch in text.chars() {
        if ch.is_alphanumeric() || ch == '_' {
            current.push(ch.to_ascii_lowercase());
        } else if !current.is_empty() {
            tokens.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }

    tokens
}

/// Generate word n-grams and count occurrences.
pub(crate) fn word_ngrams(tokens: &[String], lo: usize, hi: usize) -> HashMap<String, u32> {
    let mut ngrams = HashMap::new();

    for n in lo..=hi {
        if n > tokens.len() {
            continue;
        }
        for window in tokens.windows(n) {
            let gram = window.join(" ");
            *ngrams.entry(gram).or_insert(0) += 1;
        }
    }

    ngrams
}

/// Generate char_wb n-grams (whitespace-bounded character n-grams).
pub(crate) fn char_wb_ngrams(text: &str, lo: usize, hi: usize) -> HashMap<String, u32> {
    let mut ngrams = HashMap::new();
    let lower = text.to_lowercase();

    for word in lower.split_whitespace() {
        let padded = format!(" {word} ");
        let chars: Vec<char> = padded.chars().collect();

        for n in lo..=hi {
            if n > chars.len() {
                continue;
            }
            for window in chars.windows(n) {
                let gram: String = window.iter().collect();
                *ngrams.entry(gram).or_insert(0) += 1;
            }
        }
    }

    ngrams
}

/// Compute TF-IDF weighted dot product with model weights.
pub(crate) fn tfidf_score(
    ngrams: &HashMap<String, u32>,
    vw: &VectorizerWeights,
) -> f64 {
    let mut tfidf_pairs: Vec<(usize, f64)> = Vec::new();

    for (gram, &count) in ngrams {
        if let Some(&idx) = vw.vocabulary.get(gram) {
            let tf = if vw.binary {
                1.0
            } else if vw.sublinear_tf {
                (count as f64 + 1.0).ln()
            } else {
                count as f64
            };

            let tfidf = tf * vw.idf[idx];
            tfidf_pairs.push((idx, tfidf));
        }
    }

    // L2 normalize
    let norm_sq: f64 = tfidf_pairs.iter().map(|(_, v)| v * v).sum();
    if norm_sq == 0.0 {
        return 0.0;
    }
    let norm = norm_sq.sqrt();

    // Dot product with weights
    let mut score = 0.0;
    for (idx, val) in &tfidf_pairs {
        score += (val / norm) * vw.weights[*idx];
    }

    score
}

/// Classify text using a binary model. Returns (label, confidence).
/// The positive class is classes[1], negative is classes[0].
pub(crate) fn classify_binary<'a>(model: &'a Model, text: &str) -> (&'a str, f32) {
    let tokens = word_tokenize(text);
    let word_ng = word_ngrams(
        &tokens,
        model.word.ngram_range[0],
        model.word.ngram_range[1],
    );
    let char_ng = char_wb_ngrams(
        text,
        model.char.ngram_range[0],
        model.char.ngram_range[1],
    );

    let word_score = tfidf_score(&word_ng, &model.word);
    let char_score = tfidf_score(&char_ng, &model.char);

    let raw = word_score + char_score + model.bias;
    let prob = 1.0 / (1.0 + (-raw).exp());

    if prob >= 0.5 {
        (model.classes[1].as_str(), prob as f32)
    } else {
        (model.classes[0].as_str(), (1.0 - prob) as f32)
    }
}

/// Get or create a classifier_run row. Returns the run ID.
///
/// Uses INSERT OR IGNORE on (name, model_hash) to avoid duplicates,
/// then SELECT to get the ID.
pub fn ensure_classifier_run(
    conn: &rusqlite::Connection,
    name: &str,
    model_hash: &str,
    corpus_size: Option<i64>,
    cv_accuracy: Option<f64>,
    metadata: Option<&str>,
) -> Result<i64, crate::NmemError> {
    use rusqlite::params;

    let existing: Option<i64> = conn
        .query_row(
            "SELECT id FROM classifier_runs WHERE name = ?1 AND model_hash = ?2",
            params![name, model_hash],
            |r| r.get(0),
        )
        .ok();

    if let Some(id) = existing {
        return Ok(id);
    }

    conn.execute(
        "INSERT INTO classifier_runs (name, model_hash, corpus_size, cv_accuracy, metadata)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![name, model_hash, corpus_size, cv_accuracy, metadata],
    )?;

    Ok(conn.last_insert_rowid())
}

/// Generic backfill for any binary classifier dimension.
pub fn generic_backfill(
    db_path: &std::path::Path,
    args: &crate::cli::BackfillArgs,
    column: &str,
    run_id_column: &str,
    classifier_name: &str,
    classify_fn: fn(&str) -> Option<ClassificationResult>,
    model_hash_fn: fn() -> Option<&'static str>,
) -> Result<(), crate::NmemError> {
    use rusqlite::params;

    let conn = crate::db::open_db(db_path)?;

    let null_count: i64 = conn.query_row(
        &format!("SELECT COUNT(*) FROM observations WHERE {column} IS NULL"),
        [],
        |r| r.get(0),
    )?;

    if null_count == 0 {
        println!("No observations with NULL {column} — nothing to backfill.");
        return Ok(());
    }

    println!("Found {null_count} observations with NULL {column}.");

    if args.dry_run {
        println!("Dry run — no changes made.");
        return Ok(());
    }

    let run_id = match model_hash_fn() {
        Some(hash) => {
            let meta = args.metadata_json();
            let id = ensure_classifier_run(
                &conn,
                classifier_name,
                hash,
                args.corpus_size,
                args.cv_accuracy,
                meta.as_deref(),
            )?;
            println!("Classifier run #{id} (hash: {hash})");
            Some(id)
        }
        None => {
            eprintln!("Warning: no {classifier_name} model loaded, cannot backfill");
            return Ok(());
        }
    };

    let mut stmt = conn.prepare(
        &format!("SELECT id, content FROM observations WHERE {column} IS NULL"),
    )?;
    let rows: Vec<(i64, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .filter_map(|r| r.ok())
        .collect();

    let mut classified = 0i64;
    let mut counts: HashMap<String, i64> = HashMap::new();
    let mut skipped = 0i64;

    let update_sql = format!(
        "UPDATE observations SET {column} = ?1, {run_id_column} = ?2 WHERE id = ?3"
    );

    for chunk in rows.chunks(args.batch_size) {
        let tx = conn.unchecked_transaction()?;
        let mut update = tx.prepare_cached(&update_sql)?;

        for (id, content) in chunk {
            if let Some(result) = classify_fn(content) {
                update.execute(params![result.label, run_id, id])?;
                classified += 1;
                *counts.entry(result.label.to_string()).or_insert(0) += 1;
            } else {
                skipped += 1;
            }
        }
        drop(update);
        tx.commit()?;

        if classified % 500 == 0 && classified > 0 {
            println!("  ...{classified}/{null_count}");
        }
    }

    let counts_str: Vec<String> = counts.iter().map(|(k, v)| format!("{v} {k}")).collect();
    println!(
        "Backfilled {classified} observations ({}), {skipped} skipped.",
        counts_str.join(", ")
    );
    if let Some(rid) = run_id {
        println!("All tagged with {run_id_column} = {rid}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_word_tokenize() {
        let tokens = word_tokenize("fix the auth bug in src/main.rs");
        assert_eq!(tokens, vec!["fix", "the", "auth", "bug", "in", "src", "main", "rs"]);
    }

    #[test]
    fn test_word_tokenize_mixed_case() {
        let tokens = word_tokenize("Read FILE_PATH from Config");
        assert_eq!(tokens, vec!["read", "file_path", "from", "config"]);
    }

    #[test]
    fn test_word_ngrams() {
        let tokens: Vec<String> = vec!["fix", "the", "bug"].into_iter().map(String::from).collect();
        let ng = word_ngrams(&tokens, 1, 2);
        assert_eq!(ng.get("fix"), Some(&1));
        assert_eq!(ng.get("fix the"), Some(&1));
        assert_eq!(ng.get("the bug"), Some(&1));
        assert_eq!(ng.get("fix the bug"), None); // only up to bigrams
    }

    #[test]
    fn test_char_wb_ngrams() {
        let ng = char_wb_ngrams("fix", 3, 3);
        assert!(ng.contains_key(" fi"));
        assert!(ng.contains_key("fix"));
        assert!(ng.contains_key("ix "));
    }

    #[test]
    fn test_tfidf_score_empty() {
        let vw = VectorizerWeights {
            vocabulary: HashMap::new(),
            idf: vec![],
            weights: vec![],
            ngram_range: [1, 2],
            binary: false,
            sublinear_tf: true,
        };
        let ngrams = HashMap::new();
        assert_eq!(tfidf_score(&ngrams, &vw), 0.0);
    }

    #[test]
    fn test_tfidf_score_basic() {
        let mut vocab = HashMap::new();
        vocab.insert("fix".to_string(), 0);
        vocab.insert("bug".to_string(), 1);

        let vw = VectorizerWeights {
            vocabulary: vocab,
            idf: vec![1.5, 2.0],
            weights: vec![-0.5, -0.3],
            ngram_range: [1, 1],
            binary: true,
            sublinear_tf: false,
        };

        let mut ngrams = HashMap::new();
        ngrams.insert("fix".to_string(), 1);
        ngrams.insert("bug".to_string(), 1);

        let score = tfidf_score(&ngrams, &vw);
        assert!((score - (-0.54)).abs() < 1e-10);
    }

    #[test]
    fn test_sigmoid() {
        let raw = 0.0_f64;
        let prob = 1.0 / (1.0 + (-raw).exp());
        assert!((prob - 0.5).abs() < 1e-10);

        let raw = 2.0_f64;
        let prob = 1.0 / (1.0 + (-raw).exp());
        assert!(prob > 0.88);

        let raw = -2.0_f64;
        let prob = 1.0 / (1.0 + (-raw).exp());
        assert!(prob < 0.12);
    }
}
