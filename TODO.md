# nmem — TODO

Missing features, why they're missing, and what triggers implementation.

## Deferred by design

### S4 Synthesis (LLM-based summarization)
Periodic LLM synthesis over observation clusters to produce cross-session patterns. ADR-002 chose structured extraction (Position A) as the starting point — LLM synthesis is explicitly gated on evidence that retrieval quality is poor without it.

**Trigger**: Retrieval consistently misses contextually relevant results across sessions; volume exceeds ~10K observations where abstraction adds value.

**Depends on**: Mature S1 capture, `syntheses` table (schema designed in ADR-002 Q3 but not created).

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
