use crate::cli::{DispatchArgs, QueueArgs};
use crate::db::open_db;
use crate::NmemError;
use std::path::Path;
use std::process::Command as ProcessCommand;

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

// --- Queue ---

pub fn handle_queue(db_path: &Path, args: &QueueArgs) -> Result<(), NmemError> {
    let cwd = args
        .cwd
        .clone()
        .or_else(|| std::env::current_dir().ok().map(|p| p.to_string_lossy().into_owned()));

    let project = args.project.clone().or_else(|| {
        cwd.as_deref()
            .map(|c| crate::s5_project::derive_project(c))
    });

    let conn = open_db(db_path)?;

    conn.execute(
        "INSERT INTO tasks (prompt, project, cwd) VALUES (?1, ?2, ?3)",
        rusqlite::params![args.prompt, project, cwd],
    )?;

    let task_id = conn.last_insert_rowid();
    println!("{task_id}");
    Ok(())
}

// --- Dispatch ---

#[allow(dead_code)]
struct TaskRow {
    id: i64,
    prompt: String,
    project: Option<String>,
    cwd: Option<String>,
    tmux_target: Option<String>,
}

pub fn handle_dispatch(db_path: &Path, args: &DispatchArgs) -> Result<(), NmemError> {
    let conn = open_db(db_path)?;

    // 1. Reap finished tasks
    let running: Vec<TaskRow> = {
        let mut stmt = conn.prepare(
            "SELECT id, prompt, project, cwd, tmux_target FROM tasks WHERE status = 'running'",
        )?;
        stmt.query_map([], |row| {
            Ok(TaskRow {
                id: row.get(0)?,
                prompt: row.get(1)?,
                project: row.get(2)?,
                cwd: row.get(3)?,
                tmux_target: row.get(4)?,
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

    // 3. Find pending tasks
    let pending: Vec<TaskRow> = {
        let mut stmt = conn.prepare(
            "SELECT id, prompt, project, cwd, tmux_target FROM tasks WHERE status = 'pending' ORDER BY created_at ASC LIMIT ?1",
        )?;
        stmt.query_map([slots], |row| {
            Ok(TaskRow {
                id: row.get(0)?,
                prompt: row.get(1)?,
                project: row.get(2)?,
                cwd: row.get(3)?,
                tmux_target: row.get(4)?,
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

        // Build claude command — escape single quotes in prompt
        let escaped_prompt = task.prompt.replace('\'', "'\\''");
        tmux_send_keys(&target, &format!("claude -p '{escaped_prompt}'"))?;

        // Update task status
        conn.execute(
            "UPDATE tasks SET status = 'running', started_at = unixepoch('now'), tmux_target = ?1 WHERE id = ?2",
            rusqlite::params![target, task.id],
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
        };
        handle_queue(&db_path, &args).unwrap();

        // Dispatch with max_concurrent=1 — the running task's pane won't exist,
        // so it will be reaped, freeing a slot. This tests the reap-then-dispatch flow.
        let dispatch_args = DispatchArgs {
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
}
