use nmem::db::register_udfs;
use nmem::serve::{
    GetObservationsParams, NmemServer, RecentContextParams, SearchParams,
    TimelineParams,
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
        })
        .unwrap();

    let arr = result_json(&result);
    let items = arr.as_array().unwrap();
    assert_eq!(items.len(), 1);
    let score = items[0]["score"].as_f64().unwrap();
    assert!(score > 0.0, "score should be > 0, got {score}");
}
