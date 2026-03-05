# nmem

Cross-session memory for Claude Code. Captures observations from every session (file reads, edits, commands, errors), stores them in encrypted SQLite, and retrieves them via MCP tools and CLI search. Sessions build on what came before.

## Install

1. Install the binary:

```sh
curl -fsSL https://raw.githubusercontent.com/viablesys/nmem/main/scripts/install.sh | sh
```

2. Add the marketplace and install the plugin:

```sh
claude plugin marketplace add viablesys/claude-plugins
claude plugin install nmem@viablesys
```

3. Restart Claude Code.

## What gets captured

| obs_type | Source | Signal |
|----------|--------|--------|
| `file_read` | Read | Investigation |
| `file_write` | Write | Execution |
| `file_edit` | Edit | Execution |
| `search` | Grep, Glob | Investigation |
| `command` | Bash | Varies |
| `git_commit` | git commit | Completion |
| `git_push` | git push | Completion |
| `github` | gh CLI | External interaction |
| `task_spawn` | Task | Delegation |
| `web_fetch` | WebFetch | Research |
| `web_search` | WebSearch | Research |
| `mcp_call` | MCP tools | External tool |

Every observation is classified on four dimensions at write time: **phase** (think/act), **scope** (converge/diverge), **locus** (internal/external), **novelty** (routine/novel). A fifth dimension, **friction**, is applied per-episode at session end.

## MCP tools

| Tool | Purpose |
|------|---------|
| `search` | Full-text search over observations (FTS5, AND/OR/NOT, phrases) |
| `get_observations` | Fetch full observation details by ID |
| `recent_context` | Recent observations ranked by recency + type weight |
| `session_summaries` | Structured summaries of past sessions |
| `timeline` | Observations surrounding an anchor point |
| `file_history` | Trace a file's history across sessions |
| `session_trace` | Step-by-step session replay |
| `current_stance` | Current session's cognitive trajectory |
| `queue_task` | Queue a task for later dispatch |
| `create_marker` | Record a decision or conclusion |
| `regenerate_context` | Re-run context injection |

## Project detection

nmem tags every session with a project name derived from the working directory.

**Default strategy (`git`):** walks parent directories for a `.git` directory or file, and uses the repository root's basename. This is stable regardless of which subdirectory you start a session from. Falls back to the basename of the working directory when no git root is found.

**Alternative strategy (`cwd`):** uses the basename of the current working directory directly. Useful for task-directory workflows where you create subdirectories per work unit (e.g., `tmp/OPS-1234`).

```toml
# ~/.nmem/config.toml
[project]
strategy = "cwd"   # default: "git"
```

| Working directory | `git` strategy | `cwd` strategy |
|---|---|---|
| `~/dev/my-repo/src` | `my-repo` | `src` |
| `~/dev/my-repo/tmp/OPS-42` | `my-repo` | `OPS-42` |
| `/tmp/scratch` | `scratch` | `scratch` |
| `~` | `home` | `home` |

## Configuration

`~/.nmem/config.toml`

```toml
[project]
strategy = "git"   # or "cwd" for task-directory workflows

[filter]
patterns = ["sk-[a-zA-Z0-9]{20,}"]  # secret patterns to redact

[retention]
file_read = 30    # days
file_edit = 90
command = 30
git_commit = 365

[summarization]
enabled = true
endpoint = "http://localhost:1234/v1/chat/completions"
model = "ibm/granite-4-h-tiny"
```

## Building from source

```sh
cargo install --path .
```

Requires Rust 1.80+. The build bundles SQLCipher (no system dependency needed).

## Design

Architecture docs in [`design/`](design/):
- [DESIGN.md](design/DESIGN.md) — overall framing
- [VSM.md](design/VSM.md) — Viable System Model mapping and roadmap
- [ADR/](design/ADR/) — architectural decision records

## License

MIT
