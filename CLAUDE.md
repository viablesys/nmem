# nmem — CLAUDE.md

> **Source**: Copied from `~/CLAUDE.md` (workspace root).
> This is the shared project config that applies across all workspace projects.
> Edit the original at `~/CLAUDE.md` — this copy is for reference within the nmem repo.

## Hard Rules
- **Always use Context7** — look up library docs before implementing
- **PRIVATE REPOS ONLY** — never create public repos
- Commit directly to main — no PRs, no feature branches
- Org: `github.com/viablesys`

## Environment
Fedora 43 • Zsh • `HSA_OVERRIDE_GFX_VERSION=11.0.0` (AMD GPU)

## Stack
Rust 1.92 • Go 1.25 • Python 3.13 • Node 22 • Android (WearOS)

## Preferences
- Concise code, descriptive names
- Handle errors at call sites
- Minimize dependencies
- Delay abstractions (rule of three)
- External config over code
- Breaking changes over legacy hacks

## Key Paths
```
~/dev/viablesys/     — github repos
~/Android/Sdk/       — Android SDK + adb
~/Applications/      — AppImages (Cursor, LM Studio)
```

## Reference Docs
When relevant, read these for detail:
- `~/.claude/docs/philosophy.md` — development principles
- `~/.claude/docs/context-management.md` — session and token strategies
- `~/.claude/docs/hardware-notes.md` — Framework 13 AI quirks
- `~/.claude/docs/power-management.md` — suspend/hibernate issues

## Library (`~/workspace/library/`)
Read the relevant doc before using a library. **Update this index when adding/removing docs.**

| File | Covers |
|------|--------|
| `rust.md` | Lifecycle roles, design patterns, error handling, async/concurrency, DevOps/Docker, security, pitfalls |
| `axum.md` | Axum 0.8 — routing, handlers, extractors, state, middleware (tower-http), WebSocket, SSE, testing, graceful shutdown, REST patterns |
| `rusqlite.md` | rusqlite — connections, WAL, transactions, FTS5, JSON, migrations, hooks, vector search, r2d2 pooling, async wrappers |
| `askama.md` | Askama 0.15 — compile-time templates, filters, inheritance, macros, whitespace, axum integration, htmx partials |
| `htmx.md` | htmx 2.0 — attributes, swap strategies, triggers, events, headers, extensions (SSE/WS/json-enc), patterns, axum-htmx |
| `tailwindcss.md` | Tailwind CSS v4 — utilities, layout/flex/grid, spacing, typography, responsive, dark mode, @theme/@utility/@variant, v3→v4 migration |
| `litestream.md` | Litestream — SQLite WAL streaming to S3/R2/GCS, Docker/K8s patterns, restore, monitoring, Rust integration |
| `claude-code-plugins.md` | Claude Code plugins — hooks, MCP servers (rmcp), commands, skills, agents, data storage, communication patterns |
| `rmcp.md` | rmcp 0.15 — MCP server in Rust, #[tool] macro, stdio transport, server state, Parameters/Json wrappers, error handling, testing, Claude Code integration |
| `css-doodle.md` | `<css-doodle>` — CSS art/patterns, grid, selectors, @shape, @svg, @shaders |
| `claude-code-hooks-events.md` | Claude Code hook event schemas — per-event payload fields, tool_input/tool_response by tool type, extraction patterns for structured observation capture |
| `claude-code-telemetry.md` | Claude Code telemetry — services (Statsig/Sentry/OTel), env vars, data collection, privacy controls, network endpoints |
| `rusqlite-migration.md` | rusqlite_migration 2.3 — schema versioning via user_version, inline/file/directory migrations, hooks, validation, async, evolution patterns, testing |
| `serde-json.md` | serde + serde_json — derive macros, attributes, enum representations, Value, custom serialization, error handling, SQLite+JSON patterns |
| `fts5.md` | FTS5 advanced — tokenizers, external content, column config, query syntax, maintenance, JSON indexing, input sanitization, pitfalls |
| `sqlcipher.md` | SQLCipher + rusqlite — encryption setup, PRAGMAs, key management, encrypting existing DBs, compatibility, performance, secure delete, pitfalls |
| `tokio-rusqlite.md` | tokio-rusqlite 0.6 — async SQLite, `.call()` pattern, reader/writer split, transactions, error handling, write batching, daemon patterns |
| `sqlite-retrieval-patterns.md` | SQLite multi-signal retrieval — FTS5+WHERE composition, recency weighting, composite scoring, file-based queries, BM25 at small scale, Rust patterns |
| `clap.md` | clap 4.5 — derive API, subcommands, stdin/stdout patterns, exit codes, error handling, fast startup, env vars, testing (assert_cmd), Cargo release profile |
| `regex.md` | regex 1.11 — Regex/RegexSet, compilation caching (LazyLock), replace_all (Cow), multi-pattern fast rejection, closure replacement, pattern syntax, testing, regex-lite |
| `victoria-logging.md` | VictoriaLogs — jsonline ingestion (Python stdlib + Rust reqwest), LogsQL queries, field conventions, hook error pattern, batch ingestion |
| `meta-cognition.md` | Meta-cognition in agent systems — cognitive loop, observation levels (reactive→anticipatory), intent hierarchy, self-referential observation, forgetting as compression, cross-session identity, retrieval as cognition |
