# ADR-004: Project Scoping and Isolation

## Status
Accepted

## Framing
*Single database vs per-project databases.*

This is the recursion question from VSM. Forces decisions about: secret leakage between projects, cross-project queries, cold start per project, storage budgets, and schema migration complexity. Adversarial framing: "What if every project is fully isolated?" vs "What if there's only one global database?" — push both extremes and see where they break.

## Depends On
ADR-002 (Extraction Strategy) — what's stored determines what crosses project boundaries.

## Unlocks
ADR-007 (Trust Boundary) — isolation model affects blast radius of leaked secrets.

---

## Context

claude-mem had an implicit project model: the `cwd` from hook payloads was stored but never enforced as a boundary. All observations went into one SQLite + one Chroma instance. Cross-project leakage was invisible — an observation about project A's auth tokens could surface in project B's context injection. Nobody noticed because the system was single-developer, single-machine, and the MCP search tools didn't filter by project unless explicitly asked.

This worked by accident. It breaks under two conditions:
1. **Secret leakage** — project A has API keys in its observations; project B's session start context inadvertently includes them.
2. **Noise accumulation** — 10 projects with 500 observations each means 5000 rows. Without project scoping, "recent context" for project A includes irrelevant observations from projects B through J.

ADR-002 established that observations have a `project TEXT NOT NULL` column. ADR-003 established no daemon — each hook invocation opens the database, writes, and closes. The question here is whether that column is sufficient isolation or whether stronger boundaries are needed.

## The Three Positions

### Position A: Single Database, Row-Level Scoping

One `~/.nmem/nmem.db`. The `project` column on every observation row is the only isolation mechanism. Queries filter by `WHERE project = ?`. Cross-project queries omit the filter.

**How it works:**
- Every observation carries a `project` field (derived from `cwd`)
- Default queries scope to the current project
- Cross-project queries are explicit (`WHERE project IN (...)` or no project filter)
- `idx_obs_project` index (ADR-002) makes project-filtered queries fast

**What works:**
- Simplest possible model. One database, one schema, one migration path.
- Cross-project queries are trivial — just SQL. "Show all Rust errors across projects" is a WHERE clause.
- Cold start on a new machine: copy one file.
- Storage budget is one number, one vacuum, one checkpoint.
- Schema migrations run once.

**What's lost:**
- No hard isolation. A bug in query construction, a missing WHERE clause, or a future feature that forgets to scope — any of these leak cross-project data.
- Secret blast radius is the entire database. A leaked API key in project A's observation is physically present in the same file as project B's data.
- No per-project lifecycle. Can't delete project A's data without touching the shared database (DELETE + vacuum, not DROP DATABASE).
- Storage growth is shared. A noisy project inflates the database for all projects.

**Failure mode:** Silent cross-project leakage. The data is correct but the access boundary is enforced by convention (query discipline), not structure (file separation).

### Position B: Per-Project Databases

Each project gets its own SQLite file: `~/.nmem/projects/<project-id>/nmem.db`. No shared state.

**How it works:**
- Project identity determines database path
- Each hook invocation resolves `cwd` to a project ID, opens that project's database
- Cross-project queries use `ATTACH DATABASE` to join across files
- Schema migrations run per database on open (already the pattern from ADR-003)

**What works:**
- Hard isolation by default. A query on project A's database cannot access project B's data — it's a different file.
- Secret blast radius is one project. Leaked key in project A's DB doesn't touch project B.
- Per-project lifecycle: delete the directory, project is gone. No vacuum needed.
- Per-project storage budgets: trivially measurable per file.
- Portable subsets: copy one project's DB to a new machine without carrying all projects.

**What's lost:**
- Cross-project queries are expensive. ATTACH has a limit of 10 databases by default (`SQLITE_MAX_ATTACHED = 10`). Querying across 15 projects requires multiple rounds.
- Schema migrations multiply. 20 projects = 20 migrations on version bump. If a migration fails on project 7, the system is in a partially-migrated state.
- FTS5 indices per database. Each project maintains its own FTS index. No unified full-text search without ATTACH.
- Cold start per project: a new project has zero context, even if the user has worked in Rust across 10 other projects.
- More files to manage. `~/.nmem/` goes from 1 file to N directories with 3 files each (db, wal, shm during operation).
- Connection overhead multiplied if MCP needs to serve queries across projects.

**Failure mode:** The isolation that makes it safe also makes it stupid. "What Rust patterns have I used?" requires opening and querying every project database. The system remembers per-project but can't generalize.

### Position C: Single Database with Visibility Tiers

One database, but observations are tagged with a visibility tier that controls cross-project access.

**How it works:**
- Every observation has a `project` field (scoping) and a `visibility` field (access control)
- Visibility tiers: `local` (project-only), `global` (all projects), `restricted` (explicit project list)
- Default visibility is `local`
- Certain observation types default to `global` (language preferences, tool patterns)
- Cross-project queries see only `global` observations from other projects

**What works:**
- Granular control. "prefer reqwest over ureq" is global. "auth module is in src/auth/" is local.
- Single database simplicity with logical isolation.
- Cross-project pattern recognition without full leakage.
- Secrets filtering (ADR-007) can enforce `local` visibility for sensitive-looking content.

**What's lost:**
- Classification complexity. Who decides visibility? Structured extraction (ADR-002) parses tool calls — it doesn't understand whether "use HMAC-SHA256" is a global Rust insight or a project-specific secret.
- Visibility is a policy decision, not a structural one. Misclassification silently leaks or silently hides.
- Adds a column, an index, and query complexity to every retrieval path.
- At nmem's current design (no LLM at extraction), there's no intelligence to classify visibility correctly. Rules would be crude: "errors are local, preferences are global" — but how does a structured extractor know what a "preference" is?

**Failure mode:** Overclassification (everything is local, cross-project is useless) or underclassification (everything is global, same as Position A but with more complexity).

## Adversarial Analysis

### Attacking Position A: "Row-level scoping is a ticking secret leak"

The strongest objection: a single missing `WHERE project = ?` in any query path exposes all projects' data to any session. This is especially dangerous for SessionStart context injection (ADR-006), where observations are pushed to the LLM — a cross-project observation containing an API key from another project would be injected into a session where it doesn't belong.

**Counter:** The blast radius argument is real but the threat model is specific. nmem is single-user, single-machine. The "attacker" is a bug in nmem's own query code, not an external adversary. Defense:
1. All query functions take `project` as a required parameter — no "unscoped" query API exists.
2. SessionStart context injection always filters by project. The function signature enforces it.
3. Cross-project queries are a separate, explicit code path — not the default.
4. ADR-007's secret filtering runs before storage, not at query time. If filtering works, there are no secrets in the database to leak.

The risk is a defense-in-depth failure: filtering misses a secret AND a query leaks cross-project. Two independent failures. Possible but not probable for a single-developer tool.

**Verdict:** The risk is real but manageable with query API design. Position B's file-level isolation is stronger but costs cross-project intelligence.

### Attacking Position B: "Per-project databases throw away the best feature"

The strongest objection: cross-project learning is the entire point of a persistent memory system. "I solved this locking issue in project X — it applies here in project Y." Per-project databases make this discovery require opening N databases, running N queries, and merging results. In practice, nobody writes that query — the cross-project insight is lost.

**Counter:** ATTACH DATABASE exists. A cross-project search could open the top-5 most recent project databases and run a unified FTS5 query. But:
1. ATTACH limit is 10 by default. Recompiling SQLite with a higher limit means maintaining a fork or using compile-time options (the `bundled` feature supports this, but it's an additional configuration surface).
2. FTS5 across attached databases requires querying each FTS table separately — no unified full-text index.
3. The MCP server (ADR-003) would need to know about all project databases, not just the current one. Session-scoped stdio process now needs a directory scan on startup.

**Verdict:** Per-project databases sacrifice the feature that justifies having a persistent memory system in the first place.

### Attacking Position C: "Visibility tiers without LLM classification are guesswork"

The strongest objection: ADR-002 decided on pure structured extraction. There is no LLM to classify whether an observation is project-local or globally relevant. Rules like "file writes are local, command patterns are global" are crude heuristics that will misclassify regularly. The visibility tier adds schema complexity for a classification system that can't work well without S4 intelligence.

**Counter:** Correct. Position C is premature — it solves a real problem (what crosses boundaries?) with a mechanism that requires intelligence nmem doesn't have yet. When S4 synthesis is added (ADR-002's "when earned" criterion), visibility tiers become feasible. Adding the column now is cheap, but building the classification logic is not.

**Verdict:** Defer visibility tiers until S4 exists to populate them.

## Decision

**Position A: single database, row-level project scoping.**

Rationale:
1. Simplest model that supports both project isolation and cross-project queries
2. Cross-project pattern recognition is a core value proposition — Position B destroys it
3. ADR-002's structured extraction produces typed, filterable observations — project scoping is a WHERE clause
4. Secret leakage risk is mitigated by ADR-007 filtering (before storage) and query API design (required project parameter)
5. Cold start on a new machine is copying one file
6. Position C's visibility tiers require classification intelligence that doesn't exist yet — can be layered on later when S4 is added

The database remains `~/.nmem/nmem.db` as established in ADR-001. The `project` column and `idx_obs_project` index from ADR-002 are the isolation mechanism.

## Project Identity

How `cwd` becomes a `project` value. This is the normalization question — the same project can be opened from different paths (symlinks, home dir variations), and the identity must be stable.

### Resolution strategy (ordered by precedence):

1. **Explicit config**: A `.nmem.toml` in the project root. Checked first. Allows user override and handles edge cases (monorepos, unusual directory structures).

   ```toml
   # .nmem.toml — minimal schema
   project = "my-project"  # Required. Overrides git/cwd detection.
   ```

   Only the `project` field is defined at launch. Future fields (retention overrides, sensitivity level) can be added without schema versioning — unknown keys are ignored by the TOML parser.

2. **Git root detection**: Run `git rev-parse --show-toplevel`. If the cwd is inside a git repo, the repo root becomes the project identity. Stable across subdirectory navigation within the same repo. Normalizes symlinks (git resolves to real path).

3. **Canonical cwd**: `std::fs::canonicalize(cwd)` — resolves symlinks, `..`, and relative components. Last resort when there's no git repo and no explicit config.

### The project identifier

The stored value is the **directory name** (last component of the resolved path), not the full path. Reasoning:
- Portable across machines. `/home/alice/dev/nmem` and `/Users/alice/dev/nmem` both produce `nmem`.
- Stable across reorganization. Moving a project from `~/dev/` to `~/projects/` doesn't change its identity.
- Collision risk: two projects named `app` on different paths collide. Mitigated by explicit config override.

For git repos, use the directory name of `git rev-parse --show-toplevel`. For non-git directories, use the directory name of the canonicalized cwd.

```rust
/// Read project name from .nmem.toml if it exists.
/// Uses serde for TOML deserialization (see serde-json.md § 1 for derive patterns).
fn read_nmem_config(cwd: &Path) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct NmemConfig {
        project: String,
    }
    let config_path = cwd.join(".nmem.toml");
    let contents = std::fs::read_to_string(&config_path).ok()?;
    let config: NmemConfig = toml::from_str(&contents).ok()?;
    Some(config.project)
}

fn resolve_project(cwd: &Path) -> String {
    // 1. Check for explicit config
    if let Some(name) = read_nmem_config(cwd) {
        return name;
    }
    // 2. Try git root
    if let Ok(output) = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(cwd)
        .output()
    {
        if output.status.success() {
            let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if let Some(name) = Path::new(&root).file_name() {
                return name.to_string_lossy().to_string();
            }
        }
    }
    // 3. Canonical cwd directory name
    let canonical = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    canonical.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}
```

**Performance note:** `git rev-parse` spawns a subprocess (~5-10ms). For `nmem record` (one-shot per hook invocation), this cost is acceptable — it runs once per invocation. For `nmem serve` (long-running MCP server), the project should be resolved once at startup and cached for the session. The MCP server receives `cwd` implicitly — it's the working directory of the stdio subprocess spawned by Claude Code.

### Edge cases

| Scenario | Resolution |
|----------|-----------|
| Monorepo with multiple logical projects | Explicit config per subdirectory, or accept repo-level grouping |
| Home directory (`~/`) | Project name is the home dir name (e.g., `bpd`). Unusual but not wrong. |
| Temp directories (`/tmp/...`) | Canonicalized dir name. Observations are ephemeral — matches the directory. |
| Same project name, different locations | Collision. Use explicit config to disambiguate. |
| Symlink farm (e.g., `~/dev/current -> ~/dev/nmem`) | Git root or canonicalize resolves through symlinks. Stable. |

## Cross-Project Observations

What's project-local vs what should cross boundaries.

### MCP Server Project Awareness

`nmem serve` is a session-scoped stdio subprocess (ADR-003). It needs to know the current project to default the `project` parameter in `search` and `recent_context` (ADR-006). Resolution: `nmem serve` resolves the project from its own working directory at startup — Claude Code spawns the MCP server with `cwd` set to the project root. The resolved project name is cached in the server struct for the session lifetime. If the user switches directories mid-session, the MCP server's project scope does not change — it reflects the project at session start. This matches the SessionStart hook's behavior: context is scoped to the project where the session began.

```rust
struct NmemServer {
    conn: Connection,
    project: String,  // resolved once at startup
}
```

### Current model (Position A, no visibility tiers)

All observations are stored with a `project` field. Query-time decisions:

- **Default (session start, MCP search):** `WHERE project = ?current_project`. Only current project's observations. In ADR-006's MCP tools, omitting the `project` parameter defaults to the current project (resolved at server startup, see above).
- **Explicit cross-project:** Consumer passes `project: null` to MCP tools to query across all projects. Or `project: "other-name"` for a specific foreign project.
- **No automatic cross-pollination.** nmem does not proactively surface cross-project observations at session start. The consumer (Claude Code, or whatever invokes MCP tools) can issue cross-project queries if it wants to.

**Consistency rule:** All query interfaces (MCP tools, context injection, future CLI) follow the same defaulting: absent `project` = current project, explicit `null` = all projects. ADR-006's `search`, `get_observations`, `timeline`, and `recent_context` all follow this convention.

### What naturally crosses boundaries

When cross-project queries *are* issued, the observation types that are useful across projects differ from those that aren't:

| Cross-project useful | Project-local |
|---------------------|---------------|
| Recurring error patterns | File paths and directory structure |
| Tool preferences (flags, options) | Project-specific config values |
| Language patterns and idioms | API keys, tokens, credentials |
| Build/deploy patterns | Local environment setup |
| Library choices and rationale | Branch names, commit hashes |

This classification doesn't need to be encoded in the schema now. It's a retrieval concern — when S4 synthesis is added, it can use observation type and content heuristics to weight cross-project relevance. For now, cross-project queries return everything and the consuming LLM filters by relevance in context.

### Future: S4-driven cross-pollination

When S4 synthesis (ADR-002, "when earned") is implemented, it could:
1. Analyze observations across projects for recurring patterns
2. Produce `syntheses` rows (ADR-002 schema) scoped to `project = NULL` (global)
3. Session start injection includes global syntheses alongside project-local observations
4. Visibility tiers (Position C) become viable at this point — S4 classifies, schema stores

This is not designed now. The schema supports it (NULL-able `project` on `syntheses` table). The decision is deferred.

## Path Storage

Observations frequently reference file paths (from Read, Write, Edit, Grep, Glob tool calls). How these are stored affects portability.

### Decision: project-relative paths

Store file paths relative to the project root, not absolute.

```rust
fn normalize_path(file_path: &str, project_root: &Path) -> String {
    let path = Path::new(file_path);
    match path.strip_prefix(project_root) {
        Ok(relative) => relative.to_string_lossy().to_string(),
        Err(_) => file_path.to_string(), // Outside project root: store as-is
    }
}
```

**Rationale:**
- Portable across machines. `/home/alice/dev/nmem/src/main.rs` becomes `src/main.rs`.
- Stable across moves. Renaming the project root doesn't invalidate stored paths.
- Matches how developers think about files ("src/main.rs", not the absolute path).
- Cold start portability: database copied to new machine with different home directory still works.

**Exception:** Paths outside the project root (e.g., system files, dependencies in `~/.cargo/`) are stored as-is. These are less common and less portable regardless.

**The project root** used for relativization is the same path resolved by the project identity logic: git root, explicit config, or canonical cwd.

## Open Questions

### Q1: Should project identity include a namespace for collision avoidance?

Using directory name alone means two projects named `app` collide. Options:
- Accept collisions as rare and fixable with explicit config
- Use `org/project` format (e.g., `viablesys/nmem`) derived from git remote
- Use a hash of the full path as a suffix (e.g., `nmem-a3f2`)

Git remote parsing is fragile (SSH vs HTTPS URLs, multiple remotes). Hashing loses human readability. Leaning toward accepting collisions with explicit config as escape hatch, but this deserves implementation-time testing.

### Q2: How should project deletion work?

Currently: `DELETE FROM observations WHERE project = ?` + incremental vacuum reclaims space. No separate "project lifecycle" — the project exists as long as it has observations.

Should there be an explicit `nmem forget-project <name>` command? Or is ad-hoc SQL sufficient for a developer tool? If forgetting is added (ADR-005), it may subsume project deletion as a special case of retention policy.

## When to Reconsider

Move to per-project databases (Position B) if:
- Secret leakage across projects causes an actual incident (not hypothetical risk)
- Project count exceeds ~50 and per-project queries degrade (unlikely at nmem's row volume)
- Regulatory or compliance requirements demand physical data isolation

Add visibility tiers (Position C) when:
- S4 synthesis is implemented and can classify observation scope
- Cross-project context injection is added to session start
- There's evidence that unclassified cross-project queries return too much noise

## Consequences

### Positive

- **Cross-project learning is possible.** The single database makes "what Rust patterns have I used across all projects?" a single query. This is nmem's differentiator over per-project config files.
- **One file to manage.** Backup, restore, cold start, storage budget — all single-file operations. Matches ADR-001's portability story.
- **No schema multiplication.** One migration path, one FTS index, one vacuum schedule.
- **Project identity is cheap.** Git root detection + directory name is fast (single subprocess), cacheable, and handles the common case. Explicit config handles edge cases.
- **Relative paths are portable.** Database works on a different machine without path rewriting.

### Negative

- **Soft isolation only.** Project scoping is enforced by query code, not file boundaries. A bug can leak cross-project data. Defense-in-depth depends on ADR-007 filtering.
- **Project name collisions.** Two projects named `app` share observations. Detectable but requires user intervention (explicit config) to fix.
- **No per-project lifecycle.** Deleting a project's data is a DELETE + vacuum, not rm -rf. The database file doesn't shrink immediately without vacuum.
- **Cross-project noise.** Without visibility tiers, cross-project queries return everything. At small scale this is manageable; at large scale it needs S4 filtering.

## References

- ADR-001 — Storage layer. Single database at `~/.nmem/nmem.db`. WAL mode, FTS5, rusqlite.
- ADR-002 — Extraction strategy. `project TEXT NOT NULL` column, `idx_obs_project` index. Structured extraction, no LLM.
- ADR-003 — Process model. No daemon. Per-invocation database open/close. MCP server is session-scoped.
- ADR-006 — Interface protocol. Project parameter defaulting convention (absent = current, null = all).
- ADR-007 — Trust boundary (unlocked). Isolation model determines secret leakage blast radius.
- `rusqlite.md` — Connection management, parameterized queries for project-filtered SQL.
- `rmcp.md` — MCP server struct with project field, session-scoped stdio transport.
- `claude-code-hooks-events.md` — `cwd` field in hook payloads, source of project identity.
- DESIGN.md — Project recursion, cross-project pattern recognition, cold start portability.

## Revision History

| Date | Version | Changes |
|------|---------|---------|
| 2026-02-08 | 0.1 | Stub with framing and dependencies. |
| 2026-02-14 | 1.0 | Full ADR. Three positions (single DB, per-project DBs, visibility tiers). Adversarial analysis. Decision: single database with row-level project scoping. Project identity resolution (git root, explicit config, canonical cwd). Cross-project observation model. Relative path storage. |
| 2026-02-14 | 1.1 | Refined. Added git subprocess caching note. MCP server project awareness (resolve once at startup). `.nmem.toml` minimal schema. Cross-ADR defaulting convention aligned with ADR-006. |
| 2026-02-14 | 1.2 | Refined with library topics. Added `read_nmem_config` implementation (serde + toml). References updated: rusqlite.md, rmcp.md, claude-code-hooks-events.md, ADR-006. |
