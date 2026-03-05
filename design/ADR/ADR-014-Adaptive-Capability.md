# ADR-014: Adaptive Capability

## Status
Draft

## Framing
*How does nmem handle capabilities that may be absent at install time, arrive later, and improve over time with the user's data?*

This is a requisite variety problem (Ashby's Law, VSM S5). The system's processing capacity must match its environment's complexity. But that environment isn't static — a user starts with Claude Code alone, later adds a local LLM, accumulates project-specific data that could improve classifiers. Capabilities appear, disappear, and improve over the lifetime of an installation. nmem must accumulate raw material at every capability level and exploit new capabilities when they arrive, without requiring reinstallation or data migration.

The adversarial angle: "What if a user runs nmem for six months without a local LLM, then adds one?" — is the accumulated data usable, or has the window closed?

## Depends On
- ADR-003 (Daemon Lifecycle) — constrains when processing can happen (no background daemon)
- ADR-008 (Distribution and Installation) — determines what ships with the binary vs what's user-provided
- ADR-010 (Work Unit Detection) — episode detection works without LLM, narrative generation requires one
- ADR-011 (Phase Classification) — classifier models trained on one user's data may not generalize

## Unlocks
- Summary backfill for installations that add LLM capability later
- Classifier retraining from user-specific data
- Graceful degradation as a first-class design property (not an accident)

---

## Context

### Two concrete gaps

**Gap 1: Classifier generalization.** The phase, scope, locus, and novelty classifiers (ADR-011, ADR-013) are trained on one user's nmem sessions — primarily Rust development on a single project. A user working in Python, Go, or JavaScript will generate observations with different vocabulary (different file extensions, different command patterns, different tool names). TF-IDF features like `cargo`, `.rs`, `clippy` are project-specific; features like `file_read`, `search`, `git commit` are generic. The current models conflate both.

**Gap 2: Unsummarized session accumulation.** Users who run nmem without a local LLM (Tier 0) accumulate sessions with `summary IS NULL`. The S3 sweep precondition (`summary IS NOT NULL`) already prevents these sessions from being garbage-collected — their observations persist indefinitely. When the user later adds a local LLM, there's no built-in path to backfill summaries for the accumulated sessions.

Both are instances of the same problem: nmem's intelligence layers depend on capabilities that may not be present, and the system must preserve the option to use those capabilities later.

### The VSM framing

In VSM terms, S2 (classifiers) and S1's S4 (summarization) are intelligence layers that enhance S1 (raw observation capture). S1 operates at every tier — it captures observations regardless of what intelligence is available. The intelligence layers add value but must not be prerequisites. S3 (retention) must not destroy data that a future intelligence layer could process.

This is variety engineering: S1 captures variety (raw observations). S2 and S4 compress variety (classifications, summaries, episodes). If the compressors aren't available yet, the raw variety must be preserved until they are.

## Three Capability Tiers

### Tier 0: No LLM (Claude Code only)

The user has Claude Code but no local LLM running. This is the minimum viable installation.

**Available:**
- Observation capture (S1) — full PostToolUse extraction
- Classifiers (S2) — embedded TF-IDF models, sub-millisecond inference
- Episode detection (S4) — prompt-driven boundary detection (ADR-010)
- Secret filtering (S5) — regex + entropy, no LLM needed
- Search and retrieval — FTS5, CLI, MCP server
- Context injection — episodes (without narratives), observation tables, suggested tasks from prior `next_steps`

**Unavailable:**
- Session summarization (S1's S4) — requires LLM
- Episode narratives (S4) — requires LLM
- LLM-assisted classifier relabeling — requires LLM

**What accumulates:** Raw observations, classifier labels (from embedded models), episodes (boundaries and `obs_trace`, but no narratives), sessions (with `summary IS NULL`).

### Tier 1: Local LLM available

The user runs a local LLM (e.g., LM Studio with Granite, Qwen, or similar). nmem's summarization pipeline activates.

**Adds to Tier 0:**
- Session summarization — Stop hook generates structured summaries via OpenAI-compatible API
- Episode narratives — natural language descriptions of what happened in each episode
- Summary streaming to VictoriaLogs (if configured)
- Backfill path for Tier 0 accumulated sessions

**Quality ceiling:** Local LLMs produce adequate summaries and narratives. They're sufficient for context reconstruction but may miss nuance in complex sessions. Classifier relabeling with a local LLM is possible but quality-limited (ADR-011 showed local LLMs plateau at 72-80% on phase classification).

### Tier 2: Local LLM + Claude

The user has both a local LLM and access to Claude (via Claude Code agent or API). This is the current development environment.

**Adds to Tier 1:**
- Frontier-quality labeling for classifier retraining — Claude as the labeler (ADR-011's Method A)
- Richer narrative generation — agent-driven episode annotation
- Cross-session pattern analysis with frontier-level understanding

**Quality ceiling:** Highest available. Frontier models understand the distinction between "deciding to do something" and "doing it" (ADR-011's key insight). This quality flows into training data, which flows into classifier accuracy.

## The Accumulation Invariant

**nmem accumulates raw material at every tier.** This is a design property, not an accident.

The S3 sweep precondition — `summary IS NOT NULL` — was introduced in the retention design to prevent data loss: observations from unsummarized sessions are never swept because the summary is the compressed representation that survives after observations are deleted. This precondition has a second, equally important function: it preserves unsummarized sessions for future processing.

A Tier 0 installation accumulates:
- Sessions with `summary IS NULL AND ended_at IS NOT NULL` — complete but unsummarized
- Episodes with `narrative IS NULL` — detected but unnarrated
- Observations with classifier labels from embedded models — may be re-labeled when better models arrive

None of this data is lost to retention. The sweep precondition ensures it persists until the intelligence layer that can process it becomes available.

**The invariant:** For any capability that nmem might gain in the future, the raw material needed to exercise that capability is preserved in the present. S1 captures everything. S3 doesn't delete what S4 hasn't processed.

## Backfill When Capability Appears

Three backfill paths, ordered by dependency:

### 1. Summary backfill

When a local LLM becomes available, backfill summaries for accumulated sessions:

```
nmem backfill-summaries
```

Scans for sessions where `summary IS NULL AND ended_at IS NOT NULL`. For each, reconstructs the session context from observations and prompts, runs the summarization pipeline (same as the Stop hook), and writes the summary. Once summarized, these sessions become eligible for S3 sweep.

**Constraint:** Backfill runs as a CLI command (ADR-003 — no daemon). The user triggers it manually or via a scheduled task. It's idempotent — re-running skips already-summarized sessions.

### 2. Episode narrative backfill

Episodes detected at Tier 0 have boundaries and `obs_trace` but no narratives. When an LLM becomes available:

```
nmem backfill --dimension narratives
```

Scans `work_units` where `narrative IS NULL`. For each, generates a narrative from the episode's `obs_trace`, phase_signature, and surrounding prompt context. Same LLM pipeline as the Stop hook's narrative generation.

### 3. Classifier retraining

When the user has accumulated enough project-specific observations, retrain classifiers from their own data:

1. Extract unlabeled observations: `python3 tools/classify-extract.py`
2. Label via available LLM (Tier 1: local LLM, Tier 2: Claude agent)
3. Train: `python3 tools/classify-train.py`
4. Deploy: copy model JSON to `~/.nmem/models/`

The existing training pipeline (ADR-011) already supports this workflow. What's new is the framing: retraining is not maintenance — it's adaptation. The user's accumulated observations are the training data for models that fit their specific environment.

## Classifier Generalization

### The domain adaptation problem

Current models are trained on one user's nmem sessions — predominantly Rust development on the nmem project itself. TF-IDF features split into two categories:

**Generic features** (transfer across projects):
- Tool patterns: `file_read`, `file_edit`, `search`, `command`, `git_commit`
- Action verbs: `create`, `delete`, `modify`, `read`, `write`
- Structural markers: `error`, `warning`, `test`, `build`, `install`

**Project-specific features** (don't transfer):
- File extensions: `.rs`, `.toml`, `.py`, `.ts`
- Module names: `s1_record`, `s4_context`, `schema`
- Tool names: `cargo`, `clippy`, `rustfmt`
- Path fragments: `src/`, `design/`, `models/`

A model trained on Rust sessions will have learned that `cargo test` correlates with a particular phase/scope. A Python user running `pytest` carries the same cognitive signal but uses different vocabulary. The generic features (tool patterns, action verbs) transfer; the project-specific features create noise.

### Three adaptation strategies

All operate within nmem's constraints: no daemon, no API calls in the hot path, sub-millisecond inference budget, training remains offline Python.

#### Strategy A: Confidence gating (immediate, no dependencies)

Below a confidence threshold, return `NULL` instead of a potentially wrong label.

The current classifiers always emit a label — the sigmoid output is thresholded at 0.5, and even a 0.51 confidence produces a classification. For a model encountering unfamiliar vocabulary, the sigmoid output will cluster near 0.5 (low confidence), producing unreliable labels.

**Implementation:** Add a configurable confidence threshold (default: 0.7). Below it, `classify()` returns `None`. The observation gets `phase = NULL` (or scope, locus, novelty). This is already the behavior when the model file is missing — extending it to low-confidence predictions is a natural generalization.

**Trade-off:** Reduces coverage (more NULLs) but increases precision. For episode phase_signature computation, fewer confident labels are more useful than many uncertain ones.

#### Strategy B: Self-training (no LLM needed)

Use high-confidence predictions from the existing model as pseudo-labels. Periodically retrain on the combined corpus (original training data + pseudo-labeled user data).

1. Accumulate observations where classifier confidence > 0.9
2. Extract these as pseudo-labeled training data
3. Combine with original training corpus
4. Retrain the model
5. Deploy updated model

**Trade-off:** Self-training can amplify systematic biases — if the model consistently misclassifies a pattern with high confidence, self-training reinforces the error. Mitigated by keeping the original training corpus as an anchor and limiting pseudo-labeled data to a fraction of the total.

**Constraint:** Requires the user to run the Python training pipeline. No automatic retraining — ADR-003 prohibits background processes.

#### Strategy C: LLM-assisted relabeling (Tier 1 or 2)

Use whichever LLM is available to label a sample of the user's observations, then retrain.

- **Tier 1 (local LLM):** Labels at 72-80% accuracy (ADR-011 baseline). Better than nothing, worse than frontier. Useful for project-specific vocabulary that the current model doesn't know at all.
- **Tier 2 (Claude):** Labels at ~95% accuracy. The same workflow as ADR-011's Method A (agent-driven corpus generation), applied to the user's own data.

**Trade-off:** Highest quality adaptation but requires the most user involvement (running the labeling pipeline). The ADR-011 tooling already exists — this strategy reuses it, not extends it.

### Recommended progression

1. **Ship confidence gating first** — immediate improvement, no pipeline changes, no user action required beyond a config setting.
2. **Document self-training as a user workflow** — for users who accumulate enough data and want to adapt without an LLM.
3. **Promote LLM-assisted relabeling for Tier 1/2 users** — the existing ADR-011 pipeline with the user's own observations as input.

## Constraints

What's ruled out by existing decisions:

| Constraint | Source | Implication |
|-----------|--------|-------------|
| No daemon | ADR-003 | Backfill and retraining are user-triggered CLI commands, not background jobs |
| No API calls in the hot path | ADR-003, ADR-011 | Inference remains embedded models only. LLM calls happen offline |
| No new dependencies | Project policy | Adaptation strategies use existing Cargo.toml dependencies |
| Training remains offline Python | ADR-011 | No Rust training code. Python + scikit-learn for model generation |
| Sub-millisecond inference budget | ADR-011 | Confidence gating adds one float comparison. No impact |
| Sweep precondition preserved | S3 design | `summary IS NOT NULL` remains the gate for observation deletion |

## Open Questions

### Q1: When to trigger retraining?

Three possible signals:
- **Observation count threshold** — after N observations from the user's project, suggest retraining. Simple but arbitrary.
- **Confidence degradation** — if the rolling average confidence drops below a threshold, the model is encountering unfamiliar territory. Requires tracking confidence over time.
- **User-initiated** — the user decides when to retrain. Simplest, most aligned with ADR-003 (no automation without explicit trigger).

Current lean: user-initiated, with a `nmem status` indicator that reports classifier confidence statistics to inform the decision.

### Q2: How to detect accuracy degradation without labeled ground truth?

Confidence is a proxy for accuracy but not a guarantee — a model can be confidently wrong. Without labeled ground truth from the user's domain, we can't measure actual accuracy.

Possible approaches:
- **Confidence distribution monitoring** — a healthy model has a bimodal confidence distribution (most predictions near 0 or 1). A degrading model shifts toward uniform (predictions cluster near 0.5).
- **Consistency checks** — classify the same observation text with multiple classifiers (phase, scope). Inconsistent combinations (e.g., a `git commit` classified as think+diverge) may indicate model confusion.
- **User spot-checks** — surface a sample of low-confidence predictions for the user to validate. Manual but ground-truth.

### Q3: Ship diverse training corpora vs self-training on user data?

Two paths to generalization:
- **Diverse corpora:** Train on observations from multiple users, languages, and project types. Ship a model that's broadly adequate but not specifically tuned to anyone.
- **Self-training:** Ship a model trained on the developer's data (current approach). Let each user adapt to their environment via the strategies above.

Current lean: self-training. A diverse corpus requires collecting observations from multiple environments, which conflicts with nmem's privacy model (observations contain file paths, code snippets, commands). The self-training path keeps all data local.

## Consequences

### Positive
- **Accumulated data is never wasted.** Tier 0 observations become Tier 1 summaries when the LLM arrives.
- **Graceful degradation is explicit.** Each tier has defined capabilities, not ad-hoc fallbacks.
- **Existing tooling reused.** Backfill and retraining use the same pipelines as initial training and summarization.
- **ADR-003 compliant.** All backfill and retraining operations are user-triggered CLI commands.
- **Privacy preserved.** No training data leaves the user's machine. Self-training uses only local observations.

### Negative
- **User burden.** Retraining and backfill require the user to run commands. No automatic adaptation.
- **Python dependency for retraining.** Users who want to retrain classifiers need Python + scikit-learn. This is already true (ADR-011) but becomes more visible when retraining is framed as a user workflow rather than a developer-only activity.
- **Confidence gating reduces coverage.** More NULLs in classifier labels means sparser phase_signatures and less informative episode characterization for users in unfamiliar domains.
- **No accuracy guarantee for self-training.** Pseudo-labels can reinforce systematic biases. Without ground truth, the user can't verify improvement.

## References

- ADR-003 — Daemon Lifecycle (runtime constraints)
- ADR-008 — Distribution and Installation (what ships with the binary)
- ADR-010 — Work Unit Detection (episode detection and narratives)
- ADR-011 — Phase Classification (classifier architecture, training pipeline)
- ADR-013 — Scope Classification (second classifier dimension, same architecture)
- Ashby, W.R. (1956) — Law of Requisite Variety
- Beer, S. (1972) — Brain of the Firm (VSM variety engineering)

## Revision History

| Date | Version | Changes |
|------|---------|---------|
| 2026-03-05 | 1.0 | Initial. Three capability tiers (no LLM / local LLM / local LLM + Claude). Accumulation invariant. Three backfill paths (summaries, narratives, classifiers). Three classifier adaptation strategies (confidence gating, self-training, LLM-assisted relabeling). Five open questions. |
