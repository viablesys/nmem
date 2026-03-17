---
name: Ensembled Research
description: This skill should be used when the user asks to "research a topic", "create a library doc", "write a reference document", "ensemble research", "research with multiple agents", or wants comprehensive, unbiased research on any topic. Spawns multiple independent researchers on the same topic with no pre-assigned angles, then ensembles results into a single document using independent agreement as a quality signal.
---

# Ensembled Research

Produce high-quality reference documents by spawning multiple independent researchers on the same topic, then synthesizing their outputs into a single document using ensemble principles. The key insight: independent agreement across researchers is a strong quality signal, while unique findings from a single researcher are preserved but weighted accordingly.

## Why This Works

- **Zero confirmation bias** — no researcher is primed by another's framing or angle
- **No pre-slicing** — each researcher explores the full topic independently, choosing their own path through the search space
- **Diversity of paths** — different researchers find different sources, emphasize different aspects, and structure differently
- **Probabilistic surfacing** — details that N of M researchers independently converge on are high-confidence; unique findings are still captured

## Workflow

### Phase 1: Spawn Independent Researchers

Spawn N researchers (default: 5) in parallel using the Task tool. All receive the **identical prompt** with no assigned angles or aspects.

**Researcher prompt template:**
```
Research [TOPIC] — [brief description]. Use web search, Context7, and any sources
available to build a comprehensive reference document in markdown.

IMPORTANT: When retrieving web content, use `curl -sL <url> | sed 's/<[^>]*>//g'`
via the Bash tool instead of WebFetch. WebFetch summarizes content through a small
model, likely cached — all researchers would receive the same lossy summary,
destroying independence at the retrieval layer. curl returns raw content that you
interpret independently, which is what makes ensemble agreement meaningful.

Cover whatever is most important and useful for a developer who will be using [TOPIC]
in production with [RELEVANT STACK]. Don't limit yourself to any particular aspect —
cover architecture, APIs, patterns, deployment, client libraries, configuration,
persistence, security, clustering, whatever is relevant. Include code examples, version
numbers, and practical pitfalls.

Return findings as a single markdown document.
```

**Spawn configuration:**
- `subagent_type`: `general-purpose`
- `model`: `sonnet` (cost-efficient for research volume)
- `run_in_background`: `true` (all run concurrently)
- Give each a sequential name: `researcher-1` through `researcher-N`

### Phase 2: Collect Outputs

As researchers complete, save each output to a named file:
- `[topic]-researcher-1.md` through `[topic]-researcher-N.md`
- Store in the target directory (e.g., `~/workspace/library/`)

Monitor progress via output file sizes. Researchers typically produce 100-200KB of raw output including tool call logs.

### Phase 3: Correlation

Before synthesis, cross-reference key facts across all researcher outputs to identify divergences. Build a correlation table for the facts with the **largest variation** across researchers.

**What to correlate:**
- Version numbers (library versions, server versions, API versions)
- Performance claims (throughput, latency, resource requirements)
- Feature availability (which version introduced what)
- Configuration defaults (default values, limits, timeouts)
- Dates (deprecation dates, release dates, enforcement deadlines)

**How to present:**
Create a table with researchers as columns and facts as rows. Flag discrepancies:
- All agree → high confidence
- Most agree, 1-2 outliers → investigate outliers
- Wide disagreement → must fact-check before proceeding

**Unique finds:**
Note any content that only a single researcher found. These are either:
- Genuine unique valuable discoveries (e.g., a CVE, a production tuning parameter)
- Potential hallucinations that need verification

Present the correlation analysis to the user or use it to inform the next phase.

### Phase 4: Fact-Checking

For each divergence identified in correlation, verify against authoritative sources using web search. Focus on:

**Highest priority (always check):**
- Version numbers where researchers disagree — search crates.io, PyPI, GitHub releases, official docs
- Performance claims that vary by >2x — search official benchmarks, distinguish measurement methodologies
- Feature claims made by only one researcher — search official release notes, changelogs, ADRs
- Dates and deadlines — search official announcements

**Methodology:**
- Use `WebSearch` to find URLs, then `curl -sL <url> | sed 's/<[^>]*>//g'` to retrieve raw content — do not use `WebFetch`, which summarizes through a cached small model and introduces systematic bias shared across all researchers
- Target authoritative sources: official docs, package registries, GitHub releases, official blogs
- When a fact is confirmed, note the source
- When a fact is contradicted, note the correction and which researchers were wrong

**Common hallucination patterns:**
- Version numbers slightly behind actual (researchers find stale docs)
- ALL researchers wrong on the same fact (stale information propagated across sources)
- Mixing up measurement contexts (e.g., throughput-derived latency vs end-to-end latency)
- Features attributed to wrong version

**Output:**
Produce a corrections table:

| Fact | Researchers Said | Reality | Source | Action |
|---|---|---|---|---|
| Library version | 0.35-0.37 | 0.46.0 | crates.io | Correct all |
| Binary size | 10-20MB | <20MB binary, <10MB Docker | Official FAQ | Distinguish both |

### Phase 5: Ensemble Synthesis

With correlation and fact-checking complete, synthesize into a single document. Apply these principles:

**Convergence weighting:**
- Content that appears in 4+ of 5 researchers: high confidence, include with full detail
- Content in 2-3 researchers: moderate confidence, include with appropriate depth
- Content in only 1 researcher: evaluate — if it's a verified unique valuable finding, include; if unverified and suspicious, omit

**Fact corrections:**
- Apply all corrections from Phase 4 — do not propagate known-wrong facts even if most researchers stated them
- Distinguish measurement contexts (e.g., "this number measures X, not Y")

**Structure selection:**
- Compare how each researcher organized the material
- Select the most logical structure, or synthesize a better one from the best aspects of each
- Prefer structures that match existing library docs in style and depth

**Code examples:**
- When multiple researchers provide examples for the same concept, select the most correct and idiomatic one
- If examples differ in approach, include the one that best fits the target stack
- Cross-validate code examples against each other for correctness
- Apply version corrections (e.g., correct dependency versions in Cargo.toml/go.mod/requirements.txt)

**Pitfalls section:**
- Union of all pitfalls found across researchers — this is where unique findings are most valuable
- A pitfall mentioned by only one researcher is still worth including (pitfalls are low-risk to include, high-cost to miss)
- Deduplicate pitfalls that multiple researchers stated differently

### Phase 6: Produce Final Document

Write the ensembled result as the final document (e.g., `nats.md`). Clean up the individual researcher files unless the user wants to keep them.

Update any indexes (e.g., CLAUDE.md library table) with the new document entry.

## Tuning Parameters

| Parameter | Default | Notes |
|-----------|---------|-------|
| Researcher count | 5 | More researchers = more diversity but higher cost. 5 is a good balance. |
| Model | sonnet | Cost-efficient for research. Use opus for highly technical topics. |
| Prompt variation | None | All researchers get identical prompts. Variation defeats the purpose. |

## When to Use More or Fewer Researchers

- **3 researchers**: Well-documented, narrow topics (single library, single API)
- **5 researchers**: Broad topics, systems with many components (messaging systems, databases, frameworks)
- **7+ researchers**: Critical reference docs where accuracy matters most, topics with conflicting information online

## Anti-Patterns

- **Pre-slicing topics** — assigning "you cover X, you cover Y" introduces bias and eliminates the ensemble benefit
- **Sharing results between researchers** — defeats independence
- **Using different prompts** — variation in prompts means differences in output reflect prompt differences, not genuine diversity of findings
- **Skipping the ensemble step** — just concatenating outputs misses the whole point; synthesis with convergence weighting is where quality emerges
