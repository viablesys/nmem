# ADR-001: Storage Layer

## Status
Accepted

## Context

nmem is a local-first, single-user developer tool for cross-session memory. Its storage requirements:

- **Write volume**: Low-moderate. Structured observations from every tool call — 50-250 writes per session (after dedup), a few sessions per day. Thousands of observations per month. See ADR-002 Q2/Q5.
- **Read pattern**: Retrieval at session start (recent + relevant observations) and on-demand queries during sessions. Read-heavy relative to writes.
- **Data lifetime**: Observations accumulate over months. Annual volume at active use: 36,000-360,000 rows (unfiltered capture, ADR-002 Q2). Still a small-data problem for SQLite.
- **Reliability**: Data is developer session notes, not business-critical state. Loss costs accumulated context, not money. Corruption resistance matters; disaster recovery does not.
- **Concurrency**: A daemon or session process writes while MCP queries read. Single writer, multiple readers.
- **Deployment**: Runs on the developer's machine. No servers, no cloud, no containers.

### Predecessor

claude-mem used SQLite (WAL mode) + Chroma (vector DB). SQLite was reliable — no corruption, concurrent access worked. Chroma added a separate process, Python dependency, and embedding generation for questionable retrieval quality. The MCP query interface was essentially structured SQL with pagination. There is no evidence that vector search found observations that structured queries could not.

## Decision

### Core: SQLite + rusqlite

```toml
[dependencies]
rusqlite = { version = "0.38", features = ["bundled", "backup", "hooks", "serde_json"] }
rusqlite_migration = "2.3"
tokio-rusqlite = "0.6"
```

`bundled` compiles SQLite from source (currently 3.51.1), eliminating system SQLite version dependencies. This also includes FTS5 and JSON1.

**Database location:** `~/.nmem/nmem.db` (default). Single file, user-local. ADR-004 may introduce per-project databases — if so, this becomes the global/cross-project database.

Feature flags selected for nmem:

| Feature | Purpose |
|---------|---------|
| `bundled` | Consistent SQLite version, includes FTS5/JSON1 |
| `backup` | SQLite online backup API for file-level backups |
| `hooks` | WAL hook for checkpoint control, update notifications |
| `serde_json` | Direct `serde_json::Value` storage/retrieval for flexible fields |

Not needed at launch: `load_extension` (no external extensions), `vtab` (no virtual tables beyond FTS5), `blob` (no large binary storage), `chrono`/`time` (store timestamps as integers).

### Schema

Three tables: sessions, prompts, observations. User prompts are stored separately as intent markers — they frame the "why" for subsequent tool observations. Observations reference their preceding prompt via foreign key.

```sql
-- 001_initial.sql

CREATE TABLE sessions (
    id          TEXT PRIMARY KEY,   -- Claude Code session UUID
    project     TEXT NOT NULL,      -- derived from cwd (e.g. "nmem", "library")
    started_at  INTEGER NOT NULL,   -- unix timestamp (seconds)
    ended_at    INTEGER,            -- null until Stop hook
    signature   TEXT,               -- JSON: event type distribution, computed at session end
    summary     TEXT                -- session summary from Stop hook
);

CREATE TABLE prompts (
    id          INTEGER PRIMARY KEY,
    session_id  TEXT NOT NULL REFERENCES sessions(id),
    timestamp   INTEGER NOT NULL,   -- unix timestamp (seconds)
    source      TEXT NOT NULL,       -- "user" (directive) or "agent" (reasoning)
    content     TEXT NOT NULL        -- prompt text or thinking block, truncated
);

CREATE TABLE observations (
    id          INTEGER PRIMARY KEY,
    session_id  TEXT NOT NULL REFERENCES sessions(id),
    prompt_id   INTEGER REFERENCES prompts(id),  -- most recent prompt: user directive or agent reasoning
    timestamp   INTEGER NOT NULL,   -- unix timestamp (seconds)
    obs_type    TEXT NOT NULL,       -- file_read, file_edit, command, search, session_compact, etc.
    source_event TEXT NOT NULL,      -- PostToolUse, SessionStart, Stop
    tool_name   TEXT,                -- Bash, Read, Edit, Write, Grep, etc.
    file_path   TEXT,                -- normalized absolute path, when applicable
    content     TEXT NOT NULL,       -- extracted content (command, pattern, description)
    metadata    TEXT                 -- JSON: flexible extra fields
);

-- Indexes for dedup and retrieval
CREATE INDEX idx_obs_dedup ON observations(session_id, obs_type, file_path, timestamp);
CREATE INDEX idx_obs_session ON observations(session_id, timestamp);
CREATE INDEX idx_obs_prompt ON observations(prompt_id);
CREATE INDEX idx_obs_type ON observations(obs_type);
CREATE INDEX idx_obs_file ON observations(file_path) WHERE file_path IS NOT NULL;
CREATE INDEX idx_prompts_session ON prompts(session_id, id);
```

**Design rationale:**

- **Prompts as a separate table.** Both user directives and agent reasoning are intent markers that frame subsequent tool calls. Storing them once and referencing by ID avoids duplication. At retrieval, one join reconstructs intent: `SELECT o.*, p.content AS intent, p.source FROM observations o LEFT JOIN prompts p ON o.prompt_id = p.id`.
- **source distinguishes origin, not importance.** User prompts ("fix the bug") and agent reasoning ("The user wants me to...") are peers. `source = 'user'` filters to directives; `source = 'agent'` filters to reasoning. Both are FTS-indexed and searchable.
- **prompt_id is nullable.** SessionStart and early tool calls before the first user prompt have no intent context. Tool calls during autonomous agent work (task_spawn chains) may also lack a direct prompt.
- **content is the extraction target.** For file operations: the path. For commands: the command string. For searches: the pattern. This is what FTS5 indexes. The `file_path` column duplicates the path for structured queries without parsing content.
- **metadata as JSON.** Escape hatch for per-obs-type fields that don't warrant columns yet. If a field appears on >30% of observations, promote it to a column in a migration.
- **Timestamps as integer seconds.** Millisecond precision isn't needed for a memory system. Seconds simplify dedup window math and index comparison.
- **Effort signals as observations.** Context compaction (`session_compact`), resume (`session_resume`), and clear (`session_clear`) events are stored as observations with `source_event = 'SessionStart'`. Compaction count per session is a proxy for context window exhaustion — effort expenditure that's otherwise invisible. A session with 3 compactions consumed ~3x the context of one that stayed in a single window.

### Text Search: FTS5

FTS5 is built into bundled SQLite — zero additional cost. Provides BM25-ranked full-text search sufficient for the expected data volume and query patterns.

```sql
-- External content table: index without duplicating data
CREATE VIRTUAL TABLE observations_fts USING fts5(
    content,
    content='observations',
    content_rowid='id',
    tokenize='porter unicode61'
);

-- Sync triggers (external content tables require manual sync)
CREATE TRIGGER observations_ai AFTER INSERT ON observations BEGIN
    INSERT INTO observations_fts(rowid, content) VALUES (new.id, new.content);
END;

CREATE TRIGGER observations_ad AFTER DELETE ON observations BEGIN
    INSERT INTO observations_fts(observations_fts, rowid, content)
        VALUES('delete', old.id, old.content);
END;

CREATE TRIGGER observations_au AFTER UPDATE ON observations BEGIN
    INSERT INTO observations_fts(observations_fts, rowid, content)
        VALUES('delete', old.id, old.content);
    INSERT INTO observations_fts(rowid, content) VALUES (new.id, new.content);
END;
```

Both tables use external content FTS — the index is a derived artifact, rebuildable with `INSERT INTO <table>_fts(<table>_fts) VALUES('rebuild')`. The porter tokenizer stems English words ("running" matches "run"), which suits observation prose and agent reasoning. For exact substring matching on code or file paths, a trigram tokenizer would be needed — defer unless retrieval misses justify it.

Prompts are also FTS-indexed:

```sql
CREATE VIRTUAL TABLE prompts_fts USING fts5(
    content,
    content='prompts',
    content_rowid='id',
    tokenize='porter unicode61'
);

CREATE TRIGGER prompts_ai AFTER INSERT ON prompts BEGIN
    INSERT INTO prompts_fts(rowid, content) VALUES (new.id, new.content);
END;

CREATE TRIGGER prompts_ad AFTER DELETE ON prompts BEGIN
    INSERT INTO prompts_fts(prompts_fts, rowid, content)
        VALUES('delete', old.id, old.content);
END;
```

FTS on prompts enables decision trail reconstruction: searching for "store everything" across both tables returns the user directive, the agent's interpretation, and the resulting actions — the complete intent-to-action chain.

At nmem's data volume (<20K rows annually), even `LIKE '%term%'` scans complete in milliseconds. FTS5 is an optimization, not a requirement. But it's free and handles boolean queries (`AND`, `OR`, `NOT`), phrase matching, and prefix search — capabilities that LIKE cannot provide.

### PRAGMA Configuration

```sql
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA busy_timeout = 5000;
PRAGMA temp_store = MEMORY;
PRAGMA foreign_keys = ON;
PRAGMA auto_vacuum = INCREMENTAL;
```

All PRAGMAs except `journal_mode` and `auto_vacuum` must be set per-connection — they are not persisted in the database file. `journal_mode = WAL` persists once set. `auto_vacuum` must be set before the first table is created.

Read-only connections (MCP readers) need only `busy_timeout` and `temp_store`. Skip `foreign_keys` (no writes), `auto_vacuum` (write-side concern), and `synchronous` (irrelevant for reads).

| PRAGMA | Value | Rationale |
|--------|-------|-----------|
| `journal_mode` | `WAL` | Concurrent readers + single writer without blocking. Persistent across connections. |
| `synchronous` | `NORMAL` | Corruption-safe and always consistent. **Not durable on power loss** — a committed transaction can roll back if the OS crashes or power fails (application crashes are always durable). Acceptable: losing a few observations on power failure is trivial. |
| `busy_timeout` | `5000` | 5 seconds of retry on `SQLITE_BUSY`. Handles contention between writer and reader connections without immediate failure. |
| `temp_store` | `MEMORY` | Temp tables and indices in RAM. Minor performance gain for sort/aggregate operations. |
| `foreign_keys` | `ON` | Enforced referential integrity. Per-connection (SQLite default is OFF). |
| `auto_vacuum` | `INCREMENTAL` | Reclaims space from deleted rows without locking the database for a full VACUUM. Run `PRAGMA incremental_vacuum(N)` periodically to reclaim N pages. |

### Concurrent Access

WAL mode gives nmem the concurrency model it needs:

- **Writer** (daemon or session process): Holds a single write connection. Uses `TransactionBehavior::Immediate` for write transactions to fail fast on contention rather than deadlocking.
- **Readers** (MCP query connections): Open read-only connections. Never block the writer, never blocked by the writer.
- **Checkpoint**: WAL auto-checkpoints at 1000 pages by default. For nmem's low write volume this is infrequent. On graceful shutdown, run `PRAGMA wal_checkpoint(TRUNCATE)` to fold the WAL back into the main database file — this eliminates the `.db-wal` and `.db-shm` files, leaving a clean single-file state for backup or portability.
- **Multiple sessions**: If two Claude Code sessions run simultaneously, both can write to the same database. `busy_timeout` handles contention — one writer blocks briefly while the other commits. At nmem's write frequency (seconds between writes), contention is effectively nonexistent.
- **Async access**: rusqlite is `Send` but not `Sync` — it can't be shared across async tasks. `tokio-rusqlite` wraps a connection on a dedicated OS thread, accessed via `.call()` closures that move into the thread. This is the expected pattern for nmem's daemon. Reader and writer connections should be separate `tokio-rusqlite` instances.

### Schema Migration

`rusqlite_migration` tracks schema version via SQLite's `user_version` PRAGMA — a single integer at a fixed file offset, no extra tables. Migrations run on connection open.

```rust
use rusqlite_migration::{Migrations, M};

const MIGRATIONS: Migrations<'static> = Migrations::from_slice(&[
    M::up(include_str!("../migrations/001_initial.sql")),
    // Future migrations appended here
]);

fn open_db(path: &Path) -> Result<Connection, Box<dyn std::error::Error>> {
    let mut conn = Connection::open(path)?;
    // PRAGMAs first — most are per-connection and must be set before
    // any queries, including migrations.
    configure_pragmas(&conn)?;
    MIGRATIONS.to_latest(&mut conn)?;
    Ok(conn)
}
```

### Backup Strategy

Proportional to the data's actual value:

1. **Default: No backup.** nmem data is rebuilt naturally through continued use. A new database starts cold and warms over sessions. This is acceptable for most users.
2. **Optional: SQLite `.backup` API.** The `backup` feature enables programmatic hot backups. nmem could offer a `backup` command that copies the database to a user-specified location. Non-blocking (uses SQLite's incremental backup, doesn't lock readers).
3. **Simplest: File copy.** Run `PRAGMA wal_checkpoint(TRUNCATE)` first, then copy the `.db` file alone. Without checkpointing, all three files (`.db`, `.db-wal`, `.db-shm`) must be copied atomically — unreliable while the writer is active. For unattended copies, prefer the backup API.

Litestream, cloud replication, and point-in-time recovery are out of scope. If nmem's data ever becomes valuable enough to warrant continuous replication, that decision can be revisited — but the architecture doesn't need to accommodate it now.

## Extensions

### Included (built-in, zero cost)

- **FTS5**: Full-text search with BM25 ranking.
- **JSON1**: `json_extract()`, `json_each()`, etc. for flexible field queries.

### Excluded

Principle: **thin database, smart application.** Rust's ecosystem handles computation better than SQL extensions — with type safety, better error handling, and no FFI/extension loading complexity.

| Extension | Reason for exclusion |
|-----------|---------------------|
| sqlite-http | HTTP in Rust (reqwest, ureq) |
| SQLean/text | String ops in Rust |
| SQLean/math, SQLean/stats | Math in Rust (std, statrs) |
| sqlite-regex | Regex in Rust (regex crate) |
| sqlite-lines | File I/O in Rust |
| sqlite-zstd | Compression in Rust if needed |
| spatialite | Irrelevant to nmem |

### Conditional (not at launch)

- **sqlite-vec**: Vector similarity search. Deferred per ADR-002 — the extraction strategy (structured, no LLM) means observations are typed and indexed, making FTS5 + metadata queries sufficient for retrieval. sqlite-vec also has unresolved issues: DELETE doesn't remove vector blobs (GitHub #178, #54), VACUUM doesn't reclaim space (#220), and no optimize/vacuum command exists (#184, #185). Revisit only if retrieval quality degrades demonstrably at scale (>10K observations) and structured queries can't close the gap.
- **SQLCipher** (via `bundled-sqlcipher` feature): Encryption at rest. See open question below.

## Open Questions

### Encryption at rest

Not decided. Options and their trade-offs:

| Option | Pros | Cons |
|--------|------|------|
| **None** (filesystem permissions) | Zero complexity. Standard tooling works (`sqlite3` CLI). | Secrets stored in plaintext if filtering fails. |
| **SQLCipher** (`bundled-sqlcipher`) | Transparent full-database encryption. Replaces SQLite library. | Incompatible with standard `sqlite3` CLI. May complicate extension loading if sqlite-vec is ever added. Performance overhead (~5-15%). |
| **Filesystem encryption** (LUKS, fscrypt) | Transparent to application. No code changes. | OS-dependent. Not portable. User must configure it. |
| **Application-level field encryption** | Selective — only sensitive fields encrypted. | Can't query encrypted fields. Complex key management. |

The decision connects to ADR-007 (Trust Boundary): if secrets filtering is robust, encryption is defense-in-depth. If filtering is fallible, encryption is load-bearing. Defer until ADR-007 establishes the filtering strategy.

If SQLCipher is chosen, rusqlite supports it directly:

```toml
rusqlite = { version = "0.38", features = ["bundled-sqlcipher"] }
```

This replaces the `bundled` feature — they are mutually exclusive.

### Storage budget

With unfiltered capture (ADR-002 Q2: store everything, dedup handles noise), volumes are calibrated from real data. The prototype DB loaded 242 sessions (~6 weeks of use) producing 5,353 prompts and 3,571 observations in 5 MB. Extrapolating:

| Timeframe | Prompts | Observations | DB size (with FTS5) |
|-----------|---------|-------------|---------------------|
| Per month | ~3,500 | ~2,400 | ~3 MB |
| Per year | ~42,000 | ~29,000 | ~36 MB |
| 5-year ceiling | ~210,000 | ~145,000 | ~180 MB |

These estimates include agent reasoning (35% of prompts by count, 64% by content volume). The 73.9x compression ratio from raw transcripts (358 MB → 5 MB) validates that structured extraction captures sufficient signal at low storage cost.

Even the high end is well within SQLite's capabilities — indexed queries at 1M+ rows remain single-digit milliseconds. Storage budgets are not a day-one concern. `auto_vacuum = INCREMENTAL` handles space reclamation from deletions. A `PRAGMA page_count * PRAGMA page_size` query reports actual database size for monitoring.

If storage becomes a concern (years of accumulation, or if S4 synthesis is added later), ADR-005 (Forgetting) addresses retention and compaction strategies.

## Consequences

### Positive

- **Single file**: The database is one file (plus WAL/SHM during operation). Portable — copy to another machine and it works. No absolute paths in the storage layer.
- **Zero infrastructure**: No separate processes, no cloud credentials, no systemd services.
- **Predictable performance**: At nmem's data volume, every query is fast. Full table scans complete in milliseconds. FTS5 is an optimization, not a lifeline.
- **Rust-native**: rusqlite's type system catches errors at compile time. `Connection` is `Send` (movable between threads) — works with both sync and async Rust patterns.
- **Incremental adoption**: Start with the minimal feature set. Add FTS5 indexes when retrieval needs them. Add sqlite-vec if and when vector search proves necessary. Add SQLCipher if encryption is required. Each addition is independent.

### Negative

- **Single writer**: SQLite allows one writer at a time. Multiple concurrent sessions contend on writes. At nmem's write frequency this is a non-issue, but it's a hard architectural ceiling.
- **No native async**: rusqlite is `Send` but not `Sync`. Async access requires `tokio-rusqlite` (spawns a dedicated thread per connection). Each `.call()` closure moves into the thread and back — ergonomic but adds indirection compared to direct rusqlite access.
- **Bundled SQLite size**: Compiling SQLite from source adds ~30 seconds to clean builds and ~1.5 MB to the binary. Acceptable for a developer tool.

## References

- [rusqlite 0.38 docs](https://docs.rs/rusqlite/0.38/rusqlite/)
- [SQLite WAL mode](https://www.sqlite.org/wal.html)
- [FTS5](https://www.sqlite.org/fts5.html)
- [rusqlite_migration](https://docs.rs/rusqlite_migration/)
- [sqlite-vec](https://github.com/asg017/sqlite-vec) — deferred, see conditional extensions
- [SQLCipher](https://www.zetetic.net/sqlcipher/) — pending encryption decision

## Revision History

| Date | Version | Changes |
|------|---------|---------|
| 2026-02-08 | 1.0 | Initial draft (generated in Claude web session) |
| 2026-02-08 | 1.1 | Annotated with review against nmem requirements |
| 2026-02-14 | 2.0 | Rewritten. Scoped to nmem. Removed Litestream, enterprise framing, DR runbooks. Demoted sqlite-vec to conditional. Added concurrent access model, encryption question, storage budget analysis. Incorporated ADR-002 direction (structured extraction, no vector search at launch). |
| 2026-02-14 | 2.1 | Refined. Added FTS5 tokenizer choice + sync triggers. Added async access strategy (tokio-rusqlite). Fixed open_db error handling. Corrected WAL file copy backup advice. Removed unnecessary cache_size PRAGMA. Added PRAGMA persistence notes. |
| 2026-02-14 | 2.2 | Added tokio-rusqlite to deps. Reader vs writer PRAGMA config. WAL checkpoint on shutdown. Database file location. |
| 2026-02-14 | 2.3 | Updated volume estimates to match ADR-002 Q2 resolution (store everything, dedup handles noise). Write volume, data lifetime, and storage budget revised upward. |
| 2026-02-14 | 3.0 | Added schema: sessions, prompts, observations tables. Prompts stored separately as intent markers (option B from analysis). Indexes for dedup, retrieval, and intent joins. Derived from capture data analysis (684 events, 7 sessions) showing user prompts as work-unit boundaries. |
| 2026-02-14 | 3.1 | Unified reasoning (thinking blocks) and user prompts as first-class intents. Added `source` column ("user"/"agent") to prompts table. Both are FTS-indexed and searchable. Validated against 5,353 prompts across 97 sessions. |
| 2026-02-14 | 3.2 | Added FTS5 on prompts table (was implemented but undocumented). Added effort signal obs_types (session_compact/resume/clear). Updated volume estimates from real prototype data (73.9x compression, ~5 MB for 6 weeks). Fixed prompt_id semantics. Validated with live v2 extractor producing real hook data. |
