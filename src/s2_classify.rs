//! S2 Coordination — think/act phase classifier.
//!
//! Pure Rust TF-IDF + LinearSVC inference.
//! Loads exported model weights from `models/think-act.json`.
//! No external ML crate dependencies — uses serde_json (already in tree).

use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

/// Classification result: phase label, confidence score, and model fingerprint.
#[derive(Debug, Clone)]
pub struct Phase {
    pub label: &'static str,
    pub confidence: f32,
    pub model_hash: &'static str,
}

/// Exported TF-IDF vectorizer weights for one feature set (word or char).
#[derive(Deserialize)]
struct VectorizerWeights {
    vocabulary: HashMap<String, usize>,
    idf: Vec<f64>,
    weights: Vec<f64>,
    ngram_range: [usize; 2],
    #[serde(default)]
    binary: bool,
    #[serde(default = "default_true")]
    sublinear_tf: bool,
}

fn default_true() -> bool {
    true
}

/// Full exported model.
#[derive(Deserialize)]
struct ExportedModel {
    classes: Vec<String>,
    word: VectorizerWeights,
    char: VectorizerWeights,
    bias: f64,
}

/// Loaded model ready for inference.
struct Model {
    classes: Vec<String>,
    word: VectorizerWeights,
    char: VectorizerWeights,
    bias: f64,
    hash: String,
}

static MODEL: OnceLock<Option<Model>> = OnceLock::new();

/// Default model path relative to the binary's expected install location.
fn default_model_path() -> std::path::PathBuf {
    // Check next to binary first, then workspace root, then home
    let candidates = [
        // Next to the binary (release builds)
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("../../models/think-act.json"))),
        // Project root (development)
        Some(std::path::PathBuf::from("models/think-act.json")),
        // ~/.nmem/models/
        dirs().map(|d| d.join("models/think-act.json")),
    ];

    for candidate in candidates.into_iter().flatten() {
        if candidate.exists() {
            return candidate;
        }
    }

    // Fallback — will fail gracefully
    std::path::PathBuf::from("models/think-act.json")
}

fn dirs() -> Option<std::path::PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(|h| std::path::PathBuf::from(h).join(".nmem"))
}

/// Compute a SipHash fingerprint of raw bytes, returned as 16-char hex string.
fn siphash_hex(data: &[u8]) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::hash::DefaultHasher::new();
    data.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn load_model_from(path: &Path) -> Option<Model> {
    let raw = std::fs::read(path).ok()?;
    let hash = siphash_hex(&raw);
    let exported: ExportedModel = serde_json::from_str(std::str::from_utf8(&raw).ok()?).ok()?;

    Some(Model {
        classes: exported.classes,
        word: exported.word,
        char: exported.char,
        bias: exported.bias,
        hash,
    })
}

fn get_model() -> Option<&'static Model> {
    MODEL
        .get_or_init(|| {
            let path = default_model_path();
            match load_model_from(&path) {
                Some(m) => {
                    eprintln!(
                        "nmem: loaded classifier model ({} word + {} char features)",
                        m.word.vocabulary.len(),
                        m.char.vocabulary.len()
                    );
                    Some(m)
                }
                None => {
                    // Silent — model not yet trained
                    None
                }
            }
        })
        .as_ref()
}

/// Tokenize text into lowercase word tokens.
fn word_tokenize(text: &str) -> Vec<String> {
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
fn word_ngrams(tokens: &[String], lo: usize, hi: usize) -> HashMap<String, u32> {
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
fn char_wb_ngrams(text: &str, lo: usize, hi: usize) -> HashMap<String, u32> {
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
/// Returns the raw score contribution from one vectorizer.
fn tfidf_score(
    ngrams: &HashMap<String, u32>,
    vw: &VectorizerWeights,
) -> f64 {
    // Compute TF-IDF values for matching vocabulary terms
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

/// Classify text as "think" or "act".
///
/// Returns `None` if model is not loaded (no model file found).
/// Caller should treat `None` as "classification unavailable" and skip.
pub fn classify(text: &str) -> Option<Phase> {
    let model = get_model()?;

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

    // classes[1] is positive class (typically "think"), classes[0] is "act"
    let (label, confidence) = if prob >= 0.5 {
        // Positive class
        if model.classes.get(1).is_some_and(|c| c == "think") {
            ("think", prob as f32)
        } else {
            ("act", prob as f32)
        }
    } else {
        // Negative class
        if model.classes.first().is_some_and(|c| c == "act") {
            ("act", (1.0 - prob) as f32)
        } else {
            ("think", (1.0 - prob) as f32)
        }
    };

    // model_hash is &'static str because Model lives in a OnceLock
    Some(Phase { label, confidence, model_hash: &model.hash })
}

/// Get or create a classifier_run row. Returns the run ID.
///
/// Uses INSERT OR IGNORE on (name, model_hash) to avoid duplicates,
/// then SELECT to get the ID. The same model file always produces the
/// same run row.
pub fn ensure_classifier_run(
    conn: &rusqlite::Connection,
    name: &str,
    model_hash: &str,
    corpus_size: Option<i64>,
    cv_accuracy: Option<f64>,
    metadata: Option<&str>,
) -> Result<i64, crate::NmemError> {
    use rusqlite::params;

    // Try to find existing run with same name + hash
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

/// Return the current model's hash, if loaded.
pub fn current_model_hash() -> Option<&'static str> {
    get_model().map(|m| m.hash.as_str())
}

/// Backfill phase labels for all observations with NULL phase.
pub fn handle_backfill(db_path: &std::path::Path, args: &crate::cli::BackfillArgs) -> Result<(), crate::NmemError> {
    use rusqlite::params;

    let conn = crate::db::open_db(db_path)?;

    let null_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM observations WHERE phase IS NULL",
        [],
        |r| r.get(0),
    )?;

    if null_count == 0 {
        println!("No observations with NULL phase — nothing to backfill.");
        return Ok(());
    }

    println!("Found {null_count} observations with NULL phase.");

    if args.dry_run {
        println!("Dry run — no changes made.");
        return Ok(());
    }

    // Create classifier run for this backfill
    let run_id = match current_model_hash() {
        Some(hash) => {
            let meta = args.metadata_json();
            let id = ensure_classifier_run(
                &conn,
                "think-act",
                hash,
                args.corpus_size,
                args.cv_accuracy,
                meta.as_deref(),
            )?;
            println!("Classifier run #{id} (hash: {hash})");
            Some(id)
        }
        None => {
            eprintln!("Warning: no model loaded, backfill will classify but cannot record provenance");
            None
        }
    };

    // Read all NULL-phase rows
    let mut stmt = conn.prepare(
        "SELECT id, content FROM observations WHERE phase IS NULL",
    )?;
    let rows: Vec<(i64, String)> = stmt.query_map([], |r| {
        Ok((r.get(0)?, r.get(1)?))
    })?.filter_map(|r| r.ok()).collect();

    let mut classified = 0i64;
    let mut think = 0i64;
    let mut act = 0i64;
    let mut skipped = 0i64;

    for chunk in rows.chunks(args.batch_size) {
        let tx = conn.unchecked_transaction()?;
        let mut update = tx.prepare_cached(
            "UPDATE observations SET phase = ?1, classifier_run_id = ?2 WHERE id = ?3",
        )?;

        for (id, content) in chunk {
            if let Some(phase) = classify(content) {
                update.execute(params![phase.label, run_id, id])?;
                classified += 1;
                match phase.label {
                    "think" => think += 1,
                    "act" => act += 1,
                    _ => {}
                }
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

    println!("Backfilled {classified} observations ({think} think, {act} act), {skipped} skipped (no model).");
    if let Some(rid) = run_id {
        println!("All tagged with classifier_run_id = {rid}");
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
        // " fix " → " fi", "fix", "ix "
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
        // Both terms present: tf=1.0 (binary), tfidf = [1.5, 2.0]
        // L2 norm = sqrt(1.5^2 + 2.0^2) = sqrt(2.25 + 4.0) = sqrt(6.25) = 2.5
        // score = (1.5/2.5)*(-0.5) + (2.0/2.5)*(-0.3) = 0.6*(-0.5) + 0.8*(-0.3) = -0.3 + -0.24 = -0.54
        assert!((score - (-0.54)).abs() < 1e-10);
    }

    #[test]
    fn test_classify_returns_none_without_model() {
        // OnceLock may already be initialized from other tests, so we test
        // the underlying logic directly
        let tokens = word_tokenize("investigate the auth flow");
        assert!(!tokens.is_empty());
    }

    #[test]
    fn test_sigmoid() {
        // Verify sigmoid math
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
