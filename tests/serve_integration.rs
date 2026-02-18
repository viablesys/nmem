use nmem::db::register_udfs;
use nmem::serve::{
    FileHistoryParams, GetObservationsParams, NmemServer, RecentContextParams, SearchParams,
    SessionSummariesParams, SessionTraceParams, TimelineParams,
};
use rusqlite::Connection;
use std::sync::{Arc, Mutex};

fn test_db() -> Arc<Mutex<Connection>> {
    let mut conn = Connection::open_in_memory().unwrap();
    nmem::schema_migrations().to_latest(&mut conn).unwrap();
    register_udfs(&conn).unwrap();

    conn.execute_batch(
        "
        INSERT INTO sessions (id, project, started_at) VALUES ('sess-a', 'myproj', 1707400000);
        INSERT INTO sessions (id, project, started_at) VALUES ('sess-b', 'other', 1707400100);

        INSERT INTO prompts (id, session_id, timestamp, source, content)
            VALUES (1, 'sess-a', 1707400010, 'user', 'Fix the login bug');

        INSERT INTO observations (id, session_id, prompt_id, timestamp, obs_type, source_event, tool_name, file_path, content, metadata)
            VALUES (1, 'sess-a', 1, 1707400020, 'file_read', 'PostToolUse', 'Read', '/src/auth.rs', 'Read /src/auth.rs', NULL);
        INSERT INTO observations (id, session_id, prompt_id, timestamp, obs_type, source_event, tool_name, file_path, content, metadata)
            VALUES (2, 'sess-a', 1, 1707400030, 'file_edit', 'PostToolUse', 'Edit', '/src/auth.rs', 'Edit /src/auth.rs: fix token validation', '{\"redacted\":false}');
        INSERT INTO observations (id, session_id, prompt_id, timestamp, obs_type, source_event, tool_name, file_path, content, metadata)
            VALUES (3, 'sess-a', 1, 1707400040, 'command', 'PostToolUse', 'Bash', NULL, 'cargo test -- auth::tests', NULL);
        INSERT INTO observations (id, session_id, prompt_id, timestamp, obs_type, source_event, tool_name, file_path, content, metadata)
            VALUES (4, 'sess-a', 1, 1707400050, 'command_error', 'PostToolUse', 'Bash', NULL, 'cargo test failed with assertion error on line 42', NULL);
        INSERT INTO observations (id, session_id, prompt_id, timestamp, obs_type, source_event, tool_name, file_path, content, metadata)
            VALUES (5, 'sess-b', NULL, 1707400110, 'file_read', 'PostToolUse', 'Read', '/src/main.rs', 'Read /src/main.rs for other project', NULL);
        INSERT INTO observations (id, session_id, prompt_id, timestamp, obs_type, source_event, tool_name, file_path, content, metadata)
            VALUES (6, 'sess-a', 1, 1707400060, 'file_edit', 'PostToolUse', 'Edit', '/src/auth.rs', 'Edit /src/auth.rs: second edit to fix tests', NULL);
        ",
    )
    .unwrap();

    Arc::new(Mutex::new(conn))
}

fn make_server() -> NmemServer {
    NmemServer::new(test_db())
}

/// Extract result text from a CallToolResult
fn result_text(result: &rmcp::model::CallToolResult) -> String {
    result
        .content
        .first()
        .map(|c| c.as_text().unwrap().text.clone())
        .unwrap_or_default()
}

fn result_json(result: &rmcp::model::CallToolResult) -> serde_json::Value {
    serde_json::from_str(&result_text(result)).unwrap()
}

// --- search tests ---

#[test]
fn search_finds_by_content() {
    let server = make_server();
    let result = server
        .do_search(SearchParams {
            query: "cargo test".into(),
            project: None,
            obs_type: None,
            limit: None,
            offset: None,
            order_by: None,
            before: None,
            after: None,
        })
        .unwrap();

    assert!(!result.is_error.unwrap_or(false));
    let arr = result_json(&result).as_array().unwrap().clone();
    assert!(arr.len() >= 2);
}

#[test]
fn search_filters_by_project() {
    let server = make_server();
    let result = server
        .do_search(SearchParams {
            query: "Read".into(),
            project: Some("myproj".into()),
            obs_type: None,
            limit: None,
            offset: None,
            order_by: None,
            before: None,
            after: None,
        })
        .unwrap();

    let arr = result_json(&result);
    for item in arr.as_array().unwrap() {
        assert_eq!(item["session_id"], "sess-a");
    }
}

#[test]
fn search_filters_by_obs_type() {
    let server = make_server();
    let result = server
        .do_search(SearchParams {
            query: "auth".into(),
            project: None,
            obs_type: Some("file_edit".into()),
            limit: None,
            offset: None,
            order_by: None,
            before: None,
            after: None,
        })
        .unwrap();

    let arr = result_json(&result);
    let items = arr.as_array().unwrap();
    assert!(!items.is_empty());
    for item in items {
        assert_eq!(item["obs_type"], "file_edit");
    }
}

#[test]
fn search_returns_empty_for_no_match() {
    let server = make_server();
    let result = server
        .do_search(SearchParams {
            query: "nonexistent_xyz_query".into(),
            project: None,
            obs_type: None,
            limit: None,
            offset: None,
            order_by: None,
            before: None,
            after: None,
        })
        .unwrap();

    assert!(!result.is_error.unwrap_or(false));
    assert_eq!(result_json(&result).as_array().unwrap().len(), 0);
}

#[test]
fn search_respects_limit() {
    let server = make_server();
    let result = server
        .do_search(SearchParams {
            query: "auth OR cargo OR Read OR Edit".into(),
            project: None,
            obs_type: None,
            limit: Some(2),
            offset: None,
            order_by: None,
            before: None,
            after: None,
        })
        .unwrap();

    assert!(result_json(&result).as_array().unwrap().len() <= 2);
}

// --- get_observations tests ---

#[test]
fn get_observations_returns_full_objects() {
    let server = make_server();
    let result = server
        .do_get_observations(GetObservationsParams { ids: vec![1, 3] })
        .unwrap();

    assert!(!result.is_error.unwrap_or(false));
    let arr = result_json(&result);
    let items = arr.as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["id"], 1);
    assert_eq!(items[1]["id"], 3);
    assert!(items[0]["content"].as_str().unwrap().contains("auth.rs"));
    assert!(items[1]["content"].as_str().unwrap().contains("cargo test"));
}

#[test]
fn get_observations_empty_ids_error() {
    let server = make_server();
    let result = server
        .do_get_observations(GetObservationsParams { ids: vec![] })
        .unwrap();

    assert!(result.is_error.unwrap_or(false));
    assert!(result_text(&result).contains("must not be empty"));
}

#[test]
fn get_observations_missing_ids_returns_partial() {
    let server = make_server();
    let result = server
        .do_get_observations(GetObservationsParams {
            ids: vec![1, 9999],
        })
        .unwrap();

    assert!(!result.is_error.unwrap_or(false));
    let arr = result_json(&result);
    let items = arr.as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], 1);
}

#[test]
fn get_observations_with_metadata() {
    let server = make_server();
    let result = server
        .do_get_observations(GetObservationsParams { ids: vec![2] })
        .unwrap();

    let arr = result_json(&result);
    let obs = &arr.as_array().unwrap()[0];
    assert!(obs["metadata"].is_object());
    assert_eq!(obs["metadata"]["redacted"], false);
}

// --- timeline tests ---

#[test]
fn timeline_returns_surrounding_context() {
    let server = make_server();
    let result = server
        .do_timeline(TimelineParams {
            anchor: 3,
            before: Some(2),
            after: Some(2),
        })
        .unwrap();

    assert!(!result.is_error.unwrap_or(false));
    let json = result_json(&result);

    assert_eq!(json["anchor"]["id"], 3);

    let before = json["before"].as_array().unwrap();
    assert_eq!(before.len(), 2);
    assert!(before[0]["id"].as_i64().unwrap() < before[1]["id"].as_i64().unwrap());

    let after = json["after"].as_array().unwrap();
    assert!(!after.is_empty());
    assert!(after[0]["id"].as_i64().unwrap() > 3);
}

#[test]
fn timeline_missing_anchor_error() {
    let server = make_server();
    let result = server.do_timeline(TimelineParams {
        anchor: 9999,
        before: None,
        after: None,
    });

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.message.contains("anchor observation not found"));
}

#[test]
fn timeline_at_session_boundary() {
    let server = make_server();
    let result = server
        .do_timeline(TimelineParams {
            anchor: 1,
            before: Some(5),
            after: Some(5),
        })
        .unwrap();

    let json = result_json(&result);
    assert_eq!(json["anchor"]["id"], 1);
    assert_eq!(json["before"].as_array().unwrap().len(), 0);
    assert!(!json["after"].as_array().unwrap().is_empty());
}

// --- recent_context tests ---

#[test]
fn recent_context_returns_deduped_by_file_path() {
    let server = make_server();
    let result = server
        .do_recent_context(RecentContextParams {
            project: Some("myproj".into()),
            limit: Some(100),
            before: None,
            after: None,
        })
        .unwrap();

    assert!(!result.is_error.unwrap_or(false));
    let arr = result_json(&result);
    let items = arr.as_array().unwrap();

    // /src/auth.rs appears in obs 1, 2, 6 — dedup keeps highest-scored file_edit
    let auth_entries: Vec<_> = items
        .iter()
        .filter(|o| o["file_path"].as_str() == Some("/src/auth.rs"))
        .collect();
    assert_eq!(auth_entries.len(), 1);
    assert_eq!(auth_entries[0]["obs_type"], "file_edit");

    // NULL file_path observations are NOT deduped
    let null_fp: Vec<_> = items.iter().filter(|o| o["file_path"].is_null()).collect();
    assert!(null_fp.len() >= 2);
}

#[test]
fn recent_context_filters_by_project() {
    let server = make_server();
    let result = server
        .do_recent_context(RecentContextParams {
            project: Some("other".into()),
            limit: None,
            before: None,
            after: None,
        })
        .unwrap();

    let arr = result_json(&result);
    let items = arr.as_array().unwrap();
    // With composite scoring, project is a boost signal not a filter —
    // same-project observations rank higher but cross-project still appear
    assert!(!items.is_empty());
    // The "other" project observation (sess-b) should be present
    let has_other = items.iter().any(|o| o["session_id"] == "sess-b");
    assert!(has_other, "expected sess-b observation to be present");
}

#[test]
fn recent_context_all_projects() {
    let server = make_server();
    let result = server
        .do_recent_context(RecentContextParams {
            project: None,
            limit: None,
            before: None,
            after: None,
        })
        .unwrap();

    let arr = result_json(&result);
    let sessions: std::collections::HashSet<&str> = arr
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["session_id"].as_str().unwrap())
        .collect();
    assert!(sessions.contains("sess-a"));
    assert!(sessions.contains("sess-b"));
}

#[test]
fn recent_context_empty_project() {
    let server = make_server();
    let result = server
        .do_recent_context(RecentContextParams {
            project: Some("nonexistent".into()),
            limit: None,
            before: None,
            after: None,
        })
        .unwrap();

    // With composite scoring, project is a boost signal not a filter —
    // observations still appear but with lower project weight (0.3)
    let items = result_json(&result);
    let arr = items.as_array().unwrap();
    assert!(!arr.is_empty());
    // All observations should have score > 0
    for item in arr {
        assert!(item["score"].as_f64().unwrap() > 0.0);
    }
}

// --- pin tests ---

#[test]
fn search_includes_is_pinned() {
    let server = make_server();

    // Pin observation 2 directly via SQL
    {
        let db = server.db_handle().lock().unwrap();
        db.execute("UPDATE observations SET is_pinned = 1 WHERE id = 2", [])
            .unwrap();
    }

    let result = server
        .do_search(SearchParams {
            query: "auth".into(),
            project: None,
            obs_type: None,
            limit: None,
            offset: None,
            order_by: None,
            before: None,
            after: None,
        })
        .unwrap();

    let arr = result_json(&result);
    let items = arr.as_array().unwrap();
    // Find the pinned one (id=2)
    let pinned_item = items.iter().find(|o| o["id"] == 2);
    assert!(pinned_item.is_some(), "should find observation 2");
    assert_eq!(pinned_item.unwrap()["is_pinned"], true);

    // Find an unpinned one
    let unpinned_item = items.iter().find(|o| o["id"] != 2);
    if let Some(item) = unpinned_item {
        assert_eq!(item["is_pinned"], false);
    }
}

#[test]
fn get_observations_includes_is_pinned() {
    let server = make_server();

    // Pin observation 1 directly via SQL
    {
        let db = server.db_handle().lock().unwrap();
        db.execute("UPDATE observations SET is_pinned = 1 WHERE id = 1", [])
            .unwrap();
    }

    let result = server
        .do_get_observations(GetObservationsParams { ids: vec![1, 3] })
        .unwrap();

    let arr = result_json(&result);
    let items = arr.as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["id"], 1);
    assert_eq!(items[0]["is_pinned"], true);
    assert_eq!(items[1]["id"], 3);
    assert_eq!(items[1]["is_pinned"], false);
}

// --- scored context tests ---

/// Create a test DB with timestamps relative to `now` for scoring tests.
fn scored_test_db(now: i64) -> NmemServer {
    let mut conn = Connection::open_in_memory().unwrap();
    nmem::schema_migrations().to_latest(&mut conn).unwrap();
    register_udfs(&conn).unwrap();

    conn.execute_batch(&format!(
        "
        INSERT INTO sessions (id, project, started_at) VALUES ('s1', 'proj-a', {now});
        INSERT INTO sessions (id, project, started_at) VALUES ('s2', 'proj-b', {now});
        "
    ))
    .unwrap();

    NmemServer::new(Arc::new(Mutex::new(conn)))
}

fn insert_obs(
    server: &NmemServer,
    id: i64,
    session_id: &str,
    timestamp: i64,
    obs_type: &str,
    file_path: Option<&str>,
    content: &str,
) {
    let db = server.db_handle();
    let db = db.lock().unwrap();
    let fp = file_path
        .map(|s| format!("'{s}'"))
        .unwrap_or("NULL".into());
    db.execute_batch(&format!(
        "INSERT INTO observations (id, session_id, prompt_id, timestamp, obs_type, source_event, tool_name, file_path, content, metadata)
         VALUES ({id}, '{session_id}', NULL, {timestamp}, '{obs_type}', 'PostToolUse', NULL, {fp}, '{content}', NULL);"
    ))
    .unwrap();
}

#[test]
fn scored_context_type_weight_ordering() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let server = scored_test_db(now);

    // Same timestamp, different types, different file_paths
    insert_obs(&server, 1, "s1", now, "file_read", Some("/a.rs"), "read a");
    insert_obs(&server, 2, "s1", now, "file_edit", Some("/b.rs"), "edit b");
    insert_obs(&server, 3, "s1", now, "command", Some("/c.rs"), "cmd c");

    let result = server
        .do_recent_context(RecentContextParams {
            project: None,
            limit: Some(10),
            before: None,
            after: None,
        })
        .unwrap();

    let arr = result_json(&result);
    let items = arr.as_array().unwrap();
    // file_edit should rank first, then command, then file_read
    assert_eq!(items[0]["obs_type"], "file_edit");
    assert_eq!(items[1]["obs_type"], "command");
    assert_eq!(items[2]["obs_type"], "file_read");
}

#[test]
fn scored_context_recency_beats_type() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let server = scored_test_db(now);

    let fourteen_days_ago = now - 14 * 86400;
    // Old file_edit (14d ago): recency=0.25, type_w=1.0 → score=0.25*0.6+1.0*0.4=0.55
    insert_obs(
        &server,
        1,
        "s1",
        fourteen_days_ago,
        "file_edit",
        Some("/old.rs"),
        "old edit",
    );
    // Fresh file_read (now): recency≈1.0, type_w=0.17 → score=1.0*0.6+0.17*0.4=0.668
    insert_obs(
        &server,
        2,
        "s1",
        now,
        "file_read",
        Some("/new.rs"),
        "fresh read",
    );

    let result = server
        .do_recent_context(RecentContextParams {
            project: None,
            limit: Some(10),
            before: None,
            after: None,
        })
        .unwrap();

    let arr = result_json(&result);
    let items = arr.as_array().unwrap();
    // Fresh file_read should beat old file_edit
    assert_eq!(items[0]["id"], 2);
    assert_eq!(items[1]["id"], 1);
}

#[test]
fn scored_context_project_boost() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let server = scored_test_db(now);

    // Same type, same timestamp, different projects
    insert_obs(
        &server,
        1,
        "s1",
        now,
        "file_edit",
        Some("/a.rs"),
        "edit in proj-a",
    );
    insert_obs(
        &server,
        2,
        "s2",
        now,
        "file_edit",
        Some("/b.rs"),
        "edit in proj-b",
    );

    let result = server
        .do_recent_context(RecentContextParams {
            project: Some("proj-a".into()),
            limit: Some(10),
            before: None,
            after: None,
        })
        .unwrap();

    let arr = result_json(&result);
    let items = arr.as_array().unwrap();
    assert_eq!(items.len(), 2);
    // proj-a observation should rank higher due to project boost
    assert_eq!(items[0]["id"], 1);
    assert_eq!(items[1]["id"], 2);
}

#[test]
fn scored_context_dedup_keeps_highest() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let server = scored_test_db(now);

    // Same file_path, same timestamp, different types
    insert_obs(
        &server,
        1,
        "s1",
        now,
        "file_read",
        Some("/same.rs"),
        "read same",
    );
    insert_obs(
        &server,
        2,
        "s1",
        now,
        "file_edit",
        Some("/same.rs"),
        "edit same",
    );

    let result = server
        .do_recent_context(RecentContextParams {
            project: None,
            limit: Some(10),
            before: None,
            after: None,
        })
        .unwrap();

    let arr = result_json(&result);
    let items = arr.as_array().unwrap();
    // Dedup should keep file_edit (higher score) over file_read
    let same_entries: Vec<_> = items
        .iter()
        .filter(|o| o["file_path"].as_str() == Some("/same.rs"))
        .collect();
    assert_eq!(same_entries.len(), 1);
    assert_eq!(same_entries[0]["obs_type"], "file_edit");
}

#[test]
fn scored_context_has_score_field() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let server = scored_test_db(now);

    insert_obs(&server, 1, "s1", now, "file_edit", Some("/x.rs"), "edit x");

    let result = server
        .do_recent_context(RecentContextParams {
            project: None,
            limit: Some(10),
            before: None,
            after: None,
        })
        .unwrap();

    let arr = result_json(&result);
    let items = arr.as_array().unwrap();
    assert_eq!(items.len(), 1);
    let score = items[0]["score"].as_f64().unwrap();
    assert!(score > 0.0, "score should be > 0, got {score}");
}

// --- temporal scoping tests ---

#[test]
fn search_with_before_filter() {
    let server = make_server();
    // All test data timestamps are around 1707400000-1707400110.
    // Setting before=1707400035 should exclude observations at 1707400040, 1707400050, 1707400060, 1707400110.
    let result = server
        .do_search(SearchParams {
            query: "auth OR cargo OR Read OR Edit OR main".into(),
            project: None,
            obs_type: None,
            limit: Some(100),
            offset: None,
            order_by: None,
            before: Some(1707400035),
            after: None,
        })
        .unwrap();

    let arr = result_json(&result);
    let items = arr.as_array().unwrap();
    for item in items {
        let ts = item["timestamp"].as_i64().unwrap();
        assert!(
            ts < 1707400035,
            "expected timestamp < 1707400035, got {ts}"
        );
    }
    // Should include obs 1 (1707400020) and 2 (1707400030)
    assert!(items.len() >= 2, "expected at least 2 results, got {}", items.len());
}

#[test]
fn search_with_after_filter() {
    let server = make_server();
    // Setting after=1707400045 should only include obs at 1707400050, 1707400060, 1707400110.
    let result = server
        .do_search(SearchParams {
            query: "auth OR cargo OR Read OR Edit OR main".into(),
            project: None,
            obs_type: None,
            limit: Some(100),
            offset: None,
            order_by: None,
            before: None,
            after: Some(1707400045),
        })
        .unwrap();

    let arr = result_json(&result);
    let items = arr.as_array().unwrap();
    for item in items {
        let ts = item["timestamp"].as_i64().unwrap();
        assert!(
            ts > 1707400045,
            "expected timestamp > 1707400045, got {ts}"
        );
    }
    assert!(items.len() >= 2, "expected at least 2 results, got {}", items.len());
}

#[test]
fn search_with_before_and_after() {
    let server = make_server();
    // Window: after=1707400025, before=1707400055 → obs at 1707400030, 1707400040, 1707400050
    let result = server
        .do_search(SearchParams {
            query: "auth OR cargo OR Read OR Edit OR main".into(),
            project: None,
            obs_type: None,
            limit: Some(100),
            offset: None,
            order_by: None,
            before: Some(1707400055),
            after: Some(1707400025),
        })
        .unwrap();

    let arr = result_json(&result);
    let items = arr.as_array().unwrap();
    for item in items {
        let ts = item["timestamp"].as_i64().unwrap();
        assert!(ts > 1707400025 && ts < 1707400055, "timestamp {ts} outside window");
    }
    assert!(items.len() >= 2);
}

#[test]
fn recent_context_with_before_filter() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let server = scored_test_db(now);

    let t1 = now - 3600;
    let t2 = now - 1800;
    let t3 = now;
    insert_obs(&server, 1, "s1", t1, "file_edit", Some("/a.rs"), "edit a");
    insert_obs(&server, 2, "s1", t2, "file_edit", Some("/b.rs"), "edit b");
    insert_obs(&server, 3, "s1", t3, "file_edit", Some("/c.rs"), "edit c");

    // before=t2+1 should exclude obs 3
    let result = server
        .do_recent_context(RecentContextParams {
            project: None,
            limit: Some(10),
            before: Some(t2 + 1),
            after: None,
        })
        .unwrap();

    let arr = result_json(&result);
    let items = arr.as_array().unwrap();
    for item in items {
        let ts = item["timestamp"].as_i64().unwrap();
        assert!(ts < t2 + 1, "expected timestamp < {}, got {ts}", t2 + 1);
    }
    assert_eq!(items.len(), 2);
}

#[test]
fn recent_context_with_after_filter() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let server = scored_test_db(now);

    let t1 = now - 3600;
    let t2 = now - 1800;
    let t3 = now;
    insert_obs(&server, 1, "s1", t1, "file_edit", Some("/a.rs"), "edit a");
    insert_obs(&server, 2, "s1", t2, "file_edit", Some("/b.rs"), "edit b");
    insert_obs(&server, 3, "s1", t3, "file_edit", Some("/c.rs"), "edit c");

    // after=t1+1 should exclude obs 1
    let result = server
        .do_recent_context(RecentContextParams {
            project: None,
            limit: Some(10),
            before: None,
            after: Some(t1 + 1),
        })
        .unwrap();

    let arr = result_json(&result);
    let items = arr.as_array().unwrap();
    for item in items {
        let ts = item["timestamp"].as_i64().unwrap();
        assert!(ts > t1 + 1, "expected timestamp > {}, got {ts}", t1 + 1);
    }
    assert_eq!(items.len(), 2);
}

#[test]
fn session_summaries_with_before_filter() {
    let mut conn = Connection::open_in_memory().unwrap();
    nmem::schema_migrations().to_latest(&mut conn).unwrap();
    register_udfs(&conn).unwrap();

    conn.execute_batch(
        "
        INSERT INTO sessions (id, project, started_at, summary)
            VALUES ('s1', 'proj', 1000, '{\"intent\":\"first\",\"completed\":[],\"learned\":[],\"next_steps\":[],\"files_edited\":[],\"notes\":[]}');
        INSERT INTO sessions (id, project, started_at, summary)
            VALUES ('s2', 'proj', 2000, '{\"intent\":\"second\",\"completed\":[],\"learned\":[],\"next_steps\":[],\"files_edited\":[],\"notes\":[]}');
        INSERT INTO sessions (id, project, started_at, summary)
            VALUES ('s3', 'proj', 3000, '{\"intent\":\"third\",\"completed\":[],\"learned\":[],\"next_steps\":[],\"files_edited\":[],\"notes\":[]}');
        ",
    )
    .unwrap();

    let server = NmemServer::new(Arc::new(Mutex::new(conn)));

    // before=2500 should exclude s3 (started_at=3000)
    let result = server
        .do_session_summaries(SessionSummariesParams {
            project: None,
            limit: None,
            before: Some(2500),
            after: None,
        })
        .unwrap();

    let arr = result_json(&result);
    let items = arr.as_array().unwrap();
    assert_eq!(items.len(), 2);
    for item in items {
        let started = item["started_at"].as_i64().unwrap();
        assert!(started < 2500, "expected started_at < 2500, got {started}");
    }
}

#[test]
fn session_summaries_with_after_filter() {
    let mut conn = Connection::open_in_memory().unwrap();
    nmem::schema_migrations().to_latest(&mut conn).unwrap();
    register_udfs(&conn).unwrap();

    conn.execute_batch(
        "
        INSERT INTO sessions (id, project, started_at, summary)
            VALUES ('s1', 'proj', 1000, '{\"intent\":\"first\",\"completed\":[],\"learned\":[],\"next_steps\":[],\"files_edited\":[],\"notes\":[]}');
        INSERT INTO sessions (id, project, started_at, summary)
            VALUES ('s2', 'proj', 2000, '{\"intent\":\"second\",\"completed\":[],\"learned\":[],\"next_steps\":[],\"files_edited\":[],\"notes\":[]}');
        INSERT INTO sessions (id, project, started_at, summary)
            VALUES ('s3', 'proj', 3000, '{\"intent\":\"third\",\"completed\":[],\"learned\":[],\"next_steps\":[],\"files_edited\":[],\"notes\":[]}');
        ",
    )
    .unwrap();

    let server = NmemServer::new(Arc::new(Mutex::new(conn)));

    // after=1500 should exclude s1 (started_at=1000)
    let result = server
        .do_session_summaries(SessionSummariesParams {
            project: None,
            limit: None,
            before: None,
            after: Some(1500),
        })
        .unwrap();

    let arr = result_json(&result);
    let items = arr.as_array().unwrap();
    assert_eq!(items.len(), 2);
    for item in items {
        let started = item["started_at"].as_i64().unwrap();
        assert!(started > 1500, "expected started_at > 1500, got {started}");
    }
}

// --- session_trace tests ---

#[test]
fn session_trace_returns_prompts_with_observations() {
    let server = make_server();
    let result = server
        .do_session_trace(SessionTraceParams {
            session_id: "sess-a".into(),
            before: None,
            after: None,
        })
        .unwrap();

    assert!(!result.is_error.unwrap_or(false));
    let json = result_json(&result);

    assert_eq!(json["session_id"], "sess-a");
    assert_eq!(json["project"], "myproj");
    assert_eq!(json["started_at"], 1707400000);

    let prompts = json["prompts"].as_array().unwrap();
    // sess-a has 1 user prompt with observations
    assert!(!prompts.is_empty());

    // Find the user prompt (prompt_id=1)
    let user_prompt = prompts
        .iter()
        .find(|p| p["prompt_id"] == 1)
        .expect("should find prompt_id=1");
    assert_eq!(user_prompt["source"], "user");
    assert_eq!(
        user_prompt["content"].as_str().unwrap(),
        "Fix the login bug"
    );

    let obs = user_prompt["observations"].as_array().unwrap();
    // prompt 1 has observations 1, 2, 3, 4, 6
    assert_eq!(obs.len(), 5);
    assert_eq!(
        user_prompt["observation_count"].as_i64().unwrap(),
        5
    );

    // Observations should be ordered by timestamp
    let timestamps: Vec<i64> = obs.iter().map(|o| o["timestamp"].as_i64().unwrap()).collect();
    let mut sorted = timestamps.clone();
    sorted.sort();
    assert_eq!(timestamps, sorted);
}

#[test]
fn session_trace_unknown_session_error() {
    let server = make_server();
    let result = server.do_session_trace(SessionTraceParams {
        session_id: "nonexistent".into(),
        before: None,
        after: None,
    });

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.message.contains("session not found"));
}

#[test]
fn session_trace_with_temporal_filter() {
    let server = make_server();
    // before=1707400035 should exclude observations at 1707400040+
    let result = server
        .do_session_trace(SessionTraceParams {
            session_id: "sess-a".into(),
            before: Some(1707400035),
            after: None,
        })
        .unwrap();

    let json = result_json(&result);
    let prompts = json["prompts"].as_array().unwrap();
    // The user prompt at 1707400010 is before the cutoff, so it should appear
    assert!(!prompts.is_empty());

    // But only observations before 1707400035 should be included
    for prompt in prompts {
        for obs in prompt["observations"].as_array().unwrap() {
            let ts = obs["timestamp"].as_i64().unwrap();
            assert!(ts < 1707400035, "expected obs timestamp < 1707400035, got {ts}");
        }
    }
}

#[test]
fn session_trace_includes_orphan_observations() {
    let server = make_server();
    // sess-b has observation 5 with prompt_id=NULL
    let result = server
        .do_session_trace(SessionTraceParams {
            session_id: "sess-b".into(),
            before: None,
            after: None,
        })
        .unwrap();

    let json = result_json(&result);
    let prompts = json["prompts"].as_array().unwrap();
    // Should have a "system" prompt entry for the orphan observation
    let system_prompt = prompts
        .iter()
        .find(|p| p["source"] == "system")
        .expect("should find system prompt for orphan obs");
    assert!(system_prompt["prompt_id"].is_null());
    let obs = system_prompt["observations"].as_array().unwrap();
    assert_eq!(obs.len(), 1);
    assert_eq!(obs[0]["id"], 5);
}

#[test]
fn session_trace_includes_summary() {
    // Create a session with a summary
    let mut conn = Connection::open_in_memory().unwrap();
    nmem::schema_migrations().to_latest(&mut conn).unwrap();
    register_udfs(&conn).unwrap();

    conn.execute_batch(
        "INSERT INTO sessions (id, project, started_at, summary)
         VALUES ('s1', 'proj', 1000, '{\"intent\":\"test session\",\"completed\":[\"did stuff\"],\"learned\":[],\"next_steps\":[],\"files_edited\":[],\"notes\":[]}');",
    )
    .unwrap();

    let server = NmemServer::new(Arc::new(Mutex::new(conn)));
    let result = server
        .do_session_trace(SessionTraceParams {
            session_id: "s1".into(),
            before: None,
            after: None,
        })
        .unwrap();

    let json = result_json(&result);
    assert!(json["summary"].is_object());
    assert_eq!(json["summary"]["intent"], "test session");
}

// --- file_history tests ---

#[test]
fn file_history_groups_by_session() {
    let server = make_server();
    // /src/auth.rs appears in sess-a with observations 1, 2, 6
    let result = server
        .do_file_history(FileHistoryParams {
            file_path: "/src/auth.rs".into(),
            before: None,
            after: None,
            limit: None,
        })
        .unwrap();

    assert!(!result.is_error.unwrap_or(false));
    let json = result_json(&result);

    assert_eq!(json["file_path"], "/src/auth.rs");
    let sessions = json["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["session_id"], "sess-a");
    assert_eq!(sessions[0]["project"], "myproj");

    let touches = sessions[0]["touches"].as_array().unwrap();
    assert_eq!(touches.len(), 3); // obs 1, 2, 6
}

#[test]
fn file_history_includes_prompt_content() {
    let server = make_server();
    let result = server
        .do_file_history(FileHistoryParams {
            file_path: "/src/auth.rs".into(),
            before: None,
            after: None,
            limit: None,
        })
        .unwrap();

    let json = result_json(&result);
    let sessions = json["sessions"].as_array().unwrap();
    let touches = sessions[0]["touches"].as_array().unwrap();
    // The observations for auth.rs are under prompt "Fix the login bug"
    let has_prompt = touches
        .iter()
        .any(|t| t["prompt_content"].as_str() == Some("Fix the login bug"));
    assert!(has_prompt, "should include user prompt content");
}

#[test]
fn file_history_unknown_file_empty() {
    let server = make_server();
    let result = server
        .do_file_history(FileHistoryParams {
            file_path: "/nonexistent/file.rs".into(),
            before: None,
            after: None,
            limit: None,
        })
        .unwrap();

    assert!(!result.is_error.unwrap_or(false));
    let json = result_json(&result);
    let sessions = json["sessions"].as_array().unwrap();
    assert!(sessions.is_empty());
}

#[test]
fn file_history_with_temporal_filter() {
    let server = make_server();
    // Only include touches before 1707400035 — should get obs 1 (1707400020) and 2 (1707400030) but not 6 (1707400060)
    let result = server
        .do_file_history(FileHistoryParams {
            file_path: "/src/auth.rs".into(),
            before: Some(1707400035),
            after: None,
            limit: None,
        })
        .unwrap();

    let json = result_json(&result);
    let sessions = json["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    let touches = sessions[0]["touches"].as_array().unwrap();
    assert_eq!(touches.len(), 2);
    for t in touches {
        let ts = t["timestamp"].as_i64().unwrap();
        assert!(ts < 1707400035, "expected timestamp < 1707400035, got {ts}");
    }
}

#[test]
fn file_history_respects_limit() {
    let server = make_server();
    // Limit to 1 observation total
    let result = server
        .do_file_history(FileHistoryParams {
            file_path: "/src/auth.rs".into(),
            before: None,
            after: None,
            limit: Some(1),
        })
        .unwrap();

    let json = result_json(&result);
    let sessions = json["sessions"].as_array().unwrap();
    // With limit=1, only 1 observation returned → 1 session with 1 touch
    let total_touches: usize = sessions
        .iter()
        .map(|s| s["touches"].as_array().unwrap().len())
        .sum();
    assert_eq!(total_touches, 1);
}

#[test]
fn file_history_includes_summary_intent() {
    // Create a session with a summary and file observations
    let mut conn = Connection::open_in_memory().unwrap();
    nmem::schema_migrations().to_latest(&mut conn).unwrap();
    register_udfs(&conn).unwrap();

    conn.execute_batch(
        "INSERT INTO sessions (id, project, started_at, summary)
         VALUES ('s1', 'proj', 1000, '{\"intent\":\"refactor auth module\",\"completed\":[],\"learned\":[],\"next_steps\":[],\"files_edited\":[],\"notes\":[]}');
         INSERT INTO observations (id, session_id, prompt_id, timestamp, obs_type, source_event, tool_name, file_path, content, metadata)
         VALUES (1, 's1', NULL, 1010, 'file_edit', 'PostToolUse', 'Edit', '/src/auth.rs', 'edited auth', NULL);",
    )
    .unwrap();

    let server = NmemServer::new(Arc::new(Mutex::new(conn)));
    let result = server
        .do_file_history(FileHistoryParams {
            file_path: "/src/auth.rs".into(),
            before: None,
            after: None,
            limit: None,
        })
        .unwrap();

    let json = result_json(&result);
    let sessions = json["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(
        sessions[0]["summary_intent"].as_str().unwrap(),
        "refactor auth module"
    );
}
