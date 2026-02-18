# ADR-010: Work Unit Detection — Episodic Memory

## Status
Draft

## Framing
*How does nmem identify work units (episodes) within a session, and where does the boundary signal come from?*

Previous design (VSM.md §S4, ADR-009) assumed work unit boundaries would be detected from the observation stream — tool composition ratios, file clustering, phase transitions (investigation → execution). This ADR challenges that assumption. The observation stream records what the *agent* did. But the agent is the user's S1 — it executes, it doesn't decide. The work unit boundary is an S4-level signal that originates from the *user*, not the agent.

Adversarial angle: "What if tool patterns can't detect work units at all?" — is the observation stream the wrong place to look?

## Depends On
- ADR-009 (S4 Data Model) — defines available signals and capture gaps
- ADR-002 (Observation Extraction) — defines what S1 captures

## Unlocks
- Work unit summary generation (finer-grained than session summaries)
- Relevance-ordered context injection (inject the right past episode, not the most recent)
- `nmem learn` integration (cross-session episode recurrence)

---

## Context

### The VSM recursion

The user is a viable system. Claude Code is their S1 — operations. The user's prompts are S4 directives: control signals from a higher-order intelligence to its execution layer.

```
User (external VSM)
  └─ S4: decides intent → issues prompt
  └─ S1: Claude Code executes
         └─ nmem observes S1's execution
```

nmem observes the user's S1, not the user's S4. The S4 signal — intent, direction, work unit boundaries — arrives through **prompt text**. The observation stream (file reads, edits, commands) is the downstream execution of that intent. Useful for describing *what happened* within an episode, but the episode itself is defined by the user's intent shifts.

This reframes the detection problem. We're not looking for patterns in tool calls. We're looking for intent changes in user prompts.

### Why tool patterns fail

ADR-009 designed the detection signal as per-prompt tool composition:

```sql
SELECT prompt_id, obs_type, COUNT(*) as n
FROM observations WHERE session_id = ?1
GROUP BY prompt_id, obs_type
```

The ratio of `(file_read + search) : (file_edit + file_write + command)` per prompt was supposed to characterize work phases. Real data analysis (2026-02-18) shows why this doesn't work:

1. **Most user prompts have zero observations.** The user types "perhaps a decay, hotness/age" — no tool calls. The tool calls happen in the *agent's* turn (a separate prompt). The user prompt carries intent; the agent prompt carries execution.

2. **Phase transitions are noisy.** A session with 35 user prompts showed M→E→M→E→M oscillation at nearly every prompt. The "investigation → execution" phase pattern exists, but it's not clean enough to segment on.

3. **File overlap is a consequence, not a cause.** You can work on `s3_learn.rs` for three different reasons in one session. The file doesn't change; the episode does. Conversely, fixing a single bug might span 5 files — one episode, many files.

4. **Gaps between prompts aren't boundaries.** A 6-hour gap is sleep. A 5-minute gap is the user reading output. A 30-second gap is typing. Gap duration says nothing about whether intent changed. Only the prompt text does.

### What the user drives

The user's prompts contain the work unit structure directly. From a real session:

```
"please add the cargo test failures to the workspace claude.md"
  ← 47s →
"we should exclude the ../library"
  ← 117s →
"what is the histogram (monotonic) for all 3 signals"
```

Same work unit — refining `nmem learn`. File coherence would fail here (different files). Phase ratios would fail (all "mixed"). But the prompt text is unambiguous: all three prompts are about the same thing.

```
"ok push"
  ← 74s →
"so what signals are we trying to detect again, this list will probably grow"
```

New work unit — design discussion. No gap signal (74s is short). No file signal (no files in either). But the intent shift is clear in the text.

The user *declares* work unit boundaries by changing what they're asking about. The detection problem reduces to: **did the user's intent change between consecutive prompts?**

## The Two Positions

### Position A: Observation-driven detection (the ADR-009 design)

Detect work units from tool patterns — per-prompt composition ratios, file clustering, phase transitions.

**The case for it:**
- Doesn't require natural language understanding. Pure structured data.
- Works for automated sessions (dispatched tasks) where there's one prompt and many tool calls.
- The observation stream is rich (15+ obs_types, file paths, timestamps, failure flags).
- Aligns with the original VSM.md design for S4.

**The case against it:**
- Inverts the causal arrow. Tool patterns are effects of the user's intent, not the intent itself.
- Real data shows per-prompt ratios are noisy and most user prompts have zero observations.
- File coherence is a weak proxy — same file ≠ same episode, different files ≠ different episode.
- Requires complex heuristics (sliding windows, thresholds, smoothing) that are hard to validate.
- Fundamentally: trying to infer strategy from accounting. The ledger records execution, not decisions.

### Position B: Prompt-driven detection (narrative)

Detect work units from user prompt text — intent continuity between consecutive prompts.

**The case for it:**
- Reads the signal at the source. The user's prompt IS the S4 directive.
- The machinery already exists: `intent_keywords()` and `jaccard()` from `s3_learn` work at both intra-session (prompt pairs) and inter-session (summary intent clustering) scales.
- Simple: compare keyword bags of consecutive user prompts. Low Jaccard = boundary.
- Human-aligned: segments at intent shifts, which is how episodic memory works.
- Observation data becomes annotation (what happened during the episode) rather than boundary detection.

**The case against it:**
- Requires text comparison on short strings. User prompts can be terse ("yes", "ok push", "5,6"). Keyword bags on 2-3 word prompts are near-empty — Jaccard is undefined or noisy.
- Doesn't work for automated sessions with a single prompt.
- Jaccard on keyword bags is crude. "fix the auth bug" and "fix the build bug" share keywords ("fix", "bug") but are different work units. "refactor the middleware" and "clean up the auth handler" share no keywords but might be the same episode.
- Depends on the user writing descriptive prompts. A user who types "do it" repeatedly gives no signal.

### Adversarial stress test: terse prompts

The hardest case for Position B. From real data:

```
prompt 527: "5,6"
prompt 528: "i want the s1_extractor? to get tool responses when it is a fail"
```

"5,6" has no keywords. Jaccard with anything is 0 (or undefined). But it's a continuation — the user is selecting from options presented in the previous agent turn.

**Counter:** Terse prompts like "5,6", "yes", "ok push" are *continuations by definition*. They're responses to the agent's previous output, not new directives. A heuristic: prompts shorter than N words (e.g., 4) are continuations unless they follow a long gap. This catches "yes", "ok", "do it", "5,6", "push" without requiring keyword comparison.

The boundary signal comes from the *longer* prompts where the user states new intent. Short prompts are almost always within-episode.

## Decision

**Position B: prompt-driven detection with observation annotation.**

Rationale:
1. The user is the viable system. Their prompts are the authoritative signal for intent changes. Detecting work units from tool patterns is inferring strategy from the ledger.
2. The machinery exists. `s3_learn` already implements `intent_keywords()`, `jaccard()`, and keyword-bag clustering for inter-session patterns. The same functions work for intra-session boundary detection on consecutive prompts.
3. Observation data serves the episode after detection, not during. Phase ratios, hot files, and failure counts *describe* the episode. They don't define it.
4. The terse-prompt heuristic (short prompts = continuation) handles the adversarial case cleanly.

### The detection algorithm

```
for each user prompt in session:
    if prompt is short (< 5 words):
        continuation — same work unit
    else:
        keywords = intent_keywords(prompt)
        similarity = jaccard(keywords, current_episode_keywords)
        if similarity < threshold (0.3):
            close current episode
            open new episode with these keywords
        else:
            merge keywords into current episode
            continuation
```

The episode's keyword bag grows as new prompts are added — it accumulates the vocabulary of the work unit. The boundary fires when a prompt introduces substantially new vocabulary.

Gaps (time between prompts) serve as a secondary signal: lower the Jaccard threshold after a long gap (the user is more likely to have shifted after sleeping than after 30 seconds). But the verdict is always in the text.

### Observation annotation

After episodes are identified, each gets annotated from the observation stream:

```sql
-- Per-episode metadata, computed after boundary detection
SELECT
    COUNT(*) as obs_count,
    SUM(CASE WHEN obs_type IN ('file_read','search') THEN 1 ELSE 0 END) as investigate,
    SUM(CASE WHEN obs_type IN ('file_edit','file_write','command') THEN 1 ELSE 0 END) as execute,
    SUM(CASE WHEN json_extract(metadata, '$.failed') = 1 THEN 1 ELSE 0 END) as failures,
    GROUP_CONCAT(DISTINCT file_path) as hot_files
FROM observations
WHERE session_id = ?1
  AND prompt_id BETWEEN ?2 AND ?3  -- episode prompt range
```

This gives each episode: hot files, phase character (investigate-heavy vs execute-heavy), failure count, duration. Descriptive, not prescriptive.

### s3_learn integration

Intra-session and inter-session detection are the same machinery at different timescales:

| | Intra-session (this ADR) | Inter-session (s3_learn) |
|---|---|---|
| **Source** | User prompts | Session summary `intent` field |
| **Unit** | Work unit / episode | Cross-session pattern |
| **Boundary signal** | Intent shift between prompts | Intent recurrence across sessions |
| **Shared machinery** | `intent_keywords()`, `jaccard()` | Same |
| **Output** | `work_units` table | `learnings.md` report |

When a detected episode's intent keywords overlap with a known pattern from `nmem learn` (confirmed stuck loop, repeated failure), the episode inherits that context. This is signal multiplication: intra-session episode × inter-session pattern = actionable warning.

### s1_view: the inter-system channel

Per VSM.md's view pattern, S4 observes S1 through SQL views without coupling:

```sql
-- S4's view over S1's prompt table
CREATE VIEW user_intent_stream AS
SELECT
    p.id as prompt_id,
    p.session_id,
    p.timestamp,
    p.content,
    LENGTH(p.content) - LENGTH(REPLACE(p.content, ' ', '')) + 1 as word_count,
    p.timestamp - LAG(p.timestamp) OVER (
        PARTITION BY p.session_id ORDER BY p.id
    ) as gap_seconds
FROM prompts p
WHERE p.source = 'user';
```

S4 creates this view at initialization. `schema.rs` remains ignorant of S4's concerns. The view is the channel; the `work_units` table is S4's own state.

### Schema

```sql
CREATE TABLE work_units (
    id INTEGER PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    started_at INTEGER NOT NULL,
    ended_at INTEGER,
    intent TEXT,                -- accumulated keywords or representative prompt
    first_prompt_id INTEGER,   -- prompt range start
    last_prompt_id INTEGER,    -- prompt range end
    hot_files TEXT,            -- JSON: distinct file_paths from observations
    phase_signature TEXT,      -- JSON: {investigate: N, execute: N, failures: N}
    obs_count INTEGER,
    summary TEXT,              -- LLM-generated (optional, at session end)
    learned TEXT,              -- JSON array, from LLM or extracted from thinking blocks
    notes TEXT                 -- negative knowledge for this episode
);
```

This is the same schema as ADR-009 Q2 but with `first_prompt_id`/`last_prompt_id` replacing `source_obs_range` — because the episode is defined by prompts, not observations.

## Open Questions

### Q1: When does detection run?

Three options:
- **Inline (PostToolUse):** Check on every hook fire. Requires comparing the latest user prompt against the current episode. Cheap (one keyword comparison), but fires on every tool call when only user prompts matter.
- **UserPromptSubmit:** Check only when a new user prompt arrives. Natural trigger — the boundary signal is in the prompt, so check when a prompt appears. But UserPromptSubmit fires before tool execution, so the episode annotation (observations) isn't available yet.
- **Stop (retrospective):** Detect all episodes at session end. Full data available, single pass. But no mid-session signals.

Position: **UserPromptSubmit for boundary detection, Stop for annotation.** Detect the boundary when the prompt arrives (the signal is in the text, no observation data needed). Annotate with observation metadata at session end. This separates the fast path (boundary detection = keyword comparison) from the slow path (annotation = SQL aggregation + optional LLM).

### Q2: How do episodes feed context injection?

Current context injection (SessionStart) uses session summaries — one per session. Episodes are finer-grained. Options:
- **Replace session summaries with episode summaries** in context injection. More targeted, but more items to rank.
- **Session summaries remain primary, episodes available on demand** via MCP tool. Conservative, minimal change.
- **Hybrid: inject the last N episodes from the most recent session, plus session summaries for older sessions.** Recent work at episode resolution, older work at session resolution.

Deferred until episodes are generating data.

### Q3: What about automated sessions?

Dispatched tasks (`s4_dispatch.rs`) have a single prompt and no user interaction. The entire session is one episode by definition — no intent shifts to detect. The episode inherits the task prompt as its intent. No special handling needed; the detection algorithm produces one episode per session when there's one user prompt.

### Q4: Should the LLM generate episode summaries?

S1's S4 (session summarization) already generates `intent`, `learned`, `completed`, `notes` at session end. Episode-level summaries would be the same fields at finer granularity. Options:
- **No LLM for episodes.** Intent from accumulated keywords, hot files and phase from SQL. Cheaper, deterministic.
- **LLM at session end.** After detection, pass each episode's prompt text + observation summary to granite for structured summary. Same cost model as session summaries but per-episode.
- **LLM only for multi-prompt episodes.** Single-prompt episodes (most automated sessions, quick fixes) don't need narrative compression. Only episodes spanning 3+ prompts get LLM treatment.

Position C (threshold-based) seems right but deferred until episodes exist and we can measure the value-add.

## Consequences

### Positive
- **Reads the signal at the source.** User prompts carry intent directly. No inference from downstream effects.
- **Reuses existing machinery.** `intent_keywords()` and `jaccard()` from `s3_learn` work without modification.
- **Unifies intra and inter-session.** Same algorithm, different timescale. Episodes within sessions, patterns across sessions.
- **Observation data isn't wasted.** It annotates episodes with rich metadata (files, phases, failures) rather than driving detection.

### Negative
- **Depends on prompt quality.** Users who type "do it" repeatedly give no signal. Terse-prompt heuristic mitigates but doesn't eliminate.
- **Keyword bags are crude.** Semantic similarity isn't the same as keyword overlap. "Fix the auth bug" and "debug the login issue" share no keywords but are the same topic. A more sophisticated text comparison (embeddings, LLM classification) would catch this, at higher cost.
- **Single-prompt sessions collapse to one episode.** No sub-session structure for automated tasks. Acceptable — the task prompt defines the intent.

## References

- VSM.md §S4 — work unit concept, views as inter-system channels
- ADR-009 — S4 data model, signal gaps, signal multiplication
- `s3_learn.rs` — `intent_keywords()`, `jaccard()`, cross-session pattern detection
- `s1_record.rs` — hook handler, UserPromptSubmit handling
- `s1_context.rs` — context injection (consumer of episodes)

## Revision History

| Date | Version | Changes |
|------|---------|---------|
| 2026-02-18 | 0.1 | Initial draft. Reframes work unit detection from observation-driven (ADR-009 design) to prompt-driven (user intent). VSM recursion analysis: user as external viable system, agent as user's S1. Two positions evaluated, Position B (prompt-driven) accepted. Detection algorithm, schema, s3_learn integration, 4 open questions. |
