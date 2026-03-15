# nmem

Cross-session memory for AI coding agents. nmem observes what an agent does — files read, edits made, commands run, searches performed — classifies each observation along five cognitive dimensions, and makes the full history available to future sessions via MCP tools and CLI. Sessions build on what came before.

nmem is private by default: all data lives in an encrypted SQLite database on your machine. Secrets are redacted before storage. No data leaves your system unless you explicitly configure external services.

## How it works

```
┌─────────────────────────────────────────────────────────────┐
│ Claude Code session                                         │
│                                                             │
│  hooks fire on every tool call ──► nmem record              │
│                                      │                      │
│                                      ▼                      │
│                              classify (5 dims)              │
│                              filter secrets                 │
│                              store observation              │
│                                                             │
│  session start ──────────────► nmem serve (MCP)             │
│                                  │                          │
│                                  ▼                          │
│                          inject prior context               │
│                          answer retrieval queries            │
│                                                             │
│  session end ────────────► nmem maintain                    │
│                              │                              │
│                              ▼                              │
│                       detect episodes                       │
│                       summarize (local LLM)                 │
│                       retention sweep                       │
└─────────────────────────────────────────────────────────────┘
```

**No daemon, no cloud, no API keys.** nmem runs as three short-lived process types:

- **Hook handler** (`nmem record`) — one process per hook event, reads JSON from stdin, classifies and stores the observation
- **MCP server** (`nmem serve`) — session-scoped subprocess, answers retrieval queries from the agent
- **Maintenance** (`nmem maintain`) — runs at session end and via systemd timer, handles summarization, episode detection, and retention sweeps

## What gets captured

Every tool call the agent makes becomes a typed observation:

| obs_type | Source tool | Signal |
|----------|------------|--------|
| `file_read` | Read | Investigation |
| `file_write` | Write | Execution |
| `file_edit` | Edit | Execution |
| `search` | Grep, Glob | Investigation |
| `command` | Bash (generic) | Varies |
| `git_commit` | git commit | Completion |
| `git_push` | git push | Completion |
| `github` | gh CLI | External interaction |
| `task_spawn` | Task | Delegation |
| `web_fetch` | WebFetch | Research |
| `web_search` | WebSearch | Research |
| `mcp_call` | MCP tools | External tool |

## Five classifier dimensions

Every observation is classified at write time on four dimensions using TF-IDF + LinearSVC models in pure Rust (sub-millisecond inference, no external dependencies). A fifth dimension is applied per-episode at session end.

| Dimension | Values | Signal |
|-----------|--------|--------|
| **Phase** | think / act | Reasoning vs. executing |
| **Scope** | converge / diverge | Narrowing toward solution vs. broadening investigation |
| **Locus** | internal / external | Within the project vs. reaching outside |
| **Novelty** | routine / novel | Familiar operation vs. new territory |
| **Friction** | smooth / friction | Clean progress vs. encountering resistance (episode-level) |

Phase and scope form four stance quadrants that characterize the agent's cognitive rhythm:

| Stance | Character |
|--------|-----------|
| think + diverge | Exploring, investigating |
| think + converge | Reasoning toward a solution |
| act + diverge | Executing exploratory work |
| act + converge | Executing toward completion |

## Session lifecycle

At **session start**, nmem injects context from prior work: recent episodes (within a configurable window), fallback session summaries for older sessions, and suggested next steps. The agent starts with continuity rather than a blank slate.

During the session, every tool call is captured, classified, and stored. The MCP server answers targeted retrieval queries — full-text search, file history, session traces, stance analysis.

At **session end**, nmem detects episode boundaries (coherent units of work within the session), generates a structured JSON summary using an embedded GGUF language model, and runs retention sweeps. The summary captures intent, decisions made, work completed, and logical next steps — optimized for the next AI session to reconstruct context, not for human reading.

## Episodic memory

nmem detects **episodes** — bounded chunks of coherent work within a session. An episode begins when the user's intent shifts (detected via Jaccard similarity on prompt keyword bags) and includes all observations until the next shift.

Each episode is annotated with:
- Hot files (most-touched paths)
- Phase signature (think/act distribution)
- Stance character (5 classifier dimensions)
- LLM-generated narrative
- Observation trace rollup (compact fingerprints for each observation)

Episodes are the bridge between raw observations (too granular) and session summaries (too compressed). They feed context injection for recent sessions and enable safe forgetting — once an episode's observation trace is frozen, S3 can sweep the raw observations.

## MCP tools

These tools are available to the agent during a session:

| Tool | Purpose |
|------|---------|
| `search` | Full-text search over observations (FTS5: AND/OR/NOT, "phrases", prefix*) |
| `get_observations` | Fetch full observation details by ID |
| `recent_context` | Recent observations ranked by recency + type weight + project match |
| `session_summaries` | Structured summaries of past sessions (intent, learned, completed, next_steps) |
| `timeline` | Observations surrounding an anchor point within the same session |
| `file_history` | Trace a file's history across sessions with intent context |
| `session_trace` | Step-by-step session replay |
| `current_stance` | Current session's EMA-smoothed cognitive trajectory with retrieval guidance |
| `queue_task` | Queue a task for later dispatch into a tmux Claude Code session |
| `create_marker` | Record a decision or conclusion as a durable observation |
| `regenerate_context` | Re-run context injection with current data |

## CLI

```
nmem status              # DB health: size, counts, last session
nmem search <query>      # FTS5 search with BM25 + recency ranking
nmem context             # Preview what would be injected at session start
nmem maintain            # Vacuum, WAL checkpoint, FTS integrity
nmem maintain --sweep    # Run retention sweep
nmem maintain --catch-up # Summarize missed sessions
nmem purge               # Targeted deletion (by date, project, session, type, search)
nmem pin <id>            # Exempt an observation from retention sweeps
nmem unpin <id>          # Restore normal retention
nmem learn               # Cross-session pattern detection report
nmem queue <prompt>      # Queue a task for later dispatch
nmem dispatch            # Check for pending tasks, dispatch to tmux
nmem backfill            # Retroactively classify historical observations
nmem mark <text>         # Create an agent-authored marker observation
```

## Architecture

nmem is organized around the [Viable System Model](https://en.wikipedia.org/wiki/Viable_system_model) (VSM). Every module maps to a VSM system:

| System | Role | Key modules |
|--------|------|-------------|
| **S1** Operations | Capture, store, retrieve | `s1_record`, `s1_serve`, `s1_search`, `s1_extract` |
| **S1's S4** | Session summarization (VSM recursion) | `s1_4_summarize`, `s1_4_inference` |
| **S2** Coordination | Dedup, ordering, classification | `s2_inference`, `s2_classify`, `s2_scope`, `s2_locus`, `s2_novelty` |
| **S3** Control | Storage budgets, retention, compaction | `s3_sweep`, `s3_maintain`, `s3_purge` |
| **S4** Intelligence | Context injection, task dispatch, episodic memory, patterns | `s4_context`, `s4_dispatch`, `s4_memory`, `s3_learn` |
| **S5** Policy | Config, identity, boundaries | `s5_config`, `s5_filter`, `s5_project` |

Session summarization uses an embedded GGUF model via [llama-cpp-2](https://github.com/utilityai/llama-cpp-rs) — no external LLM service required. The default model ([granite-4.0-h-tiny](https://huggingface.co/lmstudio-community/granite-4.0-h-tiny-GGUF)) auto-downloads from HuggingFace on first summarization and is cached locally. GPU acceleration is supported via feature flags (`--features cuda` or `--features rocm`).

## Storage

Encrypted SQLite (SQLCipher) at `~/.nmem/nmem.db`. An encryption key is auto-generated at `~/.nmem/key` on first run.

Five tables: `sessions`, `prompts`, `observations`, `tasks`, `work_units`, plus FTS5 indexes and a `classifier_runs` audit table. Schema versioned via `rusqlite_migration` (12 migrations).

Secret filtering runs before storage: regex patterns for common API key formats plus Shannon entropy detection for high-entropy strings. Per-project sensitivity levels (strict/relaxed) tune the filtering threshold.

## Draft features

These are designed but not yet implemented:

- **Autonomous context management** — S4 injects context mid-session when it detects a work unit boundary, rather than only at session start. Blocked on Claude Code platform evolution ([upstream issues](https://github.com/anthropics/claude-code/issues/19909)).
- **Cross-project retention** — per-project retention policies. Blocked on multipass bootstrap.
- **Multi-agent coordination** — shared memory, cross-agent retrieval. Requires networking layer.
- **PreCompact capture** — long sessions lose signal when Claude Code compacts context. Rolling summaries would preserve continuity.

## Install

### Build from source (recommended)

Requires Rust 1.85+, cmake, and a C++ compiler (for llama.cpp).

```sh
# CPU-only (works everywhere)
cargo install --path .

# With NVIDIA GPU support
cargo install --path . --features cuda

# With AMD GPU support (ROCm)
cargo install --path . --features rocm
```

The build bundles SQLCipher — no system SQLite dependency needed. First build takes 2-3 minutes (llama.cpp compiles from C++ source); subsequent builds are incremental.

### Configure Claude Code hooks

Add to your Claude Code settings (`.claude/settings.json`):

```json
{
  "hooks": {
    "PostToolUse": [{ "command": "nmem record", "timeout": 5000 }],
    "Stop": [{ "command": "nmem record", "timeout": 30000 }],
    "SessionStart": [{ "command": "nmem record", "timeout": 5000 }],
    "UserPromptSubmit": [{ "command": "nmem record", "timeout": 5000 }]
  }
}
```

Add MCP server (`.claude/mcp.json`):

```json
{
  "mcpServers": {
    "nmem": {
      "type": "stdio",
      "command": "nmem",
      "args": ["serve"]
    }
  }
}
```

### Verify

```sh
nmem status
```

## Configuration

`~/.nmem/config.toml` — all sections are optional, defaults shown below.

```toml
# --- Project detection ---
[project]
strategy = "git"              # "git" (repo root basename) or "cwd" (directory basename)

# --- Secret filtering ---
[filter]
extra_patterns = []           # additional regex patterns to redact
# entropy_threshold = 4.0     # Shannon entropy threshold (default 4.0)
# entropy_min_length = 20     # minimum string length for entropy check (default 20)
# disable_entropy = false     # disable entropy-based detection entirely

# --- Per-project overrides ---
# [projects.my-secret-app]
# sensitivity = "strict"      # "default", "strict" (lower entropy threshold), or "relaxed" (no entropy)
# context_local_limit = 20    # max local-project observations in context injection
# context_cross_limit = 10    # max cross-project observations in context injection
# suppress_cross_project = false  # hide cross-project observations entirely
# context_episode_window_hours = 48  # episode window for context injection

# --- Encryption ---
[encryption]
# key_file = "~/.nmem/key"   # auto-generated if absent

# --- Retention (enabled by default) ---
[retention]
enabled = true
# max_db_size_mb = 500        # optional size-based sweep trigger

[retention.days]
git_commit = 730              # 2 years — high-value completion signals
git_push = 730
file_write = 365
file_edit = 365
session_startup = 365
session_compact = 365
session_resume = 365
session_clear = 365
command = 180
github = 180
file_read = 90                # high-volume investigation — shorter retention
search = 90
mcp_call = 90
web_fetch = 90
web_search = 90
task_spawn = 90
tool_other = 90

# --- Session summarization (embedded GGUF model) ---
[summarization]
enabled = true
model_path = "lmstudio-community/granite-4.0-h-tiny-GGUF:granite-4.0-h-tiny-Q4_K_M.gguf"
temperature = 0.0             # greedy decoding
max_tokens = 1024
n_ctx = 32768                 # context window (covers 98% of sessions untruncated)
n_threads = 0                 # 0 = auto-detect (half of available cores)
n_gpu_layers = 999            # 999 = offload all layers to GPU (ignored without cuda/rocm feature)
# lora_path = "/path/to/adapter.gguf"  # optional LoRA adapter

# --- Metrics (optional) ---
# [metrics]
# endpoint = "http://localhost:8428/opentelemetry/api/v1/push"
```

## Design

Architecture docs in [`design/`](design/):
- [DESIGN.md](design/DESIGN.md) — overall framing
- [VSM.md](design/VSM.md) — Viable System Model mapping and roadmap
- [ADR/](design/ADR/) — architectural decision records

## License

MIT
