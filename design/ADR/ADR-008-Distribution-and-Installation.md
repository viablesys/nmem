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

### Q4: Upgrade Path
- How does a user know a new version is available?
- Can the binary self-update, or is that the marketplace/package manager's job?
- Schema migrations (`rusqlite_migration`) handle DB upgrades — but what about config format changes?
- Breaking changes in hook payload format — how to handle version skew between Claude Code and nmem?

### Q5: Platform Support
- Linux x86_64 is primary (Fedora dev machine).
- macOS arm64 is the largest Claude Code user base — cross-compilation with SQLCipher?
- Windows — is it in scope?
- musl static linking for maximum portability vs glibc dynamic?

## Decision
*Pending — needs investigation of Claude Code plugin packaging format and marketplace requirements.*

## Consequences
*TBD*
