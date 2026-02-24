use crate::cli::{DispatchArgs, QueueArgs, TaskArgs};
use crate::db::open_db;
use crate::NmemError;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::time::{SystemTime, UNIX_EPOCH};

// --- Tmux helpers ---

fn tmux_session_exists(session: &str) -> bool {
    ProcessCommand::new("tmux")
        .args(["has-session", "-t", session])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn tmux_create_session(session: &str) -> Result<(), NmemError> {
    let status = ProcessCommand::new("tmux")
        .args(["new-session", "-d", "-s", session])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    if !status.success() {
        return Err(NmemError::Config(format!(
            "tmux new-session failed with {}",
            status
        )));
    }
    Ok(())
}

fn tmux_create_window(session: &str, name: &str) -> Result<(), NmemError> {
    let status = ProcessCommand::new("tmux")
        .args(["new-window", "-t", session, "-n", name])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    if !status.success() {
        return Err(NmemError::Config(format!(
            "tmux new-window failed with {}",
            status
        )));
    }
    Ok(())
}

fn tmux_send_keys(target: &str, keys: &str) -> Result<(), NmemError> {
    let status = ProcessCommand::new("tmux")
        .args(["send-keys", "-t", target, keys, "Enter"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    if !status.success() {
        return Err(NmemError::Config(format!(
            "tmux send-keys failed with {}",
            status
        )));
    }
    Ok(())
}

fn tmux_pane_exists(target: &str) -> bool {
    ProcessCommand::new("tmux")
        .args(["list-panes", "-t", target])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

// --- Schedule parsing ---

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// Parse natural language schedule into a Unix timestamp.
///
/// Accepts:
/// - Relative: "5m", "2h", "1d", "30s", "1w" (also "5min", "2hours", "1day", etc.)
/// - Named: "tomorrow", "tonight"
/// - ISO: "2026-02-18", "2026-02-18T15:00", "2026-02-18 15:00"
/// - Unix timestamp: raw integer
pub fn parse_schedule(input: &str) -> Result<i64, NmemError> {
    let input = input.trim().to_lowercase();
    if input.is_empty() {
        return Err(NmemError::Config("empty schedule string".into()));
    }
    let now = now_unix();

    // Named times
    match input.as_str() {
        "now" => return Ok(now),
        "tomorrow" => return Ok(now + 86400),
        "tonight" => {
            // Today at 21:00 local time, or tomorrow 21:00 if past
            if let Some(tonight) = today_at_hour(21) {
                return Ok(if tonight > now { tonight } else { tonight + 86400 });
            }
            return Err(NmemError::Config("cannot determine 'tonight' — `date` command failed".into()));
        }
        _ => {}
    }

    // Relative: "5m", "2h", "30s", "1d", "1w", "in 5 minutes", etc.
    if let Some(secs) = parse_relative(&input) {
        return Ok(now + secs);
    }

    // Raw unix timestamp
    if let Ok(ts) = input.parse::<i64>()
        && ts > 1_000_000_000
    {
        return Ok(ts);
    }

    // ISO-ish datetime: "2026-02-18", "2026-02-18T15:00", "2026-02-18 15:00:00"
    if let Some(ts) = parse_iso_local(&input) {
        return Ok(ts);
    }

    Err(NmemError::Config(format!(
        "cannot parse schedule: {input:?} — try \"5m\", \"2h\", \"tomorrow\", or ISO datetime"
    )))
}

fn parse_relative(input: &str) -> Option<i64> {
    // Strip leading "in " if present
    let s = input.strip_prefix("in ").unwrap_or(input).trim();

    // Split into number and unit: "5m", "2 hours", "30 seconds"
    let (num_str, unit) = if let Some(pos) = s.find(|c: char| !c.is_ascii_digit()) {
        let (n, u) = s.split_at(pos);
        (n.trim(), u.trim())
    } else {
        return None;
    };

    let n: i64 = num_str.parse().ok()?;
    if n <= 0 {
        return None;
    }

    let multiplier = match unit {
        "s" | "sec" | "secs" | "second" | "seconds" => 1,
        "m" | "min" | "mins" | "minute" | "minutes" => 60,
        "h" | "hr" | "hrs" | "hour" | "hours" => 3600,
        "d" | "day" | "days" => 86400,
        "w" | "wk" | "wks" | "week" | "weeks" => 604800,
        _ => return None,
    };

    Some(n * multiplier)
}

fn today_at_hour(hour: u32) -> Option<i64> {
    // Use date command for local timezone conversion — no chrono dependency
    let output = ProcessCommand::new("date")
        .args(["+%s", "-d", &format!("today {hour}:00")])
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&output.stdout);
    s.trim().parse().ok()
}

fn parse_iso_local(input: &str) -> Option<i64> {
    // Delegate to `date -d` for local timezone parsing
    let output = ProcessCommand::new("date")
        .args(["+%s", "-d", input])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&output.stdout);
    s.trim().parse().ok()
}

// --- Task file parsing ---

#[derive(Debug, Default)]
pub struct TaskFile {
    pub project: Option<String>,
    pub cwd: Option<String>,
    pub after: Option<String>,
    pub prompt: String,
}

pub fn parse_task_file(content: &str) -> TaskFile {
    let mut tf = TaskFile::default();

    // Check for frontmatter delimiters
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        tf.prompt = content.trim().to_string();
        return tf;
    }

    // Find closing ---
    let after_first = &trimmed[3..];
    let after_first = after_first.trim_start_matches(['\r', '\n']);
    if let Some(end) = after_first.find("\n---") {
        let frontmatter = &after_first[..end];
        let body = &after_first[end + 4..]; // skip \n---

        // Parse key: value lines
        for line in frontmatter.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Some((key, value)) = line.split_once(':') {
                let key = key.trim().to_lowercase();
                let value = value.trim().to_string();
                if value.is_empty() {
                    continue;
                }
                match key.as_str() {
                    "project" => tf.project = Some(value),
                    "cwd" => tf.cwd = Some(value),
                    "after" => tf.after = Some(value),
                    _ => {} // ignore unknown keys
                }
            }
        }

        tf.prompt = body.trim().to_string();
    } else {
        // No closing --- found, treat entire content as prompt
        tf.prompt = content.trim().to_string();
    }

    tf
}

fn tasks_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join(".nmem").join("tasks")
}

fn output_path_for_task(task_id: i64) -> PathBuf {
    tasks_dir().join(format!("task-{task_id}.md"))
}

fn prompt_path_for_task(task_id: i64) -> PathBuf {
    tasks_dir().join(format!("task-{task_id}.prompt"))
}

// --- Queue ---

pub fn handle_queue(db_path: &Path, args: &QueueArgs) -> Result<(), NmemError> {
    let cwd = args
        .cwd
        .clone()
        .or_else(|| std::env::current_dir().ok().map(|p| p.to_string_lossy().into_owned()));

    let project = args.project.clone().or_else(|| {
        cwd.as_deref()
            .map(crate::s5_project::derive_project)
    });

    let run_after = parse_schedule(&args.after)?;

    let conn = open_db(db_path)?;

    conn.execute(
        "INSERT INTO tasks (prompt, project, cwd, run_after) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![args.prompt, project, cwd, run_after],
    )?;

    let task_id = conn.last_insert_rowid();
    eprintln!("nmem: task {task_id} scheduled for {run_after}");
    println!("{task_id}");
    Ok(())
}

// --- Dispatch ---

struct ReapRow {
    id: i64,
    tmux_target: Option<String>,
}

struct PendingRow {
    id: i64,
    prompt: String,
    cwd: Option<String>,
}

pub fn handle_dispatch(db_path: &Path, args: &DispatchArgs) -> Result<(), NmemError> {
    // If a task file was provided, parse and queue it first
    if let Some(file) = &args.file {
        let content = std::fs::read_to_string(file)?;
        let tf = parse_task_file(&content);

        if tf.prompt.is_empty() {
            return Err(NmemError::Config("task file has no prompt body".into()));
        }

        let cwd = tf
            .cwd
            .or_else(|| std::env::current_dir().ok().map(|p| p.to_string_lossy().into_owned()));
        let project = tf.project.or_else(|| {
            cwd.as_deref()
                .map(crate::s5_project::derive_project)
        });
        let run_after: Option<i64> = tf
            .after
            .as_deref()
            .map(parse_schedule)
            .transpose()?;

        let conn = open_db(db_path)?;
        conn.execute(
            "INSERT INTO tasks (prompt, project, cwd, run_after) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![tf.prompt, project, cwd, run_after],
        )?;
        let task_id = conn.last_insert_rowid();
        eprintln!("nmem: queued task {task_id} from {}", file.display());
        drop(conn);
    }

    let conn = open_db(db_path)?;

    // 1. Reap finished tasks — only need id and tmux_target
    let running: Vec<ReapRow> = {
        let mut stmt = conn.prepare(
            "SELECT id, tmux_target FROM tasks WHERE status = 'running'",
        )?;
        stmt.query_map([], |row| {
            Ok(ReapRow {
                id: row.get(0)?,
                tmux_target: row.get(1)?,
            })
        })?
        .collect::<Result<_, _>>()?
    };

    let mut running_count: u32 = 0;
    for task in &running {
        let target = task.tmux_target.as_deref().unwrap_or("");
        if target.is_empty() || !tmux_pane_exists(target) {
            // Pane gone — mark completed
            conn.execute(
                "UPDATE tasks SET status = 'completed', completed_at = unixepoch('now') WHERE id = ?1",
                [task.id],
            )?;
            eprintln!("nmem: task {} reaped (pane gone)", task.id);
        } else {
            running_count += 1;
        }
    }

    // 2. Check capacity
    if running_count >= args.max_concurrent {
        eprintln!(
            "nmem: at capacity ({running_count}/{} running)",
            args.max_concurrent
        );
        return Ok(());
    }

    let slots = args.max_concurrent - running_count;

    // 3. Find pending tasks past their run_after time.
    // NULL run_after = immediate dispatch (no schedule specified).
    let pending: Vec<PendingRow> = {
        let mut stmt = conn.prepare(
            "SELECT id, prompt, cwd FROM tasks \
             WHERE status = 'pending' AND (run_after IS NULL OR run_after <= unixepoch('now')) \
             ORDER BY created_at ASC LIMIT ?1",
        )?;
        stmt.query_map([slots], |row| {
            Ok(PendingRow {
                id: row.get(0)?,
                prompt: row.get(1)?,
                cwd: row.get(2)?,
            })
        })?
        .collect::<Result<_, _>>()?
    };

    if pending.is_empty() {
        eprintln!("nmem: no pending tasks");
        return Ok(());
    }

    // 4. Dispatch each pending task
    for task in &pending {
        let window_name = format!("task-{}", task.id);
        let target = format!("{}:{}", args.tmux_session, window_name);

        if args.dry_run {
            eprintln!(
                "nmem: [dry-run] would dispatch task {} to {} — {:?}",
                task.id,
                target,
                truncate_prompt(&task.prompt, 60)
            );
            continue;
        }

        // Ensure tmux session exists
        if !tmux_session_exists(&args.tmux_session) {
            tmux_create_session(&args.tmux_session)?;
        }

        // Create window and send commands
        tmux_create_window(&args.tmux_session, &window_name)?;

        if let Some(cwd) = &task.cwd {
            tmux_send_keys(&target, &format!("cd {}", shell_escape(cwd)))?;
        }

        // Ensure task directory exists
        let task_dir = tasks_dir();
        std::fs::create_dir_all(&task_dir)?;

        // Write prompt to file — avoids shell injection via tmux send-keys
        let prompt_path = prompt_path_for_task(task.id);
        std::fs::write(&prompt_path, &task.prompt)?;

        let output_path = output_path_for_task(task.id);
        let prompt_path_str = prompt_path.to_string_lossy();
        let output_path_str = output_path.to_string_lossy();

        // Source user shell environment so dispatched sessions have full PATH
        // (systemd timers have minimal env; bare `cargo` etc. fail without this)
        tmux_send_keys(
            &target,
            "source ~/.cargo/env 2>/dev/null; export PATH=\"$HOME/.local/bin:$HOME/.cargo/bin:$PATH\"",
        )?;

        // Read prompt from file instead of inlining it in the shell command
        tmux_send_keys(
            &target,
            &format!(
                "claude -p \"$(cat '{prompt_path_str}')\" | tee '{output_path_str}'; sleep 5 && exit",
            ),
        )?;

        // Update task status + output path
        conn.execute(
            "UPDATE tasks SET status = 'running', started_at = unixepoch('now'), tmux_target = ?1, output_path = ?2 WHERE id = ?3",
            rusqlite::params![target, output_path_str.as_ref(), task.id],
        )?;

        eprintln!(
            "nmem: dispatched task {} to {} — {:?}",
            task.id,
            target,
            truncate_prompt(&task.prompt, 60)
        );
    }

    Ok(())
}

// --- Task view ---

pub fn handle_task(db_path: &Path, args: &TaskArgs) -> Result<(), NmemError> {
    let conn = open_db(db_path)?;

    let row = conn.query_row(
        "SELECT status, prompt, project, cwd, output_path, created_at, started_at, completed_at, error \
         FROM tasks WHERE id = ?1",
        [args.id],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, Option<i64>>(6)?,
                row.get::<_, Option<i64>>(7)?,
                row.get::<_, Option<String>>(8)?,
            ))
        },
    );

    let (status, prompt, project, cwd, output_path, created_at, started_at, completed_at, error) =
        match row {
            Ok(r) => r,
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                return Err(NmemError::Config(format!("task {} not found", args.id)));
            }
            Err(e) => return Err(e.into()),
        };

    if args.output {
        // Output-only mode for piping
        if let Some(ref path) = output_path {
            match std::fs::read_to_string(path) {
                Ok(content) => print!("{content}"),
                Err(e) => return Err(NmemError::Config(format!("cannot read output: {e}"))),
            }
        }
        return Ok(());
    }

    // Full status display
    println!("Task {}", args.id);
    println!("  status:  {status}");
    println!("  prompt:  {}", truncate_prompt(&prompt, 80));
    if let Some(p) = &project {
        println!("  project: {p}");
    }
    if let Some(c) = &cwd {
        println!("  cwd:     {c}");
    }
    println!("  created: {created_at}");
    if let Some(ts) = started_at {
        println!("  started: {ts}");
    }
    if let Some(ts) = completed_at {
        println!("  done:    {ts}");
    }
    if let Some(e) = &error {
        println!("  error:   {e}");
    }

    if let Some(ref path) = output_path {
        println!("  output:  {path}");
        match std::fs::read_to_string(path) {
            Ok(content) => {
                println!("---");
                print!("{content}");
            }
            Err(_) => {
                println!("  (output file not yet available)");
            }
        }
    } else {
        println!("  output:  (none)");
    }

    Ok(())
}

fn truncate_prompt(prompt: &str, max: usize) -> String {
    if prompt.len() <= max {
        prompt.to_string()
    } else {
        format!("{}...", &prompt[..max])
    }
}

fn shell_escape(s: &str) -> String {
    if s.contains(|c: char| c.is_whitespace() || c == '\'' || c == '"' || c == '\\') {
        format!("'{}'", s.replace('\'', "'\\''"))
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::MIGRATIONS;
    use rusqlite::Connection;

    fn test_db_path() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");
        // Create and initialize the DB
        let mut conn = Connection::open(&db_path).unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        drop(conn);
        (dir, db_path)
    }

    #[test]
    fn queue_inserts_pending_task() {
        let (_dir, db_path) = test_db_path();
        let args = QueueArgs {
            prompt: "fix the auth bug".into(),
            project: Some("nmem".into()),
            cwd: Some("/home/test/workspace/nmem".into()),
            after: "1h".into(),
        };

        handle_queue(&db_path, &args).unwrap();

        let conn = Connection::open(&db_path).unwrap();
        let (status, prompt, project, cwd): (String, String, Option<String>, Option<String>) =
            conn.query_row(
                "SELECT status, prompt, project, cwd FROM tasks WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();

        assert_eq!(status, "pending");
        assert_eq!(prompt, "fix the auth bug");
        assert_eq!(project.as_deref(), Some("nmem"));
        assert_eq!(cwd.as_deref(), Some("/home/test/workspace/nmem"));
    }

    #[test]
    fn parse_schedule_relative() {
        let now = super::now_unix();
        let ts = parse_schedule("5m").unwrap();
        assert!((ts - now - 300).abs() < 2);

        let ts = parse_schedule("2h").unwrap();
        assert!((ts - now - 7200).abs() < 2);

        let ts = parse_schedule("1d").unwrap();
        assert!((ts - now - 86400).abs() < 2);

        let ts = parse_schedule("in 30 seconds").unwrap();
        assert!((ts - now - 30).abs() < 2);

        let ts = parse_schedule("in 1 week").unwrap();
        assert!((ts - now - 604800).abs() < 2);
    }

    #[test]
    fn parse_schedule_named() {
        let now = super::now_unix();
        let ts = parse_schedule("tomorrow").unwrap();
        assert!((ts - now - 86400).abs() < 2);

        let ts = parse_schedule("now").unwrap();
        assert!((ts - now).abs() < 2);
    }

    #[test]
    fn parse_schedule_invalid() {
        assert!(parse_schedule("banana").is_err());
        assert!(parse_schedule("").is_err());
    }

    #[test]
    fn queue_with_schedule() {
        let (_dir, db_path) = test_db_path();
        let args = QueueArgs {
            prompt: "scheduled task".into(),
            project: None,
            cwd: None,
            after: "1h".into(),
        };
        handle_queue(&db_path, &args).unwrap();

        let conn = Connection::open(&db_path).unwrap();
        let run_after: Option<i64> = conn
            .query_row("SELECT run_after FROM tasks WHERE id = 1", [], |r| r.get(0))
            .unwrap();
        assert!(run_after.is_some());
        let now = super::now_unix();
        assert!((run_after.unwrap() - now - 3600).abs() < 5);
    }

    #[test]
    fn dispatch_skips_future_tasks() {
        let (_dir, db_path) = test_db_path();

        // Insert a task scheduled far in the future
        {
            let conn = Connection::open(&db_path).unwrap();
            let future = super::now_unix() + 99999;
            conn.execute(
                "INSERT INTO tasks (status, prompt, run_after) VALUES ('pending', 'future task', ?1)",
                [future],
            )
            .unwrap();
        }

        let dispatch_args = DispatchArgs {
            file: None,
            max_concurrent: 1,
            dry_run: true,
            tmux_session: "nmem-test".into(),
        };
        handle_dispatch(&db_path, &dispatch_args).unwrap();

        // Task should still be pending — not dispatched
        let conn = Connection::open(&db_path).unwrap();
        let status: String = conn
            .query_row("SELECT status FROM tasks WHERE id = 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(status, "pending");
    }

    #[test]
    fn dispatch_skips_when_at_capacity() {
        let (_dir, db_path) = test_db_path();

        // Insert a running task
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute(
                "INSERT INTO tasks (status, prompt, tmux_target, started_at) VALUES ('running', 'existing task', 'nmem:task-99', unixepoch('now'))",
                [],
            )
            .unwrap();
        }

        // Insert a pending task
        let args = QueueArgs {
            prompt: "new task".into(),
            project: None,
            cwd: None,
            after: "1h".into(),
        };
        handle_queue(&db_path, &args).unwrap();

        // Dispatch with max_concurrent=1 — the running task's pane won't exist,
        // so it will be reaped, freeing a slot. This tests the reap-then-dispatch flow.
        let dispatch_args = DispatchArgs {
            file: None,
            max_concurrent: 1,
            dry_run: true,
            tmux_session: "nmem".into(),
        };
        handle_dispatch(&db_path, &dispatch_args).unwrap();

        // Verify the old running task was reaped (pane doesn't exist in test env)
        let conn = Connection::open(&db_path).unwrap();
        let status: String = conn
            .query_row("SELECT status FROM tasks WHERE id = 1", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(status, "completed");
    }

    #[test]
    fn parse_task_file_with_frontmatter() {
        let content = "---\nproject: nmem\ncwd: /home/test/workspace\nafter: 5m\n---\n\nRefactor the search module";
        let tf = parse_task_file(content);
        assert_eq!(tf.project.as_deref(), Some("nmem"));
        assert_eq!(tf.cwd.as_deref(), Some("/home/test/workspace"));
        assert_eq!(tf.after.as_deref(), Some("5m"));
        assert_eq!(tf.prompt, "Refactor the search module");
    }

    #[test]
    fn parse_task_file_body_only() {
        let content = "Just a plain prompt with no frontmatter";
        let tf = parse_task_file(content);
        assert!(tf.project.is_none());
        assert!(tf.cwd.is_none());
        assert!(tf.after.is_none());
        assert_eq!(tf.prompt, "Just a plain prompt with no frontmatter");
    }

    #[test]
    fn parse_task_file_partial_frontmatter() {
        let content = "---\nproject: test\n---\nDo the thing";
        let tf = parse_task_file(content);
        assert_eq!(tf.project.as_deref(), Some("test"));
        assert!(tf.cwd.is_none());
        assert!(tf.after.is_none());
        assert_eq!(tf.prompt, "Do the thing");
    }

    #[test]
    fn parse_task_file_no_closing_delimiter() {
        let content = "---\nproject: test\nThis is actually just text";
        let tf = parse_task_file(content);
        // No closing ---, treat entire content as prompt
        assert!(tf.project.is_none());
        assert_eq!(tf.prompt, content.trim());
    }

    #[test]
    fn output_path_derivation() {
        let path = output_path_for_task(42);
        assert!(path.to_string_lossy().ends_with("tasks/task-42.md"));
    }

    #[test]
    fn file_dispatch_inserts_task() {
        let (_dir, db_path) = test_db_path();
        let task_dir = tempfile::TempDir::new().unwrap();
        let task_file = task_dir.path().join("test-task.md");
        std::fs::write(&task_file, "---\nproject: test-proj\n---\nSay hello").unwrap();

        let args = DispatchArgs {
            file: Some(task_file),
            max_concurrent: 1,
            dry_run: true,
            tmux_session: "nmem-test".into(),
        };
        handle_dispatch(&db_path, &args).unwrap();

        let conn = Connection::open(&db_path).unwrap();
        let (prompt, project): (String, Option<String>) = conn
            .query_row(
                "SELECT prompt, project FROM tasks WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(prompt, "Say hello");
        assert_eq!(project.as_deref(), Some("test-proj"));
    }
}
