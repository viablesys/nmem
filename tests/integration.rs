use assert_cmd::Command;
use std::path::PathBuf;
use tempfile::TempDir;

#[allow(deprecated)]
fn nmem_cmd(db_path: &PathBuf) -> Command {
    let mut cmd = Command::cargo_bin("nmem").unwrap();
    cmd.env("NMEM_DB", db_path);
    cmd
}

fn session_start(db: &PathBuf, session_id: &str) {
    session_start_project(db, session_id, "myproj");
}

fn session_start_project(db: &PathBuf, session_id: &str, project: &str) {
    nmem_cmd(db)
        .arg("record")
        .write_stdin(format!(
            r#"{{"session_id":"{session_id}","cwd":"/home/test/workspace/{project}","hook_event_name":"SessionStart"}}"#
        ))
        .assert()
        .success();
}

fn user_prompt(db: &PathBuf, session_id: &str, prompt: &str) {
    nmem_cmd(db)
        .arg("record")
        .write_stdin(format!(
            r#"{{"session_id":"{session_id}","cwd":"/home/test/workspace/myproj","hook_event_name":"UserPromptSubmit","prompt":"{prompt}"}}"#
        ))
        .assert()
        .success();
}

fn post_tool_use(db: &PathBuf, session_id: &str, tool_name: &str, tool_input: &str) {
    post_tool_use_project(db, session_id, "myproj", tool_name, tool_input);
}

fn post_tool_use_project(db: &PathBuf, session_id: &str, project: &str, tool_name: &str, tool_input: &str) {
    nmem_cmd(db)
        .arg("record")
        .write_stdin(format!(
            r#"{{"session_id":"{session_id}","cwd":"/home/test/workspace/{project}","hook_event_name":"PostToolUse","tool_name":"{tool_name}","tool_input":{tool_input}}}"#
        ))
        .assert()
        .success();
}

fn stop(db: &PathBuf, session_id: &str) {
    nmem_cmd(db)
        .arg("record")
        .write_stdin(format!(
            r#"{{"session_id":"{session_id}","cwd":"/home/test/workspace/myproj","hook_event_name":"Stop"}}"#
        ))
        .assert()
        .success();
}

fn query_db(db: &PathBuf, sql: &str) -> Vec<Vec<String>> {
    let conn = rusqlite::Connection::open_with_flags(
        db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let mut stmt = conn.prepare(sql).unwrap();
    let col_count = stmt.column_count();
    let rows: Vec<Vec<String>> = stmt
        .query_map([], |row| {
            let mut cols = Vec::new();
            for i in 0..col_count {
                let val = row.get_ref(i).unwrap();
                let s = match val {
                    rusqlite::types::ValueRef::Null => "NULL".into(),
                    rusqlite::types::ValueRef::Integer(n) => n.to_string(),
                    rusqlite::types::ValueRef::Real(f) => f.to_string(),
                    rusqlite::types::ValueRef::Text(t) => {
                        String::from_utf8_lossy(t).into_owned()
                    }
                    rusqlite::types::ValueRef::Blob(b) => format!("<blob:{}>", b.len()),
                };
                cols.push(s);
            }
            Ok(cols)
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    rows
}

#[test]
fn full_session_lifecycle() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("test.db");

    session_start(&db, "sess-1");
    user_prompt(&db, "sess-1", "Fix the login bug");
    post_tool_use(&db, "sess-1", "Read", r#"{"file_path":"/src/auth.rs"}"#);
    post_tool_use(&db, "sess-1", "Edit", r#"{"file_path":"/src/auth.rs"}"#);
    post_tool_use(&db, "sess-1", "Bash", r#"{"command":"cargo test"}"#);
    stop(&db, "sess-1");

    // Verify session
    let sessions = query_db(&db, "SELECT id, project FROM sessions");
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0][0], "sess-1");
    assert_eq!(sessions[0][1], "myproj");

    // Verify session ended with signature
    let ended = query_db(&db, "SELECT ended_at, signature FROM sessions WHERE id = 'sess-1'");
    assert_ne!(ended[0][0], "NULL");
    assert!(ended[0][1].contains("file_read"));

    // Verify prompt
    let prompts = query_db(&db, "SELECT source, content FROM prompts");
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0][0], "user");
    assert_eq!(prompts[0][1], "Fix the login bug");

    // Verify observations
    let obs = query_db(&db, "SELECT obs_type, tool_name, content FROM observations ORDER BY id");
    assert_eq!(obs.len(), 3);
    assert_eq!(obs[0][0], "file_read");
    assert_eq!(obs[1][0], "file_edit");
    assert_eq!(obs[2][0], "command");
    assert_eq!(obs[2][2], "cargo test");

    // Verify FTS
    let fts = query_db(
        &db,
        "SELECT rowid FROM observations_fts WHERE observations_fts MATCH 'cargo'",
    );
    assert_eq!(fts.len(), 1);
}

#[test]
fn secret_redaction_in_prompt() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("test.db");

    session_start(&db, "sess-2");

    nmem_cmd(&db)
        .arg("record")
        .write_stdin(r#"{"session_id":"sess-2","cwd":"/home/test/workspace/myproj","hook_event_name":"UserPromptSubmit","prompt":"Set key to sk-ant-api03-abcdefghijklmnopqrstuvwxyz"}"#)
        .assert()
        .success();

    let prompts = query_db(&db, "SELECT content FROM prompts WHERE session_id = 'sess-2'");
    assert_eq!(prompts.len(), 1);
    assert!(prompts[0][0].contains("[REDACTED]"));
    assert!(!prompts[0][0].contains("sk-ant-"));
}

#[test]
fn system_reminder_skipped() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("test.db");

    session_start(&db, "sess-3");
    user_prompt(
        &db,
        "sess-3",
        "<system-reminder>This should be ignored</system-reminder>",
    );

    let prompts = query_db(&db, "SELECT COUNT(*) FROM prompts WHERE session_id = 'sess-3'");
    assert_eq!(prompts[0][0], "0");
}

#[test]
fn empty_session_id_ignored() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("test.db");

    nmem_cmd(&db)
        .arg("record")
        .write_stdin(r#"{"session_id":"","hook_event_name":"SessionStart","cwd":"/tmp"}"#)
        .assert()
        .success();

    assert!(!db.exists());
}

#[test]
fn session_start_compact_creates_observation() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("test.db");

    nmem_cmd(&db)
        .arg("record")
        .write_stdin(r#"{"session_id":"sess-4","cwd":"/home/test/workspace/myproj","hook_event_name":"SessionStart","source":"compact"}"#)
        .assert()
        .success();

    let obs = query_db(
        &db,
        "SELECT obs_type, source_event FROM observations WHERE session_id = 'sess-4'",
    );
    assert_eq!(obs.len(), 1);
    assert_eq!(obs[0][0], "session_compact");
    assert_eq!(obs[0][1], "SessionStart");
}

#[test]
fn mcp_tool_classified_correctly() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("test.db");

    session_start(&db, "sess-5");
    post_tool_use(
        &db,
        "sess-5",
        "mcp__context7__query-docs",
        r#"{"libraryId":"/vercel/next.js","query":"routing"}"#,
    );

    let obs = query_db(
        &db,
        "SELECT obs_type FROM observations WHERE session_id = 'sess-5'",
    );
    assert_eq!(obs[0][0], "mcp_call");
}

// --- Encryption tests ---

#[test]
#[allow(deprecated)]
fn encrypted_database_works() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("test.db");
    let test_key = "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2";

    // Session start with encryption key
    let mut cmd = Command::cargo_bin("nmem").unwrap();
    cmd.env("NMEM_DB", &db)
        .env("NMEM_KEY", test_key)
        .arg("record")
        .write_stdin(
            r#"{"session_id":"enc-1","cwd":"/home/test/workspace/myproj","hook_event_name":"SessionStart"}"#,
        )
        .assert()
        .success();

    // Record a prompt
    let mut cmd = Command::cargo_bin("nmem").unwrap();
    cmd.env("NMEM_DB", &db)
        .env("NMEM_KEY", test_key)
        .arg("record")
        .write_stdin(
            r#"{"session_id":"enc-1","cwd":"/home/test/workspace/myproj","hook_event_name":"UserPromptSubmit","prompt":"Hello encrypted world"}"#,
        )
        .assert()
        .success();

    // Verify data accessible with key
    let conn = rusqlite::Connection::open_with_flags(
        &db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let pragma_value = format!("x'{test_key}'");
    conn.pragma_update(None, "key", &pragma_value).unwrap();
    let count: i64 = conn
        .query_row("SELECT count(*) FROM prompts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);

    // Verify NOT accessible without key
    let conn2 = rusqlite::Connection::open_with_flags(
        &db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let result = conn2.query_row("SELECT count(*) FROM sqlite_master", [], |_| Ok(()));
    assert!(result.is_err(), "should fail without encryption key");
}

// --- Config tests ---

#[test]
#[allow(deprecated)]
fn config_extra_pattern_redacts() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("test.db");

    // Write a config with a custom pattern
    let config_path = dir.path().join("config.toml");
    std::fs::write(
        &config_path,
        r#"
[filter]
extra_patterns = ["MYCO-[A-Za-z0-9]{32}"]
"#,
    )
    .unwrap();

    // Start session with NMEM_CONFIG pointing to our config
    let mut cmd = Command::cargo_bin("nmem").unwrap();
    cmd.env("NMEM_DB", &db)
        .env("NMEM_CONFIG", &config_path)
        .arg("record")
        .write_stdin(
            r#"{"session_id":"cfg-1","cwd":"/home/test/workspace/myproj","hook_event_name":"SessionStart"}"#,
        )
        .assert()
        .success();

    // Record a prompt containing the custom pattern
    let mut cmd = Command::cargo_bin("nmem").unwrap();
    cmd.env("NMEM_DB", &db)
        .env("NMEM_CONFIG", &config_path)
        .arg("record")
        .write_stdin(
            r#"{"session_id":"cfg-1","cwd":"/home/test/workspace/myproj","hook_event_name":"UserPromptSubmit","prompt":"Use key MYCO-abcdefghijklmnopqrstuvwxyz012345 in production"}"#,
        )
        .assert()
        .success();

    let prompts = query_db(&db, "SELECT content FROM prompts WHERE session_id = 'cfg-1'");
    assert_eq!(prompts.len(), 1);
    assert!(
        prompts[0][0].contains("[REDACTED]"),
        "custom pattern should redact"
    );
    assert!(
        !prompts[0][0].contains("MYCO-"),
        "original token should not be present"
    );
}

// --- Entropy tests ---

#[test]
fn entropy_redaction_in_prompt() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("test.db");

    session_start(&db, "ent-1");

    // Mixed-case hex blob with no regex-detectable prefix — only caught by entropy
    let hex = "c8EB7Fa171ac826Ca6EfcEe4847BB8CdCcb74Af2134E5FdD2ccDeA8B0F3FB8Ea";
    nmem_cmd(&db)
        .arg("record")
        .write_stdin(format!(
            r#"{{"session_id":"ent-1","cwd":"/home/test/workspace/myproj","hook_event_name":"UserPromptSubmit","prompt":"Use key {hex} in prod"}}"#
        ))
        .assert()
        .success();

    let prompts = query_db(&db, "SELECT content FROM prompts WHERE session_id = 'ent-1'");
    assert_eq!(prompts.len(), 1);
    assert!(
        prompts[0][0].contains("[REDACTED]"),
        "entropy should redact high-entropy token"
    );
    assert!(
        !prompts[0][0].contains(hex),
        "original hex should not be present"
    );
}

// --- Purge tests ---

#[test]
fn purge_by_id() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("test.db");

    session_start(&db, "p-sess-1");
    post_tool_use(&db, "p-sess-1", "Read", r#"{"file_path":"/src/a.rs"}"#);
    post_tool_use(&db, "p-sess-1", "Read", r#"{"file_path":"/src/b.rs"}"#);

    let obs = query_db(&db, "SELECT id FROM observations ORDER BY id");
    assert_eq!(obs.len(), 2);
    let target_id = &obs[0][0];

    // Dry run — nothing deleted
    nmem_cmd(&db)
        .args(["purge", "--id", target_id])
        .assert()
        .success();
    assert_eq!(query_db(&db, "SELECT COUNT(*) FROM observations")[0][0], "2");

    // Confirm delete
    nmem_cmd(&db)
        .args(["purge", "--id", target_id, "--confirm"])
        .assert()
        .success();

    let remaining = query_db(&db, "SELECT id FROM observations");
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0][0], obs[1][0]);
}

#[test]
fn purge_by_session() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("test.db");

    session_start(&db, "p-sess-a");
    user_prompt(&db, "p-sess-a", "Do something");
    post_tool_use(&db, "p-sess-a", "Read", r#"{"file_path":"/src/a.rs"}"#);

    session_start(&db, "p-sess-b");
    post_tool_use(&db, "p-sess-b", "Read", r#"{"file_path":"/src/b.rs"}"#);

    nmem_cmd(&db)
        .args(["purge", "--session", "p-sess-a", "--confirm"])
        .assert()
        .success();

    // Session a gone entirely
    assert_eq!(
        query_db(&db, "SELECT COUNT(*) FROM sessions WHERE id = 'p-sess-a'")[0][0],
        "0"
    );
    assert_eq!(
        query_db(&db, "SELECT COUNT(*) FROM prompts WHERE session_id = 'p-sess-a'")[0][0],
        "0"
    );
    assert_eq!(
        query_db(&db, "SELECT COUNT(*) FROM observations WHERE session_id = 'p-sess-a'")[0][0],
        "0"
    );

    // Session b untouched
    assert_eq!(
        query_db(&db, "SELECT COUNT(*) FROM sessions WHERE id = 'p-sess-b'")[0][0],
        "1"
    );
    assert_eq!(
        query_db(&db, "SELECT COUNT(*) FROM observations WHERE session_id = 'p-sess-b'")[0][0],
        "1"
    );
}

#[test]
fn purge_by_project() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("test.db");

    session_start_project(&db, "proj-a1", "alpha");
    post_tool_use(&db, "proj-a1", "Read", r#"{"file_path":"/src/a.rs"}"#);
    user_prompt(&db, "proj-a1", "Alpha work");

    session_start_project(&db, "proj-b1", "beta");
    post_tool_use(&db, "proj-b1", "Read", r#"{"file_path":"/src/b.rs"}"#);

    nmem_cmd(&db)
        .args(["purge", "--project", "alpha", "--confirm"])
        .assert()
        .success();

    assert_eq!(
        query_db(&db, "SELECT COUNT(*) FROM sessions WHERE project = 'alpha'")[0][0],
        "0"
    );
    assert_eq!(
        query_db(&db, "SELECT COUNT(*) FROM observations WHERE session_id = 'proj-a1'")[0][0],
        "0"
    );
    assert_eq!(
        query_db(&db, "SELECT COUNT(*) FROM prompts WHERE session_id = 'proj-a1'")[0][0],
        "0"
    );

    // Beta untouched
    assert_eq!(
        query_db(&db, "SELECT COUNT(*) FROM sessions WHERE project = 'beta'")[0][0],
        "1"
    );
}

#[test]
fn purge_by_search() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("test.db");

    session_start(&db, "fts-sess");
    post_tool_use(&db, "fts-sess", "Bash", r#"{"command":"cargo test"}"#);
    post_tool_use(&db, "fts-sess", "Bash", r#"{"command":"git status"}"#);

    // Verify both exist
    assert_eq!(query_db(&db, "SELECT COUNT(*) FROM observations")[0][0], "2");

    // Purge by FTS match
    nmem_cmd(&db)
        .args(["purge", "--search", "cargo", "--confirm"])
        .assert()
        .success();

    let remaining = query_db(&db, "SELECT content FROM observations");
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0][0], "git status");

    // FTS also cleaned up
    let fts = query_db(
        &db,
        "SELECT rowid FROM observations_fts WHERE observations_fts MATCH 'cargo'",
    );
    assert_eq!(fts.len(), 0);
}

#[test]
fn purge_requires_confirm() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("test.db");

    session_start(&db, "noconf-sess");
    post_tool_use(&db, "noconf-sess", "Read", r#"{"file_path":"/src/a.rs"}"#);

    // Without --confirm, nothing deleted
    nmem_cmd(&db)
        .args(["purge", "--session", "noconf-sess"])
        .assert()
        .success();

    assert_eq!(query_db(&db, "SELECT COUNT(*) FROM observations")[0][0], "1");
    assert_eq!(query_db(&db, "SELECT COUNT(*) FROM sessions")[0][0], "1");
}

#[test]
fn purge_no_match() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("test.db");

    session_start(&db, "nm-sess");
    post_tool_use(&db, "nm-sess", "Read", r#"{"file_path":"/src/a.rs"}"#);

    nmem_cmd(&db)
        .args(["purge", "--session", "nonexistent", "--confirm"])
        .assert()
        .success();

    // Everything still there
    assert_eq!(query_db(&db, "SELECT COUNT(*) FROM observations")[0][0], "1");
    assert_eq!(query_db(&db, "SELECT COUNT(*) FROM sessions")[0][0], "1");
}

#[test]
fn purge_no_filter_fails() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("test.db");

    session_start(&db, "nf-sess");

    nmem_cmd(&db)
        .args(["purge", "--confirm"])
        .assert()
        .failure();
}

#[test]
fn purge_fts_sync() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("test.db");

    session_start(&db, "fts-sync-sess");
    post_tool_use(
        &db,
        "fts-sync-sess",
        "Bash",
        r#"{"command":"special_unique_command_xyz"}"#,
    );

    // FTS finds it before purge
    let fts_before = query_db(
        &db,
        "SELECT rowid FROM observations_fts WHERE observations_fts MATCH 'special_unique_command_xyz'",
    );
    assert_eq!(fts_before.len(), 1);

    nmem_cmd(&db)
        .args(["purge", "--session", "fts-sync-sess", "--confirm"])
        .assert()
        .success();

    // FTS no longer finds it
    let fts_after = query_db(
        &db,
        "SELECT rowid FROM observations_fts WHERE observations_fts MATCH 'special_unique_command_xyz'",
    );
    assert_eq!(fts_after.len(), 0);
}

#[test]
fn purge_by_type_and_age() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("test.db");

    session_start(&db, "ta-sess");
    post_tool_use(&db, "ta-sess", "Read", r#"{"file_path":"/src/a.rs"}"#);
    post_tool_use(&db, "ta-sess", "Bash", r#"{"command":"cargo build"}"#);

    // Both file_read and command exist
    assert_eq!(query_db(&db, "SELECT COUNT(*) FROM observations")[0][0], "2");

    // Purge file_read older than 0 days (everything — timestamps are from "now")
    // Since older_than=0 means cutoff=now, and records were just created, they might not match.
    // Use older_than=1 to be safe (cutoff = 1 day ago, so recent records are NOT older).
    // Actually, we need records to BE older. Let's use a large value instead.
    // older_than=0 => cutoff = now, records just written have timestamp <= now, so they match.
    nmem_cmd(&db)
        .args(["purge", "--type", "file_read", "--older-than", "0", "--confirm"])
        .assert()
        .success();

    // Only command remains
    let remaining = query_db(&db, "SELECT obs_type FROM observations");
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0][0], "command");
}

// --- Sweep tests ---

#[test]
#[allow(deprecated)]
fn maintain_sweep_flag() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("test.db");
    let config_path = dir.path().join("config.toml");

    std::fs::write(
        &config_path,
        r#"
[retention]
enabled = true

[retention.days]
file_read = 0
command = 9999
"#,
    )
    .unwrap();

    session_start(&db, "sw-maint");
    post_tool_use(&db, "sw-maint", "Read", r#"{"file_path":"/src/a.rs"}"#);
    post_tool_use(&db, "sw-maint", "Bash", r#"{"command":"cargo test"}"#);

    // Mark session as summarized (sweep precondition: only sweep summarized sessions)
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute("UPDATE sessions SET summary = '{}' WHERE id = 'sw-maint'", []).unwrap();
    }

    assert_eq!(query_db(&db, "SELECT COUNT(*) FROM observations")[0][0], "2");

    // Run maintain --sweep with retention config
    let mut cmd = Command::cargo_bin("nmem").unwrap();
    cmd.env("NMEM_DB", &db)
        .env("NMEM_CONFIG", &config_path)
        .args(["maintain", "--sweep"])
        .assert()
        .success();

    // file_read (retention 0 days) should be deleted, command (9999 days) kept
    let remaining = query_db(&db, "SELECT obs_type FROM observations");
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0][0], "command");
}

#[test]
fn sweep_disabled_by_default() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("test.db");
    let config_path = dir.path().join("config.toml");

    // Explicitly disable retention (default is now enabled)
    std::fs::write(&config_path, "[retention]\nenabled = false\n").unwrap();

    session_start(&db, "sw-disabled");
    post_tool_use(&db, "sw-disabled", "Read", r#"{"file_path":"/src/a.rs"}"#);

    let mut cmd = Command::cargo_bin("nmem").unwrap();
    cmd.env("NMEM_DB", &db)
        .env("NMEM_CONFIG", &config_path)
        .args(["maintain", "--sweep"])
        .assert()
        .success();

    // Nothing deleted
    assert_eq!(query_db(&db, "SELECT COUNT(*) FROM observations")[0][0], "1");
}

#[test]
#[allow(deprecated)]
fn sweep_on_session_stop() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("test.db");
    let config_path = dir.path().join("config.toml");

    std::fs::write(
        &config_path,
        r#"
[retention]
enabled = true

[retention.days]
file_read = 0
"#,
    )
    .unwrap();

    // Create initial session and seed >100 old observations to trigger sweep threshold
    session_start(&db, "sw-ss-setup");
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        // Mark session as summarized (sweep precondition)
        conn.execute("UPDATE sessions SET summary = '{}' WHERE id = 'sw-ss-setup'", []).unwrap();
        let old_ts = 1000i64; // very old timestamp
        for i in 0..110 {
            conn.execute(
                "INSERT INTO observations (session_id, prompt_id, timestamp, obs_type, source_event, content)
                 VALUES ('sw-ss-setup', NULL, ?1, 'file_read', 'PostToolUse', ?2)",
                rusqlite::params![old_ts, format!("old-{i}")],
            ).unwrap();
        }
    }

    // Verify observations exist
    let before: String = query_db(&db, "SELECT COUNT(*) FROM observations")[0][0].clone();
    assert!(before.parse::<i32>().unwrap() >= 110);

    // Stop with retention config triggers opportunistic sweep
    let mut cmd = Command::cargo_bin("nmem").unwrap();
    cmd.env("NMEM_DB", &db)
        .env("NMEM_CONFIG", &config_path)
        .arg("record")
        .write_stdin(
            r#"{"session_id":"sw-ss-setup","cwd":"/home/test/workspace/myproj","hook_event_name":"Stop"}"#,
        )
        .assert()
        .success();

    // Old file_read observations should be swept
    let after: String = query_db(&db, "SELECT COUNT(*) FROM observations WHERE obs_type = 'file_read'")[0][0].clone();
    assert_eq!(after, "0", "expired file_read observations should be swept on Stop");
}

// --- Maintain tests ---

#[test]
fn maintain_on_empty_db() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("test.db");

    // Create the DB by starting a session (so tables exist)
    session_start(&db, "m-empty");

    nmem_cmd(&db)
        .arg("maintain")
        .assert()
        .success();
}

#[test]
fn maintain_after_data() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("test.db");

    session_start(&db, "m-data");
    user_prompt(&db, "m-data", "Test maintain with data");
    post_tool_use(&db, "m-data", "Bash", r#"{"command":"cargo build"}"#);
    post_tool_use(&db, "m-data", "Read", r#"{"file_path":"/src/main.rs"}"#);
    stop(&db, "m-data");

    nmem_cmd(&db)
        .arg("maintain")
        .assert()
        .success();

    // Verify data intact
    assert_eq!(query_db(&db, "SELECT COUNT(*) FROM observations")[0][0], "2");
    assert_eq!(query_db(&db, "SELECT COUNT(*) FROM prompts")[0][0], "1");
    assert_eq!(query_db(&db, "SELECT COUNT(*) FROM sessions")[0][0], "1");
}

#[test]
fn maintain_rebuild_fts() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("test.db");

    session_start(&db, "m-fts");
    post_tool_use(&db, "m-fts", "Bash", r#"{"command":"cargo test"}"#);
    user_prompt(&db, "m-fts", "Run the test suite");

    nmem_cmd(&db)
        .args(["maintain", "--rebuild-fts"])
        .assert()
        .success();

    // FTS still works after rebuild
    let fts = query_db(
        &db,
        "SELECT rowid FROM observations_fts WHERE observations_fts MATCH 'cargo'",
    );
    assert_eq!(fts.len(), 1);

    let fts_prompts = query_db(
        &db,
        "SELECT rowid FROM prompts_fts WHERE prompts_fts MATCH 'suite'",
    );
    assert_eq!(fts_prompts.len(), 1);
}

#[test]
fn maintain_fts_integrity() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("test.db");

    session_start(&db, "m-int");
    post_tool_use(&db, "m-int", "Read", r#"{"file_path":"/src/lib.rs"}"#);

    // maintain should succeed (integrity check passes on a healthy DB)
    nmem_cmd(&db)
        .arg("maintain")
        .assert()
        .success();
}

// --- Search tests ---

#[test]
fn search_basic() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("test.db");

    session_start(&db, "srch-1");
    post_tool_use(&db, "srch-1", "Bash", r#"{"command":"cargo test"}"#);
    post_tool_use(&db, "srch-1", "Read", r#"{"file_path":"/src/main.rs"}"#);

    let out = nmem_cmd(&db)
        .args(["search", "cargo"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&out.get_output().stdout);
    let results: Vec<serde_json::Value> = serde_json::from_str(&stdout).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["obs_type"], "command");
    assert!(results[0]["content_preview"].as_str().unwrap().contains("cargo test"));
    assert!(results[0]["id"].is_number());
    assert!(results[0]["timestamp"].is_number());
    assert!(results[0]["session_id"].is_string());

    let stderr = String::from_utf8_lossy(&out.get_output().stderr);
    assert!(stderr.contains("1 results"));
}

#[test]
fn search_with_project_filter() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("test.db");

    session_start_project(&db, "srch-p1", "alpha");
    post_tool_use(&db, "srch-p1", "Bash", r#"{"command":"cargo build"}"#);

    session_start_project(&db, "srch-p2", "beta");
    post_tool_use(&db, "srch-p2", "Bash", r#"{"command":"cargo test"}"#);

    let out = nmem_cmd(&db)
        .args(["search", "cargo", "--project", "alpha"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&out.get_output().stdout);
    let results: Vec<serde_json::Value> = serde_json::from_str(&stdout).unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0]["content_preview"].as_str().unwrap().contains("cargo build"));
}

#[test]
fn search_with_type_filter() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("test.db");

    session_start(&db, "srch-t1");
    post_tool_use(&db, "srch-t1", "Bash", r#"{"command":"cargo test"}"#);
    post_tool_use(&db, "srch-t1", "Read", r#"{"file_path":"/src/cargo.toml"}"#);

    let out = nmem_cmd(&db)
        .args(["search", "cargo", "--type", "command"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&out.get_output().stdout);
    let results: Vec<serde_json::Value> = serde_json::from_str(&stdout).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["obs_type"], "command");
}

#[test]
fn search_ids_mode() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("test.db");

    session_start(&db, "srch-ids");
    post_tool_use(&db, "srch-ids", "Bash", r#"{"command":"cargo test"}"#);
    post_tool_use(&db, "srch-ids", "Bash", r#"{"command":"cargo build"}"#);

    let out = nmem_cmd(&db)
        .args(["search", "cargo", "--ids"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&out.get_output().stdout);
    let lines: Vec<&str> = stdout.trim().lines().collect();
    assert_eq!(lines.len(), 2);
    // Each line should be a numeric ID
    for line in &lines {
        assert!(line.parse::<i64>().is_ok(), "expected numeric ID, got: {line}");
    }
}

#[test]
fn search_full_mode() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("test.db");

    session_start(&db, "srch-full");
    post_tool_use(&db, "srch-full", "Bash", r#"{"command":"cargo test"}"#);

    let out = nmem_cmd(&db)
        .args(["search", "cargo", "--full"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&out.get_output().stdout);
    let results: Vec<serde_json::Value> = serde_json::from_str(&stdout).unwrap();
    assert_eq!(results.len(), 1);
    // Full mode includes extra fields
    assert!(results[0]["source_event"].is_string());
    assert!(results[0]["content"].is_string());
    assert_eq!(results[0]["content"], "cargo test");
}

#[test]
fn search_no_results() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("test.db");

    session_start(&db, "srch-empty");
    post_tool_use(&db, "srch-empty", "Bash", r#"{"command":"cargo test"}"#);

    let out = nmem_cmd(&db)
        .args(["search", "nonexistent_xyz_zzz"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&out.get_output().stdout);
    let results: Vec<serde_json::Value> = serde_json::from_str(&stdout).unwrap();
    assert!(results.is_empty());

    let stderr = String::from_utf8_lossy(&out.get_output().stderr);
    assert!(stderr.contains("0 results"));
}

#[test]
fn search_no_db() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("nonexistent.db");

    nmem_cmd(&db)
        .args(["search", "anything"])
        .assert()
        .failure();
}

// --- Pin tests ---

#[test]
fn pin_observation() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("test.db");

    session_start(&db, "pin-1");
    post_tool_use(&db, "pin-1", "Read", r#"{"file_path":"/src/a.rs"}"#);

    let obs = query_db(&db, "SELECT id FROM observations");
    let id = &obs[0][0];

    nmem_cmd(&db)
        .args(["pin", id])
        .assert()
        .success();

    let pinned = query_db(&db, &format!("SELECT is_pinned FROM observations WHERE id = {id}"));
    assert_eq!(pinned[0][0], "1");
}

#[test]
fn unpin_observation() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("test.db");

    session_start(&db, "unpin-1");
    post_tool_use(&db, "unpin-1", "Read", r#"{"file_path":"/src/a.rs"}"#);

    let obs = query_db(&db, "SELECT id FROM observations");
    let id = &obs[0][0];

    // Pin then unpin
    nmem_cmd(&db).args(["pin", id]).assert().success();
    nmem_cmd(&db).args(["unpin", id]).assert().success();

    let pinned = query_db(&db, &format!("SELECT is_pinned FROM observations WHERE id = {id}"));
    assert_eq!(pinned[0][0], "0");
}

#[test]
fn pin_nonexistent_fails() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("test.db");

    session_start(&db, "pin-ne");

    nmem_cmd(&db)
        .args(["pin", "99999"])
        .assert()
        .failure();
}

#[test]
#[allow(deprecated)]
fn sweep_skips_pinned() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("test.db");
    let config_path = dir.path().join("config.toml");

    std::fs::write(
        &config_path,
        r#"
[retention]
enabled = true

[retention.days]
file_read = 0
"#,
    )
    .unwrap();

    session_start(&db, "sw-pin");
    post_tool_use(&db, "sw-pin", "Read", r#"{"file_path":"/src/a.rs"}"#);
    post_tool_use(&db, "sw-pin", "Read", r#"{"file_path":"/src/b.rs"}"#);

    // Mark session as summarized (sweep precondition)
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute("UPDATE sessions SET summary = '{}' WHERE id = 'sw-pin'", []).unwrap();
    }

    // Pin the first observation
    let obs = query_db(&db, "SELECT id FROM observations ORDER BY id");
    let pinned_id = &obs[0][0];
    nmem_cmd(&db).args(["pin", pinned_id]).assert().success();

    assert_eq!(query_db(&db, "SELECT COUNT(*) FROM observations")[0][0], "2");

    // Run sweep — should delete only the unpinned one
    let mut cmd = Command::cargo_bin("nmem").unwrap();
    cmd.env("NMEM_DB", &db)
        .env("NMEM_CONFIG", &config_path)
        .args(["maintain", "--sweep"])
        .assert()
        .success();

    let remaining = query_db(&db, "SELECT id, is_pinned FROM observations");
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0][0], *pinned_id);
    assert_eq!(remaining[0][1], "1");
}

#[test]
fn purge_deletes_pinned() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("test.db");

    session_start(&db, "purge-pin");
    post_tool_use(&db, "purge-pin", "Read", r#"{"file_path":"/src/a.rs"}"#);

    let obs = query_db(&db, "SELECT id FROM observations");
    let id = &obs[0][0];

    // Pin it
    nmem_cmd(&db).args(["pin", id]).assert().success();
    assert_eq!(
        query_db(&db, &format!("SELECT is_pinned FROM observations WHERE id = {id}"))[0][0],
        "1"
    );

    // Purge by ID with --confirm — should delete regardless of pin
    nmem_cmd(&db)
        .args(["purge", "--id", id, "--confirm"])
        .assert()
        .success();

    assert_eq!(query_db(&db, "SELECT COUNT(*) FROM observations")[0][0], "0");
}

#[test]
fn search_shows_pin_status() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("test.db");

    session_start(&db, "srch-pin");
    post_tool_use(&db, "srch-pin", "Bash", r#"{"command":"cargo test"}"#);

    let obs = query_db(&db, "SELECT id FROM observations");
    let id = &obs[0][0];

    nmem_cmd(&db).args(["pin", id]).assert().success();

    let out = nmem_cmd(&db)
        .args(["search", "cargo"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&out.get_output().stdout);
    let results: Vec<serde_json::Value> = serde_json::from_str(&stdout).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["is_pinned"], true);
}

#[test]
fn search_full_shows_pin_status() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("test.db");

    session_start(&db, "srch-fpin");
    post_tool_use(&db, "srch-fpin", "Bash", r#"{"command":"cargo build"}"#);

    let obs = query_db(&db, "SELECT id FROM observations");
    let id = &obs[0][0];

    nmem_cmd(&db).args(["pin", id]).assert().success();

    let out = nmem_cmd(&db)
        .args(["search", "cargo", "--full"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&out.get_output().stdout);
    let results: Vec<serde_json::Value> = serde_json::from_str(&stdout).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["is_pinned"], true);
}

// --- Status tests ---

#[test]
fn status_no_db() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("nonexistent.db");

    let out = nmem_cmd(&db).arg("status").assert().success();
    let stderr = String::from_utf8_lossy(&out.get_output().stderr);
    assert!(stderr.contains("no database"));
}

#[test]
fn status_empty_db() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("test.db");

    session_start(&db, "st-empty");

    let out = nmem_cmd(&db).arg("status").assert().success();
    let stderr = String::from_utf8_lossy(&out.get_output().stderr);
    assert!(stderr.contains("observations"));
    assert!(stderr.contains("sessions"));
}

#[test]
fn status_with_data() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("test.db");

    session_start(&db, "st-data");
    user_prompt(&db, "st-data", "Check status output");
    post_tool_use(&db, "st-data", "Read", r#"{"file_path":"/src/a.rs"}"#);
    post_tool_use(&db, "st-data", "Bash", r#"{"command":"cargo test"}"#);
    stop(&db, "st-data");

    let out = nmem_cmd(&db).arg("status").assert().success();
    let stderr = String::from_utf8_lossy(&out.get_output().stderr);
    assert!(stderr.contains("database"));
    assert!(stderr.contains("observations — 2"));
    assert!(stderr.contains("prompts — 1"));
    assert!(stderr.contains("sessions — 1"));
    assert!(stderr.contains("last session"));
    assert!(stderr.contains("myproj"));
}

// --- Blended search tests ---

#[test]
fn search_blended_order_by() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("test.db");

    session_start(&db, "blend-1");
    post_tool_use(&db, "blend-1", "Bash", r#"{"command":"cargo test"}"#);
    post_tool_use(&db, "blend-1", "Read", r#"{"file_path":"/src/cargo.toml"}"#);

    let out = nmem_cmd(&db)
        .args(["search", "cargo", "--order-by", "blended"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&out.get_output().stdout);
    let results: Vec<serde_json::Value> = serde_json::from_str(&stdout).unwrap();
    // Both match "cargo"
    assert_eq!(results.len(), 2);
    // Each result has expected fields
    assert!(results[0]["id"].is_number());
    assert!(results[0]["timestamp"].is_number());
    assert!(results[0]["obs_type"].is_string());
}

#[test]
fn search_blended_ids_mode() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("test.db");

    session_start(&db, "blend-ids");
    post_tool_use(&db, "blend-ids", "Bash", r#"{"command":"cargo test"}"#);
    post_tool_use(&db, "blend-ids", "Bash", r#"{"command":"cargo build"}"#);

    let out = nmem_cmd(&db)
        .args(["search", "cargo", "--ids", "--order-by", "blended"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&out.get_output().stdout);
    let lines: Vec<&str> = stdout.trim().lines().collect();
    assert_eq!(lines.len(), 2);
    for line in &lines {
        assert!(line.parse::<i64>().is_ok(), "expected numeric ID, got: {line}");
    }
}

#[test]
fn search_invalid_order_by_fails() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("test.db");

    session_start(&db, "blend-bad");
    post_tool_use(&db, "blend-bad", "Bash", r#"{"command":"cargo test"}"#);

    nmem_cmd(&db)
        .args(["search", "cargo", "--order-by", "nonsense"])
        .assert()
        .failure();
}

// --- Context injection intent tests ---
// (Intents section removed in favor of episodes — these tests verify the new flow)

#[test]
fn context_injection_no_intents_section() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("test.db");

    // Seed session with prompt + tool uses
    session_start(&db, "int-seed");
    user_prompt(&db, "int-seed", "Fix the login bug");
    post_tool_use(&db, "int-seed", "Read", r#"{"file_path":"/src/auth.rs"}"#);
    post_tool_use(&db, "int-seed", "Edit", r#"{"file_path":"/src/auth.rs"}"#);
    post_tool_use(&db, "int-seed", "Bash", r#"{"command":"cargo test"}"#);
    stop(&db, "int-seed");

    // New session — should NOT have intents section (removed)
    let out = nmem_cmd(&db)
        .arg("record")
        .write_stdin(
            r#"{"session_id":"int-new","cwd":"/home/test/workspace/myproj","hook_event_name":"SessionStart"}"#,
        )
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&out.get_output().stdout);
    assert!(!stdout.contains("## Recent Intents"), "intents section should not exist");
    assert!(stdout.contains("# nmem context"), "should still produce context");
}

// --- Context config tests ---

#[test]
#[allow(deprecated)]
fn context_injection_respects_config_limits() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("test.db");
    let config_path = dir.path().join("config.toml");

    // Config: suppress cross-project context entirely
    std::fs::write(
        &config_path,
        r#"
[projects.alpha]
context_cross_limit = 0
"#,
    )
    .unwrap();

    // Seed local data — file_edit so it appears in context
    session_start_project(&db, "cfg-alpha", "alpha");
    post_tool_use_project(&db, "cfg-alpha", "alpha", "Edit", r#"{"file_path":"/src/main.rs"}"#);
    stop(&db, "cfg-alpha");

    // Seed cross-project data — pin so it would appear if not suppressed
    session_start_project(&db, "cfg-beta", "beta");
    post_tool_use_project(&db, "cfg-beta", "beta", "Edit", r#"{"file_path":"/src/lib.rs"}"#);
    stop(&db, "cfg-beta");

    let obs = query_db(&db, "SELECT id FROM observations WHERE session_id = 'cfg-beta' AND obs_type = 'file_edit'");
    let id = &obs[0][0];
    nmem_cmd(&db).args(["pin", id]).assert().success();

    // New session for alpha with config
    let mut cmd = Command::cargo_bin("nmem").unwrap();
    let out = cmd
        .env("NMEM_DB", &db)
        .env("NMEM_CONFIG", &config_path)
        .arg("record")
        .write_stdin(
            r#"{"session_id":"cfg-alpha2","cwd":"/home/test/workspace/alpha","hook_event_name":"SessionStart"}"#,
        )
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&out.get_output().stdout);
    assert!(stdout.contains("## alpha"), "should contain local project section");
    assert!(!stdout.contains("Other projects"), "cross_limit=0 should suppress cross-project section");
}

#[test]
#[allow(deprecated)]
fn context_injection_suppress_cross_project() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("test.db");
    let config_path = dir.path().join("config.toml");

    std::fs::write(
        &config_path,
        r#"
[projects.alpha]
suppress_cross_project = true
"#,
    )
    .unwrap();

    // Seed local data — file_edit so it appears in context
    session_start_project(&db, "scp-alpha", "alpha");
    post_tool_use_project(&db, "scp-alpha", "alpha", "Edit", r#"{"file_path":"/src/main.rs"}"#);
    stop(&db, "scp-alpha");

    // Seed cross-project data — pin so it would appear if not suppressed
    session_start_project(&db, "scp-beta", "beta");
    post_tool_use_project(&db, "scp-beta", "beta", "Edit", r#"{"file_path":"/src/lib.rs"}"#);
    stop(&db, "scp-beta");

    let obs = query_db(&db, "SELECT id FROM observations WHERE session_id = 'scp-beta' AND obs_type = 'file_edit'");
    let id = &obs[0][0];
    nmem_cmd(&db).args(["pin", id]).assert().success();

    // New session for alpha with suppress_cross_project config
    let mut cmd = Command::cargo_bin("nmem").unwrap();
    let out = cmd
        .env("NMEM_DB", &db)
        .env("NMEM_CONFIG", &config_path)
        .arg("record")
        .write_stdin(
            r#"{"session_id":"scp-alpha2","cwd":"/home/test/workspace/alpha","hook_event_name":"SessionStart"}"#,
        )
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&out.get_output().stdout);
    assert!(stdout.contains("## alpha"), "should contain local project section");
    assert!(!stdout.contains("Other projects"), "suppress_cross_project should suppress cross-project section");
}

// --- Context injection tests ---

#[test]
fn context_injection_on_session_start() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("test.db");

    // Seed data: first session with some observations
    session_start(&db, "ctx-seed");
    post_tool_use(&db, "ctx-seed", "Read", r#"{"file_path":"/src/auth.rs"}"#);
    post_tool_use(&db, "ctx-seed", "Edit", r#"{"file_path":"/src/auth.rs"}"#);
    post_tool_use(&db, "ctx-seed", "Bash", r#"{"command":"cargo test"}"#);
    stop(&db, "ctx-seed");

    // New session — should get context injection in stdout
    let out = nmem_cmd(&db)
        .arg("record")
        .write_stdin(
            r#"{"session_id":"ctx-new","cwd":"/home/test/workspace/myproj","hook_event_name":"SessionStart"}"#,
        )
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&out.get_output().stdout);
    assert!(stdout.contains("# nmem context"), "should contain context header");
    assert!(stdout.contains("## myproj"), "should contain project section");
    assert!(stdout.contains("/src/auth.rs"), "should contain edited file path");
    // Commands only appear if pinned or git commit/push — generic commands are filtered out
    assert!(!stdout.contains("cargo test"), "generic commands should not appear in context");
}

#[test]
fn context_injection_empty_db() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("test.db");

    // First-ever SessionStart — no prior data
    let out = nmem_cmd(&db)
        .arg("record")
        .write_stdin(
            r#"{"session_id":"ctx-empty","cwd":"/home/test/workspace/myproj","hook_event_name":"SessionStart"}"#,
        )
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&out.get_output().stdout);
    assert!(stdout.is_empty(), "empty DB should produce no stdout context");
}

#[test]
fn context_injection_cross_project() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("test.db");

    // Seed data for project alpha — file_edit shows in local context
    session_start_project(&db, "ctx-alpha", "alpha");
    post_tool_use_project(&db, "ctx-alpha", "alpha", "Edit", r#"{"file_path":"/src/main.rs"}"#);
    stop(&db, "ctx-alpha");

    // Seed data for project beta — pin an observation so it appears cross-project
    session_start_project(&db, "ctx-beta", "beta");
    post_tool_use_project(&db, "ctx-beta", "beta", "Edit", r#"{"file_path":"/src/lib.rs"}"#);
    stop(&db, "ctx-beta");

    // Pin beta's observation — cross-project only shows pinned items
    let obs = query_db(&db, "SELECT id FROM observations WHERE session_id = 'ctx-beta' AND obs_type = 'file_edit'");
    let id = &obs[0][0];
    nmem_cmd(&db).args(["pin", id]).assert().success();

    // New session for alpha — should see beta in cross-project (pinned)
    let out = nmem_cmd(&db)
        .arg("record")
        .write_stdin(
            r#"{"session_id":"ctx-alpha2","cwd":"/home/test/workspace/alpha","hook_event_name":"SessionStart"}"#,
        )
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&out.get_output().stdout);
    assert!(stdout.contains("## alpha"), "should contain project section");
    assert!(stdout.contains("Other projects"), "should contain cross-project section");
    assert!(stdout.contains("[beta]"), "cross-project should show beta project name");
}

#[test]
fn context_injection_recovery_mode() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("test.db");

    // Seed data — file_edit so it appears in filtered context
    session_start(&db, "ctx-rec-seed");
    post_tool_use(&db, "ctx-rec-seed", "Edit", r#"{"file_path":"/src/a.rs"}"#);
    stop(&db, "ctx-rec-seed");

    // Normal SessionStart
    let out_normal = nmem_cmd(&db)
        .arg("record")
        .write_stdin(
            r#"{"session_id":"ctx-rec-normal","cwd":"/home/test/workspace/myproj","hook_event_name":"SessionStart"}"#,
        )
        .assert()
        .success();
    let stdout_normal = String::from_utf8_lossy(&out_normal.get_output().stdout);

    // Compact recovery SessionStart
    let out_compact = nmem_cmd(&db)
        .arg("record")
        .write_stdin(
            r#"{"session_id":"ctx-rec-compact","cwd":"/home/test/workspace/myproj","hook_event_name":"SessionStart","source":"compact"}"#,
        )
        .assert()
        .success();
    let stdout_compact = String::from_utf8_lossy(&out_compact.get_output().stdout);

    // Both should produce output (with this small dataset the difference in limits won't matter,
    // but both modes should work without error)
    assert!(stdout_normal.contains("# nmem context"), "normal mode should produce context");
    assert!(stdout_compact.contains("# nmem context"), "compact mode should produce context");
}

#[test]
fn context_injection_shows_pinned() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("test.db");

    session_start(&db, "ctx-pin-seed");
    post_tool_use(&db, "ctx-pin-seed", "Bash", r#"{"command":"important-cmd"}"#);
    stop(&db, "ctx-pin-seed");

    // Pin the observation
    let obs = query_db(&db, "SELECT id FROM observations WHERE content = 'important-cmd'");
    let id = &obs[0][0];
    nmem_cmd(&db).args(["pin", id]).assert().success();

    // New session — should see pin marker
    let out = nmem_cmd(&db)
        .arg("record")
        .write_stdin(
            r#"{"session_id":"ctx-pin-new","cwd":"/home/test/workspace/myproj","hook_event_name":"SessionStart"}"#,
        )
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&out.get_output().stdout);
    assert!(stdout.contains("# nmem context"), "should produce context");
    // Find the row with important-cmd and verify it has pin marker
    let line = stdout.lines().find(|l| l.contains("important-cmd")).expect("should find important-cmd row");
    assert!(line.contains("(pinned)"), "pinned observation should have (pinned) marker");
}

// --- Episode context injection tests ---

#[test]
fn context_injection_shows_episodes() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("test.db");

    // Seed session with prompt + tool uses
    session_start(&db, "ep-seed");
    user_prompt(&db, "ep-seed", "Fix the authentication bug in the login handler");
    post_tool_use(&db, "ep-seed", "Read", r#"{"file_path":"/src/auth.rs"}"#);
    post_tool_use(&db, "ep-seed", "Edit", r#"{"file_path":"/src/auth.rs"}"#);
    stop(&db, "ep-seed");

    // Insert work_units directly (episodes are normally detected at Stop time)
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        conn.execute(
            "INSERT INTO work_units (session_id, started_at, intent, obs_count, hot_files, phase_signature)
             VALUES ('ep-seed', ?1, 'fix authentication bug', 5, '[\"src/auth.rs\",\"src/handler.rs\"]', '{\"investigate\":2,\"execute\":3,\"failures\":0}')",
            [now - 600],
        ).unwrap();
    }

    // New session — should see episodes section
    let out = nmem_cmd(&db)
        .arg("record")
        .write_stdin(
            r#"{"session_id":"ep-new","cwd":"/home/test/workspace/myproj","hook_event_name":"SessionStart"}"#,
        )
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&out.get_output().stdout);
    assert!(stdout.contains("## Recent Episodes"), "should contain episodes section");
    assert!(stdout.contains("fix authentication bug"), "should show episode intent");
    assert!(stdout.contains("5 obs"), "should show observation count");
    assert!(stdout.contains("execute"), "should show phase character");
    assert!(stdout.contains("src/auth.rs"), "should show hot files");
}

#[test]
fn context_injection_episode_fallback() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("test.db");

    // Seed session — old enough to be outside episode window but with summary
    session_start(&db, "fb-seed");
    post_tool_use(&db, "fb-seed", "Edit", r#"{"file_path":"/src/main.rs"}"#);
    stop(&db, "fb-seed");

    // Directly insert an old session summary (simulate LLM summarization)
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        let old_ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64 - 300000; // ~3.5 days ago

        conn.execute(
            "INSERT INTO sessions (id, project, started_at, summary) VALUES ('fb-old', 'myproj', ?1, ?2)",
            rusqlite::params![
                old_ts,
                r#"{"intent":"Add user notifications","completed":["Added notification endpoint"],"learned":["WebSocket preferred over polling"],"next_steps":["Run cargo test"],"files_read":[],"files_edited":[],"notes":null}"#
            ],
        ).unwrap();
    }

    // New session — old session should appear as session summary (no episodes for it)
    let out = nmem_cmd(&db)
        .arg("record")
        .write_stdin(
            r#"{"session_id":"fb-new","cwd":"/home/test/workspace/myproj","hook_event_name":"SessionStart"}"#,
        )
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&out.get_output().stdout);
    assert!(stdout.contains("## Session Summaries"), "should contain session summaries for old sessions");
    assert!(stdout.contains("Add user notifications"), "should show old session intent");
    assert!(stdout.contains("WebSocket preferred"), "should show learned items");
}

#[test]
fn context_injection_suggested_tasks() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("test.db");

    // Seed session with summary that has next_steps
    session_start(&db, "st-seed");
    post_tool_use(&db, "st-seed", "Edit", r#"{"file_path":"/src/main.rs"}"#);
    stop(&db, "st-seed");

    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64 - 3600;

        conn.execute(
            "UPDATE sessions SET started_at = ?1, summary = ?2 WHERE id = 'st-seed'",
            rusqlite::params![
                ts,
                r#"{"intent":"Implement feature X","completed":["Added endpoint"],"learned":[],"next_steps":["Run cargo test after changes","Update documentation"],"files_read":[],"files_edited":[],"notes":null}"#
            ],
        ).unwrap();
    }

    // New session — should see suggested tasks
    let out = nmem_cmd(&db)
        .arg("record")
        .write_stdin(
            r#"{"session_id":"st-new","cwd":"/home/test/workspace/myproj","hook_event_name":"SessionStart"}"#,
        )
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&out.get_output().stdout);
    assert!(stdout.contains("## Suggested Tasks"), "should contain suggested tasks section");
    assert!(stdout.contains("Run cargo test after changes"), "should show next step from summary");
    assert!(stdout.contains("Update documentation"), "should show second next step");
}
