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

## S4 — Partial (task dispatch functional, work unit detection designed)

### Task dispatch — functional
Task queue with systemd-driven dispatch (`s4_dispatch.rs`). Queues work via CLI (`nmem queue`) or MCP tool (`queue_task`), dispatches into tmux panes running Claude Code on a 60s heartbeat (`nmem dispatch` via systemd timer). Each dispatched session is its own viable system — S4 initiates, the session operates autonomously.

**Remaining gaps:**
- **Task result capture**: No way to know what a dispatched session produced. The task is marked "completed" when its tmux pane exits, but the outcome (success/failure, what was changed) isn't captured. Linking a task to the session it spawned (via session_id) would close this loop.
- **Task cancellation**: No `cancel` subcommand or MCP tool. Running tasks can only be stopped by manually killing the tmux pane.
- **Task dependencies/chaining**: Tasks are independent. No way to express "run B after A completes" or "run B only if A succeeded."
- **Completion notification**: No signal when a dispatched task finishes. The user must check manually or wait for the next session's context injection.
- **Task listing**: No `nmem tasks` CLI to show pending/running/completed tasks. Currently requires direct SQL.

### Work unit detection (core S4 algorithm — inward-facing)
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

### New classifier training (locus, novelty, friction)
Three new classifier dimensions added (2026-02-22): locus (internal/external), novelty (routine/novel), friction (smooth/friction). Infrastructure wired — shared `s2_inference.rs` engine, schema migration 10, hook pipeline, backfill CLI — but models need training.

**Training workflow:**
1. Generate heuristic-labeled corpus: `python3 tools/classify-label-heuristic.py --dimension <dim> --output tools/corpus-<dim>.json`
2. Train: `python3 tools/classify-train.py --corpus tools/corpus-<dim>.json --output models/<labels>.json`
3. Backfill: `nmem backfill --dimension <dim>`

**Trigger**: Enough observations in the DB (500+) to generate meaningful training data. Heuristic labels serve as bootstrap; agent-augmented paraphrases can improve accuracy as with think/act.

### Scope classifier augmentation strategy
The converge/diverge scope classifier (ADR-013) achieves 71.4% CV on real data — functional but below the 80% floor that think/act meets. The bottleneck is augmentation quality: word-dropout transforms don't add decision-boundary signal, so 5478 augmented entries perform no better than 870 base entries on held-out data. Think/act reached 98.8% because 10x LLM-generated paraphrases added genuine semantic variety.

**Strategies to investigate:**
1. **LLM-generated paraphrases** — the think/act pipeline used parallel agents to generate 10 paraphrases per entry. The scope augmentation agents were launched but timed out generating Python scripts. Retry with more constrained prompts (output JSON directly, no scripts).
2. **Feature engineering** — include `obs_type` as an explicit feature alongside text. `file_edit` is strongly converge-biased, `web_search` is diverge-biased. A combined text+metadata model may capture what text alone misses.
3. **Sequential context** — pair each observation's text with the previous observation's text or obs_type. Converge/diverge depends on what happened before (re-reading a file = converge; first read = diverge).
4. **Larger base corpus** — data volume was the primary lever for think/act (176→7648). Current scope base is 870 entries. Extract and label 2000+ observations from the growing DB.
5. **Active learning** — classify with the current model, surface low-confidence predictions for manual correction, retrain.

**Trigger**: Scope model accuracy demonstrably affects episode quality or context injection decisions. Current 71% may be sufficient at the episode aggregation level.

## S3 — Semi-autonomous (baseline achieved)

Retention sweeps now run automatically at session end (Stop hook), after summarization and before WAL checkpoint. Enabled by default — no config needed. Two triggers: count-based (>100 expired observations older than 1 day) and size-based (`max_db_size_mb` in config, checks DB + WAL).

**Remaining S3 gaps:**
- **Compaction scheduling** — vacuum and FTS rebuild are manual only (`nmem maintain`). No idle-period detection.
- **Anomaly escalation** — if sweeps can't reclaim enough or writes fail, nothing escalates. Silent degradation.
- **Sweep audit** — deletions logged to stderr only. No persistent record of what was swept when.

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
