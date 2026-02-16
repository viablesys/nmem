# nmem — Project CLAUDE.md

Cross-session memory for Claude Code. Captures observations (file reads, edits, commands, errors) via hooks, stores in encrypted SQLite, retrieves via MCP server and CLI search.

## Build & Test

```bash
cargo build                    # debug
cargo build --release          # optimized (opt-level=z, LTO, stripped)
cargo test                     # all tests
NMEM_DB=/tmp/test.db nmem status   # test against throwaway DB
```

### When to rebuild release

The Claude Code hook runs `target/release/nmem record` — the **release binary**, not the debug build. Changes to hook-path code are invisible until you rebuild:

```bash
cargo build --release
```

**Must rebuild after changing:**
- `record.rs` — hook event handling, observation storage
- `extract.rs` — tool classification, content extraction
- `filter.rs` — secret redaction patterns
- `context.rs` — SessionStart context injection
- `summarize.rs` — end-of-session summarization
- `transcript.rs` — thinking block extraction
- `config.rs` — config parsing (affects all hooks)
- `schema.rs` — DB migrations
- `db.rs` — connection setup, encryption

**Rebuild takes effect immediately for hooks** (each hook invocation is a fresh process). **MCP server (`serve.rs`) and CLI changes take effect next session** — the MCP server is a long-lived subprocess that restarts when a new Claude Code session starts.

**No rebuild needed for:**
- `design/`, `TODO.md`, `CLAUDE.md` — docs

## VSM Mapping

nmem is designed around Stafford Beer's Viable System Model. Every module maps to a VSM system. The naming convention (S1, S2, S1's S4) is used throughout TODO.md, VSM.md, and commit messages.

| System | Role in nmem | Modules |
|--------|-------------|---------|
| **S1** Operations | Capture, store, retrieve | `record.rs`, `extract.rs`, `serve.rs`, `search.rs`, `context.rs` |
| **S1's S4** | Session summarization — S1's own intelligence layer | `summarize.rs`, `transcript.rs` |
| **S2** Coordination | Dedup, ordering, concurrency | SQLite WAL, dedup checks in `record.rs` |
| **S3** Control | Storage budgets, retention, compaction | `sweep.rs`, `maintain.rs`, `purge.rs` |
| **S3*** Audit | Integrity checks | `maintain.rs` (FTS rebuild, integrity) |
| **S4** Intelligence | Work unit detection, cross-session patterns | Designed, not implemented |
| **S5** Policy | Config, identity, boundaries | `config.rs`, ADRs |

**"S1's S4"** means S1 is itself a viable system (VSM recursion). S1's S4 is the intelligence layer *within* operations — session summarization that compresses what happened within a session. The outer S4 synthesizes *across* sessions. S1's S4 must work before the outer S4 can build on it.

**Current state**: S1 functional (S1's S4 validated), S2 functional, S3 manual, S4 designed. See `design/VSM.md` for full assessment.

## Architecture

No daemon. Three process modes:

1. **Hook handler** (`nmem record`) — standalone process per hook event, reads JSON from stdin
2. **MCP server** (`nmem serve`) — session-scoped subprocess on stdio, read-only queries
3. **CLI** — manual search, maintenance, purge, pin/unpin

### Hook event flow

```
SessionStart → create session row, inject context into stdout
PostToolUse  → extract observation from tool_input/tool_response, dedup, write
Stop         → mark session ended, compute signature, WAL checkpoint
```

### Module map

| Module | Role |
|--------|------|
| `main.rs` | CLI dispatch, `run()` entry point |
| `cli.rs` | clap derive definitions only |
| `record.rs` | Hook stdin → JSON → observation extraction + storage |
| `serve.rs` | MCP server (`NmemServer`), tools: `search`, `get_observations`, `recent_context` |
| `search.rs` | CLI search with BM25 + recency blended ranking |
| `extract.rs` | `classify_tool()`, `classify_bash()`, `extract_content()`, `extract_file_path()` |
| `summarize.rs` | End-of-session LLM summarization, VictoriaLogs streaming |
| `filter.rs` | `SecretFilter` — regex patterns + Shannon entropy redaction |
| `context.rs` | SessionStart context injection (intents + local/cross-project obs) |
| `db.rs` | `open_db()`, SQLCipher key management, PRAGMAs |
| `schema.rs` | `rusqlite_migration` definitions (2 migrations) |
| `config.rs` | TOML config loading from `~/.nmem/config.toml` |
| `project.rs` | Derive project name from cwd |
| `sweep.rs` | Retention-based purge (per obs_type TTL, respects pins) |
| `maintain.rs` | Vacuum, WAL checkpoint, FTS integrity/rebuild |
| `purge.rs` | Manual purge by date/project/session/type/search |
| `pin.rs` | Pin/unpin observations |
| `transcript.rs` | Scan transcript for prompt tracking |
| `metrics.rs` | Optional OTLP metrics export |

## Database

SQLite with `bundled-sqlcipher`. DB at `~/.nmem/nmem.db` (override: `--db` or `NMEM_DB`).

Three tables: `sessions`, `prompts`, `observations` + external FTS5 indexes (`observations_fts`, `prompts_fts`). Full schema in `design/SCHEMA.md`. Schema versioned via `rusqlite_migration` `user_version` PRAGMA.

Key PRAGMAs: `journal_mode=WAL`, `synchronous=NORMAL`, `busy_timeout=5000`, `foreign_keys=ON`.

Encryption key resolution: `NMEM_KEY` env → `encryption.key_file` in config → `~/.nmem/key` (auto-generated).

## Config

`~/.nmem/config.toml` (override: `NMEM_CONFIG` env).

Sections: `[filter]` (secret patterns, entropy), `[projects.<name>]` (sensitivity, context limits), `[encryption]` (key_file), `[retention]` (per-type TTL in days), `[metrics]` (OTLP endpoint), `[summarization]` (enabled, endpoint, model, timeout).

## External services

| Service | Port | Purpose |
|---------|------|---------|
| LM Studio | 1234 | Local LLM for session summarization (OpenAI-compatible API) |
| VictoriaMetrics | 8428 | OTLP metrics ingestion (Prometheus-compatible) |
| VictoriaLogs | 9428 | Structured log ingestion (jsonline) — session summaries streamed here |
| Grafana | 3000 | Dashboard at `nmem-memory-pipeline` UID, auth `admin:admin` |

LM Studio must have a model loaded for summarization to work. Default: `ibm/granite-4-h-tiny`. All services are localhost, all streaming is non-fatal (failures don't block hooks).

## Key types

- `NmemError` — `Database | Io | Json | Config` (in `lib.rs`)
- `HookPayload` — deserialized hook JSON (in `record.rs`)
- `SecretFilter` — `RegexSet` + entropy detection (in `filter.rs`)
- `NmemConfig` — full config tree (in `config.rs`)
- `NmemServer` — MCP server state, holds DB path (in `serve.rs`)

## Design docs

ADRs in `design/ADR/`. Read before changing load-bearing decisions:
- ADR-001: Storage layer (SQLite, FTS5, encryption)
- ADR-002: Observation extraction (structured, no LLM)
- ADR-003: Daemon lifecycle (in-process hooks, no daemon)
- ADR-004: Project scoping and isolation
- ADR-005: Forgetting strategy
- ADR-006: Interface protocol
- ADR-007: Trust boundary and secrets filtering
- ADR-008: Distribution and installation

`design/DESIGN.md` has the overall design framing.

## Library docs by module

| Module | Read first |
|--------|------------|
| `db.rs` | `rusqlite.md`, `sqlcipher.md` |
| `schema.rs` | `rusqlite-migration.md` |
| `search.rs` | `fts5.md`, `sqlite-retrieval-patterns.md` |
| `serve.rs` | `rmcp.md`, `fts5.md` |
| `record.rs`, `extract.rs` | `claude-code-hooks-events.md`, `serde-json.md` |
| `filter.rs` | `regex.md` |
| `context.rs` | `sqlite-retrieval-patterns.md` |
| `cli.rs` | `clap.md` |
| `metrics.rs` | `victoria-logging.md` |
| `design/` | `meta-cognition.md`, `claude-code-plugins.md` |

## Tracking

- `design/VSM.md` — system viability assessment (S1-S5), gaps, what closes the loop. The primary roadmap.
- `TODO.md` — specific deferred features with rationale and activation triggers.

Update both when adding features or discovering bugs.

## obs_types

Observation classification vocabulary. Bash commands are sub-classified by `classify_bash()`.

| obs_type | Source | Signal |
|----------|--------|--------|
| `file_read` | Read | Investigation |
| `file_write` | Write | Execution |
| `file_edit` | Edit | Execution |
| `search` | Grep, Glob | Investigation |
| `command` | Bash (generic) | Varies |
| `git_commit` | Bash (`git commit`) | Completion — "worth keeping" |
| `git_push` | Bash (`git push`) | Completion — "worth sharing" |
| `github` | Bash (`gh` CLI) | External interaction |
| `task_spawn` | Task | Delegation |
| `web_fetch` | WebFetch | Research |
| `web_search` | WebSearch | Research |
| `mcp_call` | `*__*` tools | External tool |
| `tool_other` | Unknown tools | Uncategorized |

## Conventions

- nmem serves the agent, not the user — summaries optimize for context reconstruction by the next AI session
- Observations are facts, not interpretations — extract structured data from tool calls
- Dedup at write time (check session + obs_type + file_path + timestamp window)
- Content truncated at 2000 chars for prompts
- Secret filtering runs before storage, never after
- `is_pinned` exempts observations from retention sweeps
