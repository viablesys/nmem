//! S4 Intelligence — Fleet Beacon
//!
//! NATS subscriber that responds to federated search queries from peer nmem
//! instances. Each query runs tiered FTS5 against the local encrypted SQLite,
//! returning EpisodeResult as the primary unit of cross-fleet knowledge exchange.
//!
//! Lifecycle: `nmem beacon` → tokio runtime → subscribe loop → Ctrl-C shutdown.
//! Subject hierarchy: `nmem.{org}.search`, `nmem.{org}.heartbeat`.
//!
//! Adaptive timeouts: Jacobson/Karels RTO estimator (RFC 6298) calibrates
//! scatter/gather deadlines from heartbeat RTT samples. Cold start at 3s,
//! converges within ~10 samples to a data-driven value.

use bytes::Bytes;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::cli::BeaconArgs;
use crate::NmemError;

// ---------------------------------------------------------------------------
// Message types (wire contract — matches tools/nats-proto/ prototype)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchRequest {
    pub query: String,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub obs_type: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: u32,
    pub requester: String,
}

fn default_limit() -> u32 {
    20
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodeResult {
    pub id: i64,
    pub session_id: String,
    pub started_at: i64,
    #[serde(default)]
    pub ended_at: Option<i64>,
    pub intent: String,
    #[serde(default)]
    pub hot_files: Vec<String>,
    #[serde(default)]
    pub phase_signature: serde_json::Value,
    pub obs_count: i64,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub learned: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResponse {
    pub responder: String,
    #[serde(default)]
    pub episodes: Vec<EpisodeResult>,
    pub search_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatPing {
    pub sender: String,
    pub sent_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatPong {
    pub responder: String,
    pub echo_ms: u64,
}

// ---------------------------------------------------------------------------
// Jacobson/Karels RTO Estimator (RFC 6298)
// ---------------------------------------------------------------------------

/// Adaptive timeout estimator based on TCP's Jacobson/Karels algorithm (RFC 6298).
///
/// Tracks smoothed RTT (SRTT) and RTT variance (RTTVAR) to produce a
/// retransmission timeout (RTO) that converges as data accumulates.
///
/// Adapted for scatter/gather: feed max_rtt from each heartbeat round.
/// The variance term shrinks naturally with stable data, replacing a
/// static floor with a data-driven value.
#[derive(Debug, Clone)]
pub struct RtoEstimator {
    /// Smoothed RTT
    srtt: f64,
    /// Smoothed RTT variance
    rttvar: f64,
    /// Number of samples seen
    samples: u32,
    /// Smoothing factor for SRTT (fleet: 1/4 for faster convergence than TCP's 1/8)
    alpha: f64,
    /// Smoothing factor for RTTVAR (TCP default: 1/4)
    beta: f64,
    /// Variance multiplier (TCP default: 4 for ~99.99% coverage)
    k: f64,
    /// Absolute minimum RTO in seconds (prevents unreasonably tight timeouts)
    min_rto: f64,
}

impl Default for RtoEstimator {
    fn default() -> Self {
        Self::new()
    }
}

impl RtoEstimator {
    /// Create a new estimator with fleet-tuned defaults.
    ///
    /// - alpha=1/4 (faster convergence than TCP's 1/8, since heartbeats are infrequent)
    /// - beta=1/4 (same as TCP)
    /// - k=4 (4σ coverage)
    /// - min_rto=50ms (network floor)
    pub fn new() -> Self {
        RtoEstimator {
            srtt: 0.0,
            rttvar: 0.0,
            samples: 0,
            alpha: 0.25,
            beta: 0.25,
            k: 4.0,
            min_rto: 0.050,
        }
    }

    /// Seed with an initial RTT estimate (for testing or when you have a prior).
    /// Sets samples=1 so the estimator behaves as if it has seen one round.
    pub fn with_initial_rtt(mut self, rtt: Duration) -> Self {
        let r = rtt.as_secs_f64();
        self.srtt = r;
        self.rttvar = r / 2.0;
        self.samples = 1;
        self
    }

    /// Feed a new RTT sample. For scatter/gather, this should be the max RTT
    /// from a heartbeat round (the slowest responder determines the timeout).
    pub fn update(&mut self, rtt: Duration) {
        let r = rtt.as_secs_f64();

        if self.samples == 0 {
            // RFC 6298 Section 2.2: first measurement
            self.srtt = r;
            self.rttvar = r / 2.0;
        } else {
            // RFC 6298 Section 2.3: subsequent measurements
            self.rttvar = (1.0 - self.beta) * self.rttvar + self.beta * (self.srtt - r).abs();
            self.srtt = (1.0 - self.alpha) * self.srtt + self.alpha * r;
        }
        self.samples += 1;
    }

    /// Apply Karn's algorithm: double the RTO after a timeout event.
    /// Call this when scatter/gather gets fewer responses than expected.
    pub fn backoff(&mut self) {
        self.srtt *= 2.0;
    }

    /// Current RTO (retransmission timeout).
    /// Returns max(SRTT + K × RTTVAR, min_rto). Cold start returns 3s.
    pub fn rto(&self) -> Duration {
        if self.samples == 0 {
            return Duration::from_secs(3);
        }
        let rto = self.srtt + self.k * self.rttvar;
        Duration::from_secs_f64(rto.max(self.min_rto))
    }

    /// Current smoothed RTT estimate.
    pub fn srtt(&self) -> Duration {
        Duration::from_secs_f64(self.srtt)
    }

    /// Current RTT variance estimate.
    pub fn rttvar(&self) -> Duration {
        Duration::from_secs_f64(self.rttvar)
    }

    /// Number of samples processed.
    pub fn samples(&self) -> u32 {
        self.samples
    }
}

// ---------------------------------------------------------------------------
// Scatter/Gather
// ---------------------------------------------------------------------------

/// A response from scatter/gather with arrival timing.
pub struct TimedMessage {
    pub message: async_nats::Message,
    /// Time elapsed since the scatter was sent
    pub elapsed: Duration,
}

/// Scatter/gather: publish a request to a subject, collect all responses within timeout.
///
/// Unlike `client.request()` which returns only the first response, this
/// collects from all responders (fleet instances) within the timeout window.
/// Each response is tagged with its arrival time relative to the send.
pub async fn scatter_gather(
    client: &async_nats::Client,
    subject: &str,
    payload: Bytes,
    timeout: Duration,
) -> Vec<TimedMessage> {
    let inbox = client.new_inbox();
    let mut sub = match client.subscribe(inbox.clone()).await {
        Ok(s) => s,
        Err(_) => return vec![],
    };

    let t0 = Instant::now();
    if client
        .publish_with_reply(subject.to_string(), inbox, payload)
        .await
        .is_err()
    {
        return vec![];
    }
    let _ = client.flush().await;

    let mut responses = Vec::new();
    let deadline = tokio::time::Instant::now() + timeout;

    loop {
        match tokio::time::timeout_at(deadline, sub.next()).await {
            Ok(Some(msg)) => responses.push(TimedMessage {
                message: msg,
                elapsed: t0.elapsed(),
            }),
            Ok(None) => break,
            Err(_) => break,
        }
    }

    responses
}

// ---------------------------------------------------------------------------
// Fleet state from heartbeat probing
// ---------------------------------------------------------------------------

/// Snapshot of fleet membership and calibrated timeout from a heartbeat round.
#[derive(Debug, Clone)]
pub struct FleetState {
    /// Online instances and their round-trip latency
    pub instances: Vec<(String, Duration)>,
    /// Calibrated timeout from RTO estimator
    pub calibrated_timeout: Duration,
}

/// Send a heartbeat, collect all responses, update the RTO estimator.
///
/// The `rto` estimator determines the timeout: on first call (no data),
/// it uses a generous cold-start value (3s). Subsequent calls use the
/// Jacobson/Karels RTO which converges as data accumulates.
///
/// Returns `FleetState` with per-instance latencies and the current RTO.
pub async fn heartbeat(
    client: &async_nats::Client,
    subject: &str,
    sender: &str,
    rto: &mut RtoEstimator,
) -> FleetState {
    let t0 = Instant::now();

    let ping = HeartbeatPing {
        sender: sender.to_string(),
        sent_ms: t0.elapsed().as_millis() as u64,
    };

    let timeout = rto.rto();
    let payload = match serde_json::to_vec(&ping) {
        Ok(p) => Bytes::from(p),
        Err(_) => {
            return FleetState {
                instances: vec![],
                calibrated_timeout: timeout,
            }
        }
    };

    let responses = scatter_gather(client, subject, payload, timeout).await;

    let mut instances = Vec::new();
    for timed in &responses {
        if let Ok(pong) = serde_json::from_slice::<HeartbeatPong>(&timed.message.payload) {
            // Skip own echo
            if pong.responder != sender {
                instances.push((pong.responder, timed.elapsed));
            }
        }
    }

    // Feed max RTT from this round into the estimator
    if let Some(max_rtt) = instances.iter().map(|(_, d)| *d).max() {
        rto.update(max_rtt);
    }
    // If no responses, don't update — keep current estimate.
    // Caller can apply rto.backoff() if this indicates degradation.

    FleetState {
        instances,
        calibrated_timeout: rto.rto(),
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub fn handle_beacon(db_path: &Path, args: &BeaconArgs) -> Result<(), NmemError> {
    let config = crate::s5_config::load_config()?;
    let beacon_cfg = &config.beacon;

    let nats_url = args
        .nats_url
        .as_deref()
        .unwrap_or(&beacon_cfg.nats_url)
        .to_string();

    let org = args
        .org
        .as_deref()
        .or(beacon_cfg.org.as_deref())
        .ok_or_else(|| {
            NmemError::Config(
                "beacon: org not set (use --org or [beacon] org in config)".into(),
            )
        })?
        .to_string();

    let identity = args
        .identity
        .clone()
        .or_else(|| beacon_cfg.identity.clone())
        .unwrap_or_else(resolve_hostname);

    let limit = beacon_cfg.limit;
    let respond = beacon_cfg.respond;
    let dry_run = args.dry_run;
    let db_path = db_path.to_path_buf();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(NmemError::Io)?;

    rt.block_on(run_beacon(
        db_path, nats_url, org, identity, limit, respond, dry_run,
    ))
}

// ---------------------------------------------------------------------------
// Async beacon loop
// ---------------------------------------------------------------------------

async fn run_beacon(
    db_path: PathBuf,
    nats_url: String,
    org: String,
    identity: String,
    limit: u32,
    respond: bool,
    dry_run: bool,
) -> Result<(), NmemError> {
    log::info!("beacon: connecting to {nats_url}");

    let client = async_nats::connect(&nats_url)
        .await
        .map_err(|e| NmemError::Nats(format!("connect {nats_url}: {e}")))?;

    log::info!("beacon: connected (identity={identity}, org={org})");

    let search_subject = format!("nmem.{org}.search");
    let heartbeat_subject = format!("nmem.{org}.heartbeat");

    let mut search_sub = client
        .subscribe(search_subject.clone())
        .await
        .map_err(|e| NmemError::Nats(format!("subscribe {search_subject}: {e}")))?;

    let mut heartbeat_sub = client
        .subscribe(heartbeat_subject.clone())
        .await
        .map_err(|e| NmemError::Nats(format!("subscribe {heartbeat_subject}: {e}")))?;

    log::info!("beacon: subscribed to {search_subject}, {heartbeat_subject}");
    if dry_run {
        log::info!("beacon: DRY RUN — will log queries but not respond");
    }

    // RTO estimator — calibrates scatter/gather timeouts from heartbeat RTT.
    // Probes fleet every 30s; converges within ~10 samples (~5 min).
    let mut rto = RtoEstimator::new();
    let probe_interval = Duration::from_secs(30);
    let mut probe_timer = tokio::time::interval(probe_interval);
    // First tick fires immediately — initial fleet discovery
    probe_timer.tick().await;

    loop {
        tokio::select! {
            msg = search_sub.next() => {
                match msg {
                    Some(msg) => {
                        let client = client.clone();
                        let db_path = db_path.clone();
                        let identity = identity.clone();
                        tokio::spawn(async move {
                            handle_search_msg(
                                &client, msg, &db_path, &identity,
                                limit, respond, dry_run,
                            ).await;
                        });
                    }
                    None => {
                        log::warn!("beacon: search subscription closed");
                        break;
                    }
                }
            }
            msg = heartbeat_sub.next() => {
                match msg {
                    Some(msg) => {
                        let client = client.clone();
                        let identity = identity.clone();
                        tokio::spawn(async move {
                            handle_heartbeat_msg(&client, msg, &identity).await;
                        });
                    }
                    None => {
                        log::warn!("beacon: heartbeat subscription closed");
                        break;
                    }
                }
            }
            _ = probe_timer.tick() => {
                let fleet = heartbeat(
                    &client, &heartbeat_subject, &identity, &mut rto,
                ).await;
                if !fleet.instances.is_empty() {
                    let names: Vec<&str> = fleet.instances.iter()
                        .map(|(n, _)| n.as_str()).collect();
                    log::info!(
                        "beacon: fleet [{}] RTO {:?} (samples={})",
                        names.join(", "),
                        fleet.calibrated_timeout,
                        rto.samples(),
                    );
                }
            }
            _ = tokio::signal::ctrl_c() => {
                log::info!("beacon: shutting down");
                break;
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Message handlers
// ---------------------------------------------------------------------------

async fn handle_search_msg(
    client: &async_nats::Client,
    msg: async_nats::Message,
    db_path: &Path,
    identity: &str,
    limit: u32,
    respond: bool,
    dry_run: bool,
) {
    let reply = match &msg.reply {
        Some(r) => r.clone(),
        None => return,
    };

    let req: SearchRequest = match serde_json::from_slice(&msg.payload) {
        Ok(r) => r,
        Err(e) => {
            log::warn!("beacon: malformed request: {e}");
            return;
        }
    };

    // Don't respond to own queries
    if req.requester == identity {
        return;
    }

    log::info!(
        "beacon: query {:?} from {} (project={:?})",
        req.query, req.requester, req.project
    );

    if dry_run || !respond {
        return;
    }

    let t0 = Instant::now();
    let db_path = db_path.to_path_buf();
    let query = req.query.clone();
    let project = req.project.clone();
    let effective_limit = req.limit.min(limit) as i64;

    // SQLite on blocking thread (rusqlite is sync)
    let episodes = tokio::task::spawn_blocking(move || {
        execute_search(&db_path, &query, project.as_deref(), effective_limit)
    })
    .await
    .unwrap_or_else(|e| {
        log::error!("beacon: search task panicked: {e}");
        Ok(vec![])
    })
    .unwrap_or_else(|e| {
        log::warn!("beacon: search error: {e}");
        vec![]
    });

    let search_ms = t0.elapsed().as_millis() as u64;

    let response = SearchResponse {
        responder: identity.to_string(),
        episodes,
        search_ms,
    };

    if let Ok(payload) = serde_json::to_vec(&response) {
        let _ = client.publish(reply, Bytes::from(payload)).await;
    }

    log::info!(
        "beacon: responded in {search_ms}ms ({} episodes)",
        response.episodes.len()
    );
}

async fn handle_heartbeat_msg(
    client: &async_nats::Client,
    msg: async_nats::Message,
    identity: &str,
) {
    let reply = match &msg.reply {
        Some(r) => r.clone(),
        None => return,
    };

    if let Ok(ping) = serde_json::from_slice::<HeartbeatPing>(&msg.payload) {
        let pong = HeartbeatPong {
            responder: identity.to_string(),
            echo_ms: ping.sent_ms,
        };
        if let Ok(payload) = serde_json::to_vec(&pong) {
            let _ = client.publish(reply, Bytes::from(payload)).await;
        }
    }
}

// ---------------------------------------------------------------------------
// DB query — tiered FTS5 episode search
// ---------------------------------------------------------------------------

fn execute_search(
    db_path: &Path,
    query: &str,
    project: Option<&str>,
    limit: i64,
) -> Result<Vec<EpisodeResult>, NmemError> {
    let conn = crate::db::open_db(db_path)?;

    // Try tiered FTS5 queries
    let tiers = crate::query::rewrite_query(query);
    for tier_query in &tiers {
        if let Some(sanitized) = crate::query::sanitize_fts_query(tier_query) {
            let episodes = query_episodes_fts(&conn, &sanitized, project, limit)?;
            if !episodes.is_empty() {
                return Ok(episodes);
            }
        }
    }

    // Fallback: LIKE on intent + summary
    let episodes = query_episodes_like(&conn, query, project, limit)?;
    Ok(episodes)
}

fn query_episodes_fts(
    conn: &rusqlite::Connection,
    fts_query: &str,
    project: Option<&str>,
    limit: i64,
) -> Result<Vec<EpisodeResult>, NmemError> {
    let sql = "
        SELECT DISTINCT wu.id, wu.session_id, wu.started_at, wu.ended_at,
               wu.intent, wu.hot_files, wu.phase_signature,
               wu.obs_count, wu.summary, wu.learned, wu.notes
        FROM work_units wu
        JOIN observations o ON o.session_id = wu.session_id
            AND o.prompt_id BETWEEN wu.first_prompt_id AND wu.last_prompt_id
        JOIN observations_fts f ON f.rowid = o.id
        JOIN sessions s ON wu.session_id = s.id
        WHERE observations_fts MATCH ?1
          AND (?2 IS NULL OR s.project = ?2)
        GROUP BY wu.id
        ORDER BY MIN(f.rank)
        LIMIT ?3";

    let mut stmt = conn.prepare(sql)?;
    let rows = stmt
        .query_map(rusqlite::params![fts_query, project, limit], row_to_episode)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(rows)
}

fn query_episodes_like(
    conn: &rusqlite::Connection,
    term: &str,
    project: Option<&str>,
    limit: i64,
) -> Result<Vec<EpisodeResult>, NmemError> {
    let pattern = format!(
        "%{}%",
        term.replace('%', "\\%").replace('_', "\\_")
    );
    let sql = "
        SELECT wu.id, wu.session_id, wu.started_at, wu.ended_at,
               wu.intent, wu.hot_files, wu.phase_signature,
               wu.obs_count, wu.summary, wu.learned, wu.notes
        FROM work_units wu
        JOIN sessions s ON wu.session_id = s.id
        WHERE (wu.intent LIKE ?1 ESCAPE '\\' OR wu.summary LIKE ?1 ESCAPE '\\')
          AND (?2 IS NULL OR s.project = ?2)
        ORDER BY wu.started_at DESC
        LIMIT ?3";

    let mut stmt = conn.prepare(sql)?;
    let rows = stmt
        .query_map(rusqlite::params![pattern, project, limit], row_to_episode)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(rows)
}

fn row_to_episode(row: &rusqlite::Row<'_>) -> rusqlite::Result<EpisodeResult> {
    let hot_files_raw: Option<String> = row.get(5)?;
    let phase_sig_raw: Option<String> = row.get(6)?;

    let hot_files: Vec<String> = hot_files_raw
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let phase_signature: serde_json::Value = phase_sig_raw
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::Value::Object(Default::default()));

    Ok(EpisodeResult {
        id: row.get(0)?,
        session_id: row.get(1)?,
        started_at: row.get(2)?,
        ended_at: row.get(3)?,
        intent: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
        hot_files,
        phase_signature,
        obs_count: row.get(7)?,
        summary: row.get(8)?,
        learned: row.get(9)?,
        notes: row.get(10)?,
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn resolve_hostname() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_request_serde_roundtrip() {
        let req = SearchRequest {
            query: "session summary".into(),
            project: Some("nmem".into()),
            obs_type: None,
            limit: 10,
            requester: "test".into(),
        };
        let json = serde_json::to_vec(&req).unwrap();
        let de: SearchRequest = serde_json::from_slice(&json).unwrap();
        assert_eq!(de.query, "session summary");
        assert_eq!(de.project, Some("nmem".into()));
    }

    #[test]
    fn search_response_empty_serde() {
        let resp = SearchResponse {
            responder: "alice".into(),
            episodes: vec![],
            search_ms: 5,
        };
        let json = serde_json::to_vec(&resp).unwrap();
        let de: SearchResponse = serde_json::from_slice(&json).unwrap();
        assert_eq!(de.responder, "alice");
        assert!(de.episodes.is_empty());
    }

    #[test]
    fn episode_result_serde_roundtrip() {
        let ep = EpisodeResult {
            id: 1,
            session_id: "s1".into(),
            started_at: 1000,
            ended_at: Some(2000),
            intent: "implement beacon".into(),
            hot_files: vec!["src/s4_beacon.rs".into()],
            phase_signature: serde_json::json!({"converge": 5, "diverge": 10}),
            obs_count: 15,
            summary: Some("Built the beacon module".into()),
            learned: Some("spawn_blocking for sync DB".into()),
            notes: None,
        };
        let json = serde_json::to_vec(&ep).unwrap();
        let de: EpisodeResult = serde_json::from_slice(&json).unwrap();
        assert_eq!(de.intent, "implement beacon");
        assert_eq!(de.hot_files, vec!["src/s4_beacon.rs"]);
        assert_eq!(de.phase_signature["converge"], 5);
    }

    #[test]
    fn heartbeat_serde_roundtrip() {
        let ping = HeartbeatPing {
            sender: "alice".into(),
            sent_ms: 42,
        };
        let json = serde_json::to_vec(&ping).unwrap();
        let de: HeartbeatPing = serde_json::from_slice(&json).unwrap();
        assert_eq!(de.sender, "alice");

        let pong = HeartbeatPong {
            responder: "bob".into(),
            echo_ms: 42,
        };
        let json = serde_json::to_vec(&pong).unwrap();
        let de: HeartbeatPong = serde_json::from_slice(&json).unwrap();
        assert_eq!(de.responder, "bob");
    }

    #[test]
    fn episode_search_against_memory_db() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::schema::MIGRATIONS.to_latest(&mut conn).unwrap();

        // Seed session + work_unit
        conn.execute(
            "INSERT INTO sessions(id, project, started_at) VALUES ('s1', 'test', 1000)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO work_units(session_id, started_at, intent, first_prompt_id, last_prompt_id, hot_files, phase_signature, obs_count, summary)
             VALUES ('s1', 1000, 'implement session summarization', 1, 10, '[\"src/main.rs\"]', '{\"converge\":5}', 10, 'Built summarization')",
            [],
        )
        .unwrap();

        // LIKE fallback should find it (no observations for FTS)
        let results =
            query_episodes_like(&conn, "summarization", Some("test"), 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].intent, "implement session summarization");
        assert_eq!(results[0].hot_files, vec!["src/main.rs"]);
    }

    #[test]
    fn episode_search_empty_db() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::schema::MIGRATIONS.to_latest(&mut conn).unwrap();

        let results = query_episodes_like(&conn, "anything", None, 10).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn hostname_resolves() {
        let h = resolve_hostname();
        assert!(!h.is_empty());
        assert_ne!(h, "unknown");
    }

    // --- RtoEstimator tests ---

    #[test]
    fn rto_cold_start() {
        let rto = RtoEstimator::new();
        assert_eq!(rto.samples(), 0);
        assert_eq!(rto.rto(), Duration::from_secs(3));
    }

    #[test]
    fn rto_first_sample() {
        let mut rto = RtoEstimator::new();
        rto.update(Duration::from_millis(100));
        assert_eq!(rto.samples(), 1);
        // RFC 6298: SRTT=R, RTTVAR=R/2 → RTO = R + 4*(R/2) = 3R = 300ms
        let val = rto.rto();
        assert!(
            val >= Duration::from_millis(295) && val <= Duration::from_millis(305),
            "first sample RTO ({val:?}) should be ~300ms"
        );
    }

    #[test]
    fn rto_converges_with_stable_data() {
        let mut rto = RtoEstimator::new();
        for _ in 0..50 {
            rto.update(Duration::from_millis(100));
        }
        let val = rto.rto();
        // After many stable samples, RTTVAR → 0, RTO → SRTT ≈ 100ms
        assert!(
            val >= Duration::from_millis(90) && val <= Duration::from_millis(150),
            "stable RTO ({val:?}) should converge near 100ms"
        );
    }

    #[test]
    fn rto_reacts_to_spike() {
        let mut rto = RtoEstimator::new();
        for _ in 0..20 {
            rto.update(Duration::from_millis(50));
        }
        let baseline = rto.rto();
        rto.update(Duration::from_millis(500));
        let after_spike = rto.rto();
        assert!(
            after_spike > baseline,
            "spike should increase RTO: baseline={baseline:?}, after={after_spike:?}"
        );
    }

    #[test]
    fn rto_min_floor() {
        let mut rto = RtoEstimator::new();
        for _ in 0..50 {
            rto.update(Duration::from_micros(100));
        }
        assert!(
            rto.rto() >= Duration::from_millis(50),
            "RTO ({:?}) should not go below min_rto=50ms",
            rto.rto()
        );
    }

    #[test]
    fn rto_backoff_doubles() {
        let mut rto = RtoEstimator::new();
        rto.update(Duration::from_millis(100));
        let before = rto.rto();
        rto.backoff();
        let after = rto.rto();
        assert!(
            after > before,
            "backoff should increase RTO: before={before:?}, after={after:?}"
        );
    }

    #[test]
    fn rto_with_initial_seed() {
        let rto = RtoEstimator::new().with_initial_rtt(Duration::from_millis(200));
        assert_eq!(rto.samples(), 1);
        // SRTT=200ms, RTTVAR=100ms → RTO = 200 + 4*100 = 600ms
        let val = rto.rto();
        assert!(
            val >= Duration::from_millis(595) && val <= Duration::from_millis(605),
            "seeded RTO ({val:?}) should be ~600ms"
        );
    }
}
