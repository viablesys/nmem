# ADR-007: Trust Boundary and Secrets Filtering

## Status
Accepted

## Framing
*What must never be stored, and where is it enforced.*

Security-first is a stated principle but has no concrete design yet. Adversarial angle: "What if a secret gets stored despite filtering?" — what's the blast radius? Can it be purged? This ADR forces the encryption-at-rest decision deferred by ADR-001 as a downstream consequence.

## Depends On
- ADR-003 (Daemon Lifecycle) — filtering runs in `nmem record`, the in-process hook handler. WAL checkpoint on shutdown is the cleanup boundary.
- ADR-004 (Project Scoping) — single DB with project column means a leaked secret is visible to all project scopes.

## Unlocks
- ADR-005 (Forgetting) — purging secrets is a special case of forgetting.

---

## Context

### Where Secrets Appear

nmem captures observations from Claude Code hook events. ADR-002 decided: store extracted facts, not raw tool output. This bounds the attack surface. Secrets can only enter the database through fields that `nmem record` writes:

| Field | Source | Secret Risk |
|-------|--------|-------------|
| `content` | Extracted fact text (command, error, file path) | **High** — commands may contain inline tokens (`curl -H "Authorization: Bearer sk-..."`) |
| `file_path` | Normalized path from tool input | **Low** — paths rarely contain secrets, but possible (e.g., `/tmp/credentials.json`) |
| `metadata` | JSON object with tool-specific fields | **Medium** — Bash stderr, grep patterns, command arguments |
| `session_id` | Hook common field | **None** — UUID |
| `project` | Derived from `cwd` | **None** — directory path |
| `obs_type` | Derived from tool/event | **None** — enum string |
| `source_event` | Hook event name | **None** — enum string |
| `tool_name` | From PostToolUse | **None** — tool identifier |

The raw `tool_response` is **not stored** (ADR-002). This is the single most important security property of the extraction strategy — tool responses contain file contents, command output, and API responses that are the primary carriers of secrets. Structured extraction discards them by design.

### The Highest-Risk Input

`UserPromptSubmit` captures the user's prompt text as `content`. Users routinely paste secrets into prompts:
- "Set the API key to `sk-proj-abc123...`"
- "The database password is `hunter2`"
- "Use this token: `ghp_xxxxxxxxxxxx`"

Unlike tool calls where nmem controls what it extracts, `UserPromptSubmit` stores the user's literal text. This is the field most likely to contain secrets and the hardest to filter without false positives, because the user's intent text is inherently unstructured.

### What Secrets Look Like

Secrets have recognizable structure. Most follow provider-specific formats with distinctive prefixes: AWS keys (`AKIA...`), GitHub PATs (`ghp_...`, `github_pat_...`), OpenAI/Anthropic keys (`sk-...`, `sk-ant-...`), Bearer tokens, PEM private key headers, connection strings with embedded credentials (`://user:pass@host`), and generic `password=` / `token=` / `secret=` assignments.

This list is not exhaustive. New providers create new formats. But known formats cover the vast majority of accidental secret exposure in developer workflows. The full pattern set is in the Pattern Registry below.

## Threat Model

### What We're Defending Against

nmem is a **local, single-user tool**. The threat is not an external attacker with network access — it's accidental persistence. The adversary is:

1. **The user themselves** — pasting secrets into prompts, running commands with inline credentials. They don't expect these to be stored permanently in a separate database.
2. **Future retrieval** — a stored secret surfaces in a SessionStart context injection or MCP query response months later, in a context where it shouldn't appear (different project, shared screen, log file).
3. **Backup/copy exposure** — the `~/.nmem/nmem.db` file is copied, backed up, or transferred to another machine. Secrets in the database travel with it.

### What We're NOT Defending Against

Compromised harness (malicious extensions), root access, memory forensics, and side channels. All are out of scope for a local CLI tool — if an attacker has this level of access, nmem's database is the least of the user's problems.

### Blast Radius of a Leaked Secret

If filtering fails and a secret is stored in `observations`:

1. **It persists on disk** — SQLite does not zero-fill deleted pages. Even after DELETE, the data remains in free pages until overwritten by new data.
2. **It may appear in WAL** — WAL frames contain page images with the secret. They persist until checkpoint folds them into the main DB.
3. **It's queryable** — FTS5 indexes the `content` field. A search for "sk-" or "Bearer" would surface it.
4. **It crosses project boundaries** — ADR-004's single-DB model means a secret from project A is accessible in project B's queries unless query filtering is perfect.
5. **It survives forgetting** — even if ADR-005's retention policy deletes old observations, the data lingers in free pages.

This blast radius is why filtering is the **primary** defense, not the **only** defense.

## Decision: Filtering Strategy

### Approach: Denylist with Redaction

**Denylist, not allowlist.** An allowlist ("only store content matching these safe patterns") would be too restrictive — user prompts and error messages are freeform text. Instead, scan all high-risk fields against known secret patterns and redact matches.

**Redact, don't skip.** Dropping an entire observation because it contains a secret loses the non-sensitive context. A user prompt like "Set the API key to sk-proj-abc123 in the config" becomes "Set the API key to [REDACTED] in the config" — the intent is preserved, the secret is not.

### Pattern Registry

A static set of compiled regex patterns, checked in order against each high-risk field:

```rust
use regex::{Regex, RegexSet};
use std::sync::LazyLock;

struct SecretFilter {
    set: RegexSet,           // fast multi-pattern rejection (single pass)
    patterns: Vec<Regex>,    // individual patterns for replacement
    placeholder: &'static str,
}

impl SecretFilter {
    fn new() -> Self {
        // IMPORTANT: More-specific patterns must precede broader ones.
        // RegexSet::matches().into_iter() returns matched indices —
        // replacement runs only for patterns that matched the original input.
        // Ordering still matters: sk-ant- before sk- ensures the specific
        // pattern's replacement runs first during sequential application.
        let pattern_strings = vec![
            // AWS
            r"AKIA[0-9A-Z]{16}",
            r"(?i)aws[_\-]?secret[_\-]?access[_\-]?key\s*[=:]\s*\S+",
            // GitHub (longest prefix first)
            r"github_pat_[A-Za-z0-9_]{82}",
            r"ghp_[A-Za-z0-9]{36}",
            r"gho_[A-Za-z0-9]{36}",
            r"ghs_[A-Za-z0-9]{36}",
            // API keys (Anthropic before generic sk-)
            r"sk-ant-[A-Za-z0-9\-]{20,}",
            r"sk-[A-Za-z0-9]{20,}",
            // Bearer tokens
            r"(?i)bearer\s+[A-Za-z0-9_\-.~+/]{20,}=*",
            // Private keys
            r"-----BEGIN\s+(RSA|EC|DSA|OPENSSH|PGP)\s+PRIVATE\s+KEY-----",
            // Connection strings with credentials (common DB/API schemes only)
            r"(postgres|mysql|mongodb|redis|amqp|https?)://[^:]+:[^@\s]+@",
            // Generic password/secret/token assignment
            r"(?i)(password|passwd|secret|token|api_key|apikey)\s*[=:]\s*\S+",
        ];

        let set = RegexSet::new(&pattern_strings).unwrap();
        let patterns: Vec<Regex> = pattern_strings.iter()
            .map(|p| Regex::new(p).unwrap())
            .collect();

        Self { set, patterns, placeholder: "[REDACTED]" }
    }

    fn redact(&self, input: &str) -> (String, bool) {
        // Fast path: single-pass check across all patterns (see regex.md § 4)
        if !self.set.is_match(input) {
            return (input.to_string(), false);
        }

        // Slow path: only reached when a secret is detected.
        // Use matched() to run replacement only for patterns that matched.
        let mut output = input.to_string();
        let mut redacted = false;
        let matches = self.set.matches(input);
        for idx in matches.into_iter() {
            if let std::borrow::Cow::Owned(new) = self.patterns[idx]
                .replace_all(&output, self.placeholder)
            {
                output = new;
                redacted = true;
            }
        }
        (output, redacted)
    }
}

// Singleton — compile once at process startup (see regex.md § 2)
static FILTER: LazyLock<SecretFilter> = LazyLock::new(SecretFilter::new);
```

**Two-tier matching (see `regex.md` § 4):** `RegexSet::is_match()` is the fast path — a single pass over the input against all 12 patterns simultaneously. On the common path (no secret), this is the only cost (~100-200ns). Individual `Regex::replace_all` runs only for patterns that actually matched, identified by `RegexSet::matches().into_iter()`. `replace_all` returns `Cow::Borrowed` on no-match (zero allocation).

**Compilation cost:** `RegexSet::new` with 12 patterns takes ~100-1000us. `LazyLock` ensures this happens once per process, not per invocation. For `nmem record` (one-shot process per hook), the compilation cost is paid once at startup — acceptable given the ~3-8ms total invocation budget (ADR-003).

**Dependency:** `regex` crate (version 1.11). Not listed in ADR-001's dependency table because it's near-universal in Rust (pulled transitively by most crates). Adds negligible binary size. See `regex.md` for full API reference.

### Where Filtering Runs

**In `nmem record`, before the DB write.** This is the only write path (ADR-003). Filtering is a function call between extraction and INSERT:

```
Hook payload (stdin JSON)
  → Deserialize
  → Extract observation (per ADR-002)
  → Filter secrets (redact high-risk fields)   ← HERE
  → Dedup check (per ADR-003)
  → INSERT into observations
```

Filtering happens in-process, synchronously, before any data touches SQLite. There is no code path that writes unfiltered content to the database. The filter function is called on `content`, `file_path`, and `metadata` fields before they are bound to the INSERT statement.

**Metadata requires JSON-aware redaction.** The `metadata` field is a JSON object, not a plain string. Redacting a regex match within serialized JSON could break structure (e.g., if a replacement crosses a quote boundary). The correct approach: deserialize `metadata` to `serde_json::Value`, recursively walk all string-typed leaf values, apply `redact()` to each, then re-serialize. This preserves JSON structure while redacting secret values.

```rust
fn redact_json_value(filter: &SecretFilter, value: &mut serde_json::Value) -> bool {
    let mut any_redacted = false;
    match value {
        serde_json::Value::String(s) => {
            let (redacted, was_redacted) = filter.redact(s);
            if was_redacted {
                *s = redacted;
                any_redacted = true;
            }
        }
        serde_json::Value::Object(map) => {
            for v in map.values_mut() {
                any_redacted |= redact_json_value(filter, v);
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr.iter_mut() {
                any_redacted |= redact_json_value(filter, v);
            }
        }
        _ => {} // numbers, bools, null — no secrets
    }
    any_redacted
}
```

### What Happens on Match

When a pattern matches:

1. **The matched text is replaced with `[REDACTED]`** in the field value.
2. **A `redacted: true` flag is set in `metadata`** (JSON merge) to mark the observation as having been filtered. This enables auditing — "how many observations were redacted this month?" — without storing what was redacted.
3. **The observation is still stored.** Only the secret text is removed, not the surrounding context.
4. **A warning is written to stderr** (visible in Claude Code's hook diagnostics): `nmem: redacted potential secret from {obs_type} observation`.

No exit code change — redaction is not an error. Exit code 2 (ADR-003) is reserved for critical failures like DB corruption, not for successful filtering.

### False Positives and Pattern Extensibility

The denylist will over-redact. A variable named `password_validator`, a base64-encoded hash, or a file path containing "token" as a directory name will all trigger matches. Over-redaction loses context but causes no harm. Under-redaction (missing a real secret) is the dangerous failure mode. The `redacted: true` metadata flag lets users identify filtered observations and tune if needed.

Patterns are compiled into the binary at build time. Security defaults are baked in — a runtime config file that could be emptied or corrupted is not acceptable for the base pattern set. If user-defined patterns are needed (e.g., internal token formats), they can be loaded from `~/.nmem/config.toml` and merged with built-in patterns. User patterns can only add to the denylist, never remove built-in patterns.

### Filter Test Expectations

The pattern registry must be tested against known secret formats and known non-secrets. At minimum:

**Must redact (true positives):**
| Input | Pattern |
|-------|---------|
| `AKIAIOSFODNN7EXAMPLE` | AWS access key |
| `ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdef1234` | GitHub PAT |
| `sk-ant-api03-abcdefghijklmnopqrstuvwxyz` | Anthropic key |
| `sk-proj-abcdefghijklmnopqrstuvwxyz1234` | OpenAI key |
| `Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.abc` | JWT bearer |
| `-----BEGIN RSA PRIVATE KEY-----` | PEM header |
| `postgres://admin:s3cret@db.host:5432/mydb` | Connection string |
| `password=hunter2` | Generic assignment |

**Must NOT redact (false positives to avoid):**
| Input | Why it's safe |
|-------|---------------|
| `password_validator` | Variable name, not an assignment |
| `git://github.com/user/repo` | Git URL, no credentials |
| `file:///tmp/token_cache/data` | File path with "token" directory |
| `sk-iplink` | Short, not a key (< 20 chars after prefix) |
| `the token count was 150` | Natural language, no assignment |

This table is not exhaustive — it's the minimum regression set. Expand as false positives are reported via the `redacted: true` audit trail.

## Decision: Encryption at Rest

### Revised Decision (v2.0)

**Decision: SQLCipher available, opt-in via key.** The binary is built with `bundled-sqlcipher` (replacing `bundled`). Encryption activates when a key exists (`NMEM_KEY` env var, `~/.nmem/key` file, or config `key_file`). Without a key, the database operates unencrypted — same behavior as before.

This resolves ADR-001's open encryption question with a practical middle ground: no friction for users who don't need it, full 256-bit AES encryption for those who do.

### Key Management

**Precedence:** `NMEM_KEY` env var > config `[encryption].key_file` > `~/.nmem/key` file > no encryption.

**Key format:** 64 hex chars (32 bytes). Applied as raw hex (`PRAGMA key = x'...'`) to skip PBKDF2 — critical for CLI per-invocation performance. PBKDF2 at 256000 iterations adds ~50-200ms per open; raw key adds <1ms.

**Auto-generation:** `nmem encrypt` generates 32 bytes from `/dev/urandom`, hex-encodes to 64 chars, writes to `~/.nmem/key` with 0600 permissions.

### Migration from Unencrypted

Two paths to encryption:

1. **Explicit:** `nmem encrypt` — checks if DB is already encrypted, generates/loads key, runs `sqlcipher_export()` migration, verifies result.
2. **Automatic:** `open_db()` detects key + unencrypted DB, auto-migrates. The original is saved as `.db-unencrypted-backup` for user verification.

Migration uses `sqlcipher_export()` (copies schema, data, indexes, triggers) via ATTACH DATABASE. Atomic swap: original → backup, encrypted → original. WAL/SHM files from the old DB are cleaned up.

### Build Change

```toml
# Before
rusqlite = { version = "0.38", features = ["bundled"] }
# After
rusqlite = { version = "0.38", features = ["bundled-sqlcipher"] }
```

Requires `openssl-devel` (Fedora) or equivalent. Links against system OpenSSL. FTS5, WAL mode, and `rusqlite_migration` all work unchanged with SQLCipher.

### Status Display

`nmem status` reports encryption state: `nmem: encryption — enabled/disabled`. Detection opens the DB without a key and checks if `sqlite_master` is readable.

### Original Rationale (retained for context)

The original decision (v1.0) was "no encryption by default" based on: filtering is load-bearing, encryption breaks standard tooling, adds build complexity, filesystem encryption exists, and blast radius is local. These points remain valid — encryption is opt-in, not default. Users who don't set a key get the same behavior as before.

### File Permissions

`nmem record` sets database file permissions on creation: `0600` (owner read/write only). The `~/.nmem/` directory itself is `0700`. Both are checked and enforced on first run.

```rust
use std::os::unix::fs::PermissionsExt;

fn ensure_secure_permissions(db_path: &Path) -> std::io::Result<()> {
    let nmem_dir = db_path.parent().expect("db_path has parent");
    // Create directory if needed
    if !nmem_dir.exists() {
        std::fs::create_dir_all(nmem_dir)?;
    }
    std::fs::set_permissions(nmem_dir, std::fs::Permissions::from_mode(0o700))?;
    // Set DB file permissions after creation (Connection::open creates if missing)
    if db_path.exists() {
        std::fs::set_permissions(db_path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}
```

## Purging: When Filtering Fails

Despite best-effort filtering, a secret may reach the database — a new token format, a pattern miss, a bug. The purge path must exist.

### Purge Procedure

The correct order ensures secrets are zeroed on disk:

1. `PRAGMA secure_delete = ON` — must be set *before* delete so freed pages are zeroed
2. `DELETE FROM observations WHERE ...` — FTS5 entries removed by AFTER DELETE trigger (ADR-001)
3. `PRAGMA incremental_vacuum` — return zeroed pages to the filesystem
4. `PRAGMA wal_checkpoint(TRUNCATE)` — fold WAL into main DB, remove WAL file

**Do not enable `secure_delete` globally.** Normal observation deletion (ADR-005 retention policies) does not need zero-fill. Only the purge path (explicit secret removal) needs it. At nmem's volume (deleting a handful of observations), the overhead is negligible.

### Exposing Purge as a Command

```
nmem purge --id 42          # purge specific observation
nmem purge --search "sk-"   # find and purge matching observations
nmem purge --session <sid>  # purge all observations from a session
```

The `purge` subcommand enables `secure_delete`, performs the DELETE, runs incremental vacuum, and checkpoints the WAL. It confirms before acting — purging is destructive and the zero-fill makes it irreversible.

## WAL Considerations

When a secret is written, it exists in **two places**: the main DB file and the WAL file (`nmem.db-wal`). WAL frames persist until checkpoint folds them into the main DB. Deleting the observation and vacuuming does not remove the secret from uncheckpointed WAL frames.

**Checkpoint-on-shutdown** (ADR-003) mitigates this: `PRAGMA wal_checkpoint(TRUNCATE)` on session end removes the WAL file entirely. After a clean shutdown, secrets only exist in the main DB where `secure_delete` + vacuum can remove them. On abnormal shutdown (crash without Stop hook), the next `nmem record` invocation auto-replays the WAL via SQLite recovery. The purge procedure always includes a checkpoint as its final step.

The WAL file and SHM file inherit the parent database's permissions. `nmem record` should verify this on startup — a WAL file with wrong permissions is a security defect.

## Resolved: High-Entropy Detection (formerly Q1)

**Decision: Implemented.** Shannon entropy analysis runs as a second filtering layer after regex, catching random-looking tokens (hex keys, base64 blobs) that bypass the 12 regex patterns.

### Design

Phase 2 in `redact()`: tokenize input on whitespace + delimiters (`"'()[]{}`,;`), skip tokens shorter than `entropy_min_length` (default 20) or matching the allowlist, flag remaining tokens with entropy ≥ `entropy_threshold` (default 4.0 bits/char).

**Tokenizer preserves** `-_/.:=` within tokens (common in paths, URLs, assignments). Tokens are processed in reverse byte-offset order so replacements don't invalidate subsequent offsets.

**Allowlist** prevents false positives on known high-entropy non-secrets:
- Git SHAs: exactly 40 hex chars
- Short git SHAs: 7-12 hex chars
- UUIDs: 36 chars, 8-4-4-4-12 hex pattern
- File paths: starts with `/`, `./`, `~/`
- URLs: starts with `http://`, `https://`, `file://`
- Already redacted: `[REDACTED]` (10 chars, below min length anyway)

**No re-flagging risk:** `[REDACTED]` is 10 chars, below the 20-char minimum length, so entropy never re-processes regex output.

**Performance:** `shannon_entropy()` uses a stack-allocated `[0u32; 256]` frequency array. ~100-200ns per token. Only runs on the slow path (after regex).

### Threshold Rationale

| Content | Entropy | Length | Flagged? |
|---------|---------|--------|----------|
| English prose | 1.5-3.5 | any | No |
| Code identifiers | 2.5-3.8 | <20 | No (short) |
| Git SHA (40 hex) | ~4.0 | 40 | No (allowlisted) |
| UUID | ~3.3 | 36 | No (allowlisted) |
| Random hex key | ~4.0+ | 64 | Yes |
| Random base64 | ~4.5-5.7 | 32+ | Yes |
| Mixed-case hex | ~4.2 | 64 | Yes |
| File paths | ~3.8 | varies | No (allowlisted) |

Pure lowercase hex (16-char alphabet) at 64 bytes produces entropy near 3.9-4.0 — right at the threshold boundary. Mixed-case hex and base64 (larger alphabets) reliably exceed 4.0. The threshold is conservative: better to miss pure-lowercase hex than over-redact code identifiers.

## Resolved: Per-Project Config File (formerly Q2)

**Decision: Implemented.** `~/.nmem/config.toml` (or `NMEM_CONFIG` env var) supports user-defined patterns, per-project sensitivity levels, and entropy overrides.

### Config Format

```toml
[filter]
extra_patterns = ["my-company-[A-Za-z0-9]{32}"]
entropy_threshold = 3.8        # override default 4.0
entropy_min_length = 16        # override default 20
disable_entropy = false

[projects.secret-app]
sensitivity = "strict"    # threshold=3.5, min_length=16

[projects.open-source-tool]
sensitivity = "relaxed"   # entropy disabled, regex only

[encryption]
key_file = "/path/to/custom-key"
```

### Sensitivity Levels

| Level | Entropy Threshold | Min Length | Entropy Enabled |
|-------|-------------------|------------|-----------------|
| `default` | 4.0 | 20 | yes |
| `strict` | 3.5 | 16 | yes |
| `relaxed` | (n/a) | (n/a) | no (regex only) |

**Precedence:** Global `[filter]` overrides trump project-level sensitivity defaults. Extra patterns are additive only — user patterns cannot remove built-in patterns. Invalid regex patterns fail at config load time with a clear error message.

### Integration

`handle_record()` loads config once per invocation, derives project from `cwd`, resolves `FilterParams`, and creates a `SecretFilter::with_params()` instance. The global `FILTER` singleton remains for backward-compatible codepaths (serve, purge).

**Dependency:** `toml = "0.8"` added for config deserialization.

## Consequences

### Positive

- **Deterministic filtering.** Regex patterns produce consistent, testable results. No LLM judgment in the security path.
- **Defense in depth.** Five layers: (1) structured extraction discards raw tool output, (2) regex filtering redacts known secret formats, (3) entropy detection catches format-unknown random tokens, (4) opt-in SQLCipher encryption at rest, (5) `secure_delete` + purge for recovery when all else fails.
- **Configurable per project.** Sensitive projects get stricter thresholds; open-source projects can relax entropy scanning. User-defined patterns extend the denylist without modifying the binary.
- **Minimal performance cost.** `RegexSet::is_match` checks all patterns in a single pass (~100-200ns on the common no-match path). Entropy scan adds ~100-200ns per token only on the slow path. Total filtering cost is sub-microsecond on the common path, ~5-15us when both regex and entropy fire.
- **Auditable.** The `redacted` metadata flag creates a trail of filtering actions without storing the secrets themselves.
- **Encryption available without schema changes.** Same DB format, same queries. Only the on-disk representation changes. Users opt in by creating a key.

### Negative

- **Denylist is incomplete.** New secret formats are not caught until patterns are added. Entropy partially mitigates this for random tokens, but structured secrets with low entropy (short passwords, predictable formats) are not caught.
- **False positives.** Over-redaction loses non-sensitive context. Developers working on auth-related code will see more redaction. Entropy can false-positive on legitimate high-entropy data (compressed content, hashes used as identifiers).
- **Build dependency.** `bundled-sqlcipher` requires system OpenSSL dev headers. This is a build-time cost even for users who never enable encryption.
- **Purge is manual.** Discovering that a secret was stored requires the user to notice and run `nmem purge`. No automatic detection of stored secrets post-write.
- **Config adds a file.** `~/.nmem/config.toml` is another file to manage. Misconfigured extra patterns (invalid regex) fail at load time but could silently degrade if the config file is deleted.

## References

- ADR-001 — Storage layer, `secure_delete` PRAGMA, auto_vacuum, WAL checkpoint, SQLCipher option
- ADR-002 — Structured extraction (no raw tool output stored), observation schema, UserPromptSubmit as high-risk input
- ADR-003 — In-process hooks, `nmem record` as the single write path, WAL checkpoint on shutdown, exit code semantics
- ADR-004 — Single DB with project column, cross-project blast radius
- ADR-005 — Purge command (`nmem purge`), secure_delete integration, FTS5 rebuild after large deletions
- `regex.md` — RegexSet fast rejection, replace_all with Cow, LazyLock compilation, testing patterns
- `serde-json.md` — serde_json::Value recursive walking for JSON-aware metadata redaction
- `rusqlite.md` — `execute_batch` for PRAGMAs, file permissions via `std::os::unix::fs`
- `fts5.md` — FTS5 indexes `content` field (blast radius), external content table behavior
- `sqlcipher.md` — SQLCipher option if encryption-at-rest is reconsidered
- `claude-code-hooks-events.md` — UserPromptSubmit payload structure (highest-risk input)
- [OWASP Secrets Detection](https://owasp.org/www-community/vulnerabilities/Use_of_hard-coded_password) — patterns and classification
- [detect-secrets](https://github.com/Yelp/detect-secrets) — Yelp's secret detection library, pattern reference
- [SQLite secure_delete](https://www.sqlite.org/pragma.html#pragma_secure_delete) — zero-fill behavior on DELETE
- [SQLite WAL](https://www.sqlite.org/wal.html) — frame persistence and checkpoint semantics

## Revision History

| Date | Version | Changes |
|------|---------|---------|
| 2026-02-08 | 0.1 | Stub with framing and dependencies. |
| 2026-02-14 | 1.0 | Full ADR. Threat model, filtering strategy (denylist + redaction), encryption decision (none by default), purge procedure, WAL considerations. Resolves ADR-001's encryption-at-rest open question. |
| 2026-02-14 | 1.1 | Refined. Pattern ordering (specific before broad). Connection string scheme whitelist. JSON-aware metadata redaction. regex crate dependency note. Filter test expectations table. |
| 2026-02-14 | 1.2 | Refined with regex.md. Complete SecretFilter implementation: RegexSet fast rejection, LazyLock singleton, Cow-aware replace_all, matches().into_iter() targeted replacement. Added regex.md and serde-json.md references. Performance numbers from regex.md benchmarks. |
| 2026-02-14 | 1.3 | Refined with all library topics. File permissions implementation sketch. References: rusqlite.md, fts5.md, sqlcipher.md, claude-code-hooks-events.md, ADR-005. |
| 2026-02-14 | 2.0 | **Defense-in-depth implementation.** Resolved Q1: Shannon entropy detection as Phase 2 filter (allowlist for SHAs/UUIDs/paths, threshold 4.0 bits/char, min length 20). Resolved Q2: per-project config file (`~/.nmem/config.toml`) with sensitivity levels (default/strict/relaxed), extra patterns, entropy overrides. Revised encryption decision: SQLCipher opt-in via key (`bundled-sqlcipher` feature, raw hex key, auto-migration from unencrypted). Added `nmem encrypt` subcommand. Updated consequences to reflect five-layer defense. |
