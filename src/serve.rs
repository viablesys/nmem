use crate::db::open_db_readonly;
use crate::NmemError;
use rmcp::{
    ErrorData, ServerHandler, ServiceExt,
    handler::server::{tool::ToolRouter, wrapper::Parameters},
    model::*,
    tool, tool_handler, tool_router,
    transport::stdio,
};
use rusqlite::Connection;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Arc, Mutex};

type DbHandle = Arc<Mutex<Connection>>;

#[derive(Clone)]
pub struct NmemServer {
    db: DbHandle,
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
}

// --- Parameter types ---

#[derive(Deserialize, JsonSchema)]
pub struct SearchParams {
    /// FTS5 query. Supports AND/OR/NOT, "phrase", prefix*.
    pub query: String,
    /// Filter by project name. Omit for all projects.
    #[serde(default)]
    pub project: Option<String>,
    /// Filter by observation type (file_read, file_edit, command, etc).
    #[serde(default)]
    pub obs_type: Option<String>,
    /// Max results (default 20, max 100).
    #[serde(default)]
    pub limit: Option<i64>,
    /// Pagination offset (default 0).
    #[serde(default)]
    pub offset: Option<i64>,
}

#[derive(Deserialize, JsonSchema)]
pub struct GetObservationsParams {
    /// Observation IDs to fetch. Max 50.
    pub ids: Vec<i64>,
}

#[derive(Deserialize, JsonSchema)]
pub struct TimelineParams {
    /// Observation ID to center on.
    pub anchor: i64,
    /// Observations before anchor (default 5).
    #[serde(default)]
    pub before: Option<i64>,
    /// Observations after anchor (default 5).
    #[serde(default)]
    pub after: Option<i64>,
}

#[derive(Deserialize, JsonSchema)]
pub struct RecentContextParams {
    /// Project scope. Omit for all projects.
    #[serde(default)]
    pub project: Option<String>,
    /// Max observations (default 30, max 100).
    #[serde(default)]
    pub limit: Option<i64>,
}

// --- Response types ---

#[derive(Serialize)]
struct SearchResult {
    id: i64,
    timestamp: i64,
    obs_type: String,
    content_preview: String,
    file_path: Option<String>,
    session_id: String,
}

#[derive(Serialize)]
struct FullObservation {
    id: i64,
    timestamp: i64,
    session_id: String,
    obs_type: String,
    source_event: String,
    tool_name: Option<String>,
    file_path: Option<String>,
    content: String,
    metadata: Option<serde_json::Value>,
}

#[derive(Serialize)]
struct TimelineResult {
    anchor: FullObservation,
    before: Vec<FullObservation>,
    after: Vec<FullObservation>,
}

// --- Helpers ---

fn db_err(e: &impl std::fmt::Display) -> ErrorData {
    ErrorData::new(
        ErrorCode::INTERNAL_ERROR,
        format!("db: {e}"),
        None,
    )
}

fn clamp(val: Option<i64>, default: i64, max: i64) -> i64 {
    val.unwrap_or(default).max(1).min(max)
}

fn row_to_full_obs(row: &rusqlite::Row) -> rusqlite::Result<FullObservation> {
    let metadata_str: Option<String> = row.get(8)?;
    let metadata = metadata_str.and_then(|s| serde_json::from_str(&s).ok());
    Ok(FullObservation {
        id: row.get(0)?,
        timestamp: row.get(1)?,
        session_id: row.get(2)?,
        obs_type: row.get(3)?,
        source_event: row.get(4)?,
        tool_name: row.get(5)?,
        file_path: row.get(6)?,
        content: row.get(7)?,
        metadata,
    })
}

// --- Core query logic (pub for testing) ---

impl NmemServer {
    pub fn do_search(&self, params: SearchParams) -> Result<CallToolResult, ErrorData> {
        let limit = clamp(params.limit, 20, 100);
        let offset = params.offset.unwrap_or(0).max(0);

        let db = self.db.lock().map_err(|e| db_err(&e))?;

        let mut stmt = db
            .prepare(
                "SELECT o.id, o.timestamp, o.obs_type,
                        SUBSTR(o.content, 1, 120) AS content_preview,
                        o.file_path, o.session_id
                 FROM observations o
                 JOIN sessions s ON o.session_id = s.id
                 JOIN observations_fts f ON o.id = f.rowid
                 WHERE observations_fts MATCH ?1
                   AND (?2 IS NULL OR s.project = ?2)
                   AND (?3 IS NULL OR o.obs_type = ?3)
                 ORDER BY f.rank
                 LIMIT ?4 OFFSET ?5",
            )
            .map_err(|e| db_err(&e))?;

        let results: Vec<SearchResult> = stmt
            .query_map(
                rusqlite::params![params.query, params.project, params.obs_type, limit, offset],
                |row| {
                    Ok(SearchResult {
                        id: row.get(0)?,
                        timestamp: row.get(1)?,
                        obs_type: row.get(2)?,
                        content_preview: row.get(3)?,
                        file_path: row.get(4)?,
                        session_id: row.get(5)?,
                    })
                },
            )
            .map_err(|e| {
                let msg = e.to_string();
                if msg.contains("fts5") || msg.contains("syntax") {
                    return ErrorData::new(
                        ErrorCode::INVALID_PARAMS,
                        format!("FTS5 query error: {msg}"),
                        None,
                    );
                }
                db_err(&e)
            })?
            .collect::<Result<_, _>>()
            .map_err(|e| db_err(&e))?;

        let json = serde_json::to_string(&results).map_err(|e| db_err(&e))?;
        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    pub fn do_get_observations(
        &self,
        params: GetObservationsParams,
    ) -> Result<CallToolResult, ErrorData> {
        let ids = &params.ids;

        if ids.is_empty() {
            return Ok(CallToolResult::error(vec![Content::text(
                "ids array must not be empty",
            )]));
        }
        if ids.len() > 50 {
            return Ok(CallToolResult::error(vec![Content::text(
                "ids array must not exceed 50 elements",
            )]));
        }

        let db = self.db.lock().map_err(|e| db_err(&e))?;

        let placeholders: Vec<String> = ids
            .iter()
            .enumerate()
            .map(|(i, _)| format!("?{}", i + 1))
            .collect();
        let sql = format!(
            "SELECT o.id, o.timestamp, o.session_id, o.obs_type, o.source_event,
                    o.tool_name, o.file_path, o.content, o.metadata
             FROM observations o
             WHERE o.id IN ({})
             ORDER BY CASE o.id {} END",
            placeholders.join(", "),
            ids.iter()
                .enumerate()
                .map(|(i, id)| format!("WHEN {id} THEN {i}"))
                .collect::<Vec<_>>()
                .join(" "),
        );

        let mut stmt = db.prepare(&sql).map_err(|e| db_err(&e))?;

        let sql_params: Vec<Box<dyn rusqlite::types::ToSql>> = ids
            .iter()
            .map(|id| Box::new(*id) as Box<dyn rusqlite::types::ToSql>)
            .collect();
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            sql_params.iter().map(|b| b.as_ref()).collect();

        let results: Vec<FullObservation> = stmt
            .query_map(param_refs.as_slice(), row_to_full_obs)
            .map_err(|e| db_err(&e))?
            .collect::<Result<_, _>>()
            .map_err(|e| db_err(&e))?;

        let json = serde_json::to_string(&results).map_err(|e| db_err(&e))?;
        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    pub fn do_timeline(&self, params: TimelineParams) -> Result<CallToolResult, ErrorData> {
        let before_count = clamp(params.before, 5, 50);
        let after_count = clamp(params.after, 5, 50);

        let db = self.db.lock().map_err(|e| db_err(&e))?;

        let anchor: FullObservation = db
            .query_row(
                "SELECT id, timestamp, session_id, obs_type, source_event,
                        tool_name, file_path, content, metadata
                 FROM observations WHERE id = ?1",
                rusqlite::params![params.anchor],
                row_to_full_obs,
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => ErrorData::new(
                    ErrorCode::INVALID_PARAMS,
                    "anchor observation not found",
                    None,
                ),
                other => db_err(&other),
            })?;

        let session_id = &anchor.session_id;

        let mut before_stmt = db
            .prepare(
                "SELECT id, timestamp, session_id, obs_type, source_event,
                        tool_name, file_path, content, metadata
                 FROM observations
                 WHERE session_id = ?1 AND id < ?2
                 ORDER BY id DESC
                 LIMIT ?3",
            )
            .map_err(|e| db_err(&e))?;

        let mut before: Vec<FullObservation> = before_stmt
            .query_map(
                rusqlite::params![session_id, params.anchor, before_count],
                row_to_full_obs,
            )
            .map_err(|e| db_err(&e))?
            .collect::<Result<_, _>>()
            .map_err(|e| db_err(&e))?;
        before.reverse();

        let mut after_stmt = db
            .prepare(
                "SELECT id, timestamp, session_id, obs_type, source_event,
                        tool_name, file_path, content, metadata
                 FROM observations
                 WHERE session_id = ?1 AND id > ?2
                 ORDER BY id ASC
                 LIMIT ?3",
            )
            .map_err(|e| db_err(&e))?;

        let after: Vec<FullObservation> = after_stmt
            .query_map(
                rusqlite::params![session_id, params.anchor, after_count],
                row_to_full_obs,
            )
            .map_err(|e| db_err(&e))?
            .collect::<Result<_, _>>()
            .map_err(|e| db_err(&e))?;

        let result = TimelineResult {
            anchor,
            before,
            after,
        };

        let json = serde_json::to_string(&result).map_err(|e| db_err(&e))?;
        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    pub fn do_recent_context(
        &self,
        params: RecentContextParams,
    ) -> Result<CallToolResult, ErrorData> {
        let limit = clamp(params.limit, 30, 100);

        let db = self.db.lock().map_err(|e| db_err(&e))?;

        let sql = if params.project.is_some() {
            "WITH ranked AS (
                SELECT o.*,
                       ROW_NUMBER() OVER (
                           PARTITION BY COALESCE(o.file_path, CAST(o.id AS TEXT))
                           ORDER BY o.timestamp DESC
                       ) AS rn
                FROM observations o
                JOIN sessions s ON o.session_id = s.id
                WHERE s.project = ?1
            )
            SELECT id, timestamp, session_id, obs_type, source_event,
                   tool_name, file_path, content, metadata
            FROM ranked
            WHERE rn = 1
            ORDER BY timestamp DESC
            LIMIT ?2"
        } else {
            "WITH ranked AS (
                SELECT o.*,
                       ROW_NUMBER() OVER (
                           PARTITION BY COALESCE(o.file_path, CAST(o.id AS TEXT))
                           ORDER BY o.timestamp DESC
                       ) AS rn
                FROM observations o
            )
            SELECT id, timestamp, session_id, obs_type, source_event,
                   tool_name, file_path, content, metadata
            FROM ranked
            WHERE rn = 1
            ORDER BY timestamp DESC
            LIMIT ?1"
        };

        let results: Vec<FullObservation> = if params.project.is_some() {
            let mut stmt = db.prepare(sql).map_err(|e| db_err(&e))?;
            stmt.query_map(rusqlite::params![params.project, limit], row_to_full_obs)
                .map_err(|e| db_err(&e))?
                .collect::<Result<_, _>>()
                .map_err(|e| db_err(&e))?
        } else {
            let mut stmt = db.prepare(sql).map_err(|e| db_err(&e))?;
            stmt.query_map(rusqlite::params![limit], row_to_full_obs)
                .map_err(|e| db_err(&e))?
                .collect::<Result<_, _>>()
                .map_err(|e| db_err(&e))?
        };

        let json = serde_json::to_string(&results).map_err(|e| db_err(&e))?;
        Ok(CallToolResult::success(vec![Content::text(json)]))
    }
}

// --- MCP tool wrappers (delegate to do_* methods) ---

#[tool_router]
impl NmemServer {
    pub fn new(db: DbHandle) -> Self {
        Self {
            db,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "Search observations by full-text query. Returns ranked index with IDs and previews.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn search(
        &self,
        p: Parameters<SearchParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.do_search(p.0)
    }

    #[tool(
        description = "Fetch full observation details by IDs. Returns complete observation objects.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_observations(
        &self,
        p: Parameters<GetObservationsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.do_get_observations(p.0)
    }

    #[tool(
        description = "Get observations surrounding an anchor point within the same session.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn timeline(
        &self,
        p: Parameters<TimelineParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.do_timeline(p.0)
    }

    #[tool(
        description = "Recent observations for session context. Newest first, deduped by file_path.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn recent_context(
        &self,
        p: Parameters<RecentContextParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.do_recent_context(p.0)
    }
}

#[tool_handler]
impl ServerHandler for NmemServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some("nmem: cross-session observation search and retrieval".into()),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}

pub fn handle_serve(db_path: &Path) -> Result<(), NmemError> {
    let conn = open_db_readonly(db_path)?;
    let db: DbHandle = Arc::new(Mutex::new(conn));
    let server = NmemServer::new(db);

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(NmemError::Io)?;

    rt.block_on(async {
        eprintln!("nmem: serve starting");
        let service = server
            .serve(stdio())
            .await
            .map_err(|e| NmemError::Config(format!("mcp: {e}")))?;
        service
            .waiting()
            .await
            .map_err(|e| NmemError::Config(format!("mcp: {e}")))?;
        eprintln!("nmem: serve stopped");
        Ok(())
    })
}
