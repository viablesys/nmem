# nmem

Cross-session memory for AI coding agents. Fleet-capable.

## What it does

AI coding agents lose everything when a session ends. The next session re-reads files, re-derives decisions, and repeats mistakes. nmem gives agents continuity by observing what they do, classifying each action on five cognitive dimensions, detecting episodes of coherent work, and feeding it all back at the next session start.

For teams, nmem federates queries across developer machines over NATS — every agent in the org can search what any other agent has learned, without centralizing data.

## Why it matters

On long-running projects, agents spend 20-30% of each session re-establishing context. That's wasted compute and developer wait time. nmem eliminates re-derivation by compressing prior work into episodes — intent-driven work units with narrative summaries, hot files, and stance signatures that the next session uses to pick up where the last one left off.

For organizations running multiple agents across a codebase, the fleet capability means one developer's agent can find that another developer's agent already solved the same problem last week — without anyone filing a ticket or writing documentation.

## Architecture

```
┌──────────────────────────────────────────────────────────────┐
│ Claude Code Session                                           │
│                                                               │
│  every tool call ────► nmem record                            │
│                           ├── classify (5 dimensions, <1ms)   │
│                           ├── filter secrets                  │
│                           └── store observation               │
│                                                               │
│  session start ──────► nmem serve (MCP server)                │
│                           ├── inject episodes + summaries     │
│                           └── answer retrieval queries         │
│                                                               │
│  session end ────────► nmem maintain                          │
│                           ├── detect episodes                 │
│                           ├── summarize (embedded GGUF model) │
│                           └── retention sweep                 │
│                                                               │
│  fleet (opt-in) ─────► nmem beacon                            │
│                           ├── subscribe to NATS               │
│                           ├── respond to federated queries    │
│                           └── heartbeat discovery             │
└──────────────────────────────────────────────────────────────┘
```

**No daemon, no cloud, no API keys.** Four process types:

| Process | Lifecycle | Purpose |
|---------|-----------|---------|
| `nmem record` | Per hook event | Capture and classify observations |
| `nmem serve` | Per session (MCP) | Answer retrieval queries from the agent |
| `nmem maintain` | Session end + timer | Summarize, detect episodes, sweep |
| `nmem beacon` | Long-lived (opt-in) | Federated fleet search over NATS |

Everything runs locally. Session summarization uses an embedded GGUF model (no LLM API calls). Fleet federation is opt-in and runs over NATS on your network.

## Capabilities

### Session continuity

At session start, nmem injects recent episodes (intent, hot files, stance character) and session summaries for older work. The agent starts oriented, not blank.

At session end, an embedded language model generates a structured summary — intent, decisions, completed work, next steps — optimized for the next AI session to reconstruct context.

### Five-dimension cognitive classification

Every observation is classified at write time using TF-IDF + LinearSVC models in pure Rust (sub-millisecond, 98.8% accuracy):

| Dimension | Values | Signal |
|-----------|--------|--------|
| **Phase** | think / act | Reasoning vs executing |
| **Scope** | converge / diverge | Narrowing toward solution vs broadening |
| **Locus** | internal / external | Within the project vs reaching outside |
| **Novelty** | routine / novel | Familiar territory vs new ground |
| **Friction** | smooth / friction | Clean progress vs encountering resistance |

These enable queries raw logs can't answer: "show sessions where the agent got stuck on external integrations" or "which files consistently cause friction."

### Episodic memory

Sessions are decomposed into **episodes** — bounded chunks of coherent work detected via intent shift analysis. Each episode carries hot files, a phase signature, an LLM narrative, and a compact observation trace. Episodes are the primary unit for context injection, fleet exchange, and safe forgetting (once the trace is frozen, raw observations can be swept).

### Fleet federation (NATS)

`nmem beacon` connects to a NATS server and subscribes to `nmem.{org}.search`. When a peer queries the fleet, each instance runs tiered FTS5 against its local encrypted DB and responds with matching episodes. No data is centralized — each machine keeps its own DB, and queries fan out over the network.

- **Tiered query rewriting** — phrase, AND, OR, prefix tiers ensure first-shot accuracy over the network
- **Adaptive timeouts** — Jacobson/Karels (RFC 6298) RTO calibrated from heartbeat RTT
- **Org isolation** — subject hierarchy (`nmem.{org}.*`) prevents cross-org leakage
- **Episode-first results** — responders return episodes (intent + narrative + stance), not raw observations

### Git integration

An LSP server publishes git history as diagnostics on file open — commit count, churn, co-changes, reverts. Per-line blame with co-change context on hover. The `git_file_summary` MCP tool provides the same data for on-demand queries. All computed in-process via libgit2.

### Active retrieval (MCP tools)

| Tool | Purpose |
|------|---------|
| `search` | Full-text search (FTS5: AND/OR/NOT, phrases, prefix) |
| `session_summaries` | Structured summaries of past sessions |
| `file_history` | A file's history across sessions with intent context |
| `recent_context` | Recent observations ranked by composite score |
| `current_stance` | Session's cognitive trajectory with retrieval guidance |
| `git_file_summary` | Git history for a file (commits, churn, co-changes) |
| `create_marker` | Record a decision or conclusion as a durable observation |
| `queue_task` | Queue work for later dispatch into a tmux session |

### Markers

Agent-authored observations — conclusions, decisions, research findings, rollback points — recorded explicitly mid-session. Full-text indexed alongside automatic observations, surfacing in search and context injection.

## Privacy and security

- **Encrypted at rest** — SQLCipher with auto-generated key
- **Secret redaction** — regex patterns + Shannon entropy detection, applied before storage
- **No cloud dependency** — summarization via embedded GGUF model, no API calls
- **Retention policies** — per-type TTLs, pinning for exemption
- **Fleet isolation** — NATS subject hierarchy scopes queries to your org

## Getting started

### Build

Requires Rust 1.85+, cmake, C++ compiler (llama.cpp).

```sh
cargo install --path .                  # CPU only
cargo install --path . --features cuda  # NVIDIA GPU
cargo install --path . --features rocm  # AMD GPU
```

First build: 2-3 minutes (llama.cpp C++ compilation). Subsequent builds are incremental.

### Configure Claude Code

Hooks (`.claude/settings.json`):

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

MCP server (`.claude/mcp.json`):

```json
{
  "mcpServers": {
    "nmem": { "type": "stdio", "command": "nmem", "args": ["serve"] }
  }
}
```

### Configure fleet (optional)

Add to `~/.nmem/config.toml`:

```toml
[beacon]
nats_url = "nats://nats.yourcompany.internal:4222"
org = "yourorg"
```

Run on each developer machine:

```sh
nmem beacon
```

### Verify

```sh
nmem status
```

## Configuration

`~/.nmem/config.toml` — all sections optional, sensible defaults.

```toml
[project]
strategy = "git"                # "git" (repo root) or "cwd" (directory name)

[filter]
extra_patterns = []             # additional regex patterns to redact

[encryption]
# key_file = "~/.nmem/key"     # auto-generated if absent

[retention]
enabled = true
[retention.days]
git_commit = 730                # 2 years
file_edit = 365
command = 180
file_read = 90                  # high-volume, shorter retention

[summarization]
enabled = true
model_path = "lmstudio-community/granite-4.0-h-tiny-GGUF:granite-4.0-h-tiny-Q4_K_M.gguf"
temperature = 0.0
n_ctx = 32768
n_gpu_layers = 999              # 999 = all layers to GPU (ignored without cuda/rocm)

[beacon]
nats_url = "nats://127.0.0.1:4222"
org = "yourorg"
respond = true
limit = 20
# credentials_file = "~/.nmem/nats.creds"
```

## CLI

```
nmem status              # DB health
nmem search <query>      # FTS5 search with BM25 ranking
nmem context             # Preview session-start injection
nmem beacon              # Connect to fleet NATS (long-lived)
nmem beacon --dry-run    # Connect but don't respond (debug)
nmem maintain            # Vacuum, checkpoint, FTS integrity
nmem maintain --sweep    # Run retention sweep
nmem maintain --catch-up # Summarize missed sessions
nmem purge               # Targeted deletion
nmem learn               # Cross-session pattern detection
nmem queue <prompt>      # Queue task for later dispatch
nmem dispatch            # Dispatch queued tasks to tmux
nmem mark <text>         # Create agent-authored marker
nmem backfill            # Classify historical observations
```

## Design

nmem is organized around Stafford Beer's [Viable System Model](https://en.wikipedia.org/wiki/Viable_system_model). Every module maps to a VSM system:

| System | Role | Modules |
|--------|------|---------|
| **S1** Operations | Capture, store, retrieve | `s1_record`, `s1_serve`, `s1_search`, `s1_extract`, `s1_git`, `s1_lsp` |
| **S1's S4** | Session intelligence (VSM recursion) | `s1_4_summarize`, `s1_4_inference` |
| **S2** Coordination | Classification, dedup | `s2_inference`, `s2_classify`, `s2_scope`, `s2_locus`, `s2_novelty` |
| **S3** Control | Retention, compaction, integrity | `s3_sweep`, `s3_maintain`, `s3_purge` |
| **S4** Intelligence | Context, dispatch, episodes, fleet beacon | `s4_context`, `s4_dispatch`, `s4_memory`, `s4_beacon`, `s3_learn` |
| **S5** Policy | Config, identity, boundaries | `s5_config`, `s5_filter`, `s5_project` |

Storage is encrypted SQLite (SQLCipher) with FTS5 full-text indexes. Session summarization uses an embedded GGUF model via [llama-cpp-2](https://github.com/utilityai/llama-cpp-rs) — the default model auto-downloads from HuggingFace on first use.

Architecture docs: [`design/`](design/) — [DESIGN.md](design/DESIGN.md), [VSM.md](design/VSM.md), [ADR/](design/ADR/).

## Roadmap

**Implemented:** Session continuity, 5-dimension classification, episodic memory, direct inference (embedded GGUF), git/LSP integration, fleet beacon (NATS query federation), tiered FTS5 rewriting, adaptive timeouts (Jacobson/Karels).

**In progress:** Fleet heartbeat coordinator, scatter/gather from MCP server, GitHub org SSO for fleet auth.

**Designed (ADR):** RAG distribution across fleet, ensemble research coordination, TruffleHog secret patterns, autonomous mid-session context injection.

## License

MIT
