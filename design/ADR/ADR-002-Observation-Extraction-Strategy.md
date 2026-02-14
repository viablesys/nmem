# ADR-002: Observation Extraction Strategy

## Status
Accepted

## Framing
*Structured extraction vs LLM synthesis vs hybrid.*

This is the load-bearing decision. It determines: whether sqlite-vec is needed, what the schema looks like, how much data is generated, the cost model, and whether S4 is inline or periodic. Write it as a direct confrontation: "What if we never use an LLM for extraction?" Force the case for pure structured capture and see what's actually lost.

## Depends On
None — this is the root decision.

## Unlocks
ADR-005 (Forgetting), ADR-004 (Project Scoping), ADR-003 (Daemon Lifecycle)

---

## Context

claude-mem used an SDK agent (LLM subprocess) to extract observations from every tool call. The results:
- 9 observations in early testing, 7 were hallucinated
- ~1700 tokens per observation regardless of quality
- No validation layer — garbage was stored with the same confidence as facts
- The observer agent fabricated files, features, and bugfixes that never happened
- The MCP retrieval interface (search → timeline → get_observations) was essentially structured SQL queries, not semantic search

This isn't just a quality problem. It's a category error: using an LLM (probabilistic, creative, expensive) for a job that is mostly factual record-keeping (deterministic, verifiable, cheap).

## The Three Positions

### Position A: Pure Structured Extraction (No LLM)

Every observation is derived from deterministic parsing of tool calls and their results. No LLM is involved in extraction. S4 (intelligence) operates only on already-stored structured data, periodically, not inline.

**What can be captured structurally:**
- Files read/written (path, size, timestamp) — from Read/Write/Edit tool calls
- Commands run and their exit codes — from Bash tool calls
- Errors encountered (error text, stack traces) — from tool result parsing
- Search queries and result counts — from Grep/Glob tool calls
- Commit messages and diffs — from git operations
- Session boundaries (start/end, duration, working directory)
- User-declared intent — from the initial prompt (literal text, no interpretation)

**What is lost:**
- *Why* a file was read (intent behind the action)
- Connections between actions (this error led to that fix)
- Inferred decisions ("chose library X over Y because...")
- Summarization across sessions ("this week was mostly debugging auth")

**The hard question:** Do we actually need what's lost? The temporal structure (action A happened before action B in the same session, in response to user prompt C) provides implicit causality. A human reviewing the structured log can reconstruct the "why" — can an LLM at retrieval time do the same without needing it pre-extracted?

**Schema implications:** Predictable, typed columns. FTS5 on text fields is sufficient for search. No vector embeddings needed. sqlite-vec is not required.

**Cost:** Near zero. Parsing is string manipulation. No API calls, no token costs, no latency.

**Failure mode:** Information is too granular — 50 file reads per session produce 50 observations, most uninteresting. Retrieval returns noise because everything is recorded with equal weight. The system remembers *everything* but understands *nothing*.

### Position B: LLM Extraction (Every Tool Call)

This is what claude-mem did. An LLM subprocess observes every tool call and produces natural-language observations with type classifications (bugfix, feature, discovery, decision, etc.).

**What is gained:**
- Semantic compression — 50 file reads become "investigated auth module structure"
- Intent capture — "read config.yaml to check OAuth settings"
- Decision recording — "chose PKCE flow over implicit grant"
- Cross-action synthesis — "fixed the bug by changing X after discovering Y"

**What is lost:**
- Truthfulness — the LLM hallucinates freely (7/9 fabricated in testing)
- Determinism — same input may produce different observations
- Cost control — ~1700 tokens per observation, every tool call
- Latency — extraction adds seconds to each operation
- Verifiability — no way to check if "fixed memory leak" actually happened

**Schema implications:** Unstructured text fields. Vector embeddings become compelling for semantic retrieval. sqlite-vec enters the picture.

**Cost:** High. Every tool call triggers an API call. At ~1700 tokens/observation and dozens of tool calls per session, a single session costs thousands of tokens just for observation extraction — independent of the actual work.

**Failure mode:** Demonstrated. claude-mem's observer fabricated OAuth2 implementations, email notification systems, and deployment pipelines that never existed. The system confidently remembers things that never happened. Worse: there is no ground truth to validate against, so garbage accumulates forever.

### Position C: Hybrid (Structured Capture, Periodic LLM Synthesis)

Structured extraction at S1 (operations). LLM synthesis at S4 (intelligence), running periodically over accumulated structured data — not inline on every tool call.

**S1 captures facts:** files, commands, errors, session boundaries.
**S4 synthesizes patterns:** "this week's sessions focused on auth refactoring" or "the user keeps hitting the same SQLite locking error across projects."

**Key difference from Position B:** The LLM sees structured records, not raw tool output. It synthesizes over verified facts, not over noisy streams. And it runs on a schedule (end of session, daily, weekly), not on every action.

**What is gained:**
- Factual foundation — S1 data is verifiable and deterministic
- Semantic layer — S4 adds meaning on top of facts
- Cost control — LLM runs periodically, not per-tool-call
- Validation possible — S4 synthesis can be checked against S1 records
- Separation of concerns — VSM alignment (operations vs intelligence)

**What is lost vs Position A:**
- Complexity — two extraction paths instead of one
- S4 still hallucinates — periodic synthesis can still fabricate patterns
- Delayed insight — session-level synthesis isn't available during the session

**What is lost vs Position B:**
- No inline intent capture — "why did I read this file?" isn't recorded in real-time
- Synthesis granularity — session or daily summaries lose per-action detail

**Schema implications:** Structured observations (typed columns) plus a separate synthesis table (unstructured text). FTS5 handles the structured layer. Vector search is optional — only if S4 synthesis produces enough unstructured text to warrant semantic retrieval.

## Adversarial Analysis

### Attacking Position A: "Structured extraction produces a useless log"

The strongest objection: a list of files read and commands run is a *log*, not a *memory*. Memory requires selection (what matters), connection (how things relate), and abstraction (what it means). Structured extraction gives you none of these.

**Counter:** Selection can be rules-based without an LLM. Not every file read is worth storing. Heuristics that matter:
- File was *written* (not just read) — indicates actual work
- Command *failed* — errors are more memorable than successes
- Session was *long* (>N minutes) — indicates substantial work
- User prompt contained decision language ("let's use X", "switch to Y")
- Same file touched in multiple sessions — indicates importance through repetition

Connection can emerge from structure: observations sharing a session ID are temporally connected. Observations sharing file paths are topically connected. No LLM needed.

Abstraction is the one genuine gap. "What was I doing this week?" requires synthesis that structure alone can't provide. But this is exactly what S4 is for — and it doesn't need to happen at extraction time.

**Verdict:** The objection holds only for abstraction, which Position C addresses by deferring it to S4.

### Attacking Position B: "LLM extraction is the right tool for the job"

The strongest defense of LLM extraction: tool calls are noisy, and only an LLM can identify what's *noteworthy* in a stream of reads, writes, and greps. A structured extractor would need hundreds of heuristic rules to approximate what an LLM does in one prompt.

**Counter:** claude-mem *had* an LLM doing this, and it failed catastrophically. The failure wasn't tuning — it was architectural. An LLM observing tool calls has:
1. No ground truth — it can't verify that a "bugfix" actually fixed a bug
2. No feedback — it never learns which observations were useful
3. No context budget — it sees one tool call at a time, not the session arc
4. Pressure to produce — the prompt asks "what's noteworthy?" and the LLM always finds *something*, even when nothing happened

These aren't fixable with better prompts. They're structural properties of using an LLM as a fact recorder.

**Verdict:** LLM extraction is the wrong tool for fact recording. It's the right tool for synthesis *over* facts. Position C separates these correctly.

### Attacking Position C: "Hybrid is just complexity for complexity's sake"

The objection: Position C is two systems instead of one. Why not start with A and add S4 later if needed?

**Counter:** This objection is actually *correct*. Position A is a strict subset of Position C. The right move is to implement A, measure retrieval quality, and add S4 synthesis only if there's a demonstrated gap. The hybrid framing is useful for design (it maps to VSM), but the implementation order is: A first, S4 synthesis when needed.

**Verdict:** Position C is the design target. Position A is the implementation starting point.

## The Harness-Declared Noteworthiness Question

There's a fourth extraction source not covered by Positions A-C: the *harness* (Claude Code, or whatever consumer is generating observations) could declare noteworthiness explicitly.

Examples:
- Claude Code's Stop hook already generates session summaries — that's harness-declared synthesis
- A user could explicitly say "remember this" — that's harness-declared importance
- A tool call could carry metadata ("this edit was a bugfix") — that's harness-declared classification

This is interesting because it offloads the "what's noteworthy?" question to the agent that has context (the LLM in conversation), without nmem needing its own LLM. nmem becomes a *storage and retrieval system*, not an *observation system*. The observation intelligence lives in the consumer.

**Tension with viability:** If nmem depends on the harness for noteworthiness signals, it's not viable in its own right — it's a dumb store. The VSM principle says nmem should have its own intelligence (S4). But S4 doesn't need to be an LLM — it could be statistical (most-accessed observations are important) or rule-based (errors are more important than reads).

**Tension with coupling:** Harness-declared noteworthiness couples nmem to the harness's observation model. Different consumers (Claude Code, Cursor, a CLI tool) would need to speak nmem's noteworthiness vocabulary. This is an interface design problem (ADR-006) but the extraction strategy needs to decide: does nmem *accept* external noteworthiness signals, or does it *derive* importance internally?

**Resolution:** Accept both. Harness can declare noteworthiness via the protocol (a field in the observation payload). nmem also derives importance internally (frequency, recency, error status). Neither source is authoritative alone — they're signals that S4 can weight.

## Open Questions

### Q1: What's the minimum viable observation? — RESOLVED

Every observation has these fields:

| Field | Type | Source |
|-------|------|--------|
| `id` | INTEGER PRIMARY KEY | Auto-increment |
| `timestamp` | INTEGER NOT NULL | `unixepoch()` at capture |
| `session_id` | TEXT NOT NULL | Hook common field |
| `project` | TEXT NOT NULL | Derived from `cwd` |
| `obs_type` | TEXT NOT NULL | Derived from tool/event (see Q4) |
| `source_event` | TEXT NOT NULL | `hook_event_name` |
| `tool_name` | TEXT | From PostToolUse (NULL for non-tool events) |
| `content` | TEXT NOT NULL | Extracted fact (file path, command, error text) |
| `file_path` | TEXT | Normalized path when applicable |
| `metadata` | TEXT | JSON via `serde_json::Value` for tool-specific fields |

**Raw content: No.** Store extracted facts, not raw tool output. Raw output lives in the transcript file (`transcript_path` from hook common fields) and can be retrieved if needed. Observations are summaries of what happened, not copies of what was said.

**Project derivation:** `cwd` from hook input, normalized to a project identifier. ADR-004 defines the scoping model — for now, the working directory is the project.

### Q2: How aggressive should filtering be? — RESOLVED

**Store everything. Filter at retrieval, not capture.**

At nmem's data volumes, storage is not a constraint. Storing every tool call without capture-time filtering:

| Timeframe | Observations | DB size (with FTS5) |
|-----------|-------------|---------------------|
| Per session | 50-250 | 25-125 KB |
| Per day (3-5 sessions) | 150-1,250 | 75-625 KB |
| Per month | 4,500-37,500 | 2-19 MB |
| Per year | 54,000-450,000 | 27-225 MB |

Even the high end (450K rows, 225 MB after a year of heavy use) is trivial for SQLite — indexed queries remain single-digit milliseconds. There is no storage pressure that justifies discarding observations at write time.

**Noise is handled by dedup, not filtering.** ADR-003's dedup logic (same `session_id` + `obs_type` + `file_path` within a time window) collapses the noisiest pattern — repeated reads of the same file. This reduces volume without information loss.

**Signal-to-noise is a retrieval concern.** MCP tools (ADR-006) filter by `obs_type`, `project`, recency, and FTS5 relevance. The consumer chooses what to surface. A `file_read` observation that seems like noise today may be the only record of investigation into a file that was later deleted.

claude-mem drowned in noise because its LLM observer interpreted noise as signal, fabricating significance for mundane tool calls. Structured extraction doesn't have this problem — a `file_read` observation is just a record of a path and timestamp. It's inert until queried.

### Q3: What schema supports both structured and future synthesis? — RESOLVED

```sql
-- Primary: structured observations (Position A)
CREATE TABLE observations (
    id INTEGER PRIMARY KEY,
    timestamp INTEGER NOT NULL,
    session_id TEXT NOT NULL,
    project TEXT NOT NULL,
    obs_type TEXT NOT NULL,
    source_event TEXT NOT NULL,
    tool_name TEXT,
    content TEXT NOT NULL,
    file_path TEXT,
    metadata TEXT,  -- serde_json::Value
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE INDEX idx_obs_session ON observations(session_id);
CREATE INDEX idx_obs_project ON observations(project);
CREATE INDEX idx_obs_type ON observations(obs_type);
CREATE INDEX idx_obs_timestamp ON observations(timestamp DESC);
CREATE INDEX idx_obs_file_path ON observations(file_path);

-- Future: S4 synthesis (Position C, when earned)
CREATE TABLE syntheses (
    id INTEGER PRIMARY KEY,
    timestamp INTEGER NOT NULL,
    scope TEXT NOT NULL,  -- 'session', 'daily', 'weekly'
    project TEXT,
    content TEXT NOT NULL,
    source_obs_ids TEXT NOT NULL,  -- JSON array of observation IDs
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);
```

FTS5 external content table and sync triggers are defined in ADR-001. The `syntheses` table is not created at launch — it's added via migration when S4 is implemented. The `source_obs_ids` column links synthesis back to the structured records it was derived from, enabling validation ("does this summary actually reflect the underlying observations?").

### Q4: Does the extraction strategy change per observation source? — RESOLVED

Yes. Each source has its own extractor, but all produce the same observation schema (Q1).

| Source Event | Extractor | obs_type values | What's extracted |
|-------------|-----------|-----------------|-----------------|
| PostToolUse (Bash) | Parse `tool_input.command`, `tool_response` exit code | `command`, `command_error` | Command text, exit code, stderr on failure |
| PostToolUse (Read) | Parse `tool_input.file_path` | `file_read` | File path |
| PostToolUse (Write) | Parse `tool_input.file_path` | `file_write` | File path |
| PostToolUse (Edit) | Parse `tool_input.file_path` | `file_edit` | File path |
| PostToolUse (Grep/Glob) | Parse `tool_input.pattern`, result count | `search` | Query pattern, match count |
| PostToolUse (MCP) | Parse `tool_name` (server prefix) | `mcp_call` | Server + method, params |
| UserPromptSubmit | `prompt` field (after secret filtering per ADR-007) | `user_prompt` | User intent text |
| SessionStart | `source` field | `session_start` | Source (startup/resume/compact) |
| Stop | Session boundary marker | `session_end` | Duration derived from first/last timestamps |

See `claude-code-hooks-events.md` in the library for complete field schemas per event. All sources produce observations — Q2 resolved: store everything, dedup handles noise.

### Q5: What volume should we actually design for? — RESOLVED

With Q2 resolved (store everything), volume estimates reflect unfiltered capture with dedup:

- 3-5 sessions/day, 50-250 tool calls/session → 150-1,250 raw events/day
- After dedup (ADR-003, same file/session within window): ~100-1,000/day
- Monthly: 3,000-30,000 observations
- Annual: 36,000-360,000 observations
- Storage: 18-180 MB/year (with FTS5 indexes)

At 18K rows, SQLite doesn't even notice. FTS5 is overkill. Full table scans complete in milliseconds. This is a "small data" problem pretending to be a "big data" problem.

## Decision

**Position A: pure structured extraction.** No LLM at launch.

Rationale:
1. It's the simplest thing that could work
2. It produces verifiable, deterministic data
3. It eliminates the LLM cost and hallucination problems that killed claude-mem
4. The data volumes are trivially small for SQLite
5. FTS5 (or even LIKE queries) handle retrieval at this scale
6. sqlite-vec is not needed, avoiding the broken DELETE problem
7. S4 synthesis can be layered on later as a separate concern
8. Harness-declared noteworthiness (session summaries, user "remember this") supplements structured capture without nmem needing its own LLM

**What this means for downstream ADRs:**
- **ADR-001 (Storage):** sqlite-vec is not needed at launch. FTS5 is sufficient. Litestream remains overkill.
- **ADR-003 (Daemon):** No LLM subprocess needed. Extraction is pure parsing — could be in-process, no daemon required for extraction alone.
- **ADR-004 (Scoping):** Structured observations have explicit project scope (working directory). No ambiguity about which project an observation belongs to.
- **ADR-005 (Forgetting):** DELETE works normally on structured tables (no sqlite-vec complications). Simple retention policies are feasible.
- **ADR-007 (Trust):** Structured extraction can apply deterministic secret filters (regex patterns, known token formats) before storage. No LLM in the path means no risk of the LLM echoing secrets into observations.

## When to Reconsider

Add LLM synthesis (Position C / S4) when:
- Retrieval quality is demonstrably poor — users can't find relevant observations
- Structured queries consistently miss contextually relevant results
- Cross-session patterns exist that temporal/file-based proximity can't surface
- The volume grows large enough that abstraction adds retrieval value (probably >10K observations)

Do **not** add LLM synthesis preemptively. The predecessor proved that premature LLM extraction produces garbage at cost. Earn the complexity with evidence.

## References

- `claude-code-hooks-events.md` — Hook event schemas, what data each event provides for extraction
- `sqlite-retrieval-patterns.md` — Multi-signal retrieval strategies proving FTS5 + metadata queries are sufficient
- `fts5.md` — FTS5 mechanics (tokenizers, triggers, query syntax)
- `serde-json.md` — JSON metadata storage patterns with rusqlite

## Revision History

| Date | Version | Changes |
|------|---------|---------|
| 2026-02-08 | 1.0 | Initial draft. Three positions, adversarial analysis, open questions. |
| 2026-02-14 | 2.0 | Accepted. Resolved Q1 (observation schema), Q3 (database schema), Q4 (per-source extraction mapping). Promoted Preliminary Direction to Decision. Added library references. |
| 2026-02-14 | 2.1 | Resolved Q2 (store everything, filter at retrieval) and Q5 (volume estimates updated for unfiltered capture). All open questions now resolved. |
