# nmem — TODO

Missing features, why they're missing, and what triggers implementation.

## Parity gap (was in claude-mem, missing in nmem)

### S1's S4: session summarization (rolling + end-of-session)
S1 is itself a viable system (VSM recursion). Its S4 — operational intelligence about what was captured — is missing entirely. claude-mem produced structured summaries at every prompt turn and at session end. Each summary had: `request`, `investigated`, `learned`, `completed`, `next_steps`, `files_read`, `files_edited`, `notes`. These were FTS5-indexed and surfaced in context injection.

nmem has none of this. `sessions.summary` column exists but is never populated. No rolling summaries, no PreCompact snapshots, no structured narrative.

This is the biggest functional gap. Without S1's S4:
- Context injection surfaces raw observations but no narrative of what was accomplished
- Long sessions lose signal when Claude Code compacts context (PreCompact fires, nmem ignores it)
- Cross-session retrieval finds file paths and commands but not intent or outcomes
- The outer S4 (cross-session synthesis) has no summaries to work with

**Schema reference** (claude-mem's `session_summaries` table):
- Per-prompt rows keyed by `(memory_session_id, prompt_number)`
- Structured fields: request, investigated, learned, completed, next_steps, files_read, files_edited, notes
- FTS5 on text fields for retrieval
- `discovery_tokens` tracked LLM cost per summary

**What needs to happen**:
1. Add a `summaries` table with structured fields (not a single TEXT blob)
2. Hook into PreCompact to snapshot rolling session state before compaction
3. Generate summaries at Stop for session-end narrative
4. Decide on LLM strategy: local model, API call, or structured template from observations
5. Surface summaries in context injection and MCP search

**Why it was missing**: ADR-002 chose no-LLM for observation extraction to avoid hallucination. But summarization is compression of existing facts, not extraction of new ones — different function, different risk profile. Framing it as S1's S4 (operational intelligence within capture) rather than the outer S4 (cross-session pattern detection) makes the scope tractable: compress what happened in this session, don't synthesize across sessions.

**Design question**: Can S1's S4 be deterministic? Structured templates computed from observations (file lists, command outcomes, prompt intents) might produce a useful balance sheet without LLM. The question is whether deterministic compression captures enough signal or whether narrative coherence requires language generation. Try templates first.

## Deferred by design

### S4 Synthesis (cross-session pattern detection)
Periodic synthesis over observation clusters to produce cross-session patterns. Distinct from per-session summarization above — S4 operates across sessions to detect recurring themes, hotspots, and convergence signals.

**Trigger**: Per-session summarization implemented AND volume exceeds ~10K observations.

**Depends on**: Session summaries as input, `syntheses` table (schema designed in ADR-002 Q3 but not created).

### Auto-pinning (landmark detection)
Automatic identification of important observations exempt from retention. Manual `nmem pin <id>` works; intelligence-driven pinning requires S4.

**Trigger**: S4 synthesis implementation.

### Vector search (sqlite-vec)
Semantic similarity via embeddings. sqlite-vec DELETE is broken (upstream issues #178, #54, #220 — validity bit flipped but vector blobs and rowids stay in chunks; VACUUM doesn't reclaim). Storage leak is tolerable at nmem's scale (~400MB/year dead weight), and a periodic rebuild workaround fits `nmem maintain`. But the real issue is ROI: nmem stores short extracted facts, not long documents — FTS5 with porter stemming covers vocabulary matching well. The one win is semantic search over prompts (natural language intents), but that's a narrow surface.

Revisit after multi-agent networking with shared memory, where cross-agent semantic retrieval over richer content may justify the complexity.

**Trigger**: Multi-agent memory sharing implemented AND retrieval quality gaps demonstrated.

### Visibility tiers (cross-project filtering)
Classify observations as local/global/restricted to prevent unintended cross-project leakage. Crude heuristics (file_path = local) misclassify too often without S4.

**Trigger**: S4 synthesis can assign tiers with confidence.

## Incomplete

### Metrics instrumentation
`metrics.rs` has the OTLP provider and config wired, but no metrics are actually emitted. No metric taxonomy defined (what to measure: record latency, FTS5 query time, sweep duration, observation counts).

**Why**: Infrastructure was built speculatively. Need to decide what's worth measuring before wiring counters into call paths.

### ADR-008: Distribution and installation
Stub — five open questions, no decisions. Blocks `nmem init`, binary packaging, and upgrade strategy.

**Why**: Claude Code marketplace/plugin packaging mechanics are unknown. Can't finalize distribution without understanding the target.

## Low priority

### Per-project retention overrides
Global `[retention.days]` only. No `[projects.X.retention.days]` override.

**Why**: Global policy sufficient at current scale. Add when a user needs per-project tuning.

### Configurable type weights in context injection
`context.rs` hardcodes type_weight (file_edit=1.0, command=0.67, etc.). Not exposed in config.

**Why**: Defaults are reasonable. No evidence they need tuning.

### Expanded tool classification
`extract.rs` maps tools to obs_types but is coarse (e.g., task_spawn covers TaskCreate/TaskUpdate/TaskList without distinguishing). SendMessage and Skill invocations not captured.

**Why**: ADR-002 chose minimal classification. New types added as capture proves insufficient.

## Won't do (unless evidence changes)

### Per-project databases (ATTACH)
ADR-004 evaluated and rejected (Position B). Single DB with row-level scoping chosen. ATTACH only needed if architecture reverses.

### Cursor-based pagination
Current `limit`/`offset` with cap at 100 results. Volume doesn't warrant keyset pagination (~20K rows/year).
