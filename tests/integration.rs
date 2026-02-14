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
    nmem_cmd(db)
        .arg("record")
        .write_stdin(format!(
            r#"{{"session_id":"{session_id}","cwd":"/home/test/workspace/myproj","hook_event_name":"PostToolUse","tool_name":"{tool_name}","tool_input":{tool_input}}}"#
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
