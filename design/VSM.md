# nmem — Viable System Model Assessment

Current state of each VSM system. Update as capabilities mature.

## S1 — Operations

Capture, store, retrieve. **Functional.**

- Hooks record observations on SessionStart, PostToolUse, Stop
- Structured extraction classifies tool calls into obs_types without LLM
- FTS5 indexes observations and prompts with porter stemming
- MCP server exposes search, get_observations, recent_context
- Context injection pushes relevant history at session start
- Secret filtering redacts before storage

No known gaps at current scale. Extraction coverage could expand (SendMessage, Skill invocations not captured) but this is incremental.

## S2 — Coordination

Dedup, sequencing, concurrency. **Functional.**

- SQLite WAL provides concurrent read access with single writer
- Dedup checks (session + obs_type + file_path + timestamp window) prevent duplicate observations
- prompt_id links observations to most recent user intent, providing causal ordering
- Session boundaries (started_at/ended_at) scope temporal context

No known gaps. Coordination is inherently simpler in a single-user, single-machine system. Multi-agent coordination would stress S2 significantly.

## S3 — Control

Resource management, storage budgets, compaction. **Manual.**

What exists:
- Retention config with per-type TTL (90-730 days)
- `nmem maintain --sweep` runs retention purge
- `nmem maintain --rebuild-fts` reconstructs indexes
- `nmem purge` provides targeted deletion
- WAL checkpoint on session end

What's missing: **autonomy.** S3 is a control panel, not a controller. Nothing triggers sweeps, compaction, or integrity checks without human invocation. A viable S3 would:
- Monitor storage growth and trigger sweeps when budgets are exceeded
- Schedule compaction during idle periods (no active sessions)
- Escalate anomalies (unexpected growth, failed writes) rather than silently degrade

**Path to autonomy**: A background timer or post-session hook that checks storage size against a budget and runs sweeps when thresholds are crossed. The logic exists; the trigger doesn't.

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

Adaptation, pattern recognition, environment sensing. **Missing entirely.**

S4 answers: "what's changing, and how should we adapt?" In nmem's context:

**Inward-facing (self-model):**
- Which observations get retrieved vs. ignored? (retrieval feedback loop)
- Are certain obs_types over-represented in storage but under-used in retrieval? (capture ROI)
- Are sessions getting shorter or longer on a project? (convergence signal)
- Do certain file paths appear across many sessions? (hotspot detection)

**Outward-facing (environment model):**
- What are other agents working on? (multi-agent awareness — requires networking)
- Has the project structure changed? (new modules, renamed files)
- Are hook payloads evolving? (upstream format detection)

**Cross-session synthesis:**
- Cluster related observations into higher-level summaries
- Detect recurring patterns ("every debugging session on this project touches the same three files")
- Produce durable insights that outlive individual observations

S4 is where the system transitions from recorder to memory. Without it, nmem captures but doesn't learn. The `syntheses` table is designed (ADR-002 Q3) but not created.

**Activation criteria**: S4 should be earned, not speculated. Implement when:
- Retrieval quality is demonstrably poor across multiple sessions
- Volume exceeds where scan-and-rank works (~10K+ observations)
- Multi-agent networking creates richer content worth semantic analysis

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
| S1 Operations | Functional | Incremental extraction coverage |
| S2 Coordination | Functional | Multi-agent would stress this |
| S3 Control | Manual | Needs autonomous triggers |
| S3* Audit | Minimal | Needs functional integrity checks |
| S4 Intelligence | Missing | Core gap — no learning, no adaptation |
| S5 Policy | Static | No tension to resolve without S4 |

The organism captures and coordinates (S1-S2) but doesn't regulate itself (S3), verify its own health (S3*), learn from experience (S4), or evolve its policies (S5). It's a viable recorder, not yet a viable system.

## What closes the loop

1. **S3 autonomy** — smallest change, highest immediate impact. A post-session hook or timer that checks storage and runs sweeps. The logic exists; wire a trigger.
2. **S3* functional audits** — track extraction success rate, retrieval hit rate, filter accuracy. Surface in `nmem status`.
3. **S4 retrieval feedback** — instrument whether context-injected observations appear in the agent's subsequent tool calls. This is the minimum viable learning signal.
4. **S4 synthesis** — cluster and summarize after enough structured data accumulates. Don't rush this.
5. **Multi-agent S2/S4** — networking, shared memory, cross-agent retrieval. Changes the nature of S2 coordination and gives S4 richer input.
6. **S5 adaptive policy** — emerges naturally once S3 and S4 are both active and creating tension.
