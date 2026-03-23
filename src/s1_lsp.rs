use crate::NmemError;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use tokio::sync::Mutex;
use tower_lsp_server::jsonrpc::Result;
use tower_lsp_server::ls_types::*;
use tower_lsp_server::{Client, LanguageServer, LspService, Server};

#[derive(Debug)]
struct NmemLsp {
    client: Client,
    emitted: Mutex<HashSet<String>>,
    blame_cache: Mutex<HashMap<String, crate::s1_git::CachedBlame>>,
}

impl NmemLsp {
    async fn maybe_publish_diagnostics(&self, uri: Uri) {
        let file_path = match uri_to_path(&uri) {
            Some(p) => p,
            None => return,
        };

        {
            let emitted = self.emitted.lock().await;
            if emitted.contains(&file_path) {
                return;
            }
        }

        self.publish_git_diagnostic(&uri, &file_path).await;
    }

    async fn force_publish_diagnostics(&self, uri: Uri) {
        let file_path = match uri_to_path(&uri) {
            Some(p) => p,
            None => return,
        };

        // Remove from emitted so we re-publish, and invalidate blame cache
        {
            let mut emitted = self.emitted.lock().await;
            emitted.remove(&file_path);
        }
        {
            let mut cache = self.blame_cache.lock().await;
            cache.remove(&file_path);
        }

        self.publish_git_diagnostic(&uri, &file_path).await;
    }

    async fn publish_git_diagnostic(&self, uri: &Uri, file_path: &str) {
        let cwd = match std::env::current_dir() {
            Ok(d) => d,
            Err(_) => return,
        };

        let relative = make_relative(&cwd, file_path);
        let opts = crate::s1_git::QueryOpts {
            max_commits: 50,
            ..Default::default()
        };

        let summary = match crate::s1_git::file_history(&cwd, &relative, &opts) {
            Ok(history) => crate::s1_git::dense_summary(&history),
            Err(_) => return,
        };

        let diagnostics = vec![Diagnostic {
            range: Range::new(Position::new(0, 0), Position::new(0, 0)),
            severity: Some(DiagnosticSeverity::INFORMATION),
            source: Some("nmem".into()),
            message: summary,
            ..Default::default()
        }];

        self.client
            .publish_diagnostics(uri.clone(), diagnostics, None)
            .await;

        let mut emitted = self.emitted.lock().await;
        emitted.insert(file_path.to_string());
    }
}

fn uri_to_path(uri: &Uri) -> Option<String> {
    let s = uri.as_str();
    s.strip_prefix("file://").map(|p| p.to_string())
}

fn make_relative(cwd: &Path, abs_path: &str) -> String {
    let abs = PathBuf::from(abs_path);
    // Try to find the git repo root for proper relative paths
    if let Ok(repo) = git2::Repository::discover(cwd)
        && let Some(workdir) = repo.workdir()
        && let Ok(rel) = abs.strip_prefix(workdir)
    {
        return rel.to_string_lossy().to_string();
    }
    // Fallback: strip cwd prefix
    abs.strip_prefix(cwd)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| abs_path.to_string())
}

impl LanguageServer for NmemLsp {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        #[allow(clippy::needless_update)]
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::NONE,
                )),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                ..Default::default()
            },
            server_info: Some(ServerInfo {
                name: "nmem".into(),
                version: Some(env!("CARGO_PKG_VERSION").into()),
            }),
            ..Default::default()
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "nmem lsp started")
            .await;
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        self.maybe_publish_diagnostics(params.text_document.uri)
            .await;
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        self.force_publish_diagnostics(params.text_document.uri)
            .await;
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = params.text_document_position_params.text_document.uri;
        let line = params.text_document_position_params.position.line as usize + 1; // LSP 0-based → blame 1-based

        let file_path = match uri_to_path(&uri) {
            Some(p) => p,
            None => return Ok(None),
        };

        let cwd = match std::env::current_dir() {
            Ok(d) => d,
            Err(_) => return Ok(None),
        };

        let relative = make_relative(&cwd, &file_path);

        // Check cache, populate on miss
        {
            let cache = self.blame_cache.lock().await;
            if cache.contains_key(&file_path) {
                if let Some(hunk) = cache[&file_path].find_hunk(line) {
                    return Ok(Some(format_hover(hunk)));
                }
                return Ok(None);
            }
        }

        // Blame is CPU-bound — run on blocking thread
        let cwd_clone = cwd.clone();
        let relative_clone = relative.clone();
        let blame_result = tokio::task::spawn_blocking(move || {
            crate::s1_git::blame_file(&cwd_clone, &relative_clone)
        }).await;

        let cached = match blame_result {
            Ok(Ok(b)) => b,
            _ => return Ok(None),
        };

        let hover = cached.find_hunk(line).map(format_hover);

        let mut cache = self.blame_cache.lock().await;
        cache.insert(file_path, cached);

        Ok(hover)
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

fn format_hover(hunk: &crate::s1_git::BlameHunk) -> Hover {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let age = if hunk.timestamp == 0 {
        "uncommitted".to_string()
    } else {
        let days = (now - hunk.timestamp) / 86400;
        if days == 0 {
            "today".to_string()
        } else if days == 1 {
            "1 day ago".to_string()
        } else if days < 30 {
            format!("{days} days ago")
        } else if days < 365 {
            format!("{} months ago", days / 30)
        } else {
            format!("{} years ago", days / 365)
        }
    };

    let mut md = format!("**{}** `{}` {}\n\n{}", hunk.author, hunk.oid, age, hunk.message);

    if !hunk.co_changed.is_empty() {
        let list: Vec<&str> = hunk.co_changed.iter().take(5).map(|s| s.as_str()).collect();
        md.push_str(&format!("\n\n**Co-changed:** {}", list.join(", ")));
    }

    Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: md,
        }),
        range: None,
    }
}

pub fn handle_lsp(_db_path: &Path) -> std::result::Result<(), NmemError> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(NmemError::Io)?;

    rt.block_on(async {
        let stdin = tokio::io::stdin();
        let stdout = tokio::io::stdout();

        let (service, socket) = LspService::new(|client| NmemLsp {
            client,
            emitted: Mutex::new(HashSet::new()),
            blame_cache: Mutex::new(HashMap::new()),
        });

        log::info!("lsp starting");
        Server::new(stdin, stdout, socket).serve(service).await;
        log::info!("lsp stopped");

        Ok(())
    })
}
