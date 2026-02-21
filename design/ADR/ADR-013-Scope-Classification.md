# ADR-013: Scope Classification — Converge/Diverge Dimension

## Status
Accepted (2026-02-21)

## Framing
*How does nmem classify whether an observation broadens or narrows the solution space?*

Phase classification (ADR-011) captures the cognitive mode — think or act. But within each mode, work can either *diverge* (broadening the search space, exploring options, opening new files) or *converge* (narrowing to a solution, targeting specific edits, closing out work). Scope classification adds this second dimension, enabling a 4-quadrant episode schema that captures both cognitive mode and directionality.

## Depends On
- ADR-011 (Phase Classification) — provides the think/act dimension and the validated TF-IDF + LinearSVC pipeline
- ADR-002 (Observation Extraction) — defines what content is available for classification
- ADR-010 (Work Unit Detection) — primary consumer of scope labels via episode modality

## Unlocks
- 4-quadrant episode schema (think×diverge, think×converge, act×diverge, act×converge)
- Richer phase_signature incorporating scope distribution
- Episode modality labels (e.g., "exploration", "implementation", "debugging", "finishing")

---

## Context

### The classification problem

ADR-011 established that every observation carries a *phase* (think or act), but phase alone doesn't capture the full shape of cognitive work. Within "think", exploring five different modules to understand a system is qualitatively different from diagnosing a specific hypothesis about a single function. Within "act", scaffolding new files across a project is different from making targeted edits to fix a known bug.

The missing dimension is *scope* — whether work is broadening or narrowing the solution space.

### Research validation

Research across multiple domains validates converge/diverge as orthogonal to think/act:

- **Cognitive science** — the explore/exploit tradeoff is a fundamental dimension of decision-making, independent of whether the agent is planning or executing.
- **Software engineering activity studies** — navigate/edit/test/inspect classifications capture directionality (broad navigation vs. targeted editing) separately from cognitive mode.
- **Dialogue act theory** (ISO 24617-2) — communicative functions distinguish between information-seeking (divergent) and information-providing (convergent) acts, orthogonal to task vs. feedback dimensions.
- **HuggingFace datasets** — labeled coding activity corpora show that scope and phase are independently predictable from surface text features.

### The 4-quadrant model

|  | diverge | converge |
|---|---------|----------|
| **think** | Exploring problem space — reading unfamiliar code, broad searches, comparing approaches | Diagnosing specific hypothesis — targeted reads, narrowing down root cause |
| **act** | Scaffolding across files — creating new modules, writing boilerplate, building structure | Finishing — targeted edits, committing, pushing, fixing specific lines |

Each quadrant has a distinct character that episode detection can leverage:
- **think×diverge**: exploration episodes — high file_read count, many distinct directories, broad glob patterns
- **think×converge**: diagnosis episodes — repeated reads of the same files, narrowing search patterns
- **act×diverge**: scaffolding episodes — many file_write/file_edit across new files, directory creation
- **act×converge**: finishing episodes — few targeted edits, git commits, test re-runs

### Definitions

- **diverge**: broadening search space — new files, new directories, broad glob patterns, web searches for options, reading unfamiliar code, exploring multiple approaches, creating new modules, scaffolding across files
- **converge**: narrowing to solution — targeted edits, specific file reads to verify, committing, re-running tests, fixing specific lines, pushing, diagnosing a single hypothesis, closing out work

## The Three Positions

### Position A: Derive from obs_type heuristic (no model)

Map obs_type directly to scope: file_read → diverge, file_edit → converge, search → diverge, git_commit → converge.

**The case for it:**
- Zero complexity. No model, no training pipeline.
- Fast to implement — a match statement in Rust.
- obs_type correlates with scope reasonably well in the common case.

**The case against it:**
- Loses intent signal. Reading a file to verify a fix is converge, not diverge. The same obs_type maps to different scopes depending on context.
- Same fundamental weakness identified in ADR-011 Position A — obs_type tells you *what tool was used*, not *what cognitive direction the work was heading*.
- No way to improve accuracy without changing the heuristic code.

### Position B: Rule-based on file_path patterns

Use file-level heuristics: new file creation → diverge, re-editing a file already edited this session → converge, reading a file in a new directory → diverge.

**The case for it:**
- Captures some context that obs_type alone misses.
- No model training required.
- Moderate accuracy for well-structured sessions.

**The case against it:**
- Requires session state tracking — the rule engine must know which files have already been touched, which directories are "new", etc.
- Brittle. Edge cases proliferate: is editing a test file converge (fixing a test) or diverge (adding tests for a new module)?
- Complexity grows with each edge case addressed, approaching model-level maintenance cost without model-level accuracy.

### Position C: TF-IDF + LinearSVC classifier (same architecture as think/act)

Reuse the validated pipeline from ADR-011. Train a second model on converge/diverge labels using the same architecture: TF-IDF FeatureUnion (word + char_wb n-grams) + LinearSVC, exported as JSON, inference in pure Rust.

**The case for it:**
- Proven pipeline. ADR-011 demonstrated that this architecture reaches 98.8% on think/act. The training tools, export format, and Rust inference code are all validated.
- Learnable from surface text. Tool call descriptions, file paths, search patterns, and command text contain strong lexical signals for scope (e.g., "glob **/*.rs" → diverge, "edit line 47" → converge).
- Same retraining workflow — agent-driven labeling, no API keys required.
- No session state tracking needed — classification is per-observation, stateless.

**The case against it:**
- Second model to maintain (training data, weights, accuracy monitoring).
- Converge/diverge boundary may be fuzzier than think/act — the surface text signal may be weaker, requiring more training data or different features.
- Two classifiers run per observation (though both are <1ms).

## Decision

**Position C: reuse the validated pipeline.** Same architecture (TF-IDF FeatureUnion + LinearSVC), same export format, same Rust inference code structure.

Rationale:
1. The pipeline is proven. ADR-011's progression from 80.7% to 98.8% demonstrates that the architecture works and that accuracy scales with data volume.
2. Reuse minimizes new code. The Rust inference logic in `s2_classify.rs` already implements TF-IDF + linear classification. A second model uses the same code path with different weights.
3. Stateless classification avoids the complexity of tracking session-level file history for rule-based approaches.
4. Graceful degradation — model not found → NULL scope → everything works. Same pattern as phase classification.

## Architecture

### Training pipeline (offline, Python)

Reuses existing tools with converge/diverge labels:

1. Extract: `python3 tools/classify-extract.py --limit 500 --output /tmp/unlabeled-scope.json`
2. Agent labels each observation as converge or diverge using `tools/scope-label-prompt.md`
3. Ingest: `python3 tools/classify-ingest.py --extracted /tmp/unlabeled-scope.json --labels /tmp/scope-labels.json --output tools/corpus-converge-diverge-N.json`
4. Augment: agent paraphrases (10x) to expand corpus
5. Train: `python3 tools/classify-train.py --corpus tools/corpus-converge-diverge-N.json --output models/converge-diverge.json`

### Inference (hot path, Rust)

**Module:** `src/s2_scope.rs`

- Second `OnceLock` for scope model, independent of phase model in `s2_classify.rs`
- Loads `models/converge-diverge.json` using the same model format as `models/think-act.json`
- Same inference path: text → word TF-IDF → char TF-IDF → dot product → sigmoid → (label, confidence)
- Returns `Option<Scope>` — `None` if model file not found

**Model file resolution order:**
1. `../../models/converge-diverge.json` relative to binary (release builds)
2. `models/converge-diverge.json` relative to cwd (development)
3. `~/.nmem/models/converge-diverge.json` (user install)

### Integration point

In `s1_record.rs::handle_post_tool_use`, after phase classification:

```rust
let scope = s2_scope::classify(&filtered_content).map(|s| s.label);
```

> **[ANNOTATION 2026-02-21, v1.0]:** The actual function name is `s2_scope::classify_scope()`, not `s2_scope::classify()`. In `s1_record.rs` line 270: `let scope_result = s2_scope::classify_scope(&filtered_content);`

### Schema

Migration 9 in `schema.rs`:

```sql
ALTER TABLE observations ADD COLUMN scope TEXT;
ALTER TABLE observations ADD COLUMN scope_run_id INTEGER REFERENCES classifier_runs(id);
```

Reuses the `classifier_runs` table (introduced for phase classification) with `name = "converge-diverge"`.

No index on `scope` — same reasoning as phase: low-cardinality column (two values + NULL). Queries filtering by scope use existing indexes and scan inline.

## Design Properties

### Graceful degradation

The scope classifier is fully optional. If `models/converge-diverge.json` doesn't exist:
- `classify()` returns `None`
- Scope is stored as NULL
- All existing functionality is unaffected
- Episode detection, phase classification, and context injection work without scope

### Orthogonality

Scope classification is independent of phase classification. The two models:
- Load separate weight files
- Use separate `OnceLock` instances
- Run independently on the same input text
- Store results in separate columns

Either can be present without the other. Both NULL, one NULL, or both populated are all valid states.

### Deterministic inference

Same model + same input → same output. No randomness, no network calls, no GPU. Two hook processes classifying the same text produce the same scope label.

### Performance

Target: <1ms per classification, same as phase classifier. The inference path is identical — HashMap lookups, floating-point multiplication, L2 normalization, dot product, sigmoid. Combined cost of both classifiers: <2ms per observation.

## File Manifest

| File | Layer | Purpose |
|------|-------|---------|
| `src/s2_scope.rs` | S2 | Rust-native TF-IDF + linear inference for scope |
| `tools/scope-label-prompt.md` | Tooling | Labeling instructions for agents |
| `models/converge-diverge.json` | Data | Exported model weights |
| `src/schema.rs` | Infra | Migration 9: scope TEXT, scope_run_id |

## Open Questions

### Q1: Should the scope classifier use the same feature dimensions as think/act, or different n-gram ranges?

The think/act model uses word n-grams (1-2, 3000 features) + char_wb n-grams (3-5, 2000 features). Converge/diverge may benefit from different feature extraction — for example, file path patterns might carry stronger signal for scope than for phase.

Position: start with identical architecture. Tune feature dimensions only if accuracy is low after initial training. The pipeline makes experimentation cheap — changing n-gram ranges is a config change in classify-train.py.

### Q2: Is the converge/diverge boundary sharp enough for a linear model?

Think/act had clear signal in surface text — words like "investigate", "explore", "understand" vs. "fix", "commit", "write". Converge/diverge may require more context: reading a file is diverge if it's a new file, converge if it's a file being re-checked after an edit. The surface text of the observation alone may not carry this signal.

Position: start with surface text, measure accuracy, iterate. If accuracy plateaus below 85%, consider enriching input features with metadata (e.g., prepending obs_type or file_path to the text before classification). This doesn't change the model architecture — it changes the input preprocessing.

## Consequences

### Positive
- **4-quadrant episodes.** Phase × scope gives four distinct episode modalities, enabling richer context injection and session analysis.
- **Proven pipeline.** No architectural risk — the TF-IDF + LinearSVC architecture is validated at 98.8% accuracy on the phase dimension.
- **Graceful.** Model not found → NULL scope → everything still works. Same degradation pattern as phase.
- **Cheap.** Sub-millisecond inference. Combined phase + scope classification adds <2ms per observation.
- **Decoupled training.** Model quality improves by retraining with better data, not by changing code.

### Negative
- **Second model to maintain.** Two training corpora, two weight files, two accuracy targets. Maintenance cost is additive, not multiplicative (same tools, same format).
- **Scope boundary may be fuzzier.** Converge/diverge is more context-dependent than think/act. The same observation text may be converge or diverge depending on session history. A surface-text-only classifier may hit a lower accuracy ceiling.
- **Two cold starts.** Each hook process loads two model files on first classification. Combined JSON parse cost ~2-10ms on first invocation. Amortized to zero for long-lived MCP server process.

## References

- ADR-011 — Phase Classification (think/act dimension, TF-IDF + LinearSVC pipeline)
- ADR-002 — Observation Extraction (what content is available for classification)
- ADR-010 — Work Unit Detection (primary consumer of scope labels via episode modality)

## Revision History

| Date | Version | Changes |
|------|---------|---------|
| 2026-02-21 | 1.0 | Initial. Three positions evaluated (obs_type heuristic, rule-based, TF-IDF + LinearSVC). Position C accepted — reuse validated pipeline from ADR-011. 4-quadrant model (think/act × converge/diverge). Schema: migration 9 adds scope TEXT + scope_run_id. File manifest, 2 open questions. |
