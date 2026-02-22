//! S2 Coordination — smooth/friction classifier.
//!
//! Thin wrapper over `s2_inference` for the friction dimension.
//! Loads exported model weights from `models/smooth-friction.json`.

use crate::s2_inference::{self, ClassificationResult, load_or_get_model, classify_binary, Model};
use std::sync::OnceLock;

static MODEL: OnceLock<Option<Model>> = OnceLock::new();

fn get_model() -> Option<&'static Model> {
    load_or_get_model(&MODEL, "smooth-friction.json", "friction")
}

/// Classify text as "smooth" or "friction".
pub fn classify_friction(text: &str) -> Option<ClassificationResult> {
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

/// Backfill friction labels for all observations with NULL friction.
pub fn handle_backfill_friction(
    db_path: &std::path::Path,
    args: &crate::cli::BackfillArgs,
) -> Result<(), crate::NmemError> {
    s2_inference::generic_backfill(
        db_path,
        args,
        "friction",
        "friction_run_id",
        "smooth-friction",
        classify_friction,
        current_model_hash,
    )
}
