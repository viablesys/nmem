# nmem Project Memory

## Project Overview
- **Location**: `~/workspace/nmem/`
- **Purpose**: Autonomous cross-session memory system, successor to claude-mem
- **Status**: Active implementation — core S1 operations complete
- **Organizing principle**: Viable System Model (VSM)
- **Key docs**: `DESIGN.md`, `ADR/CLAUDE.md`, `ADR/ADR-001` through `ADR-007`

## Implementation Status (2026-02-14)

### Shipped

| Module | File(s) | ADR | Summary |
|--------|---------|-----|---------|
| Schema | `src/schema.rs` | ADR-001 | SQLite with WAL, FTS5, 2 migrations (base + is_pinned) |
| Record | `src/s1_record.rs` | ADR-002 | Structured extraction from hook events (SessionStart, UserPromptSubmit, PostToolUse, Stop) |
| Filter | `src/s5_filter.rs` | ADR-007 | Secrets redaction — regex patterns + Shannon entropy, configurable |
| Config | `src/s5_config.rs` | ADR-005/007 | TOML config: retention policy, filter patterns, encryption key file |
| Extract | `src/s1_extract.rs` | ADR-002 | Tool classification, content extraction, file path resolution |
| Project | `src/s5_project.rs` | ADR-004 | Project derivation from cwd |
| Purge | `src/s3_purge.rs` | ADR-005 | 7 filter modes, secure delete, FTS5 sync, orphan cleanup |
| Sweep | `src/s3_sweep.rs` | ADR-005 | Type-aware retention, syntheses protection, pin-aware |
| Maintain | `src/s3_maintain.rs` | ADR-003 | Vacuum, WAL checkpoint, FTS5 integrity/rebuild, sweep trigger |
| Search | `src/s1_search.rs` | ADR-006 | FTS5 search CLI — index, full, ids modes |
| Serve | `src/s1_serve.rs` | ADR-006 | MCP server (rmcp) — search, get_observations, timeline, recent_context |
| Status | `src/status.rs` | ADR-003 | DB health: size, counts, types, last session, encryption, pinned |
| Pin | `src/s1_pin.rs` | ADR-005 | Pin/unpin observations — exempt from retention sweeps |
| DB | `src/db.rs` | ADR-001 | SQLCipher encryption, key management, migration, open/open_readonly |
| CLI | `src/cli.rs` | ADR-003 | clap derive: record, serve, purge, maintain, status, search, encrypt, pin, unpin |
| Transcript | `src/s14_transcript.rs` | ADR-002 | Session signature generation |

### Not yet implemented
- **S4 Synthesis** — LLM-based summarization of observation clusters (ADR-002 Q3)
- **Syntheses table** — source_obs_ids linking summaries to source observations
- **Auto-pinning** — S4-driven landmark detection (ADR-005 Q1 deferred)
- **Vector search** — sqlite-vec deferred (broken DELETE, ADR-001 annotation)
- **Context injection** — SessionStart hook pushing relevant context to new sessions

## Architecture

```
Hook events (stdin JSON) → nmem record → extract → filter → SQLite
                                                              ↓
                           nmem serve (MCP/stdio) ← search/retrieve
                           nmem search (CLI)      ← FTS5 queries
                           nmem status            ← health metrics
                           nmem maintain          ← vacuum/sweep
                           nmem purge             ← explicit deletion
                           nmem pin/unpin         ← landmark protection
```

Single binary, no daemon. All operations open/close the DB per invocation. Opportunistic maintenance on SessionStart when threshold met (100+ old observations).

## Key Design Decisions

### Resolved
- **No LLM for extraction** — structured only, reserve LLM for S4 synthesis
- **No vector search** — FTS5 + composite scoring sufficient, sqlite-vec deferred
- **No daemon** — in-process hooks, opportunistic maintenance
- **Single DB** — project scoping via column, not separate files
- **SQLCipher** — optional encryption at rest, raw hex key (no PBKDF2)
- **Type-aware retention** — configurable per obs_type, disabled by default
- **Observation pinning** — is_pinned flag, exempt from sweeps, purge as escape valve

### Key Findings (2026-02-08)
- sqlite-vec DELETE is broken (issues #178, #54, #220)
- sqlite-vec uses brute-force KNN, not HNSW
- PRAGMA synchronous = NORMAL with WAL: safe from corruption, not durable on power loss

## ADR Structure

| ADR | Title | Status |
|-----|-------|--------|
| ADR-001 | Storage Layer | Accepted, implemented (v3.3) |
| ADR-002 | Observation Extraction Strategy | Accepted, implemented |
| ADR-003 | Daemon Lifecycle | Accepted, implemented (no daemon) |
| ADR-004 | Project Scoping & Isolation | Accepted, implemented |
| ADR-005 | Forgetting Strategy | Accepted, implemented (v4.0 — purge + sweep + pin) |
| ADR-006 | Interface Protocol | Accepted, implemented (MCP serve + CLI search) |
| ADR-007 | Trust Boundary & Secrets Filtering | Accepted, implemented |

## Test Coverage

- 42 unit tests (schema, config, extract, filter, db, sweep, purge)
- 42 integration tests (full CLI lifecycle via assert_cmd + tempfile)
- 23 serve integration tests (MCP tool methods via in-memory DB)
- 107 total, all passing

## Recent Changes (2026-02-14)

### Observation Pinning (ADR-005 v4.0)
- Schema migration 2: `ALTER TABLE observations ADD COLUMN is_pinned INTEGER NOT NULL DEFAULT 0`
- `nmem pin <id>` / `nmem unpin <id>` CLI commands
- Sweep: `AND is_pinned = 0` guards both DELETE branches
- Purge: ignores pin status (escape valve for explicit deletion)
- Search/serve: `is_pinned` field in all JSON output (SearchResult, FullObservation, ScoredObservation)
- Status: pinned count displayed when > 0
- 10 new tests (7 integration + 2 serve + 1 unit)
