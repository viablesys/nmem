//! repo-map — VSM-shaped repository map prototype
//!
//! Scans a Rust project's source files and produces a structural map:
//! - `_project.toml` — project overview with module index
//! - `<module>.toml` — per-module detail (items, deps, metrics)
//! - `_intelligence.toml` — graph analysis (in/out degree, roles, rankings)
//!
//! Standalone prototype extracted from nmem s4_map.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use clap::Parser;
use regex::Regex;
use serde::Serialize;

// ── CLI ──────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(name = "repo-map", about = "Generate VSM-shaped repository map")]
struct Args {
    /// Source directory to scan (default: src/)
    #[arg(long, default_value = "src")]
    src: PathBuf,

    /// Project name (defaults to cwd directory name)
    #[arg(long)]
    project: Option<String>,

    /// Output directory (default: ./map-output/)
    #[arg(long)]
    output: Option<PathBuf>,
}

// ── TOML types: _project.toml ──────────────────────────────────

#[derive(Serialize)]
struct ProjectMap {
    project: ProjectMeta,
    modules: BTreeMap<String, ModuleSummary>,
}

#[derive(Serialize)]
struct ProjectMeta {
    name: String,
    git_hash: String,
    scanned_at: i64,
    module_count: usize,
    total_lines: usize,
}

#[derive(Serialize)]
struct ModuleSummary {
    path: String,
    lines: usize,
    deps: Vec<String>,
    pub_items: Vec<String>,
}

// ── TOML types: <module>.toml ──────────────────────────────────

#[derive(Serialize)]
struct ModuleMap {
    module: ModuleMeta,
    coordination: ModuleCoord,
    control: ModuleControl,
    policy: ModulePolicy,
    items: Vec<ItemEntry>,
}

#[derive(Serialize)]
struct ModuleMeta {
    name: String,
    path: String,
    lines: usize,
}

#[derive(Serialize)]
struct ModuleCoord {
    deps: Vec<String>,
}

#[derive(Serialize)]
struct ModuleControl {
    lines: usize,
    dep_count: usize,
}

#[derive(Serialize)]
struct ModulePolicy {
    pub_fns: Vec<String>,
    pub_types: Vec<String>,
}

#[derive(Serialize)]
struct ItemEntry {
    name: String,
    kind: String,
}

// ── TOML types: _intelligence.toml ─────────────────────────────

#[derive(Serialize)]
struct IntelligenceMap {
    graph: GraphSummary,
    ranking: Rankings,
    modules: BTreeMap<String, ModuleIntel>,
}

#[derive(Serialize)]
struct GraphSummary {
    total_modules: usize,
    total_edges: usize,
    foundation: Vec<String>,
    leaf: Vec<String>,
}

#[derive(Serialize)]
struct Rankings {
    by_in_degree: Vec<String>,
    by_lines: Vec<String>,
    by_api_surface: Vec<String>,
}

#[derive(Serialize)]
struct ModuleIntel {
    in_degree: usize,
    out_degree: usize,
    role: String,
    lines_per_pub_item: usize,
}

// ── Scanner ────────────────────────────────────────────────────

struct FileScan {
    name: String,
    stem: String,
    rel_path: String,
    lines: usize,
    pub_fns: Vec<String>,
    pub_types: Vec<String>,
    deps: Vec<String>,
}

fn scan_rust_file(path: &Path, root: &Path) -> std::io::Result<FileScan> {
    let content = fs::read_to_string(path)?;
    let line_count = content.lines().count();

    let rel_path = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned();

    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();

    let name = rel_path
        .strip_suffix(".rs")
        .unwrap_or(&rel_path)
        .replace("/src/", "/")
        .replace("\\src\\", "\\");

    let re_pub_fn = Regex::new(r"^\s*pub\s+(?:async\s+)?fn\s+(\w+)").unwrap();
    let re_pub_type =
        Regex::new(r"^\s*pub\s+(?:struct|enum|trait|type|const|static)\s+(\w+)").unwrap();
    let re_dep = Regex::new(r"use\s+crate::(\w+)").unwrap();

    let mut pub_fns = Vec::new();
    let mut pub_types = Vec::new();
    let mut deps = BTreeSet::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("//") {
            continue;
        }

        for cap in re_dep.captures_iter(line) {
            deps.insert(cap[1].to_string());
        }

        if let Some(cap) = re_pub_fn.captures(line) {
            pub_fns.push(cap[1].to_string());
            continue;
        }

        if let Some(cap) = re_pub_type.captures(line) {
            pub_types.push(cap[1].to_string());
        }
    }

    Ok(FileScan {
        name,
        stem,
        rel_path,
        lines: line_count,
        pub_fns,
        pub_types,
        deps: deps.into_iter().collect(),
    })
}

fn git_head_short(root: &Path) -> String {
    let Ok(repo) = git2::Repository::open(root) else {
        return "unknown".into();
    };
    repo.head()
        .ok()
        .and_then(|head| head.target())
        .map(|oid| oid.to_string()[..7].to_string())
        .unwrap_or_else(|| "unknown".into())
}

fn walk_rs_files(dir: &Path, files: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            walk_rs_files(&path, files)?;
        } else if path.extension().is_some_and(|e| e == "rs") {
            files.push(path);
        }
    }
    Ok(())
}

// ── Context injection ──────────────────────────────────────────

/// Scan the current project and return a formatted map for context injection.
/// Returns None if src/ doesn't exist or scan finds nothing.
pub fn generate_map_context(src_dir: &Path) -> Option<String> {
    let root = src_dir.parent().unwrap_or(src_dir);

    let mut rs_files = Vec::new();
    walk_rs_files(src_dir, &mut rs_files).ok()?;
    if rs_files.is_empty() {
        return None;
    }

    let mut scans = Vec::new();
    for path in &rs_files {
        if let Ok(scan) = scan_rust_file(path, root) {
            scans.push(scan);
        }
    }
    if scans.is_empty() {
        return None;
    }
    scans.sort_by(|a, b| a.name.cmp(&b.name));

    let total_lines: usize = scans.iter().map(|s| s.lines).sum();

    let stem_to_name: BTreeMap<&str, &str> = scans
        .iter()
        .map(|s| (s.stem.as_str(), s.name.as_str()))
        .collect();
    let module_names: BTreeSet<&str> = scans.iter().map(|s| s.name.as_str()).collect();

    let mut in_degree: BTreeMap<&str, usize> = BTreeMap::new();
    let mut total_edges = 0usize;
    for scan in &scans {
        for dep in &scan.deps {
            let resolved = if module_names.contains(dep.as_str()) {
                Some(dep.as_str())
            } else {
                stem_to_name.get(dep.as_str()).copied()
            };
            if let Some(target) = resolved {
                *in_degree.entry(target).or_default() += 1;
                total_edges += 1;
            }
        }
    }

    let mut out = format!(
        "## Repo map ({} modules, {} lines, {} edges)\n\n",
        scans.len(),
        total_lines,
        total_edges
    );

    let mut ranked: Vec<(&str, usize)> = scans
        .iter()
        .map(|s| (s.name.as_str(), in_degree.get(s.name.as_str()).copied().unwrap_or(0)))
        .filter(|(_, d)| *d > 0)
        .collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1));

    if !ranked.is_empty() {
        out.push_str("Foundation (most depended on): ");
        let top: Vec<String> = ranked
            .iter()
            .take(10)
            .map(|(name, d)| format!("{name}({d})"))
            .collect();
        out.push_str(&top.join(", "));
        out.push('\n');
    }

    out.push_str("\n| Module | Lines | Deps | Pub API |\n|---|---|---|---|\n");
    for scan in &scans {
        let pub_count = scan.pub_fns.len() + scan.pub_types.len();
        let deps_str = if scan.deps.is_empty() {
            "\u{2014}".into()
        } else {
            scan.deps.join(", ")
        };
        let pub_str = if pub_count == 0 {
            "\u{2014}".into()
        } else {
            let mut items: Vec<&str> = scan.pub_fns.iter().map(|s| s.as_str()).collect();
            items.extend(scan.pub_types.iter().map(|s| s.as_str()));
            if items.len() > 5 {
                let first: Vec<&str> = items[..4].to_vec();
                format!("{} +{} more", first.join(", "), items.len() - 4)
            } else {
                items.join(", ")
            }
        };
        out.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            scan.name, scan.lines, deps_str, pub_str
        ));
    }

    Some(out)
}

// ── CLI handler ────────────────────────────────────────────────

fn run(args: &Args) -> Result<(), Box<dyn std::error::Error>> {
    let cwd = std::env::current_dir()?;

    let project = args.project.clone().unwrap_or_else(|| {
        cwd.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "unknown".into())
    });

    let src_dir = cwd.join(&args.src);
    if !src_dir.is_dir() {
        return Err(format!("source directory not found: {}", src_dir.display()).into());
    }

    let out_dir = args.output.clone().unwrap_or_else(|| {
        cwd.join("map-output").join(&project)
    });
    fs::create_dir_all(&out_dir)?;

    let mut rs_files = Vec::new();
    walk_rs_files(&src_dir, &mut rs_files)?;

    let mut scans = Vec::new();
    for path in &rs_files {
        match scan_rust_file(path, &cwd) {
            Ok(scan) => scans.push(scan),
            Err(e) => eprintln!("repo-map: skipping {}: {e}", path.display()),
        }
    }
    scans.sort_by(|a, b| a.name.cmp(&b.name));

    let git_hash = git_head_short(&cwd);
    let total_lines: usize = scans.iter().map(|s| s.lines).sum();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    // ── Write _project.toml ──

    let mut modules = BTreeMap::new();
    for scan in &scans {
        let mut pub_items: Vec<String> = scan.pub_fns.clone();
        pub_items.extend(scan.pub_types.clone());

        modules.insert(
            scan.name.clone(),
            ModuleSummary {
                path: scan.rel_path.clone(),
                lines: scan.lines,
                deps: scan.deps.clone(),
                pub_items,
            },
        );
    }

    let project_map = ProjectMap {
        project: ProjectMeta {
            name: project.clone(),
            git_hash: git_hash.clone(),
            scanned_at: now,
            module_count: scans.len(),
            total_lines,
        },
        modules,
    };

    let project_toml = toml::to_string_pretty(&project_map)?;
    fs::write(out_dir.join("_project.toml"), &project_toml)?;

    // ── Write per-module files ──

    for scan in &scans {
        let items: Vec<ItemEntry> = scan
            .pub_fns
            .iter()
            .map(|f| ItemEntry {
                name: f.clone(),
                kind: "function".into(),
            })
            .chain(scan.pub_types.iter().map(|t| ItemEntry {
                name: t.clone(),
                kind: "type".into(),
            }))
            .collect();

        let module_map = ModuleMap {
            module: ModuleMeta {
                name: scan.name.clone(),
                path: scan.rel_path.clone(),
                lines: scan.lines,
            },
            coordination: ModuleCoord {
                deps: scan.deps.clone(),
            },
            control: ModuleControl {
                lines: scan.lines,
                dep_count: scan.deps.len(),
            },
            policy: ModulePolicy {
                pub_fns: scan.pub_fns.clone(),
                pub_types: scan.pub_types.clone(),
            },
            items,
        };

        let module_toml = toml::to_string_pretty(&module_map)?;
        let safe_name = scan.name.replace('/', "--");
        fs::write(out_dir.join(format!("{safe_name}.toml")), &module_toml)?;
    }

    // ── Compute and write _intelligence.toml ──

    let stem_to_name: BTreeMap<&str, &str> = scans
        .iter()
        .map(|s| (s.stem.as_str(), s.name.as_str()))
        .collect();
    let module_names: BTreeSet<&str> = scans.iter().map(|s| s.name.as_str()).collect();

    let mut in_degree: BTreeMap<&str, usize> = BTreeMap::new();
    let mut total_edges = 0usize;
    for scan in &scans {
        for dep in &scan.deps {
            let resolved = if module_names.contains(dep.as_str()) {
                Some(dep.as_str())
            } else {
                stem_to_name.get(dep.as_str()).copied()
            };
            if let Some(target) = resolved {
                *in_degree.entry(target).or_default() += 1;
                total_edges += 1;
            }
        }
    }

    let mut intel_modules = BTreeMap::new();
    for scan in &scans {
        let in_d = in_degree.get(scan.name.as_str()).copied().unwrap_or(0);
        let out_d = scan.deps.iter().filter(|d| {
            module_names.contains(d.as_str()) || stem_to_name.contains_key(d.as_str())
        }).count();
        let pub_count = scan.pub_fns.len() + scan.pub_types.len();
        let lines_per_pub = if pub_count > 0 { scan.lines / pub_count } else { scan.lines };

        let role = if in_d == 0 && out_d == 0 {
            "isolated"
        } else if in_d == 0 {
            "leaf"
        } else if out_d == 0 {
            "foundation"
        } else if in_d >= 5 && out_d >= 5 {
            "hub"
        } else if out_d > in_d * 2 {
            "orchestrator"
        } else if in_d > out_d * 2 {
            "foundation"
        } else {
            "internal"
        };

        intel_modules.insert(
            scan.name.clone(),
            ModuleIntel {
                in_degree: in_d,
                out_degree: out_d,
                role: role.into(),
                lines_per_pub_item: lines_per_pub,
            },
        );
    }

    let foundation: Vec<String> = scans
        .iter()
        .filter(|s| {
            s.deps.iter().all(|d| {
                !module_names.contains(d.as_str()) && !stem_to_name.contains_key(d.as_str())
            }) && in_degree.get(s.name.as_str()).copied().unwrap_or(0) > 0
        })
        .map(|s| s.name.clone())
        .collect();

    let leaf: Vec<String> = scans
        .iter()
        .filter(|s| in_degree.get(s.name.as_str()).copied().unwrap_or(0) == 0)
        .map(|s| s.name.clone())
        .collect();

    let mut by_in_degree: Vec<(String, usize)> = scans
        .iter()
        .map(|s| (s.name.clone(), in_degree.get(s.name.as_str()).copied().unwrap_or(0)))
        .collect();
    by_in_degree.sort_by(|a, b| b.1.cmp(&a.1));

    let mut by_lines: Vec<(String, usize)> = scans.iter().map(|s| (s.name.clone(), s.lines)).collect();
    by_lines.sort_by(|a, b| b.1.cmp(&a.1));

    let mut by_api: Vec<(String, usize)> = scans
        .iter()
        .map(|s| (s.name.clone(), s.pub_fns.len() + s.pub_types.len()))
        .collect();
    by_api.sort_by(|a, b| b.1.cmp(&a.1));

    let intel = IntelligenceMap {
        graph: GraphSummary {
            total_modules: scans.len(),
            total_edges,
            foundation,
            leaf,
        },
        ranking: Rankings {
            by_in_degree: by_in_degree.iter().take(15).map(|(n, _)| n.clone()).collect(),
            by_lines: by_lines.iter().take(15).map(|(n, _)| n.clone()).collect(),
            by_api_surface: by_api.iter().take(15).map(|(n, _)| n.clone()).collect(),
        },
        modules: intel_modules,
    };

    let intel_toml = toml::to_string_pretty(&intel)?;
    fs::write(out_dir.join("_intelligence.toml"), &intel_toml)?;

    eprintln!(
        "repo-map: mapped {} modules ({} lines, {} edges) → {}",
        scans.len(),
        total_lines,
        total_edges,
        out_dir.display()
    );
    Ok(())
}

fn main() {
    let args = Args::parse();
    if let Err(e) = run(&args) {
        eprintln!("repo-map: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_file(dir: &Path, name: &str, content: &str) {
        fs::write(dir.join(name), content).unwrap();
    }

    fn make_project(tmp: &Path) -> PathBuf {
        let src = tmp.join("src");
        fs::create_dir_all(&src).unwrap();
        src
    }

    fn scan_str(tmp: &tempfile::TempDir, filename: &str, content: &str) -> FileScan {
        let src = make_project(tmp.path());
        write_file(&src, filename, content);
        scan_rust_file(&src.join(filename), tmp.path()).unwrap()
    }

    #[test]
    fn scan_extracts_pub_fns() {
        let tmp = tempfile::tempdir().unwrap();
        let scan = scan_str(&tmp, "foo.rs", "pub fn hello() {}\npub fn world() {}\nfn private() {}\n");
        assert_eq!(scan.pub_fns, vec!["hello", "world"]);
        assert!(scan.pub_types.is_empty());
    }

    #[test]
    fn scan_extracts_pub_types() {
        let tmp = tempfile::tempdir().unwrap();
        let scan = scan_str(
            &tmp,
            "types.rs",
            "pub struct Foo {}\npub enum Bar {}\npub trait Baz {}\npub type Alias = i32;\npub const MAX: usize = 10;\npub static GLOBAL: i32 = 0;\nstruct Private {}\n",
        );
        assert_eq!(scan.pub_types, vec!["Foo", "Bar", "Baz", "Alias", "MAX", "GLOBAL"]);
        assert!(scan.pub_fns.is_empty());
    }

    #[test]
    fn scan_extracts_async_fn() {
        let tmp = tempfile::tempdir().unwrap();
        let scan = scan_str(&tmp, "a.rs", "pub async fn fetch() {}\npub fn sync_fn() {}\n");
        assert_eq!(scan.pub_fns, vec!["fetch", "sync_fn"]);
    }

    #[test]
    fn scan_extracts_deps_sorted() {
        let tmp = tempfile::tempdir().unwrap();
        let scan = scan_str(
            &tmp,
            "c.rs",
            "use crate::zebra;\nuse crate::alpha::sub;\nuse std::path::Path;\npub fn run() {}\n",
        );
        assert_eq!(scan.deps, vec!["alpha", "zebra"]);
    }

    #[test]
    fn scan_deduplicates_deps() {
        let tmp = tempfile::tempdir().unwrap();
        let scan = scan_str(
            &tmp,
            "d.rs",
            "use crate::db;\nuse crate::db::open;\nuse crate::db::close;\n",
        );
        assert_eq!(scan.deps, vec!["db"]);
    }

    #[test]
    fn scan_skips_comments() {
        let tmp = tempfile::tempdir().unwrap();
        let scan = scan_str(
            &tmp,
            "commented.rs",
            "// pub fn fake() {}\npub fn real() {}\n// pub struct Ghost {}\n",
        );
        assert_eq!(scan.pub_fns, vec!["real"]);
        assert!(scan.pub_types.is_empty());
    }

    #[test]
    fn scan_counts_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let scan = scan_str(&tmp, "lines.rs", "line1\nline2\nline3\n");
        assert_eq!(scan.lines, 3);
    }

    #[test]
    fn scan_empty_file() {
        let tmp = tempfile::tempdir().unwrap();
        let scan = scan_str(&tmp, "empty.rs", "");
        assert!(scan.pub_fns.is_empty());
        assert!(scan.pub_types.is_empty());
        assert!(scan.deps.is_empty());
        assert_eq!(scan.lines, 0);
    }

    #[test]
    fn context_returns_none_for_missing_dir() {
        assert!(generate_map_context(Path::new("/nonexistent/src")).is_none());
    }

    #[test]
    fn context_returns_none_for_empty_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let src = make_project(tmp.path());
        assert!(generate_map_context(&src).is_none());
    }

    #[test]
    fn context_produces_header_and_table() {
        let tmp = tempfile::tempdir().unwrap();
        let src = make_project(tmp.path());
        write_file(&src, "db.rs", "pub fn open() {}\npub struct Conn {}\n");
        write_file(&src, "app.rs", "use crate::db;\npub fn run() {}\n");

        let ctx = generate_map_context(&src).unwrap();
        assert!(ctx.contains("## Repo map"), "missing header");
        assert!(ctx.contains("2 modules"), "wrong module count");
        assert!(ctx.contains("| Module |"), "missing table header");
    }

    #[test]
    fn context_computes_edges_and_foundation() {
        let tmp = tempfile::tempdir().unwrap();
        let src = make_project(tmp.path());
        write_file(&src, "base.rs", "pub fn foundation() {}\n");
        write_file(&src, "mid.rs", "use crate::base;\npub fn middle() {}\n");
        write_file(&src, "top.rs", "use crate::mid;\npub fn entry() {}\n");

        let ctx = generate_map_context(&src).unwrap();
        assert!(ctx.contains("2 edges"), "wrong edge count: {ctx}");
        assert!(ctx.contains("Foundation"), "missing foundation");
    }

    #[test]
    fn context_diamond_dependency() {
        let tmp = tempfile::tempdir().unwrap();
        let src = make_project(tmp.path());
        write_file(&src, "d.rs", "pub fn shared() {}\n");
        write_file(&src, "b.rs", "use crate::d;\npub fn left() {}\n");
        write_file(&src, "c.rs", "use crate::d;\npub fn right() {}\n");
        write_file(&src, "a.rs", "use crate::b;\nuse crate::c;\npub fn top() {}\n");

        let ctx = generate_map_context(&src).unwrap();
        assert!(ctx.contains("4 edges"), "diamond = 4 edges: {ctx}");
        assert!(ctx.contains("Foundation"), "d should be foundation in diamond");
    }
}
