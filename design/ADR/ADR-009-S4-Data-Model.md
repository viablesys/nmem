# ADR-009: S4 Data Model — Signals for Work Unit Detection

## Status
Draft (mid-development)

## Framing
*What data does S4 need to detect work unit boundaries, and where does it come from?*

S4's core algorithm (VSM.md) detects work unit boundaries from the observation stream — pattern shifts in the ratio of investigation to execution, intent changes, file clustering. This ADR asks: what signals feed that detection, which are already captured, which need to be added, and which are blocked on platform constraints?

This is a data model question, not an algorithm question. The algorithm is designed (VSM.md §S4). The question is whether S1's capture gives S4 enough input to work with.

## Depends On
- ADR-002 (Observation Extraction) — defines what S1 captures
- ADR-006 (Interface Protocol) — defines how S4 exposes results

## Unlocks
- Work unit detection implementation
- Work unit summary schema (extends ADR-002 Q3's `syntheses` table)
- S4 context actuation (when platform allows)

---

## Context

S4 needs to recognize this pattern in the observation stream:

1. **Intent** — user prompt sets direction
2. **Investigation** — reads, searches, exploration (high read:edit ratio)
3. **Execution** — edits, commands, builds (high edit:read ratio)
4. **Completion** — pattern resets (new unrelated intent, or ratio shifts dramatically)

The signal is the composition and ratio of tool calls per user prompt, combined with file access patterns and intent shifts. The question is: does nmem capture enough to compute these signals?

### Current capture state (pre-session baseline)

| What | Captured | Where |
|------|----------|-------|
| Tool type classification | Yes | `obs_type` via `classify_tool()` + `classify_bash()` |
| Tool name | Yes | `tool_name` column |
| File paths | Yes | `file_path` for Read/Write/Edit/Grep/Glob |
| Command text | Yes | `content` for Bash (truncated 500 chars) |
| Search patterns | Yes | `content` for Grep/Glob |
| User prompts | Yes | `prompts` table, `source = 'user'` |
| Agent thinking | Yes | `prompts` table, `source = 'agent'` (from transcript) |
| Prompt→observation link | Yes | `prompt_id` FK |
| Session boundaries | Yes | `sessions.started_at`/`ended_at` |
| Session signature | Yes | `sessions.signature` — JSON obs_type counts at Stop |
| Session summary | Yes | `sessions.summary` — S1's S4 LLM output |
| Timestamps | Yes | Per-observation Unix timestamp |

### What's missing for S4

Audit of this session (2026-02-17) identified these gaps, ranked by signal value:

#### Gap 1: Tool outcomes (tool_response) — HIGH

nmem records what the agent tried but not what happened. `tool_response` is in the hook payload but was completely ignored. This is like having eyes but no proprioception — S4 can see actions but not results.

**Specific signals lost:**
- Did the build pass or fail? (Bash exit status in tool_response)
- Did the search find anything? (Grep/Glob empty vs. hits)
- Did the edit succeed? (Edit confirmation vs. error)
- What error occurred? (PostToolUseFailure response)

**Resolution (partial, this session):** Added `PostToolUseFailure` handling. Failures now recorded with full `tool_response` in `metadata.response` (truncated 2000 chars, secret-filtered). `source_event = 'PostToolUseFailure'` distinguishes from successes.

**Open question Q1:** Should successful `tool_response` also be captured? The full response can be huge (file contents from Read, full command output from Bash). Options:

- **A: Don't capture success responses.** Content is in the transcript. S4 can derive outcome from obs_type sequence (Edit followed by Bash `cargo test` = build attempt; if no PostToolUseFailure follows, assume success).
- **B: Capture a truncated outcome signal.** First 200-500 chars of tool_response, enough to detect pass/fail patterns. Store in `metadata.outcome`.
- **C: Capture a derived boolean.** Parse tool_response for success/failure heuristics (exit code patterns, "error" keywords, empty results). Store `metadata.succeeded: bool`.

Position C is the most S4-useful — a pre-computed signal rather than raw text. But it requires heuristic maintenance per tool type. Position A is simplest and may be sufficient if S4 can infer outcomes from temporal patterns (failure → retry is visible from observation sequences alone).

#### Gap 2: Per-prompt tool composition — HIGH

The core S4 signal: for each user prompt, what tool types were used and in what proportion? This is derivable from existing data (`GROUP BY prompt_id, obs_type`) but not materialized. S4 needs to run this query on every hook fire to detect ratio shifts.

**Not a capture gap — a query pattern.** No schema change needed. S4's detection loop runs:
```sql
SELECT prompt_id, obs_type, COUNT(*) as n
FROM observations
WHERE session_id = ?1
GROUP BY prompt_id, obs_type
```

The ratio of `file_read + search` to `file_edit + file_write + command` per prompt characterizes the work phase.

#### Gap 3: Tool sequence number — MEDIUM

No incrementing counter per session. S4 needs activity density (tools per minute, tools per prompt) to detect pace changes. Currently derivable from timestamps but a sequence number would be cheaper.

**Options:**
- **A: Derive from timestamps.** `COUNT(*) WHERE session_id = ?1 AND timestamp <= ?2` — works but requires a scan.
- **B: Add sequence column.** `seq INTEGER` auto-incremented per session in the handler. Cheap to query, but requires schema migration.
- **C: Use observation ID ordering.** IDs are auto-increment and session-scoped queries return them in order. `ROW_NUMBER() OVER (ORDER BY id)` gives sequence within any result set.

Position C avoids schema changes and works with existing data. Position B is cleaner for repeated S4 queries.

#### Gap 4: Bash description — MEDIUM

`tool_input.description` is the agent's own semantic label for what a Bash command does (e.g., "Run library unit tests", "Build release binary"). This is a free classification signal — richer than `classify_bash()` which only detects git operations. Currently dropped.

**Resolution:** Capture in `metadata.description` when present. No schema change — uses existing metadata JSON column.

#### Gap 5: Task subagent_type — MEDIUM

`tool_input.subagent_type` from Task tool calls distinguishes `Explore` (investigation) from `general-purpose` (execution) agent spawns. Direct phase classification signal, currently dropped.

**Resolution:** Capture in `metadata.subagent_type` when present.

#### Gap 6: PostToolUseFailure events — MEDIUM

Errors and failures were invisible to nmem. Failure patterns (repeated failures on same file, build failures after edits) are strong work unit signals — they indicate struggle, which often precedes phase transitions.

**Resolution (this session):** `PostToolUseFailure` now handled. Same classification as success, distinguished by `source_event`. Error response in `metadata.response`.

#### Gap 7: Inter-prompt time gaps — LOW

Large gaps between `UserPromptSubmit` events suggest the user left and came back — a likely work unit boundary. Derivable from existing `prompts.timestamp` data. Not a capture gap.

#### Gap 8: PreCompact events — LOW

Context compaction signals that the session is long and context-heavy. Currently ignored. Flagged in TODO.md as an S1's S4 gap (summarization opportunity).

**Deferred.** PreCompact handling adds latency during compaction. Track in TODO.md.

### Platform-blocked signals

These are available via the Anthropic Messages API or Agent SDK but NOT via Claude Code hooks:

| Signal | Source | S4 value | Blocked by |
|--------|--------|----------|------------|
| `input_tokens` / `output_tokens` | API response `usage` | Context pressure, cost tracking | Not in hook payload |
| `cache_read_input_tokens` | API response `usage` | Cache efficiency — reuse signal | Not in hook payload |
| `stop_reason` | API response | Agent chose to stop vs. hit limit | Not in hook payload |
| `costUSD` | Agent SDK `ModelUsage` | Direct cost per turn | Agent SDK only |
| `contextWindow` | Agent SDK `ModelUsage` | Proximity to context limit | Agent SDK only |

These signals become available if nmem moves to an API-based harness (VSM.md §S4 alternative path). The Agent SDK exposes per-message `Usage` with `input_tokens`, `output_tokens`, `cache_creation_input_tokens`, `cache_read_input_tokens`, plus `ModelUsage` with `costUSD`, `contextWindow`, and `webSearchRequests`.

**Implication for this ADR:** S4's data model should not assume these signals are available. The work unit detection algorithm must work with hook-level data only. Token/cost signals are a future enhancement that adds precision but isn't required for boundary detection.

### The `metadata` column as S4's extension point

All new fields go into the existing `metadata` JSON column rather than adding typed columns. Rationale:

1. **No schema migration.** `metadata TEXT` already exists.
2. **Tool-specific.** Not every observation has `description` or `subagent_type` — sparse fields fit JSON better than nullable columns.
3. **Forward-compatible.** S4 may discover new signals to capture. JSON absorbs them without migrations.
4. **Queryable.** SQLite `json_extract()` handles metadata queries. For hot paths, create generated columns or indexes later.

The cost: no type enforcement at the schema level. Extraction code (`s1_extract.rs`, `s1_record.rs`) is the type boundary.

### Fields captured after this session

| Field | Location | Source | Added |
|-------|----------|--------|-------|
| `tool_response` (failures) | `metadata.response` | PostToolUseFailure payload | 2026-02-17 |
| `failed` flag | `metadata.failed` | Derived from source_event | 2026-02-17 |

### Fields to capture next (no schema change needed)

| Field | Location | Source | Signal |
|-------|----------|--------|--------|
| Bash `description` | `metadata.description` | `tool_input.description` | Agent's semantic label for commands |
| Task `subagent_type` | `metadata.subagent_type` | `tool_input.subagent_type` | Investigation vs. execution delegation |

## Open Questions

### Q1: Should successful tool_response be captured?
See Gap 1 above. Three positions (don't capture / truncated outcome / derived boolean). Decision deferred until S4 detection prototype shows whether temporal inference is sufficient.

### Q2: Work unit summary schema
ADR-002 Q3 designed a `syntheses` table but it was never created. Work unit summaries need a table — should it be the `syntheses` table as designed, or a new `work_units` table with fields specific to the work unit model (intent, phase sequence, hot files, outcome)?

A `work_units` table would be more specific:
```sql
CREATE TABLE work_units (
    id INTEGER PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    started_at INTEGER NOT NULL,
    ended_at INTEGER,
    intent TEXT,              -- detected or declared
    phase_signature TEXT,     -- JSON: sequence of phases with obs counts
    hot_files TEXT,           -- JSON: file paths with access counts
    summary TEXT,             -- LLM-generated at boundary
    source_prompt_ids TEXT,   -- JSON array linking to prompts
    source_obs_range TEXT     -- JSON: {first_obs_id, last_obs_id}
);
```

vs. reusing `syntheses` with `scope = 'work_unit'` and stuffing everything into `content`.

The dedicated table is more honest — work units have structure that a generic synthesis blob doesn't capture. But it's another migration and another table to maintain.

### Q3: Does S4 detection run in the hook handler or as a separate process?

The hook handler (`nmem record`) runs on every PostToolUse. S4 detection queries (per-prompt tool composition, ratio shifts) are cheap SQL. Running them inline adds latency to every tool call.

Options:
- **Inline in handler:** Detection runs on every hook fire. Adds ~1-5ms for the SQL queries. Boundary detected immediately.
- **Deferred to Stop:** Run detection at session end. Cheaper but retrospective — can't signal mid-session.
- **Separate timer/process:** Periodic detection (every N seconds or N observations). Decoupled but adds daemon-like complexity (ADR-003 rejected daemons).

The inline approach aligns with ADR-003 (no daemon) and the detection queries are trivially cheap. The question is what to *do* when a boundary is detected — without context actuation (platform-blocked), detection only stores a marker.

### Q4: What's the minimum viable S4?

S4 could ship as detection-only:
1. Compute per-prompt tool composition on every PostToolUse
2. Detect ratio shifts (investigation→execution or vice versa)
3. Store detected boundaries as observations (`obs_type = 'work_unit_boundary'`)
4. At session end, generate work unit summaries for each detected segment

This gives S4 eyes without hands — it can see boundaries but not act on them (no context clear). The value is: SessionStart context injection can use work unit summaries instead of (or alongside) session summaries, providing finer-grained context to the next session.

## Signal Multiplication: Observations × Summaries → Context

The gap analysis above treats observation signals and summary signals as separate inputs. They're not — they multiply.

### Two signal layers

**Layer 1 — Micro (observations):** Per-tool-call facts. Tool composition ratios per prompt, file access patterns, failure sequences, timestamps, command descriptions. These signals operate within a session and detect work unit *boundaries* — the point where investigation shifts to execution or intent changes.

**Layer 2 — Macro (summaries):** Per-session structured semantics. `intent` (what), `learned` (decisions), `completed` (outcomes), `next_steps` (continuations), `notes` (failures). These signals operate across sessions and detect *patterns* — recurring intents, compounding decisions, stale next_steps, repeated failures.

### The multiplication

Neither layer is sufficient alone:
- Micro signals detect boundaries but don't know *meaning*. A shift from reads to edits is a phase transition, but which past decisions are relevant to this new phase?
- Macro signals carry meaning but are coarse. A session summary says "learned: JWT middleware pattern preferred over guard" but doesn't know when to surface it.

**Multiplied:** The micro layer's hot files (current work unit touches `src/auth.rs`, `src/middleware.rs`) select against the macro layer's learned insights (previous session's `files_edited` overlap + `learned` field mentions middleware pattern). The result is targeted retrieval — not "here's everything from your last session" but "here's the specific decision relevant to what you're doing right now."

This is the difference between time-ordered context injection (what nmem does now — summaries sorted by recency) and relevance-ordered injection (what S4 enables — summaries selected by signal overlap with current work).

### The feedback loop

The multiplication output becomes the API input:

```
Observations (S1) ──→ Work unit detection (S4)
                            │
Summaries (S1's S4) ──→ Pattern matching (S4)
                            │
                    Signal multiplication
                            │
                            ▼
                  Context injection (SessionStart)
                            │
                            ▼
                    Anthropic API context window
                            │
                            ▼
                    Agent behavior (tool calls)
                            │
                            ▼
                    New observations (S1) ──→ ...
```

The loop closes: observations feed summaries, summaries feed context injection, context shapes agent behavior, behavior generates new observations. S4 sits at the multiplication point — it's the intelligence that decides which past signals are relevant to the current work, and injects them.

**What this means for the data model:** S4 doesn't just need observation signals and summary signals separately. It needs *join paths* between them:

- `work_units.hot_files` ↔ `sessions.summary→files_edited` — file overlap between current work unit and past session summaries
- `work_units.intent` ↔ `sessions.summary→intent` — intent similarity between current and past work
- `work_units.phase_signature` ↔ `sessions.summary→notes` — if current phase shows struggle (high failure rate), retrieve past sessions' failure notes on the same files
- `observations.file_path` ↔ `sessions.summary→learned` — file-specific decisions from past sessions

These joins are the S4 queries that produce relevance-ordered context. They're SQL queries over existing tables (observations, sessions) plus the new work_units table — no vector search needed. FTS5 handles intent similarity. File path matching is exact. The signal multiplication is relational, not semantic.

### Summary field utility for S4

Each `SessionSummary` field has a specific role in the multiplication:

| Field | S4 role | Join signal |
|-------|---------|-------------|
| `intent` | Intent clustering, recurrence detection | FTS5 similarity with current work unit intent |
| `learned` | Decision retrieval — don't re-derive | File overlap with current hot files |
| `completed` | Progress tracking — what's done | Overlap with current intent (avoid re-doing) |
| `next_steps` | Continuation detection — pick up where left off | Match against current session's first prompt |
| `files_read` | Investigation history | File overlap with current reads |
| `files_edited` | Execution history, file heat across sessions | File overlap with current edits |
| `notes` | Negative knowledge — what failed | File + intent overlap (same mistake pattern) |

The `learned` and `notes` fields are the highest-value targets for cross-session injection. `learned` prevents re-derivation of decisions. `notes` prevents repetition of failed approaches. Both are currently captured by S1's S4 but only injected as bulk session summary text — not selected by relevance to current work.

### Implication for Q2 (work unit schema)

This strengthens the case for a dedicated `work_units` table over the generic `syntheses` table. The join paths above require structured fields (`hot_files`, `intent`, `phase_signature`) — not a blob. The `syntheses` table's `content TEXT` column can't support relational queries like "find sessions where `learned` mentions files in my current hot set."

The `work_units` table should also carry the summary fields that will be used for cross-session joins:

```sql
CREATE TABLE work_units (
    id INTEGER PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    started_at INTEGER NOT NULL,
    ended_at INTEGER,
    intent TEXT,              -- detected or declared
    phase_signature TEXT,     -- JSON: [{phase, obs_count, duration}]
    hot_files TEXT,           -- JSON: [{path, read_count, edit_count}]
    summary TEXT,             -- LLM-generated at boundary
    learned TEXT,             -- JSON array, mirroring SessionSummary.learned
    notes TEXT,               -- negative knowledge for this work unit
    source_prompt_ids TEXT,   -- JSON array linking to prompts
    source_obs_range TEXT     -- JSON: {first_obs_id, last_obs_id}
);
```

This gives S4 the structured fields it needs for relational joins while keeping LLM-generated content (`summary`, `learned`, `notes`) alongside the structured signals (`phase_signature`, `hot_files`).

## Decision

**Deferred.** This ADR captures the data model analysis. Decisions on Q1-Q4 will be made when S4 detection is prototyped.

**Immediate actions taken:**
- `PostToolUseFailure` handling added to `s1_record.rs` (Gap 6)
- `tool_response` field added to `HookPayload` (Gap 1, partial)
- Hook registered in `settings.json`

**Next actions (pre-decision):**
- Capture Bash `description` and Task `subagent_type` in metadata (Gaps 4, 5)
- Prototype S4 detection query to validate that per-prompt tool composition produces clean phase signals from real data

## References

- VSM.md §S4 — work unit concept and S4 design
- ADR-002 — extraction strategy, `syntheses` table schema
- ADR-003 — daemon lifecycle (no daemon constraint)
- `claude-code-hooks-events.md` — hook payload field inventory
- Claude Agent SDK docs — `Usage`, `ModelUsage` types, hook input schemas
- Anthropic Messages API — `usage`, `stop_reason` response fields

## Revision History

| Date | Version | Changes |
|------|---------|---------|
| 2026-02-17 | 0.1 | Initial draft. Gap analysis from live session. 8 gaps identified, 3 resolved (PostToolUseFailure, tool_response on failure, hook registration). 4 open questions. |
| 2026-02-17 | 0.2 | Added Signal Multiplication section. Observations × Summaries as two-layer signal model. Feedback loop diagram. Per-summary-field S4 join paths. Revised Q2 work_units schema to include `learned` and `notes` fields for cross-session relational queries. |
