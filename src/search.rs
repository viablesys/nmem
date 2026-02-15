use crate::cli::SearchArgs;
use crate::db::open_db_readonly;
use crate::NmemError;
use serde::Serialize;
use std::path::Path;

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

pub fn handle_search(db_path: &Path, args: &SearchArgs) -> Result<(), NmemError> {
    let conn = open_db_readonly(db_path)?;
    let limit = args.limit.clamp(1, 100);

    let blended = match args.order_by.as_str() {
        "relevance" => false,
        "blended" => true,
        other => {
            return Err(NmemError::Config(format!(
                "invalid --order-by: {other:?} (expected \"relevance\" or \"blended\")"
            )));
        }
    };

    if blended {
        crate::db::register_udfs(&conn)?;
    }

    if args.ids {
        print_ids(&conn, &args.query, args.project.as_deref(), args.obs_type.as_deref(), limit, blended)?;
    } else if args.full {
        print_full(&conn, &args.query, args.project.as_deref(), args.obs_type.as_deref(), limit, blended)?;
    } else {
        print_index(&conn, &args.query, args.project.as_deref(), args.obs_type.as_deref(), limit, blended)?;
    }

    Ok(())
}

const BLENDED_INDEX_SQL: &str = "WITH fts_matches AS (
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
LIMIT ?4";

const BLENDED_FULL_SQL: &str = "WITH fts_matches AS (
    SELECT o.id, o.timestamp, o.session_id, o.obs_type, o.source_event,
           o.tool_name, o.file_path, o.content, o.metadata, o.is_pinned,
           f.rank AS raw_rank
    FROM observations o
    JOIN sessions s ON o.session_id = s.id
    JOIN observations_fts f ON o.id = f.rowid
    WHERE observations_fts MATCH ?1
      AND (?2 IS NULL OR s.project = ?2)
      AND (?3 IS NULL OR o.obs_type = ?3)
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
SELECT id, timestamp, session_id, obs_type, source_event,
       tool_name, file_path, content, metadata, is_pinned
FROM scored
ORDER BY (bm25_norm * 0.5 + recency * 0.3 + type_w * 0.2) DESC
LIMIT ?4";

const BLENDED_IDS_SQL: &str = "WITH fts_matches AS (
    SELECT o.id, o.timestamp, o.obs_type,
           f.rank AS raw_rank
    FROM observations o
    JOIN sessions s ON o.session_id = s.id
    JOIN observations_fts f ON o.id = f.rowid
    WHERE observations_fts MATCH ?1
      AND (?2 IS NULL OR s.project = ?2)
      AND (?3 IS NULL OR o.obs_type = ?3)
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
SELECT id
FROM scored
ORDER BY (bm25_norm * 0.5 + recency * 0.3 + type_w * 0.2) DESC
LIMIT ?4";

fn print_index(
    conn: &rusqlite::Connection,
    query: &str,
    project: Option<&str>,
    obs_type: Option<&str>,
    limit: i64,
    blended: bool,
) -> Result<(), NmemError> {
    let sql = if blended {
        BLENDED_INDEX_SQL
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
         ORDER BY f.rank
         LIMIT ?4"
    };
    let mut stmt = conn.prepare(sql)?;

    let results: Vec<SearchResult> = stmt
        .query_map(
            rusqlite::params![query, project, obs_type, limit],
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
        )?
        .collect::<Result<_, _>>()?;

    let json = serde_json::to_string(&results)?;
    println!("{json}");
    eprintln!("nmem: {} results for {:?}", results.len(), query);
    Ok(())
}

fn print_full(
    conn: &rusqlite::Connection,
    query: &str,
    project: Option<&str>,
    obs_type: Option<&str>,
    limit: i64,
    blended: bool,
) -> Result<(), NmemError> {
    let sql = if blended {
        BLENDED_FULL_SQL
    } else {
        "SELECT o.id, o.timestamp, o.session_id, o.obs_type, o.source_event,
                o.tool_name, o.file_path, o.content, o.metadata, o.is_pinned
         FROM observations o
         JOIN sessions s ON o.session_id = s.id
         JOIN observations_fts f ON o.id = f.rowid
         WHERE observations_fts MATCH ?1
           AND (?2 IS NULL OR s.project = ?2)
           AND (?3 IS NULL OR o.obs_type = ?3)
         ORDER BY f.rank
         LIMIT ?4"
    };
    let mut stmt = conn.prepare(sql)?;

    let results: Vec<FullObservation> = stmt
        .query_map(
            rusqlite::params![query, project, obs_type, limit],
            |row| {
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
            },
        )?
        .collect::<Result<_, _>>()?;

    let json = serde_json::to_string(&results)?;
    println!("{json}");
    eprintln!("nmem: {} results for {:?}", results.len(), query);
    Ok(())
}

fn print_ids(
    conn: &rusqlite::Connection,
    query: &str,
    project: Option<&str>,
    obs_type: Option<&str>,
    limit: i64,
    blended: bool,
) -> Result<(), NmemError> {
    let sql = if blended {
        BLENDED_IDS_SQL
    } else {
        "SELECT o.id
         FROM observations o
         JOIN sessions s ON o.session_id = s.id
         JOIN observations_fts f ON o.id = f.rowid
         WHERE observations_fts MATCH ?1
           AND (?2 IS NULL OR s.project = ?2)
           AND (?3 IS NULL OR o.obs_type = ?3)
         ORDER BY f.rank
         LIMIT ?4"
    };
    let mut stmt = conn.prepare(sql)?;

    let ids: Vec<i64> = stmt
        .query_map(
            rusqlite::params![query, project, obs_type, limit],
            |row| row.get(0),
        )?
        .collect::<Result<_, _>>()?;

    for id in &ids {
        println!("{id}");
    }
    eprintln!("nmem: {} results for {:?}", ids.len(), query);
    Ok(())
}
