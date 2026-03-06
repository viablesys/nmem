# LSP Server Investigation — Issue #9

## What the proposal says

Add an `nmem lsp` subcommand running a Language Server Protocol server over stdio, injecting session memory and git history as diagnostics when Claude Code opens or edits files. The pitch: passive file-level context injection that fills the gap between MCP (active, on-demand) and SessionStart (push, session-scoped).

Five implementation phases: scaffold, nmem-store diagnostics, git integration (libgit2), hover + code actions, observation write-back from other LSPs.

## What the proposal assumes

### 1. The model doesn't get file-level context today

This is partly true. SessionStart injection is project-scoped, not file-scoped. But the `file_history` MCP tool already returns per-file cross-session history, and the CLAUDE.md retrieval triggers mandate calling it on first contact with any file. If the model isn't doing that, the problem is retrieval behavior, not tooling. Adding a passive channel works around the behavior gap rather than fixing it.

**Question**: How often does the model open a file where prior context would have changed its approach, and fail to query `file_history`? Without data on this, we're building infrastructure for a hypothesized problem.

### 2. Claude Code supports custom LSP servers from plugins

**Confirmed.** `.lsp.json` is a fully documented, supported Claude Code plugin feature (since v2.0.74, December 2025). The official plugins reference at code.claude.com documents the format, required fields (`command`, `extensionToLanguage`), optional fields (`transport`, `env`, `initializationOptions`, `restartOnCrash`, etc.), and three official LSP plugins exist in the marketplace (`pyright-lsp`, `typescript-lsp`, `rust-lsp`).

This assumption holds. The registration mechanism is real.

### 3. How diagnostics actually reach the model

Investigated via Claude Code GitHub issues and documentation. The mechanism:

1. **Push-based injection after edits.** After every file Read/Edit/Write, the LSP server pushes diagnostics. Claude Code wraps them in `<new-diagnostics>` tags and injects them into the conversation context.

2. **No severity filtering.** Claude Code injects ALL diagnostic severities — Error, Warning, Information, and Hint — without distinction. This is confirmed by [issue #26634](https://github.com/anthropics/claude-code/issues/26634), where pyright's Hint-level `DiagnosticTag.Unnecessary` diagnostics get promoted into conversation context, causing the model to "fix" non-issues.

3. **No batching or dedup.** Diagnostics inject per file event, not batched. The same diagnostics re-inject on every Read/Edit/Write of the same file.

4. **No per-file token cap.** There is no built-in truncation. The LSP server's output is injected verbatim.

5. **Active tools also exist.** Beyond passive diagnostics, Claude Code exposes active LSP operations as an `LSP()` tool: `goToDefinition`, `findReferences`, `hover`, `documentSymbol`, `workspaceSymbol`, `goToImplementation`, call hierarchy. The [diagnostics operation was requested](https://github.com/anthropics/claude-code/issues/15302) separately and later implemented.

**Implications for nmem LSP:**

- The issue #9 comment's conclusion that "nmem owns the budget" is correct — Claude Code won't cap output.
- The 200-500 token target per file event is necessary and must be enforced by nmem.
- Severity choice matters more than expected. Issue #26634 shows that Hint-level diagnostics cause active harm (model tries to "fix" them). The proposal's decision to avoid Hint severity is validated.
- The `<new-diagnostics>` injection tag means nmem diagnostics will be visually indistinguishable from real language server diagnostics. Using `source: "nmem"` helps, but the model may still treat memory diagnostics as code errors to fix.

**Risk identified:** Memory observations presented as "diagnostics" create a category confusion. A real LSP diagnostic says "this code has an error." An nmem diagnostic says "a prior session had trouble with this file." These are fundamentally different signals, but they arrive through the same channel in the same format. The model may respond to "Prior failure: auth timeout" by trying to find and fix the auth timeout, even if the current code is correct and the failure was in a different context.

### 4. The `extensionToLanguage` requirement

The `.lsp.json` format requires an `extensionToLanguage` field mapping file extensions to language identifiers. This is how Claude Code decides which LSP server handles which files. There's no documented wildcard — you must enumerate extensions.

For a language-agnostic memory server, this means either:
- Listing every file extension nmem should cover (`.rs`, `.py`, `.ts`, `.go`, `.md`, etc.) — brittle, incomplete
- Finding whether a catch-all pattern exists — not documented

This is a practical constraint the proposal doesn't address. The issue says `filePatterns: ["**/*"]` but that field doesn't exist in the actual `.lsp.json` schema. The real field is `extensionToLanguage`, which requires explicit extension-to-language mappings.

**Question:** Can `extensionToLanguage` map arbitrary extensions to a made-up language identifier like `"nmem"`? If so, does Claude Code still route file events to the server? This needs testing.

### 4. Coupling and co-change data exists

The diagnostic generator assumes data sources that don't exist in nmem today:

- **Fragile couplings** — no coupling table, no coupling detector, no fragility score
- **Abandoned branches** — nmem captures git commit/push metadata, not branch lifecycle
- **Revert detection** — no revert tracking in the data model
- **Co-change analysis** — requires git2 log traversal, not currently implemented

The proposal is designing an output surface for data that hasn't been built. Phases 2-3 of the implementation aren't "wire up existing data" — they're "build the data pipeline AND the output surface."

### 5. The "no daemon" architecture accommodates a long-running LSP server

ADR-003 explicitly chose against long-running processes. The four process modes (hook handler, MCP server, CLI, dispatcher) are either short-lived or session-scoped. An LSP server is a persistent daemon — it must stay running across file events to maintain state (git caches, diagnostic dedup). This contradicts a load-bearing architectural decision.

The MCP server is the closest precedent (session-scoped, long-lived during a session), but it's spawned by Claude Code and dies with the session. An LSP server has the same lifecycle, so this may be acceptable — but it should be explicitly addressed as an extension of the MCP server pattern, not assumed compatible.

## Tensions with existing design

### Harness independence (ADR-006)

ADR-006 carefully confines Claude Code coupling to two boundary points: ingestion adapter (`nmem record`) and MCP server (`nmem serve`). The LSP server adds a third interface surface that specifically targets Claude Code's diagnostic injection behavior. The diagnostic severity choices, token budget, and dedup strategy are all designed around Claude Code's known behavior of injecting all diagnostics without filtering.

If Claude Code changes how it handles diagnostics (adds filtering, batching, severity gates), the LSP server's assumptions break. This is tighter coupling than the MCP server, which speaks a standard protocol.

### Complexity budget

nmem is 30 modules today. The LSP server adds:
- New crate dependency: `tower-lsp` (pulls in tower, async stack)
- New crate dependency: `git2` (libgit2 C bindings, ~5MB)
- New module: `s1_lsp.rs` or similar
- New data queries: coupling detection, co-change analysis, revert detection
- New cache layer: git data with TTL
- New configuration: `lsp_diagnostics_enabled`, `lsp_max_diagnostics_per_file`

This is not a small addition. The dependency footprint (especially `git2`) is significant for a project that values minimal dependencies.

### Overlap with MCP tools

The diagnostic content proposed — failures, decisions, couplings, co-changes — overlaps heavily with what `file_history`, `search`, and `recent_context` already provide. The difference is push vs pull. But the CLAUDE.md retrieval triggers already establish a protocol for when to pull. If push is genuinely needed, a simpler approach might work: enhance SessionStart context injection to include per-file alerts for files the model is likely to touch (based on recent session patterns).

## What might actually be useful

Stripping away the assumptions, the core value proposition is: "when the model opens a file with known failures, it should know about them without an explicit query." That's a real need. But LSP is one way to deliver it, not the only way.

### Alternative: Enhanced context injection

SessionStart already pushes context. It could be extended to include file-level alerts:

```
## File Alerts
- src/auth/mod.rs — 3 failures in last 2 sessions, last resolution: "add timeout to token refresh"
- src/db.rs — coupled with schema.rs (8/10 recent commits touch both)
```

This uses existing infrastructure (s4_context.rs), existing data (observations), and adds zero dependencies. It's session-scoped rather than per-file-event, so it's less precise but also less complex. Token cost is fixed at session start, not compounding per file event.

### Alternative: MCP tool enhancement

Add a `file_alerts` MCP tool that returns high-priority observations for a file path — failures, recent reverts, high churn. The model calls it alongside `file_history` on first contact. This keeps the pull model but makes the query cheaper and more targeted than `file_history` (which returns everything).

### Alternative: Hook-based file alerts

A PreToolUse hook on Read/Edit that checks the file path against a lightweight alert table (maintained by the Stop hook or a sweep). If alerts exist, inject them as hook output. This is push-on-file-access without LSP machinery.

## Resolved questions

1. **Is `.lsp.json` a real Claude Code plugin feature?** Yes. Fully documented, three official plugins in marketplace, supported since v2.0.74. Not a blocker.

2. **How do diagnostics reach the model?** Via `<new-diagnostics>` tag injection after every file Read/Edit/Write. No severity filtering, no batching, no token cap. nmem must self-limit.

## Open questions

1. **Does `extensionToLanguage` support a catch-all?** The proposal assumes language-agnostic coverage (`**/*`), but the actual schema requires explicit extension-to-language mappings. Can we map arbitrary extensions to a synthetic language ID? Needs testing.

2. **Category confusion risk.** Memory observations presented as diagnostics may cause the model to treat "prior session had trouble here" as "this code has a bug to fix." How does the model actually behave when it receives non-code-error diagnostics? Needs testing with a minimal prototype.

3. **What's the actual failure rate of retrieval-trigger compliance?** If the model follows retrieval triggers 90% of the time, LSP adds marginal value. If 30%, there's a real gap — but maybe the fix is better trigger enforcement, not a new channel.

4. **Does the token cost compound to a problem?** 500 tokens per file event, 30+ file events per session, no batching. Is 15,000+ tokens of diagnostic overhead acceptable? The issue #26634 experience suggests even small per-file overhead (5-10 lines of false positives) causes measurable harm over a refactoring session.

5. **Can the data pipeline (coupling detection, co-change analysis) justify itself independently?** If these are useful for other purposes (session summaries, episode enrichment), building them is defensible even without LSP. If they only serve LSP diagnostics, the investment is harder to justify.

6. **What's the minimal experiment?** Before building the full LSP server, is there a way to test whether passive file-level alerts actually change model behavior? Options: (a) manually add file alerts to CLAUDE.md for a week, (b) use a PreToolUse hook on Read to inject `additionalContext` with file alerts, (c) build a trivial LSP server that emits static diagnostics for a few known files and observe model behavior.

## Recommendation

The delivery mechanism (`.lsp.json`) is confirmed viable. The proposal is not blocked by platform support. However, three concerns remain before committing to implementation:

1. **Category confusion** — Memory observations masquerading as code diagnostics is an untested interaction pattern. The model may misinterpret "prior session context" as "current code errors." This needs a cheap experiment before a full build.

2. **Data doesn't exist** — The diagnostic content (couplings, co-changes, reverts, abandoned branches) requires building data pipelines that don't exist. The LSP server is only as useful as the data it surfaces. Building the server first is premature.

3. **Cheaper alternatives exist** — A PreToolUse hook on Read/Edit can inject `additionalContext` per file access with zero new dependencies. This tests the core value proposition (does passive file-level context change model behavior?) without the LSP machinery.

**Suggested next step:** Build alternative (c) from question 6 — a trivial prototype to test whether the model responds usefully to passive file-level memory injection, using whichever delivery mechanism is cheapest (hook-based `additionalContext` injection). If behavior change is confirmed, proceed to LSP. If not, the LSP server solves a problem that doesn't exist.

## Sources

- [Claude Code plugins reference — LSP servers](https://code.claude.com/docs/en/plugins-reference)
- [Claude Code plugins guide](https://code.claude.com/docs/en/plugins)
- [Issue #26634 — pyright hint-level diagnostics promoted into context](https://github.com/anthropics/claude-code/issues/26634)
- [Issue #15302 — LSP diagnostics support feature request](https://github.com/anthropics/claude-code/issues/15302)
- [Issue #15955 — LSP diagnostics for IDE errors/warnings](https://github.com/anthropics/claude-code/issues/15955)
- [Issue #16804 — textDocument/didOpen never sent](https://github.com/anthropics/claude-code/issues/16804)
