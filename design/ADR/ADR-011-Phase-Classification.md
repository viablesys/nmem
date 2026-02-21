# ADR-011: Phase Classification — S2 Text Classifier

## Status
Accepted (labels renamed think/act 2026-02-21)

## Framing
*How does nmem classify observations and prompts as "think" or "act" at write time, without a daemon, without an LLM in the hot path, and without external dependencies?*

Phase classification enables episode detection (ADR-010), context prioritization, and session phase analysis. The question is where the classification intelligence lives: in a frontier LLM at inference time (expensive, accurate), in a local LLM (cheap, inaccurate), or in a trained statistical model (cheap, accurate, deterministic).

## Depends On
- ADR-002 (Observation Extraction) — defines what content is available for classification
- ADR-003 (Daemon Lifecycle) — constrains the runtime model (no daemon, process-only)
- ADR-010 (Work Unit Detection) — primary consumer of phase labels

## Unlocks
- Per-observation phase labels for episode phase_signature computation
- Session-level phase distribution analysis
- Context injection prioritization (plan-heavy episodes vs build-heavy episodes)

---

## Context

### The classification problem

Every observation in nmem carries an `obs_type` (file_read, file_edit, command, search, etc.) but not a *cognitive phase*. `obs_type` tells you *what tool was used*; phase tells you *what mode the session was in*. Reading a file during investigation (plan) is different from reading a file to verify a fix (build). The same tool can serve different phases.

Two classes:
- **think**: figuring out what to do — investigating, exploring, deciding, reviewing, diagnosing, asking questions, evaluating trade-offs
- **act**: doing the thing — implementing, executing, committing, writing code/docs, fixing bugs, creating files, running tests

### What was tried

Empirical testing across multiple approaches (2026-02-18):

| Approach | Accuracy | Build Recall | Latency | Runtime |
|----------|----------|--------------|---------|---------|
| Claude Haiku (frontier) | ~95% | ~93% | 200-500ms | API |
| Qwen3 30B (local LLM) | 78% | 63% | 800ms | GPU |
| Granite Tiny (local LLM) | 72% | 50% | 150ms | GPU |
| GLiClass zero-shot | 74% | 68% | 300ms | GPU |
| WordNet-bridged LLM | 70% | 55% | 200ms | GPU+CPU |

Local LLMs plateau at 72-80% accuracy with persistent think bias — they over-classify agent text (tool calls, file operations) as "think" because the surface form of agent reasoning looks investigative even when it's executing.

The frontier model works because it understands the distinction between *deciding to do something* and *doing it*. But calling an API on every PostToolUse hook violates ADR-003 (no daemon, no external dependencies in the hot path) and adds unacceptable latency.

### The insight

The user observed that since the frontier model outperforms all pretrained models, the training data problem can be solved by *generating unlimited labeled data from the frontier model*. This is the synthetic data approach: use the expensive accurate model offline to label a large corpus, then train a cheap fast model on that corpus.

## The Three Positions

### Position A: Skip classification entirely

Don't classify. Use `obs_type` as a proxy for phase: file_read/search → plan, file_edit/file_write/command → build.

**The case for it:**
- Zero complexity. No model, no training, no maintenance.
- `obs_type` correlates with phase reasonably well.
- Episode detection (ADR-010) already works from prompt text, not observation phases.

**The case against it:**
- `obs_type` is a weak proxy. A `command` running `cargo test` after fixing a bug is act; a `command` running `cargo test` to diagnose a failure is think. Same obs_type, different phase.
- Loses the phase_signature in work_units — no way to characterize episodes as investigation-heavy or execution-heavy.
- Context injection can't prioritize think-phase episodes (which contain decisions and trade-offs) over act-phase episodes (which contain implementation details).

### Position B: Frontier LLM at write time

Call Haiku on every observation via the Anthropic API.

**The case for it:**
- Highest accuracy (~95%).
- No training pipeline needed.

**The case against it:**
- Violates ADR-003. Each hook invocation is a standalone process with no daemon. An API call adds 200-500ms latency to every PostToolUse event.
- Requires network connectivity and an API key in the hook environment.
- Cost scales linearly with observation volume (~585K records/year from ADR-002).
- Single point of failure — API outage means no classification.

### Position C: Synthetic data + statistical model in Rust

Use the frontier model offline to label a large synthetic corpus. Train a TF-IDF + Logistic Regression model in Python. Export weights as JSON. Classify in pure Rust at inference time.

**The case for it:**
- Inference is deterministic, fast (<1ms), and requires no external dependencies.
- Model is a static JSON file (~100-500KB) loaded once per process.
- Training pipeline runs offline, decoupled from the hot path.
- Accuracy inherits from the frontier model's labeling quality, not from the statistical model's capacity — the model learns the frontier model's decision boundary, not the raw linguistic features.
- ADR-003 compliant: each hook process loads a file and does arithmetic. No daemon, no API, no GPU.

**The case against it:**
- Training pipeline complexity (Python scripts, scikit-learn dependency).
- Model must be retrained if the classification vocabulary evolves.
- TF-IDF + LogReg has a capacity ceiling — it learns surface patterns, not semantics. Frontier-level accuracy is unlikely.
- Requires maintaining the export format compatibility between Python (train) and Rust (inference).

## Decision

**Position C: synthetic data + TF-IDF/LogReg trained in Python, inference in Rust.**

> **[ANNOTATION 2026-02-21, v1.1]:** The production model is LinearSVC, not LogReg. This header reflects the original v1.0 decision which was updated in v1.1 (see Training Results). The inference math is identical (dot product + bias), but the model is an SVM, not logistic regression. The `s2_classify.rs` module doc comment has been updated to say "LinearSVC".

Rationale:
1. ADR-003 compliance is non-negotiable. No API calls, no daemon, no GPU in the hook path.
2. The accuracy floor from synthetic labeling is high — even if the statistical model captures 85% of the frontier model's decisions, that's better than any local LLM achieved.
3. The model is a static artifact. Updating it is retraining + replacing a JSON file, not changing code.
4. Graceful degradation — if the model file doesn't exist, phase is NULL. No classification is better than wrong classification.

## Architecture

### Training pipeline (offline, Python)

```
tools/classify-generate.py    → Synthetic corpus from DB + Haiku
tools/classify-train.py       → TF-IDF + LogReg → JSON export
tools/classify-eval.py        → Validation (LLM eval, model-file eval)
```

**Corpus generation** (`classify-generate.py`):
- Pulls real prompts from nmem DB (user + agent, diverse sessions)
- Classifies each via Anthropic API (Haiku) with confidence filtering (≥0.8)
- Balances to equal think/act counts
- Supports `--augment` for paraphrase expansion of existing corpus
- Output: `tools/corpus-synthetic-N.json`

**Training** (`classify-train.py`):
- FeatureUnion of two TF-IDF vectorizers:
  - Word n-grams (1-2), binary=True, max 3000 features
  - Char_wb n-grams (3-5), sublinear_tf, max 2000 features
- LinearSVC (C=1.0, balanced class weights)
- Stratified 5-fold cross-validation with per-class F1 reporting
- Exports coef_ and intercept_ directly (no calibration wrapper)
- Output: `models/think-act.json`

**Model format** (`models/think-act.json`):
```json
{
  "classes": ["act", "think"],
  "word": {
    "vocabulary": {"token": index, ...},
    "idf": [float, ...],
    "weights": [float, ...],
    "ngram_range": [1, 2],
    "binary": true,
    "sublinear_tf": true
  },
  "char": {
    "vocabulary": {"gram": index, ...},
    "idf": [float, ...],
    "weights": [float, ...],
    "ngram_range": [3, 5],
    "binary": false,
    "sublinear_tf": true
  },
  "bias": float
}
```

The format is self-describing: all parameters needed to reproduce the vectorization and classification are embedded. No external schema or version negotiation.

**Evaluation** (`classify-eval.py --model-file`):
- Loads the exported JSON model in Python
- Reimplements the same TF-IDF + sigmoid inference
- Validates against labeled corpus before Rust integration
- Catches export bugs in Python (where debugging is faster)

### Inference (hot path, Rust)

**Module:** `src/s2_classify.rs`

```
text → word_tokenize → word_ngrams → TF-IDF(word) → dot(weights) ─┐
                                                                      ├→ sum + bias → sigmoid → (label, confidence)
text → char_wb_ngrams → TF-IDF(char) → dot(weights) ─────────────┘
```

**Implementation details:**
- Model loaded via `OnceLock` on first use, cached for process lifetime
- Word tokenization: `\b\w+\b` equivalent, lowercase
- Char_wb tokenization: pad each whitespace-delimited word with spaces, extract character n-grams
- TF-IDF: term frequency (binary for word, sublinear for char) × IDF, L2-normalized
- Classification: dot product with weights + bias → sigmoid → threshold at 0.5
- Returns `Option<Phase>` — `None` if model file not found
- Labels: "think" (cognitive, investigative) / "act" (execution, implementation)
- No new crate dependencies (uses `serde_json` and `std::collections::HashMap`)

**Model file resolution order:**
1. `../../models/think-act.json` relative to binary (release builds)
2. `models/think-act.json` relative to cwd (development)
3. `~/.nmem/models/think-act.json` (user install)

### Integration point

In `s1_record.rs::handle_post_tool_use`, after content extraction and secret filtering:

```rust
let phase = s2_classify::classify(&filtered_content).map(|p| p.label);
```

Stored in the new `phase TEXT` column on observations (migration 7). Nullable — NULL means model not loaded or classification unavailable.

### Schema

Migration 7 in `schema.rs`:

```sql
ALTER TABLE observations ADD COLUMN phase TEXT;
```

No index on `phase` — it's a low-cardinality column (two values + NULL). Queries filtering by phase will use existing indexes and scan the phase column inline.

> **[ANNOTATION 2026-02-21, v1.1]:** A subsequent migration 8 added the `classifier_runs` table and `classifier_run_id` FK column on observations for provenance tracking. Each unique (name, model_hash) pair gets a row in `classifier_runs`, and every classified observation links back via `classifier_run_id`. This is implemented in `s2_classify.rs::ensure_classifier_run()` and called from `s1_record.rs::handle_post_tool_use()`.

## Design Properties

### Graceful degradation

The classifier is fully optional. If `models/think-act.json` doesn't exist:
- `classify()` returns `None`
- Phase is stored as NULL
- All existing functionality is unaffected
- Episode detection (ADR-010) works from prompt keywords, not observation phases

This means the release binary ships and works without a trained model. The model is an enhancement, not a dependency.

### Determinism

Given the same model file and input text, classification is deterministic. No randomness, no network calls, no GPU. Two hook processes classifying the same text will always produce the same result.

### Performance

Target: <1ms per classification. The hot path is:
1. HashMap lookups for vocabulary matching (~100-200 lookups per text)
2. Floating-point multiplication for TF-IDF (~100-200 multiplications)
3. L2 normalization (one sqrt)
4. Dot product with weights (~100-200 multiplications)
5. Sigmoid (one exp)

Model loading is amortized — `OnceLock` ensures the JSON file is parsed once per process. For hook invocations (one classification per process), the load cost dominates. For the MCP server (long-lived), it's a one-time cost.

### Retraining

Two corpus generation methods:

**Method A: Agent-driven (no API keys)**
1. Extract: `python3 tools/classify-extract.py --limit 500 --output /tmp/unlabeled.json`
2. Agent reads batches, classifies each as think/act, writes labels JSON
3. Ingest: `python3 tools/classify-ingest.py --extracted /tmp/unlabeled.json --labels /tmp/labels.json --output tools/corpus-think-act-N.json`
4. Train: `python3 tools/classify-train.py --corpus tools/corpus-think-act-N.json`
5. Validate: `python3 tools/classify-eval.py --model-file models/think-act.json --corpus tools/corpus-think-act-N.json`

**Method B: API-driven (requires ANTHROPIC_API_KEY)**
1. Generate: `python3 tools/classify-generate.py --limit 1000`
2. Train: `python3 tools/classify-train.py --corpus tools/corpus-synthetic-1000.json`
3. Validate: `python3 tools/classify-eval.py --model-file models/think-act.json --corpus tools/corpus-think-act-200.json`

**Augmentation** (either method): generate paraphrases of existing corpus entries to expand the training set. Agent-driven augmentation (3 paraphrases per entry) took a 176-entry corpus to 692 entries and improved CV accuracy from 80.7% to 89.7%.

No code changes required for retraining. The model is data, not logic.

### Training results (2026-02-21)

| Corpus | Size | Model | CV Accuracy | Think F1 | Act F1 |
|--------|------|-------|-------------|----------|--------|
| Agent-labeled only | 176 | CalibratedLR | 80.7% | 0.811 | 0.802 |
| + augmentation (3x paraphrase) | 692 | CalibratedLR | 89.7% | 0.897 | 0.898 |
| Same corpus | 692 | LinearSVC C=1.0 | 95.5% | — | — |
| + 10x augmentation + fresh labels | 7648 | LinearSVC C=1.0 | **98.8%** | 0.988 | 0.988 |

**Key findings:**
- LinearSVC outperforms CalibratedClassifierCV(LR) by 4-5 points — SVM margin maximization beats likelihood maximization on high-dimensional TF-IDF features.
- Ensemble experiments (bagging 3/5/7 estimators) showed no improvement — linear models on TF-IDF features lack diversity for ensemble benefit.
- CalibratedClassifierCV costs ~1% accuracy on small corpora by reducing effective training data via internal 3-fold splits.
- Data volume is the primary lever: 176→692→7648 entries drove 80.7%→89.7%→98.8%.
- Current production model: LinearSVC on 7648 balanced entries (3824 think + 3824 act).

### Model distribution (Q3 revisited)

Current: sidecar JSON file loaded at runtime. `include_str!` embedding is a one-line change once the model stabilizes. During active iteration, sidecar allows hot-swapping without recompilation.

## File Manifest

| File | Layer | Purpose |
|------|-------|---------|
| `tools/classify-extract.py` | Tooling | Extract unlabeled prompts from DB for agent labeling |
| `tools/classify-ingest.py` | Tooling | Merge agent labels with extracted text, balance corpus |
| `tools/classify-generate.py` | Tooling | Synthetic corpus generation (DB + Haiku API) |
| `tools/classify-train.py` | Tooling | Train TF-IDF/LogReg, export JSON |
| `tools/classify-eval.py` | Tooling | Evaluation (LLM + model-file modes) |
| `models/think-act.json` | Data | Exported model weights (277 KB, 5000 features) |
| `src/s2_classify.rs` | S2 | Rust-native TF-IDF + linear inference |
| `src/s1_record.rs` | S1 | Integration: calls classifier, stores phase |
| `src/schema.rs` | Infra | Migration 7: `phase TEXT` column |

## Open Questions

### Q1: Should prompts also be classified?

Currently only observations (PostToolUse) are classified. User prompts and agent thinking blocks in the `prompts` table don't get phase labels. For episode phase_signature computation, prompt-level classification might give better signal than observation-level — the user's prompt *is* the intent signal.

Deferred: ADR-010's prompt-driven boundary detection already reads intent from prompt text directly. Adding phase to prompts is additive, not blocking.

### Q2: What accuracy is the floor for usefulness?

If the trained model achieves 80% accuracy, is that useful? The phase_signature in work_units aggregates across all observations in an episode — individual misclassifications wash out in the aggregate. A 10-observation episode with 80% accuracy will have ~8 correct phase labels, enough to characterize the episode as plan-heavy or build-heavy.

Position: 80% is the floor. Below that, phase_signature noise exceeds signal.

### Q3: Should the model ship with the binary?

Three distribution options:
1. **Bundled** — embed model JSON in the binary via `include_str!`. Always available, but increases binary size and requires rebuild to update model.
2. **Sidecar** — ship model as a separate file. Current approach. Flexible, but requires the file to be in the right place.
3. **Generated on first run** — auto-train from the user's own data. Self-bootstrapping, but requires Python + scikit-learn on the user's machine.

Current: sidecar (option 2). Revisit if distribution (ADR-008) demands a single-file binary.

## Consequences

### Positive
- **ADR-003 compliant.** No daemon, no API, no GPU in the hot path. Each hook process does arithmetic on a loaded JSON file.
- **Deterministic.** Same input, same model, same output. No stochastic classification.
- **Graceful.** Model not found → NULL phase → everything still works.
- **Cheap.** Sub-millisecond inference. No token costs, no network latency.
- **Decoupled training.** Model quality improves by retraining with better data, not by changing code.

### Negative
- **Training pipeline is Python.** Offline tooling requires scikit-learn, numpy, and (for corpus generation) the Anthropic API. Not pure Rust end-to-end.
- **Capacity ceiling.** TF-IDF + LogReg learns surface patterns. "investigate" → think, "fix" → act. It won't understand that "reading the file to verify the fix worked" is act, not think. Semantic nuance is lost.
- **Model maintenance.** If the classification vocabulary evolves (new phases, refined definitions), the model must be retrained. The JSON format is stable, but the training data must be regenerated.
- **Cold start.** First hook invocation pays the JSON parse cost (~1-5ms for a 500KB file). Subsequent classifications within the same process are free (OnceLock), but hook processes are short-lived.

## References

- ADR-002 — Observation extraction (what content is available)
- ADR-003 — Daemon lifecycle (runtime constraints)
- ADR-010 — Work unit detection (primary consumer of phase labels)
- `~/workspace/library/sklearn-text-classification.md` — TF-IDF pipeline patterns
- `~/workspace/library/setfit.md` — Alternative few-shot approach (not selected)
- `~/workspace/library/sentence-transformers.md` — Embedding approach (not selected)

## Revision History

| Date | Version | Changes |
|------|---------|---------|
| 2026-02-18 | 1.0 | Initial. Three positions evaluated (skip, frontier LLM, synthetic+statistical). Position C accepted. Architecture: Python training pipeline + Rust inference. Empirical accuracy table from testing. File manifest, 3 open questions. |
| 2026-02-21 | 1.1 | Labels renamed plan/build → think/act. Added agent-driven corpus generation tools (classify-extract.py, classify-ingest.py). Switched from CalibratedLR to LinearSVC. 10x corpus expansion (7648 entries) → 98.8% CV accuracy. |
