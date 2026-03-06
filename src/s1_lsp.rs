use crate::NmemError;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tokio::sync::Mutex;
use tower_lsp_server::jsonrpc::Result;
use tower_lsp_server::ls_types::*;
use tower_lsp_server::{Client, LanguageServer, LspService, Server};

#[derive(Debug)]
struct NmemLsp {
    client: Client,
    emitted: Mutex<HashSet<String>>,
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

        // Remove from emitted so we re-publish
        {
            let mut emitted = self.emitted.lock().await;
            emitted.remove(&file_path);
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
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::NONE,
                )),
                ..Default::default()
            },
            server_info: Some(ServerInfo {
                name: "nmem".into(),
                version: Some(env!("CARGO_PKG_VERSION").into()),
            }),
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

    async fn shutdown(&self) -> Result<()> {
        Ok(())
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
        });

        eprintln!("nmem: lsp starting");
        Server::new(stdin, stdout, socket).serve(service).await;
        eprintln!("nmem: lsp stopped");

        Ok(())
    })
}
