# ADR-016: Direct Inference — Embedded LLM for Session Summarization

## Status
Accepted (2026-03-14) — Position B: direct inference, HTTP removed

## Decision
Embed a GGUF model via `llama-cpp-2` for session summarization. Remove the LM Studio HTTP dependency entirely. The `ureq` crate remains for VictoriaLogs streaming but is no longer in the summarization path. Use GBNF grammar constraints to guarantee valid JSON output — the `SessionSummary` struct schema becomes the grammar, eliminating parse failures entirely.

## Framing
*Should nmem embed a GGUF model and run inference directly via `llama-cpp-2`, or continue calling an external LM Studio endpoint for session summarization?*

The current summarization architecture depends on LM Studio running at `localhost:1234` when a session ends. The Stop hook spawns a background `nmem maintain --session <id>` process, which calls the LM Studio OpenAI-compatible API to generate a structured JSON summary. This works when LM Studio is up with a model loaded. When it isn't — and there's no retry mechanism — the session goes permanently unsummarized.

The bug surfaced on 2026-03-14: 13 sessions (5 substantive, with 28-86 observations each) had no summaries. These represent gaps in cross-session memory that affect context injection, episode detection, and S3 retention sweeps (which require `summary IS NOT NULL` as a precondition).

The adversarial angles:

**Against direct inference:** "Embedding llama.cpp adds a C++ build dependency, inflates the binary, and couples nmem to a specific inference runtime. LM Studio already solves model management, quantization selection, and GPU offloading. The real bug is missing retry logic, not the inference architecture."

**Against keeping HTTP:** "Every external dependency is a silent failure point. LM Studio must be installed, running, with a model loaded, at the exact moment a session ends. That's four preconditions for a capability that should be intrinsic. The S2 classifiers (ADR-011, ADR-013) already proved the pattern: bring inference in-process, eliminate the dependency."

## Depends On
- ADR-003 (Daemon Lifecycle) — no daemon; maintain runs as a standalone background process
- ADR-011 (Phase Classification) — established the pattern: train offline, infer in-process
- ADR-002 (Observation Extraction) — defines the observation content that feeds summarization

## Unlocks
- Zero-dependency summarization — no LM Studio, no network, no external process
- Catch-up summarization for missed sessions (can run anytime, not just at session end)
- GBNF grammar-constrained generation — structurally valid JSON guaranteed at the token level
- Potential reuse of the inference engine for other S4 capabilities (episode narrative, pattern detection)
- LoRA adapter path for task-specific fine-tuning without model replacement
- Distribution simplicity (ADR-008) — single binary, single model file

---

## Context

### Current architecture

```
Stop hook → spawn `nmem maintain --session <id>`
         → gather_session_payload() (prompts + observations → text)
         → HTTP POST to localhost:1234/v1/chat/completions
         → parse JSON response → store in sessions.summary
```

Dependencies: `ureq` (sync HTTP client), LM Studio running, model loaded (currently `ibm/granite-4-h-tiny`).

The payload is ~2-4 KB of structured text (up to 10 user prompts, 5 agent reasoning blocks, 50 observations). The response is a JSON object with 7 fields (intent, learned, completed, next_steps, files_read, files_edited, notes). Temperature 0.0, max 1024 tokens.

### What the S2 classifiers proved

ADR-011 and ADR-013 solved a similar problem: per-observation classification without external dependencies. The solution was TF-IDF + LinearSVC trained offline, exported as JSON weights, inferred in Rust at sub-millisecond latency. But summarization is fundamentally different — it requires *generation*, not classification. You can't solve "produce a structured summary of 50 observations" with a linear model. This requires a language model.

### The hardware context

Target: Framework 13 AI (AMD Ryzen, DDR5, Fedora 43). The `maintain` process runs in the background after session end, so latency tolerance is high — 10-30 seconds is acceptable. The process loads, infers once, and exits. Memory pressure is the constraint: loading a model shouldn't compete with the user's active workload.

### Model sizing for the task

Session summarization is a focused task: extract intent, list completions, identify next steps from structured observation data. The current model — IBM Granite 4.0 H Tiny — already demonstrates this works in production via LM Studio.

#### Granite 4.0 H Tiny — primary model (validated)

| Parameter | Value |
|-----------|-------|
| Architecture | `granitehybrid` — hybrid attention with per-layer variable KV heads |
| Layers | 40 |
| Embedding dim | 1536 |
| Query heads | 12 |
| KV heads | Variable per layer (array of 40) — some layers full attention, others minimal/linear |
| Vocab size | 100,352 |
| Native context length | 1,048,576 (1M tokens) |
| GGUF file (Q4_K_M) | 4.0 GB |
| Recommended temperature | 0.0 (greedy) |

The hybrid architecture means KV cache costs are lower than standard GQA at the same layer count — layers with minimal KV heads (linear/SSM layers) contribute negligibly to cache. At `n_ctx = 4096`, the KV cache is a small fraction of the 4 GB model weight footprint.

**Production validation**: Granite 4.0 H Tiny has been used for nmem session summarization since 2026-02 via LM Studio, generating 99 successful summaries. The structured JSON output quality is sufficient for context reconstruction. LM Studio used `n_ctx = 131072` (128K) — nmem needs ~2K tokens, so the model is dramatically underutilized on context.

#### Candidate models for future evaluation

Each model needs its own profile — architecture, recommended sampling, and context limits differ:

| Model | GGUF size (Q4_K_M) | Architecture | Attention | Recommended temp | Native n_ctx | Notes |
|-------|-------------------|--------------|-----------|-----------------|-------------|-------|
| Granite 4.0 H Tiny | 4.0 GB | `granitehybrid` | Variable KV/layer | 0.0 | 1M | **Current — validated** |
| Qwen2.5-0.5B-Instruct | ~300 MB | `qwen2` | GQA (2 KV heads) | 0.3-0.7 | 32K | Smallest, may struggle with structured JSON |
| Qwen2.5-1.5B-Instruct | ~900 MB | `qwen2` | GQA (2 KV heads) | 0.3-0.7 | 32K | Strong JSON, fast on CPU |
| Gemma-3-1B-IT | ~600 MB | `gemma2` | GQA + interleaved local/global | 0.7 | 32K | Available in LM Studio |

Model-specific parameters (temperature, n_ctx, n_gpu_layers) live in the task config, not the inference engine. Switching models means updating `[summarization]` config — no code changes.

The output is constrained (structured JSON via GBNF grammar, ~1024 tokens max), and the input is small (~1000-2000 tokens with enrichment). This is the best case for a small model.

---

## Positions Considered

### Position A: Fix the HTTP approach (retry + catch-up)

Keep calling LM Studio over HTTP, but add reliability:
1. Retry logic in `spawn_deferred_maintain` (3 attempts with backoff)
2. A `nmem maintain --catch-up` command that finds unsummarized sessions and processes them
3. Optionally, check on SessionStart if prior sessions are unsummarized and catch up then

**The case for it:**
- Minimal code change. The summarization logic stays in `s1_4_summarize.rs`, just add retry.
- LM Studio handles model management, GPU offloading, quantization — mature tooling.
- No build complexity increase (no C++ compilation, no cmake dependency).
- Can switch models freely via config without rebuilding nmem.

**The case against it:**
- Adds reliability band-aids around a fundamental architectural weakness: an external dependency in the critical path.
- LM Studio must still be installed and running. On headless servers, CI environments, or fresh installs, summarization silently doesn't work.
- The catch-up mechanism is a workaround for not owning the inference capability.
- Four preconditions remain: LM Studio installed, LM Studio running, model loaded, network accessible. Any one failing = silent loss.
- Distribution (ADR-008) becomes "install nmem AND install LM Studio AND download a model AND keep it running" — that's not a single-binary tool.

### Position B: Direct inference via `llama-cpp-2`

Embed `llama-cpp-2` in nmem. The `maintain` process loads a GGUF model, tokenizes the prompt, generates the response, and parses it. No HTTP, no external process.

**The case for it:**
- Eliminates all four external preconditions. Summarization works if the binary and model file exist. Period.
- Follows the pattern established by S2 classifiers: bring intelligence in-process.
- Background process model is ideal — load model, infer once, exit. No need to keep the model in memory.
- The model file ships alongside the binary (or auto-downloads from HuggingFace on first run).
- Catch-up becomes trivial: `nmem maintain --catch-up` can run anytime without external dependencies.
- CPU inference is fine for this use case. From `cpu-inference-performance.md`: a 1.5B Q4_K_M model generates ~20-40 tok/s on modern CPUs. At 1024 max tokens, that's 25-50 seconds worst case — acceptable for background post-session work.

**The case against it:**
- **Build complexity**: `llama-cpp-2` compiles llama.cpp from source via cmake. Build times increase significantly (2-5 minutes for release builds). CI must have cmake and a C++ toolchain.
- **Binary size**: llama.cpp statically linked adds ~10-20 MB to the binary (currently stripped to ~4 MB with `opt-level=z`). This is a 3-5x increase.
- **Model distribution**: The GGUF file must be distributed separately. Auto-download from HuggingFace adds the `hf-hub` crate dependency and requires network on first run.
- **Memory**: Even a 0.5B Q4_K_M model uses ~500 MB runtime. The `maintain` process runs in the background but consumes real RAM while active.
- **API surface churn**: `llama-cpp-2` doesn't follow semver — it tracks upstream llama.cpp closely. Pin exact versions and expect breakage on updates.
- **GPU setup complexity**: ROCm support requires `HSA_OVERRIDE_GFX_VERSION=11.0.0` and the ROCm stack. CPU-only avoids this but leaves performance on the table.
- **Model coupling**: Changing models requires rebuilding if the chat template or tokenization differs. Less flexible than LM Studio's hot-swap.

### Position C: Hybrid — rejected

A hybrid (try direct, fall back to HTTP) was considered and rejected. Two code paths to maintain, unpredictable behavior ("sometimes it calls LM Studio, sometimes it doesn't"), and the HTTP failure mode is only reduced, not eliminated. Breaking changes over legacy hacks.

---

## Open Questions

### Q1: Model selection and distribution
**Partially resolved.** Primary model: Granite 4.0 H Tiny Q4_K_M (4.0 GB, `granitehybrid` architecture). Validated with 99 successful summaries via LM Studio. Continues as the default for direct inference.

Model file location: `~/.nmem/models/` with path configurable via `summarization.model_path`. Open question: auto-download from HuggingFace on first run (adds `hf-hub` dependency) vs manual download with instructions.

### Q2: CPU-only vs GPU support
**Resolved.** GPU is Phase 1 via compile-time feature flags (`cuda`, `rocm`, Metal auto-detected). CPU-only is the default build and always works. GPU builds are produced alongside CPU in the release workflow. Runtime config: `n_gpu_layers = 999` (GPU) or `n_gpu_layers = 0` (CPU). Setting `n_gpu_layers > 0` on a CPU-only build falls back to CPU gracefully.

### Q3: Build profile impact
The current release profile is `opt-level=z` (size-optimized), `lto=true`, `strip=true`. Adding llama.cpp changes the optimization calculus — llama.cpp performance depends heavily on `-march=native` and AVX2/AVX-512, which conflict with size optimization. Should the release profile change, or should llama.cpp compilation use its own optimization flags via cmake?

### Q4: Thread count at inference time
From `cpu-inference-performance.md`: use physical cores only, never logical cores. The maintain process should detect physical core count and set `n_threads` accordingly. But how many cores should a background process claim? All of them (fastest) or a subset (polite to foreground work)?

### Q5: What about the empty session problem?
**Resolved.** Sessions with < 3 observations get a sentinel summary written directly in the Stop hook. No model needed. Unblocks S3 sweep.

### Q6: Chat template handling
**Resolved.** Use `model.chat_template(None)` + `model.apply_chat_template()` to read the template from GGUF metadata. This is model-agnostic — swap the GGUF file, the template follows. No manual formatting, no model-specific code in nmem.

### Q7: Reuse for other S4 capabilities
**Resolved.** Design `s1_4_inference.rs` with a generic `generate()` interface from the start. The function accepts system prompt, user prompt, optional JSON schema (for grammar), optional LoRA path, max tokens, and thread count. Episode narrative and pattern detection can use the same interface when they need generation.

---

## Dependencies

```toml
# New
llama-cpp-2 = { version = "=0.1.133", default-features = false }

# GPU feature flags — compile-time selection
[features]
default = []
cuda = ["llama-cpp-2/cuda"]
rocm = ["llama-cpp-2/rocm"]

# Removed from summarization path (retained for VictoriaLogs streaming only)
ureq = { version = "3", features = ["json"] }
```

Build requirements: cmake, C++ toolchain (g++ or clang++). GPU builds additionally require CUDA Toolkit (NVIDIA) or ROCm (AMD). Metal is auto-detected on macOS.

### CI: two-tier build workflow

| Workflow | Trigger | Matrix | Purpose |
|----------|---------|--------|---------|
| **dev** | Push to main | CPU-only, linux-x86_64 | Fast feedback (~5 min): tests, clippy, build |
| **release** | Tag `v*` | Full platform × GPU matrix | Distribution binaries |

Release matrix:

| Platform | CPU | CUDA | ROCm | Metal |
|----------|-----|------|------|-------|
| linux-x86_64 | ✓ | ✓ | ✓ | — |
| linux-aarch64 | ✓ | ✓ | — | — |
| macOS-arm64 | ✓ | — | — | ✓ |
| windows-x86_64 | ✓ | ✓ | — | — |

Dev builds stay fast. The GPU matrix cost is only paid at release time.

### Config changes

```toml
[summarization]
enabled = true
model_path = "~/.nmem/models/granite-4.0-h-tiny-Q4_K_M.gguf"
temperature = 0.0         # granite recommends 0.0
max_tokens = 1024         # generation limit — summaries are concise
n_ctx = 32768             # fit 98% of sessions untruncated + enrichment
n_threads = 0             # 0 = auto-detect physical cores
n_gpu_layers = 999        # full GPU offload (0 for CPU-only builds)
# lora_path = "~/.nmem/models/summarize-adapter.gguf"  # Phase 3

# Removed: endpoint, model, timeout_secs, fallback_endpoint
```

**Why these values:**
- `temperature = 0.0` — Granite's recommended setting, produces deterministic greedy output
- `max_tokens = 1024` — sufficient for structured JSON summaries, grammar keeps output tight
- `n_ctx = 32768` — measured session data: 74% of sessions < 4K tokens untruncated, 98% < 32K. With enrichment (prior summaries, next_steps), 32K covers all but the largest sessions. Granite's hybrid architecture (variable KV heads per layer) makes the KV cache cost at 32K far lower than a standard transformer. The prior n_ctx=4096 was sized to the *truncated* payload — a constraint inherited from the HTTP path, not the data
- `n_gpu_layers = 999` — GPU prompt eval at 32K tokens: ~1-2 seconds vs ~15-30 seconds on CPU. Since GPU builds are available (CUDA/ROCm/Metal feature flags), default to full offload. Falls back to CPU gracefully if compiled without GPU support

Sampling parameters are per-task, not per-engine. Different models have different optimal settings (Granite: temp=0.0, Qwen2.5: temp=0.3 works better for structured output). The config lives with the task, not the inference module. When a second task needs generation, the pattern repeats — each `[section]` carries its own model path and sampling params. If model profiles emerge as a need (rule of three), extract a `[models.<name>]` table and reference by name. Not before.

## Implementation

### What changes

| File | Change |
|------|--------|
| `s1_4_summarize.rs` | Replace `call_completion()` / `try_endpoint()` with call to `s1_4_inference`. Remove HTTP logic, `strip_fences()`, and `string_or_vec` deserializer (grammar guarantees valid JSON). Remove truncation limits in `gather_session_payload()` and `format_action_line()` — see payload changes below. |
| `s1_4_inference.rs` | **New.** LlamaBackend init, model loading, chat template application, GBNF grammar constraint, generation loop, greedy sampling. |
| `s5_config.rs` | `SummarizationConfig`: replace `endpoint`/`model`/`timeout_secs`/`fallback_endpoint` with `model_path`/`temperature`/`max_tokens`/`n_ctx`/`n_threads`/`n_gpu_layers`/`lora_path` (optional). |
| `s3_maintain.rs` | Add `--catch-up` flag: find sessions with `ended_at IS NOT NULL AND summary IS NULL AND obs_count >= 3`, summarize each. |
| `s1_record.rs` | `handle_stop()`: write sentinel summary for empty sessions (< 3 observations) so they don't block S3 sweep. |
| `schema.rs` | Migration: add `summarization_ms INTEGER` column to `sessions` table. |
| `Cargo.toml` | Add `llama-cpp-2`. |

### Architecture

```
Stop hook → spawn `nmem maintain --session <id>`
         → gather_session_payload() (all prompts, all observations, full content — no truncation)
         → build InferenceParams from [summarization] config
         → generate(params, system_prompt, user_prompt):
             → init LlamaBackend
             → load GGUF model (mmap, n_gpu_layers from params)
             → create context (n_ctx=32768, n_threads from params)
             → apply chat template from GGUF metadata
             → build GBNF grammar from SessionSummary JSON schema
             → build sampler chain (grammar + temp from params)
             → decode prompt (GPU-accelerated if available), generate up to max_tokens
             → output is guaranteed valid JSON
         → deserialize directly → store in sessions.summary
         → exit (model dropped, memory freed)
```

### Payload: truncation removal

The HTTP path imposed truncation at two levels — payload construction (`gather_session_payload`) and display formatting (`format_action_line`). With `n_ctx = 32768` and no HTTP timeout pressure, these caps are removed.

#### `gather_session_payload()` — before → after

| Data | Before (HTTP) | After (direct) |
|------|--------------|----------------|
| User prompts | 10, truncated to 100 chars | All prompts, full content |
| Agent reasoning | 5, truncated to 200 chars | All reasoning blocks, full content |
| Observations | 50 most recent | All observations |

#### `format_action_line()` — before → after

| Data | Before (HTTP) | After (direct) |
|------|--------------|----------------|
| Content preview (with file_path) | 60 chars | Full content (up to storage cap of 2000 chars) |
| Content preview (no file_path) | 80 chars | Full content |
| Error response (`PostToolUseFailure`) | 120 chars from metadata | Full 2000-char error response from metadata |

The error response change is particularly valuable. Errors are the one case where `tool_response` is stored — `s1_record.rs` captures up to 2000 chars of the failure response in the `metadata` JSON column. But `format_action_line()` truncates this again to 120 chars for the summarization payload. The model sees `FAILED: error[E0308]: mismatched types` but not the full compiler output that explains *why*. Removing the 120-char cap lets the summarizer produce better `notes` and `learned` entries from failure context.

Note: successful `tool_response` content (command output, file contents, search results) is **not stored** in observations — that's an ADR-002 decision to keep storage small. Only errors and git metadata capture the response. Enriching summaries with successful tool responses would require querying the transcript at summarization time, which is a separate scope.

#### Overflow handling

For the 2% of sessions exceeding 32K tokens (the largest observed: 62K tokens), truncate the **oldest observations first** — most recent observations are more relevant for summary quality. The system prompt, user prompts, and agent reasoning are never truncated.

### GBNF grammar-constrained generation

The key capability that direct inference enables over HTTP. `llama-cpp-2` provides `json_schema_to_grammar()` which converts a JSON schema into a GBNF grammar string. The grammar sampler filters the token probability distribution at each step — the model literally cannot produce tokens that violate the schema.

The `SessionSummary` JSON schema:

```json
{
  "type": "object",
  "properties": {
    "intent": { "type": "string" },
    "learned": { "type": "array", "items": { "type": "string" } },
    "completed": { "type": "array", "items": { "type": "string" } },
    "next_steps": { "type": "array", "items": { "type": "string" } },
    "files_read": { "type": "array", "items": { "type": "string" } },
    "files_edited": { "type": "array", "items": { "type": "string" } },
    "notes": {}
  },
  "required": ["intent", "learned", "completed", "next_steps", "files_read", "files_edited"]
}
```

This schema becomes the grammar constraint. The sampler chain adapts to temperature:

```rust
let grammar_str = json_schema_to_grammar(SUMMARY_SCHEMA)?;
let sampler = if params.temperature == 0.0 {
    // Greedy — deterministic (Granite, or any model that recommends temp=0)
    LlamaSampler::chain_simple([
        LlamaSampler::grammar(&model, &grammar_str, "root")?,
        LlamaSampler::greedy(),
    ])
} else {
    // Sampling — for models that benefit from temperature (Qwen, Phi)
    LlamaSampler::chain_simple([
        LlamaSampler::grammar(&model, &grammar_str, "root")?,
        LlamaSampler::temp(params.temperature),
        LlamaSampler::dist(42),  // fixed seed for reproducibility
    ])
};
```

**What this eliminates:**
- `strip_fences()` — model can't emit code fences, grammar only allows JSON
- `string_or_vec` deserializer — grammar enforces arrays where arrays are required
- Parse failures from malformed JSON — structurally impossible
- Missing required fields — grammar enforces all required keys
- Retry loops for bad output — every generation is valid

The prompt still describes the desired content (what "intent" means, what "learned" should contain). The grammar enforces the *structure*; the prompt guides the *content*.

### Empty session resolution

Sessions with < 3 observations get a sentinel summary written directly in the Stop hook (no model needed):

```json
{"intent": "empty session", "learned": [], "completed": [], "next_steps": [], "files_read": [], "files_edited": [], "notes": null}
```

This unblocks S3 sweep for trivial sessions without wasting inference.

### Catch-up

```bash
nmem maintain --catch-up   # summarize all missed sessions
```

Finds sessions where `ended_at IS NOT NULL AND summary IS NULL` and has >= 3 observations. Loads the model once, processes all sessions sequentially. Can be run manually or via systemd timer.

### Summarization timing

No timing is recorded today. With direct inference — where GPU vs CPU, model choice, payload size, and `n_ctx` all affect latency — timing becomes essential for validation and regression detection.

**Schema change**: add `summarization_ms INTEGER` to `sessions` table (nullable, migration 12). Populated by `summarize_session()` after inference completes.

**What's recorded**: total wall-clock time for `generate()` — covers model load (mmap), prompt eval, and generation. Logged to stderr and stored in the DB:

```
nmem: session summarized (1247ms, 832 prompt tokens, 412 generated tokens)
```

**What this enables**:
- Validate GPU vs CPU: is `n_gpu_layers = 999` actually faster for this session size?
- Detect model regressions when updating the pinned `llama-cpp-2` version
- Track payload size impact: do untruncated payloads slow down summarization?
- `nmem status` can report average summarization time
- Grafana dashboard metric via VictoriaLogs streaming (add `summarization_ms` to the log entry)

### Module placement

`s1_4_inference.rs` sits in S1's S4 (intelligence within operations), alongside `s1_4_summarize.rs`. It is not `s2_inference.rs` — that module handles TF-IDF/LinearSVC statistical inference. This module handles generative LLM inference. Different tools, different VSM layer purpose, same naming pattern.

The module exposes a params struct and a single entry point:

```rust
/// All inference parameters — constructed by each task from its config section.
/// Model-specific settings (temperature, n_ctx, n_gpu_layers) live here,
/// not hardcoded in the engine.
pub struct InferenceParams {
    pub model_path: PathBuf,
    pub temperature: f32,          // 0.0 = greedy, >0 = sampling
    pub n_ctx: u32,                // context window size
    pub max_tokens: u32,           // generation limit
    pub n_threads: u32,            // 0 = auto-detect physical cores
    pub n_gpu_layers: u32,         // 0 = CPU only, 999 = full offload
    pub json_schema: Option<String>, // GBNF grammar from JSON schema
    pub lora_path: Option<PathBuf>,  // optional LoRA adapter
}

pub fn generate(
    params: &InferenceParams,
    system_prompt: &str,
    user_prompt: &str,
) -> Result<String, NmemError>
```

Each task constructs `InferenceParams` from its own config section. Summarization builds it from `[summarization]`. When episode narrative needs generation, it builds from `[narrative]` with its own model path and temperature. The inference engine doesn't know or care which task is calling — it takes params and returns text.

This is the "flat now, profiles later" approach: each task section carries its own model path and sampling params inline. If the same model appears in three task sections with identical params, that's the signal to extract `[models.<name>]` profiles and reference by name. Not before (rule of three).

---

## Phased Implementation

### Phase 1 — Replace HTTP with direct inference (the ADR-016 implementation)

Core lifecycle + GBNF grammar + GPU support + full session payload + chat templates + catch-up + empty session sentinel.

| Component | Detail |
|-----------|--------|
| Inference engine | `s1_4_inference.rs`: Backend → Model (with GPU layers) → Context → Batch → Grammar Sampler → generate loop |
| Grammar constraint | `json_schema_to_grammar()` from `SessionSummary` schema — day-one, not an optimization |
| Chat template | Read from GGUF metadata via `model.chat_template()` — model-agnostic |
| Sampling | Config-driven: greedy at temp=0.0 (Granite), temp sampling for other models. Grammar constraint applied regardless |
| Params struct | `InferenceParams` — model-agnostic, constructed per-task from config. Flat config now, profiles at rule-of-three |
| GPU support | Compile-time feature flags (`cuda`, `rocm`). `n_gpu_layers` in config. CPU-only default build, GPU in release matrix |
| Full payload | Remove truncation limits in `gather_session_payload()` — all prompts, all observations, full content. `n_ctx = 32768` covers 98% of sessions untruncated |
| Empty sessions | Sentinel summary in Stop hook for < 3 observations |
| Catch-up | `nmem maintain --catch-up` — batch process missed sessions, load model once |
| Config | `model_path`, `temperature`, `max_tokens`, `n_ctx`, `n_threads`, `n_gpu_layers` |
| HTTP removal | Delete `call_completion()`, `try_endpoint()`, `strip_fences()`, `string_or_vec` |
| CI | Two-tier: dev (CPU-only, fast) + release (full platform × GPU matrix) |

**Validates**: model quality, inference speed (CPU + GPU), memory footprint, build integration, full-payload summary quality vs truncated.

### Phase 2 — Optimize resource usage

After Phase 1 is validated in production use.

| Component | Detail |
|-----------|--------|
| KV cache quantization | `q8_0` cache — ~50% memory reduction, minimal quality impact. Reduces 1.5B runtime from ~1.2 GB to ~600 MB |
| Multi-sequence batching | `LlamaBatch::new(512, n)` — batch multiple sessions in one model load during `--catch-up`. One mmap load, N inferences |
| Thread tuning | Auto-detect physical cores, use half for background politeness (configurable via `n_threads`) |

### Phase 3 — Fine-tuning and extended capabilities

When summary quality needs improvement or new S4 features need generation.

| Component | Detail |
|-----------|--------|
| LoRA adapters | `summarization.lora_path` in config. Fine-tune adapter for nmem's specific summary format without replacing base model. Small file (~10-50 MB), applied at inference time, preserves mmap base model |
| LoRA training pipeline | Collect low-quality summaries → label corrections → QLoRA fine-tune → export GGUF adapter → deploy via config |
| Embeddings | `LlamaContextParams::with_embeddings(true)` — semantic search for nmem retrieval. Requires embedding-capable model, changes process model (model must be loaded for search, not just session end). Separate evaluation needed |
| Session save/load | Cache system prompt KV state to disk. Amortize prompt encoding across multiple summarizations in `--catch-up` |

### API churn mitigation

`llama-cpp-2` releases every ~9 days, does not follow semver. Empirical analysis (0.1.133 → 0.1.138, 5 releases) shows the core API (Backend/Model/Context/Batch/Sampler) is stable — churn is additive features (`llguidance`, `mtmd`, session save/load), not breaking changes to the inference lifecycle.

Mitigation:
1. **Pin exact version**: `"=0.1.133"` — update on our schedule, not theirs
2. **Minimal API surface**: Use only core lifecycle + grammar + chat template. No advanced features in Phase 1
3. **Thin wrapper**: `s1_4_inference.rs` exposes `generate()`. If the underlying API shifts, one file changes
4. **Update deliberately**: Bump pin, test, commit. Not reactive to upstream releases
