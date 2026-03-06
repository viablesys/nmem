# ADR-008: Distribution and Installation

## Status
Draft

## Framing
*How does nmem get from a git repo to a user's machine, and what does "install" mean for each audience?*

Three distinct install paths serve different needs: marketplace distribution for discovery, binary release for direct install, and source checkout for development. The adversarial question: "What if a user installs via marketplace but needs to customize hooks or config?" — does the marketplace abstraction leak, and at what cost?

## Depends On
- ADR-003 (Daemon Lifecycle) — install must set up both `record` (hook handler) and `serve` (MCP server) correctly.
- ADR-007 (Trust Boundary) — encryption key provisioning is an install-time concern.

## Unlocks
- Versioning and upgrade strategy (future ADR).
- Telemetry opt-in/out UX.

---

## Context

nmem is a single Rust binary (`nmem`) with two subcommands: `record` (synchronous hook handler) and `serve` (MCP server). Installation requires:

> **[ANNOTATION 2026-02-21, v0.2]:** The binary now has 16 subcommands, not 2. Beyond `record` and `serve`: `purge`, `maintain`, `status`, `search`, `encrypt`, `pin`, `unpin`, `context`, `queue`, `dispatch`, `task`, `learn`, `backfill`, `backfill-scope`. See `src/cli.rs` for the full `Command` enum. The core installation requirement (binary on PATH + hooks + MCP registration) remains accurate, but the characterization of nmem as having only two subcommands is stale.

1. The binary on `$PATH` (or an absolute path in config)
2. Claude Code hooks configured in `~/.claude/settings.json` (PostToolUse, SessionStart, UserPromptSubmit, Stop)
3. MCP server registered in `.claude.json` or project-level config
4. Optional: `~/.nmem/config.toml` for encryption, filtering, metrics, retention

## Open Questions

### Q1: Claude Code Marketplace (Plugin Distribution)
- What is the plugin packaging format? Single binary + manifest? Archive?
- Does the marketplace handle platform-specific binaries (linux-x86_64, darwin-arm64, etc.)?
- Can a plugin declare hooks and MCP servers declaratively, or does the user still edit settings.json manually?
- What's the update mechanism — auto-update, manual, pinned versions?
- How does the marketplace handle plugins that need post-install setup (key generation, DB init)?

### Q2: End-User Binary Install
- GitHub Releases with prebuilt binaries per platform?
- Install script (`curl | sh`) that downloads the right binary + writes hook config?
- Homebrew formula / cargo install / AUR package?
- What's the minimum setup: binary + one command to wire hooks + MCP, or does the user manually edit JSON files?
- Should `nmem init` exist as a subcommand that writes the required config?

### Q3: Development Install
- `cargo install --path .` from checkout is the current workflow.
- Release profile (opt-level=z, LTO, strip) produces a ~5MB binary — acceptable for dev use too?
- Should dev install skip release optimizations for faster iteration?
- How to handle the chicken-and-egg: nmem hooks call the nmem binary, but during development the binary path changes between debug/release builds.

> **[NOTE 2026-02-15]** Current dev install uses a symlink: `ln -sf ~/workspace/nmem/target/release/nmem ~/.local/bin/nmem`. This keeps `nmem` on PATH without `cargo install`, and `cargo build --release` updates it in place (hardlinked). Hooks and slash commands can reference `nmem` by name rather than absolute path. Trade-off: forgets to rebuild → stale binary; but that's already the case with hooks calling `target/release/nmem` directly.

### Q4: Upgrade Path
- How does a user know a new version is available?
- Can the binary self-update, or is that the marketplace/package manager's job?
- Schema migrations (`rusqlite_migration`) handle DB upgrades — but what about config format changes?
- Breaking changes in hook payload format — how to handle version skew between Claude Code and nmem?

> **[ANNOTATION 2026-03-05, v0.3]:** Investigated Q1 and Q4 in the context of v1.0.6. Findings and candidate features:
>
> **Marketplace update mechanism (Q1/Q4):** The Claude Code marketplace does not have built-in support for distributing or auto-updating platform-specific binaries. It handles plugin components (hooks, MCP config, manifest) but not native binaries. The nmem architecture already separates these: binary via GitHub Releases + install script, plugin via marketplace. Auto-update (self-replacing binary) was considered and rejected — too much implicit trust. The marketplace should own plugin updates; the binary is a separate concern.
>
> **Candidate features for upgrade path (Q4):**
>
> 1. **Version check at SessionStart** — During context injection, compare `env!("CARGO_PKG_VERSION")` against the latest GitHub release tag. Cache the result (e.g. `~/.nmem/latest-version` with a TTL) to avoid hitting the API on every session. If stale, emit a one-line notice: `nmem: update available (v1.0.6 → v1.0.7). Run: curl -fsSL ... | sh`. No auto-download, no self-modification — just information.
>
> 2. **Config drift detection** — Add `nmem config check` subcommand that loads `~/.nmem/config.toml` and diffs it against the current binary's expected schema. Reports: missing new sections/keys (with defaults and descriptions), unknown/deprecated keys, and values outside valid ranges. Optional: a `version` key in config.toml to track which binary version last wrote it. Pairs with the version check — when a new release introduces config options, the user learns about them.
>
> 3. **SessionStart config nudge** — If `config check` detects drift, surface a one-liner during context injection alongside the version notice. Keeps the user informed without blocking.
>
> **Design constraint:** All three features are informational only. No auto-modification of binaries or config. The user runs the install script or edits config manually. This matches the project's "breaking changes over legacy hacks" principle — tell the user what changed, don't paper over it.
>
> **Activation trigger:** Implement when the config schema starts changing between releases (currently stable) or when marketplace adoption creates a user base that wouldn't check GitHub releases manually.

### Q5: Platform Support
- Linux x86_64 is primary (Fedora dev machine).
- macOS arm64 is the largest Claude Code user base — cross-compilation with SQLCipher?
- Windows — is it in scope?
- musl static linking for maximum portability vs glibc dynamic?

## Decision
*Pending — needs investigation of Claude Code plugin packaging format and marketplace requirements.*

## Consequences
*TBD*

## Revision History

| Date | Version | Changes |
|------|---------|---------|
| 2026-02-15 | 0.1 | Draft with open questions, dev install note. |
| 2026-02-21 | 0.2 | Annotated stale subcommand count (2 → 16). |
| 2026-03-05 | 0.3 | Q4 annotation: marketplace has no binary update mechanism; three candidate features (version check, config drift, SessionStart nudge) with activation trigger. |
