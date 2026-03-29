# ADR-017: Fleet Builds — Validated Cross-Platform Build, Test, and Toolbox Distribution

## Status
Draft

## Design Criteria

Three criteria govern every decision in this ADR. They are not trade-offs — all three must be satisfied simultaneously.

### 1. Security: all executable artifacts must be validated

Nothing runs on a fleet machine unless it has been validated on at least one platform and cryptographically signed by the author. Nothing is distributed via the mothership unless it carries a validation chain. An unvalidated artifact is a local draft — it does not exist to the fleet. A malicious or broken artifact cannot propagate.

### 2. Opportunistic capacity: fleet work is the lowest-priority work

Fleet dispatch never competes with a developer's own sessions. Capacity is scavenged, not allocated. If a developer is active or has consumed more than 30% of their Claude subscription window, their instance is unavailable for fleet work. On a team, light agentic users contribute idle capacity that heavy users' fleet builds consume. The system exploits the natural variance in usage across a team.

### 3. Path normalization: cross-platform operations require canonical paths

No cross-platform operation is reliable without consistent path representation. Symlinks, path separators, drive letter casing, Unicode normalization, and URI encoding all produce platform-specific representations of the same file. Validation records, artifact provenance, secret filtering, and federated search all break when paths from different platforms can't be compared. This is a security prerequisite — path confusion enables validation bypass and filter evasion.

## Framing

*How does a fleet of nmem instances, running on heterogeneous platforms, share validated build recipes, test results, and executable artifacts — without allowing unvalidated code to propagate, without competing with developers' own work, and without breaking on platform-specific path representations?*

This is a **coordination problem** (VSM S2) governed by **trust policy** (VSM S5). ADR-015 provides the pipes — NATS, mothership, identity. Fleet Builds defines what flows through those pipes for build and test operations.

ADR-015 establishes that observations never leave the machine — only query responses do. Fleet Builds extends this principle: **executable artifacts leave the machine, but only after validation.** The trust boundary moves from "data stays local" to "only validated artifacts cross the boundary."

The adversarial angles:

**Against trust-on-push:** "What if Alice pushes a Dockerfile that works on her Mac but runs `rm -rf /` on Linux?" The artifact must be validated on the target platform before it's trusted there. Author signature proves provenance; platform validation proves safety.

**Against validation-as-gate:** "What if requiring validation before distribution kills velocity?" One bad build script distributed to 10 instances is 10 incidents. One validated build script distributed to 10 instances is 10 working machines. The validation gate is cheaper than the recovery.

**Against centralized validation:** "What if the beacon validates everything?" Each instance is sovereign (ADR-015). The beacon coordinates; it does not approve. Validation happens on each platform, by each instance, for each artifact. The beacon aggregates results — it doesn't grant trust.

**Against eager dispatch:** "What if fleet builds consume everyone's Claude quota?" Fleet work is opportunistic scavenging. A developer who has used 30% of their window is excluded. The system waits for idle capacity — it never creates load.

## Depends On
- ADR-015 (Fleet Beacon) — NATS infrastructure, mothership blob store, identity model, opt-in distribution, response envelope protocol
- ADR-016 (Direct Inference) — embedded inference engine, feature flags (cuda, rocm, metal) that create the platform matrix
- ADR-007 (Trust Boundary and Secrets Filtering) — secret filtering extends to artifact content before distribution
- ADR-003 (Daemon Lifecycle) — relaxed for beacon; fleet builds extend the relaxation to test dispatching

## Unlocks
- Cross-platform CI validation without CI: fleet instances ARE the test matrix
- Effort discovery: "who already solved this build problem?"
- Toolbox distribution: validated Dockerfiles, CI configs, test harnesses shared across fleet
- Platform-normalized paths as a prerequisite for all cross-platform operations
- Integration with GitHub Actions, Forgejo/Gitea Actions, and other CI systems
- RALPH loop for build intelligence: recognize failure → search fleet → learn recipe → predict applicability

---

## Context

### The session that surfaced this

A macOS LSP test hung in CI for 54 minutes. Root cause: macOS `/var` → `/private/var` symlink. The same `tempfile` path that works on Linux fails on macOS because `std::env::current_dir()` and `git2::Repository::workdir()` resolve symlinks differently. The LSP tests were `#[cfg(not(windows))]` — they'd never run on Windows either.

To diagnose, we SSH'd from a Windows dev machine to a Mac on the LAN: installed Rust, cloned the repo, built with `--features metal`, and reproduced the hang. This is fleet building by hand — using heterogeneous machines to validate cross-platform behavior. The effort is captured in nmem observations. But it's not reusable.

nmem now ships CPU, CUDA, ROCm, and Metal builds across Linux x86_64, Linux aarch64, macOS ARM64, and Windows x86_64. That's an 8-cell release matrix. Each cell has platform-specific gotchas:

| Platform | Gotchas discovered (captured in nmem observations) |
|----------|---------------------------------------------------|
| Windows | `\\?\` UNC paths from `canonicalize()`, drive letter casing (`c:` vs `C:`), path separator `\` vs `/` |
| macOS | `/var` → `/private/var` symlink, NFD Unicode in filenames, Metal framework init on headless CI |
| Linux CUDA | `CMAKE_POSITION_INDEPENDENT_CODE=ON`, specific `cublas_dev` sub-package needed |
| Linux ROCm | `linker = "gcc"` workaround, `HSA_OVERRIDE_GFX_VERSION` env var |
| Windows CUDA | LLVM install for `libclang`, `CMAKE_GENERATOR=Ninja`, MSVC dev env setup |

Every gotcha was solved once. None of it is discoverable by the next developer who hits the same wall.

### What nmem already captures

Every build attempt is an observation. `cargo build --features cuda` that fails is a `command` observation with the error in metadata. The fix commit is a `git_commit` observation. The CI config change is a `file_edit` observation. Episode detection (ADR-010) can identify "CUDA build troubleshooting" as a coherent work unit with hot files, phase signature, and narrative.

What's missing: a way to extract the solution from these observations and distribute it as a validated, reusable artifact.

---

## Architecture

Three layers, each building on the last. Layer 1 is a prerequisite for Layers 2 and 3. Layer 2 is a prerequisite for Layer 3.

### Layer 1: Path Normalization

A shared utility that canonicalizes all paths at system boundaries. This is infrastructure that benefits local operations (LSP, git, map) independently of fleet features.

**Crates:** `dunce` (canonicalize without Windows `\\?\` UNC prefix), `url` (URI→path per RFC 8089), `path-slash` (separator normalization for git2).

**Pipeline:** URI parse → separator normalize → drive letter uppercase → canonicalize → strip trailing slash.

**Issues to handle:**

| # | Issue | Platform | Fix |
|---|-------|----------|-----|
| 1 | `fs::canonicalize()` returns `\\?\` UNC paths | Windows | `dunce::canonicalize()` |
| 2 | Drive letter casing (`c:` vs `C:`) | Windows | Normalize to uppercase |
| 3 | `/var` → `/private/var` symlink | macOS | `canonicalize()` resolves |
| 4 | git2 expects forward slashes, returns trailing slash on `workdir()` | All | `path-slash` + strip trailing |
| 5 | Unicode NFD vs NFC in filenames | macOS | Normalize to NFC |
| 6 | `canonicalize()` fails if path doesn't exist | All | Soft fallback with lexical normalization |
| 7 | `Path` hash/eq inconsistency with trailing slashes | All | Normalize via `components().collect()` |
| 8 | LSP URIs: percent-encoded colons, lowercase drive letters | Windows + VSCode | `url::Url::to_file_path()` + post-process |

**Security relevance:** Path normalization prevents validation bypass via path confusion. A validation record for `src/build.sh` must match the actual artifact regardless of how the platform represents the path. Secret filter rules must match regardless of path format — a filter for `/secret/key.pem` must catch `\secret\key.pem` on Windows.

### Layer 2: Toolbox (validated artifacts)

Extend the mothership's `library_docs` pattern (ADR-015) to executable artifacts. Same infrastructure: NATS for notification, mothership for storage, opt-in push/pull. Different trust model: **all artifacts must be validated before distribution.**

#### Artifact types

| Artifact type | Example | Platform tag | Validation method |
|--------------|---------|-------------|-------------------|
| Dockerfile | `nmem-build-cuda.dockerfile` | `linux/amd64` | `docker build` succeeds |
| CI fragment | `check-metal.yml` | `darwin/arm64` | Syntax validation + dry-run |
| Build script | `install-rocm-deps.sh` | `linux/amd64` | Execution in sandbox/container |
| Test harness | `lsp-timeout-wrapper.sh` | `darwin/*` | Execution succeeds |
| Config snippet | `cargo-rocm-config.toml` | `linux/amd64` | `cargo check` with config applied |
| Path normalization test | `path-canon-test.rs` | `*` (cross-platform) | `rustc` + run on each platform |

#### Trust levels

Every artifact has exactly one of four trust levels. Trust is monotonic — it can only increase, never decrease without re-validation.

| Level | Name | Who can see it | Who can execute it | How to reach |
|-------|------|---------------|-------------------|-------------|
| **0** | Draft | Author only | Author only | `nmem toolbox create` |
| **1** | Signed | Fleet (metadata only) | Author only | Author validates locally + signs |
| **2** | Validated | Fleet (full content) | Instances that validated it | ≥1 platform validation passes |
| **3** | Fleet-validated | Fleet (full content) | All fleet instances | Validated on ≥N distinct platforms (configurable, default 2) |

**Level 0 → 1:** Author runs the artifact on their own machine, observes success, and signs with their identity (GitHub-linked Ed25519 key from ADR-015 auth). The signature proves: this person, at this time, ran this artifact, and it succeeded.

**Level 1 → 2:** The signed artifact is pushed to the mothership. Fleet instances can see the metadata (title, platform, author, hash). Any instance that pulls and validates the artifact on their platform adds a validation record. The artifact becomes executable on platforms where it has been validated.

**Level 2 → 3:** When the artifact accumulates validations on N distinct platforms (configurable, default 2), it becomes fleet-validated — trusted for execution anywhere in the fleet. This is the threshold for fleet dispatch to include it in automated workflows.

**Invariant: no execution without validation.** An artifact is never executable on a platform where it hasn't been validated. A Dockerfile validated on `linux/amd64` and `darwin/arm64` is NOT executable on `windows/amd64` — even at Level 3. Fleet-validated means "trusted on platforms that have validated it," not "trusted everywhere unconditionally."

This is the load-bearing security property. It means:
- A malicious artifact pushed by a compromised account cannot execute on any fleet instance until someone validates it
- A broken artifact that passes on one platform doesn't silently break another
- The validation matrix is explicit — you can see exactly which platforms trust which artifacts

#### Cryptographic chain

```
Author creates artifact
    ↓
Author validates locally → success
    ↓
Author signs: Ed25519(author_key, SHA-256(artifact_content) || platform || timestamp || result)
    ↓
Push to mothership: artifact + signature + validation_record
    ↓
Fleet instance pulls → verifies signature against author's public key (from GitHub)
    ↓
Instance validates on own platform → success
    ↓
Instance signs own validation: Ed25519(instance_key, SHA-256(artifact_content) || platform || timestamp || result)
    ↓
Validation record pushed to mothership
    ↓
Artifact accumulates validation chain:
  [author@darwin/arm64:PASS, bob@linux/amd64:PASS, ci@windows/amd64:PASS]
```

The chain is append-only. Entries cannot be removed — a failed validation stays in the chain alongside passing ones. Each entry is independently verifiable via the validator's public key (resolved from GitHub).

#### Secret filtering on artifacts

ADR-007's `SecretFilter` runs on artifact content before push — same regex + entropy patterns that protect observations. A Dockerfile containing `ENV API_KEY=sk-...` is blocked at push time. The filter runs locally (S5 at the boundary), not at the mothership.

#### Schema

```sql
CREATE TABLE fleet_artifacts (
    id INTEGER PRIMARY KEY,
    filename TEXT NOT NULL UNIQUE,
    artifact_type TEXT NOT NULL,     -- 'dockerfile', 'ci_fragment', 'build_script', 'test_harness', 'config'
    platform TEXT NOT NULL,          -- 'linux/amd64', 'darwin/arm64', 'windows/amd64', '*'
    title TEXT,
    version INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL,
    updated_at INTEGER,
    hash TEXT NOT NULL,              -- SHA-256 of artifact content
    size_bytes INTEGER,
    author TEXT NOT NULL,            -- GitHub username
    author_signature TEXT NOT NULL,  -- Ed25519 signature of (hash || platform || timestamp)
    contributors TEXT,               -- JSON array
    tags TEXT,                       -- JSON array: ["cuda", "build", "windows"]
    mothership_url TEXT,
    local_path TEXT NOT NULL,
    source TEXT NOT NULL DEFAULT 'local',

    -- Trust level (0-3)
    trust_level INTEGER NOT NULL DEFAULT 0,

    -- Validation chain (append-only audit trail)
    validation_chain TEXT NOT NULL DEFAULT '[]',
    -- JSON array: [
    --   {"validator": "alice", "platform": "darwin/arm64", "result": "pass",
    --    "timestamp": 1774757000, "signature": "Ed25519(...)",
    --    "nmem_version": "1.2.0", "duration_ms": 12000}
    -- ]

    -- Provenance: link to the nmem observations that created this artifact
    source_session_id TEXT,
    source_episode_id INTEGER,
    source_observations TEXT         -- JSON array of observation IDs
);

CREATE INDEX idx_fleet_artifacts_trust ON fleet_artifacts(trust_level);
CREATE INDEX idx_fleet_artifacts_platform ON fleet_artifacts(platform);
CREATE INDEX idx_fleet_artifacts_hash ON fleet_artifacts(hash);
CREATE INDEX idx_fleet_artifacts_tags ON fleet_artifacts(tags);
```

**`source_observations`**: Every artifact traces back to the observations that created it. The developer who spent 3 hours debugging CUDA on Windows — those observations are the provenance. The artifact is the crystallized solution.

### Layer 3: Fleet Dispatch (validated orchestration)

The beacon distributes build/test tasks across fleet instances based on platform capabilities and available capacity. **Only fleet-validated artifacts (Level 3) can be included in automated dispatch.**

#### The manual version (what we did today)

```
Windows machine → SSH → Mac (10.0.0.27)
  → Install Rust, cmake
  → Clone repo
  → cargo test --features metal -- lsp_did_open
  → Observe: test hangs
  → Diagnose: path symlink mismatch
```

This is agentic fleet building. The agent (Claude Code on Windows) dispatched work to a remote platform (Mac) to validate cross-platform behavior. nmem captured the observations. The diagnosis became a marker.

**Security gap in the manual version:** The SSH key was added to `authorized_keys` during the session. There's no audit trail of what commands were run on the Mac beyond nmem observations on the Windows side. Fleet dispatch closes this gap.

#### The automated version

```
nmem fleet test --platforms darwin/arm64,linux/amd64,windows/amd64 -- cargo test --features metal
```

**Security constraints on fleet dispatch:**

1. **Command allowlist.** Fleet dispatch only accepts commands from a configured allowlist per instance. Default: `cargo test`, `cargo build`, `cargo clippy`, `docker build`. Arbitrary shell commands require explicit opt-in.

2. **Instance approval.** Each instance must opt in to fleet dispatch (`fleet.accept_dispatch = true`). Default: false.

3. **Dispatch audit log.** Every dispatched command is recorded as an observation on both the sender and receiver instances. The receiver logs: who sent it, what command, when, result.

4. **Sandboxed execution.** Dispatched build commands run in an isolated context: fresh git clone or worktree, environment variables stripped to a safe default set, timeout enforced (configurable, default 30 minutes), results captured and returned via NATS.

5. **No credential forwarding.** Dispatched commands cannot access the instance owner's SSH keys, API tokens, or nmem encryption keys. The sandbox has no access to `~/.ssh`, `~/.nmem/key`, or environment credentials.

---

## Capacity-Aware Dispatch

Fleet dispatch never competes with a developer's own work. The capacity model has two parts: a real-time availability signal and a dispatch eligibility gate.

### Usage histogram

Every NATS response from every fleet instance includes a usage histogram — two 5-hour windows matching the Claude subscription structure. This is an S2 coordination signal. It tells the fleet one thing: **who is available and who is not.**

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

- `active: true` → developer is working right now — **unavailable**
- `active: false, usage_pct: 0` → nobody home, fresh window — **available**
- `usage_pct: 68` → this window is mostly consumed — low headroom even if developer stops

The beacon doesn't predict patterns or learn schedules. It reads the fleet's current state from the responses it's already receiving. On a team of 10 developers, at any given moment some are actively coding and some aren't. Developers who aren't heavy agentic users report `active: false` most of the time — their instances are available for fleet work almost always.

**ADR-015 implication:** The capacity block is part of the response envelope for ALL NATS reply messages, not just fleet builds. This is a schema addition to the beacon response protocol. It benefits fleet search ranking too — responses from idle instances could be weighted higher.

### The team-level picture

```
Fleet availability (live, from last response per instance)

Instance          Platform         Active   Window Usage
alice@linux       linux/amd64      yes      82%        ← working, unavailable
bob@mac           darwin/arm64     no       12%        ← available
eve@windows       windows/amd64    no       3%         ← available (light user)
charlie@linux     linux/amd64      no       0%         ← available (not using Claude today)
dave@mac          darwin/arm64     yes      45%        ← working, unavailable

Dispatch eligible: bob@mac, eve@windows, charlie@linux
```

On a team, heavy agentic users (alice, dave) consume their windows. Light users (eve, charlie) barely touch theirs. Fleet builds exploit this naturally — the spare capacity across the team is the compute pool. No scheduling, no prediction. Just: who's available right now?

### Dispatch eligibility gate

**Hard cap: 30% of current window.** If an instance's `usage_pct > 30` in the current window, it is excluded from fleet dispatch entirely — regardless of other signals. Fleet work is opportunistic by default — it consumes spare capacity, not working capacity.

The 30% default is deliberately conservative. Instances can raise it (`fleet.quota_ceiling = 50`) if the developer has headroom they want to donate. But the baseline assumption is: **fleet dispatch is the lowest-priority work on the machine.** It runs when nothing else wants the resources.

**Active session gate:** If `active: true`, the instance is excluded. Period. A developer in a session — even with low usage — is working. Fleet builds wait.

### Personal fleet

A developer with multiple machines (Windows workstation + Mac laptop + Linux server) can opt their own instances into a **personal fleet** — a recursive layer below the org fleet.

```toml
# ~/.nmem/config.toml
[fleet.personal]
enabled = true                      # opt-in
instances = ["bpd10@windows", "bpd10@mac", "bpd10@linux"]
```

Within a personal fleet, the org-level constraints are relaxed:

| Constraint | Org fleet | Personal fleet |
|-----------|-----------|----------------|
| Validation required | Yes (Level 3 for dispatch) | No — same identity, implicit trust |
| Capacity ceiling | 30% per window | None — developer's own quota to manage |
| Active-session gate | Yes | Optional — developer may want to dispatch from one machine while working on another |
| Command allowlist | Enforced | Relaxed — same developer, same machines |
| Dispatch approval | Instance must opt in | Auto-approved between personal instances |

The personal fleet is what we did manually today — SSH from Windows to Mac to run tests. The difference: it's formalized, the dispatch is captured as observations on both sides, and the results flow back through NATS.

This is VSM recursion. The org fleet is a viable system. Each developer's personal fleet is a viable system within it. The same S2 coordination (NATS), S1 operations (dispatch + capture), and S5 policy (opt-in) apply — just with different trust parameters.

**Personal fleet capacity:** All instances in a personal fleet share one Claude subscription. The developer manages their own quota — the beacon treats personal-fleet dispatch as the developer's own work, not as fleet work. No capacity gate applies. The developer chooses to burn their quota on cross-platform testing — that's their prerogative.

### Platform capability registration

Each fleet instance advertises its capabilities:

```toml
# ~/.nmem/config.toml
[fleet]
platform = "darwin/arm64"           # auto-detected
capabilities = ["metal", "cpu"]     # from Cargo feature flags available
rust_version = "1.87.0"            # auto-detected
tools = ["cmake", "brew"]           # auto-detected
accept_dispatch = false             # opt-in to receiving fleet commands
allow_arbitrary = false             # restrict to command allowlist
allowed_commands = ["cargo test", "cargo build", "cargo clippy", "docker build"]
quota_ceiling = 30                  # max usage_pct to accept fleet work (default 30)
```

Capability advertisements are signed by the instance — a compromised beacon cannot forge capabilities.

### Dispatch scoring

When multiple instances are eligible, the beacon selects using a composite score:

| Signal | Weight | Rationale |
|--------|--------|-----------|
| Platform match | Gate (must pass) | Can't run Metal on Linux |
| Validation status | Gate (Level 3 for automated dispatch) | Security constraint |
| Active session | Gate (must be false) | Never interrupt a developer |
| Window usage < ceiling | Gate (must pass) | Opportunistic baseline |
| Remaining window headroom | 0.4 | More headroom = better candidate |
| Current load (builds in progress) | 0.3 | Prefer unloaded machines |
| Recent dispatch history | 0.3 | Spread load, don't hammer one instance |

Gates are binary pass/fail. Scoring applies only to instances that pass all gates.

**Privacy note:** `usage_pct` within the subscription window is the only usage signal exposed to the beacon. No billing details, no API keys, no account identifiers beyond the GitHub username already used for identity (ADR-015).

---

## Effort Discovery

New MCP tool and CLI command:

```
nmem toolbox search "cuda windows build"
```

Before creating a new artifact, search the fleet for prior effort:

1. **Local search** — FTS5 over `fleet_artifacts` table
2. **Fleet search** (if beacon connected) — scatter/gather over NATS `nmem.{org}.toolbox.search`
3. **Observation search** — search local+fleet observations for build/test commands matching the query

The observation search is what makes this different from a package registry. nmem knows that someone spent time on a problem even if they didn't package the solution. "3 hours of CUDA debugging on Windows in session abc123" surfaces as a finding even without a Dockerfile artifact. The episode narrative (ADR-010) captures the intent and outcome — the observations contain the specific commands and errors.

**Security constraint:** Observation search respects the same boundaries as ADR-015 federated search — each instance runs the query locally and returns only pre-filtered results. No raw observations cross instance boundaries. Artifact search returns metadata at Level 1+, full content at Level 2+.

---

## Integration with CI Systems

Fleet builds complement GitHub Actions and Forgejo/Gitea Actions — they don't replace them.

**Fleet → CI:** A fleet-validated artifact (Level 3) can be exported as a CI workflow fragment. The validation chain is embedded as a comment:

```yaml
# Fleet-validated artifact: cuda-windows-build v3
# Validation chain:
#   alice@darwin/arm64:PASS (2026-03-28)
#   bpd10@windows/amd64:PASS (2026-03-29)
#   ci@linux/amd64:PASS (2026-03-29)
- name: Install CUDA toolkit (Windows)
  uses: Jimver/cuda-toolkit@v0.2.34
  with:
    cuda: '12.4.0'
    sub-packages: '["nvcc", "cublas", "cublas_dev", "cudart"]'
```

**CI → Fleet:** CI failure observations flow back into nmem. A GitHub Actions failure on `check-metal` becomes an observation that triggers effort discovery — "has anyone solved this on darwin/arm64?"

**Forgejo/Gitea Actions:** Same artifact, different syntax. The toolbox stores the platform-neutral recipe; CI-specific wrappers are generated. Forgejo Actions are YAML-compatible with GitHub Actions for most use cases — the generator handles divergences (runner labels, container support differences).

---

## VSM Mapping

Fleet builds extend every VSM layer. S5 governs, S2 coordinates, S4 discovers.

| Layer | Current (ADR-015) | Extended (Fleet Builds) |
|-------|-------------------|------------------------|
| **S1 Operations** | Query federation, RAG distribution | + Artifact storage, build observation capture, sandboxed execution |
| **S2 Coordination** | NATS message routing | + Platform capability matching, capacity-aware dispatch, usage histogram aggregation |
| **S3 Control** | Fleet maintenance policies | + Artifact retention governed by trust level, stale recipe cleanup |
| **S3* Audit** | Mothership as canonical store | + Validation chain verification, cross-platform validation matrix |
| **S4 Intelligence** | Cross-instance synthesis, ensemble research | + Effort discovery ("who solved this?"), build pattern detection, RALPH loop |
| **S5 Policy** | Org boundary, opt-in | + Validation gates, trust levels, command allowlists, sandbox enforcement, artifact signing, capacity ceiling |

### S5 as the Governing System

In the local nmem, S5 is static config (VSM.md assessment). In fleet builds, **S5 becomes active**. It mediates:

- **S4 wants to distribute:** S5 asks: "Is it validated? On which platforms? By whom?" Only Level 2+ artifacts pass.
- **S2 wants to dispatch:** S5 asks: "Is the target instance available? Is their usage below the ceiling? Is the command in the allowlist?" The capacity gate is S5 policy enforced by S2 coordination.
- **S1 wants to execute:** S5 asks: "Is this artifact validated for this platform? Is the dispatch from an authorized sender?"
- **S3 wants to forget:** S5 says: "Validated artifacts have longer retention. Trust level IS the retention signal."

This is the S3↔S4 mediation that VSM.md identifies as missing from the local system. Fleet builds create the tension: S4 generates artifacts (intelligence), S3 must decide which to keep (control), S5 mediates based on validation status (policy). The capacity ceiling is S5 mediating S2 — coordination constrained by policy.

### S4: The RALPH Loop for Builds

Fleet builds create an intelligence cycle:

1. **Recognize** — Build/test failure detected (observation with error metadata). S2 classifies as `command` with `friction` label.
2. **Act** — Search fleet toolbox for prior validated solutions. Search fleet observations for related effort.
3. **Learn** — When fix is found, extract as toolbox artifact with observation provenance. Validate locally.
4. **Predict** — When a new platform+dependency combination appears, suggest relevant validated recipes. "This CUDA fix on Linux might apply to your ROCm build."
5. **Hypothesize** — Cross-platform inference: "The macOS path symlink fix probably also affects FreeBSD." Surface as suggested tasks — but the suggested artifact must still be validated on the new platform before it's trusted there.

### S3: Retention Governed by Trust

| Trust level | Retention | Rationale |
|-------------|-----------|-----------|
| Level 3 (fleet-validated) | 730 days | Highest value — amortized effort, multi-platform proof |
| Level 2 (validated) | 365 days | Proven on ≥1 platform |
| Level 1 (signed) | 180 days | Author-tested but not fleet-proven |
| Level 0 (draft) | 30 days | Local-only, likely superseded |

The obs_trace rollup (ADR-005 v5.0) preserves the build sequence fingerprint even after individual observations are swept. A toolbox artifact's `source_observations` may point to swept observations — but the episode narrative and obs_trace preserve the essential context.

---

## Open Questions

### Q1: Where does "toolbox" end and "package registry" begin?
The toolbox stores recipes (Dockerfiles, scripts, configs). Does it store binaries? **Security stance:** binaries require a stricter validation model (reproducible builds, hash pinning) than text artifacts. Draw the line at text.

### Q2: Validation sandbox depth
`docker build` in a container is reasonably sandboxed. `cargo test` on bare metal has access to the local filesystem. How deep does sandboxing go? Full container isolation for all dispatched commands? Or trust the allowlist + audit log?

### Q3: Key management
Ed25519 signing keys tied to GitHub identity. Where are private keys stored? System keyring (ADR-015 precedent) or nmem-managed? Key rotation? Revocation when a developer leaves the org?

### Q4: Artifact extraction from observations
`nmem toolbox create --from-session <id>` implies extracting a reusable artifact from raw observations. How much can S4 automate vs. requiring human curation? **Security constraint:** auto-generated artifacts start at Level 0 regardless of source — they must still pass validation.

### Q5: Platform capability drift
A fleet instance advertises `cuda` capability. Then CUDA is uninstalled. Stale capabilities could cause dispatch to a machine that can't execute. Capability verified at dispatch time before execution?

### Q6: Relationship to `s4_dispatch.rs`
Task dispatch already exists for tmux sessions. Fleet dispatch extends this across machines. Same queue? Same schema? Or a separate fleet-specific dispatch table with security columns?

### Q7: Forgejo/Gitea Actions compatibility
GitHub Actions and Forgejo Actions share YAML syntax for ~90% of use cases. The 10% divergence (runner labels, container support, marketplace actions) may require platform-specific overrides. One generator or N?

### Q8: Validation failure handling
What happens when a fleet-validated artifact (Level 3) fails validation on a new platform? Does its trust level drop? Only for that platform? How is the fleet notified of a regression?

### Q9: Claude usage tracking mechanism
How does an instance know its `usage_pct` within the current subscription window? Options: parse API response headers, track locally from hook invocations, query Claude API directly. The tracking must be passive — no additional API calls.

---

## Consequences

### Positive
- **Effort is never wasted twice.** A platform fix solved once becomes a validated fleet artifact, discoverable by every future developer.
- **The fleet IS the test matrix.** Heterogeneous dev machines validate cross-platform behavior — with explicit opt-in and sandboxed execution.
- **Trust is explicit and auditable.** Every artifact has a cryptographic validation chain. Anyone can verify who tested what, where, when.
- **No silent propagation.** Unvalidated artifacts cannot reach fleet instances. A compromised account can push but cannot execute — validation is the gate.
- **No quota competition.** Fleet work scavenges spare capacity. The 30% ceiling and active-session gate ensure developers' own work always takes priority.
- **Team-level capacity exploitation.** Light agentic users contribute idle capacity naturally. The system adapts to the team's usage distribution without configuration.
- **Observation provenance.** Every artifact traces to the observations that created it.
- **Standard CI integration.** Fleet-validated artifacts feed into GitHub Actions / Forgejo with embedded provenance.
- **VSM coherent.** S5 becomes active — mediating S3↔S4 tension via trust levels, mediating S2 dispatch via capacity ceilings. First concrete S5 evolution beyond static config.
- **Path normalization benefits everything.** The prerequisite fix improves local operations (LSP, git, map) independently of fleet features.

### Negative
- **Velocity cost.** Validation gates slow artifact distribution. An urgent fix must still pass validation before fleet distribution.
- **Key management burden.** Ed25519 keys, identity verification, rotation, revocation — operational overhead.
- **Validation matrix explosion.** 4 OS × 4 GPU × N dependency versions = combinatorial space. Bounded by the N-platform threshold, not exhaustive coverage.
- **Capacity signal freshness.** Usage histogram is only as fresh as the last NATS response. A developer who starts working between responses won't be excluded until their next response updates the beacon.
- **Scope creep risk.** "Toolbox" + "validation" + "signing" + "capacity management" approaches a full platform. Where does nmem end?
- **Operational complexity.** NATS subjects, mothership storage, platform capability tracking, sandbox configuration, key management — each adds moving parts with security implications.

## References

- ADR-007 — Trust Boundary and Secrets Filtering (S5 filtering extends to artifact content)
- ADR-015 — Fleet Beacon (NATS, mothership, identity, opt-in distribution, response envelope protocol)
- ADR-016 — Direct Inference (feature flags creating the platform matrix)
- VSM.md — S4 intelligence, S3 control, S5 mediation, views as inter-system channels
- dunce crate — https://docs.rs/dunce (Windows canonicalize without UNC)
- url crate — https://docs.rs/url (RFC 8089 file URI handling)
- path-slash crate — https://docs.rs/path-slash (separator normalization)
- Sigstore — https://sigstore.dev (artifact signing precedent, not a dependency)
- in-toto — https://in-toto.io (supply chain attestation framework, architectural reference)

## Revision History

| Date | Version | Changes |
|------|---------|---------|
| 2026-03-29 | 1.0 | Full rewrite. Three governing design criteria (security, opportunistic capacity, path normalization) elevated from annotations to primary constraints. Three architectural layers (path normalization, toolbox, fleet dispatch). Security: four trust levels, cryptographic validation chain, no execution without per-platform validation, command allowlists, sandboxed execution, secret filtering on artifacts. Capacity: usage histogram (two 5-hour windows) embedded in every NATS response, 30% hard cap, active-session gate, team-level capacity exploitation across light/heavy users. Personal fleet: VSM recursion — developer's own instances with relaxed trust/capacity constraints, opt-in, implicit trust (same identity). Path normalization: 8 cross-platform issues, security relevance (validation bypass, filter evasion). VSM: S5 active (mediates S3↔S4 via trust, mediates S2 via capacity ceiling). RALPH loop. CI integration (GitHub Actions, Forgejo). Effort discovery from observations. Schema for fleet_artifacts with validation chain and provenance. |
