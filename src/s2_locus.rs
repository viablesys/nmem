//! S2 Coordination — internal/external locus classifier.
//!
//! Thin wrapper over `s2_inference` for the locus dimension.
//! Loads exported model weights from `models/internal-external.json`.

use crate::s2_inference::{self, ClassificationResult, load_or_get_model, classify_binary, Model};
use std::sync::OnceLock;

static MODEL: OnceLock<Option<Model>> = OnceLock::new();

fn get_model() -> Option<&'static Model> {
    load_or_get_model(&MODEL, "internal-external.json", "locus")
}

/// Classify text as "internal" or "external".
pub fn classify_locus(text: &str) -> Option<ClassificationResult> {
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

/// Backfill locus labels for all observations with NULL locus.
pub fn handle_backfill_locus(
    db_path: &std::path::Path,
    args: &crate::cli::BackfillArgs,
) -> Result<(), crate::NmemError> {
    s2_inference::generic_backfill(
        db_path,
        args,
        "locus",
        "locus_run_id",
        "internal-external",
        classify_locus,
        current_model_hash,
    )
}
