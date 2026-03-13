# ADR-015: Fleet Beacon — Federated Query and RAG Distribution

## Status
Draft

## Framing
*How does a team of developers, each running their own local nmem, share cross-session memory and curated knowledge without centralizing their observations?*

This is a coordination problem (VSM S2 at the org level) with an intelligence layer (VSM S4 at the org level). Each developer accumulates local knowledge through their sessions — debugged errors, investigated APIs, designed solutions, curated library documentation. The valuable pattern exists in Alice's nmem but is invisible to Bob when he encounters the same problem. The failure mode is parallel re-derivation: every developer solving the same problem independently because their memory systems don't communicate.

The second problem is parallel knowledge curation: when Alice researches NATS via ensembled research (5 independent researchers, correlated, fact-checked, synthesized), Bob shouldn't repeat that work. The library doc is valuable to the whole team but locked to Alice's machine.

The adversarial angles:

**Against centralization:** "What if every team member's nmem is exposed to every other team member?" Push it against trust — does org-wide access collapse into a free-for-all, or does filtering at the source preserve safety? "What if a centralized index eliminates the federation entirely?" Push it against sovereignty — encryption key distribution, sync conflicts, compliance risk, operational burden. The VSM S3/S5 layers for a fleet database are unsolved social problems, not just technical ones.

**Against single-author docs:** "What if one developer researches a topic and writes the reference doc?" Push it against quality — confirmation bias, gaps from single search path, no diversity of sources, no independent validation. A single researcher optimizes for speed; an ensemble optimizes for accuracy.

**Against the mothership:** "What if the cloud blob store becomes the new single point of control?" Push it against the local-first principle — is the mothership a shared resource or a dependency? Does opt-in distribution mean developers who don't push get locked out of shared knowledge?

The dual constraint: session memory (observations, episodes, summaries) is privacy-sensitive, high-volume, per-developer — it must stay local. Curated knowledge (library docs, ensembled research) is public-by-construction, low-volume, team-shared — it can be centralized because it contains no observation data. The same federation mechanism handles both, but the privacy implications differ.

## Depends On
- ADR-001 (Storage Layer) — SQLite + SQLCipher encryption remains local, beacon doesn't touch it
- ADR-003 (Daemon Lifecycle) — no daemon constraint applies to beacon client architecture; relaxed for beacon infrastructure
- ADR-006 (Interface Protocol) — MCP tools and FTS5 query contracts are the foundation for federated queries
- ADR-007 (Trust Boundary and Secrets Filtering) — secret filtering happens locally before responses leave the instance
- ADR-010 (Work Unit Detection) — episode narratives are shareable context
- ADR-014 (Adaptive Capability) — beacon distributes capabilities across tiers; ensembled research as Tier 3

## Unlocks
- Cross-developer context retrieval: "who has debugged this error before?"
- Org-wide pattern detection: distributed failure archaeology across the fleet
- Attribution for free: GitHub identity tied to each nmem instance
- Query escalation: local first, beacon second when local context insufficient
- FTS5 query robustness improvement benefits local and federated queries
- Distributed ensemble research: fleet diversity as the ensemble signal
- Curated knowledge base: library docs distributed across the org with provenance tracking
- Single source of truth: mothership as authoritative blob store, instances as caches

---

## Context

### The single-developer ceiling

nmem's current retrieval scope is session-local (timeline, session_trace) or instance-local (search, session_summaries, file_history). A developer encountering an auth bug can query their own history. If a teammate solved the same bug last week, that knowledge is unreachable — it's in a different SQLite database on a different machine.

The team has shared knowledge distributed across individual memories AND individual libraries. nmem captures memory but can't retrieve it across instances. The library/ directory captures curation but can't distribute it. In VSM terms, each developer's nmem is a viable system (S1-S5), but at the team level there's no S2 (coordination) or S4 (intelligence synthesis across instances).

### The library curation ceiling

When a developer researches a new library using ensembled research (spawn 5 independent researchers, correlate findings, fact-check, synthesize), the output is a curated library doc representing significant research effort across multiple agents. If another developer on the team researches the same topic next week, they repeat the entire process — the library doc exists on Alice's machine but is invisible to Bob.

The ensemble research pattern has two constraints when applied locally:
1. **Sequential bottleneck.** One user spawns 5 researchers on their machine. If the fleet could distribute researchers across 5 machines, total time becomes max(researcher durations) instead of sum.
2. **Single-instance synthesis.** One developer produces a doc, stores it locally, and it's invisible to the rest of the team unless manually shared.

### Three architectural paths

**Path A: Centralized data store.** All observations pushed to a shared database. Query against the central corpus.

What breaks:
- **Encryption sovereignty.** Each nmem uses its own SQLCipher key. A central store either decrypts everything (eliminates local encryption) or holds N encrypted databases (no cross-instance queries).
- **Secret filtering asymmetry.** S5 filters at write time on each instance. A central store must either trust every instance's filtering or re-filter centrally.
- **Data residency.** Observations contain file paths, code snippets, commands. nmem's privacy model (your data stays on your disk) collapses.
- **Single point of failure.** Central DB down = no cross-team queries, no degradation.
- **Operational burden.** Who runs the central DB? Who defines retention? Who pays for storage?

**Path B: Query federation + RAG distribution (chosen).** Each nmem stays local. A beacon routes queries across instances using scatter/gather AND distributes shareable artifacts (library docs, research outputs) to opted-in instances.

Preserved properties:
- Data sovereignty — observations never leave the machine. Only query responses (matching result previews) are transmitted.
- Encryption sovereignty — each instance's DB remains encrypted with its own key.
- Secret filtering at source — each instance has already filtered secrets before storage.
- Graceful degradation — offline instances don't respond, partial results returned. Beacon offline → local operation continues.
- Opt-in contribution — instances choose what library docs to push. No forced centralization.

**Path C: P2P mesh.** Nmem instances discover each other directly, no central beacon.

What breaks:
- Discovery requires multicast (doesn't work across subnets), DHT (complex), or gossip (chatty).
- Every instance must know the full peer set and scatter queries itself.
- Hard to enforce org-level rules without a central authority.
- No natural place to synthesize ensemble results.

**Decision: Path B — query federation + RAG distribution.** The beacon is a query router and RAG coordinator, not a data store for observations.

## Architecture

### VSM Mapping

The fleet forms a recursive viable system. Each instance is a VSM (has its own S1-S5). The fleet is a VSM. The beacon itself is an nmem instance, so it has its own internal S1-S5. This is VSM recursion — viable systems composed of viable systems (Beer, 1972).

| Layer | Local (per instance) | Fleet (via beacon) |
|-------|---------------------|-------------------|
| **S1 Operations** | Observation capture, storage, retrieval | Query federation, RAG distribution |
| **S2 Coordination** | Dedup, ordering, classification | NATS message routing — no state, no intelligence |
| **S3 Control** | Retention, compaction, maintenance | Fleet-wide maintenance policies (future) |
| **S3* Audit** | Integrity checks | Mothership — canonical storage, single source of truth for RAG |
| **S4 Intelligence** | Context injection, task dispatch, episodic memory | Beacon — cross-instance synthesis, ensemble coordination, pattern detection |
| **S5 Policy** | Config, identity, secret filtering | Org boundary, trust model, opt-in distribution policies |

**Key insight:** The beacon is S4 at the fleet level. It synthesizes across instances the same way local S4 synthesizes across sessions. NATS is S2 — pure coordination, no memory. The mothership is S3* (audit/archive) not S3 (control) — it has no enforcement mechanism; instances choose whether to push.

### Beacon as nmem Instance

**Decision (resolves v1.0 Open Question Q1):** The beacon is not a separate service. It's an nmem instance running the same codebase, using the same `nmem.db` schema, with additional responsibilities.

**What makes it the beacon:**
1. **Always-on NATS subscription.** Unlike user instances (session-scoped), the beacon stays connected via systemd or cron heartbeat.
2. **Synthesis capability.** Performs ensemble synthesis for distributed research tasks.
3. **Mothership sync.** Pulls from and pushes to the cloud blob store.
4. **Fleet coordination.** Responds to queries, distributes RAG docs, orchestrates research.

**What it's NOT:**
- Not a separate binary — it's `nmem` with `[beacon] mode = "dedicated"` in config.
- Not a data store — it has its own `nmem.db` for its sessions, but doesn't aggregate other instances' observations.
- Not an authority — it distributes what instances contribute, doesn't curate or edit.

**Deployment:** VPS, EC2 instance, Docker container, or always-on dev machine. Just nmem in always-on mode.

**ADR-003 tension:** The beacon is a daemon, violating ADR-003's no-daemon rule. Resolution: ADR-003 applies to user-facing nmem instances. The beacon is infrastructure, explicitly opt-in (one instance in the fleet plays the role). The daemon constraint is relaxed for the beacon only.

### NATS Request/Reply as the Message Bus

**Why NATS:**
- **Native scatter/gather.** Request to subject, all subscribed instances respond independently. No custom routing.
- **No beacon state.** NATS handles message routing. The beacon client is a thin layer.
- **Auth built-in.** NATS 2.10+ supports auth callout to an external validator (GitHub org membership check).
- **Lightweight.** Single Go binary (<20MB), stateless message routing, no persistent storage needed for core NATS.
- **Observable.** Monitoring endpoints (`/varz`, `/connz`) export connection and message stats.
- **Pub/sub + request/reply.** Query federation uses request/reply (scatter/gather). RAG distribution uses pub/sub (push to all subscribers).

**Against alternatives:**

| Technology | Scatter/Gather | Pub/Sub | Auth | Verdict |
|-----------|---------------|---------|------|---------|
| HTTP poll | No — can't push to instances | No | OAuth | Can't scatter, can't push |
| WebSocket | Yes | Yes | OAuth + WS upgrade | Viable but stateful |
| gRPC bidir stream | Yes | Yes | mTLS or JWT | Heavyweight |
| **NATS request/reply + pub/sub** | **Yes — native** | **Yes — native** | **JWT + NKey** | **Chosen** |

**Architectural invariant:** The beacon never stores observations. If the beacon or NATS is down, individual nmems continue to function locally — only cross-instance queries and RAG distribution are unavailable.

### Query Flow (Session Memory)

```
Agent (via MCP) → nmem serve (local)
                     ↓ (search local first)
                     ↓ (if insufficient or fleet=true, escalate)
                     ↓
                  NATS publish (nmem.{org}.search)
                     ↓
              ┌──────┴──────┬──────────┬─────────┐
              ↓             ↓          ↓         ↓
         nmem@alice   nmem@bob   nmem@eve   nmem@charlie
              ↓             ↓          ↓         ↓
         FTS5 query    (offline)   FTS5 query   FTS5 query
              ↓                       ↓         ↓
         (no results)            3 results   1 result
              ↓                       ↓         ↓
              └───────────────────────┴─────────┘
                     ↓
              Response aggregation (merge, dedup, re-rank)
                     ↓
              nmem serve (local) → Agent
```

1. Developer queries locally first (existing `search` tool).
2. If local results insufficient or `fleet=true`, query is published to NATS subject.
3. NATS delivers to all subscribed instances.
4. Each instance runs local FTS5 search (same SQL as `s1_serve.rs`), returns top results.
5. Querying instance collects responses within timeout window.
6. Results are pooled, deduplicated by `(instance_id, session_id, observation_id)`, re-ranked by composite score, and returned.

**Timeout:** NATS request/reply with configurable timeout (default: 3 seconds). Offline or slow instances simply don't respond — no blocking.

### RAG Distribution Flow

```
Developer creates library doc → ~/workspace/library/nats.md
                                        ↓
                                nmem rag push nats.md
                                        ↓
                              Upload to mothership (S3/R2)
                              Write RAG metadata to local nmem.db
                                        ↓
                                  NATS publish (nmem.{org}.rag.new)
                                        ↓
                              ┌─────────┴─────────┬──────────┐
                              ↓                   ↓          ↓
                         nmem@bob            nmem@eve   nmem@charlie
                              ↓                   ↓          ↓
                         (opt-in pull)       (opt-in pull) (offline)
                              ↓                   ↓
                         Pull from mothership    Pull from mothership
                              ↓                   ↓
                         library/nats.md     library/nats.md
                         + metadata in db    + metadata in db
```

**Mothership is the single source of truth.** If it's not in the mothership, it's not available to the fleet. The mothership is a cloud blob store (S3/R2/GCS) with versioning.

**Opt-in at every layer:**
- Push to mothership: explicit command (`nmem rag push`), never automatic.
- Pull from mothership: configurable, explicit or on notification.
- Respond to queries: per-instance config.
- Participate in research: per-instance config.

### Ensemble as Beacon Capability

The ensembled-research skill (from claude-plugins) spawns N independent researchers on the same topic, synthesizes results, and produces a single high-quality library doc. For fleet execution, the beacon distributes the research prompt over NATS:

```
Beacon receives research request
    ↓
NATS publish (nmem.{org}.research.request) with identical prompt
    ↓
┌──────┴──────┬─────────┬─────────┐
↓             ↓         ↓         ↓
nmem@alice  nmem@bob  nmem@eve  nmem@charlie
    ↓             ↓         ↓         ↓
Researches  (offline)  Researches  Researches
    ↓                       ↓         ↓
Returns findings        Returns      Returns
    ↓                       ↓         ↓
    └───────────────────────┴─────────┘
                ↓
        Beacon correlates (Phase 3)
        Beacon fact-checks (Phase 4)
        Beacon synthesizes (Phase 5)
                ↓
        library/{topic}.md + metadata
                ↓
        Push to mothership
                ↓
        NATS publish (nmem.{org}.rag.new)
                ↓
        Fleet instances pull
```

**Why this works:**
- **Fleet diversity IS ensemble diversity.** Different instances have different contexts, different prior research, different local library docs. The researchers genuinely explore different paths.
- **Load distribution.** Research is expensive (web search, LLM calls). Fleet ensemble distributes the load across N machines.
- **Time compression.** Total time is max(researcher durations), not sum.

**Topology fingerprint for fleet ensembles:** `5f` (5 fleet instances contributed) vs `5` (5 local subagents). The `f` suffix distinguishes distributed topology from local. Augmented fleet ensembles: `5f×3` (5 fleet base + 3 augmented). Maximum: `5f×3×3` (11 fleet researchers).

**Degradation:** If only 3 of 5 instances respond within timeout, the ensemble proceeds with `3f`. The correlation phase adapts to fewer researchers — quality degrades gracefully.

### Identity Model

**One user = one nmem = one dev machine.** This is a design constraint.

- Each nmem instance is tied to a single GitHub user via OAuth.
- Responses include the developer's GitHub username for attribution.
- Multi-machine users choose their primary. No cross-machine unification.
- Attribution is free — every federated observation and RAG contribution carries the source developer's identity.

### Response Merging and Ranking

Querying instance merges responses from all respondents:

- **BM25 normalization.** FTS5 rank values are corpus-dependent and not comparable across instances (different corpus sizes, different term frequencies). Min-max normalization across all responses makes scores comparable.
- **Composite score:** `(0.5 * bm25_normalized) + (0.3 * recency_decay) + (0.2 * source_weight)`. Recency uses the same exponential decay with 7-day half-life as `recent_context` (ADR-006).
- **Deduplication.** Observations with identical content (after normalization) are deduplicated, keeping the highest-scoring version.
- **Limit.** Return top N results (default 50, configurable). Per-instance responses are already limited (default 20 per instance) to reduce network volume.

### Discovery: Automatic via NATS Subscription

No explicit registration. When an nmem instance enables beacon mode, it subscribes to the fleet's NATS subject. The subscription itself is the registration. NATS handles discovery — subscribers appear when they connect, disappear when they disconnect. No registry to maintain, no stale entries.

### Opt-in Model

`~/.nmem/config.toml` controls what an instance shares:

```toml
[beacon]
respond = true              # respond to federated search queries
respond_to_research = true  # participate in fleet ensemble research

[beacon.rag]
push = false                # push library docs to mothership (default off)
pull = true                 # pull library docs from mothership
```

A developer can:
- Query the fleet but not respond
- Respond to research requests but not searches
- Pull library docs but not push
- Fully participate in everything

Default: respond to everything, push nothing (until explicitly configured).

## Mothership: Cloud Blob Store

The mothership is a cloud blob store (S3-compatible). It stores library docs and their metadata. It does NOT store observations.

**Invariant:** If it's not in the mothership, it's not available to the fleet.

**Access control:** Authenticated via GitHub OAuth token. Same token used for NATS auth works for mothership upload/download.

**Cost estimate (10-developer org):** 50 library docs × 100KB avg = 5MB. S3 pricing: $0.023/GB/month = $0.0002/month. Bandwidth negligible for incremental pulls. Mothership cost is effectively zero for small orgs.

**Why cloud, not self-hosted:** Zero ops for small teams. S3/R2 is managed, durable, globally distributed. Migration path: start with S3/R2, swap to MinIO if self-hosting is needed (same API).

**Offline resilience:** If the beacon is down, the mothership still serves docs via direct HTTP. Instances can sync from mothership without the beacon — the beacon is an optimization for notification, not the only distribution path.

## RAG Metadata Schema

New table in `nmem.db` (same schema on every instance, including beacon):

```sql
CREATE TABLE library_docs (
    id INTEGER PRIMARY KEY,
    filename TEXT NOT NULL UNIQUE,       -- e.g., "nats.md"
    title TEXT,                          -- e.g., "NATS Request/Reply Patterns"
    version INTEGER NOT NULL DEFAULT 1,  -- increments on update
    topology TEXT NOT NULL,              -- "5", "5f", "5f×3", "5×3×3"
    created_at INTEGER NOT NULL,
    updated_at INTEGER,
    hash TEXT NOT NULL,                  -- SHA-256 of file content
    size_bytes INTEGER,
    author TEXT,                         -- GitHub username of original author
    contributors TEXT,                   -- JSON array of GitHub usernames
    tags TEXT,                           -- JSON array: ["rust", "messaging", "nats"]
    mothership_url TEXT,                 -- S3/R2 URL (NULL if local-only)
    local_path TEXT NOT NULL,            -- ~/workspace/library/{filename}
    source TEXT NOT NULL DEFAULT 'local',-- 'local' | 'mothership'

    -- Ensemble quality signal (computed once at synthesis, never recomputed)
    agents INTEGER NOT NULL,             -- number of ensemble agents (min 5)
    total_claims INTEGER,                -- total claims extracted during correlation
    agreement_distribution TEXT,         -- JSON: {"5":40,"4":8,"3":2} — claims per agreement tier
    p_hat REAL,                          -- estimated single-agent accuracy (Condorcet MLE)
    ensemble_acc REAL,                   -- majority-vote ensemble accuracy from p_hat
    verified_total INTEGER,              -- claims tagged [verified] by agents
    verified_wrong INTEGER,              -- [verified] claims that failed fact-check
    corrections INTEGER,                 -- total claims corrected during fact-check
    calibration REAL,                    -- 1 - (verified_wrong / verified_total)
    q_final REAL                         -- ensemble_acc × calibration (post-correction confidence)
);

CREATE INDEX idx_library_docs_tags ON library_docs(tags);
CREATE INDEX idx_library_docs_hash ON library_docs(hash);
CREATE INDEX idx_library_docs_updated ON library_docs(updated_at DESC);
```

**Why in nmem.db, not a separate DB:** Same schema everywhere. Every nmem instance can query `library_docs` locally. The beacon is just an nmem instance — it uses the same schema. No separate beacon.db to maintain.

**Library doc frontmatter:** Library docs include YAML frontmatter for metadata that travels with the file:

```markdown
---
title: NATS Request/Reply Patterns
author: alice
topology: 5f
contributors: [alice, bob, eve]
tags: [rust, messaging, nats, distributed]
created: 2026-03-12T10:30:00Z
quality:
  agents: 5
  p_hat: 0.947
  ensemble_acc: 0.997
  calibration: 0.80
  corrections: 10
  q_final: 0.798
---

# NATS Request/Reply Patterns
...
```

The frontmatter is parsed at push time and synced to the `library_docs` table. The DB is the queryable index; the frontmatter is the portable metadata.

**Quality signal computation (performed once at synthesis):**

1. **Correlation phase** extracts claims and counts agreement per tier (k/N agents agreeing)
2. **p_hat** — single-agent accuracy estimated from agreement distribution via Condorcet MLE: fit `p` to observed tier fractions against `E[k/N] = C(N,k)·pᵏ·(1-p)^(N-k)`. Back-of-napkin shortcut: `p̂ ≈ mean(max(kᵢ, N-kᵢ) / N)`
3. **ensemble_acc** — majority-vote accuracy: `Σ_{k=⌈N/2⌉}^{N} C(N,k)·p̂ᵏ·(1-p̂)^(N-k)`
4. **calibration** — `1 - (verified_wrong / verified_total)`. Agents self-tag claims as `[verified]` or `[unverified]` via prompt. Only `[verified]` + wrong counts as calibration error. `[unverified]` tags are the system working correctly — requesting fact-check
5. **q_final** — `ensemble_acc × calibration`. Post-correction confidence. This is the number people see

**Interpretation:** `q_final: 0.997` means "99.7% confident after fact-checking." The decomposition is stored for auditability — anyone can drill from the headline number to the raw agreement distribution.

**Observed baselines from real ensemble runs:**

| Run | N | Type | Claims | Corrections | p_hat | ensemble_acc | calibration | q_final |
|-----|---|------|--------|-------------|-------|-------------|-------------|---------|
| library docs (5 topics) | 5 | research | ~190 | 10 | 0.947 | 99.7% | 0.80 | 0.798 raw / 0.997 post-correction |
| ADR-015 rewrite | 5 | design | ~13 | 0 | 1.0 (arch) | 1.0 | 1.0 | 1.0 |
| Helm/Flux question | 7 | Q&A | 14 | 0 | 1.0 (core) | 1.0 | 1.0 | 1.0 |

**Pattern:** Error clusters in numerical/factual claims (sizes, speeds, version numbers), not architectural or mechanistic claims. Design and Q&A ensembles achieve q_final = 1.0 because the claims are structural — overdetermined by constraints. Research docs hit ~95% single-agent accuracy because they contain numerical facts the model is less certain about.

**Implication for ensemble sizing:** N=5 is sufficient for research (lifts p=0.947 to 99.7%). N=7 adds marginal accuracy but provides stronger disagreement signal for edge cases. For binary yes/no questions with structural answers, even N=5 yields unanimous agreement — the ensemble's value there is confidence certification, not error correction.

**MCP tools for RAG:**
- `list_library_docs` — list locally available library docs with metadata
- `search_library_docs` — FTS5 search over library doc content
- `rag_push` — upload local library doc to mothership
- `rag_pull` — download doc from mothership by filename

## Authentication: GitHub Org SSO via OAuth Device Flow

### Flow

1. `nmem beacon login` initiates OAuth Device Flow (RFC 8628) against a GitHub OAuth App registered to the `viablesys` org.
2. User receives a one-time code, opens browser, authorizes. Works on headless dev machines — no local HTTP server needed.
3. nmem polls GitHub's token endpoint until authorization completes.
4. nmem validates org membership: `GET /orgs/viablesys/members/{username}`. Returns 204 if member, 404 if not.
5. If member → beacon issues NATS credentials (JWT via auth callout).
6. NATS credentials stored locally in system keyring (GNOME Keyring on Linux, Keychain on macOS) with plaintext config fallback.
7. All future NATS connections use the stored credentials.

### GitHub OAuth App Details

- **Client ID is public.** Embedded in the nmem binary. `gh` CLI does the same (`178c6fc778ccc68e1d6a`).
- **Scopes:** `read:org` (org membership check), `read:user` (get username).
- **Client secret not required for Device Flow.** The flow is designed for public clients.
- **Revocation:** When a user leaves the org, their membership check fails on re-login. No beacon-side user table to maintain.

### NATS Auth Callout (NATS 2.10+)

NATS supports authorization callouts — when a client connects, NATS calls out to an HTTP service with the client's credentials. The service validates and returns allow/deny.

1. nmem connects to NATS with GitHub OAuth token as bearer credential.
2. NATS calls out to auth service (lightweight HTTP, stateless).
3. Auth service validates: token is valid GitHub OAuth token + user is member of configured org.
4. If valid: issues NATS JWT with permissions to pub/sub on `nmem.{org}.>` subjects.
5. If invalid: connection denied.

The auth service caches GitHub API responses (username → org membership) to avoid rate limits. Stateless — no database, no user table.

### Trust Model: "If You Can Auth, You're In"

No per-query ACLs. No capability tokens. Org membership is the gate.

**Rationale:** Teams that use nmem together trust each other with shared context. The threat is external access, not teammate curiosity. The alternative (per-developer or per-project visibility policies) adds S5 complexity without addressing the real threat model for a small trusted org.

**Defense layers against secret leakage:**

1. **Instance-level filtering (ADR-007).** Every observation passes through `SecretFilter` before storage. Regex patterns + Shannon entropy. Credentials should never be stored in the first place.
2. **Org boundary.** Only org members can query. The org is the trust boundary.
3. **No read-your-writes amplification.** If a credential slipped past filtering, it's already in the local DB — the beacon doesn't increase exposure to the developer who owns it.
4. **Library docs are public-by-construction.** They're curated references, not observation data. Opt-in push means sensitive research stays local unless explicitly shared.

## FTS5 Query Robustness

### The Problem

Current `sanitize_fts_query` (`lib.rs:123`) passes multi-word input as implicit AND to FTS5 MATCH. All terms must appear in the same observation. Agents degrade word-by-word until they get results, often landing on a single generic term — high recall, zero precision.

Over NATS, this retry loop is unacceptable. The query must be good on the first shot — network latency and multi-instance response aggregation make trial-and-error prohibitively expensive.

### Tiered Fallback Strategy

Application-layer query rewriting. The rewrite happens in Rust before the query hits FTS5.

**Tiers (try in order, stop at first tier with results):**

1. **Exact phrase:** `"session summary backfill"` — cheapest, highest precision
2. **AND (current default):** `session summary backfill` — all terms present, any order
3. **OR with BM25 ranking:** `session OR summary OR backfill` — partial matches, ranked by relevance
4. **Prefix expansion:** `session* OR summary* OR backfill*` — broadest, needs prefix indexes

**Additional techniques:**
- **Stopword stripping** before tier construction — remove common words (`the`, `a`, `is`, `for`, `and`).
- **Prefix indexes** on FTS5 table: `prefix='2,3,4'` trades index size (dataset-dependent overhead) for prefix query speed.
- **Column weighting** in BM25: `content` higher weight than `file_path`.

**Implementation:** New function `rewrite_query(input: &str) -> Vec<String>` in `lib.rs`. Returns a Vec of query tiers. Callers (`s1_search.rs`, `s1_serve.rs`, beacon query handler) iterate until a tier returns results.

**Schema change required:** Add `prefix='2,3,4'` to `observations_fts` in `schema.rs`. Existing databases need FTS rebuild: `INSERT INTO observations_fts(observations_fts) VALUES('rebuild')`.

**Key insight:** This improvement benefits local search immediately. It's not beacon-specific but becomes critical when queries are federated.

## Secret Filtering Enhancement: TruffleHog Integration

### Motivation

Fleet federation amplifies exposure risk. A secret that leaks into one instance's DB is bad. A secret that leaks into fleet query responses propagates to every querying developer. Knowledge distribution amplifies further — a secret in a library doc pushed to the mothership becomes org-wide.

### Plan

TruffleHog is an open-source secret scanner with 800+ detector patterns covering API keys, tokens, credentials across major services. Its detectors are battle-tested against real credential leaks in public repos.

**Integration:**

1. **Build-time extraction script** (`tools/extract-trufflehog-patterns.py`) pulls detector definitions from TruffleHog's GitHub repo, extracts regex patterns.
2. **Generate Rust source** (`src/s5_trufflehog_patterns.rs`) with patterns as a `RegexSet`.
3. **Merge into `s5_filter.rs`.** Existing `SecretFilter` already uses `RegexSet` — integration is expanding the pattern list, not changing architecture.
4. **Update cadence:** Re-run extraction script periodically (monthly or on-demand). Commit generated source. No runtime dependency on TruffleHog.

**No network verification.** TruffleHog's verification engine makes API calls to check if credentials are live. nmem skips this — a regex match is sufficient to redact. Filtering happens at write time (PostToolUse hook), so federated responses and library docs are already filtered.

**False positive trade-off:** TruffleHog optimizes for recall. Shannon entropy detection in `s5_filter.rs` provides the precision signal — a match that also has high entropy is more likely to be a real secret.

## NATS Subject Hierarchy

**Decision:** Two-level hierarchy with org scope, extended for RAG and research.

### Query Federation
- **Broadcast:** `nmem.{org}.search` — all instances in the org receive the query
- **Targeted (future):** `nmem.{org}.{username}.search` — query a specific developer's instance
- **Each instance subscribes to both:** `nmem.{org}.search` (org-wide) and `nmem.{org}.{username}.search` (user-specific)

### RAG Distribution
- **New doc notification:** `nmem.{org}.rag.new` — published when a library doc is pushed to mothership
- **Doc deletion:** `nmem.{org}.rag.deleted` — tombstone notification when a doc is removed

### Research Coordination
- **Research request:** `nmem.{org}.research.request` — beacon distributes ensemble research tasks
- **Research response:** `nmem.{org}.research.response.{task_id}` — instances return findings

**Rationale:** Structured enough for org isolation (different orgs on the same NATS server don't see each other's traffic), flat enough within the org to avoid routing complexity. Team subdivision deferred — it's an S5 policy question without a clear use case yet. Org boundary is sufficient for viablesys.

## Open Questions

### Q1: ~~Beacon deployment model~~ RESOLVED

The beacon is an nmem instance with `[beacon] mode = "dedicated"` in config, running with a systemd timer or cron heartbeat. Same codebase, same schema. See "Beacon as nmem Instance" section.

### Q2: ADR-003 daemon constraint — how does the beacon client run?

**For user instances (resolved):** Beacon subscription lives in the MCP server (`nmem serve`). Session-scoped — when the Claude Code session ends, the subscription ends. No new process.

**For the beacon itself (resolved):** Runs as a long-lived process via systemd or cron. This explicitly relaxes ADR-003 for the beacon only — the beacon is infrastructure, not a user tool.

**Optional for power users:** `nmem beacon connect` as an explicit opt-in long-lived process on a developer machine. Available for users who want always-on availability.

### Q3: What happens when instances are offline?

NATS request/reply with timeout. Offline instances don't respond. Beacon returns results from online instances only.

**Consequence:** Partial results for federated search. No feedback about what's missing.

**Mitigation via RAG:** Library docs are in the mothership regardless of instance availability. Only ephemeral observations are affected by offline instances. Session summaries could be opt-in pushed to the mothership for offline resilience (deferred — adds complexity, unclear value).

### Q4: Response volume at scale?

50 instances × 20 results each = 1000 observations to merge. Most will be low-relevance.

**Mitigations:**
- Per-instance limit (each returns top 20 max).
- BM25 score threshold: instances only respond if their best match exceeds minimum relevance.
- NATS timeout: slow instances excluded naturally.

**Current lean:** Ship without a relevance threshold. The tiered query fallback already improves precision. Add threshold if volume becomes a problem in practice.

### Q5: Version skew across instances?

If Alice runs nmem v1.0 and Bob runs v1.2, response schemas may differ.

**Current lean:** Ignore initially. The observation schema (ADR-002) is stable. RAG metadata schema includes `version` column for per-doc versioning. Add schema versioning in responses (`"schema_version": "1.0"`) when breaking changes are needed.

### Q6: Per-project visibility policies?

Should a developer working on `internal-security-tool` have their observations queryable by the entire org?

**Current lean:** Defer. Org membership is the trust boundary. Per-project opt-out via config: `[projects.internal-security-tool] beacon_respond = false`. RAG supports opt-out via `push = false` — if you shouldn't share it, don't push it. This is an S5 (policy) decision.

### Q7: RAG doc conflict resolution?

Two developers independently produce `library/nats.md` and push to the mothership. Which version wins?

**Option A: Last-write-wins.** Simple but risks losing work.

**Option B: Hash-based conflict detection (chosen).** Mothership detects hash mismatch, responds with HTTP 409 and the existing doc's metadata. Developer pulls, reviews, decides whether to merge or replace.

**Option C: Automatic ensemble merge.** Treat the conflict as additional researchers, correlate, synthesize. Sophisticated but fragile — ensemble synthesis assumes independent researchers, not conflicting editors.

**Current lean:** Option B. Conflicts should be rare for curated docs. When they occur, the developer resolves manually. If ensembling is desired, the developer can run `nmem ensemble --merge` locally to synthesize both versions.

### Q8: RAG doc deletion and tombstones?

When a doc is deprecated or superseded, how does the fleet handle it?

**Current lean:** Tombstone records. Beacon publishes `nmem.{org}.rag.deleted` with `filename`. Instances remove from `library/` and soft-delete in `library_docs` (preserves provenance). Mothership moves to archive prefix. No hard deletes — provenance is preserved.

### Q9: Mothership storage choice?

**Current lean:** Cloudflare R2 for zero egress (fleet instances pulling frequently). Use S3-compatible client libraries (`aws-sdk-s3` for Rust) for portability. Swap to AWS S3, GCS, or MinIO without changing client code.

## Constraints

What's ruled out by existing decisions:

| Constraint | Source | Implication |
|-----------|--------|-------------|
| No centralized observation store | ADR-007, VSM S1 | Beacon routes queries, never stores observations |
| Observations encrypted at rest locally | ADR-001 | Beacon never decrypts — responses are pre-filtered |
| Secret filtering at write time | ADR-007 | Federated responses and library docs are post-filter |
| FTS5 is the query engine | ADR-001 | Beacon federates FTS5 queries, no vector search |
| No new heavy dependencies | Project policy | `async-nats` crate is the only new Cargo dependency |
| MCP tools read-only | ADR-006 | Fleet queries are read-only, no federation of writes |
| Org boundary = trust boundary | This ADR | No per-project ACLs in initial version |
| Beacon is nmem | This ADR | Same codebase, same schema, same `nmem.db` |
| Mothership is single source of truth | This ADR | RAG docs not in mothership are not available to fleet |
| Opt-in contribution | This ADR | Instances choose what to push, no forced centralization |
| Library docs are public-by-construction | This ADR | No observation data in library/ content |

## Consequences

### Positive
- **Data sovereignty preserved.** Observations never leave the dev machine. Federation is query-only.
- **Graceful degradation.** Beacon offline → local queries still work. Instances offline → partial results. Mothership offline → local library cache serves.
- **Zero sync conflicts for observations.** No replication, no eventual consistency, no CRDTs.
- **Simple trust model.** Org membership is the gate. No per-query ACLs, no capability tokens, no user table.
- **Attribution for free.** Every federated response and RAG contribution carries the developer's GitHub username.
- **Operational simplicity.** NATS is stateless routing. Mothership is managed blob storage. No backups or migrations for either.
- **Local-first stays viable.** Beacon is an enhancement, not a requirement. nmem works standalone.
- **FTS5 query robustness benefits everyone.** Tiered fallback improves local search even without fleet federation.
- **Secret filtering strengthened.** TruffleHog integration benefits local instances and reduces fleet/mothership exposure risk.
- **Fleet diversity as ensemble signal.** Distributed research leverages the team's different contexts as genuine independent paths.
- **Single codebase.** Beacon is nmem. No separate binary, schema, or tooling to maintain.
- **Curated knowledge base.** Library docs are versioned, provenance-tracked, and queryable via metadata.
- **RAG metadata queryability.** "Which docs cover NATS?" answered locally without round-tripping to every instance.
- **Opt-in at every layer.** Developers control what they share. No surprise propagation.
- **VSM coherence.** Beacon as S4 maps cleanly to local S4. Same pattern at different scale (Beer recursion).

### Negative
- **Query latency.** Network round-trip per instance. Acceptable for MCP queries, noticeable for interactive use.
- **Partial failures.** Offline instances → incomplete search results. No feedback about what's missing.
- **No retry loops over NATS.** Query must be good on first shot. Mitigated by tiered query rewriting.
- **NATS operational burden.** Someone runs the NATS server. Mitigated by NATS being a single binary.
- **GitHub OAuth dependency.** Auth requires GitHub. Can't federate across orgs without multiple OAuth Apps.
- **Secret filtering is best-effort.** 800+ TruffleHog patterns aren't exhaustive. Mitigated by entropy detection.
- **Beacon availability matters.** Beacon offline → no fleet queries, no RAG distribution. Mitigated by running beacon as infrastructure. Future: multi-beacon for redundancy.
- **Mothership is an external dependency.** Cloud blob storage adds cost (negligible) and availability concern. Mitigated by local library cache.
- **RAG conflict resolution is manual.** Two developers pushing the same filename requires human intervention.
- **Beacon is a daemon.** Relaxes ADR-003. Justified as infrastructure-only exception.
- **Synthesis quality risk.** Fleet ensemble synthesis may produce lower quality than manual. Mitigated by correlation/fact-checking phases and optional human approval.

## References

- ADR-001 — Storage Layer (SQLite local-first, SQLCipher encryption, FTS5)
- ADR-003 — Daemon Lifecycle (no daemon constraint, process model — relaxed for beacon)
- ADR-006 — Interface Protocol (MCP tools, FTS5 query handling, search contracts)
- ADR-007 — Trust Boundary and Secrets Filtering (S5 regex + entropy)
- ADR-010 — Work Unit Detection (episodes, narratives, session-level synthesis)
- ADR-011 — Phase Classification (inference performance budget)
- ADR-013 — Scope Classification (converge/diverge, same architecture)
- ADR-014 — Adaptive Capability (capability tiers, backfill, ensembled research as Tier 3)
- RFC 8628 — OAuth 2.0 Device Authorization Grant
- Beer, S. (1972) — Brain of the Firm (VSM recursion, S4 as intelligence at multiple scales)
- NATS documentation — https://docs.nats.io/ (request/reply, pub/sub, JWT auth, auth callout)
- TruffleHog — https://github.com/trufflesecurity/trufflehog (800+ secret detector patterns)
- SQLite FTS5 — https://sqlite.org/fts5.html (MATCH syntax, BM25 ranking, prefix indexes)
- `gh` CLI OAuth — https://github.com/cli/cli (Device Flow reference implementation)
- Ensembled Research Skill — `claude-plugins/ensembled-research/skills/ensembled-research/SKILL.md`

## Revision History

| Date | Version | Changes |
|------|---------|---------|
| 2026-03-10 | 1.0 | Initial. Architecture: query federation not data store. NATS request/reply for scatter/gather. GitHub org SSO via OAuth Device Flow. One user = one nmem = one machine. NATS subject hierarchy: `nmem.{org}.search` for broadcast, `nmem.{org}.{username}.search` for targeted. FTS5 tiered fallback: phrase → AND → OR → prefix, `rewrite_query()` function, prefix indexes, stopword stripping. TruffleHog integration (800+ patterns) for s5_filter.rs. Trust model: org membership = full query access. Response merging with BM25 normalization + recency + source weighting. Beacon opt-out via config. Six open questions: beacon deployment model, ADR-003 daemon tension, offline resilience, response volume, version skew, per-project visibility. |
| 2026-03-12 | 2.0 | **Major expansion: RAG distribution and fleet ensemble.** Beacon is an nmem instance (resolves Q1) — same codebase, same schema, cron heartbeat, `[beacon] mode = "dedicated"`. ADR-003 relaxed for beacon (infrastructure exception). Mothership (S3-compatible blob store) as single source of truth for library docs. `library/` directory alongside `models/` and `nmem.db`. `library_docs` table in nmem.db with topology, provenance, contributors, correlation stats, confidence score, tags, hash. YAML frontmatter in library doc markdown files. Fleet ensemble research: beacon distributes identical prompts over NATS, instances respond independently, beacon synthesizes. Topology fingerprint extended: `5f` suffix for fleet (vs `5` local), augmented: `5f×3`, max `5f×3×3`. Opt-in model for push/pull/respond. NATS subjects extended: `rag.new`, `rag.deleted`, `research.request/response`. MCP tools: `list_library_docs`, `search_library_docs`, `rag_push`, `rag_pull`. VSM mapping: beacon=S4 (fleet intelligence), NATS=S2 (coordination), mothership=S3* (audit). New open questions: conflict resolution (hash-based detection, manual merge), tombstones (soft delete), mothership choice (R2 for zero egress). Ensemble synthesized from 5 independent researchers (topology `5`). |
