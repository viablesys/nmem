# nmem Project Memory

## Project Overview
- **Location**: `~/forge/nmem/`
- **Purpose**: Autonomous cross-session memory system, successor to claude-mem
- **Status**: Early design phase — no implementation yet
- **Organizing principle**: Viable System Model (VSM)
- **Key docs**: `DESIGN.md`, `ADR/CLAUDE.md`, `ADR/ADR-001` through `ADR-007`

## Key Findings (2026-02-08)

### sqlite-vec DELETE is broken
- DELETE on vec0 tables doesn't remove vector blobs (issue #178)
- rowid/vector data not cleared (issue #54, filed by author)
- VACUUM doesn't reclaim space (issue #220)
- No vacuum/optimize command exists yet (issues #184, #185)
- Open PR #243 for fix, not merged
- **Impact**: Forgetting/retention strategy cannot rely on DELETE if sqlite-vec is used

### sqlite-vec uses brute-force KNN, not HNSW
- ADR-001 incorrectly claims HNSW indexing
- Predecessor sqlite-vss (deprecated) used Faiss with HNSW
- sqlite-vec is pure C, brute-force + partition keys

### PRAGMA synchronous = NORMAL with WAL
- Safe from corruption, always consistent
- NOT durable on power loss (committed txns can roll back)
- Durable across application crashes (only OS/hardware failure matters)
- Acceptable trade-off for nmem (session notes, not critical data)

## Design Tensions
- Vector search may be unnecessary — structured retrieval (FTS5 + metadata) could suffice
- Litestream is overkill for a local developer tool
- Extraction strategy (structured vs LLM) determines whether sqlite-vec is needed
- Project recursion: one DB vs many (isolation vs query complexity)
- Vector index as derived artifact (rebuildable) vs primary store

## ADR Structure
- **ADR-001**: SQLite Extensions & Litestream — reviewed, annotated (sqlite-vec issues, Litestream overkill)
- **ADR-002**: Observation Extraction Strategy — stub, root decision in dependency chain
- **ADR-003**: Daemon Lifecycle — stub, depends on 002
- **ADR-004**: Project Scoping & Isolation — stub, depends on 002
- **ADR-005**: Forgetting Strategy — stub, depends on 002
- **ADR-006**: Interface Protocol — stub, independent
- **ADR-007**: Trust Boundary & Secrets Filtering — stub, depends on 003+004
- **ADR/CLAUDE.md**: Documents adversarial refinement approach and conventions
- Dependency order for expansion: 002 → 005 → 004 → 003 → 007 → 006

## Design Process
- Adversarial refinement: push assumptions to extremes, find where they break
- Questions > answers at this stage — insightful questions against practical assumptions
- Original ADR text never modified, only annotated with blockquotes
- Claims verified against primary sources before acceptance
- Scope check: nmem is local-first, single-developer — reject generic enterprise patterns

## Files Modified This Session
- `ADR/ADR-001-SQLite-Extensions-and-Litestream.md` — copied from Downloads, annotated with review
- `ADR/ADR-002` through `ADR-007` — stubs created with adversarial framing
- `ADR/CLAUDE.md` — created, documents approach and conventions
- `DESIGN.md` — annotated with storage-layer questions raised by ADR review
