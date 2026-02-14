# ADR-003: Daemon Lifecycle

## Status
Accepted

## Framing
*Long-running daemon vs session-scoped process.*

Affects: concurrency model, database locking, WAL checkpointing, operator (S3) capabilities, crash recovery, and whether nmem can do background work (compaction, index rebuilds) between sessions. The adversarial angle: "What if there is no daemon?" — can a session-bootstrapped process do everything nmem needs?

## Depends On
ADR-002 (Extraction Strategy) — data volume and write patterns affect whether a daemon is justified.

## Unlocks
ADR-007 (Trust Boundary)

---

## Context

claude-mem ran a background worker on port 37777 — a Node.js HTTP server with an SDK agent (LLM subprocess). Every PostToolUse hook sent an HTTP POST to the daemon, which queued the payload for LLM extraction. The daemon:

- Held persistent connections to SQLite and Chroma
- Ran the SDK agent (a full Claude subprocess) for every observation
- Listened on a TCP port (localhost:37777)
- Had no lifecycle management — orphan processes survived session ends
- Had no health monitoring — silent failures went undetected
- Required Node.js runtime (npm, package.json, node_modules)

ADR-002 eliminates the LLM subprocess. Extraction is now deterministic parsing — string manipulation, no API calls. This fundamentally changes the daemon question: the expensive, slow, unreliable component is gone. What remains is I/O: parse a hook payload, write a row to SQLite.

## The Three Positions

### Position A: No Daemon (In-Process Hooks)

Each hook invocation is a standalone process. The hook script receives JSON on stdin, parses the tool call, writes to SQLite, and exits. No persistent process. No port. No lifecycle.

**How it works:**
- Claude Code hooks spawn a process per event (the hook handler)
- The handler opens SQLite, sets PRAGMAs, writes the observation, closes
- MCP queries open a separate read-only connection per request
- No coordination between handlers — SQLite WAL + `busy_timeout` handles concurrency

**What works:**
- Zero lifecycle management. No orphans, no PID files, no port conflicts.
- Each invocation is isolated — crashes don't cascade.
- Deployment is a single binary. No service files, no startup scripts.
- Multiple Claude Code sessions just work — each hook invocation writes independently, WAL handles contention.

**What's lost:**
- Connection overhead: open/configure/migrate on every hook invocation. At nmem's PRAGMA set + migration check, this adds ~1-5ms per call.
- No background work between sessions: FTS rebuild, incremental vacuum, S4 synthesis all need an active session to trigger them.
- No persistent state: write batching, deduplication across rapid tool calls, rate limiting all require memory that dies with each invocation.
- WAL checkpoint only happens during sessions (auto-checkpoint) or if the hook explicitly checkpoints (adds latency).

**Failure mode:** Reliable but dumb. Every observation is an isolated event. No cross-event intelligence, no batching, no background maintenance. The system works but never cleans up after itself unless a session happens to trigger it.

### Position B: Long-Running Daemon

A persistent process that starts on first use and runs indefinitely. Hooks communicate via IPC (Unix socket or localhost port). The daemon holds database connections, manages background tasks, and coordinates across sessions.

**How it works:**
- Daemon starts on first hook invocation (or explicitly via CLI)
- Holds a write connection (tokio-rusqlite) and reader connections for MCP
- Hooks send observations via Unix socket (or HTTP like claude-mem)
- Background tasks run on a schedule: incremental vacuum, WAL checkpoint, FTS rebuild, future S4 synthesis
- Graceful shutdown on signal (SIGTERM) or idle timeout

**What works:**
- Persistent connections — no per-invocation setup cost
- Background maintenance — vacuum, checkpoint, rebuild happen between sessions
- Write batching — rapid tool calls can be buffered and flushed
- Cross-event state — deduplication, rate limiting, session tracking
- S4 synthesis (future) has a natural home — periodic tasks in the daemon

**What's lost:**
- Lifecycle complexity: who starts it? Who stops it? What if it crashes? What if two instances start?
- Port/socket management: Unix socket needs a known path, TCP port needs conflict resolution
- Orphan risk: if Claude Code exits without triggering Stop hook, daemon persists
- Resource consumption: a Rust binary idling uses minimal CPU but holds memory and file descriptors
- Debugging: problems are no longer localized to a single hook invocation

**Failure mode:** claude-mem's failure. The daemon becomes the fragile center — if it crashes, all observation capture stops silently. If it wedges, hooks block waiting for IPC response. If it orphans, stale processes accumulate.

### Position C: Hybrid (In-Process Hooks + On-Demand Daemon)

Hooks write directly to SQLite (Position A). A separate daemon process handles background maintenance only — no inline observation capture. The daemon is optional and non-blocking.

**How it works:**
- Hooks are standalone processes. They open SQLite, write, close. Same as Position A.
- A maintenance daemon runs on a schedule (cron, systemd timer, or spawned by hooks periodically).
- The daemon does: incremental vacuum, WAL checkpoint(TRUNCATE), FTS integrity check, future S4 synthesis.
- If the daemon isn't running, nothing breaks — maintenance is deferred, not lost.

**What works:**
- Hook reliability from Position A — each invocation is isolated
- Background maintenance from Position B — cleanup happens independently
- Failure isolation — daemon crash doesn't affect observation capture
- Optional — works without the daemon, just with deferred maintenance

**What's lost:**
- Still need lifecycle management for the maintenance daemon (lighter than Position B, but still present)
- No write batching — each hook invocation opens/closes SQLite
- No cross-event state — deduplication requires querying recent observations on each invocation
- Two components instead of one — more to deploy, test, document

## Adversarial Analysis

### Attacking Position A: "The per-invocation overhead matters"

The strongest objection: opening SQLite, setting 6 PRAGMAs, checking migration version, writing a row, and closing — on every tool call — adds latency to every hook invocation.

**Counter:** PostToolUse hooks run *after* the tool response is already shown to the user — they don't block the conversation flow. The overhead (~3-8ms total: process spawn + binary startup + SQLite open/configure/write/close) is invisible because it happens in the background while Claude is already generating the next response.

**What about connection storms?** If two sessions run simultaneously and both generate rapid tool calls, each invocation opens its own connection. At nmem's volume (seconds between writes per session), this means 2-3 concurrent connections at peak. SQLite handles this trivially with WAL. At 100 concurrent connections, there would be contention — but that requires ~100 simultaneous Claude Code sessions, which is not a real scenario.

**Verdict:** The overhead is real but insignificant at nmem's scale. Position A's simplicity outweighs Position B's connection reuse.

### Attacking Position B: "A daemon is just good engineering"

The defense: background maintenance, write batching, and persistent state are standard server architecture. A Rust daemon is lightweight (~5MB RSS idle). The lifecycle problems are solved problems (PID files, signal handling, socket activation).

**Counter:** nmem is not a server. It's a local tool for one developer. The "solved problems" of daemon management are solved for servers with monitoring, restart policies, and ops teams. For a developer tool that should be invisible:
1. A daemon that crashes silently is worse than no daemon — the user loses observations without knowing
2. PID files and socket paths add failure modes (stale PID, permission errors, socket already in use)
3. The daemon must start before Claude Code and survive after it — who ensures this?
4. systemd/launchd integration is OS-specific and requires user configuration

claude-mem's daemon proved that a background process for a developer tool creates more problems than it solves. The daemon's benefits (connection reuse, background work) serve problems that don't exist at nmem's scale.

**Verdict:** The daemon's benefits are real but premature. Start without one.

### Attacking Position C: "Hybrid is complexity without commitment"

The objection: if hooks write directly to SQLite, and maintenance runs separately, you have two processes that both touch the database. The maintenance daemon needs its own lifecycle management anyway. Why not just put everything in the daemon?

**Counter:** The critical difference is failure isolation. In Position B, a daemon crash stops all observation capture. In Position C, observation capture works even if the maintenance daemon never runs. The maintenance daemon is optional — it makes things cleaner but isn't required for correctness. This matches the VSM principle: S1 (operations/capture) must work independently of S3 (control/maintenance).

But: if the maintenance daemon is truly optional, why build it at launch? Hook invocations can run maintenance opportunistically — e.g., every 100th invocation runs `PRAGMA incremental_vacuum(50)`. No daemon needed.

**Verdict:** Position C is the right architecture but Position A with opportunistic maintenance is the right implementation.

## Decision

**Position A: no daemon. In-process hooks with opportunistic maintenance.**

Each hook invocation is a standalone process. Opens SQLite, writes the observation, optionally runs maintenance, closes. No persistent process, no IPC, no lifecycle management.

### Opportunistic Maintenance

Instead of a daemon for background work, hooks occasionally perform maintenance inline:

```rust
fn maybe_maintain(conn: &Connection) -> rusqlite::Result<()> {
    // Run maintenance roughly every 100 invocations
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM observations", [], |r| r.get(0)
    )?;
    if count % 100 == 0 {
        conn.execute_batch("PRAGMA incremental_vacuum(50)")?;
    }
    Ok(())
}
```

Maintenance tasks and their triggers:

| Task | Trigger | Cost |
|------|---------|------|
| Incremental vacuum | Every ~100 writes | ~1ms for 50 pages |
| WAL checkpoint | On session end (Stop hook) | ~1-10ms |
| FTS integrity check | On session start | ~1ms at <20K rows |
| FTS rebuild | Manual or on migration | Seconds at <20K rows |

### Session Lifecycle

nmem's session awareness comes from hook events, not from a persistent process:

- **SessionStart**: Record session start. Source field indicates startup/resume/compact.
- **PostToolUse**: Record observations. The bulk of capture.
- **Stop**: Record session end. Compute session signature (event type distribution). Run `PRAGMA wal_checkpoint(TRUNCATE)` to clean up WAL.

No explicit "nmem start" or "nmem stop" — the system activates when Claude Code fires hooks and quiesces when hooks stop firing.

### Process Model

```
Claude Code Session
  ├── SessionStart hook → nmem binary (open DB, record, close)
  ├── PostToolUse hook  → nmem binary (open DB, record, close)
  ├── PostToolUse hook  → nmem binary (open DB, record, close)
  ├── ...
  ├── Stop hook         → nmem binary (open DB, record, checkpoint, close)
  │
  └── MCP server        → nmem binary (open read-only DB, query, respond)
```

Each arrow is a separate process invocation. No shared state between invocations except the SQLite database file.

### Payload Deserialization

Hook payloads are heterogeneous JSON — `tool_input` shape depends on `tool_name`. The handler deserializes to a common struct with `tool_input` as `serde_json::Value`, then dispatches per tool:

```rust
#[derive(Deserialize)]
struct HookPayload {
    session_id: String,
    cwd: String,
    hook_event_name: String,
    tool_name: Option<String>,
    tool_input: Option<serde_json::Value>,
    tool_response: Option<String>,
}
```

Tool-specific extraction uses the sibling-field dispatch pattern (see `serde-json.md` § 3, "Sibling-Field Dispatch"): match on `tool_name`, then extract typed fields from the `Value` with `.get()` / `as_str()`. No need for full typed input structs at launch — add them if validation errors become a problem.

### MCP Server

The MCP server is the one component that needs session-scoped persistence — it serves queries throughout a session and benefits from a kept-alive read-only connection.

**Decision: session-scoped stdio subprocess.** Claude Code starts the MCP server as a long-running subprocess (stdio transport). It holds a read-only SQLite connection for the session duration. Dies when stdin closes (Claude Code exit). This is the standard MCP server model — not a daemon but a child process with a clear parent and automatic cleanup.

Implementation uses rmcp with stdio transport:

```rust
// nmem serve — MCP server entry point
let conn = Connection::open_with_flags(&db_path,
    OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
let server = NmemServer::new(conn);
server.serve(rmcp::transport::stdio()).await?.waiting().await?;
```

The server struct holds the read-only connection and exposes tools: `search` (FTS5 query → ranked results), `get_observations` (IDs → full details), `recent_context` (→ session start context). See `rmcp.md` for tool definitions, server state, and stdio transport lifecycle.

### Deduplication

Without persistent cross-event state, each hook invocation is independent. If a session reads the same file 10 times, the naive approach stores 10 `file_read` observations.

Strategy: **deduplicate at write time via SQL.** Before inserting, check if an observation with the same `session_id`, `obs_type`, and `file_path` (or `content` for non-file observations) already exists within a time window:

```rust
fn should_store(conn: &Connection, obs: &Observation) -> rusqlite::Result<bool> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM observations
         WHERE session_id = ?1 AND obs_type = ?2 AND file_path = ?3
         AND timestamp > ?4)",
        params![obs.session_id, obs.obs_type, obs.file_path,
                obs.timestamp - 300], // 5-minute window
        |r| r.get(0),
    )?;
    Ok(!exists)
}
```

This adds one SELECT per write (~0.5ms with index). The 5-minute dedup window is configurable. Writes and errors are never deduplicated — only reads and searches.

### Session Signatures

Each session produces a characteristic distribution of event types — its *signature*. A refactoring session is dominated by `file_edit` and `file_read`. A build session by `command`. A research session by `user_prompt` and `web_search`. The top-3 event types distinguish session character without reading any content.

Computed at session end in the Stop hook as a `COUNT(*) GROUP BY obs_type` query on the session's observations. Stored as a JSON field on the session summary record:

```rust
// In Stop hook, after recording session end
let signature: Vec<(String, i64)> = conn.prepare(
    "SELECT obs_type, COUNT(*) as n FROM observations
     WHERE session_id = ?1 GROUP BY obs_type ORDER BY n DESC"
)?.query_map(params![session_id], |r| {
    Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
})?.collect::<Result<_, _>>()?;
```

**Uses:**

- **Retrieval filtering.** Classify sessions as `research`, `build`, `refactor`, `debug`, `conversation` from the top event types. "What did I learn while refactoring?" filters to the right sessions without scanning everything.
- **Dedup tuning.** `file_read`-heavy sessions (refactoring) produce redundant observations — tighter dedup windows. `user_prompt`-heavy sessions (conversation) have unique content per event — looser or no dedup.
- **Context injection.** On SessionStart, find past sessions with similar signatures on the same project. A refactoring session benefits from past refactoring decisions, not past research context.

**Deferred:** Mid-session signature computation (for live context injection) adds complexity for an unproven use case. The per-event data is already stored — if needed later, signatures can be backcomputed without schema changes.

## Open Questions

### Q1: Should the binary be a single multi-command executable? — RESOLVED

Yes. A single `nmem` binary with clap subcommands:

```rust
#[derive(Parser)]
#[command(name = "nmem")]
struct Cli {
    /// Database path (default: ~/.nmem/nmem.db)
    #[arg(long, env = "NMEM_DB")]
    db: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Hook handler: read JSON from stdin, extract and store observation
    Record,
    /// MCP server: stdio transport, session-scoped
    Serve,
    /// Manual maintenance: vacuum, rebuild FTS, checkpoint WAL
    Maintain,
    /// Health check: DB size, observation count, last session
    Status,
    /// Delete observations with secure zeroing (ADR-005)
    Purge,
    /// Explicit database maintenance: vacuum, FTS integrity, WAL checkpoint
    Maintain,
}
```

Hook configuration points to `nmem record`, MCP config to `nmem serve`. Database path follows precedence: `--db` flag > `NMEM_DB` env var > `~/.nmem/nmem.db` default. See `clap.md` for derive API details.

### Q2: How to handle hook handler crashes? — RESOLVED

**Log to stderr and exit with code 1.** Individual observations are low-value — a lost observation is invisible to the user. Claude Code captures hook stderr for diagnostics.

Exit code semantics for hooks:
- `0` — success, observation stored
- `1` — non-blocking error (logged, session continues)
- `2` — blocking error (Claude Code shows error to user — reserved for critical failures like DB corruption)

```rust
fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("nmem: {e:#}");
            ExitCode::from(1)
        }
    }
}
```

No spool file. No retry. The cost of losing one observation is negligible; the cost of adding I/O and retry logic to every invocation is not. See `clap.md` § 5 for exit code patterns.

### Q3: Connection initialization cost — is it actually a problem?

Estimated per-invocation cost:
- Process spawn + binary startup: ~1-3ms (Rust static binary, no dynamic linking)
- Stdin JSON deserialization: <1ms (hook payloads are typically <10KB)
- `Connection::open()`: <1ms
- 5 PRAGMAs: <1ms (WAL mode is persistent, 4 per-connection PRAGMAs are fast)
- Migration version check: <1ms (reads `user_version`, compares to constant)
- Single INSERT: <1ms
- Sync to disk (WAL append): ~1ms with `synchronous = NORMAL`
- `Connection::close()`: <1ms

Total: ~3-8ms per hook invocation. PostToolUse hooks run after the tool response is shown — this overhead is invisible to the user.

Benchmark this at implementation time. If it's consistently >10ms, reconsider.

## When to Reconsider

Add a daemon when:
- S4 synthesis is implemented and needs periodic background processing
- Multiple consumers beyond Claude Code need coordinated access
- Write volume exceeds what per-invocation SQLite opens can handle (unlikely below thousands of writes/minute)
- Background maintenance can't keep up with opportunistic triggers

The architecture supports adding a daemon later — the database is the coordination point, not the process model. A daemon would hold a persistent connection and run the same write/maintenance logic that hooks currently run per-invocation.

## Consequences

### Positive

- **Zero lifecycle management.** No PID files, no orphan processes, no port conflicts, no startup scripts.
- **Failure isolation.** A crash in one hook invocation doesn't affect the next. No cascading failures.
- **Single binary deployment.** `nmem` binary + hook configuration. No service files, no init scripts.
- **Automatic cleanup.** When Claude Code exits, everything stops. No processes to hunt down.
- **Multiple sessions just work.** Each invocation is independent. SQLite WAL handles concurrent writes.

### Negative

- **Per-invocation overhead.** ~2-5ms per hook call for connection setup. Acceptable but non-zero.
- **No write batching.** Each observation is a separate transaction. At nmem's volume this is fine but prevents amortizing fsync costs across batches.
- **Deferred maintenance.** Without a daemon, maintenance only happens during sessions. Long idle periods accumulate WAL growth and fragmentation until the next session.
- **MCP connection churn.** If MCP queries don't use a session-scoped process, each query opens and closes a connection. Measurable if queries are frequent.

## References

- ADR-001 — Storage layer, concurrent access model, WAL checkpoint strategy
- ADR-002 — Extraction strategy (structured, no LLM) eliminates need for LLM subprocess
- `clap.md` — CLI binary patterns, subcommands, exit codes, fast startup
- `rmcp.md` — MCP server implementation, stdio transport, tool definitions
- `serde-json.md` § 3 — Sibling-field dispatch for heterogeneous hook payloads
- `claude-code-plugins.md` — Hook configuration and plugin setup
- `claude-code-hooks-events.md` — Event schemas for hook handlers
- `tokio-rusqlite.md` — Async connection patterns (relevant if daemon is added later)

## Revision History

| Date | Version | Changes |
|------|---------|---------|
| 2026-02-08 | 0.1 | Stub with framing and dependencies. |
| 2026-02-14 | 1.0 | Full ADR. Three positions, adversarial analysis. Decision: no daemon, in-process hooks with opportunistic maintenance. |
| 2026-02-14 | 1.1 | Refined. Fixed hook blocking claim. Added binary startup time to cost estimate. Decided MCP server model (session-scoped stdio). Added deduplication strategy. |
| 2026-02-14 | 1.2 | Resolved Q1 (clap subcommands) and Q2 (exit codes + stderr). Added payload deserialization strategy (sibling-field dispatch). Added rmcp server skeleton for MCP. Updated references. |
| 2026-02-14 | 1.3 | Added `Purge` subcommand to clap enum (cross-ref ADR-005). |
| 2026-02-14 | 1.4 | Added session signatures — event type distribution computed at session end for retrieval filtering, dedup tuning, and context injection. Derived from capture data analysis (684 events, 7 sessions). |
| 2026-02-14 | 1.5 | Added `Maintain` subcommand to clap enum. Implements explicit trigger for maintenance operations (incremental vacuum, WAL checkpoint, FTS integrity check, optional FTS rebuild via `--rebuild-fts`). Completes the "opportunistic maintenance" story — hooks run maintenance inline, `nmem maintain` provides the manual escape hatch. |
| 2026-02-14 | 1.6 | Added `Status` subcommand — read-only health check (DB/WAL size, observation/prompt/session counts, top-5 obs_type breakdown, last session). Completes all planned subcommands from Q1. |
