# ADR-016: Direct Inference — Embedded LLM for Session Summarization

## Status
Accepted (2026-03-14) — Position B: direct inference, HTTP removed

## Decision
Embed a GGUF model via `llama-cpp-2` for session summarization. Remove the LM Studio HTTP dependency entirely. The `ureq` crate remains for VictoriaLogs streaming but is no longer in the summarization path.

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
- Potential reuse of the inference engine for other S4 capabilities (episode narrative, pattern detection)
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

Session summarization is a focused task: extract intent, list completions, identify next steps from structured observation data. This is well within the capability of sub-3B models. The current `ibm/granite-4-h-tiny` already demonstrates this — it's a ~1B model.

From `small-language-models.md`:
- 0.5B-1B models handle "classification, extraction, lightweight summarization" (4-6 GB RAM)
- Qwen2.5-0.5B Q4_K_M: ~300 MB on disk, ~500 MB runtime
- Qwen2.5-1.5B Q4_K_M: ~900 MB on disk, ~1.2 GB runtime

The output is constrained (structured JSON, ~1024 tokens max), and the input is small (2-4 KB). This is the best case for a small model.

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
Which GGUF model for session summarization? Candidates:
- Qwen2.5-0.5B-Instruct Q4_K_M (~300 MB) — smallest, fastest, may struggle with structured JSON output
- Qwen2.5-1.5B-Instruct Q4_K_M (~900 MB) — strong JSON capability, still fast on CPU
- Granite-3.1-1B Q4_K_M — current model family, known to work for this task

Where does the model file live? Options:
- `~/.nmem/models/` (auto-created, auto-downloaded on first run)
- Config path: `summarization.model_path = "/path/to/model.gguf"`
- Bundled in the release archive alongside the binary

### Q2: CPU-only vs GPU support
The maintain process runs once per session (background). CPU inference at 20-40 tok/s for a 1.5B model is 25-50 seconds worst case. Is that acceptable, or should GPU offloading (`rocm` feature flag) be supported?

GPU adds: ROCm build complexity, `HSA_OVERRIDE_GFX_VERSION` env requirement, feature flag management. CPU-only keeps the build simple and the binary portable.

### Q3: Build profile impact
The current release profile is `opt-level=z` (size-optimized), `lto=true`, `strip=true`. Adding llama.cpp changes the optimization calculus — llama.cpp performance depends heavily on `-march=native` and AVX2/AVX-512, which conflict with size optimization. Should the release profile change, or should llama.cpp compilation use its own optimization flags via cmake?

### Q4: Thread count at inference time
From `cpu-inference-performance.md`: use physical cores only, never logical cores. The maintain process should detect physical core count and set `n_threads` accordingly. But how many cores should a background process claim? All of them (fastest) or a subset (polite to foreground work)?

### Q5: What about the empty session problem?
Sessions with 0-2 observations return `None` from `gather_session_payload` and never get summarized. Independent of the inference backend, should these sessions get a sentinel summary (e.g., `{"intent": "empty session", ...}`) so they don't block S3 sweep logic?

### Q6: Chat template handling
Different models use different chat templates (ChatML, Llama, custom). `llama-cpp-2` supports reading the template from GGUF metadata (`tokenizer.chat_template`). Should nmem apply the chat template from the GGUF, or format the prompt manually? Manual formatting couples the code to specific models. GGUF templates couple to the model file being correct.

### Q7: Reuse for other S4 capabilities
If direct inference is embedded for summarization, the same engine could serve episode narrative generation (`s4_memory.rs`), cross-session pattern detection (`s3_learn.rs`), and future S4 intelligence features. Should the inference engine be designed as a shared module from the start, or kept scoped to summarization and generalized later?

---

## Dependencies

```toml
# New
llama-cpp-2 = { version = "=0.1.133", default-features = false }

# Removed from summarization path (retained for VictoriaLogs streaming only)
ureq = { version = "3", features = ["json"] }
```

Build requirements: cmake, C++ toolchain (g++ or clang++).

### Config changes

```toml
[summarization]
enabled = true
model_path = "~/.nmem/models/qwen2.5-1.5b-instruct-q4_k_m.gguf"
max_tokens = 1024
n_threads = 0  # 0 = auto-detect physical cores

# Removed: endpoint, model, timeout_secs, fallback_endpoint
```

## Implementation

### What changes

| File | Change |
|------|--------|
| `s1_4_summarize.rs` | Replace `call_completion()` / `try_endpoint()` with `call_direct_inference()`. Remove HTTP logic. `strip_fences()` retained (model may still emit fences). |
| `s1_4_inference.rs` | **New.** LlamaBackend init, model loading, chat template application, generation loop, greedy sampling. |
| `s5_config.rs` | `SummarizationConfig`: replace `endpoint`/`model`/`timeout_secs`/`fallback_endpoint` with `model_path`/`max_tokens`/`n_threads`. |
| `s3_maintain.rs` | Add `--catch-up` flag: find sessions with `ended_at IS NOT NULL AND summary IS NULL AND obs_count >= 3`, summarize each. |
| `s1_record.rs` | `handle_stop()`: write sentinel summary for empty sessions (< 3 observations) so they don't block S3 sweep. |
| `Cargo.toml` | Add `llama-cpp-2`. |

### Architecture

```
Stop hook → spawn `nmem maintain --session <id>`
         → gather_session_payload() [unchanged]
         → init LlamaBackend
         → load GGUF model (mmap, fast startup)
         → apply chat template from GGUF metadata
         → decode prompt, generate up to max_tokens (greedy, temp=0.0)
         → parse JSON response → store in sessions.summary
         → exit (model dropped, memory freed)
```

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

### Module placement

`s1_4_inference.rs` sits in S1's S4 (intelligence within operations), alongside `s1_4_summarize.rs`. It is not `s2_inference.rs` — that module handles TF-IDF/LinearSVC statistical inference. This module handles generative LLM inference. Different tools, different VSM layer purpose, same naming pattern.

If other S4 capabilities later need generation (episode narrative, pattern detection), `s1_4_inference.rs` becomes the shared engine. Design it with a clean `generate(model, system_prompt, user_prompt, max_tokens) → String` interface from the start.
