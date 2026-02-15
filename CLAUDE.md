# nmem — Project CLAUDE.md

Cross-session memory for Claude Code. Captures observations (file reads, edits, commands, errors) via hooks, stores in encrypted SQLite, retrieves via MCP server and CLI search.

## Build & Test

```bash
cargo build                    # debug
cargo build --release          # optimized (opt-level=z, LTO, stripped)
cargo test                     # all tests
NMEM_DB=/tmp/test.db nmem status   # test against throwaway DB
```

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
| `extract.rs` | `classify_tool()`, `extract_content()`, `extract_file_path()` |
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

Three tables: `sessions`, `prompts`, `observations` + external FTS5 indexes (`observations_fts`, `prompts_fts`). Schema versioned via `rusqlite_migration` `user_version` PRAGMA.

Key PRAGMAs: `journal_mode=WAL`, `synchronous=NORMAL`, `busy_timeout=5000`, `foreign_keys=ON`.

Encryption key resolution: `NMEM_KEY` env → `encryption.key_file` in config → `~/.nmem/key` (auto-generated).

## Config

`~/.nmem/config.toml` (override: `NMEM_CONFIG` env).

Sections: `[filter]` (secret patterns, entropy), `[projects.<name>]` (sensitivity, context limits), `[encryption]` (key_file), `[retention]` (per-type TTL in days), `[metrics]` (OTLP endpoint).

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

## Conventions

- Observations are facts, not interpretations — extract structured data from tool calls
- Dedup at write time (check session + obs_type + file_path + timestamp window)
- Content truncated at 2000 chars for prompts
- Secret filtering runs before storage, never after
- `is_pinned` exempts observations from retention sweeps
