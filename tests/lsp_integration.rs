use serde_json::json;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

fn lsp_message(content: &str) -> Vec<u8> {
    format!("Content-Length: {}\r\n\r\n{}", content.len(), content).into_bytes()
}

fn read_lsp_response(reader: &mut BufReader<std::process::ChildStdout>) -> String {
    // Read headers until blank line
    let mut content_length: usize = 0;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let trimmed = line.trim();
        if trimmed.is_empty() {
            break;
        }
        if let Some(len_str) = trimmed.strip_prefix("Content-Length: ") {
            content_length = len_str.parse().unwrap();
        }
    }
    assert!(content_length > 0, "no Content-Length header");

    let mut body = vec![0u8; content_length];
    std::io::Read::read_exact(reader, &mut body).unwrap();
    String::from_utf8(body).unwrap()
}

fn nmem_bin() -> std::path::PathBuf {
    // Use cargo-built binary
    let mut path = std::env::current_exe().unwrap();
    path.pop(); // remove test binary name
    path.pop(); // remove deps/
    path.push("nmem");
    path
}

#[test]
fn lsp_initialize_returns_capabilities() {
    let bin = nmem_bin();
    if !bin.exists() {
        panic!("nmem binary not found at {}", bin.display());
    }

    let mut child = Command::new(&bin)
        .arg("lsp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let stdin = child.stdin.as_mut().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);

    // Send initialize
    let init_req = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}"#;
    stdin.write_all(&lsp_message(init_req)).unwrap();
    stdin.flush().unwrap();

    let response = read_lsp_response(&mut reader);
    let json: serde_json::Value = serde_json::from_str(&response).unwrap();

    assert_eq!(json["id"], 1);
    assert!(json["result"]["capabilities"].is_object());
    assert_eq!(json["result"]["serverInfo"]["name"], "nmem");
    assert_eq!(json["result"]["capabilities"]["textDocumentSync"], 0);

    // Send initialized notification (no response expected)
    let initialized = r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#;
    stdin.write_all(&lsp_message(initialized)).unwrap();
    stdin.flush().unwrap();

    // Send shutdown request
    let shutdown = r#"{"jsonrpc":"2.0","id":2,"method":"shutdown","params":null}"#;
    stdin.write_all(&lsp_message(shutdown)).unwrap();
    stdin.flush().unwrap();

    // Read responses until we get the shutdown response (skip notifications like logMessage)
    let json = loop {
        let resp = read_lsp_response(&mut reader);
        let j: serde_json::Value = serde_json::from_str(&resp).unwrap();
        if j.get("id").is_some_and(|id| !id.is_null()) {
            break j;
        }
    };
    assert_eq!(json["id"], 2);
    assert!(json["result"].is_null());

    // Send exit notification
    let exit = r#"{"jsonrpc":"2.0","method":"exit","params":null}"#;
    stdin.write_all(&lsp_message(exit)).unwrap();
    stdin.flush().unwrap();

    let status = child.wait().unwrap();
    assert!(status.success(), "nmem lsp exited with: {status}");
}

#[test]
fn lsp_did_open_emits_diagnostic_for_git_file() {
    // Create a temp git repo with a file
    let dir = tempfile::TempDir::new().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();
    let sig = git2::Signature::new("Test", "t@t.com", &git2::Time::new(1000000, 0)).unwrap();
    std::fs::write(dir.path().join("main.rs"), "fn main() {}").unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(std::path::Path::new("main.rs")).unwrap();
    index.write().unwrap();
    let tree_oid = index.write_tree().unwrap();
    let tree = repo.find_tree(tree_oid).unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[]).unwrap();

    let bin = nmem_bin();
    let mut child = Command::new(&bin)
        .arg("lsp")
        .current_dir(dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let stdin = child.stdin.as_mut().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);

    // Initialize
    let init_req = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}"#;
    stdin.write_all(&lsp_message(init_req)).unwrap();
    stdin.flush().unwrap();
    let _ = read_lsp_response(&mut reader); // consume initialize response

    // Send initialized
    let initialized = r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#;
    stdin.write_all(&lsp_message(initialized)).unwrap();
    stdin.flush().unwrap();

    // There may be a window/logMessage notification from initialized handler
    // We need to handle both notifications and the diagnostic

    // Send didOpen
    let file_uri = format!("file://{}/main.rs", dir.path().display());
    let did_open = format!(
        r#"{{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{{"textDocument":{{"uri":"{}","languageId":"rust","version":1,"text":"fn main() {{}}"}}}}}}"#,
        file_uri
    );
    stdin.write_all(&lsp_message(&did_open)).unwrap();
    stdin.flush().unwrap();

    // Read responses until we get publishDiagnostics or timeout
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut found_diagnostic = false;

    while std::time::Instant::now() < deadline {
        // Set a read timeout by using a non-blocking check
        // Since BufReader doesn't support timeouts directly, we'll rely on
        // the server producing output promptly
        let response = read_lsp_response(&mut reader);
        let json: serde_json::Value = serde_json::from_str(&response).unwrap();

        if json["method"] == "textDocument/publishDiagnostics" {
            let params = &json["params"];
            assert_eq!(params["uri"], file_uri);
            let diagnostics = params["diagnostics"].as_array().unwrap();
            assert_eq!(diagnostics.len(), 1);
            assert_eq!(diagnostics[0]["severity"], 3); // Information
            assert_eq!(diagnostics[0]["source"], "nmem");
            let msg = diagnostics[0]["message"].as_str().unwrap();
            assert!(msg.contains("main.rs:"), "diagnostic should mention file: {msg}");
            assert!(msg.contains("1 commits"), "should show 1 commit: {msg}");
            found_diagnostic = true;
            break;
        }
        // Otherwise it's a log message or other notification — keep reading
    }

    assert!(found_diagnostic, "did not receive publishDiagnostics");

    // Shutdown
    let shutdown = r#"{"jsonrpc":"2.0","id":99,"method":"shutdown","params":null}"#;
    stdin.write_all(&lsp_message(shutdown)).unwrap();
    stdin.flush().unwrap();
    let _ = read_lsp_response(&mut reader);

    let exit = r#"{"jsonrpc":"2.0","method":"exit","params":null}"#;
    stdin.write_all(&lsp_message(exit)).unwrap();
    stdin.flush().unwrap();

    let status = child.wait().unwrap();
    assert!(status.success());
}

#[test]
fn lsp_dedup_skips_second_open() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();
    let sig = git2::Signature::new("Test", "t@t.com", &git2::Time::new(1000000, 0)).unwrap();
    std::fs::write(dir.path().join("f.rs"), "fn f() {}").unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(std::path::Path::new("f.rs")).unwrap();
    index.write().unwrap();
    let tree_oid = index.write_tree().unwrap();
    let tree = repo.find_tree(tree_oid).unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[]).unwrap();

    let bin = nmem_bin();
    let mut child = Command::new(&bin)
        .arg("lsp")
        .current_dir(dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let stdin = child.stdin.as_mut().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);

    // Initialize
    let init_req = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}"#;
    stdin.write_all(&lsp_message(init_req)).unwrap();
    stdin.flush().unwrap();
    let _ = read_lsp_response(&mut reader);

    let initialized = r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#;
    stdin.write_all(&lsp_message(initialized)).unwrap();
    stdin.flush().unwrap();

    let file_uri = format!("file://{}/f.rs", dir.path().display());
    let did_open = format!(
        r#"{{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{{"textDocument":{{"uri":"{}","languageId":"rust","version":1,"text":"fn f() {{}}"}}}}}}"#,
        file_uri
    );

    // First open — should produce diagnostic
    stdin.write_all(&lsp_message(&did_open)).unwrap();
    stdin.flush().unwrap();

    let mut diagnostic_count = 0;

    // Read until we get the first diagnostic
    loop {
        let response = read_lsp_response(&mut reader);
        let json: serde_json::Value = serde_json::from_str(&response).unwrap();
        if json["method"] == "textDocument/publishDiagnostics" {
            diagnostic_count += 1;
            break;
        }
    }

    // Second open — should NOT produce another diagnostic (dedup)
    let did_open2 = format!(
        r#"{{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{{"textDocument":{{"uri":"{}","languageId":"rust","version":2,"text":"fn f() {{}}"}}}}}}"#,
        file_uri
    );
    stdin.write_all(&lsp_message(&did_open2)).unwrap();
    stdin.flush().unwrap();

    // Now send shutdown — if a second diagnostic were queued, it would arrive before the
    // shutdown response
    let shutdown = r#"{"jsonrpc":"2.0","id":99,"method":"shutdown","params":null}"#;
    stdin.write_all(&lsp_message(shutdown)).unwrap();
    stdin.flush().unwrap();

    // Read remaining messages until shutdown response
    loop {
        let response = read_lsp_response(&mut reader);
        let json: serde_json::Value = serde_json::from_str(&response).unwrap();
        if json["method"] == "textDocument/publishDiagnostics" {
            diagnostic_count += 1;
        }
        if json["id"] == 99 {
            break;
        }
    }

    assert_eq!(diagnostic_count, 1, "dedup should prevent second diagnostic");

    let exit = r#"{"jsonrpc":"2.0","method":"exit","params":null}"#;
    stdin.write_all(&lsp_message(exit)).unwrap();
    stdin.flush().unwrap();
    child.wait().unwrap();
}

#[test]
fn lsp_hover_returns_blame_info() {
    // Create a temp repo with two commits by different authors on different lines
    let dir = tempfile::TempDir::new().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();

    // First commit: author A writes two lines
    let sig_a = git2::Signature::new("Alice", "alice@test.com", &git2::Time::new(1700000000, 0)).unwrap();
    std::fs::write(dir.path().join("code.rs"), "fn main() {}\nfn helper() {}\n").unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(std::path::Path::new("code.rs")).unwrap();
    index.write().unwrap();
    let tree_oid = index.write_tree().unwrap();
    let tree = repo.find_tree(tree_oid).unwrap();
    let commit1 = repo.commit(Some("HEAD"), &sig_a, &sig_a, "initial setup", &tree, &[]).unwrap();
    let commit1_obj = repo.find_commit(commit1).unwrap();

    // Second commit: author B modifies line 2 and adds line 3
    let sig_b = git2::Signature::new("Bob", "bob@test.com", &git2::Time::new(1700100000, 0)).unwrap();
    std::fs::write(dir.path().join("code.rs"), "fn main() {}\nfn updated() {}\nfn new_fn() {}\n").unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(std::path::Path::new("code.rs")).unwrap();
    index.write().unwrap();
    let tree_oid = index.write_tree().unwrap();
    let tree = repo.find_tree(tree_oid).unwrap();
    repo.commit(Some("HEAD"), &sig_b, &sig_b, "update helper", &tree, &[&commit1_obj]).unwrap();

    let bin = nmem_bin();
    let mut child = Command::new(&bin)
        .arg("lsp")
        .current_dir(dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let stdin = child.stdin.as_mut().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);

    // Initialize
    let init_req = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}"#;
    stdin.write_all(&lsp_message(init_req)).unwrap();
    stdin.flush().unwrap();

    // Read initialize response — verify hover capability
    let init_resp = read_lsp_response(&mut reader);
    let init_json: serde_json::Value = serde_json::from_str(&init_resp).unwrap();
    assert!(
        init_json["result"]["capabilities"]["hoverProvider"] == json!(true),
        "hoverProvider should be true: {:?}",
        init_json["result"]["capabilities"]
    );

    // Send initialized
    let initialized = r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#;
    stdin.write_all(&lsp_message(initialized)).unwrap();
    stdin.flush().unwrap();

    // Send didOpen
    let file_uri = format!("file://{}/code.rs", dir.path().display());
    let did_open = format!(
        r#"{{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{{"textDocument":{{"uri":"{}","languageId":"rust","version":1,"text":"fn main() {{}}\nfn updated() {{}}\nfn new_fn() {{}}\n"}}}}}}"#,
        file_uri
    );
    stdin.write_all(&lsp_message(&did_open)).unwrap();
    stdin.flush().unwrap();

    // Drain notifications (logMessage, diagnostics) before sending hover
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        let response = read_lsp_response(&mut reader);
        let j: serde_json::Value = serde_json::from_str(&response).unwrap();
        if j["method"] == "textDocument/publishDiagnostics" {
            break;
        }
    }

    // Hover on line 0 (should be Alice's "initial setup" commit)
    let hover_req = format!(
        r#"{{"jsonrpc":"2.0","id":10,"method":"textDocument/hover","params":{{"textDocument":{{"uri":"{}"}},"position":{{"line":0,"character":0}}}}}}"#,
        file_uri
    );
    stdin.write_all(&lsp_message(&hover_req)).unwrap();
    stdin.flush().unwrap();

    // Read hover response (skip any interleaved notifications)
    let hover_json = loop {
        let resp = read_lsp_response(&mut reader);
        let j: serde_json::Value = serde_json::from_str(&resp).unwrap();
        if j.get("id").is_some_and(|id| id == 10) {
            break j;
        }
    };

    let hover_content = hover_json["result"]["contents"]["value"].as_str()
        .expect("hover should return markdown content");
    assert!(hover_content.contains("Alice"), "hover should show author Alice: {hover_content}");
    assert!(hover_content.contains("initial setup"), "hover should show commit message: {hover_content}");

    // Hover on line 1 (should be Bob's "update helper" commit)
    let hover_req2 = format!(
        r#"{{"jsonrpc":"2.0","id":11,"method":"textDocument/hover","params":{{"textDocument":{{"uri":"{}"}},"position":{{"line":1,"character":0}}}}}}"#,
        file_uri
    );
    stdin.write_all(&lsp_message(&hover_req2)).unwrap();
    stdin.flush().unwrap();

    let hover_json2 = loop {
        let resp = read_lsp_response(&mut reader);
        let j: serde_json::Value = serde_json::from_str(&resp).unwrap();
        if j.get("id").is_some_and(|id| id == 11) {
            break j;
        }
    };

    let hover_content2 = hover_json2["result"]["contents"]["value"].as_str()
        .expect("hover should return markdown content for line 1");
    assert!(hover_content2.contains("Bob"), "hover should show author Bob: {hover_content2}");
    assert!(hover_content2.contains("update helper"), "hover should show commit message: {hover_content2}");

    // Shutdown
    let shutdown = r#"{"jsonrpc":"2.0","id":99,"method":"shutdown","params":null}"#;
    stdin.write_all(&lsp_message(shutdown)).unwrap();
    stdin.flush().unwrap();

    loop {
        let resp = read_lsp_response(&mut reader);
        let j: serde_json::Value = serde_json::from_str(&resp).unwrap();
        if j.get("id").is_some_and(|id| id == 99) {
            break;
        }
    }

    let exit = r#"{"jsonrpc":"2.0","method":"exit","params":null}"#;
    stdin.write_all(&lsp_message(exit)).unwrap();
    stdin.flush().unwrap();

    let status = child.wait().unwrap();
    assert!(status.success());
}
