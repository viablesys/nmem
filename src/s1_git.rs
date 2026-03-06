use git2::{Patch, Repository, Sort};
use regex::Regex;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::LazyLock;

use crate::NmemError;

#[derive(Debug, Clone, Serialize)]
pub struct FileCommit {
    pub oid: String,
    pub message: String,
    pub author: String,
    pub timestamp: i64,
    pub insertions: usize,
    pub deletions: usize,
    pub is_revert: bool,
    pub co_changed: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChurnMetrics {
    pub total_commits: usize,
    pub total_insertions: usize,
    pub total_deletions: usize,
    pub first_commit_ts: i64,
    pub last_commit_ts: i64,
    pub reverts: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct CoChange {
    pub path: String,
    pub frequency: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileHistory {
    pub path: String,
    pub commits: Vec<FileCommit>,
    pub churn: ChurnMetrics,
    pub co_changes: Vec<CoChange>,
}

pub struct QueryOpts {
    pub max_commits: usize,
    pub max_co_changes: usize,
    pub include_co_changes: bool,
}

impl Default for QueryOpts {
    fn default() -> Self {
        Self {
            max_commits: 50,
            max_co_changes: 10,
            include_co_changes: true,
        }
    }
}

static REVERT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^revert\b|(?i)\brollback\b").unwrap()
});

fn is_revert(message: &str) -> bool {
    REVERT_RE.is_match(message)
}

pub fn file_history(repo_path: &Path, file_path: &str, opts: &QueryOpts) -> Result<FileHistory, NmemError> {
    let repo = Repository::discover(repo_path)
        .map_err(|e| NmemError::Config(format!("git: {e}")))?;

    let head = match repo.head() {
        Ok(h) => h,
        Err(e) if e.code() == git2::ErrorCode::UnbornBranch || e.code() == git2::ErrorCode::NotFound => {
            return Err(NmemError::Config("empty repository (no commits)".into()));
        }
        Err(e) => return Err(NmemError::Config(format!("git: {e}"))),
    };

    let mut revwalk = repo.revwalk()
        .map_err(|e| NmemError::Config(format!("git: {e}")))?;
    revwalk.set_sorting(Sort::TIME | Sort::TOPOLOGICAL)
        .map_err(|e| NmemError::Config(format!("git: {e}")))?;
    revwalk.push(head.target().ok_or_else(|| {
        NmemError::Config("HEAD has no target".into())
    })?)
    .map_err(|e| NmemError::Config(format!("git: {e}")))?;

    let mut commits = Vec::new();
    let mut co_change_counts: HashMap<String, usize> = HashMap::new();
    let mut found_any = false;

    for oid_result in revwalk {
        let oid = oid_result.map_err(|e| NmemError::Config(format!("git: {e}")))?;
        let commit = repo.find_commit(oid)
            .map_err(|e| NmemError::Config(format!("git: {e}")))?;
        let tree = commit.tree()
            .map_err(|e| NmemError::Config(format!("git: {e}")))?;

        let entry = match tree.get_path(Path::new(file_path)) {
            Ok(e) => e,
            Err(_) => continue,
        };

        found_any = true;

        let parent_tree = if commit.parent_count() > 0 {
            Some(commit.parent(0)
                .map_err(|e| NmemError::Config(format!("git: {e}")))?
                .tree()
                .map_err(|e| NmemError::Config(format!("git: {e}")))?)
        } else {
            None
        };

        // Fast: compare blob OIDs — skip if unchanged
        if let Some(ref pt) = parent_tree
            && let Ok(parent_entry) = pt.get_path(Path::new(file_path))
            && parent_entry.id() == entry.id()
        {
            continue;
        }

        let diff = repo.diff_tree_to_tree(
            parent_tree.as_ref(),
            Some(&tree),
            None,
        ).map_err(|e| NmemError::Config(format!("git: {e}")))?;

        let mut insertions = 0;
        let mut deletions = 0;
        let mut co_changed = Vec::new();

        for (idx, delta) in diff.deltas().enumerate() {
            let delta_path = delta.new_file().path()
                .or_else(|| delta.old_file().path());

            let delta_path_str = match delta_path {
                Some(p) => p.to_string_lossy().to_string(),
                None => continue,
            };

            if delta_path_str == file_path {
                if let Ok(Some(ref mut patch)) = Patch::from_diff(&diff, idx)
                    && let Ok((_ctx, add, del)) = patch.line_stats()
                {
                    insertions = add;
                    deletions = del;
                }
            } else if opts.include_co_changes {
                co_changed.push(delta_path_str);
            }
        }

        let msg = commit.message().unwrap_or("").lines().next().unwrap_or("").to_string();

        for path in &co_changed {
            *co_change_counts.entry(path.clone()).or_default() += 1;
        }

        commits.push(FileCommit {
            oid: format!("{:.8}", oid),
            message: msg.clone(),
            author: commit.author().name().unwrap_or("unknown").to_string(),
            timestamp: commit.time().seconds(),
            insertions,
            deletions,
            is_revert: is_revert(&msg),
            co_changed,
        });

        if commits.len() >= opts.max_commits {
            break;
        }
    }

    if !found_any {
        return Err(NmemError::Config(format!("file not found in history: {file_path}")));
    }

    let churn = ChurnMetrics {
        total_commits: commits.len(),
        total_insertions: commits.iter().map(|c| c.insertions).sum(),
        total_deletions: commits.iter().map(|c| c.deletions).sum(),
        first_commit_ts: commits.last().map(|c| c.timestamp).unwrap_or(0),
        last_commit_ts: commits.first().map(|c| c.timestamp).unwrap_or(0),
        reverts: commits.iter().filter(|c| c.is_revert).count(),
    };

    let mut co_changes: Vec<CoChange> = co_change_counts
        .into_iter()
        .map(|(path, frequency)| CoChange { path, frequency })
        .collect();
    co_changes.sort_by(|a, b| b.frequency.cmp(&a.frequency));
    co_changes.truncate(opts.max_co_changes);

    Ok(FileHistory {
        path: file_path.to_string(),
        commits,
        churn,
        co_changes,
    })
}

pub fn dense_summary(history: &FileHistory) -> String {
    let mut out = String::new();

    let days = if history.churn.last_commit_ts > history.churn.first_commit_ts {
        (history.churn.last_commit_ts - history.churn.first_commit_ts) / 86400
    } else {
        0
    };

    out.push_str(&format!(
        "{}: {} commits over {}d, +{}/-{}",
        history.path,
        history.churn.total_commits,
        days,
        history.churn.total_insertions,
        history.churn.total_deletions,
    ));
    if history.churn.reverts > 0 {
        out.push_str(&format!(", {} reverts", history.churn.reverts));
    }
    out.push('.');

    if !history.co_changes.is_empty() {
        let top: Vec<String> = history.co_changes.iter()
            .take(3)
            .map(|c| format!("{}({})", c.path, c.frequency))
            .collect();
        out.push_str(&format!(" Co-changes: {}.", top.join(", ")));
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let recent: Vec<String> = history.commits.iter()
        .take(2)
        .map(|c| {
            let age_days = (now - c.timestamp) / 86400;
            let age_str = if age_days == 0 {
                "today".to_string()
            } else if age_days == 1 {
                "1d ago".to_string()
            } else {
                format!("{age_days}d ago")
            };
            format!("\"{}\" ({})", c.message, age_str)
        })
        .collect();

    if !recent.is_empty() {
        out.push_str(&format!(" Recent: {}.", recent.join(", ")));
    }

    out
}

#[derive(Debug, Clone)]
pub struct BlameHunk {
    pub start_line: usize, // 1-based
    pub line_count: usize,
    pub oid: String,       // 8-char prefix
    pub author: String,
    pub timestamp: i64,
    pub message: String,   // first line
    pub co_changed: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct CachedBlame {
    pub hunks: Vec<BlameHunk>,
}

impl CachedBlame {
    pub fn find_hunk(&self, line: usize) -> Option<&BlameHunk> {
        self.hunks.iter().find(|h| line >= h.start_line && line < h.start_line + h.line_count)
    }
}

pub fn blame_file(repo_path: &Path, file_path: &str) -> Result<CachedBlame, NmemError> {
    let repo = Repository::discover(repo_path)
        .map_err(|e| NmemError::Config(format!("git: {e}")))?;

    let blame = repo.blame_file(Path::new(file_path), None)
        .map_err(|e| NmemError::Config(format!("git blame: {e}")))?;

    // Collect commit metadata, deduplicating by OID
    let mut commit_cache: HashMap<git2::Oid, (String, i64, String, Vec<String>)> = HashMap::new();
    let mut seen_oids: HashSet<git2::Oid> = HashSet::new();

    let mut raw_hunks: Vec<(usize, usize, git2::Oid)> = Vec::new();

    for i in 0..blame.len() {
        let hunk = blame.get_index(i)
            .ok_or_else(|| NmemError::Config("blame hunk index out of bounds".into()))?;

        let oid = hunk.final_commit_id();
        let start = hunk.final_start_line(); // already 1-based
        let count = hunk.lines_in_hunk();

        raw_hunks.push((start, count, oid));
        seen_oids.insert(oid);
    }

    // Populate commit cache
    for &oid in &seen_oids {
        if oid.is_zero() {
            commit_cache.insert(oid, ("(uncommitted)".into(), 0, "(uncommitted changes)".into(), vec![]));
            continue;
        }
        let commit = match repo.find_commit(oid) {
            Ok(c) => c,
            Err(_) => {
                commit_cache.insert(oid, ("unknown".into(), 0, "".into(), vec![]));
                continue;
            }
        };

        let author = commit.author().name().unwrap_or("unknown").to_string();
        let timestamp = commit.time().seconds();
        let message = commit.message().unwrap_or("").lines().next().unwrap_or("").to_string();

        // Co-changed files: diff commit tree vs parent tree
        let co_changed = co_changed_files(&repo, &commit, file_path);

        commit_cache.insert(oid, (author, timestamp, message, co_changed));
    }

    let hunks = raw_hunks.into_iter().map(|(start, count, oid)| {
        let (author, timestamp, message, co_changed) = commit_cache.get(&oid)
            .cloned()
            .unwrap_or_default();
        BlameHunk {
            start_line: start,
            line_count: count,
            oid: format!("{:.8}", oid),
            author,
            timestamp,
            message,
            co_changed,
        }
    }).collect();

    Ok(CachedBlame { hunks })
}

fn co_changed_files(repo: &Repository, commit: &git2::Commit, exclude_path: &str) -> Vec<String> {
    let tree = match commit.tree() {
        Ok(t) => t,
        Err(_) => return vec![],
    };

    let parent_tree = if commit.parent_count() > 0 {
        commit.parent(0).ok().and_then(|p| p.tree().ok())
    } else {
        None
    };

    let diff = match repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), None) {
        Ok(d) => d,
        Err(_) => return vec![],
    };

    diff.deltas()
        .filter_map(|delta| {
            let path = delta.new_file().path().or_else(|| delta.old_file().path())?;
            let s = path.to_string_lossy();
            if s == exclude_path { None } else { Some(s.to_string()) }
        })
        .collect()
}
