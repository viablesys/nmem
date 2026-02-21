use serde_json::{Map, Value};

/// Classify a tool name into an observation type.
/// For Bash commands, pass the command string to sub-classify git operations.
pub fn classify_tool(name: &str) -> &'static str {
    match name {
        "Bash" => "command",
        "Read" => "file_read",
        "Write" => "file_write",
        "Edit" => "file_edit",
        "Grep" | "Glob" => "search",
        "Task" => "task_spawn",
        "WebFetch" => "web_fetch",
        "WebSearch" => "web_search",
        _ if name.contains("__") => "mcp_call",
        _ => "tool_other",
    }
}

/// Sub-classify a Bash command. Returns a more specific obs_type if the
/// command is a git commit, push, or gh CLI call, otherwise returns "command".
pub fn classify_bash(command: &str) -> &'static str {
    let cmd = command.trim();
    // Handle chained commands: `git add . && git commit -m "msg" && git push`
    // Classify by the strongest signal (push > commit > gh > command)
    if contains_git_cmd(cmd, "push") {
        "git_push"
    } else if contains_git_cmd(cmd, "commit") {
        "git_commit"
    } else if contains_cmd(cmd, "gh") {
        "github"
    } else {
        "command"
    }
}

fn contains_git_cmd(cmd: &str, subcmd: &str) -> bool {
    // Match: "git push", "git -C /path push", "git commit", etc.
    for segment in cmd.split("&&").chain(cmd.split(';')) {
        let segment = segment.trim();
        let words: Vec<&str> = segment.split_whitespace().collect();
        if let Some(pos) = words.iter().position(|&w| w == "git")
            && words[pos + 1..].contains(&subcmd)
        {
            return true;
        }
    }
    false
}

fn contains_cmd(cmd: &str, target: &str) -> bool {
    for segment in cmd.split("&&").chain(cmd.split(';')) {
        let segment = segment.trim();
        if segment.split_whitespace().next() == Some(target) {
            return true;
        }
    }
    false
}

/// Extract the primary content from a tool invocation.
pub fn extract_content(name: &str, tool_input: &Value) -> String {
    match name {
        "Bash" => tool_input
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .chars()
            .take(500)
            .collect(),
        "Read" | "Write" | "Edit" => tool_input
            .get("file_path")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .into(),
        "Grep" => {
            let pattern = tool_input
                .get("pattern")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let path = tool_input
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if path.is_empty() {
                pattern.into()
            } else {
                format!("{pattern} in {path}")
            }
        }
        "Glob" => tool_input
            .get("pattern")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .into(),
        "Task" => tool_input
            .get("description")
            .or_else(|| tool_input.get("prompt"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .chars()
            .take(200)
            .collect(),
        "WebFetch" => tool_input
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .into(),
        "WebSearch" => tool_input
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .into(),
        "AskUserQuestion" => tool_input
            .get("questions")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|q| q.get("question"))
            .and_then(|v| v.as_str())
            .unwrap_or(name)
            .into(),
        _ => name.into(),
    }
}

/// Extract a file path from tool input, if applicable.
pub fn extract_file_path(name: &str, tool_input: &Value) -> Option<String> {
    match name {
        "Read" | "Write" | "Edit" => tool_input
            .get("file_path")
            .and_then(|v| v.as_str())
            .map(Into::into),
        "Grep" | "Glob" => tool_input
            .get("path")
            .and_then(|v| v.as_str())
            .map(Into::into),
        _ => None,
    }
}

/// Extract structured metadata from git commit/push tool_response.
/// Returns a map with commit_hash, commit_message, branch, diffstat fields.
pub fn extract_git_metadata(obs_type: &str, tool_response: &str) -> Map<String, Value> {
    let mut meta = Map::new();
    match obs_type {
        "git_commit" => parse_git_commit_response(tool_response, &mut meta),
        "git_push" => parse_git_push_response(tool_response, &mut meta),
        _ => {}
    }
    meta
}

fn parse_git_commit_response(response: &str, meta: &mut Map<String, Value>) {
    for line in response.lines() {
        let line = line.trim();
        // [branch hash] Commit message
        if line.starts_with('[') {
            if let Some(bracket_end) = line.find(']') {
                let inside = &line[1..bracket_end];
                // "branch hash" or "branch (root-commit) hash"
                let parts: Vec<&str> = inside.split_whitespace().collect();
                if parts.len() >= 2 {
                    meta.insert("branch".into(), Value::String(parts[0].into()));
                    meta.insert(
                        "commit_hash".into(),
                        Value::String(parts.last().unwrap().to_string()),
                    );
                }
                let message = line[bracket_end + 1..].trim().to_string();
                if !message.is_empty() {
                    meta.insert("commit_message".into(), Value::String(message));
                }
            }
        }
        // N files changed, X insertions(+), Y deletions(-)
        if line.contains("changed") && (line.contains("insertion") || line.contains("deletion")) {
            let mut files = 0u32;
            let mut ins = 0u32;
            let mut del = 0u32;
            let words: Vec<&str> = line.split_whitespace().collect();
            for (i, w) in words.iter().enumerate() {
                if i > 0 {
                    if w.starts_with("file") {
                        files = words[i - 1].parse().unwrap_or(0);
                    } else if w.starts_with("insertion") {
                        ins = words[i - 1].parse().unwrap_or(0);
                    } else if w.starts_with("deletion") {
                        del = words[i - 1].parse().unwrap_or(0);
                    }
                }
            }
            meta.insert("files_changed".into(), Value::Number(files.into()));
            meta.insert("insertions".into(), Value::Number(ins.into()));
            meta.insert("deletions".into(), Value::Number(del.into()));
        }
        // create mode / delete mode — collect changed file names
        if line.starts_with("create mode") || line.starts_with("delete mode") {
            let file = line.rsplit_once(' ').map(|(_, f)| f).unwrap_or("");
            if !file.is_empty() {
                let arr = meta
                    .entry("new_files")
                    .or_insert_with(|| Value::Array(Vec::new()));
                if let Value::Array(v) = arr {
                    v.push(Value::String(file.into()));
                }
            }
        }
    }
}

fn parse_git_push_response(response: &str, meta: &mut Map<String, Value>) {
    for line in response.lines() {
        let line = line.trim();
        // To https://github.com/org/repo.git
        if line.starts_with("To ") {
            meta.insert("remote_url".into(), Value::String(line[3..].into()));
        }
        // oldhash..newhash  branch -> branch
        if line.contains("..") && line.contains("->") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if !parts.is_empty() {
                meta.insert("hash_range".into(), Value::String(parts[0].into()));
            }
            if let Some(arrow_pos) = parts.iter().position(|&w| w == "->") {
                if arrow_pos > 0 {
                    meta.insert(
                        "branch".into(),
                        Value::String(parts[arrow_pos - 1].into()),
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_classify_tool() {
        assert_eq!(classify_tool("Bash"), "command");
        assert_eq!(classify_tool("Read"), "file_read");
        assert_eq!(classify_tool("Write"), "file_write");
        assert_eq!(classify_tool("Edit"), "file_edit");
        assert_eq!(classify_tool("Grep"), "search");
        assert_eq!(classify_tool("Glob"), "search");
        assert_eq!(classify_tool("Task"), "task_spawn");
        assert_eq!(classify_tool("WebFetch"), "web_fetch");
        assert_eq!(classify_tool("WebSearch"), "web_search");
        assert_eq!(classify_tool("mcp__server__tool"), "mcp_call");
        assert_eq!(classify_tool("Unknown"), "tool_other");
    }

    #[test]
    fn test_classify_bash() {
        assert_eq!(classify_bash("git push"), "git_push");
        assert_eq!(classify_bash("git commit -m \"msg\""), "git_commit");
        assert_eq!(classify_bash("git -C /path push"), "git_push");
        assert_eq!(classify_bash("git -C /path commit -m \"msg\""), "git_commit");
        assert_eq!(classify_bash("git add . && git commit -m \"msg\""), "git_commit");
        assert_eq!(classify_bash("git add . && git commit -m \"msg\" && git push"), "git_push");
        assert_eq!(classify_bash("git status"), "command");
        assert_eq!(classify_bash("gh pr create --title \"feat\""), "github");
        assert_eq!(classify_bash("gh issue view 123"), "github");
        assert_eq!(classify_bash("gh search issues --repo org/repo \"query\""), "github");
        assert_eq!(classify_bash("ls -la"), "command");
        assert_eq!(classify_bash("cargo test"), "command");
    }

    #[test]
    fn test_extract_content() {
        assert_eq!(
            extract_content("Bash", &json!({"command": "ls -la"})),
            "ls -la"
        );
        assert_eq!(
            extract_content("Read", &json!({"file_path": "/tmp/foo.rs"})),
            "/tmp/foo.rs"
        );
        assert_eq!(
            extract_content("Grep", &json!({"pattern": "TODO", "path": "src/"})),
            "TODO in src/"
        );
        assert_eq!(
            extract_content("Grep", &json!({"pattern": "TODO"})),
            "TODO"
        );
        assert_eq!(
            extract_content("WebSearch", &json!({"query": "rust regex"})),
            "rust regex"
        );
        assert_eq!(extract_content("Unknown", &json!({})), "Unknown");
    }

    #[test]
    fn test_extract_content_truncates_bash() {
        let long_cmd: String = "x".repeat(600);
        let result = extract_content("Bash", &json!({"command": long_cmd}));
        assert_eq!(result.len(), 500);
    }

    #[test]
    fn test_extract_file_path() {
        assert_eq!(
            extract_file_path("Read", &json!({"file_path": "/tmp/f.rs"})),
            Some("/tmp/f.rs".into())
        );
        assert_eq!(
            extract_file_path("Grep", &json!({"path": "src/", "pattern": "x"})),
            Some("src/".into())
        );
        assert_eq!(extract_file_path("Bash", &json!({"command": "ls"})), None);
    }

    #[test]
    fn test_extract_git_commit_metadata() {
        let response = "[main 5356097] Add S2 scope classifier\n 14 files changed, 921 insertions(+), 29 deletions(-)\n create mode 100644 src/s2_scope.rs\n create mode 100644 models/converge-diverge.json";
        let meta = extract_git_metadata("git_commit", response);
        assert_eq!(meta["commit_hash"], "5356097");
        assert_eq!(meta["commit_message"], "Add S2 scope classifier");
        assert_eq!(meta["branch"], "main");
        assert_eq!(meta["files_changed"], 14);
        assert_eq!(meta["insertions"], 921);
        assert_eq!(meta["deletions"], 29);
        let new_files = meta["new_files"].as_array().unwrap();
        assert_eq!(new_files.len(), 2);
        assert_eq!(new_files[0], "src/s2_scope.rs");
    }

    #[test]
    fn test_extract_git_commit_root() {
        let response = "[master (root-commit) 52f09a1] initial\n 2 files changed, 2 insertions(+)";
        let meta = extract_git_metadata("git_commit", response);
        assert_eq!(meta["commit_hash"], "52f09a1");
        assert_eq!(meta["branch"], "master");
        assert_eq!(meta["files_changed"], 2);
        assert_eq!(meta["insertions"], 2);
        assert_eq!(meta["deletions"], 0);
    }

    #[test]
    fn test_extract_git_push_metadata() {
        let response = "To https://github.com/viablesys/nmem.git\n   0164631..5356097  main -> main";
        let meta = extract_git_metadata("git_push", response);
        assert_eq!(meta["remote_url"], "https://github.com/viablesys/nmem.git");
        assert_eq!(meta["hash_range"], "0164631..5356097");
        assert_eq!(meta["branch"], "main");
    }

    #[test]
    fn test_extract_git_metadata_empty() {
        let meta = extract_git_metadata("command", "some output");
        assert!(meta.is_empty());
    }
}
