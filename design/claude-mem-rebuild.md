# nmem — Claude-Mem Rebuild Project

## Status: Early Design Phase
Started: 2026-02-08
Project directory: ~/forge/nmem/
Design doc: ~/forge/nmem/DESIGN.md
Project CLAUDE.md: ~/forge/nmem/CLAUDE.md

## Key Decisions
- **No JavaScript/TypeScript** — security concern, npm supply chain risk
- **Security-first** — all security considerations to be explored fully
- **Design before implementation** — multi-day collaborative process

## Name Candidates
### Functional / Descriptive
- `recall` — simple, what it does
- `engram` — a memory trace in the brain
- `cortex` — the part of the brain that stores/retrieves memories

### Metaphorical
- `mnemo` — from Mnemosyne, Greek titan of memory
- `palimpsest` — a manuscript written over previous layers
- `cairn` — trail markers left behind for the next traveler

### Short / Unix-y
- `mem` — dead simple
- `rem` — remember
- `trc` — trace

### Rust/Systems Vibes
- `stash` — store and retrieve
- `vault` — secure persistent store
- `ledger` — structured record of events

## Organizing Principle: Viable System Model (VSM)
Stafford Beer's VSM as the logical design framework.

- **S1 (Operations)** — capture, store, retrieve, search. Multiple operational units.
- **S2 (Coordination)** — dedup, concurrency, sequencing across sessions.
- **S3 (Control)** — compaction, retention, storage budgets. The "operator" logic.
- **S3* (Audit)** — integrity checks, consistency verification.
- **S4 (Intelligence)** — adapt capture strategies, detect useful vs noise.
- **S5 (Policy)** — configuration, core purpose, identity.
- **Environment** — user/operator + any CLI/IDE/harness (Claude Code is one consumer).

### Key Principles
- System is viable in its own right, not a plugin bolted onto a host
- No tight coupling to any specific harness — protocol-based interface
- Recursive structure (S1 units can be viable systems themselves)
- Variety management (Ashby's Law) — absorb observation variety without overwhelm
- Portability and composition — works with any interface layer above

## Core Requirements
- **Automatic** — no manual intervention during normal operation
- **Near zero maintenance** — self-managing, no babysitting
- **Own lightweight operator** — self-managing agent/daemon pattern
  - Auto-compaction / pruning
  - Self-healing (corrupt state, index rebuilds)
  - Lifecycle management (no orphan processes)
  - Storage hygiene (rotation, vacuuming, retention)
  - Health monitoring

## Decision Points to Explore

### Language & Runtime
- Language choice (Rust vs Go vs other)

### Security
- Eliminate localhost HTTP port? (Unix socket, stdin/stdout, etc.)
- Scope of observation (every tool call vs selective)
- Database encryption at rest
- All other security considerations TBD

### Architecture
- SDK agent subprocess — keep, replace, or eliminate?
- Vector DB approach (embedded vs external vs skip entirely)
- Plugin architecture (hooks, MCP, etc.)

### Operator Pattern
- Long-running daemon (persists across sessions) vs session-bootstrapped (starts/stops with each session)?
- Invisible (never think about it) vs observable (can ask about health/status)?
- Zero configuration (works out of the box) vs initial setup acceptable?

### Forgetting & Retention
- Intentional deprecation — not just compaction but recognizing stale/untrue observations
- Theory of forgetting as S3 concern

### Algedonic Signals
- Pain/pleasure bypass channels for urgent system health issues
- Silent when healthy, interrupts when not

### Observation Extraction
- Current: SDK agent (LLM subprocess) for every tool call — expensive, fragile, security risk
- Alternative: structured extraction (heuristics, harness-declared noteworthiness) for 80% of cases
- Reserve LLM for S4 (periodic reflection), not inline extraction?

### Trust & Secrets
- Explicit exclusion rules for credentials, tokens, private data
- Classification of observations by sensitivity
- S5 policy with S1 operational implications

### Relevance vs Similarity
- Vector similarity is weak proxy for true relevance
- Contextual relevance: project, task phase, current intent
- S4 intelligence concern

### Cold Start / Bootstrapping
- New system or new machine has no memories
- Import from existing context (git history, docs, CLAUDE.md)?
- Or accept cold start and earn value over time?

### Interface Design
- Consumer shouldn't need to know internals
- Protocol-level, not implementation-level
- "What's relevant to what I'm doing?" / "Remember what's worth remembering"
- System decides the how internally

### Project Recursion
- Project-scoped but cross-pollinating
- Each project as S1 unit with local memory
- Cross-project pattern recognition without leaking secrets
- S2 coordination across projects

### Naming
- Does the name reflect agency/personality (engram, cortex, mnemo, cairn) or stay mechanical (mem, trc, stash)?
- Operator having its own identity/personality or purely functional?

## Current Architecture (for reference)
- Background worker on port 37777 (TypeScript/Node)
- SDK agent (observer-only Claude subprocess)
- Pipeline: PostToolUse hook → HTTP POST → SDK agent → SQLite + Chroma
- Hook events: SessionStart, UserPromptSubmit, PostToolUse, Stop
- MCP tools: search, timeline, get_observations
- DB: SQLite (WAL) + Chroma vector DB
- Source: ~/.claude/plugins/marketplaces/bpd1069/src/
