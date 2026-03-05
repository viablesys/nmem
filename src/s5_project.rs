use std::path::Path;

/// Strategy for deriving project name from cwd.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProjectStrategy {
    /// Walk parent directories for `.git`, use git repo basename. Falls back to cwd basename.
    #[default]
    Git,
    /// Use basename of cwd directly. For task-directory workflows (e.g., `tmp/OPS-1234`).
    Cwd,
}

/// Derive a project name from a working directory path.
///
/// Default strategy (`Git`): walk parent directories for `.git` dir/file, return that
/// directory's basename. Falls back to cwd basename if no git root found.
///
/// `Cwd` strategy: always use basename of cwd.
pub fn derive_project(cwd: &str) -> String {
    derive_project_with_strategy(cwd, ProjectStrategy::default())
}

pub fn derive_project_with_strategy(cwd: &str, strategy: ProjectStrategy) -> String {
    if cwd.is_empty() {
        return "unknown".into();
    }

    let path = Path::new(cwd);

    // $HOME with no subdirectory → "home"
    if let Ok(home) = std::env::var("HOME")
        && !home.is_empty()
        && path == Path::new(&home)
    {
        return "home".into();
    }

    match strategy {
        ProjectStrategy::Git => {
            if let Some(git_root) = find_git_root(path) {
                return basename_or_unknown(git_root);
            }
            // No git root found — fall back to cwd basename
            basename_or_unknown(path)
        }
        ProjectStrategy::Cwd => basename_or_unknown(path),
    }
}

/// Walk parent directories looking for `.git` (directory or file, for worktrees).
fn find_git_root(start: &Path) -> Option<&Path> {
    let mut current = start;
    loop {
        if current.join(".git").exists() {
            return Some(current);
        }
        match current.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => current = parent,
            _ => return None,
        }
    }
}

fn basename_or_unknown(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "unknown".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn empty_cwd() {
        assert_eq!(derive_project(""), "unknown");
    }

    #[test]
    fn home_directory() {
        let home = std::env::var("HOME").unwrap_or_default();
        if !home.is_empty() {
            assert_eq!(derive_project(&home), "home");
        }
    }

    #[test]
    fn git_repo_root() {
        // nmem itself is a git repo — cwd is the repo root
        let cwd = std::env::current_dir().unwrap();
        let result = derive_project(&cwd.to_string_lossy());
        // Should find .git and return repo basename
        assert_eq!(result, cwd.file_name().unwrap().to_string_lossy());
    }

    #[test]
    fn git_repo_subdirectory() {
        // From a subdirectory of a git repo, should still return repo name
        let cwd = std::env::current_dir().unwrap();
        let sub = cwd.join("src");
        if sub.exists() {
            let result = derive_project(&sub.to_string_lossy());
            assert_eq!(result, cwd.file_name().unwrap().to_string_lossy());
        }
    }

    #[test]
    fn no_git_falls_back_to_basename() {
        // /tmp has no .git — should return basename
        assert_eq!(derive_project("/tmp/scratch"), "scratch");
        assert_eq!(derive_project("/tmp"), "tmp");
    }

    #[test]
    fn cwd_strategy_ignores_git() {
        let cwd = std::env::current_dir().unwrap();
        let sub = cwd.join("src");
        if sub.exists() {
            let result = derive_project_with_strategy(
                &sub.to_string_lossy(),
                ProjectStrategy::Cwd,
            );
            assert_eq!(result, "src");
        }
    }

    #[test]
    fn cwd_strategy_task_directory() {
        // Simulates tmp/OPS-1234 inside a git repo
        let cwd = std::env::current_dir().unwrap();
        let task_dir = cwd.join("tmp").join("OPS-1234");
        fs::create_dir_all(&task_dir).ok();
        let result = derive_project_with_strategy(
            &task_dir.to_string_lossy(),
            ProjectStrategy::Cwd,
        );
        assert_eq!(result, "OPS-1234");
        // Cleanup
        fs::remove_dir_all(cwd.join("tmp").join("OPS-1234")).ok();
    }

    #[test]
    fn root_path() {
        assert_eq!(derive_project("/"), "unknown");
    }
}
