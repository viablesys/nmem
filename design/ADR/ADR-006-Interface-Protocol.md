# ADR-006: Interface Protocol

## Status
Accepted

## Framing
*MCP vs Unix socket vs stdin/stdout vs CLI.*

Determines how nmem couples (or doesn't) to Claude Code. Adversarial framing: "What if Claude Code disappears tomorrow?" -- does nmem still work? This forces the viability principle from VSM. Also: push vs pull retrieval, and who decides relevance (nmem or the consumer).

## Depends On
Independent -- can be explored at any time.

ADR-003 decided the *mechanism* (stdin for hooks, stdio MCP for queries). This ADR formalizes the *protocol* -- what tools are exposed, what parameters they take, what they return. ADR-002 decided the observation schema that shapes every return type here.

---

## Context

claude-mem exposed three MCP tools (`search`, `timeline`, `get_observations`) via a session-scoped stdio subprocess. The 3-layer workflow (search -> timeline -> get_observations) was token-efficient: narrow results before fetching full content. The tools were sound; the data they returned (LLM-hallucinated observations) was the problem.

nmem has three interface surfaces: ingestion (`nmem record` -- hook handler), queries (`nmem serve` -- MCP server), and context injection (SessionStart push). This ADR specifies the contracts for all three.

## Protocol Choice

**MCP via stdio for queries. Stdin JSON for ingestion. Stdout text for context injection.**

All three are stdio-based. No ports, no sockets, no HTTP. MCP is the query protocol because Claude Code natively speaks it. Stdin JSON is the ingestion protocol because Claude Code hooks pipe JSON to the handler's stdin. Stdout text is the injection protocol because SessionStart hooks return context as plain text or via `hookSpecificOutput.additionalContext`. Rationale for the mechanism is in ADR-003.

## Observation Ingestion Protocol

The hook handler (`nmem record`) reads a single JSON object from stdin. The schema is defined by Claude Code's hook contract (see `claude-code-hooks-events.md`), not by nmem.

### Ingestion Contract

| Aspect | Specification |
|--------|--------------|
| Input | Single JSON object on stdin, newline-terminated |
| Encoding | UTF-8 |
| Max payload | No hard limit; `tool_input.content` (Write) -- hash or truncate, do not store verbatim |
| Required fields | `session_id`, `cwd`, `hook_event_name` |
| Unknown fields | Ignored (forward-compatible) |
| Malformed JSON | Exit 1, log to stderr |
| Missing required fields | Exit 1, log to stderr |
| Success output | None (stdout silent, except SessionStart which emits context) |
| Error output | Human-readable message on stderr |
| Exit codes | 0 = stored, 1 = non-blocking error, 2 = blocking error (DB corruption) |

### Extraction Dispatch

`nmem record` dispatches on `hook_event_name` and `tool_name` to per-source extractors (ADR-002 Q4). Each extractor produces zero or one observation in the ADR-002 Q1 schema. Zero when filtered (e.g., duplicate file read within dedup window).

The handler uses `serde_json::Value::get()` to extract known fields and ignores the rest -- no strict per-tool schema validation. This decouples nmem from Claude Code's tool schema evolution.

## Query Protocol -- MCP Tools

Six tools exposed via `nmem serve` (session-scoped stdio MCP server, read-only SQLite connection).

### `search`

Full-text search over observations with metadata filters.

**Parameters:**

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `query` | string | yes | -- | FTS5 query. Supports `AND`/`OR`/`NOT`, `"phrase"`, `prefix*`. |
| `project` | string | no | current project | Filter by project. NULL = all projects. |
| `obs_type` | string | no | NULL | Filter by type (`file_read`, `file_write`, `file_edit`, `command`, `command_error`, `search`, `user_prompt`, `session_start`, `session_end`, `mcp_call`). |
| `limit` | integer | no | 20 | Max results. Capped at 100. |
| `offset` | integer | no | 0 | Pagination offset. |

**Returns:** JSON array ranked by BM25:

```json
[{
  "id": 42, "timestamp": 1707400000, "obs_type": "command_error",
  "content_preview": "cargo test -- auth::tests failed with...",
  "file_path": "/home/user/project/src/auth/mod.rs",
  "session_id": "d4f8a2b1-..."
}]
```

`content_preview` is `SUBSTR(content, 1, 120)` -- enough for relevance assessment, cheap on tokens.

**SQL sketch** (see `fts5.md` § 3 for MATCH syntax, `sqlite-retrieval-patterns.md` § 1 for FTS5+WHERE composition):

```sql
SELECT o.id, o.timestamp, o.obs_type,
       SUBSTR(o.content, 1, 120) AS content_preview,
       o.file_path, o.session_id
FROM observations o
JOIN observations_fts f ON o.id = f.rowid
WHERE observations_fts MATCH ?1
  AND (?2 IS NULL OR o.project = ?2)
  AND (?3 IS NULL OR o.obs_type = ?3)
ORDER BY f.rank  -- FTS5 rank is negative BM25 (more negative = better match); ASC is correct
LIMIT ?4 OFFSET ?5
```

### `get_observations`

Fetch full observation details by ID.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `ids` | integer[] | yes | Observation IDs. Max 50 per request. Empty array = error. |

**Returns:** JSON array of full observation objects (all ADR-002 schema fields):

```json
[{
  "id": 42, "timestamp": 1707400000, "session_id": "d4f8a2b1-...",
  "project": "my-project", "obs_type": "command_error",
  "source_event": "PostToolUse", "tool_name": "Bash",
  "content": "cargo test -- auth::tests failed with assertion error",
  "file_path": null,
  "metadata": { "command": "cargo test -- auth::tests", "exit_code": 101 }
}]
```

Order matches input `ids`. Missing IDs silently omitted (retention may have deleted them).

### `timeline`

Observations surrounding an anchor point within the same session.

**Parameters:**

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `anchor` | integer | yes | -- | Observation ID to center on. |
| `before` | integer | no | 5 | Observations before anchor (same session). |
| `after` | integer | no | 5 | Observations after anchor (same session). |

**Returns:** JSON object with anchor and surrounding context:

```json
{
  "anchor": { "id": 42, "obs_type": "command_error", "content": "...", ... },
  "before": [{ "id": 40, ... }, { "id": 41, ... }],
  "after": [{ "id": 43, ... }]
}
```

All entries use the full observation schema. Arrays ordered chronologically. May be shorter than requested near session boundaries.

**SQL sketch:**

```sql
SELECT session_id FROM observations WHERE id = ?1;
-- Before: WHERE session_id = ?1 AND id < ?2 ORDER BY id DESC LIMIT ?3
-- After:  WHERE session_id = ?1 AND id > ?2 ORDER BY id ASC LIMIT ?3
```

### `recent_context`

Recent observations for the current working context. MCP equivalent of SessionStart injection, available on demand.

**Parameters:**

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `project` | string | no | current project | Project scope. |
| `limit` | integer | no | 30 | Max observations. Capped at 100. |

**Returns:** JSON array of observations ranked by composite score (highest first). Each object includes all ADR-002 schema fields plus a `score` field (float). Selection:

1. **Composite scoring** — three signals, no BM25 (no text query):
   ```
   score = recency * W_r + type_weight * W_t + project_match * W_p
   ```
   With project specified: `W_r=0.5, W_t=0.3, W_p=0.2`. Without project: `W_r=0.6, W_t=0.4` (project signal dropped).

2. **Recency** — exponential decay with 7-day half-life: `exp(-ln(2) * age_days / 7.0)`. Values: 1.0 (now), 0.5 (7d), 0.25 (14d), 0.125 (21d). Implemented as a SQLite UDF (`exp_decay`).

3. **Type weight** — normalized to 0..1: `file_edit`=1.0, `command`=0.67, `session_compact`=0.5, `mcp_call`=0.33, all others=0.17.

4. **Project match** — binary boost: same project=1.0, different project=0.3. When `project` is specified, cross-project observations still appear but rank lower (boost, not filter).

5. Deduplicated by `file_path` where `file_path IS NOT NULL` (keep highest-scored per path). Observations with NULL `file_path` (commands, errors, prompts) are never deduplicated — each is unique context.

### `session_trace`

Drill into a session's structure. Returns the session's prompts in order, each with its observations. Navigates the session → prompts → observations hierarchy that `timeline` (observation-anchored) and `session_summaries` (compressed) don't expose.

**Parameters:**

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `session_id` | string | yes | -- | Session ID to trace. |
| `before` | integer | no | NULL | Only include prompts/observations before this Unix timestamp. |
| `after` | integer | no | NULL | Only include prompts/observations after this Unix timestamp. |

**Returns:** JSON object with session metadata and prompts:

```json
{
  "session_id": "d4f8a2b1-...", "project": "my-project",
  "started_at": 1707400000, "ended_at": 1707403600,
  "summary": { "intent": "fix auth bug", ... },
  "prompts": [{
    "prompt_id": 1, "timestamp": 1707400010, "source": "user",
    "content": "Fix the login bug",
    "observation_count": 3,
    "observations": [{
      "id": 42, "timestamp": 1707400020, "obs_type": "file_read",
      "file_path": "/src/auth.rs", "content_preview": "Read /src/auth.rs",
      "is_pinned": false
    }]
  }]
}
```

Observations with NULL `prompt_id` (session-level events) appear as synthetic prompts with `source: "system"` and `prompt_id: null`.

**SQL approach:** Single query using LEFT JOIN of prompts to observations, UNION ALL with orphan observations (NULL prompt_id), grouped in Rust by prompt_id. Temporal filters apply to both `p.timestamp` (in WHERE) and `o.timestamp` (in the JOIN condition) — a prompt inside the window still appears but without out-of-window observations.

**Error: unknown session** → MCP error: "session not found: {id}".

### `file_history`

Trace a file's history across sessions. Returns every session that touched this file, grouped by session with the intent behind each touch.

**Parameters:**

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `file_path` | string | yes | -- | File path to trace. |
| `before` | integer | no | NULL | Only include touches before this Unix timestamp. |
| `after` | integer | no | NULL | Only include touches after this Unix timestamp. |
| `limit` | integer | no | 10 | Max observations returned. Capped at 50. |

**Returns:** JSON object with file path and sessions:

```json
{
  "file_path": "/src/auth.rs",
  "sessions": [{
    "session_id": "d4f8a2b1-...", "project": "my-project",
    "started_at": 1707400000,
    "summary_intent": "fix auth bug",
    "touches": [{
      "observation_id": 42, "timestamp": 1707400020,
      "obs_type": "file_edit", "content_preview": "Edit /src/auth.rs: fix token...",
      "prompt_content": "Fix the login bug",
      "is_pinned": false
    }]
  }]
}
```

`summary_intent` extracts only the `intent` field from the session's summary JSON. `prompt_content` joins with `prompts` filtered to `source = 'user'` — surfaces the human intent, not agent thinking blocks. Uses existing `idx_obs_file(file_path)` index.

**Error: unknown file** → Empty sessions array `[]`, not an error.

### Error Handling

MCP tools return errors via the standard MCP error response format. Specific cases:

| Tool | Error condition | Behavior |
|------|----------------|----------|
| `search` | Malformed FTS5 query (e.g., unbalanced quotes) | MCP error with message describing the syntax issue |
| `search` | No results | Empty array `[]`, not an error |
| `get_observations` | Empty `ids` array | MCP error: "ids array must not be empty" |
| `get_observations` | All IDs missing (deleted by retention) | Empty array `[]`, not an error |
| `get_observations` | Some IDs missing | Partial array with found observations only |
| `timeline` | Anchor ID doesn't exist | MCP error: "anchor observation not found" |
| `timeline` | Anchor exists but no surrounding context | Anchor returned with empty `before`/`after` arrays |
| `recent_context` | No observations for project | Empty array `[]` |
| `session_trace` | Session ID doesn't exist | MCP error: "session not found: {id}" |
| `session_trace` | Session exists but no prompts in window | Empty `prompts` array |
| `file_history` | File path not found in observations | Empty `sessions` array |

Database errors (connection failure, corruption) return MCP internal errors with the SQLite error message. These should not expose file paths or internal state beyond what SQLite reports.

## CLI Query Interface

`nmem search` exposes the same FTS5 search available via MCP, without requiring an MCP harness. Intended for scripts, terminal debugging, and harness-independent access.

### Usage

```
nmem search <query> [--project <name>] [--type <obs_type>] [--limit <n>] [--full] [--ids]
```

### Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `<query>` | positional | required | FTS5 query (supports AND/OR/NOT, "phrases", prefix*) |
| `--project` | string | none | Filter by project name |
| `--type` | string | none | Filter by observation type |
| `--limit` | integer | 20 | Max results, clamped to [1, 100] |
| `--full` | flag | off | Include all observation fields |
| `--ids` | flag | off | Output IDs only, one per line |

### Output Modes

**Default** — JSON array of index entries to stdout:
```json
[{"id": 42, "timestamp": 1707400000, "obs_type": "command",
  "content_preview": "cargo test", "file_path": null, "session_id": "abc-..."}]
```

**`--full`** — JSON array with complete observation fields (adds `source_event`, `tool_name`, `content`, `metadata`).

**`--ids`** — One numeric ID per line. Designed for piping: `nmem search "stale" --ids | xargs -I{} nmem purge --id {} --confirm`.

All modes print `nmem: N results for "query"` to stderr.

### Design Decisions

- **Inline SQL, not shared with s1_serve.rs.** The MCP server returns `CallToolResult` with MCP-specific error types. The CLI returns `NmemError`. Sharing the SQL query but not the execution/error-handling code avoids coupling the CLI to MCP types while keeping the queries trivially auditable as identical.
- **Separate serde structs.** `search::SearchResult` and `search::FullObservation` mirror but don't import `serve::SearchResult`. Same JSON shape, independent compilation units. No coupling.
- **Read-only connection.** Uses `open_db_readonly` — same as the MCP server. Encryption keys resolved via the same `load_key` path.

## Context Injection (Push Model)

The SessionStart hook pushes context proactively. `nmem record` handles SessionStart differently: it stores the `session_start` observation *and* emits context on stdout.

### Output Format

```
## nmem: Recent Context

### Recent (my-project)
| ID | Time | Type | Summary |
|----|------|------|---------|
| #50 | 2:30 PM | file_edit | src/auth/mod.rs |
| #49 | 2:28 PM | command_error | cargo test failed |
| #48 | 2:25 PM | file_write | src/auth/token.rs |

### Cross-project
| #45 | 1:15 PM | command | docker compose up (other-project) |
```

### Injection Contract

| Aspect | Specification |
|--------|--------------|
| Output | stdout plain text (Claude Code captures as `additionalContext`) |
| Format | Markdown: `## Recent Intents` list (top 10 intents with action counts, zero-action filtered) + observation tables, compact, scannable |
| Max lines | ~50 lines (~350-500 tokens, varies by content length). Competes with CLAUDE.md for context window. |
| Timing | SessionStart only (`source` in `startup`, `resume`, `clear`, `compact`) |
| Recovery mode | On `compact`/`clear`: expanded limits (30 local + 15 cross). Agent lost its context window — inject more to compensate. |
| Normal mode | 20 project-local + up to 10 cross-project backfill |
| Scoring | Same composite scoring as `recent_context`: `exp_decay` recency (7d half-life) * 0.6 + type_weight * 0.4. Deduped by file_path. |
| Execution order | After transaction commit and sweep — context reflects post-sweep state |
| Error handling | Non-fatal. Errors go to stderr, do not fail the hook. |

> **[ANNOTATION 2026-02-14, v3.0]:** Context injection now implemented in `src/s4_context.rs`. The module duplicates the scoring SQL from `s1_serve.rs` per the ADR-006 design decision ("inline SQL, not shared with s1_serve.rs"). Cross-project section shows `[project_name]` suffix per row. Output format uses `# nmem context` as top-level header with `## project_name` and `## Other projects` subsections.

> **[ANNOTATION 2026-02-14, v1.2]:** Live data shows 56% of user intents trigger zero tool actions (conversational turns: "yes", "i do", questions). Context injection should weight intents-with-actions higher than bare conversational turns. The prototype context generator already shows action counts per intent (`→ N actions`) but doesn't filter or sort by them. Consider promoting high-action intents and demoting zero-action turns in the selection logic.

> **[ANNOTATION 2026-02-14, v3.1 — resolves v1.2]:** Action-weighted intent filtering now implemented. `INTENTS_SQL` in `src/s4_context.rs` joins `prompts → observations` via `prompt_id`, uses `HAVING COUNT(o.id) > 0` to exclude zero-action conversational turns, and shows top 10 recent intents with action counts. Output format: `- [2m ago] "commit this" → 4 actions`. Content truncated to 60 chars. Appears as `## Recent Intents` section before observation tables in context injection. 3 unit tests + 2 integration tests.

## Harness Independence

The adversarial question: what if Claude Code disappears tomorrow?

**What survives:**
- **Data.** SQLite at `~/.nmem/nmem.db`. Standard tooling works. No proprietary format.
- **Ingestion.** Any program piping JSON to `nmem record` can store observations. The contract is simple enough for `jq`.
- **MCP queries.** MCP is an open protocol. Any MCP client connects to `nmem serve`.
- **CLI fallback.** `nmem search "auth error"` wraps the same FTS5 SQL as the MCP `search` tool. No MCP harness required — works from any terminal or script.

**What doesn't survive:**
- Hook wiring (`.claude/hooks.json` is Claude Code-specific).
- Context injection timing (SessionStart is a Claude Code concept).
- Tool call schema (`tool_name`/`tool_input`/`tool_response` is Claude Code's hook contract -- a different harness needs a different ingestion adapter).

**Design principle:** nmem's internal interfaces (SQLite schema, SQL queries, observation struct) do not reference Claude Code. Coupling is confined to two boundary points: the ingestion adapter (`nmem record`, a thin translation layer) and the MCP server (`nmem serve`, harness-agnostic).

## Open Questions

### Q1: Should `search` support recency weighting?

FTS5 BM25 ranks by term relevance, not recency. Options: BM25 only (consumer assesses timestamps), BM25 + time-decay blend, or consumer-side reordering. Lean toward BM25 only at launch — the consumer has timestamps in every result. Add custom ranking later if retrieval quality suffers.

**If needed later:** `sqlite-retrieval-patterns.md` § 2 documents exponential time-decay blending with BM25 via a CTE-based composite score: `(0.7 * bm25_norm) + (0.3 * recency_score)`. This can be dropped into the `search` SQL sketch without schema changes — only the ORDER BY clause changes. The `orderBy` parameter (already in the MCP tool spec) could accept `"relevance"` (BM25 only, default) or `"blended"` (BM25 + recency).

> **[ANNOTATION 2026-02-14, v2.0]:** Recency weighting is now implemented for `recent_context` (composite scoring with `exp_decay` UDF). The `search` tool remains BM25-only — recency blending for `search` can reuse the same `exp_decay` UDF if needed later.

### Q2: Should context injection be configurable per project?

Some projects may want more or fewer injected observations, or suppress cross-project context. S5 (policy) concern. Defer until configuration strategy is decided.

## Consequences

### Positive

- **Implementation-ready contract.** Tool names, parameters, return shapes, SQL sketches specified.
- **Token-efficient retrieval.** Search-then-fetch with 120-char previews avoids dumping full content.
- **Harness-independent core.** Claude Code coupling confined to ingestion adapter.
- **Push + pull.** SessionStart injection for immediate context; MCP tools for targeted retrieval.
- **Forward-compatible ingestion.** Unknown JSON fields ignored.

### Negative

- **Six MCP tools is surface area.** Double claude-mem's three. Mitigation: distinct purposes at different abstraction levels — `search`/`get_observations` for content retrieval, `timeline`/`session_trace` for structural navigation, `recent_context`/`file_history` for contextual views.
- **No streaming model.** Request-response only. Acceptable now, limits future live dashboards.
- **Context injection is fire-and-forget.** No feedback on whether injected context was useful.

## References

- ADR-001 -- Storage layer, FTS5 configuration, SQL schema
- ADR-002 -- Observation schema (Q1), per-source extraction (Q4), obs_type values
- ADR-003 -- Process model, `nmem record`/`nmem serve`, clap subcommands
- ADR-004 -- Project scoping, project parameter defaulting convention
- `rmcp.md` -- MCP server implementation, `#[tool]` macro, stdio transport, error responses
- `fts5.md` -- FTS5 MATCH syntax, BM25 ranking, external content queries
- `sqlite-retrieval-patterns.md` -- BM25 + recency blending (Q1), FTS5+WHERE composition, composite scoring
- `rusqlite.md` -- Parameterized queries, read-only connections for MCP server
- `serde-json.md` -- JSON return shapes, serde_json::Value for dynamic responses
- `claude-code-hooks-events.md` -- Hook event schemas, common fields, tool_input per tool
- `claude-code-plugins.md` -- Hook configuration, MCP plugin setup

## Revision History

| Date | Version | Changes |
|------|---------|---------|
| 2026-02-08 | 0.1 | Stub with framing and dependencies. |
| 2026-02-14 | 1.0 | Full ADR. Protocol choice, ingestion contract, four MCP tools with parameters and return shapes, context injection format, harness independence analysis. |
| 2026-02-14 | 1.1 | Refined. FTS5 rank ordering clarified. recent_context dedup scoped to non-NULL file_path. Token budget noted as approximate. MCP error handling table added. Timeline missing-anchor behavior specified. |
| 2026-02-14 | 1.2 | Refined with library topics. Q1 recency weighting linked to sqlite-retrieval-patterns.md composite scoring. SQL sketch references to fts5.md. References: rusqlite.md, fts5.md, sqlite-retrieval-patterns.md, serde-json.md, ADR-004. |
| 2026-02-14 | 1.3 | Annotated with live data. Added recovery mode to injection contract (compact/clear get expanded limits). Noted that 56% of user intents are zero-action conversational turns — context injection should weight intents-with-actions higher. |
| 2026-02-14 | 2.0 | **Composite scoring for `recent_context`.** Replaced `ORDER BY timestamp DESC` with multi-signal scoring: recency decay (7d half-life via `exp_decay` UDF), type weight (file_edit > command > session_compact > mcp_call > file_read), project match (boost, not filter). Dedup now keeps highest-scored per file_path. Response adds `score` field (`ScoredObservation`). Added `functions` feature to rusqlite. 5 new integration tests. |
| 2026-02-14 | 2.1 | **CLI query interface.** Added `nmem search` subcommand wrapping the same FTS5 query as MCP `search`. Three output modes: default JSON index, `--full` for complete observations, `--ids` for pipe-friendly ID lists. Filters: `--project`, `--type`, `--limit`. New module `src/s1_search.rs` with inline SQL (decoupled from s1_serve.rs MCP types). 7 integration tests. Harness independence section updated from aspirational to implemented. |
| 2026-02-14 | 3.0 | **Context injection on SessionStart.** New module `src/s4_context.rs` with `generate_context()` emitting scored markdown tables to stdout. Project-local (20 rows) + cross-project (10 rows) sections. Recovery modes (`compact`/`clear`) expand to 30+15. Scoring: `exp_decay` recency (7d half-life) + type weight, deduped by file_path. Wired into `s1_record.rs::handle_session_start()` after commit+sweep, non-fatal. 5 integration tests + 7 unit tests. |
| 2026-02-14 | 3.1 | **Action-weighted intent injection (resolves v1.2 annotation).** Added `## Recent Intents` section to context injection: joins `prompts → observations` via `prompt_id`, filters zero-action conversational turns (`HAVING COUNT > 0`), shows top 10 intents with action counts. New in `src/s4_context.rs`: `INTENTS_SQL`, `IntentRow`, `query_intents()`, `format_intents()`. 3 unit tests + 2 integration tests. No schema changes. |
| 2026-02-17 | 4.0 | **Navigational tools: `session_trace` and `file_history`.** Two new read-only MCP tools filling the gap between observation-level (`timeline`) and summary-level (`session_summaries`) retrieval. `session_trace`: drill into session → prompts → observations hierarchy; temporal filters apply to both prompt and observation timestamps (observation filter in LEFT JOIN condition). `file_history`: cross-session file biography grouped by session with intent extraction from summary JSON; joins `prompts` filtered to `source='user'` for human intent. Tool count 4→6. Error handling table extended. 12 new integration tests. No schema changes. |
