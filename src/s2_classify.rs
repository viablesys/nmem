//! S2 Coordination — think/act phase classifier.
//!
//! Thin wrapper over `s2_inference` for the think/act dimension.
//! Loads exported model weights from `models/think-act.json`.

use crate::s2_inference::{self, ClassificationResult, load_or_get_model, classify_binary, Model};
use std::sync::OnceLock;

/// Classification result alias for backward compatibility.
pub type Phase = ClassificationResult;

static MODEL: OnceLock<Option<Model>> = OnceLock::new();

fn get_model() -> Option<&'static Model> {
    load_or_get_model(&MODEL, "think-act.json", "classifier")
}

/// Classify text as "think" or "act".
///
/// Returns `None` if model is not loaded (no model file found).
pub fn classify(text: &str) -> Option<Phase> {
    let model = get_model()?;
    let (label, confidence) = classify_binary(model, text);

    Some(ClassificationResult {
        label,
        confidence,
        model_hash: &model.hash,
    })
}

/// Get or create a classifier_run row. Re-exported from s2_inference.
pub fn ensure_classifier_run(
    conn: &rusqlite::Connection,
    name: &str,
    model_hash: &str,
    corpus_size: Option<i64>,
    cv_accuracy: Option<f64>,
    metadata: Option<&str>,
) -> Result<i64, crate::NmemError> {
    s2_inference::ensure_classifier_run(conn, name, model_hash, corpus_size, cv_accuracy, metadata)
}

/// Return the current model's hash, if loaded.
pub fn current_model_hash() -> Option<&'static str> {
    get_model().map(|m| m.hash.as_str())
}

/// Backfill phase labels for all observations with NULL phase.
pub fn handle_backfill(db_path: &std::path::Path, args: &crate::cli::BackfillArgs) -> Result<(), crate::NmemError> {
    s2_inference::generic_backfill(
        db_path,
        args,
        "phase",
        "classifier_run_id",
        "think-act",
        classify,
        current_model_hash,
    )
}
