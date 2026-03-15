# nmem

Cross-session memory for AI coding agents.

AI coding agents start every session from scratch. They re-read files they read yesterday, re-derive decisions they already made, and repeat mistakes they already fixed. nmem gives agents continuity: it observes what the agent does, classifies each action along five cognitive dimensions, detects coherent episodes of work, and feeds everything back into the next session. Sessions build on what came before.

## The problem

A coding agent's context window is its only memory. When a session ends, everything learned — which files matter, what was tried and abandoned, what decisions were made and why — is gone. The next session starts cold. On long-running projects, agents routinely spend 20-30% of a session re-establishing context that a prior session already had.

This isn't a retrieval problem (the code is right there). It's a *cognitive continuity* problem: knowing what you were doing, why, and what to do next.

## How nmem solves it

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

**No daemon, no cloud, no API keys.** Three short-lived process types:

- **Hook handler** (`nmem record`) — one process per hook event, captures and classifies the observation
- **MCP server** (`nmem serve`) — session-scoped subprocess, answers retrieval queries from the agent
- **Maintenance** (`nmem maintain`) — runs at session end and via systemd timer, handles summarization, episode detection, and retention

Everything runs locally. The only network calls are optional (metrics export, log streaming) and disabled by default.

## Key capabilities

### Session continuity

At session start, nmem injects context from prior work: recent episodes of work (within a configurable window), session summaries for older sessions, and suggested next steps derived from where the last session left off. The agent starts with orientation rather than a blank slate.

At session end, nmem generates a structured summary using an embedded GGUF language model — no external LLM service required. The summary captures intent, decisions made, work completed, and logical next steps. Summaries are optimized for the next AI session to reconstruct context, not for human reading.

### Cognitive classification

Every observation is classified at write time on four dimensions using TF-IDF + LinearSVC models running in pure Rust (sub-millisecond inference, no external dependencies, 98.8% cross-validated accuracy). A fifth dimension is applied per-episode at session end.

| Dimension | Values | What it reveals |
|-----------|--------|-----------------|
| **Phase** | think / act | Is the agent reasoning or executing? |
| **Scope** | converge / diverge | Narrowing toward a solution or broadening investigation? |
| **Locus** | internal / external | Working within the project or reaching outside? |
| **Novelty** | routine / novel | Familiar territory or new ground? |
| **Friction** | smooth / friction | Clean progress or encountering resistance? |

These dimensions enable queries that raw tool logs can't answer: "show me sessions where the agent got stuck on external integrations" or "which files consistently cause friction?" Phase and scope together characterize the agent's cognitive rhythm — a session that's 80% act+converge is focused implementation; one oscillating between think+diverge and act+converge is investigation-driven development.

### Episodic memory

Rather than treating a session as a flat sequence of tool calls, nmem detects **episodes** — bounded chunks of coherent work. An episode begins when the user's intent shifts (detected via Jaccard similarity on prompt keyword bags) and includes all observations until the next shift.

Each episode is annotated with hot files, phase signature, stance character across all five dimensions, an LLM-generated narrative, and a compact observation trace. Episodes sit between raw observations (too granular for context injection) and session summaries (too compressed to act on). They feed context injection for recent sessions and enable safe forgetting — once an episode's trace is frozen, the raw observations can be swept.

### Git integration and passive context enrichment

nmem fuses two information sources that are normally separate: **session memory** (what the agent did with a file across sessions) and **git history** (what happened to a file across all contributors). Git metadata from commits and pushes is extracted at hook time, while file-level history is computed and injected when the agent opens a file — no explicit query needed.

**What the agent gets when it opens a file:**

An LSP server (registered as a Claude Code plugin) publishes a diagnostic with the file's git summary — automatically, on every file open:
```
src/s4_context.rs: 42 commits over 180d, +5234/-892, 3 reverts.
Co-changes: schema.rs(28), s1_serve.rs(15).
Recent: "Fix episode window timezone bug" (2d ago), "Add cross-project context" (8d ago).
```

On hover, per-line blame with co-change context:
```
bpd  19d2ea5  3 days ago

Add episode-level friction to context injection

Co-changed: s4_memory.rs, s1_4_summarize.rs
```

The underlying `s1_git` module uses [git2](https://github.com/rust-lang/git2-rs) (libgit2 bindings) to extract commit history, churn metrics, co-change analysis, and revert detection — all in-process, no shell commands. The same data is available via the `git_file_summary` MCP tool for deeper on-demand investigation.

### Active retrieval

During a session, the agent has access to MCP tools for targeted queries:

| Tool | Purpose |
|------|---------|
| `search` | Full-text search over observations (FTS5: AND/OR/NOT, phrases, prefix matching) |
| `session_summaries` | Structured summaries of past sessions (intent, decisions, completed work, next steps) |
| `file_history` | A file's history across sessions with intent context |
| `recent_context` | Recent observations ranked by recency + type weight + project match |
| `current_stance` | Current session's cognitive trajectory with retrieval guidance |
| `git_file_summary` | Git history summary for a file (commits, churn, co-changes) |
| `timeline` | Observations surrounding an anchor point within the same session |
| `session_trace` | Step-by-step session replay |
| `create_marker` | Record a decision or conclusion as a durable observation |
| `queue_task` | Queue a task for later dispatch into a tmux session |

### Markers

Markers are agent-authored observations — conclusions, decisions, and waypoints that the agent records explicitly rather than nmem inferring from tool use. They serve as durable cross-session anchors, full-text indexed alongside automatic observations.

```
ADR-015 Fleet Beacon written. Covers: query federation architecture
(NATS request/reply, not data store), GitHub org SSO (OAuth Device Flow),
identity model (one user = one nmem = one machine)...
```

```
rollback-point: git-file-history + LSP integration. Base commit: 19d2ea5.
New files: src/s1_git.rs, src/s1_lsp.rs... To rollback: git checkout 19d2ea5 -- .
```

Unlike session summaries (generated automatically at session end), markers are intentional — the agent decides something is worth recording mid-session. Design decisions, research findings, implementation constraints, tutorial progress, rollback points.

## Privacy and security

nmem is designed for private, local operation:

- **Encrypted at rest** — SQLite database encrypted with SQLCipher. An encryption key is auto-generated at `~/.nmem/key` on first run.
- **Secret redaction** — regex patterns for common API key formats plus Shannon entropy detection for high-entropy strings. Secrets are filtered *before* storage, never after. Per-project sensitivity levels (strict/relaxed) tune the filtering threshold.
- **No cloud dependency** — all processing runs locally. Session summarization uses an embedded GGUF model, not an API call. Optional integrations (metrics, log streaming) are localhost-only and disabled by default.
- **Retention policies** — configurable per-observation-type TTLs. High-value signals (commits, edits) are retained longer than high-volume ones (file reads, searches). Pinned observations are exempt from sweeps.

## Getting started

### Build from source

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

### Configure Claude Code

Add hooks (`.claude/settings.json`):

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

Session summarization uses an embedded GGUF model via [llama-cpp-2](https://github.com/utilityai/llama-cpp-rs). The default model ([granite-4.0-h-tiny](https://huggingface.co/lmstudio-community/granite-4.0-h-tiny-GGUF)) auto-downloads from HuggingFace on first summarization and is cached locally. GPU acceleration is supported via feature flags (`--features cuda` or `--features rocm`).

### Storage

Encrypted SQLite (SQLCipher) at `~/.nmem/nmem.db`. Five tables: `sessions`, `prompts`, `observations`, `tasks`, `work_units`, plus FTS5 indexes and a `classifier_runs` audit table. Schema versioned via `rusqlite_migration` (12 migrations).

### Observation types

Every tool call the agent makes becomes a typed observation:

| obs_type | Source | Signal |
|----------|--------|--------|
| `file_read` | Read | Investigation |
| `file_write` / `file_edit` | Write / Edit | Execution |
| `search` | Grep, Glob | Investigation |
| `command` | Bash (generic) | Varies |
| `git_commit` / `git_push` | git commands | Completion |
| `github` | gh CLI | External interaction |
| `task_spawn` | Task | Delegation |
| `web_fetch` / `web_search` | WebFetch / WebSearch | Research |
| `mcp_call` | MCP tools | External tool |
| `marker` | `create_marker` / `nmem mark` | Agent-authored conclusion |

## CLI reference

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

## Roadmap

Designed but not yet implemented:

- **Autonomous context management** — inject context mid-session at work unit boundaries, not just at session start. Blocked on Claude Code platform evolution.
- **Cross-project retention** — per-project retention policies.
- **Multi-agent coordination** — shared memory, cross-agent retrieval. Requires networking layer.
- **PreCompact capture** — rolling summaries to preserve continuity when Claude Code compacts long-session context.

## Design docs

Architecture docs in [`design/`](design/):
- [DESIGN.md](design/DESIGN.md) — overall framing
- [VSM.md](design/VSM.md) — Viable System Model mapping and roadmap
- [ADR/](design/ADR/) — architectural decision records

## License

MIT
