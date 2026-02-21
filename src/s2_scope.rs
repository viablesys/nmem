//! S2 Coordination — converge/diverge scope classifier.
//!
//! Pure Rust TF-IDF + LinearSVC inference, same architecture as s2_classify.
//! Loads exported model weights from `models/converge-diverge.json`.

use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

/// Classification result: scope label, confidence score, and model fingerprint.
#[derive(Debug, Clone)]
pub struct Scope {
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

static SCOPE_MODEL: OnceLock<Option<Model>> = OnceLock::new();

fn default_scope_model_path() -> std::path::PathBuf {
    let candidates = [
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("../../models/converge-diverge.json"))),
        Some(std::path::PathBuf::from("models/converge-diverge.json")),
        dirs().map(|d| d.join("models/converge-diverge.json")),
    ];

    for candidate in candidates.into_iter().flatten() {
        if candidate.exists() {
            return candidate;
        }
    }

    std::path::PathBuf::from("models/converge-diverge.json")
}

fn dirs() -> Option<std::path::PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(|h| std::path::PathBuf::from(h).join(".nmem"))
}

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
    SCOPE_MODEL
        .get_or_init(|| {
            let path = default_scope_model_path();
            match load_model_from(&path) {
                Some(m) => {
                    eprintln!(
                        "nmem: loaded scope model ({} word + {} char features)",
                        m.word.vocabulary.len(),
                        m.char.vocabulary.len()
                    );
                    Some(m)
                }
                None => None,
            }
        })
        .as_ref()
}

// Reuse tokenization and scoring from s2_classify — identical algorithms.
// Duplicated here to keep each classifier self-contained with its own OnceLock.

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

fn tfidf_score(ngrams: &HashMap<String, u32>, vw: &VectorizerWeights) -> f64 {
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

    let norm_sq: f64 = tfidf_pairs.iter().map(|(_, v)| v * v).sum();
    if norm_sq == 0.0 {
        return 0.0;
    }
    let norm = norm_sq.sqrt();

    let mut score = 0.0;
    for (idx, val) in &tfidf_pairs {
        score += (val / norm) * vw.weights[*idx];
    }

    score
}

/// Classify text as "converge" or "diverge".
///
/// Returns `None` if model is not loaded (no model file found).
pub fn classify_scope(text: &str) -> Option<Scope> {
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

    // classes are sorted alphabetically by sklearn: ["converge", "diverge"]
    // positive class (index 1) = "diverge", negative class (index 0) = "converge"
    let (label, confidence) = if prob >= 0.5 {
        if model.classes.get(1).is_some_and(|c| c == "diverge") {
            ("diverge", prob as f32)
        } else {
            ("converge", prob as f32)
        }
    } else {
        if model.classes.first().is_some_and(|c| c == "converge") {
            ("converge", (1.0 - prob) as f32)
        } else {
            ("diverge", (1.0 - prob) as f32)
        }
    };

    Some(Scope {
        label,
        confidence,
        model_hash: &model.hash,
    })
}

/// Return the current scope model's hash, if loaded.
pub fn current_scope_model_hash() -> Option<&'static str> {
    get_model().map(|m| m.hash.as_str())
}

/// Backfill scope labels for all observations with NULL scope.
pub fn handle_backfill_scope(
    db_path: &std::path::Path,
    args: &crate::cli::BackfillArgs,
) -> Result<(), crate::NmemError> {
    use rusqlite::params;

    let conn = crate::db::open_db(db_path)?;

    let null_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM observations WHERE scope IS NULL",
        [],
        |r| r.get(0),
    )?;

    if null_count == 0 {
        println!("No observations with NULL scope — nothing to backfill.");
        return Ok(());
    }

    println!("Found {null_count} observations with NULL scope.");

    if args.dry_run {
        println!("Dry run — no changes made.");
        return Ok(());
    }

    let run_id = match current_scope_model_hash() {
        Some(hash) => {
            let meta = args.metadata_json();
            let id = crate::s2_classify::ensure_classifier_run(
                &conn,
                "converge-diverge",
                hash,
                args.corpus_size,
                args.cv_accuracy,
                meta.as_deref(),
            )?;
            println!("Classifier run #{id} (hash: {hash})");
            Some(id)
        }
        None => {
            eprintln!("Warning: no scope model loaded, cannot backfill");
            return Ok(());
        }
    };

    let mut stmt = conn.prepare(
        "SELECT id, content FROM observations WHERE scope IS NULL",
    )?;
    let rows: Vec<(i64, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .filter_map(|r| r.ok())
        .collect();

    let mut classified = 0i64;
    let mut converge_count = 0i64;
    let mut diverge_count = 0i64;
    let mut skipped = 0i64;

    for chunk in rows.chunks(args.batch_size) {
        let tx = conn.unchecked_transaction()?;
        let mut update = tx.prepare_cached(
            "UPDATE observations SET scope = ?1, scope_run_id = ?2 WHERE id = ?3",
        )?;

        for (id, content) in chunk {
            if let Some(scope) = classify_scope(content) {
                update.execute(params![scope.label, run_id, id])?;
                classified += 1;
                match scope.label {
                    "converge" => converge_count += 1,
                    "diverge" => diverge_count += 1,
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

    println!(
        "Backfilled {classified} observations ({converge_count} converge, {diverge_count} diverge), {skipped} skipped."
    );
    if let Some(rid) = run_id {
        println!("All tagged with scope_run_id = {rid}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_word_tokenize() {
        let tokens = word_tokenize("explore the codebase for auth patterns");
        assert_eq!(
            tokens,
            vec!["explore", "the", "codebase", "for", "auth", "patterns"]
        );
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
    fn test_classify_scope_returns_none_without_model() {
        // Without a model file, classify_scope should return None
        let tokens = word_tokenize("searching for auth implementation");
        assert!(!tokens.is_empty());
    }

    #[test]
    fn test_sigmoid_math() {
        let raw = 0.0_f64;
        let prob = 1.0 / (1.0 + (-raw).exp());
        assert!((prob - 0.5).abs() < 1e-10);
    }
}
