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
└──────────────────────────────────────────────────────────────┘

┌──────────────────────────────────────────────────────────────┐
│ Fleet (opt-in, per machine)                                   │
│                                                               │
│  nmem beacon ────────► subscribe to nmem.{org}.search         │
│                           ├── receive query from peer         │
│                           ├── run tiered FTS5 locally         │
│                           └── respond with episodes           │
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

nmem is organized around Stafford Beer's [Viable System Model](https://en.wikipedia.org/wiki/Viable_system_model) at two recursive levels: the **instance** (one developer's machine) and the **fleet** (all instances in an org).

### Instance level — one nmem

Each nmem is a viable system. Every module maps to a VSM system:

| System | Role | Modules |
|--------|------|---------|
| **S1** Operations | Capture, store, retrieve | `s1_record`, `s1_serve`, `s1_search`, `s1_extract`, `s1_git`, `s1_lsp` |
| **S1's S4** | Session intelligence (VSM recursion) | `s1_4_summarize`, `s1_4_inference` |
| **S2** Coordination | Classification, dedup | `s2_inference`, `s2_classify`, `s2_scope`, `s2_locus`, `s2_novelty` |
| **S3** Control | Retention, compaction, integrity | `s3_sweep`, `s3_maintain`, `s3_purge` |
| **S4** Intelligence | Context injection, episodes, cross-session patterns | `s4_context`, `s4_dispatch`, `s4_memory`, `s3_learn` |
| **S5** Policy | Config, identity, boundaries | `s5_config`, `s5_filter`, `s5_project` |

S4 synthesizes *across sessions* within one instance — it detects episodes, generates narratives, and injects prior context so each session builds on the last.

### Fleet level — many nmems

When instances connect via NATS, a second VSM emerges at the fleet level:

| Fleet system | Role | Implementation |
|-------------|------|----------------|
| **Fleet S1** | Each instance's operations | Individual nmem instances (the entire instance-level VSM) |
| **Fleet S2** | Message routing, no state | NATS subject hierarchy (`nmem.{org}.*`) |
| **Fleet S4** | Cross-instance intelligence | `s4_beacon` — federates queries, synthesizes across machines |

The beacon (`s4_beacon`) is fleet-level S4: it does for the fleet what instance-level S4 does for sessions. Instance S4 asks "what did prior sessions learn about this?" Fleet S4 asks "what did other developers' agents learn about this?" Same function, different scale.

This is VSM recursion — each instance is S1 from the fleet's perspective, containing its own complete S1-S5 internally. The fleet doesn't need its own S3 or S5 because each instance manages its own retention and policy. NATS is pure S2 — coordination without memory.

### Storage

Encrypted SQLite (SQLCipher) with FTS5 full-text indexes. Session summarization uses an embedded GGUF model via [llama-cpp-2](https://github.com/utilityai/llama-cpp-rs) — auto-downloads from HuggingFace on first use.

Architecture docs: [`design/`](design/) — [DESIGN.md](design/DESIGN.md), [VSM.md](design/VSM.md), [ADR/](design/ADR/).

## Ensemble research and document trust

AI agents generate reference documents, but how much should you trust them? A single agent researching a topic follows one path through the search space, carries its training biases into every claim, and has no mechanism to distinguish confident knowledge from confident hallucination. The result is a document that reads well but may contain errors invisible to both the agent and the reader. Garbage in, garbage out — except the garbage looks authoritative.

nmem addresses this through an ensemble research capability grounded in both empirical AI safety research and classical voting theory.

### The research

Anthropic's ["The Hot Mess of AI"](https://alignment.anthropic.com/2026/hot-mess-of-ai/) (Hägele et al., ICLR 2026; [arXiv:2601.23045](https://arxiv.org/abs/2601.23045)) decomposes AI errors into **bias** (systematic, consistent mistakes) and **variance** (incoherent, unpredictable mistakes) using the classical bias-variance framework. Their key findings:

1. **Longer reasoning produces more incoherent errors.** As tasks get harder and reasoning chains grow, failures become increasingly variance-dominated — the "hot mess" rather than coherent pursuit of wrong goals.
2. **Ensembling reduces incoherence.** Aggregating multiple independent samples reduces the variance component at rate 1/N, without touching bias.
3. **Scale reduces bias faster than variance.** Larger models learn *what* to do faster than they learn to *reliably* do it — the gap between knowing and consistently executing grows with capability.

The implication: capable AI agents fail less often, but when they fail, those failures are increasingly random and unpredictable. Ensembling is the direct intervention for this failure mode.

### The test

These findings matched what nmem observed in practice. Sessions where the agent produced reference documents showed exactly this pattern — most content correct, but scattered factual errors (version numbers, performance claims, feature availability) that varied across attempts. The errors weren't systematic; they were the "hot mess" the paper describes. Friction episodes in nmem's episodic memory captured this: novel+external work with inconsistent failures across retries.

### The theory

The ensemble benefit has a 240-year-old mathematical foundation. [Condorcet's jury theorem](https://en.wikipedia.org/wiki/Condorcet%27s_jury_theorem) (1785) proves that if N independent voters each have probability p > 0.5 of being correct, majority-vote accuracy approaches 1 as N increases:

```
ensemble_acc = Σ_{k=⌈N/2⌉}^{N} C(N,k) · p^k · (1-p)^(N-k)
```

The mapping to the Hot Mess framework is exact:
- **Bias** = systematic errors shared by all agents — ensembling cannot fix these (all agents wrong the same way)
- **Variance** = incoherent errors independent across agents — majority voting cancels these out

The corollary is the true "garbage in, garbage out": if individual accuracy p < 0.5, ensembling doesn't merely fail to help — it actively makes things *worse*, converging toward certainty of the wrong answer as N grows. Condorcet's theorem is a double-edged sword. The ensemble only works when the individual agents are better than chance.

### The codified skill

nmem's ensemble research skill operationalizes this into a six-phase workflow:

1. **Spawn** N independent researchers (default 5) with identical prompts — no pre-assigned angles, no shared context
2. **Collect** outputs independently
3. **Correlate** — cross-reference claims across all researchers, build agreement distribution tables
4. **Fact-check** — verify divergences against authoritative sources (package registries, official docs, release notes)
5. **Synthesize** — convergence-weighted merge: 5/5 agreement = high confidence, 1/5 = flag for verification
6. **Produce** final document with quality metrics

The identical-prompt constraint is load-bearing. If researchers get different prompts, differences in output reflect prompt differences, not genuine diversity of findings. Independence — each researcher choosing their own path through the search space — is what makes agreement meaningful.

### Quality metrics

Each ensembled document carries computed quality metrics ([ADR-015](design/ADR/ADR-015-Fleet-Beacon.md)):

| Metric | Definition |
|--------|-----------|
| **p_hat** | Estimated single-agent accuracy (Condorcet MLE from agreement distribution) |
| **ensemble_acc** | Majority-vote accuracy computed from p_hat via the binomial formula |
| **calibration** | 1 - (verified_wrong / verified_total) — how accurate "verified" claims actually are |
| **q_final** | ensemble_acc × calibration — post-correction confidence |

Observed baselines from production runs:

| Document type | N | p_hat | ensemble_acc | q_final |
|--------------|---|-------|-------------|---------|
| Library reference docs | 5 | 0.947 | 99.7% | 0.798 → 0.997 post-correction |
| Design/architecture | 5 | 1.0 | 1.0 | 1.0 |
| Infrastructure Q&A | 7 | 1.0 | 1.0 | 1.0 |

The pattern: errors cluster in numerical/factual claims (versions, sizes, speeds), not in architectural or mechanistic reasoning. Design ensembles achieve q_final = 1.0 because the claims are structural — overdetermined by constraints. Research docs hit ~95% single-agent accuracy because they contain numerical facts the model is less certain about. N=5 is sufficient to lift that to 99.7%.

### Honest limitations

The ensemble research skill was itself verified by ensemble — five independent researchers validated the claim chain from the Hot Mess paper through Condorcet to the quality metrics. All seven core claims achieved 5/5 agreement. But those same researchers unanimously identified the key weakness: **independence**.

Same-architecture LLM agents with identical prompts are not independent in the Condorcet sense. They share training data, architectural biases, and prompt-induced correlations. Empirical work confirms this: studies on LLM ensemble sentiment analysis ([arXiv:2409.00094](https://arxiv.org/abs/2409.00094)) found only marginal improvements from majority voting because models make correlated errors. "Consensus is Not Verification" ([arXiv:2603.06612](https://arxiv.org/abs/2603.06612)) showed that no aggregation strategy consistently beats single-sample baselines when agents share systematic biases.

This means p_hat estimated from agreement is an **upper bound**, not a direct measurement of accuracy. When all five researchers agree on a wrong fact (because it's wrong in their shared training data), agreement is high but accuracy is zero. The correlation and fact-checking phases exist precisely to catch this failure mode — they are not optional polish but the mechanism that converts unreliable consensus into verified accuracy.

The quality metrics are transparent about this: q_final decomposes into ensemble_acc (theoretical, assumes independence) × calibration (empirical, measures how often "verified" claims survived fact-checking). The calibration term is where reality corrects theory.

## Roadmap

**Implemented:** Session continuity, 5-dimension classification, episodic memory, direct inference (embedded GGUF), git/LSP integration, fleet beacon (NATS query federation), tiered FTS5 rewriting, adaptive timeouts (Jacobson/Karels), ensemble research skill with Condorcet quality metrics.

**In progress:** Fleet heartbeat coordinator, scatter/gather from MCP server, GitHub org SSO for fleet auth.

**Designed (ADR):** RAG distribution across fleet, fleet-distributed ensemble research, TruffleHog secret patterns, autonomous mid-session context injection.

## License

MIT
