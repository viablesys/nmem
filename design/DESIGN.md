# nmem

**nm** (nanometer — precision at small scale) + **mem** (Latin *memor* — mindful, remembering)

Also evokes: Mnemosyne (Greek, mn↔nm mirror), n-gram pattern recognition

## Status: Early Design Phase
Started: 2026-02-08
Predecessor: claude-mem (TypeScript/Node plugin)

> **Annotations added 2026-02-08** — storage-layer questions raised by review of ADR-001. Blockquotes are annotations; all original text is preserved.

## Key Decisions
- **Name: nmem** — small, precise, pattern-based memory
- **No JavaScript/TypeScript** — security concern, npm supply chain risk
- **Security-first** — all security considerations to be explored fully
- **Design before implementation** — multi-day collaborative process

## Organizing Principle: Viable System Model (VSM)
Stafford Beer's VSM as the logical design framework.

- **S1 (Operations)** — capture, store, retrieve, search. Multiple operational units.
- **S2 (Coordination)** — dedup, concurrency, sequencing across sessions.
- **S3 (Control)** — compaction, retention, storage budgets. The "operator" logic.
- **S3* (Audit)** — integrity checks, consistency verification.
- **S4 (Intelligence)** — adapt capture strategies, detect useful vs noise.
- **S5 (Policy)** — configuration, core purpose, identity.
- **Environment** — user/operator + any CLI/IDE/harness (Claude Code is one consumer).

### Key Principles
- System is viable in its own right, not a plugin bolted onto a host
- No tight coupling to any specific harness — protocol-based interface
- Recursive structure (S1 units can be viable systems themselves)
- Variety management (Ashby's Law) — absorb observation variety without overwhelm
- Portability and composition — works with any interface layer above

## Core Requirements
- **Automatic** — no manual intervention during normal operation
- **Near zero maintenance** — self-managing, no babysitting
- **Own lightweight operator** — self-managing agent/daemon pattern
  - Auto-compaction / pruning
  - Self-healing (corrupt state, index rebuilds)
  - Lifecycle management (no orphan processes)
  - Storage hygiene (rotation, vacuuming, retention)
  - Health monitoring

## Decision Points to Explore

### Language & Runtime
- Language choice (Rust vs Go vs other)

> **[Q: Storage layer implications]** ADR-001 assumes Rust + rusqlite. If Go is chosen instead, the storage story changes significantly — go-sqlite3 (CGO) vs modernc/sqlite (pure Go) vs something else entirely. The "thin database, smart application" principle from the ADR holds regardless of language, but the specific extension loading mechanism, FFI safety model, and concurrency patterns differ. Does the storage ADR need to wait on language choice, or is SQLite the right answer independent of language?

### Security
- Eliminate localhost HTTP port? (Unix socket, stdin/stdout, etc.)
- Scope of observation (every tool call vs selective)
- Database encryption at rest
- All other security considerations TBD

> **[Q: Encryption strategy]** ADR-001 doesn't address encryption at all. If encryption-at-rest is required, it fundamentally affects the storage layer choice. Options: (a) SQLCipher — replaces the standard SQLite library, incompatible with standard tooling like `sqlite3` CLI, may complicate extension loading (does sqlite-vec work with SQLCipher?), (b) application-level encryption of sensitive fields only — simpler, but queries can't operate on encrypted data, (c) filesystem-level encryption (LUKS, fscrypt) — transparent but depends on OS, (d) no encryption, rely on filesystem permissions. Each has different implications for the "self-healing" requirement — can the operator rebuild indexes on encrypted data? Does encryption break `PRAGMA integrity_check`?
>
> **[Q: Scope vs storage cost]** "Every tool call vs selective" directly affects database size, write frequency, and whether compaction is a day-one requirement or a future concern. claude-mem captured everything and generated mostly garbage. If nmem is selective from the start, the storage layer can be much simpler — smaller DB, less index pressure, maybe FTS5 alone is sufficient without vector search.

### Architecture
- SDK agent subprocess — keep, replace, or eliminate?
- Vector DB approach (embedded vs external vs skip entirely)
- Plugin architecture (hooks, MCP, etc.)

> **[Q: One DB or two?]** claude-mem used SQLite + Chroma (two databases, two persistence layers, two failure modes). ADR-001 proposes sqlite-vec to unify vector search into SQLite. But is this collapsing the right boundary? If vector search is an S4 concern (intelligence/reflection) and structured data is S1 (operations), maybe they should be separate stores with different lifecycles — the vector index is rebuildable from the structured data, so it's a derived artifact, not primary storage. Losing it costs recomputation, not data. That changes the reliability requirements.
>
> **[Q: What does "skip entirely" look like?]** If vector search is skipped, what retrieval strategies remain? FTS5 keyword search, recency-based retrieval, project/tag filtering, explicit user queries. Are these sufficient for the "what's relevant to what I'm doing?" interface goal? The predecessor's MCP tools (search → timeline → get_observations) are essentially structured queries, not vector lookups. Would nmem lose much by starting without vector search and adding it later only if retrieval quality is demonstrably poor?

### Operator Pattern
- Long-running daemon (persists across sessions) vs session-bootstrapped (starts/stops with each session)?
- Invisible (never think about it) vs observable (can ask about health/status)?
- Zero configuration (works out of the box) vs initial setup acceptable?

> **[Q: Daemon vs session — storage implications]** A long-running daemon can hold a persistent database connection, run background compaction, maintain WAL checkpointing, and keep caches warm. A session-bootstrapped process opens/closes the DB each time — simpler, but loses the "self-managing operator" benefits. ADR-001's PRAGMA settings (64MB cache, WAL mode) implicitly assume a persistent connection. If session-bootstrapped, the cache warms from cold each time and WAL checkpointing becomes the OS's problem (or the session's startup cost).
>
> **[Q: Concurrent access pattern]** If a daemon holds the DB open and MCP queries also need to read, WAL mode handles this cleanly (concurrent readers, single writer). But if multiple sessions could run simultaneously (e.g., two terminal tabs), who coordinates writes? SQLite's `busy_timeout` handles contention at the DB level, but at the application level — do two daemon instances share a DB? Or does one daemon serve multiple sessions? This affects whether the daemon is per-user, per-project, or per-session.

### Forgetting & Retention
- Intentional deprecation — not just compaction but recognizing stale/untrue observations
- Theory of forgetting as S3 concern

> **[Q: What does forgetting look like at the storage layer?]** Options range from simple (DELETE rows older than N days) to sophisticated (LLM-assessed relevance decay, merge-and-summarize). If observations are soft-deleted or archived, the DB grows indefinitely and needs vacuuming. If hard-deleted, they're gone — no undo unless there's a backup (which the ADR solved with Litestream, but we're questioning that). Is there a middle ground? E.g., a two-tier store: hot (recent, frequently accessed) and cold (compressed export, queryable but not indexed). SQLite's ATTACH DATABASE could serve this — query across both, but only maintain indexes on the hot tier.
>
> **[Q: Forgetting vector embeddings — ANSWERED, and it's bad]** Verified against sqlite-vec GitHub issues (2026-02-08): DELETE on vec0 tables is currently broken. Issue #178 confirms DELETE doesn't remove vector blobs. Issue #54 (filed by the author) confirms rowid and vector data aren't cleared. Issue #220 confirms VACUUM doesn't reclaim space. The author has filed #184/#185 for space reclamation and vacuum/optimize commands — neither exists yet. An open PR (#243) to fix delete cleanup hasn't been merged.
>
> **This is a hard constraint for nmem's forgetting strategy.** If sqlite-vec is adopted, the S3 (control) layer cannot use DELETE to manage storage budgets — the DB grows monotonically. Options: (a) treat vec0 as ephemeral — periodically drop and rebuild from structured observation data, (b) defer sqlite-vec adoption until upstream fixes land, (c) skip vector search entirely and rely on FTS5 + structured queries (which don't have this problem). Option (a) reinforces the DESIGN.md annotation under Architecture about vectors being a derived artifact, not primary storage.

### Algedonic Signals
- Pain/pleasure bypass channels for urgent system health issues
- Silent when healthy, interrupts when not

### Observation Extraction
- Current: SDK agent (LLM subprocess) for every tool call — expensive, fragile, security risk
- Alternative: structured extraction (heuristics, harness-declared noteworthiness) for 80% of cases
- Reserve LLM for S4 (periodic reflection), not inline extraction?

> **[Q: Extraction strategy determines schema]** If observations come from structured extraction (parsed tool calls, file paths, error codes), the schema is predictable and FTS5 is likely sufficient for retrieval. If observations come from LLM synthesis (natural language summaries, inferred intent), the data is unstructured text and vector search becomes more compelling. This decision directly determines whether sqlite-vec is needed. Resolve extraction strategy before committing to the storage extensions.
>
> **[Q: What's actually worth storing?]** claude-mem stored everything and got garbage. What would a minimal viable observation set look like? Candidates: files read/written, errors encountered, decisions made (explicit in conversation), commands run. All of these are structured data extractable without an LLM. The question is whether the *connections between them* (why a file was read, what a decision implies for future work) are worth capturing — and whether that requires LLM synthesis or can emerge from structure alone (e.g., temporal proximity, session grouping).

### Trust & Secrets
- Explicit exclusion rules for credentials, tokens, private data
- Classification of observations by sensitivity
- S5 policy with S1 operational implications

> **[Q: Where does filtering happen?]** If secrets filtering is an S1 concern (operational, inline), it must happen before data hits the database — the storage layer never sees sensitive data. If it's an S5→S1 policy flow, the rules are configured at S5 but enforced at S1 write time. Either way, the storage layer (ADR-001's concern) is downstream of this decision. But: if filtering fails and a secret is stored, can it be reliably purged? SQLite doesn't zero-fill deleted pages by default — `PRAGMA secure_delete = ON` exists but has performance cost. And if WAL mode is active, the secret may persist in WAL frames until checkpoint. This connects back to the encryption-at-rest question: if the DB is encrypted, a leaked secret in storage is less catastrophic.

### Relevance vs Similarity
- Vector similarity is weak proxy for true relevance
- Contextual relevance: project, task phase, current intent
- S4 intelligence concern

> **[Q: Can relevance be approximated without vectors?]** Structured signals that may proxy relevance better than cosine similarity: (a) recency — recent observations are more likely relevant, (b) project scope — same project is more relevant, (c) file overlap — observations mentioning files in the current working set, (d) tag/type matching — "bugfix" observations when debugging, "decision" observations when designing, (e) frequency — observations retrieved often in the past are likely useful. All of these are queryable with standard SQL (WHERE/ORDER BY/JOIN). Vector search adds value mainly for fuzzy semantic matching ("I'm doing something similar to what I did before but I can't name it"). How often does that actually happen vs structured retrieval covering the need?
>
> **[Q: Hybrid retrieval as a later optimization?]** Start with structured retrieval (FTS5 + metadata filters), measure retrieval quality, add vector search only if there's a demonstrated gap. This avoids the sqlite-vec dependency until it's proven necessary. ADR-001 can be amended at that point.

### Cold Start / Bootstrapping
- New system or new machine has no memories
- Import from existing context (git history, docs, CLAUDE.md)?
- Or accept cold start and earn value over time?

> **[Q: Cold start as a portability test]** If nmem's database can be copied to a new machine and just work, cold start is solved by file transfer. SQLite's single-file model (praised in ADR-001) enables this — but only if there are no absolute paths stored in observations, no machine-specific state in the schema, and extensions are available on the target. Does the schema need a portability constraint? E.g., store project-relative paths, not absolute ones.
>
> **[Q: Import vs earn]** Importing from git history or CLAUDE.md is an S1 bootstrap operation — but who does the extraction? If structured extraction is the primary capture method, importing git history means parsing commits/diffs programmatically. If LLM synthesis is used, importing means running the LLM over historical data (expensive, and the same hallucination risk from claude-mem applies). A middle ground: import only structured facts (files changed, commit messages, timestamps) and let S4 synthesize patterns over time.

### Interface Design
- Consumer shouldn't need to know internals
- Protocol-level, not implementation-level
- "What's relevant to what I'm doing?" / "Remember what's worth remembering"
- System decides the how internally

> **[Q: MCP as the protocol?]** claude-mem exposed MCP tools (search, timeline, get_observations). If nmem is harness-agnostic, MCP is one possible protocol but not the only one. Alternatives: Unix socket with a simple request/response protocol, stdin/stdout JSON-RPC (like LSP), or even a CLI that the harness shells out to. The storage layer doesn't care about the protocol, but the protocol choice affects how retrieval queries are expressed — MCP tools have a specific invocation model that shapes what queries are easy vs hard.
>
> **[Q: Push vs pull?]** "What's relevant to what I'm doing?" is a pull model — the consumer asks. But SessionStart hook injection is a push model — nmem proactively provides context. Which is primary? Push requires nmem to predict relevance (harder, more S4-dependent). Pull lets the consumer (or the LLM in conversation) decide when to query. The storage layer needs to support both efficiently — push means fast "recent + relevant" queries at session start, pull means arbitrary search at any time.

### Project Recursion
- Project-scoped but cross-pollinating
- Each project as S1 unit with local memory
- Cross-project pattern recognition without leaking secrets
- S2 coordination across projects

> **[Q: One database or many?]** ADR-001 assumes a single database. But project recursion suggests each project could have its own SQLite file — true isolation, no secret leakage by design, independent lifecycle (delete a project's DB when the project is done). Cross-project queries would require ATTACH DATABASE or an S2 coordinator that queries multiple DBs. Trade-off: simplicity of a single DB with row-level project scoping vs strong isolation of separate files with cross-query complexity.
>
> **[Q: What crosses project boundaries?]** If each project is an S1 unit, what observations are project-local vs global? Examples: "this Rust pattern works well" (cross-project), "the auth module is in src/auth/" (project-local), "prefer reqwest over ureq" (cross-project preference). The storage schema needs a scoping model. And the secrets concern compounds — a cross-project observation must not leak project A's secrets into project B's context.

## Predecessor Architecture (claude-mem, for reference)
- Background worker on port 37777 (TypeScript/Node)
- SDK agent (observer-only Claude subprocess)
- Pipeline: PostToolUse hook → HTTP POST → SDK agent → SQLite + Chroma
- Hook events: SessionStart, UserPromptSubmit, PostToolUse, Stop
- MCP tools: search, timeline, get_observations
- DB: SQLite (WAL) + Chroma vector DB
- Source: ~/.claude/plugins/marketplaces/bpd1069/src/

> **[Q: What from claude-mem's storage model actually worked?]** SQLite (WAL) was reliable — no corruption reports, concurrent access worked. Chroma added complexity (separate process, Python dependency, embedding generation) for questionable retrieval quality. The MCP query pattern (search → timeline → get_observations) is essentially structured SQL with pagination. Was Chroma ever the thing that found a relevant observation that SQL couldn't? If not, that's evidence for starting without vector search.
>
> **[Q: What's the actual data volume?]** claude-mem generated ~9 observations in early testing, mostly garbage. Even in steady-state use, a developer might generate tens of observations per session, a few sessions per day. That's maybe 100-500 observations/month. At that scale, brute-force approaches work fine — full table scans are fast, FTS5 is overkill, and vector search is solving a problem that doesn't exist yet. The storage layer should be designed for the data volume nmem will actually see in its first year, not for hypothetical scale.
