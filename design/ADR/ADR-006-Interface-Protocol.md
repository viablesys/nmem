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

Four tools exposed via `nmem serve` (session-scoped stdio MCP server, read-only SQLite connection).

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

**Returns:** JSON array of recent observations (full schema), newest first. Selection:

1. Most recent project-local observations by `timestamp DESC`.
2. Cross-project backfill if project has fewer than `limit` results.
3. Deduplicated by `file_path` where `file_path IS NOT NULL` (keep most recent per path). Observations with NULL `file_path` (commands, errors, prompts) are never deduplicated — each is unique context.

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

Database errors (connection failure, corruption) return MCP internal errors with the SQLite error message. These should not expose file paths or internal state beyond what SQLite reports.

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
| Output | stdout plain text or `hookSpecificOutput.additionalContext` |
| Format | Markdown table, compact, scannable |
| Max lines | ~50 lines (~350-500 tokens, varies by content length). Competes with CLAUDE.md for context window. |
| Timing | SessionStart only (`source` in `startup`, `resume`, `clear`, `compact`) |
| Recovery mode | On `compact`/`clear`: expanded limits (more intents, files, threads) + recent actions trail. Agent lost its context window — inject more to compensate. |
| Selection | 20 project-local + up to 10 cross-project backfill |

> **[ANNOTATION 2026-02-14, v1.2]:** Live data shows 56% of user intents trigger zero tool actions (conversational turns: "yes", "i do", questions). Context injection should weight intents-with-actions higher than bare conversational turns. The prototype context generator already shows action counts per intent (`→ N actions`) but doesn't filter or sort by them. Consider promoting high-action intents and demoting zero-action turns in the selection logic.

## Harness Independence

The adversarial question: what if Claude Code disappears tomorrow?

**What survives:**
- **Data.** SQLite at `~/.nmem/nmem.db`. Standard tooling works. No proprietary format.
- **Ingestion.** Any program piping JSON to `nmem record` can store observations. The contract is simple enough for `jq`.
- **MCP queries.** MCP is an open protocol. Any MCP client connects to `nmem serve`.
- **CLI fallback.** The query logic is protocol-independent. `nmem search "auth error"` wraps the same SQL as the MCP `search` tool. Not at launch, but zero additional storage work.

**What doesn't survive:**
- Hook wiring (`.claude/hooks.json` is Claude Code-specific).
- Context injection timing (SessionStart is a Claude Code concept).
- Tool call schema (`tool_name`/`tool_input`/`tool_response` is Claude Code's hook contract -- a different harness needs a different ingestion adapter).

**Design principle:** nmem's internal interfaces (SQLite schema, SQL queries, observation struct) do not reference Claude Code. Coupling is confined to two boundary points: the ingestion adapter (`nmem record`, a thin translation layer) and the MCP server (`nmem serve`, harness-agnostic).

## Open Questions

### Q1: Should `search` support recency weighting?

FTS5 BM25 ranks by term relevance, not recency. Options: BM25 only (consumer assesses timestamps), BM25 + time-decay blend, or consumer-side reordering. Lean toward BM25 only at launch — the consumer has timestamps in every result. Add custom ranking later if retrieval quality suffers.

**If needed later:** `sqlite-retrieval-patterns.md` § 2 documents exponential time-decay blending with BM25 via a CTE-based composite score: `(0.7 * bm25_norm) + (0.3 * recency_score)`. This can be dropped into the `search` SQL sketch without schema changes — only the ORDER BY clause changes. The `orderBy` parameter (already in the MCP tool spec) could accept `"relevance"` (BM25 only, default) or `"blended"` (BM25 + recency).

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

- **Four MCP tools is surface area.** One more than claude-mem's three. Mitigation: distinct purposes, no overlap.
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
