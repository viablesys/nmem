//! S2 Coordination — converge/diverge scope classifier.
//!
//! Thin wrapper over `s2_inference` for the converge/diverge dimension.
//! Loads exported model weights from `models/converge-diverge.json`.

use crate::s2_inference::{self, ClassificationResult, load_or_get_model, classify_binary, Model};
use std::sync::OnceLock;

/// Classification result alias for backward compatibility.
pub type Scope = ClassificationResult;

static SCOPE_MODEL: OnceLock<Option<Model>> = OnceLock::new();

fn get_model() -> Option<&'static Model> {
    load_or_get_model(&SCOPE_MODEL, "converge-diverge.json", "scope")
}

/// Classify text as "converge" or "diverge".
///
/// Returns `None` if model is not loaded (no model file found).
pub fn classify_scope(text: &str) -> Option<Scope> {
    let model = get_model()?;
    let (label, confidence) = classify_binary(model, text);

    Some(ClassificationResult {
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
    s2_inference::generic_backfill(
        db_path,
        args,
        "scope",
        "scope_run_id",
        "converge-diverge",
        classify_scope,
        current_scope_model_hash,
    )
}
