# ADR-015: Fleet Beacon — Federated Query for Distributed nmem Instances

## Status
Draft

## Framing
*How does a team of developers, each running their own local nmem, share cross-session memory without centralizing their observations?*

This is a coordination problem (VSM S2 at the org level). Each developer accumulates local knowledge through their sessions — debugged errors, investigated APIs, designed solutions. The valuable pattern exists in Alice's nmem but is invisible to Bob when he encounters the same problem. The failure mode is parallel re-derivation: every developer solving the same problem independently because their memory systems don't communicate.

The adversarial angle: "What if every team member's nmem is exposed to every other team member?" Push it against trust — does org-wide access collapse into a free-for-all, or does filtering at the source preserve safety? Also: "What if a centralized index eliminates the federation entirely?" Push it against sovereignty — encryption key distribution, sync conflicts, compliance risk, operational burden. The VSM S3/S5 layers for a fleet database are unsolved social problems, not just technical ones.

The constraint: nmem's privacy model (ADR-007) assumes observations never leave the local machine. Fleet beacon must federate queries without centralizing data.

## Depends On
- ADR-001 (Storage Layer) — SQLite + SQLCipher encryption remains local, beacon doesn't touch it
- ADR-003 (Daemon Lifecycle) — no daemon constraint applies to beacon client architecture
- ADR-006 (Interface Protocol) — MCP tools and FTS5 query contracts are the foundation for federated queries
- ADR-007 (Trust Boundary and Secrets Filtering) — secret filtering happens locally before responses leave the instance

## Unlocks
- Cross-developer context retrieval: "who has debugged this error before?"
- Org-wide pattern detection: distributed failure archaeology across the fleet
- Attribution for free: GitHub identity tied to each nmem instance
- Query escalation: local first, beacon second when local context insufficient
- FTS5 query robustness improvement benefits local and federated queries

---

## Context

### The single-developer ceiling

nmem's current retrieval scope is session-local (timeline, session_trace) or instance-local (search, session_summaries, file_history). A developer encountering an auth bug can query their own history. If a teammate solved the same bug last week, that knowledge is unreachable — it's in a different SQLite database on a different machine.

The team has shared knowledge distributed across individual memories. nmem captures it but can't retrieve it. In VSM terms, each developer's nmem is a viable system (S1-S5), but at the team level there's no S2 — no coordination layer. Developers operate in parallel, unaware of each other's memory state.

### Three architectural paths

**Path A: Centralized data store.** All observations pushed to a shared database. Query against the central corpus.

What breaks:
- **Encryption sovereignty.** Each nmem uses its own SQLCipher key. A central store either decrypts everything (eliminates local encryption) or holds N encrypted databases (no cross-instance queries).
- **Secret filtering asymmetry.** S5 filters at write time on each instance. A central store must either trust every instance's filtering or re-filter centrally.
- **Data residency.** Observations contain file paths, code snippets, commands. nmem's privacy model (your data stays on your disk) collapses.
- **Single point of failure.** Central DB down = no cross-team queries, no degradation.
- **Operational burden.** Who runs the central DB? Who defines retention? Who pays for storage?

**Path B: Query federation (chosen).** Each nmem stays local. A beacon routes queries across instances using scatter/gather.

Preserved properties:
- Data sovereignty — observations never leave the machine. Only query responses (matching result previews) are transmitted.
- Encryption sovereignty — each instance's DB remains encrypted with its own key.
- Secret filtering at source — each instance has already filtered secrets before storage.
- Graceful degradation — offline instances don't respond, partial results returned.

**Path C: P2P mesh.** Nmem instances discover each other directly, no central beacon.

What breaks:
- Discovery requires multicast (doesn't work across subnets), DHT (complex), or gossip (chatty).
- Every instance must know the full peer set and scatter queries itself.
- Hard to enforce org-level rules without a central authority.

**Decision: Path B — query federation.** The beacon is a query router, not a data store.

## Architecture

### NATS Request/Reply as the Message Bus

**Why NATS:**
- **Native scatter/gather.** Request to subject, all subscribed instances respond independently. No custom routing.
- **No beacon state.** NATS handles message routing. The beacon client is a thin layer.
- **Auth built-in.** NATS 2.10+ supports auth callout to an external validator (GitHub org membership check).
- **Lightweight.** Single Go binary (<20MB), stateless message routing, no persistent storage needed for core NATS.
- **Observable.** Monitoring endpoints (`/varz`, `/connz`) export connection and message stats.

**Against alternatives:**

| Technology | Scatter/Gather | Auth | Verdict |
|-----------|---------------|------|---------|
| HTTP poll | No — can't push to instances | OAuth | Can't scatter |
| WebSocket | Yes | OAuth + WS upgrade | Viable but stateful |
| gRPC bidir stream | Yes | mTLS or JWT | Heavyweight |
| **NATS request/reply** | **Yes — native** | **JWT + NKey** | **Chosen** |

**Architectural invariant:** The beacon never stores observations. If the beacon or NATS is down, individual nmems continue to function locally — only cross-instance queries are unavailable.

### Query Flow

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

### Identity Model

**One user = one nmem = one dev machine.** This is a design constraint.

- Each nmem instance is tied to a single GitHub user via OAuth.
- Responses include the developer's GitHub username for attribution.
- Multi-machine users choose their primary. No cross-machine unification.
- Attribution is free — every federated observation carries the source developer's identity.

### Response Merging and Ranking

Querying instance merges responses from all respondents:

- **BM25 normalization.** FTS5 rank values are corpus-dependent and not comparable across instances (different corpus sizes, different term frequencies). Min-max normalization across all responses makes scores comparable.
- **Composite score:** `(0.5 * bm25_normalized) + (0.3 * recency_decay) + (0.2 * source_weight)`. Recency uses the same exponential decay with 7-day half-life as `recent_context` (ADR-006).
- **Deduplication.** Observations with identical content (after normalization) are deduplicated, keeping the highest-scoring version.
- **Limit.** Return top N results (default 50, configurable). Per-instance responses are already limited (default 20 per instance) to reduce network volume.

### Discovery: Automatic via NATS Subscription

No explicit registration. When an nmem instance enables beacon mode, it subscribes to the fleet's NATS subject. The subscription itself is the registration. NATS handles discovery — subscribers appear when they connect, disappear when they disconnect. No registry to maintain, no stale entries.

### Opt-out

`~/.nmem/config.toml` can disable query responses:

```toml
[beacon]
respond = false
```

The instance can still query the fleet but won't respond to incoming queries.

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

Fleet federation amplifies exposure risk. A secret that leaks into one instance's DB is bad. A secret that leaks into fleet query responses propagates to every querying developer.

### Plan

TruffleHog is an open-source secret scanner with 800+ detector patterns covering API keys, tokens, credentials across major services. Its detectors are battle-tested against real credential leaks in public repos.

**Integration:**

1. **Build-time extraction script** (`tools/extract-trufflehog-patterns.py`) pulls detector definitions from TruffleHog's GitHub repo, extracts regex patterns.
2. **Generate Rust source** (`src/s5_trufflehog_patterns.rs`) with patterns as a `RegexSet`.
3. **Merge into `s5_filter.rs`.** Existing `SecretFilter` already uses `RegexSet` — integration is expanding the pattern list, not changing architecture.
4. **Update cadence:** Re-run extraction script periodically (monthly or on-demand). Commit generated source. No runtime dependency on TruffleHog.

**No network verification.** TruffleHog's verification engine makes API calls to check if credentials are live. nmem skips this — a regex match is sufficient to redact. Filtering happens at write time (PostToolUse hook), so federated responses are already filtered.

**False positive trade-off:** TruffleHog optimizes for recall. Shannon entropy detection in `s5_filter.rs` provides the precision signal — a match that also has high entropy is more likely to be a real secret.

## NATS Subject Hierarchy

**Decision:** Two-level hierarchy with org scope.

- **Broadcast:** `nmem.{org}.search` — all instances in the org receive the query
- **Targeted (future):** `nmem.{org}.{username}.search` — query a specific developer's instance
- **Each instance subscribes to both:** `nmem.{org}.search` (org-wide) and `nmem.{org}.{username}.search` (user-specific)

**Rationale:** Structured enough for org isolation (different orgs on the same NATS server don't see each other's queries), flat enough within the org to avoid routing complexity. Team subdivision deferred — it's an S5 policy question without a clear use case yet. Org boundary is sufficient for viablesys.

## Open Questions

### Q1: Is the beacon a dedicated service or does an nmem instance play the role?

**Option A: Dedicated beacon service.** Separate `nmem-beacon` binary or lightweight auth callout + NATS server. Runs as team infrastructure (Docker, systemd).

**Option B: nmem instance as beacon.** One developer's nmem plays a dual role: local storage + NATS relay. Their machine hosts the NATS server.

**Option C: No dedicated beacon.** NATS is the beacon. Any nmem can query and any can respond. The "beacon" is the shared subject namespace.

**Trade-offs:**
- Option A: Higher availability (server stays up). Requires ops (deploy, monitor).
- Option B: Zero ops, but single point of failure (if that developer's machine is offline, fleet queries fail). Simpler for 2-5 devs.
- Option C: Simplest — but external tools (Grafana, Slack bots) can't easily query the fleet without a stable endpoint.

**Current lean:** Start with Option C (peer-to-peer via NATS). Document Option A as an upgrade path when the team outgrows it. The implementation should be deployment-agnostic — same nmem code works in all configurations.

### Q2: ADR-003 daemon constraint — how does the beacon client run?

The beacon client must be a NATS subscriber (long-lived process) to respond to queries. This tension with ADR-003's no-daemon rule has three resolutions:

**Option A:** Beacon subscription lives in the MCP server (`nmem serve`). Session-scoped — when the Claude Code session ends, the subscription ends. No new process.

**Option B:** `nmem beacon connect` as an explicit opt-in long-lived process. Separate from hooks and MCP server. Developer starts it manually or via systemd user unit.

**Option C:** NATS subscription only active during CLI commands. `nmem search --fleet` subscribes, queries, unsubscribes. No persistent presence — the instance is only queryable when it's actively querying.

**Trade-offs:**
- Option A is cleanest (no new process) but limits availability to active Claude Code sessions.
- Option B violates ADR-003 but is opt-in and explicit.
- Option C eliminates the daemon but means instances are intermittently available.

**Current lean:** Option A for initial implementation. The MCP server already runs for the duration of a Claude Code session. Adding a NATS subscription to it is natural. Option B as upgrade path for teams that want always-on availability.

### Q3: What happens when instances are offline?

NATS request/reply with timeout. Offline instances don't respond. Beacon returns results from online instances only.

**Consequence:** Partial results. If Alice (who has the answer) is offline, the query misses her context. No feedback to the querying developer about what's missing.

**Future mitigation (deferred):** Session summary sync to a shared read-only store. Summaries are safe to centralize (LLM-compressed, no raw observation details). Full observations stay local. This is a hybrid model — federated queries for live context, summary replica for offline resilience.

### Q4: Response volume at scale?

50 instances × 20 results each = 1000 observations to merge. Most will be low-relevance.

**Mitigations:**
- Per-instance limit (already in design — each returns top 20 max).
- BM25 score threshold: instances only respond if their best match exceeds a minimum relevance. Reduces noise from instances with no meaningful results.
- NATS timeout: slow instances are excluded, reducing volume naturally.

**Current lean:** Ship without a relevance threshold. The tiered query fallback already improves precision. Add threshold if volume becomes a problem in practice.

### Q5: Version skew across instances?

If Alice runs nmem v1.0 and Bob runs v1.2, response schemas may differ.

**Current lean:** Ignore initially. The observation schema (ADR-002) is stable. Add schema versioning in responses (`"schema_version": "1.0"`) when breaking changes are needed.

### Q6: Per-project visibility policies?

Should a developer working on `internal-security-tool` have their observations queryable by the entire org?

**Current lean:** Defer. Org membership is the trust boundary for now. If needed, add per-project opt-out via config: `[projects.internal-security-tool] beacon_respond = false`. This is an S5 (policy) decision, deferred until user feedback shows the need.

## Constraints

What's ruled out by existing decisions:

| Constraint | Source | Implication |
|-----------|--------|-------------|
| No centralized data store | ADR-007, VSM S1 | Beacon routes queries, never stores observations |
| Observations encrypted at rest locally | ADR-001 | Beacon never decrypts — responses are pre-filtered |
| Secret filtering at write time | ADR-007 | Federated responses are post-filter |
| FTS5 is the query engine | ADR-001 | Beacon federates FTS5 queries, no vector search |
| No new dependencies (heavy) | Project policy | `async-nats` crate is the only new Cargo dependency |
| MCP tools read-only | ADR-006 | Fleet queries are read-only search, no federation of writes |
| Org boundary = trust boundary | This ADR | No per-project ACLs in initial version |

## Consequences

### Positive
- **Data sovereignty preserved.** Observations never leave the dev machine. Federation is query-only.
- **Graceful degradation.** Beacon offline → local queries still work. Some instances offline → partial results, not failure.
- **Zero sync conflicts.** No replication, no eventual consistency, no CRDTs.
- **Simple trust model.** Org membership is the gate. No per-query ACLs, no capability tokens, no user table.
- **Attribution for free.** Every federated response carries the developer's GitHub username.
- **Operational simplicity.** NATS is stateless message routing, not a data store. No backups, no migrations.
- **Local-first stays viable.** Beacon is an enhancement, not a requirement. nmem works standalone.
- **FTS5 query robustness benefits everyone.** Tiered fallback improves local search even without fleet federation.
- **Secret filtering strengthened.** TruffleHog integration benefits local instances and reduces fleet exposure risk.

### Negative
- **Query latency.** Network round-trip adds latency per instance. Acceptable for MCP queries (not in the hook hot path), but noticeable.
- **Partial failures.** Offline instances → incomplete results. No feedback about what's missing.
- **No retry loops over NATS.** Query must be good on the first shot. Mitigated by tiered query rewriting.
- **NATS operational burden.** Someone has to run the NATS server. Mitigated by NATS being a single binary with no external dependencies.
- **GitHub OAuth dependency.** Auth requires GitHub. Can't federate across orgs without multiple OAuth Apps.
- **Secret filtering is best-effort.** 800+ TruffleHog patterns improve coverage but aren't exhaustive. A novel secret format could leak. Mitigated by entropy detection as second signal.
- **Beacon client lifecycle.** Beacon subscription needs a long-lived process or ties to MCP server session scope, limiting availability.

## References

- ADR-001 — Storage Layer (SQLite local-first, SQLCipher encryption, FTS5)
- ADR-003 — Daemon Lifecycle (no daemon constraint, process model)
- ADR-006 — Interface Protocol (MCP tools, FTS5 query handling, search contracts)
- ADR-007 — Trust Boundary and Secrets Filtering (S5 regex + entropy)
- ADR-011 — Phase Classification (inference performance budget)
- ADR-014 — Adaptive Capability (classifier retraining, relevance to cross-instance learning)
- RFC 8628 — OAuth 2.0 Device Authorization Grant
- NATS documentation — https://docs.nats.io/ (request/reply, JWT auth, auth callout, subject-based messaging)
- TruffleHog — https://github.com/trufflesecurity/trufflehog (800+ secret detector patterns)
- SQLite FTS5 — https://sqlite.org/fts5.html (MATCH syntax, BM25 ranking, prefix indexes)
- `gh` CLI OAuth — https://github.com/cli/cli (Device Flow reference implementation)

## Revision History

| Date | Version | Changes |
|------|---------|---------|
| 2026-03-10 | 1.0 | Initial. Architecture: query federation not data store. NATS request/reply for scatter/gather. GitHub org SSO via OAuth Device Flow. One user = one nmem = one machine. NATS subject hierarchy: `nmem.{org}.search` for broadcast, `nmem.{org}.{username}.search` for targeted. FTS5 tiered fallback: phrase → AND → OR → prefix, `rewrite_query()` function, prefix indexes, stopword stripping. TruffleHog integration (800+ patterns) for s5_filter.rs. Trust model: org membership = full query access. Response merging with BM25 normalization + recency + source weighting. Beacon opt-out via config. Six open questions: beacon deployment model, ADR-003 daemon tension, offline resilience, response volume, version skew, per-project visibility. |
