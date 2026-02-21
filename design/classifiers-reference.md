# Classifier Reference — from claude-mem

Reference extracted from `~/dev/bpd1069/claude-mem/` for evaluation.
claude-mem uses two orthogonal dimensions: **type** (what kind of work) and **concept** (what kind of understanding).

## Code Mode — Observation Types

| ID | Description |
|----|-------------|
| `bugfix` | Something was broken, now fixed |
| `feature` | New capability or functionality added |
| `refactor` | Code restructured, behavior unchanged |
| `change` | Generic modification (docs, config, misc) |
| `discovery` | Learning about existing system |
| `decision` | Architectural/design choice with rationale |

## Code Mode — Observation Concepts

| ID | Description |
|----|-------------|
| `how-it-works` | Understanding mechanisms |
| `why-it-exists` | Purpose or rationale |
| `what-changed` | Modifications made |
| `problem-solution` | Issues and their fixes |
| `gotcha` | Traps or edge cases |
| `pattern` | Reusable approach |
| `trade-off` | Pros/cons of a decision |

## Email Investigation Mode — Observation Types

| ID | Description |
|----|-------------|
| `entity` | New person, organization, or email address identified |
| `relationship` | Connection between entities discovered |
| `timeline-event` | Time-stamped event in communication sequence |
| `evidence` | Supporting documentation or proof discovered |
| `anomaly` | Suspicious pattern or irregularity detected |
| `conclusion` | Investigative finding or determination |

## Email Investigation Mode — Observation Concepts

| ID | Description |
|----|-------------|
| `who` | People and organizations involved |
| `when` | Timing and sequence of events |
| `what-happened` | Events and communications |
| `motive` | Intent or purpose behind actions |
| `red-flag` | Warning signs of fraud or deception |
| `corroboration` | Evidence supporting a claim |

## Key Design Choices

- Types and concepts are **orthogonal dimensions** — a single observation has one type and multiple concepts
- Types are mode-specific — different modes define different type vocabularies
- Concepts tag *what kind of understanding* is captured, independent of *what kind of work* produced it
- Parser validates against active mode; invalid types fall back to first in list
- The `discovery` type uses a different work_emoji (magnifying glass vs wrench) — distinguishing investigation from execution at the UI level

## Relevance to nmem

nmem currently classifies at the **mechanical** level (what tool ran) plus a **cognitive** level (think/act phase). claude-mem classifies at the **semantic** level (what the work means). The concept dimension is particularly interesting — it captures understanding type independently of work type. A `bugfix` can involve `how-it-works` understanding just as much as a `discovery` can.

### nmem S2 phase classifier (shipped 2026-02-18, renamed 2026-02-21)

nmem's phase classifier uses two labels:
- **think**: figuring out what to do — investigating, exploring, deciding, reviewing, diagnosing
- **act**: doing the thing — implementing, executing, committing, writing, fixing, testing

This is a coarser taxonomy than claude-mem's type/concept grid, but it's the right granularity for a TF-IDF + LogReg model. The classifier runs in pure Rust at sub-millisecond latency from a 279 KB JSON weight file. Training uses agent-driven corpus generation (no API keys needed) — 10 parallel agents label extracted prompts, 8 agents augment via paraphrase, sklearn trains and exports.

Current accuracy: 89.7% CV (5-fold stratified, 692-entry augmented corpus). Ensemble experiments (bagging 3-7 estimators) showed no improvement over single model for this feature space.
