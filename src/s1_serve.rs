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
    /// Ranking order: "relevance" (BM25 only, default) or "blended" (BM25 + recency + type weight).
    #[serde(default, rename = "orderBy")]
    pub order_by: Option<String>,
    /// Only include observations before this Unix timestamp.
    #[serde(default)]
    pub before: Option<i64>,
    /// Only include observations after this Unix timestamp.
    #[serde(default)]
    pub after: Option<i64>,
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
pub struct SessionSummariesParams {
    /// Filter by project name. Omit for all projects.
    #[serde(default)]
    pub project: Option<String>,
    /// Max results (default 10, max 50).
    #[serde(default)]
    pub limit: Option<i64>,
    /// Only include sessions started before this Unix timestamp.
    #[serde(default)]
    pub before: Option<i64>,
    /// Only include sessions started after this Unix timestamp.
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
    /// Only include observations before this Unix timestamp.
    #[serde(default)]
    pub before: Option<i64>,
    /// Only include observations after this Unix timestamp.
    #[serde(default)]
    pub after: Option<i64>,
}

#[derive(Deserialize, JsonSchema)]
pub struct RegenerateContextParams {
    /// Project name (required). Use the project name from session start.
    pub project: String,
    /// Only include data before this Unix timestamp. Produces "context as of time T".
    #[serde(default)]
    pub before: Option<i64>,
}

#[derive(Deserialize, JsonSchema)]
pub struct SessionTraceParams {
    /// Session ID to trace.
    pub session_id: String,
    /// Only include prompts before this Unix timestamp.
    #[serde(default)]
    pub before: Option<i64>,
    /// Only include prompts after this Unix timestamp.
    #[serde(default)]
    pub after: Option<i64>,
}

#[derive(Deserialize, JsonSchema)]
pub struct FileHistoryParams {
    /// File path to trace history for.
    pub file_path: String,
    /// Only include touches before this Unix timestamp.
    #[serde(default)]
    pub before: Option<i64>,
    /// Only include touches after this Unix timestamp.
    #[serde(default)]
    pub after: Option<i64>,
    /// Max sessions to return (default 10, max 50).
    #[serde(default)]
    pub limit: Option<i64>,
}

#[derive(Deserialize, JsonSchema)]
pub struct QueueTaskParams {
    /// The task prompt to queue for later execution.
    pub prompt: String,
    /// Project scope. Defaults to current project.
    #[serde(default)]
    pub project: Option<String>,
    /// Working directory for the task.
    #[serde(default)]
    pub cwd: Option<String>,
    /// When to run: "5m", "2h", "1d", "tomorrow", "tonight", or ISO datetime.
    pub after: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct CreateMarkerParams {
    /// The marker text (conclusion, decision, waypoint).
    pub text: String,
    /// Project scope. Defaults to current project.
    #[serde(default)]
    pub project: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct CurrentStanceParams {
    /// Optional session ID. Defaults to the most recent session.
    #[serde(default)]
    pub session_id: Option<String>,
    /// EMA alpha for smoothing (default 0.08). Lower = smoother.
    #[serde(default)]
    pub alpha: Option<f64>,
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
    is_pinned: bool,
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
    is_pinned: bool,
}

#[derive(Serialize)]
struct TimelineResult {
    anchor: FullObservation,
    before: Vec<FullObservation>,
    after: Vec<FullObservation>,
}

#[derive(Serialize)]
struct SessionSummaryResult {
    session_id: String,
    project: String,
    started_at: i64,
    summary: serde_json::Value,
}

#[derive(Serialize)]
struct SessionTraceResult {
    session_id: String,
    project: String,
    started_at: i64,
    ended_at: Option<i64>,
    summary: Option<serde_json::Value>,
    prompts: Vec<PromptTrace>,
}

#[derive(Serialize)]
struct PromptTrace {
    prompt_id: Option<i64>,
    timestamp: i64,
    source: String,
    content: Option<String>,
    observation_count: usize,
    observations: Vec<ObservationSummary>,
}

#[derive(Serialize)]
struct ObservationSummary {
    id: i64,
    timestamp: i64,
    obs_type: String,
    file_path: Option<String>,
    content_preview: String,
    is_pinned: bool,
}

#[derive(Serialize)]
struct FileHistoryResult {
    file_path: String,
    sessions: Vec<FileSessionEntry>,
}

#[derive(Serialize)]
struct FileSessionEntry {
    session_id: String,
    project: String,
    started_at: i64,
    summary_intent: Option<String>,
    touches: Vec<FileTouch>,
}

#[derive(Serialize)]
struct FileTouch {
    observation_id: i64,
    timestamp: i64,
    obs_type: String,
    content_preview: String,
    prompt_content: Option<String>,
    is_pinned: bool,
}

#[derive(Serialize)]
struct QuadrantCounts {
    think_diverge: QuadrantEntry,
    think_converge: QuadrantEntry,
    act_diverge: QuadrantEntry,
    act_converge: QuadrantEntry,
}

#[derive(Serialize)]
struct QuadrantEntry {
    count: i64,
    pct: f64,
}

#[derive(Serialize)]
struct CurrentSignal {
    phase: f64,
    scope: f64,
    stance: String,
}

#[derive(Serialize)]
struct TrendSignal {
    phase_5: f64,
    phase_20: f64,
    scope_5: f64,
    scope_20: f64,
    phase_direction: String,
    scope_direction: String,
}

#[derive(Serialize)]
struct RecentShift {
    at_observation: i64,
    from: String,
    to: String,
    minutes_ago: f64,
}

#[derive(Serialize)]
struct DimensionCounts {
    #[serde(flatten)]
    counts: std::collections::HashMap<String, i64>,
}

#[derive(Serialize)]
struct NoveltyFriction {
    routine_smooth: i64,
    routine_friction: i64,
    novel_smooth: i64,
    novel_friction: i64,
}

#[derive(Serialize)]
struct StanceResult {
    session_id: String,
    observation_count: i64,
    quadrants: QuadrantCounts,
    current: CurrentSignal,
    trend: TrendSignal,
    recent_shifts: Vec<RecentShift>,
    #[serde(skip_serializing_if = "Option::is_none")]
    locus: Option<DimensionCounts>,
    #[serde(skip_serializing_if = "Option::is_none")]
    novelty: Option<DimensionCounts>,
    #[serde(skip_serializing_if = "Option::is_none")]
    friction: Option<DimensionCounts>,
    #[serde(skip_serializing_if = "Option::is_none")]
    novelty_friction: Option<NoveltyFriction>,
    guidance: String,
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

fn record_query_metrics(tool: &str, start: std::time::Instant) {
    let meter = opentelemetry::global::meter("nmem");
    meter
        .u64_counter("nmem_queries_total")
        .build()
        .add(1, &[opentelemetry::KeyValue::new("tool", tool.to_string())]);
    meter
        .f64_histogram("nmem_query_duration_seconds")
        .build()
        .record(
            start.elapsed().as_secs_f64(),
            &[opentelemetry::KeyValue::new("tool", tool.to_string())],
        );
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
        is_pinned: row.get::<_, i64>(9)? != 0,
    })
}

#[derive(Serialize)]
struct ScoredObservation {
    id: i64,
    timestamp: i64,
    session_id: String,
    obs_type: String,
    source_event: String,
    tool_name: Option<String>,
    file_path: Option<String>,
    content: String,
    metadata: Option<serde_json::Value>,
    is_pinned: bool,
    score: f64,
}

fn row_to_scored_obs(row: &rusqlite::Row) -> rusqlite::Result<ScoredObservation> {
    let metadata_str: Option<String> = row.get(8)?;
    let metadata = metadata_str.and_then(|s| serde_json::from_str(&s).ok());
    Ok(ScoredObservation {
        id: row.get(0)?,
        timestamp: row.get(1)?,
        session_id: row.get(2)?,
        obs_type: row.get(3)?,
        source_event: row.get(4)?,
        tool_name: row.get(5)?,
        file_path: row.get(6)?,
        content: row.get(7)?,
        metadata,
        is_pinned: row.get::<_, i64>(9)? != 0,
        score: row.get(10)?,
    })
}

// --- Core query logic (pub for testing) ---

impl NmemServer {
    pub fn do_search(&self, params: SearchParams) -> Result<CallToolResult, ErrorData> {
        let limit = clamp(params.limit, 20, 100);
        let offset = params.offset.unwrap_or(0).max(0);

        let blended = match params.order_by.as_deref() {
            None | Some("relevance") => false,
            Some("blended") => true,
            Some(other) => {
                return Err(ErrorData::new(
                    ErrorCode::INVALID_PARAMS,
                    format!("invalid orderBy: {other:?} (expected \"relevance\" or \"blended\")"),
                    None,
                ));
            }
        };

        let db = self.db.lock().map_err(|e| db_err(&e))?;

        let sql = if blended {
            "WITH fts_matches AS (
                SELECT o.id, o.timestamp, o.obs_type,
                       SUBSTR(o.content, 1, 120) AS content_preview,
                       o.file_path, o.session_id, o.is_pinned,
                       f.rank AS raw_rank
                FROM observations o
                JOIN sessions s ON o.session_id = s.id
                JOIN observations_fts f ON o.id = f.rowid
                WHERE observations_fts MATCH ?1
                  AND (?2 IS NULL OR s.project = ?2)
                  AND (?3 IS NULL OR o.obs_type = ?3)
                  AND (?4 IS NULL OR o.timestamp < ?4)
                  AND (?5 IS NULL OR o.timestamp > ?5)
            ),
            rank_bounds AS (
                SELECT MIN(raw_rank) AS min_r, MAX(raw_rank) AS max_r FROM fts_matches
            ),
            scored AS (
                SELECT m.*,
                       CASE WHEN b.max_r = b.min_r THEN 1.0
                            ELSE (m.raw_rank - b.max_r) / (b.min_r - b.max_r)
                       END AS bm25_norm,
                       exp_decay((unixepoch('now') - m.timestamp) / 86400.0, 7.0) AS recency,
                       CASE m.obs_type
                           WHEN 'file_edit' THEN 1.0 WHEN 'command' THEN 0.67
                           WHEN 'session_compact' THEN 0.5 WHEN 'mcp_call' THEN 0.33
                           ELSE 0.17
                       END AS type_w
                FROM fts_matches m, rank_bounds b
            )
            SELECT id, timestamp, obs_type, content_preview, file_path, session_id, is_pinned
            FROM scored
            ORDER BY (bm25_norm * 0.5 + recency * 0.3 + type_w * 0.2) DESC
            LIMIT ?6 OFFSET ?7"
        } else {
            "SELECT o.id, o.timestamp, o.obs_type,
                    SUBSTR(o.content, 1, 120) AS content_preview,
                    o.file_path, o.session_id, o.is_pinned
             FROM observations o
             JOIN sessions s ON o.session_id = s.id
             JOIN observations_fts f ON o.id = f.rowid
             WHERE observations_fts MATCH ?1
               AND (?2 IS NULL OR s.project = ?2)
               AND (?3 IS NULL OR o.obs_type = ?3)
               AND (?4 IS NULL OR o.timestamp < ?4)
               AND (?5 IS NULL OR o.timestamp > ?5)
             ORDER BY f.rank
             LIMIT ?6 OFFSET ?7"
        };

        let mut stmt = db.prepare(sql).map_err(|e| db_err(&e))?;

        let results: Vec<SearchResult> = stmt
            .query_map(
                rusqlite::params![params.query, params.project, params.obs_type, params.before, params.after, limit, offset],
                |row| {
                    Ok(SearchResult {
                        id: row.get(0)?,
                        timestamp: row.get(1)?,
                        obs_type: row.get(2)?,
                        content_preview: row.get(3)?,
                        file_path: row.get(4)?,
                        session_id: row.get(5)?,
                        is_pinned: row.get::<_, i64>(6)? != 0,
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
                    o.tool_name, o.file_path, o.content, o.metadata, o.is_pinned
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
                        tool_name, file_path, content, metadata, is_pinned
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
                        tool_name, file_path, content, metadata, is_pinned
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
                        tool_name, file_path, content, metadata, is_pinned
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

        let results: Vec<ScoredObservation> = if params.project.is_some() {
            let sql = "WITH scored AS (
                SELECT o.id, o.timestamp, o.session_id, o.obs_type, o.source_event,
                       o.tool_name, o.file_path, o.content, o.metadata, o.is_pinned,
                       exp_decay(
                           (unixepoch('now') - o.timestamp) / 86400.0, 7.0
                       ) AS recency,
                       CASE o.obs_type
                           WHEN 'file_edit' THEN 1.0 WHEN 'command' THEN 0.67
                           WHEN 'session_compact' THEN 0.5 WHEN 'mcp_call' THEN 0.33
                           ELSE 0.17
                       END AS type_w,
                       CASE WHEN s.project = ?1 THEN 1.0 ELSE 0.3 END AS proj_w
                FROM observations o
                JOIN sessions s ON o.session_id = s.id
                WHERE (?2 IS NULL OR o.timestamp < ?2)
                  AND (?3 IS NULL OR o.timestamp > ?3)
            ),
            ranked AS (
                SELECT *,
                       (recency * 0.5 + type_w * 0.3 + proj_w * 0.2) AS score,
                       ROW_NUMBER() OVER (
                           PARTITION BY COALESCE(file_path, CAST(id AS TEXT))
                           ORDER BY (recency * 0.5 + type_w * 0.3 + proj_w * 0.2) DESC
                       ) AS rn
                FROM scored
            )
            SELECT id, timestamp, session_id, obs_type, source_event,
                   tool_name, file_path, content, metadata, is_pinned, score
            FROM ranked WHERE rn = 1
            ORDER BY score DESC
            LIMIT ?4";

            let mut stmt = db.prepare(sql).map_err(|e| db_err(&e))?;
            stmt.query_map(
                rusqlite::params![params.project, params.before, params.after, limit],
                row_to_scored_obs,
            )
            .map_err(|e| db_err(&e))?
            .collect::<Result<_, _>>()
            .map_err(|e| db_err(&e))?
        } else {
            let sql = "WITH scored AS (
                SELECT o.id, o.timestamp, o.session_id, o.obs_type, o.source_event,
                       o.tool_name, o.file_path, o.content, o.metadata, o.is_pinned,
                       exp_decay(
                           (unixepoch('now') - o.timestamp) / 86400.0, 7.0
                       ) AS recency,
                       CASE o.obs_type
                           WHEN 'file_edit' THEN 1.0 WHEN 'command' THEN 0.67
                           WHEN 'session_compact' THEN 0.5 WHEN 'mcp_call' THEN 0.33
                           ELSE 0.17
                       END AS type_w
                FROM observations o
                WHERE (?1 IS NULL OR o.timestamp < ?1)
                  AND (?2 IS NULL OR o.timestamp > ?2)
            ),
            ranked AS (
                SELECT *,
                       (recency * 0.6 + type_w * 0.4) AS score,
                       ROW_NUMBER() OVER (
                           PARTITION BY COALESCE(file_path, CAST(id AS TEXT))
                           ORDER BY (recency * 0.6 + type_w * 0.4) DESC
                       ) AS rn
                FROM scored
            )
            SELECT id, timestamp, session_id, obs_type, source_event,
                   tool_name, file_path, content, metadata, is_pinned, score
            FROM ranked WHERE rn = 1
            ORDER BY score DESC
            LIMIT ?3";

            let mut stmt = db.prepare(sql).map_err(|e| db_err(&e))?;
            stmt.query_map(rusqlite::params![params.before, params.after, limit], row_to_scored_obs)
                .map_err(|e| db_err(&e))?
                .collect::<Result<_, _>>()
                .map_err(|e| db_err(&e))?
        };

        let json = serde_json::to_string(&results).map_err(|e| db_err(&e))?;
        Ok(CallToolResult::success(vec![Content::text(json)]))
    }
    pub fn do_regenerate_context(
        &self,
        params: RegenerateContextParams,
    ) -> Result<CallToolResult, ErrorData> {
        let db = self.db.lock().map_err(|e| db_err(&e))?;
        let config = crate::s5_config::load_config().unwrap_or_default();
        let (local_limit, cross_limit) =
            crate::s5_config::resolve_context_limits(&config, &params.project, false);
        let ctx = crate::s4_context::generate_context(&db, &params.project, local_limit, cross_limit, params.before)
            .map_err(|e| db_err(&e))?;
        if ctx.is_empty() {
            Ok(CallToolResult::success(vec![Content::text(format!(
                "No context available for project \"{}\".",
                params.project
            ))]))
        } else {
            Ok(CallToolResult::success(vec![Content::text(ctx)]))
        }
    }

    pub fn do_session_summaries(
        &self,
        params: SessionSummariesParams,
    ) -> Result<CallToolResult, ErrorData> {
        let limit = clamp(params.limit, 10, 50);
        let db = self.db.lock().map_err(|e| db_err(&e))?;

        let sql = "SELECT id, project, started_at, summary FROM sessions
                   WHERE summary IS NOT NULL
                     AND (?1 IS NULL OR project = ?1)
                     AND (?2 IS NULL OR started_at < ?2)
                     AND (?3 IS NULL OR started_at > ?3)
                   ORDER BY started_at DESC LIMIT ?4";
        let sql_params: Vec<Box<dyn rusqlite::types::ToSql>> = vec![
            Box::new(params.project.clone()) as Box<dyn rusqlite::types::ToSql>,
            Box::new(params.before),
            Box::new(params.after),
            Box::new(limit),
        ];

        let mut stmt = db.prepare(sql).map_err(|e| db_err(&e))?;
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            sql_params.iter().map(|b| b.as_ref()).collect();

        let results: Vec<SessionSummaryResult> = stmt
            .query_map(param_refs.as_slice(), |row| {
                let summary_str: String = row.get(3)?;
                let summary: serde_json::Value =
                    serde_json::from_str(&summary_str).unwrap_or(serde_json::Value::Null);
                Ok(SessionSummaryResult {
                    session_id: row.get(0)?,
                    project: row.get(1)?,
                    started_at: row.get(2)?,
                    summary,
                })
            })
            .map_err(|e| db_err(&e))?
            .collect::<Result<_, _>>()
            .map_err(|e| db_err(&e))?;

        let json = serde_json::to_string(&results).map_err(|e| db_err(&e))?;
        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    pub fn do_session_trace(
        &self,
        params: SessionTraceParams,
    ) -> Result<CallToolResult, ErrorData> {
        let db = self.db.lock().map_err(|e| db_err(&e))?;

        // 1. Session metadata
        let session: (String, String, i64, Option<i64>, Option<String>) = db
            .query_row(
                "SELECT id, project, started_at, ended_at, summary FROM sessions WHERE id = ?1",
                rusqlite::params![params.session_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => ErrorData::new(
                    ErrorCode::INVALID_PARAMS,
                    format!("session not found: {}", params.session_id),
                    None,
                ),
                other => db_err(&other),
            })?;

        let summary: Option<serde_json::Value> =
            session.4.as_deref().and_then(|s| serde_json::from_str(s).ok());

        // 2. Prompts + observations via LEFT JOIN, plus orphan observations (NULL prompt_id)
        let sql = "SELECT p.id AS prompt_id, p.timestamp AS prompt_ts, p.source, p.content AS prompt_content,
                          o.id AS obs_id, o.timestamp AS obs_ts, o.obs_type, o.file_path,
                          SUBSTR(o.content, 1, 120) AS obs_preview, o.is_pinned
                   FROM prompts p
                   LEFT JOIN observations o ON o.prompt_id = p.id
                     AND (?2 IS NULL OR o.timestamp < ?2)
                     AND (?3 IS NULL OR o.timestamp > ?3)
                   WHERE p.session_id = ?1
                     AND (?2 IS NULL OR p.timestamp < ?2)
                     AND (?3 IS NULL OR p.timestamp > ?3)
                   UNION ALL
                   SELECT NULL, o.timestamp, 'system', NULL,
                          o.id, o.timestamp, o.obs_type, o.file_path,
                          SUBSTR(o.content, 1, 120), o.is_pinned
                   FROM observations o
                   WHERE o.session_id = ?1 AND o.prompt_id IS NULL
                     AND (?2 IS NULL OR o.timestamp < ?2)
                     AND (?3 IS NULL OR o.timestamp > ?3)
                   ORDER BY prompt_ts ASC, obs_ts ASC";

        let mut stmt = db.prepare(sql).map_err(|e| db_err(&e))?;

        // Group rows into PromptTrace structs keyed by prompt_id (or None for system)
        let mut prompts: Vec<PromptTrace> = Vec::new();
        // Track the index of the current prompt being accumulated
        let mut current_key: Option<Option<i64>> = None;

        let rows = stmt
            .query_map(
                rusqlite::params![params.session_id, params.before, params.after],
                |row| {
                    let prompt_id: Option<i64> = row.get(0)?;
                    let prompt_ts: i64 = row.get(1)?;
                    let source: String = row.get(2)?;
                    let prompt_content: Option<String> = row.get(3)?;
                    let obs_id: Option<i64> = row.get(4)?;
                    let obs_ts: Option<i64> = row.get(5)?;
                    let obs_type: Option<String> = row.get(6)?;
                    let file_path: Option<String> = row.get(7)?;
                    let obs_preview: Option<String> = row.get(8)?;
                    let is_pinned: Option<i64> = row.get(9)?;
                    Ok((
                        prompt_id,
                        prompt_ts,
                        source,
                        prompt_content,
                        obs_id,
                        obs_ts,
                        obs_type,
                        file_path,
                        obs_preview,
                        is_pinned,
                    ))
                },
            )
            .map_err(|e| db_err(&e))?;

        for row_result in rows {
            let (
                prompt_id,
                prompt_ts,
                source,
                prompt_content,
                obs_id,
                obs_ts,
                obs_type,
                file_path,
                obs_preview,
                is_pinned,
            ) = row_result.map_err(|e| db_err(&e))?;

            let key = Some(prompt_id);
            if current_key != key {
                prompts.push(PromptTrace {
                    prompt_id,
                    timestamp: prompt_ts,
                    source,
                    content: prompt_content,
                    observation_count: 0,
                    observations: Vec::new(),
                });
                current_key = key;
            }

            if let (Some(oid), Some(ots), Some(otype), Some(preview)) =
                (obs_id, obs_ts, obs_type, obs_preview)
            {
                let prompt = prompts.last_mut().unwrap();
                prompt.observations.push(ObservationSummary {
                    id: oid,
                    timestamp: ots,
                    obs_type: otype,
                    file_path,
                    content_preview: preview,
                    is_pinned: is_pinned.unwrap_or(0) != 0,
                });
            }
        }

        // Fill observation_count
        for p in &mut prompts {
            p.observation_count = p.observations.len();
        }

        let result = SessionTraceResult {
            session_id: session.0,
            project: session.1,
            started_at: session.2,
            ended_at: session.3,
            summary,
            prompts,
        };

        let json = serde_json::to_string(&result).map_err(|e| db_err(&e))?;
        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    pub fn do_file_history(
        &self,
        params: FileHistoryParams,
    ) -> Result<CallToolResult, ErrorData> {
        let limit = clamp(params.limit, 10, 50);
        let db = self.db.lock().map_err(|e| db_err(&e))?;

        let sql = "SELECT o.id AS obs_id, o.timestamp, o.obs_type,
                          SUBSTR(o.content, 1, 120) AS content_preview,
                          o.is_pinned, o.session_id,
                          s.project, s.started_at, s.summary,
                          p.content AS prompt_content
                   FROM observations o
                   JOIN sessions s ON o.session_id = s.id
                   LEFT JOIN prompts p ON o.prompt_id = p.id AND p.source = 'user'
                   WHERE o.file_path = ?1
                     AND (?2 IS NULL OR o.timestamp < ?2)
                     AND (?3 IS NULL OR o.timestamp > ?3)
                   ORDER BY o.timestamp DESC
                   LIMIT ?4";

        let mut stmt = db.prepare(sql).map_err(|e| db_err(&e))?;

        struct RawTouch {
            obs_id: i64,
            timestamp: i64,
            obs_type: String,
            content_preview: String,
            is_pinned: bool,
            session_id: String,
            project: String,
            started_at: i64,
            summary_json: Option<String>,
            prompt_content: Option<String>,
        }

        let touches: Vec<RawTouch> = stmt
            .query_map(
                rusqlite::params![params.file_path, params.before, params.after, limit],
                |row| {
                    Ok(RawTouch {
                        obs_id: row.get(0)?,
                        timestamp: row.get(1)?,
                        obs_type: row.get(2)?,
                        content_preview: row.get(3)?,
                        is_pinned: row.get::<_, i64>(4)? != 0,
                        session_id: row.get(5)?,
                        project: row.get(6)?,
                        started_at: row.get(7)?,
                        summary_json: row.get(8)?,
                        prompt_content: row.get(9)?,
                    })
                },
            )
            .map_err(|e| db_err(&e))?
            .collect::<Result<_, _>>()
            .map_err(|e| db_err(&e))?;

        // Group by session_id, preserving encounter order
        let mut sessions: Vec<FileSessionEntry> = Vec::new();
        let mut session_index: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();

        for t in touches {
            let idx = if let Some(&i) = session_index.get(&t.session_id) {
                i
            } else {
                let intent = t.summary_json.as_deref().and_then(|s| {
                    serde_json::from_str::<serde_json::Value>(s)
                        .ok()
                        .and_then(|v| v.get("intent")?.as_str().map(String::from))
                });
                let i = sessions.len();
                sessions.push(FileSessionEntry {
                    session_id: t.session_id.clone(),
                    project: t.project,
                    started_at: t.started_at,
                    summary_intent: intent,
                    touches: Vec::new(),
                });
                session_index.insert(t.session_id, i);
                i
            };

            sessions[idx].touches.push(FileTouch {
                observation_id: t.obs_id,
                timestamp: t.timestamp,
                obs_type: t.obs_type,
                content_preview: t.content_preview,
                prompt_content: t.prompt_content,
                is_pinned: t.is_pinned,
            });
        }

        let result = FileHistoryResult {
            file_path: params.file_path,
            sessions,
        };

        let json = serde_json::to_string(&result).map_err(|e| db_err(&e))?;
        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    pub fn do_queue_task(&self, params: QueueTaskParams) -> Result<CallToolResult, ErrorData> {
        // Shell out to `nmem queue` to keep MCP server read-only.
        // Same pattern as hooks calling `nmem record`.
        let nmem_bin = std::env::current_exe().unwrap_or_else(|_| "nmem".into());

        let mut cmd = std::process::Command::new(&nmem_bin);
        cmd.arg("queue").arg(&params.prompt);

        if let Some(ref project) = params.project {
            cmd.arg("--project").arg(project);
        }
        if let Some(ref cwd) = params.cwd {
            cmd.arg("--cwd").arg(cwd);
        }
        cmd.arg("--after").arg(&params.after);

        let output = cmd.output().map_err(|e| {
            ErrorData::new(
                ErrorCode::INTERNAL_ERROR,
                format!("failed to run nmem queue: {e}"),
                None,
            )
        })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ErrorData::new(
                ErrorCode::INTERNAL_ERROR,
                format!("nmem queue failed: {stderr}"),
                None,
            ));
        }

        let task_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let response = serde_json::json!({
            "task_id": task_id.parse::<i64>().unwrap_or(0),
            "status": "pending",
            "prompt": params.prompt,
        });

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string(&response).map_err(|e| db_err(&e))?,
        )]))
    }
    pub fn do_create_marker(&self, params: CreateMarkerParams) -> Result<CallToolResult, ErrorData> {
        let nmem_bin = std::env::current_exe().unwrap_or_else(|_| "nmem".into());

        let mut cmd = std::process::Command::new(&nmem_bin);
        cmd.arg("mark").arg(&params.text);

        if let Some(ref project) = params.project {
            cmd.arg("--project").arg(project);
        }

        let output = cmd.output().map_err(|e| {
            ErrorData::new(
                ErrorCode::INTERNAL_ERROR,
                format!("failed to run nmem mark: {e}"),
                None,
            )
        })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ErrorData::new(
                ErrorCode::INTERNAL_ERROR,
                format!("nmem mark failed: {stderr}"),
                None,
            ));
        }

        let obs_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let response = serde_json::json!({
            "observation_id": obs_id.parse::<i64>().unwrap_or(0),
            "status": "created",
            "text": params.text,
        });

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string(&response).map_err(|e| db_err(&e))?,
        )]))
    }

    pub fn do_current_stance(
        &self,
        params: CurrentStanceParams,
    ) -> Result<CallToolResult, ErrorData> {
        let alpha = params.alpha.unwrap_or(0.08).clamp(0.01, 1.0);
        let db = self.db.lock().map_err(|e| db_err(&e))?;

        // 1. Resolve session
        let session_id: String = if let Some(sid) = params.session_id {
            sid
        } else {
            db.query_row(
                "SELECT id FROM sessions ORDER BY started_at DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => ErrorData::new(
                    ErrorCode::INVALID_PARAMS,
                    "no sessions found",
                    None,
                ),
                other => db_err(&other),
            })?
        };

        // 2. Quadrant counts
        let mut quad_stmt = db
            .prepare(
                "SELECT phase, scope, COUNT(*) FROM observations
                 WHERE session_id = ?1 AND phase IS NOT NULL AND scope IS NOT NULL
                 GROUP BY phase, scope",
            )
            .map_err(|e| db_err(&e))?;

        let mut td: i64 = 0;
        let mut tc: i64 = 0;
        let mut ad: i64 = 0;
        let mut ac: i64 = 0;

        let mut rows = quad_stmt
            .query(rusqlite::params![session_id])
            .map_err(|e| db_err(&e))?;
        while let Some(row) = rows.next().map_err(|e| db_err(&e))? {
            let phase: String = row.get(0).map_err(|e| db_err(&e))?;
            let scope: String = row.get(1).map_err(|e| db_err(&e))?;
            let count: i64 = row.get(2).map_err(|e| db_err(&e))?;
            match (phase.as_str(), scope.as_str()) {
                ("think", "diverge") => td = count,
                ("think", "converge") => tc = count,
                ("act", "diverge") => ad = count,
                ("act", "converge") => ac = count,
                _ => {}
            }
        }
        drop(rows);

        let total = td + tc + ad + ac;
        if total == 0 {
            return Ok(CallToolResult::success(vec![Content::text(
                serde_json::json!({
                    "session_id": session_id,
                    "observation_count": 0,
                    "guidance": "No classified observations yet. Stance analysis requires phase and scope labels."
                })
                .to_string(),
            )]));
        }

        let pct = |n: i64| (n as f64 / total as f64 * 100.0 * 10.0).round() / 10.0;

        // 3. Full sequence for EMA
        let mut seq_stmt = db
            .prepare(
                "SELECT phase, scope, timestamp FROM observations
                 WHERE session_id = ?1 AND phase IS NOT NULL AND scope IS NOT NULL
                 ORDER BY timestamp ASC",
            )
            .map_err(|e| db_err(&e))?;

        struct ObsPoint {
            phase_val: f64,  // -1 = think, +1 = act
            scope_val: f64,  // -1 = diverge, +1 = converge
            timestamp: i64,
        }

        let points: Vec<ObsPoint> = seq_stmt
            .query_map(rusqlite::params![session_id], |row| {
                let phase: String = row.get(0)?;
                let scope: String = row.get(1)?;
                let timestamp: i64 = row.get(2)?;
                Ok(ObsPoint {
                    phase_val: if phase == "act" { 1.0 } else { -1.0 },
                    scope_val: if scope == "converge" { 1.0 } else { -1.0 },
                    timestamp,
                })
            })
            .map_err(|e| db_err(&e))?
            .collect::<Result<_, _>>()
            .map_err(|e| db_err(&e))?;

        // 4. Compute EMA over full sequence
        let mut ema_phase = points[0].phase_val;
        let mut ema_scope = points[0].scope_val;
        // Store EMA values for shift detection
        let mut ema_history: Vec<(f64, f64, i64)> = Vec::with_capacity(points.len());
        ema_history.push((ema_phase, ema_scope, points[0].timestamp));

        for p in &points[1..] {
            ema_phase = alpha * p.phase_val + (1.0 - alpha) * ema_phase;
            ema_scope = alpha * p.scope_val + (1.0 - alpha) * ema_scope;
            ema_history.push((ema_phase, ema_scope, p.timestamp));
        }

        // 5. Short/medium window averages
        let n = points.len();
        let avg = |vals: &[ObsPoint], count: usize, f: fn(&ObsPoint) -> f64| -> f64 {
            let start = if vals.len() > count { vals.len() - count } else { 0 };
            let slice = &vals[start..];
            if slice.is_empty() {
                return 0.0;
            }
            slice.iter().map(&f).sum::<f64>() / slice.len() as f64
        };

        let phase_5 = avg(&points, 5, |p| p.phase_val);
        let phase_20 = avg(&points, 20, |p| p.phase_val);
        let scope_5 = avg(&points, 5, |p| p.scope_val);
        let scope_20 = avg(&points, 20, |p| p.scope_val);

        let direction = |short: f64, medium: f64, pos_label: &str, neg_label: &str| -> String {
            let diff = short - medium;
            if diff.abs() < 0.2 {
                "stable".to_string()
            } else if diff > 0.0 {
                format!("shifting_{pos_label}")
            } else {
                format!("shifting_{neg_label}")
            }
        };

        let phase_direction = direction(phase_5, phase_20, "act", "think");
        let scope_direction = direction(scope_5, scope_20, "converge", "diverge");

        // Stance label from current EMA
        let stance = match (ema_phase >= 0.0, ema_scope >= 0.0) {
            (true, true) => "act+converge",
            (true, false) => "act+diverge",
            (false, true) => "think+converge",
            (false, false) => "think+diverge",
        };

        // 6. Detect recent scope zero-crossings (last 50 observations)
        let now_ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        let mut recent_shifts: Vec<RecentShift> = Vec::new();
        let scan_start = n.saturating_sub(50);
        for i in (scan_start + 1)..n {
            let prev_scope = ema_history[i - 1].1;
            let curr_scope = ema_history[i].1;
            // Zero crossing in scope dimension
            if (prev_scope >= 0.0) != (curr_scope >= 0.0) {
                let prev_phase = ema_history[i - 1].0;
                let curr_phase = ema_history[i].0;
                let from_stance = match (prev_phase >= 0.0, prev_scope >= 0.0) {
                    (true, true) => "act+converge",
                    (true, false) => "act+diverge",
                    (false, true) => "think+converge",
                    (false, false) => "think+diverge",
                };
                let to_stance = match (curr_phase >= 0.0, curr_scope >= 0.0) {
                    (true, true) => "act+converge",
                    (true, false) => "act+diverge",
                    (false, true) => "think+converge",
                    (false, false) => "think+diverge",
                };
                let minutes_ago =
                    ((now_ts - ema_history[i].2) as f64 / 60.0 * 10.0).round() / 10.0;
                recent_shifts.push(RecentShift {
                    at_observation: i as i64,
                    from: from_stance.to_string(),
                    to: to_stance.to_string(),
                    minutes_ago,
                });
            }
        }
        // Keep only last 5 shifts
        if recent_shifts.len() > 5 {
            let drain_to = recent_shifts.len() - 5;
            recent_shifts.drain(..drain_to);
        }

        // 7. Aggregate new dimensions (locus, novelty, friction)
        let (locus_counts, novelty_counts, friction_counts, nf_cross) = {
            let mut locus_map: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
            let mut novelty_map: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
            let mut friction_map: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
            let mut nf = NoveltyFriction {
                routine_smooth: 0,
                routine_friction: 0,
                novel_smooth: 0,
                novel_friction: 0,
            };

            let mut dim_stmt = db
                .prepare(
                    "SELECT locus, novelty, friction, COUNT(*) FROM observations
                     WHERE session_id = ?1
                     GROUP BY locus, novelty, friction",
                )
                .map_err(|e| db_err(&e))?;

            let mut dim_rows = dim_stmt
                .query(rusqlite::params![session_id])
                .map_err(|e| db_err(&e))?;
            while let Some(row) = dim_rows.next().map_err(|e| db_err(&e))? {
                let locus: Option<String> = row.get(0).map_err(|e| db_err(&e))?;
                let novelty: Option<String> = row.get(1).map_err(|e| db_err(&e))?;
                let friction: Option<String> = row.get(2).map_err(|e| db_err(&e))?;
                let count: i64 = row.get(3).map_err(|e| db_err(&e))?;

                if let Some(l) = &locus {
                    *locus_map.entry(l.clone()).or_insert(0) += count;
                }
                if let Some(n) = &novelty {
                    *novelty_map.entry(n.clone()).or_insert(0) += count;
                }
                if let Some(f) = &friction {
                    *friction_map.entry(f.clone()).or_insert(0) += count;
                }

                // Cross-tabulate novelty × friction
                if let (Some(n), Some(f)) = (&novelty, &friction) {
                    match (n.as_str(), f.as_str()) {
                        ("routine", "smooth") => nf.routine_smooth += count,
                        ("routine", "friction") => nf.routine_friction += count,
                        ("novel", "smooth") => nf.novel_smooth += count,
                        ("novel", "friction") => nf.novel_friction += count,
                        _ => {}
                    }
                }
            }
            drop(dim_rows);

            let has_locus = !locus_map.is_empty();
            let has_novelty = !novelty_map.is_empty();
            let has_friction = !friction_map.is_empty();
            let has_nf = has_novelty && has_friction;

            // Add percentage fields
            fn add_pct(map: &mut std::collections::HashMap<String, i64>, key: &str) {
                let total: i64 = map.values().sum();
                if total > 0
                    && let Some(&val) = map.get(key)
                {
                    let pct = (val as f64 / total as f64 * 1000.0).round() as i64;
                    map.insert(format!("pct_{key}"), pct);
                }
            }

            if has_locus {
                add_pct(&mut locus_map, "internal");
            }
            if has_novelty {
                add_pct(&mut novelty_map, "novel");
            }
            if has_friction {
                add_pct(&mut friction_map, "friction");
            }

            (
                if has_locus { Some(DimensionCounts { counts: locus_map }) } else { None },
                if has_novelty { Some(DimensionCounts { counts: novelty_map }) } else { None },
                if has_friction { Some(DimensionCounts { counts: friction_map }) } else { None },
                if has_nf { Some(nf) } else { None },
            )
        };

        // 8. Compute guidance
        let guidance = if scope_5 < 0.0 && scope_20 > 0.0 {
            "Scope shifting toward diverge — new work unit may be starting. Check `session_summaries` for prior `next_steps`. Run `file_history` on new files before editing.".to_string()
        } else if ema_phase < -0.5 {
            "Deep investigation phase. Search nmem for prior conclusions before re-deriving — `search` for the topic or `session_summaries` for `learned` entries.".to_string()
        } else if ema_phase > 0.5
            && ema_scope > 0.5
            && phase_direction == "stable"
            && scope_direction == "stable"
        {
            "Focused implementation run. Stance is stable — no retrieval action needed unless you encounter an unfamiliar file.".to_string()
        } else if (scope_5 > 0.0) != (scope_20 > 0.0) {
            if scope_5 < 0.0 {
                "Scope reversal detected (entering diverge). Check prior `next_steps` via `session_summaries`. Run `file_history` on new files.".to_string()
            } else {
                "Scope reversal detected (entering converge). Verify approach wasn't previously abandoned — `search` for the pattern name.".to_string()
            }
        } else {
            "Mixed stance. No strong retrieval signal — use judgment.".to_string()
        };

        let result = StanceResult {
            session_id,
            observation_count: total,
            quadrants: QuadrantCounts {
                think_diverge: QuadrantEntry { count: td, pct: pct(td) },
                think_converge: QuadrantEntry { count: tc, pct: pct(tc) },
                act_diverge: QuadrantEntry { count: ad, pct: pct(ad) },
                act_converge: QuadrantEntry { count: ac, pct: pct(ac) },
            },
            current: CurrentSignal {
                phase: (ema_phase * 100.0).round() / 100.0,
                scope: (ema_scope * 100.0).round() / 100.0,
                stance: stance.to_string(),
            },
            trend: TrendSignal {
                phase_5: (phase_5 * 100.0).round() / 100.0,
                phase_20: (phase_20 * 100.0).round() / 100.0,
                scope_5: (scope_5 * 100.0).round() / 100.0,
                scope_20: (scope_20 * 100.0).round() / 100.0,
                phase_direction,
                scope_direction,
            },
            recent_shifts,
            locus: locus_counts,
            novelty: novelty_counts,
            friction: friction_counts,
            novelty_friction: nf_cross,
            guidance,
        };

        let json = serde_json::to_string(&result).map_err(|e| db_err(&e))?;
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

    pub fn db_handle(&self) -> &DbHandle {
        &self.db
    }

    #[tool(
        description = "Search observations by full-text query. Returns ranked index with IDs and previews. Use optional before/after Unix timestamps to scope results to a time range.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn search(
        &self,
        p: Parameters<SearchParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let start = std::time::Instant::now();
        let result = self.do_search(p.0);
        record_query_metrics("search", start);
        result
    }

    #[tool(
        description = "Fetch full observation details by IDs. Returns complete observation objects.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_observations(
        &self,
        p: Parameters<GetObservationsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let start = std::time::Instant::now();
        let result = self.do_get_observations(p.0);
        record_query_metrics("get_observations", start);
        result
    }

    #[tool(
        description = "Get observations surrounding an anchor point within the same session.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn timeline(
        &self,
        p: Parameters<TimelineParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let start = std::time::Instant::now();
        let result = self.do_timeline(p.0);
        record_query_metrics("timeline", start);
        result
    }

    #[tool(
        description = "Session summaries generated by local LLM. Returns structured JSON with intent, completed work, files changed, and next steps. Use optional before/after Unix timestamps to filter by session start time.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn session_summaries(
        &self,
        p: Parameters<SessionSummariesParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let start = std::time::Instant::now();
        let result = self.do_session_summaries(p.0);
        record_query_metrics("session_summaries", start);
        result
    }

    #[tool(
        description = "Regenerate the full context injection (intents, session summaries, recent observations, cross-project pins) as markdown. Same output as SessionStart but with current data. Use optional before Unix timestamp to produce context as of a past point in time.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn regenerate_context(
        &self,
        p: Parameters<RegenerateContextParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let start = std::time::Instant::now();
        let result = self.do_regenerate_context(p.0);
        record_query_metrics("regenerate_context", start);
        result
    }

    #[tool(
        description = "Recent observations ranked by composite score (recency decay + type weight + project match). Deduped by file_path, keeping highest-scored entry per file. Use optional before/after Unix timestamps to window the results.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn recent_context(
        &self,
        p: Parameters<RecentContextParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let start = std::time::Instant::now();
        let result = self.do_recent_context(p.0);
        record_query_metrics("recent_context", start);
        result
    }

    #[tool(
        description = "Drill into a session's structure. Returns the session's prompts in order, each with its observations. Use to understand what happened step-by-step within a session.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn session_trace(
        &self,
        p: Parameters<SessionTraceParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let start = std::time::Instant::now();
        let result = self.do_session_trace(p.0);
        record_query_metrics("session_trace", start);
        result
    }

    #[tool(
        description = "Trace a file's history across sessions. Returns every session that touched this file, with the intent behind each touch. Use to understand why a file was read or modified over time.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn file_history(
        &self,
        p: Parameters<FileHistoryParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let start = std::time::Instant::now();
        let result = self.do_file_history(p.0);
        record_query_metrics("file_history", start);
        result
    }

    #[tool(
        description = "Queue a task for later execution by the systemd-driven dispatcher. Tasks are dispatched into tmux panes running Claude Code. Returns the task ID.",
        annotations(read_only_hint = false, open_world_hint = false)
    )]
    async fn queue_task(
        &self,
        p: Parameters<QueueTaskParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let start = std::time::Instant::now();
        let result = self.do_queue_task(p.0);
        record_query_metrics("queue_task", start);
        result
    }

    #[tool(
        description = "Create an agent-authored marker observation. Use to record conclusions, decisions, or waypoints not tied to a tool use. Markers are classified on all 5 dimensions and attached to the most recent session.",
        annotations(read_only_hint = false, open_world_hint = false)
    )]
    async fn create_marker(
        &self,
        p: Parameters<CreateMarkerParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let start = std::time::Instant::now();
        let result = self.do_create_marker(p.0);
        record_query_metrics("create_marker", start);
        result
    }

    #[tool(
        description = "Returns the current session's stance (phase × scope) with trend analysis and retrieval guidance. Call this periodically to orient your retrieval strategy. The `guidance` field tells you what nmem tools to use based on your current cognitive trajectory. When scope trends toward diverge, prior sessions' next_steps become relevant. When in deep think, search for prior conclusions. When in sustained act+converge, no retrieval action needed unless encountering new files.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn current_stance(
        &self,
        p: Parameters<CurrentStanceParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let start = std::time::Instant::now();
        let result = self.do_current_stance(p.0);
        record_query_metrics("current_stance", start);
        result
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
    crate::db::register_udfs(&conn)?;
    let db: DbHandle = Arc::new(Mutex::new(conn));
    let server = NmemServer::new(db);

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(NmemError::Io)?;

    rt.block_on(async {
        let config = crate::s5_config::load_config().unwrap_or_default();
        let provider = crate::metrics::init_meter_provider(&config.metrics);

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

        if let Some(p) = provider {
            let _ = p.shutdown();
        }

        Ok(())
    })
}
