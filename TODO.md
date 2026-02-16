# nmem — TODO

Missing features, why they're missing, and what triggers implementation.

## Parity gap (was in claude-mem, missing in nmem)

### S1's S4: session summarization — v2 validated
End-of-session summarization via local LLM (granite-4-h-tiny on LM Studio). Stop hook calls OpenAI-compatible chat completions endpoint, stores structured JSON in `sessions.summary`, surfaces in context injection and `session_summaries` MCP tool. Summaries streamed to VictoriaLogs for dashboard visibility.

**v2 changes (2026-02-15):**
- Prompt reframed for agent context reconstruction, not human readability — consumer is the next AI session
- `request` → `intent` rename across summarize/context/serve
- Thinking blocks (`source = 'agent'` in prompts table) included in summarization payload — richest signal for `learned` field
- `investigated` field dropped (redundant with `files_read`)
- `learned` field now captures decisions, trade-offs, constraints — things the next session should not re-derive
- `notes` field reframed as negative knowledge — failed approaches and why

**Remaining gaps**:
- **LLM dependency is non-fatal but silent**: If LM Studio is not running (or the model isn't loaded), summarization fails and the session is recorded without a summary. The failure is logged to stderr (`summarization failed (non-fatal)`) but the user has no indication that summaries are missing — future sessions just have a gap in their context injection. No fallback strategy exists (e.g., template-based summary from observations alone). This is acceptable for a single-developer tool where the user controls LM Studio, but fragile for any distribution scenario.
- **PreCompact summarization**: Long sessions lose signal when Claude Code compacts context. PreCompact hook fires but nmem ignores it. Adding summarization there would delay context injection by ~7.6s while the user is waiting — needs async/background approach.
- **Rolling summaries**: claude-mem summarized at every prompt turn. nmem only summarizes at session end. For long sessions, the end-of-session summary compresses too much.
- **FTS5 indexing of summaries**: Summary content is stored as JSON in `sessions.summary` but not FTS5-indexed. Search via `session_summaries` MCP tool queries by project only, not by content.
- **Engine abstraction**: `summarize.rs` speaks the OpenAI chat completions protocol, which covers LM Studio, Ollama, vLLM, llama.cpp, and OpenAI itself. But the engine is hardcoded — no trait boundary for swapping to non-OpenAI protocols (Anthropic API, in-process model, deterministic template). Abstract when a second engine is needed (rule of three).
- **Dedicated summaries table**: v1 stores in `sessions.summary` column. If rolling/per-prompt summaries are added, a dedicated table with structured fields and FTS5 will be needed.

## S4 — Designed, blocked on platform

### Work unit detection (core S4 algorithm)
Recognize work unit boundaries from the observation stream. A work unit is a bounded chunk of coherent work: intent (prompt) → investigation (reads/searches) → execution (edits/commands) → completion (pattern resets). The signal is the ratio of user prompts : thinking blocks : tool calls, combined with hot file tracking and intent analysis. Detection logic runs on every hook fire as cheap SQL queries over current session observations. LLM summarization runs only at detected boundaries.

**Status**: Designed. Implementation is consumer-independent — same algorithm for Claude Code hooks or API harness.

**Depends on**: Nothing — can start now. S1 observation data is sufficient input.

### Context actuation (S4 actuator)
When S4 detects a work unit boundary, it should be able to clear context and inject the work unit summary plus relevant past summaries. Currently blocked: Claude Code hooks provide full observability but no programmatic context control.

**Claude Code constraints (2026-02-15):**
- No hook can trigger `/clear` or `/compact`
- `additionalContext` injection is [buggy across multiple hook types](https://github.com/anthropics/claude-code/issues/19909)
- Async hook `additionalContext` delivers on next turn but cannot clear existing context
- API context editing (`clear_tool_uses`, `clear_thinking`) exists but isn't exposed to hooks

**Upstream issues tracking this gap:**
- [#24252](https://github.com/anthropics/claude-code/issues/24252) — Context Hooks
- [#19909](https://github.com/anthropics/claude-code/issues/19909) — Lifecycle Hooks for Memory Provider Integration (5 injection bugs)
- [#25689](https://github.com/anthropics/claude-code/issues/25689) — Context usage threshold hook event
- [#21132](https://github.com/anthropics/claude-code/issues/21132) — Claude clear context for itself

**Workaround**: S4 detects boundary → stores summary → signals via async hook → user/agent initiates `/clear` → SessionStart injects curated summaries. Reactive, not autonomous.

**Alternative**: API-based harness bypasses Claude Code entirely. Claude API context editing beta gives full control. Viable path if Claude Code plugin model remains read-only for context.

### Work unit UI (S4 external interface)
User-facing dashboard powered by the same work unit model that drives context management. The UI is S4's external surface:
- Current work unit: intent, hot files, investigate→execute progress
- Work unit history: completed summaries, searchable by intent
- Context health: window utilization, what's been compacted, what nmem would inject on reset

**Depends on**: Work unit detection. UI renders what S4 knows.

## Deferred by design

### S4 Synthesis (cross-session pattern detection)
Periodic synthesis over work unit summaries to produce cross-session patterns. Clusters work units by intent, detects recurring themes, hotspots, and convergence signals.

**Trigger**: Work unit detection implemented AND volume of work unit summaries justifies clustering.

**Depends on**: Work unit summaries as input, `syntheses` table (schema designed in ADR-002 Q3 but not created).

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
`serve.rs` and `search.rs` hardcode type_weight (file_edit=1.0, command=0.67, etc.) for MCP scored retrieval. Not exposed in config. Context injection (`context.rs`) no longer uses type weights — it filters to pinned + recent file_edits + git milestones only, with summaries as primary content.

**Why**: Defaults are reasonable for MCP retrieval. Context injection moved past scoring to explicit filtering.

### Expanded tool classification
`extract.rs` maps tools to obs_types but is coarse (e.g., task_spawn covers TaskCreate/TaskUpdate/TaskList without distinguishing). SendMessage and Skill invocations not captured.

**Why**: ADR-002 chose minimal classification. New types added as capture proves insufficient.

## Won't do (unless evidence changes)

### Per-project databases (ATTACH)
ADR-004 evaluated and rejected (Position B). Single DB with row-level scoping chosen. ATTACH only needed if architecture reverses.

### Cursor-based pagination
Current `limit`/`offset` with cap at 100 results. Volume doesn't warrant keyset pagination (~20K rows/year).
