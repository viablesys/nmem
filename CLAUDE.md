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
- `s1_record.rs` — hook event handling, observation storage
- `s1_extract.rs` — tool classification, content extraction
- `s5_filter.rs` — secret redaction patterns
- `s1_context.rs` — SessionStart context injection
- `s14_summarize.rs` — end-of-session summarization
- `s14_transcript.rs` — thinking block extraction
- `s5_config.rs` — config parsing (affects all hooks)
- `schema.rs` — DB migrations
- `db.rs` — connection setup, encryption

**Rebuild takes effect immediately for hooks** (each hook invocation is a fresh process). **MCP server (`s1_serve.rs`) and CLI changes take effect next session** — the MCP server is a long-lived subprocess that restarts when a new Claude Code session starts.

**No rebuild needed for:**
- `design/`, `TODO.md`, `CLAUDE.md` — docs

## VSM Mapping

nmem is designed around Stafford Beer's Viable System Model. Every module maps to a VSM system. The naming convention (S1, S2, S1's S4) is used throughout TODO.md, VSM.md, and commit messages.

| System | Role in nmem | Modules |
|--------|-------------|---------|
| **S1** Operations | Capture, store, retrieve | `s1_record.rs`, `s1_extract.rs`, `s1_serve.rs`, `s1_search.rs`, `s1_context.rs`, `s1_pin.rs` |
| **S1's S4** | Session summarization — S1's own intelligence layer | `s14_summarize.rs`, `s14_transcript.rs` |
| **S2** Coordination | Dedup, ordering, concurrency | SQLite WAL, dedup checks in `s1_record.rs` |
| **S3** Control | Storage budgets, retention, compaction | `s3_sweep.rs`, `s3_maintain.rs`, `s3_purge.rs` |
| **S3*** Audit | Integrity checks | `s3_maintain.rs` (FTS rebuild, integrity) |
| **S4** Intelligence | Work unit detection, cross-session patterns | Designed, not implemented |
| **S5** Policy | Config, identity, boundaries | `s5_config.rs`, `s5_filter.rs`, `s5_project.rs`, ADRs |

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

Files are prefixed by VSM layer: `s1_` (Operations), `s14_` (S1's S4), `s3_` (Control), `s5_` (Policy). Unprefixed files are infrastructure.

| Module | Layer | Role |
|--------|-------|------|
| `main.rs` | infra | CLI dispatch, `run()` entry point |
| `cli.rs` | infra | clap derive definitions only |
| `db.rs` | infra | `open_db()`, SQLCipher key management, PRAGMAs |
| `schema.rs` | infra | `rusqlite_migration` definitions (2 migrations) |
| `metrics.rs` | infra | Optional OTLP metrics export |
| `status.rs` | infra | Status reporting |
| `s1_record.rs` | S1 | Hook stdin → JSON → observation extraction + storage |
| `s1_serve.rs` | S1 | MCP server (`NmemServer`), tools: `search`, `get_observations`, `recent_context` |
| `s1_search.rs` | S1 | CLI search with BM25 + recency blended ranking |
| `s1_extract.rs` | S1 | `classify_tool()`, `classify_bash()`, `extract_content()`, `extract_file_path()` |
| `s1_context.rs` | S1 | SessionStart context injection (intents + local/cross-project obs) |
| `s1_pin.rs` | S1 | Pin/unpin observations |
| `s14_summarize.rs` | S1's S4 | End-of-session LLM summarization, VictoriaLogs streaming |
| `s14_transcript.rs` | S1's S4 | Scan transcript for prompt tracking |
| `s3_sweep.rs` | S3 | Retention-based purge (per obs_type TTL, respects pins) |
| `s3_maintain.rs` | S3 | Vacuum, WAL checkpoint, FTS integrity/rebuild |
| `s3_purge.rs` | S3 | Manual purge by date/project/session/type/search |
| `s5_config.rs` | S5 | TOML config loading from `~/.nmem/config.toml` |
| `s5_filter.rs` | S5 | `SecretFilter` — regex patterns + Shannon entropy redaction |
| `s5_project.rs` | S5 | Derive project name from cwd |

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
- `HookPayload` — deserialized hook JSON (in `s1_record.rs`)
- `SecretFilter` — `RegexSet` + entropy detection (in `s5_filter.rs`)
- `NmemConfig` — full config tree (in `s5_config.rs`)
- `NmemServer` — MCP server state, holds DB path (in `s1_serve.rs`)

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
| `s1_search.rs` | `fts5.md`, `sqlite-retrieval-patterns.md` |
| `s1_serve.rs` | `rmcp.md`, `fts5.md` |
| `s1_record.rs`, `s1_extract.rs` | `claude-code-hooks-events.md`, `serde-json.md` |
| `s5_filter.rs` | `regex.md` |
| `s1_context.rs` | `sqlite-retrieval-patterns.md` |
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
