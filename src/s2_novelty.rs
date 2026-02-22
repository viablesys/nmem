//! S2 Coordination — routine/novel novelty classifier.
//!
//! Thin wrapper over `s2_inference` for the novelty dimension.
//! Loads exported model weights from `models/routine-novel.json`.

use crate::s2_inference::{self, ClassificationResult, load_or_get_model, classify_binary, Model};
use std::sync::OnceLock;

static MODEL: OnceLock<Option<Model>> = OnceLock::new();

fn get_model() -> Option<&'static Model> {
    load_or_get_model(&MODEL, "routine-novel.json", "novelty")
}

/// Classify text as "routine" or "novel".
pub fn classify_novelty(text: &str) -> Option<ClassificationResult> {
    let model = get_model()?;
    let (label, confidence) = classify_binary(model, text);

    Some(ClassificationResult {
        label,
        confidence,
        model_hash: &model.hash,
    })
}

/// Return the current model's hash, if loaded.
pub fn current_model_hash() -> Option<&'static str> {
    get_model().map(|m| m.hash.as_str())
}

/// Backfill novelty labels for all observations with NULL novelty.
pub fn handle_backfill_novelty(
    db_path: &std::path::Path,
    args: &crate::cli::BackfillArgs,
) -> Result<(), crate::NmemError> {
    s2_inference::generic_backfill(
        db_path,
        args,
        "novelty",
        "novelty_run_id",
        "routine-novel",
        classify_novelty,
        current_model_hash,
    )
}
