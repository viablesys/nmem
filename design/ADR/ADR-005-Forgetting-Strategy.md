# ADR-005: Forgetting Strategy

## Status
Accepted — Purge mechanism (v2.0) and retention sweeps (v3.0) implemented

## Framing
*What gets deleted, when, and how.*

Given the sqlite-vec DELETE finding, this ADR is forced to confront: can nmem forget at all with its current storage options? Should forgetting be deletion, archival, summarization, or decay? Adversarial angle: "What if nmem never forgets?" — is monotonic growth actually a problem at the expected data volume (hundreds of observations/month)?

## Depends On
ADR-002 (Extraction Strategy) — data volume determines whether forgetting is a day-one concern.

## Unlocks
ADR-003 (Daemon Lifecycle) — forgetting may require background processing.

---

## Context

ADR-002 established the data volumes: 300-1,500 observations/month, 3,600-18,000 rows/year. ADR-001 established the storage cost: 2-18 MB/year at expected volume (observations + FTS5 indexes). ADR-001 also set `auto_vacuum = INCREMENTAL`, which means DELETE reclaims space via periodic `PRAGMA incremental_vacuum(N)` calls — the mechanism works.

> **[ANNOTATION 2026-02-14, v1.3]:** Live production data shows significantly higher volumes than the original estimates: ~585K records/year, ~652 MB (see ADR-001 v3.3). The increase is driven by agent thinking blocks (2.9x per user prompt, 84% of content volume at avg 987 bytes). At 652 MB/year, the "never forget" position (A) remains viable for 1-2 years but storage crosses 1 GB within 2 years. Position C (type-aware retention) should be considered a year-1 activation rather than a distant contingency. Thinking blocks are prime candidates for aggressive retention — they're high-volume, low-reuse content that loses relevance quickly. A 30-day retention on agent prompts would cut ~60% of annual storage growth.

ADR-002 chose pure structured extraction over LLM synthesis. This means observations are typed rows with predictable columns (timestamp, session_id, project, obs_type, content, file_path, metadata). DELETE on these tables works normally. FTS5 external content tables sync deletions via triggers (ADR-001 defines the `observations_ad` trigger). sqlite-vec is deferred — its broken DELETE is not nmem's problem.

ADR-003 chose no daemon with opportunistic maintenance. There is no background process to run retention sweeps. Any forgetting must be triggered either by hook invocations, explicit CLI commands, or the user.

claude-mem had no forgetting mechanism. Observations accumulated indefinitely. At 9 observations total in early testing, this was not a problem. At nmem's projected volume over multiple years, the question is whether it *becomes* a problem — and if so, when.

### The predecessor had no forgetting — was that a problem?

No. claude-mem failed for other reasons (LLM hallucination, no validation, high cost). Storage growth was never the bottleneck. The database was small, queries were fast, and no one ever complained about too much data. The lesson: forgetting is a correctness concern (stale/untrue observations), not a storage concern (disk space).

## The Three Positions

### Position A: Never Forget

All observations are permanent. The database grows monotonically. No deletion, no archival, no decay. Retrieval handles relevance — old observations rank lower in recency-weighted queries but remain queryable.

**The case for it:**
- At 18 MB/year, nmem can run for a decade before reaching 200 MB. Modern disks have terabytes of free space.
- SQLite handles millions of rows without performance degradation. 180K rows after 10 years is nothing.
- FTS5 query time scales sub-linearly with row count. Even at 100K+ rows, queries return in single-digit milliseconds.
- Deletion is destructive. An observation that seems stale today might be the one relevant fact tomorrow ("what was that obscure flag I used 8 months ago?").
- Simplicity. No retention logic, no sweep triggers, no configuration, no edge cases.

**The case against it:**
- Observations can become *untrue*. A decision observation from 6 months ago ("we chose library X") may have been reversed. Stale decisions pollute retrieval.
- Noise accumulates. Thousands of `file_read` observations for files that no longer exist add nothing to retrieval quality.
- No principled way to correct mistakes. If a bad observation is stored (wrong extraction, garbled content), it persists forever.
- S4 synthesis (future) operates on the full corpus. A growing pile of stale data degrades synthesis quality over time.

### Position B: Time-Based Retention

Observations expire after a fixed age. A sweep deletes everything older than N days/months. Simple, predictable, automated.

**The case for it:**
- Simple mental model. "nmem remembers the last 6 months."
- Bounded growth. Database size has a ceiling proportional to the retention window.
- Consistent performance. Query corpus size is stable.

**The case against it:**
- Time is a terrible proxy for relevance. A 2-year-old decision observation ("we chose SQLite for storage") is more relevant than yesterday's `file_read`.
- All observation types treated equally. A `session_start` from January is no more or less valuable than a `command_error` from January — but the error might be the only record of a critical debugging insight.
- Cliff edge. An observation at day 179 is kept; at day 181 it's destroyed. No graceful degradation.

### Position C: Intelligent Decay (Type-Aware Retention)

Different observation types have different retention periods. High-value types (decisions, errors, user prompts) live longer. Low-value types (file reads, searches) decay faster. An explicit purge command handles corrections.

**The case for it:**
- Matches how memory actually works. Details fade but decisions persist.
- Keeps the corpus useful for retrieval by biasing toward high-signal observations.
- Configurable per-project or globally via S5 policy.

**The case against it:**
- Requires classifying observation types by value — a judgment call that may not generalize.
- More complex than "keep everything" or "delete after N days."
- Still doesn't handle factual staleness (a decision observation is kept for 2 years even if the decision was reversed after 2 weeks).

## Adversarial Analysis

### "What if nmem never forgets?" — stress-testing Position A

Run the numbers for 5 years of active use at the high end of expected volume (1,500 observations/month):

| Year | Rows | Estimated DB size (with FTS5) | FTS5 query time |
|------|------|-------------------------------|-----------------|
| 1 | 18,000 | ~18 MB | <5ms |
| 2 | 36,000 | ~36 MB | <5ms |
| 3 | 54,000 | ~54 MB | <10ms |
| 5 | 90,000 | ~90 MB | <10ms |

At 90 MB after 5 years of heavy use, storage is not a problem. Query performance is not a problem. SQLite handles this without noticing. The "never forget" position is not defeated by scale.

It *is* defeated by correctness. After 5 years, the database contains thousands of observations about files that no longer exist, projects that are archived, decisions that were reversed, and libraries that were replaced. Retrieval quality degrades not because the database is too large, but because the signal-to-noise ratio drops. An FTS5 query for "authentication" returns 200 results spanning 3 years of changing approaches, and the most recent is the only one that reflects current state.

**Verdict:** Position A is viable for storage and performance. It fails on retrieval quality over time. But the failure is gradual, not catastrophic — and it's a year-2+ concern, not day-one.

### "Type-aware retention is premature complexity" — stress-testing Position C

The objection: we don't know which observation types are high-value vs low-value until nmem has real usage data. Assigning retention periods now is guessing. And the complexity of per-type retention logic, configuration, sweep triggers, and edge cases adds implementation cost with uncertain payoff.

**Counter:** The type classification is not arbitrary. It follows from the observation semantics defined in ADR-002:

| obs_type | Retention value | Rationale |
|----------|----------------|-----------|
| `user_prompt` | High | User intent, the "why" behind a session |
| `command_error` | High | Debugging insights, error patterns |
| `file_write`, `file_edit` | Medium | What changed, but context degrades as files evolve |
| `session_start`, `session_end` | Medium | Session structure, temporal anchors |
| `file_read` | Low | Noise at volume — reading a file once is rarely memorable |
| `search` | Low | Ephemeral queries, rarely useful after the session |
| `mcp_call` | Low | Tool usage metadata, mostly noise |

This isn't guessing — it's a reasonable default based on the semantics. And "configurable" means users can override if the defaults are wrong.

**Verdict:** The classification is defensible but the *implementation* is premature. Define the policy now, implement it when needed.

## Decision

**Position A for launch (never forget), with Position C's policy designed and ready to activate.**

Rationale:
1. At nmem's data volume, forgetting is a year-2+ concern, not day-one. Spending implementation effort on retention logic before the database reaches 20K rows is premature.
2. The mechanism is trivial — DELETE works on structured tables, FTS5 sync triggers handle index cleanup, incremental vacuum reclaims space. There is no technical obstacle to forgetting.
3. The *policy* deserves early design even if implementation is deferred. When retrieval quality degrades, the retention rules should be ready — not designed under pressure.
4. An explicit `nmem purge` command provides immediate escape valve for corrections and manual cleanup.

## Retention Policy (Deferred, Designed)

When activated, retention follows type-aware decay with configurable thresholds:

```toml
# ~/.nmem/config.toml (future)
[retention]
enabled = false  # flip to true when needed

[retention.days]
user_prompt = 730     # 2 years
command_error = 730   # 2 years
file_write = 365      # 1 year
file_edit = 365       # 1 year
session_start = 365   # 1 year
session_end = 365     # 1 year
file_read = 90        # 3 months
search = 90           # 3 months
mcp_call = 90         # 3 months
command = 180         # 6 months
```

Default retention periods are conservative — err toward keeping too much rather than too little. Users can tighten per-project if needed.

**Protected observations:** Any observation referenced by a future `syntheses` row (ADR-002, Q3) is exempt from retention sweeps. Deleting source observations while keeping the synthesis that references them creates orphaned summaries. The sweep query excludes observations whose IDs appear in any `source_obs_ids` array.

## Purging Mechanism

### Retention sweep (when enabled)

```sql
-- Delete expired observations by type
DELETE FROM observations
WHERE obs_type = ?1
  AND timestamp < unixepoch() - (?2 * 86400)
  AND id NOT IN (
    SELECT value FROM syntheses, json_each(syntheses.source_obs_ids)
  );
```

**Note:** The `syntheses` table does not exist at launch — it's defined in ADR-002 Q3 as a future addition for S4 synthesis. The sweep implementation must check for the table's existence before including the `NOT IN` clause. If the table doesn't exist, skip the protection subquery entirely — there are no syntheses to protect.

```rust
fn syntheses_table_exists(conn: &Connection) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='syntheses')",
        [], |r| r.get(0),
    )
}
```

FTS5 cleanup happens automatically via the `observations_ad` trigger defined in ADR-001. No separate FTS5 maintenance step is needed for deletions.

After the sweep, run `PRAGMA incremental_vacuum(N)` to reclaim freed pages. This aligns with ADR-003's opportunistic maintenance model — the sweep itself is the trigger for vacuum.

**FTS5 after large deletions:** The `observations_ad` trigger handles individual deletes correctly. However, after a large purge (e.g., `--project` deleting thousands of rows), the FTS5 index may benefit from a rebuild to reclaim space and optimize the b-tree structure. Run `INSERT INTO observations_fts(observations_fts) VALUES('rebuild')` after any purge that deletes more than ~1000 rows. At <20K total rows this completes in under a second.

### Explicit purge (day-one)

Available at launch regardless of retention policy. Adds a `Purge` subcommand to ADR-003's clap enum (alongside `Record`, `Serve`, `Maintain`, `Status`). See `clap.md` § 2 for derive patterns:

```rust
/// Delete observations with secure zeroing.
#[derive(clap::Args)]
struct PurgeArgs {
    /// Delete everything before this date (YYYY-MM-DD)
    #[arg(long, value_name = "DATE")]
    before: Option<String>,
    /// Delete all observations for a project
    #[arg(long, value_name = "NAME")]
    project: Option<String>,
    /// Delete all observations from a session
    #[arg(long, value_name = "UUID")]
    session: Option<String>,
    /// Delete a single observation by ID
    #[arg(long, value_name = "ID")]
    id: Option<i64>,
    /// Filter by observation type (with --older-than)
    #[arg(long, value_name = "TYPE")]
    r#type: Option<String>,
    /// Delete observations older than N days (with --type)
    #[arg(long, value_name = "DAYS", requires = "type")]
    older_than: Option<u32>,
    /// Search for matching observations to purge
    #[arg(long, value_name = "QUERY")]
    search: Option<String>,
    /// Skip confirmation prompt
    #[arg(long)]
    confirm: bool,
}
```

Usage:
```
nmem purge --before 2025-01-01       # delete everything before a date
nmem purge --project old-app         # delete all observations for a project
nmem purge --session <uuid>          # delete a specific session
nmem purge --id 42                   # delete a single observation
nmem purge --type file_read --older-than 90  # type + age
nmem purge --search "sk-"            # find and purge matching observations
```

All purge operations:
1. Report the count of rows to be deleted before executing
2. Require `--confirm` flag (or interactive confirmation) to proceed
3. Run `PRAGMA incremental_vacuum` after deletion
4. Log the purge action (count, criteria, timestamp) to stderr

This provides an immediate escape valve without enabling automated retention. A user who stores a secret by mistake can purge it. A user who archives a project can clean up its observations.

### Sweep trigger

When retention is enabled, the sweep runs opportunistically per ADR-003's model: on session start (SessionStart hook) or via `nmem maintain`. No daemon, no cron job. If no session starts for a week, expired observations persist an extra week — acceptable.

## Secure Deletion

Standard DELETE removes row data but does not zero the underlying database pages. Deleted content may be recoverable from:
1. Freed pages in the database file (until overwritten by new data or vacuumed)
2. WAL frames (until checkpoint folds them into the main file)
3. OS-level filesystem journal or disk sectors

For observations that contain secrets (despite ADR-007's filtering), standard DELETE is insufficient.

**Connection to ADR-007:** The trust boundary ADR will define *what* must never be stored and *how* filtering prevents it. This ADR addresses the fallback: what happens when filtering fails and a secret is stored.

**Mechanism:** `PRAGMA secure_delete = ON` causes SQLite to overwrite deleted content with zeros. This adds a measurable cost to DELETE operations (roughly 2x for page-level zeroing). For nmem's low deletion volume (manual purges and periodic sweeps), this cost is acceptable.

**Recommendation:** Enable `secure_delete` only for explicit purge operations, not for retention sweeps. Retention sweeps delete old, low-value observations where secure deletion adds cost without benefit. Purge operations target specific observations (possibly containing secrets) where zeroing is worth the cost.

```rust
/// Secure purge: zero-fill deleted pages, then reclaim space.
/// See rusqlite.md § 3 for execute_batch and PRAGMA patterns.
fn secure_purge(conn: &Connection, criteria: &PurgeCriteria) -> rusqlite::Result<usize> {
    conn.execute_batch("PRAGMA secure_delete = ON")?;
    let count = execute_purge(conn, criteria)?;
    conn.execute_batch("PRAGMA secure_delete = OFF")?;
    // Reclaim freed pages (see rusqlite.md § 2, incremental vacuum)
    conn.execute_batch("PRAGMA incremental_vacuum")?;
    // Rebuild FTS5 after large deletions (see fts5.md § 5, maintenance)
    if count > 1000 {
        conn.execute_batch(
            "INSERT INTO observations_fts(observations_fts) VALUES('rebuild')"
        )?;
    }
    Ok(count)
}
```

WAL frame persistence is a separate concern. After a secure purge, run `PRAGMA wal_checkpoint(TRUNCATE)` to fold WAL into the main file and delete the WAL, eliminating any copies of the secret in WAL frames. ADR-007 will determine whether this is sufficient or whether encryption at rest is the load-bearing defense.

## Implementation Notes (2026-02-14)

> **[ANNOTATION 2026-02-14, v2.0]:** `nmem purge` shipped. Implementation in `src/purge.rs` (~250 lines), wired through `src/cli.rs` (PurgeArgs), `src/main.rs` (dispatch), `src/lib.rs` (module). Key implementation details:

### What shipped vs. spec

| Spec item | Status | Notes |
|-----------|--------|-------|
| `--before DATE` | ✅ | YYYY-MM-DD parsed to UTC epoch via pure arithmetic (no chrono dep) |
| `--project NAME` | ✅ | Cascades: observations + prompts + _cursor + sessions for project |
| `--session UUID` | ✅ | Cascades: observations + prompts + _cursor + session row |
| `--id N` | ✅ | Single observation delete, no cascade |
| `--type TYPE --older-than DAYS` | ✅ | `requires = "obs_type"` in clap. Cutoff = now+1 - days*86400 |
| `--search QUERY` | ✅ | FTS5 MATCH on observations_fts |
| `--confirm` flag | ✅ | Without it: prints count, exits cleanly. No interactive prompt. |
| Secure delete | ✅ | `PRAGMA secure_delete = ON` for all purge operations |
| Incremental vacuum | ✅ | `PRAGMA incremental_vacuum` after deletion |
| FTS5 rebuild | ✅ | Rebuild if >1000 observations deleted |
| WAL checkpoint | ✅ | `PRAGMA wal_checkpoint(TRUNCATE)` post-purge |
| Retention sweeps | ✅ | `nmem maintain --sweep` + opportunistic on SessionStart |
| Syntheses protection | ✅ | Guard checks `sqlite_master` — skips subquery when table absent |

### Deviations from spec

1. **No `--dry-run` flag.** The default behavior (without `--confirm`) serves as dry-run — it reports counts and exits. Q2 in Open Questions is effectively answered: count-only, no sample output.
2. **Date parsing is arithmetic, not `chrono`.** Avoids adding a dependency for a single date-to-timestamp conversion. The calculation handles leap years correctly.
3. **`days_ago_ts` adds +1 second.** Without this, `--older-than 0` would miss records written in the same second due to `timestamp < now` being false. The +1 ensures `--older-than 0` means "all records regardless of age."
4. **Orphan cleanup strategy.** For `--session` and `--project`: explicit cascade delete of all related rows. For other modes (`--id`, `--type`, `--search`, `--before`): delete matching observations, then clean up sessions that have no remaining observations or prompts.

### Test coverage

9 integration tests via assert_cmd (CLI-level, tempfile DBs):
- `purge_by_id` — single observation, others remain
- `purge_by_session` — full cascade (obs + prompts + session)
- `purge_by_project` — multi-session cascade by project name
- `purge_by_search` — FTS match deletion + FTS sync verification
- `purge_by_type_and_age` — type filter with --older-than
- `purge_requires_confirm` — dry-run behavior
- `purge_no_match` — clean exit on zero matches
- `purge_no_filter_fails` — error without any filter flag
- `purge_fts_sync` — FTS index consistency after purge

2 unit tests for date parsing (`parse_date_known_epoch`, `parse_date_invalid`).

## Retention Sweep Implementation Notes (2026-02-14)

> **[ANNOTATION 2026-02-14, v3.0]:** Automated retention sweeps shipped. Position C (type-aware retention) activated as designed. Implementation in `src/sweep.rs` (~80 lines), wired through `src/cli.rs` (`--sweep` flag), `src/maintain.rs`, and `src/record.rs` (opportunistic trigger).

### Architecture

**Config** (`src/config.rs`): `RetentionConfig` struct with `enabled: bool` and `days: HashMap<String, u32>`. Defaults match the policy table from this ADR. When `enabled = false` (default), sweeps are no-ops. Types not present in the `days` map are never swept — safe default for unknown/future types.

**Sweep logic** (`src/sweep.rs`): `run_sweep(conn, config)` iterates over configured `(obs_type, days)` pairs, builds per-type DELETE with cutoff timestamp. Syntheses guard checks `sqlite_master` for table existence before adding the `NOT IN` subquery. Returns `SweepResult { deleted, by_type, orphans_cleaned }`.

**Two entry points:**
1. `nmem maintain --sweep` — explicit CLI invocation, reports per-type counts
2. Opportunistic on `SessionStart` in `src/record.rs` — runs only when `enabled = true` AND DB has 100+ observations older than 1 day. Non-fatal: sweep errors are logged to stderr but don't block the record operation.

### Reuse from purge

`cleanup_orphans` and `post_purge_maintenance` from `src/purge.rs` made `pub` and reused by sweep. Same orphan cleanup (sessions with no observations or prompts) and same post-deletion maintenance (incremental vacuum, FTS5 rebuild if >1000 deleted, WAL checkpoint).

**No `secure_delete` for sweeps** — as specified in this ADR. Secure deletion is reserved for manual purges of secrets, not routine retention.

### Test coverage

4 unit tests in `src/sweep.rs`:
- `sweep_disabled_is_noop` — config with `enabled: false`, 0 deleted
- `sweep_deletes_expired` — only observations past retention window deleted
- `sweep_preserves_unexpired` — observations within window survive
- `sweep_unknown_type_preserved` — obs_type not in config.days never swept

3 integration tests in `tests/integration.rs`:
- `maintain_sweep_flag` — CLI `--sweep` with retention config, verify correct type-selective deletion
- `sweep_disabled_by_default` — no config file, no deletions
- `sweep_on_session_start` — opportunistic sweep fires when 100+ old observations exist

## Open Questions

### Q1: Should retention protect "landmark" observations regardless of type?

Some observations are landmarks — first session in a new project, first error in a debugging arc, the observation that resolved a long-running issue. These have disproportionate retrieval value but no structural marker distinguishing them from ordinary observations of the same type. Should there be an `is_pinned` flag that exempts observations from retention sweeps? If so, who sets it — the user explicitly, or nmem's S4 when it identifies patterns?

### Q2: Does the purge command need dry-run output?

The `--confirm` flag prevents accidental deletion, but a `--dry-run` flag that shows *which* observations would be deleted (not just the count) would let users verify before committing. At high observation counts this output could be large. Is count-only sufficient, or should dry-run show a sample (first N observations)?

## When to Reconsider

Enable automated retention when:
- Retrieval quality visibly degrades due to stale results (user reports, or S4 measures signal-to-noise)
- Database size exceeds 100 MB (roughly 5+ years of heavy use)
- S4 synthesis is implemented and struggles with the volume of source observations
- A new observation type is added with high volume and low retention value

Do **not** enable retention preemptively. The cost of keeping too much data is low (extra disk space, slightly noisier retrieval). The cost of deleting too aggressively is high (lost context that can't be recovered). Err toward hoarding.

## Consequences

### Positive

- **No data loss at launch.** Every observation is permanent until explicitly purged. Users cannot accidentally lose context through misconfigured retention.
- **Purge command available day-one.** Manual cleanup and secret removal work immediately, no retention policy required.
- **Policy is designed before it's needed.** When retrieval quality degrades, the retention configuration and sweep logic are ready to activate — not designed under pressure.
- **Mechanism is trivial.** DELETE + FTS5 triggers + incremental vacuum. No complex compaction, no archival tier, no two-phase deletion. The hard problem is policy, and the mechanism just works.

### Negative

- **Monotonic growth until retention is enabled.** The database grows indefinitely at launch. At expected volumes this is harmless for years, but it's unbounded in principle.
- **Stale observations degrade retrieval over time.** Without active forgetting, old observations about deleted files, reversed decisions, and archived projects accumulate. Retrieval results include noise that the user must mentally filter.
- **No automatic correction.** If an observation is wrong (bad extraction, garbled content), it persists until the user notices and explicitly purges it. There's no self-healing for data quality.
- **Secure deletion adds complexity.** The `secure_delete` PRAGMA, WAL checkpoint coordination, and the distinction between regular and secure purge create edge cases that must be tested carefully.

## References

- ADR-001 -- Storage layer: `auto_vacuum = INCREMENTAL`, FTS5 sync triggers, WAL checkpoint strategy
- ADR-002 -- Extraction strategy: observation schema, data volumes (300-1,500/month), obs_type taxonomy
- ADR-003 -- Daemon lifecycle: opportunistic maintenance model, no background sweep process, `Purge` subcommand
- ADR-007 -- Trust boundary: secrets filtering, secure_delete for purge, WAL checkpoint after purge
- `rusqlite.md` -- Connection, `execute_batch` for PRAGMAs, parameterized queries for sweep SQL
- `fts5.md` -- FTS5 rebuild after large deletions, external content trigger behavior
- `clap.md` -- Derive API for `PurgeArgs`, `--confirm` flag, `requires` attribute for dependent args
- [SQLite PRAGMA secure_delete](https://www.sqlite.org/pragma.html#pragma_secure_delete)
- [SQLite PRAGMA incremental_vacuum](https://www.sqlite.org/pragma.html#pragma_incremental_vacuum)

## Revision History

| Date | Version | Changes |
|------|---------|---------|
| 2026-02-08 | 0.1 | Stub with framing and dependencies. |
| 2026-02-14 | 1.0 | Full ADR. Three positions, adversarial analysis. Decision: never forget at launch, type-aware retention designed and ready. Purge command day-one. Secure deletion strategy. |
| 2026-02-14 | 1.1 | Refined. Syntheses table existence guard for retention sweep. Purge subcommand added to ADR-003 clap enum. FTS5 rebuild note after large deletions. |
| 2026-02-14 | 1.2 | Refined with library topics. PurgeArgs clap derive struct. FTS5 rebuild integrated into secure_purge. References: rusqlite.md, fts5.md, clap.md. |
| 2026-02-14 | 1.3 | Annotated with live production data. Volume estimates revised to ~585K records/year (~652 MB). Thinking blocks identified as prime retention candidates (84% of content, low reuse). Position C activation timeline shortened to year-1. |
| 2026-02-14 | 2.0 | **Implemented.** `nmem purge` subcommand shipped in `src/purge.rs`. All 7 filter flags from spec implemented (`--before`, `--project`, `--session`, `--id`, `--type`/`--older-than`, `--search`). Confirmation via `--confirm` flag (no interactive stdin). Secure deletion: `PRAGMA secure_delete = ON`, `incremental_vacuum`, FTS5 rebuild >1000 rows, WAL checkpoint. FK-safe deletion order: observations → prompts → _cursor → sessions. Orphan cleanup for non-session/project modes. 9 integration tests, all passing. |
| 2026-02-14 | 3.0 | **Retention sweeps implemented.** Position C (type-aware retention) activated. `src/sweep.rs` new module. Two entry points: `nmem maintain --sweep` (explicit) and opportunistic trigger on SessionStart (threshold: 100+ old observations). `RetentionConfig` in `src/config.rs` with default days from ADR policy table. Syntheses guard via `sqlite_master` check. `cleanup_orphans`/`post_purge_maintenance` made pub for reuse. 4 unit + 3 integration tests. |
