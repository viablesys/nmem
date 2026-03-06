use git2::{Repository, Signature, Time};
use nmem::s1_git::{self, QueryOpts};
use std::fs;
use std::path::Path;
use tempfile::TempDir;

fn sig(ts: i64) -> Signature<'static> {
    Signature::new("Test", "test@test.com", &Time::new(ts, 0)).unwrap()
}

fn make_commit(repo: &Repository, files: &[(&str, &str)], message: &str, ts: i64) -> git2::Oid {
    let mut index = repo.index().unwrap();
    for (path, content) in files {
        let full = repo.workdir().unwrap().join(path);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&full, content).unwrap();
        index.add_path(Path::new(path)).unwrap();
    }
    index.write().unwrap();
    let tree_oid = index.write_tree().unwrap();
    let tree = repo.find_tree(tree_oid).unwrap();
    let sig = sig(ts);

    let parent = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
    let parents: Vec<&git2::Commit> = parent.iter().collect();

    repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parents).unwrap()
}

#[test]
fn file_history_returns_correct_commit_count_and_churn() {
    let dir = TempDir::new().unwrap();
    let repo = Repository::init(dir.path()).unwrap();

    make_commit(&repo, &[("f.txt", "v1")], "initial", 1000000);
    make_commit(&repo, &[("f.txt", "v2\nline2")], "update", 1000100);
    make_commit(&repo, &[("f.txt", "v3\nline2\nline3")], "more", 1000200);

    let hist = s1_git::file_history(dir.path(), "f.txt", &QueryOpts::default()).unwrap();
    assert_eq!(hist.churn.total_commits, 3);
    assert!(hist.churn.total_insertions > 0);
    assert_eq!(hist.path, "f.txt");
}

#[test]
fn dense_summary_within_token_budget() {
    let dir = TempDir::new().unwrap();
    let repo = Repository::init(dir.path()).unwrap();

    make_commit(&repo, &[("f.txt", "v1"), ("other.txt", "x")], "add files", 1000000);
    make_commit(&repo, &[("f.txt", "v2"), ("other.txt", "y")], "update stuff", 1086400);

    let hist = s1_git::file_history(dir.path(), "f.txt", &QueryOpts::default()).unwrap();
    let summary = s1_git::dense_summary(&hist);

    // Dense summary should be compact — ~40 tokens ≈ ~200 chars
    assert!(summary.len() < 500, "summary too long: {} chars", summary.len());
    assert!(summary.starts_with("f.txt:"));
    assert!(summary.contains("commits over"));
    assert!(summary.contains("Recent:"));
}

#[test]
fn co_change_detection() {
    let dir = TempDir::new().unwrap();
    let repo = Repository::init(dir.path()).unwrap();

    make_commit(&repo, &[("a.txt", "a"), ("b.txt", "b")], "init", 1000000);
    make_commit(&repo, &[("a.txt", "a2"), ("b.txt", "b2"), ("c.txt", "c")], "update", 1000100);
    make_commit(&repo, &[("a.txt", "a3"), ("b.txt", "b3")], "again", 1000200);

    let hist = s1_git::file_history(dir.path(), "a.txt", &QueryOpts::default()).unwrap();
    assert!(!hist.co_changes.is_empty());
    assert_eq!(hist.co_changes[0].path, "b.txt");
    assert!(hist.co_changes[0].frequency >= 2);
}

#[test]
fn revert_detection() {
    let dir = TempDir::new().unwrap();
    let repo = Repository::init(dir.path()).unwrap();

    make_commit(&repo, &[("f.txt", "v1")], "initial", 1000000);
    make_commit(&repo, &[("f.txt", "v2")], "Revert \"initial\"", 1000100);
    make_commit(&repo, &[("f.txt", "v3")], "rollback to stable", 1000200);

    let hist = s1_git::file_history(dir.path(), "f.txt", &QueryOpts::default()).unwrap();
    assert_eq!(hist.churn.reverts, 2);
}

#[test]
fn file_not_found_returns_error() {
    let dir = TempDir::new().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    make_commit(&repo, &[("other.txt", "content")], "init", 1000000);

    let result = s1_git::file_history(dir.path(), "nonexistent.txt", &QueryOpts::default());
    assert!(result.is_err());
    let err_msg = format!("{}", result.unwrap_err());
    assert!(err_msg.contains("file not found"), "unexpected error: {err_msg}");
}

#[test]
fn empty_repo_returns_error() {
    let dir = TempDir::new().unwrap();
    Repository::init(dir.path()).unwrap();

    let result = s1_git::file_history(dir.path(), "f.txt", &QueryOpts::default());
    assert!(result.is_err());
    let err_msg = format!("{}", result.unwrap_err());
    assert!(err_msg.contains("empty repository"), "unexpected error: {err_msg}");
}

#[test]
fn max_commits_respected() {
    let dir = TempDir::new().unwrap();
    let repo = Repository::init(dir.path()).unwrap();

    for i in 0..20 {
        let content = format!("v{i}");
        make_commit(&repo, &[("f.txt", &content)], &format!("commit {i}"), 1000000 + i * 100);
    }

    let opts = QueryOpts { max_commits: 5, ..Default::default() };
    let hist = s1_git::file_history(dir.path(), "f.txt", &opts).unwrap();
    assert_eq!(hist.churn.total_commits, 5);
    assert_eq!(hist.commits.len(), 5);
}

#[test]
fn history_serializes_to_json() {
    let dir = TempDir::new().unwrap();
    let repo = Repository::init(dir.path()).unwrap();

    make_commit(&repo, &[("f.txt", "v1")], "initial", 1000000);

    let hist = s1_git::file_history(dir.path(), "f.txt", &QueryOpts::default()).unwrap();
    let json = serde_json::to_string(&hist).unwrap();
    assert!(json.contains("\"path\":\"f.txt\""));
    assert!(json.contains("\"total_commits\":1"));
}
