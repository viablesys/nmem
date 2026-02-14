/// Derive a project name from a working directory path.
/// Strips common prefixes (~/workspace, ~/dev, ~/viablesys, ~/forge).
pub fn derive_project(cwd: &str) -> String {
    if cwd.is_empty() {
        return "unknown".into();
    }

    let home = std::env::var("HOME").unwrap_or_default();
    let rel = if !home.is_empty() {
        match cwd.strip_prefix(&home) {
            Some("") | Some("/") => return "home".into(),
            Some(rest) => rest.strip_prefix('/').unwrap_or(rest),
            None => {
                // Not under $HOME — use last path component
                return cwd
                    .rsplit('/')
                    .find(|s| !s.is_empty())
                    .unwrap_or("unknown")
                    .into();
            }
        }
    } else {
        cwd
    };

    let skip = ["workspace", "dev", "viablesys", "forge"];
    let mut last_part = "";
    for part in rel.split('/').filter(|p| !p.is_empty()) {
        last_part = part;
        if !skip.contains(&part) {
            return part.into();
        }
    }

    if last_part.is_empty() {
        "unknown".into()
    } else {
        last_part.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_derive_project() {
        let home = std::env::var("HOME").unwrap_or_default();

        assert_eq!(derive_project(""), "unknown");
        assert_eq!(derive_project(&home), "home");
        assert_eq!(derive_project(&format!("{home}/workspace/nmem")), "nmem");
        assert_eq!(
            derive_project(&format!("{home}/dev/viablesys/forge/myapp")),
            "myapp"
        );
        assert_eq!(derive_project(&format!("{home}/projects/foo")), "projects");
        assert_eq!(derive_project("/tmp/scratch"), "scratch");
    }
}
