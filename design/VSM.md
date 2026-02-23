# nmem — Viable System Model Assessment

**Feature complete as of 2026-02-22.** All VSM layers functional. Remaining work: S3 cross-project retention (blocked on multipass bootstrap) and distribution/installation (ADR-008).

## S1 — Operations

Capture, store, retrieve. **Functional, S4 partial.**

S1 is itself a viable system (VSM recursion). Its internal subsystems:

| S1's... | Function | State |
|---------|----------|-------|
| S1 | Raw capture (observations, prompts) | Functional |
| S2 | Dedup, ordering, prompt-observation linking | Functional |
| S3 | Content limits, truncation, what to capture | Partial |
| S4 | Summarization — compress what was captured | Functional (v2) |
| S5 | Capture policy (config, sensitivity, filtering) | Functional |

What works:
- Hooks record observations on SessionStart, PostToolUse, UserPromptSubmit, Stop
- Structured extraction classifies tool calls into obs_types without LLM
- FTS5 indexes observations and prompts with porter stemming
- MCP server exposes search, get_observations, recent_context, timeline
- Context injection is summary-primary: session summaries (with learned/next_steps) are the main signal, raw observations filtered to pinned items + recent file_edits + git milestones only. Cross-project limited to pinned observations.
- Secret filtering redacts before storage

**S1's S4 (session summarization) — v2 validated.** End-of-session summarization via local LLM (granite-4-h-tiny on LM Studio). Stop hook generates structured JSON summaries (intent, learned, completed, next_steps, files_read, files_edited, notes) stored in `sessions.summary`, surfaced in context injection and `session_summaries` MCP tool, streamed to VictoriaLogs.

Key design insight: nmem is a tool for the agent, not the user. Summaries are optimized for context reconstruction by the next AI session — decisions, trade-offs, and conclusions that should not be re-derived. The prompt explicitly frames the consumer as the next Claude session. Thinking blocks (`source = 'agent'` in prompts table, extracted from transcript by `scan_transcript`) feed the `learned` field — they contain the richest reasoning signal.

**Remaining S1's S4 gaps:**
- PreCompact events are ignored — long sessions lose signal when context is compacted
- No rolling summaries — only end-of-session, which compresses too much for long sessions
- Summary content not FTS5-indexed — search by project only, not by content
- Summaries stored in `sessions.summary` column, not a dedicated table (limits future rolling/per-prompt summaries)

Framing this as S1's S4 (not the outer S4) keeps the design coherent:
- S1's S4 compresses what happened *within a session* — bounded, operational, simple
- The outer S4 synthesizes *across sessions* — unbounded, adaptive, complex
- The outer S4 depends on S1's S4 having done its job first

The implementation confirmed that narrative coherence requires language generation — structured templates from observations alone don't capture intent or causality. An LLM (even a small local one) is necessary for this layer, validating ADR-002's Position C framing for S4-level synthesis. Prompt engineering matters even for small models — framing the task correctly (agent context reconstruction vs. human report) substantially changes output quality.

Incremental gaps: extraction coverage could expand (SendMessage, Skill invocations not captured).

## S2 — Coordination

Dedup, sequencing, concurrency, classification. **Functional.**

- SQLite WAL provides concurrent read access with single writer
- Dedup checks (session + obs_type + file_path + timestamp window) prevent duplicate observations
- prompt_id links observations to most recent user intent, providing causal ordering
- Session boundaries (started_at/ended_at) scope temporal context
- **Five classifier dimensions** at write time: phase (think/act), scope (converge/diverge), locus (internal/external), novelty (routine/novel), friction (smooth/friction)
- Shared TF-IDF + LinearSVC inference engine (`s2_inference.rs`) — sub-millisecond per classification
- All classifiers use the same architecture: exported JSON model weights, Rust-native inference, OnceLock caching
- `nmem backfill --dimension <name>` retroactively classifies historical observations

No known gaps. The five classifier dimensions replaced the need for cross-session synthesis — session fingerprints, episode character, and retrieval signals are now structured data queryable with SQL, not text requiring LLM interpretation. Multi-agent coordination would stress S2 significantly.

## S3 — Control

Resource management, storage budgets, compaction. **Functional.**

What exists:
- Retention config with per-type TTL (90-730 days), **enabled by default**
- Automatic sweep on session Stop — runs after summarization, before WAL checkpoint
- **Sweep precondition**: only sweeps sessions where `summary IS NOT NULL` — ensures the compression pipeline (S1's S4 → S4 episodes → obs_trace) completes before forgetting begins
- **obs_trace rollup**: `work_units.obs_trace` freezes per-observation fingerprints at episode detection time — the downsampling tier that makes observation deletion safe
- Two sweep triggers: count-based (>100 expired observations) and size-based (`max_db_size_mb` config, DB + WAL)
- `nmem maintain --sweep` for manual intervention
- `nmem maintain --rebuild-fts` reconstructs indexes
- `nmem purge` provides targeted deletion
- WAL checkpoint on session end

Incremental gaps (not blocking):
- **Compaction scheduling** — no idle-period detection for vacuum/rebuild
- **Cross-project retention** (S3:nmem + S3:multipass) — per-project retention policies across repos. Blocked on multipass bootstrap.
- **Consumer fallback paths** — `current_stance` and `file_history` don't yet read from `obs_trace` when observations are swept. ~86 days from 2026-02-22 before the 90-day TTL makes this necessary.

## S3* — Audit

Integrity verification, anomaly detection. **Minimal.**

What exists:
- FTS integrity check (`nmem maintain`)
- `nmem status` reports DB size, observation counts, last session
- `redacted: true` metadata flag on filtered observations

What's missing:
- **Extraction quality monitoring** — if tool payloads change format and extraction starts producing empty content fields, nothing notices
- **Filter calibration** — no signal on false positive rate (legitimate content redacted) or false negatives (secrets that slipped through)
- **Retrieval usefulness** — no measurement of whether surfaced observations were actually consumed by the agent
- **Schema drift detection** — if hook payload format changes upstream, extraction silently degrades

S3* should be the immune system — detecting pathology before it becomes visible in S1 output. Currently it only checks structural integrity (FTS index health), not functional integrity (is the system doing its job).

## S4 — Intelligence

Adaptation, pattern recognition, future-oriented action. **Functional.**

S4 answers: "what's changing, what should we do next, and how should we adapt?" It has two faces:

**Outward-facing (initiating future work):** The task queue (`s4_dispatch.rs`) is S4's first concrete module. It queues work for future execution and dispatches it into tmux panes running Claude Code — each dispatched session is its own viable system (VSM recursion at the system level). A systemd timer provides the clock. This is indirect control: S4 decides *what* to do; the spawned session decides *how*.

**Inward-facing (recognizing patterns):** S4 now has three inward-facing capabilities:

1. **Episodic memory** (`s4_memory.rs`) — detects episode boundaries from user prompt intent shifts, annotates with observation metadata (hot files, phase signature, 5 classifier dimensions), generates LLM narratives, freezes `obs_trace` rollup for S3 sweep safety. Episodes feed context injection (48h window) and session stance analysis.

2. **Session stance** (`current_stance` MCP tool) — EMA-smoothed phase×scope trajectory over the observation sequence, with retrieval guidance. Surfaces the session's cognitive rhythm.

3. **Cross-session pattern detection** — `nmem learn` (`s3_learn.rs`) scans the full observation and summary corpus to surface:
1. **Repeated failures** — same command failing across sessions, normalized and heat-scored
2. **Recurring errors** — error signatures from `metadata.response` appearing across sessions
3. **Repeated intents** — similar session intents clustered via Jaccard similarity on keyword bags
4. **Unresolved investigations** — files read across sessions but never edited (reference paths excluded)
5. **Confirmed stuck loops** — cross-reference of intent + failure/error session overlap (≥2 shared sessions)

Heat scoring uses exponential decay (configurable half-life, default 7 days) normalized to 0-100. Output is `~/.nmem/learnings.md` — a structured report for collaborative review before merging insights into CLAUDE.md. This is pattern detection, not autonomous action — the report is a seed that requires human/agent judgment to act on.

Work unit detection (ADR-010: prompt-driven episodic memory) and autonomous context management remain designed but not yet implemented. Context injection (`s4_context.rs`, moved from S1 to S4 to reflect its intelligence function) uses summaries as primary content, raw observations filtered to signal only (pinned + recent edits + git milestones). Currently mechanical: the same queries run every SessionStart regardless of session type or agent need. Concrete limitation: if a user switches between two features in the same project, context injection doesn't prioritize the feature being returned to — it's time-ordered, not relevance-ordered. True S4 context injection would adapt based on the session's first prompt or detected work unit, selecting relevant episodes by intent match (ADR-010 Q2).

### Core concept: the work unit

A work unit is a bounded chunk of coherent work within a session, recognized by observing the stream — not declared by the user. The signal is the pattern:

1. **Intent** — user prompt sets direction
2. **Investigation** — reads, searches, exploration (high read:edit ratio)
3. **Execution** — edits, commands, builds (high edit:read ratio)
4. **Completion** — the pattern resets (new unrelated intent, or ratio shifts dramatically)

The ratio of user prompts : thinking blocks : tool calls (and the composition of tool types) characterizes the work phase. Hot files — informed by intent and access patterns across sessions — provide the topical signal. When the pattern shifts, that's a work unit boundary.

### S4 as context manager

S4's primary function is autonomous context window management:

1. **Detect work unit boundaries** — pattern recognition over the observation stream (cheap SQL queries, runs on every hook fire)
2. **Generate work unit summaries** — structured semantic summaries at boundaries (LLM, runs only at detected boundaries)
3. **Control context injection** — after a `/clear`, inject the last work unit summary plus relevant past summaries from memory
4. **Signal the agent** — when a boundary is detected mid-session, signal via async hook `additionalContext` that context should be refreshed

Work unit summaries are the new primary semantic structure — they replace raw observations as the unit of cross-session memory. They capture intent, files involved, outcome, and logical next steps at a granularity between individual observations (too fine) and session summaries (too coarse).

### S4 as the UI data model

The same work unit model powers the user-facing interface. The UI is S4's external surface, the way the MCP server is S1's:

- **Current work unit** — intent, hot files, progress through investigate→execute pattern
- **Work unit history** — completed summaries, searchable by intent
- **Context health** — window utilization, what's been compacted, what nmem would inject on reset

### Platform constraints (2026-02-15)

**Claude Code hooks** provide full observability (14 hook events, every tool call) but limited actuation:

| Capability | Status |
|-----------|--------|
| Observe all tool calls, prompts, session events | Yes — 14 hook events |
| Inject context at session boundaries | Yes — SessionStart `additionalContext` (fires on startup/resume/clear/compact) |
| Inject context mid-session | Partial — async hook `additionalContext` delivered next turn, but [multiple bugs](https://github.com/anthropics/claude-code/issues/19909) block injection across PostToolUse/PreToolUse/UserPromptSubmit |
| Trigger context clear programmatically | No — no hook or MCP tool can invoke `/clear` |
| Partial context editing (clear old tool results) | No — API supports `clear_tool_uses` and `clear_thinking` strategies, but Claude Code doesn't expose these to hooks |

**Relevant upstream issues:**
- [#24252](https://github.com/anthropics/claude-code/issues/24252) — Context Hooks (hook into anything added to context)
- [#19909](https://github.com/anthropics/claude-code/issues/19909) — Conversation Lifecycle Hooks for Memory Provider Integration (lists 5 context injection bugs)
- [#25689](https://github.com/anthropics/claude-code/issues/25689) — Context usage threshold hook event (ContextThreshold at configurable %, blocking support)
- [#21132](https://github.com/anthropics/claude-code/issues/21132) — Claude clear context for itself (agent-initiated `/clear`)
- [#18427](https://github.com/anthropics/claude-code/issues/18427) — PostToolUse hooks cannot inject context visible to Claude

**Implication:** nmem can build S4's intelligence (work unit detection, summary generation, pattern recognition) now. The actuators depend on Claude Code platform evolution. In the meantime, S4 operates reactively — detect boundaries, store summaries, inject on the next SessionStart (after user-initiated `/clear`). Full autonomous context control requires either upstream hook improvements or an API-based harness.

**API-based harness alternative:** The Claude API's context editing beta (`clear_tool_uses_20250919`, `clear_thinking_20251015`) provides the full actuator set. An API-based agent harness (instead of Claude Code) would give S4 direct control over context on every turn — no platform dependency. This is a viable path if Claude Code's plugin model remains read-only for context.

### Original S4 concerns (still valid, lower priority)

**Inward-facing (self-model):**
- Which observations get retrieved vs. ignored? (retrieval feedback loop)
- Are certain obs_types over-represented in storage but under-used in retrieval? (capture ROI)
- Do certain file paths appear across many sessions? (hotspot detection)

**Outward-facing (environment model):**
- What are other agents working on? (multi-agent awareness — requires networking)
- Has the project structure changed? (new modules, renamed files)
- Are hook payloads evolving? (upstream format detection)

**Cross-session synthesis:**
- Cluster work unit summaries into higher-level patterns
- Detect recurring themes ("every debugging session on this project touches the same three files")
- Produce durable insights that outlive individual work units

## S5 — Policy

Identity, purpose, boundaries. **Static config.**

What exists:
- `config.toml` defines project sensitivity, retention windows, filter thresholds
- Project scoping derives boundaries from cwd
- ADRs codify architectural decisions and their rationale

What's missing: **mediation.** S5's role is to balance S3 (control — "conserve resources") against S4 (intelligence — "learn more, keep more"). With S4 absent, S5 has no tension to resolve. Config is set once and never revisited.

A mature S5 would:
- Adjust retention windows based on S4's assessment of observation value
- Tighten or relax sensitivity per project based on observed patterns
- Decide when S4 synthesis is worth the compute cost vs. when structured retrieval suffices

## System Viability Summary

| System | State | Gap |
|--------|-------|-----|
| S1 Operations | Functional | Summary-primary context injection; PreCompact, rolling, FTS5 indexing are incremental |
| S2 Coordination | Functional | Five classifiers replace need for cross-session synthesis; multi-agent would stress this |
| S3 Control | Functional | Sweep with summarization precondition + obs_trace rollup; cross-project retention deferred |
| S3* Audit | Minimal | Needs functional integrity checks |
| S4 Intelligence | Functional | Episodes, stance, patterns, task dispatch, context injection, obs_trace rollup |
| S5 Policy | Static | S3↔S4 tension exists (sweep precondition is the first mediation) but not yet dynamic |

**Feature complete (2026-02-22).** The system records, classifies (5 dimensions at write time), compresses (session summaries + episode narratives + obs_trace rollup), selectively forgets (sweep with summarization precondition), recalls (context injection with episodes + fallback summaries), detects patterns (cross-session learning), analyzes its own cognitive rhythm (session stance), initiates future work (task dispatch), and leaves structured markers for future sessions. The five classifiers give structured quantitative signals that compose with SQL — replacing the originally planned LLM-based cross-session synthesis.

**Remaining work (not blocking viability):**
1. **Distribution and installation** (ADR-008) — packaging, marketplace, `nmem init`
2. **Cross-project retention** (S3:nmem + S3:multipass) — per-project retention policies. Blocked on multipass bootstrap.

## Recurring Patterns

### Views as inter-system channels

Higher VSM systems observe lower systems without requiring the lower system to change. The implementation pattern: **SQL views over lower-system tables**.

A higher system (e.g. S4) needs derived signals from a lower system (e.g. S1). Two options:

1. **Table** — S1 writes to a signals table on every hook fire. This couples S1 to S4: S1 must know what S4 needs, and S4's requirements shape S1's write path. Inverts the VSM hierarchy.
2. **View** — S4 defines a view over S1's existing tables. S1 captures facts without knowing S4 exists. S4 reads S1 through its own lens. No coupling.

The view is the *channel* — S4's sensory input from S1. The higher system's own *table* (e.g. `work_units`) stores its conclusions. Both are needed, but they serve different roles:

| Mechanism | VSM role | Example |
|-----------|----------|---------|
| Lower-system tables | Operations data | `observations`, `prompts`, `sessions` |
| Views over those tables | Inter-system channel | `prompt_signals` (S4 observing S1) |
| Higher-system tables | Higher system's state | `work_units` (S4's conclusions) |

The view is defined and created by the higher system's own code, not in shared infrastructure (`schema.rs`). S4 creates its views at startup; `schema.rs` stays ignorant of S4's concerns. The schema owns the base tables — each system owns its own derived views.

This pattern repeats wherever a higher system needs to interpret lower-system data: S3 observing S1 for retention decisions, S3* auditing S1 for integrity, S5 reading S3 and S4 to mediate policy.

## What closes the loop

All core items are done. Remaining items are incremental improvements or future extensions.

1. ~~**S1's S4 (session summarization)**~~ — **Done (v2).** Agent-oriented summarization via local LLM. Thinking blocks feed `learned` field. Summaries streamed to VictoriaLogs.
2. ~~**S4 task dispatch**~~ — **Done.** Task queue with systemd-driven dispatch. Each dispatched session is its own viable system.
3. ~~**S4 work unit detection**~~ — **Done.** Prompt-driven episodic memory (`s4_memory.rs`). Boundary detection via Jaccard similarity on intent keyword bags. Annotation with hot files, phase signature, obs_trace rollup. LLM narrative generation. Idempotent.
4. ~~**S2 classification**~~ — **Done.** Five dimensions (phase, scope, locus, novelty, friction) at write time. TF-IDF + LinearSVC in pure Rust, sub-millisecond inference. These replace the need for LLM-based cross-session synthesis — session character is structured data, not text.
5. ~~**S3 safe forgetting**~~ — **Done.** Retention sweeps with summarization precondition + obs_trace rollup. The compression pipeline completes before S3 can sweep.
6. ~~**S4 session stance**~~ — **Done.** `current_stance` MCP tool — EMA-smoothed phase×scope trajectory with retrieval guidance.
7. ~~**S4 cross-session patterns**~~ — **Done.** `nmem learn` — repeated failures, recurring errors, stuck loops, unresolved investigations.
8. **S4 context actuation** — depends on Claude Code platform evolution (issues #24252, #25689, #21132) or building an API-based harness. Currently reactive: detect → store → inject on next SessionStart.
9. **Distribution and installation** (ADR-008) — packaging, marketplace, `nmem init`. Blocked on Claude Code plugin packaging format.
10. **Cross-project retention** — S3:nmem + S3:multipass. Blocked on multipass bootstrap.
11. **Multi-agent S2/S4** — networking, shared memory, cross-agent retrieval. Future extension.
