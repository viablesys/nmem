//! ADR-015 Fleet Beacon — Prototype 1: NATS Connectivity
//!
//! Validates async-nats 0.46 patterns needed for fleet beacon federation:
//! pub/sub, request/reply, scatter/gather, subject hierarchy with org isolation.
//!
//! Usage:
//!   cargo run                              # demo against localhost:4222
//!   NATS_URL=nats://host:4222 cargo run    # custom NATS server
//!
//! Tests (require Docker):
//!   cargo test -- --ignored

use bytes::Bytes;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Section 1: Message Types (mirrors ADR-015 subject hierarchy)
// ---------------------------------------------------------------------------

/// Search request broadcast to nmem.{org}.search
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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

/// Individual search result from a responding instance.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchResult {
    pub id: i64,
    pub timestamp: i64,
    pub obs_type: String,
    pub content_preview: String,
    #[serde(default)]
    pub file_path: Option<String>,
    pub session_id: String,
}

/// Search response from one nmem instance.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchResponse {
    pub responder: String,
    pub results: Vec<SearchResult>,
    pub search_ms: u64,
}

/// RAG new-doc notification broadcast to nmem.{org}.rag.new
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RagNotification {
    pub filename: String,
    pub title: String,
    pub author: String,
    pub hash: String,
    pub tags: Vec<String>,
}

/// Heartbeat ping broadcast to nmem.{org}.heartbeat
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HeartbeatPing {
    /// Who sent the heartbeat
    pub sender: String,
    /// Millisecond timestamp (monotonic, sender-local)
    pub sent_ms: u64,
}

/// Heartbeat pong from each online instance
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HeartbeatPong {
    /// Identity of the responding instance
    pub responder: String,
    /// Echo back the sender's sent_ms for round-trip calculation
    pub echo_ms: u64,
}

/// Fleet state derived from a heartbeat round.
#[derive(Debug, Clone)]
pub struct FleetState {
    /// Online instances and their round-trip latency
    pub instances: Vec<(String, Duration)>,
    /// Calibrated timeout: max observed latency × 3, minimum 100ms
    pub calibrated_timeout: Duration,
}

// ---------------------------------------------------------------------------
// Section 2: Test Harness — NatsTestServer (Docker lifecycle)
// ---------------------------------------------------------------------------

/// Manages a NATS server Docker container for integration tests.
pub struct NatsTestServer {
    container_id: String,
    pub port: u16,
}

impl NatsTestServer {
    /// Start a NATS server in Docker on the given port.
    pub async fn start(port: u16) -> Self {
        // Start container
        let output = std::process::Command::new("docker")
            .args([
                "run", "-d",
                "--name", &format!("nats-proto-test-{port}"),
                "-p", &format!("{port}:4222"),
                "nats:latest",
            ])
            .output()
            .expect("failed to start docker");

        let container_id = String::from_utf8(output.stdout)
            .expect("invalid container id")
            .trim()
            .to_string();

        assert!(
            !container_id.is_empty(),
            "docker run failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        // Wait for NATS to be ready (TCP connect)
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if Instant::now() > deadline {
                panic!("NATS server did not become ready within 10s");
            }
            if tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
                .await
                .is_ok()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        NatsTestServer { container_id, port }
    }

    pub fn url(&self) -> String {
        format!("nats://127.0.0.1:{}", self.port)
    }
}

impl Drop for NatsTestServer {
    fn drop(&mut self) {
        let _ = std::process::Command::new("docker")
            .args(["rm", "-f", &self.container_id])
            .output();
    }
}

// ---------------------------------------------------------------------------
// Section 3: Scatter/Gather Helper
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
    subject: impl Into<String>,
    payload: Bytes,
    timeout: Duration,
) -> Vec<TimedMessage> {
    let subject = subject.into();
    let inbox = client.new_inbox();
    let mut sub = client.subscribe(inbox.clone()).await.expect("subscribe inbox");

    let t0 = Instant::now();
    client
        .publish_with_reply(subject, inbox, payload)
        .await
        .expect("publish_with_reply");
    client.flush().await.expect("flush");

    let mut responses = Vec::new();
    let deadline = tokio::time::Instant::now() + timeout;

    loop {
        match tokio::time::timeout_at(deadline, sub.next()).await {
            Ok(Some(msg)) => responses.push(TimedMessage {
                message: msg,
                elapsed: t0.elapsed(),
            }),
            Ok(None) => break, // subscription closed
            Err(_) => break,   // timeout
        }
    }

    responses
}

/// Send a heartbeat, collect all responses, derive calibrated timeout.
///
/// First heartbeat uses `initial_timeout` (generous, e.g. 5s).
/// Returns `FleetState` with per-instance latencies and a calibrated timeout
/// for subsequent scatter/gather operations.
pub async fn heartbeat(
    client: &async_nats::Client,
    subject: impl Into<String>,
    sender: &str,
    initial_timeout: Duration,
) -> FleetState {
    let subject = subject.into();
    let t0 = Instant::now();

    let ping = HeartbeatPing {
        sender: sender.to_string(),
        sent_ms: t0.elapsed().as_millis() as u64,
    };

    let responses = scatter_gather(
        client,
        &subject,
        serde_json::to_vec(&ping).unwrap().into(),
        initial_timeout,
    )
    .await;

    let mut instances = Vec::new();
    for timed in &responses {
        if let Ok(pong) = serde_json::from_slice::<HeartbeatPong>(&timed.message.payload) {
            instances.push((pong.responder, timed.elapsed));
        }
    }

    // Calibrated timeout: max observed RTT × 3, floor at 500ms
    //
    // Why ×3: absorbs jitter, GC pauses, load spikes, model-loading delays.
    // Why 500ms floor: cross-continent RTT is 60-300ms. An agent on the
    // opposite coast with a loaded machine can spike well past 100ms.
    // 500ms floor means even localhost fleets have room for a bad moment.
    //
    // The initial_timeout for first heartbeat should be generous (3-5s)
    // because we don't know fleet geography yet.
    let max_rtt = instances
        .iter()
        .map(|(_, d)| *d)
        .max()
        .unwrap_or(Duration::from_millis(500));
    let calibrated = (max_rtt * 3).max(Duration::from_millis(500));

    FleetState {
        instances,
        calibrated_timeout: calibrated,
    }
}

// ---------------------------------------------------------------------------
// Section 4: Demo main()
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let nats_url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());

    eprintln!("=== ADR-015 NATS Prototype ===");
    eprintln!("Server: {nats_url}");
    eprintln!();

    // --- Connect ---
    let t0 = Instant::now();
    let client = async_nats::connect(&nats_url).await?;
    eprintln!("[1/4] Connected: {:?}", t0.elapsed());

    // --- Pub/Sub demo ---
    let t1 = Instant::now();
    let mut sub = client.subscribe("nmem.demo.rag.new").await?;

    let notification = RagNotification {
        filename: "nats.md".into(),
        title: "NATS Reference".into(),
        author: "alice".into(),
        hash: "abc123".into(),
        tags: vec!["rust".into(), "messaging".into()],
    };
    client
        .publish("nmem.demo.rag.new", serde_json::to_vec(&notification)?.into())
        .await?;
    client.flush().await?;

    let msg = tokio::time::timeout(Duration::from_secs(2), sub.next())
        .await?
        .expect("expected message");
    let received: RagNotification = serde_json::from_slice(&msg.payload)?;
    eprintln!("[2/4] Pub/Sub: {:?} — received: {}", t1.elapsed(), received.title);

    // --- Request/Reply demo ---
    let t2 = Instant::now();
    let mut responder_sub = client.subscribe("nmem.demo.search").await?;
    let client2 = client.clone();

    // Spawn responder
    let responder = tokio::spawn(async move {
        if let Some(msg) = responder_sub.next().await {
            let _req: SearchRequest = serde_json::from_slice(&msg.payload).unwrap();
            let response = SearchResponse {
                responder: "alice-laptop".into(),
                results: vec![SearchResult {
                    id: 42,
                    timestamp: 1773543500,
                    obs_type: "file_edit".into(),
                    content_preview: "Modified s1_serve.rs".into(),
                    file_path: Some("src/s1_serve.rs".into()),
                    session_id: "abc-123".into(),
                }],
                search_ms: 3,
            };
            if let Some(reply) = msg.reply {
                client2
                    .publish(reply, serde_json::to_vec(&response).unwrap().into())
                    .await
                    .unwrap();
            }
        }
    });

    let request = SearchRequest {
        query: "session summarization".into(),
        project: Some("nmem".into()),
        obs_type: None,
        limit: 10,
        requester: "bob-desktop".into(),
    };
    let reply = client
        .request("nmem.demo.search", serde_json::to_vec(&request)?.into())
        .await?;
    let response: SearchResponse = serde_json::from_slice(&reply.payload)?;
    eprintln!(
        "[3/4] Request/Reply: {:?} — {} results from {}",
        t2.elapsed(),
        response.results.len(),
        response.responder
    );
    responder.await?;

    // --- Heartbeat + Scatter/Gather demo ---
    let t3 = Instant::now();

    // Spawn 3 mock fleet instances that respond to both heartbeat and search
    let mut handles = Vec::new();
    for i in 0..3 {
        let c = client.clone();
        let name = format!("instance-{i}");
        // Each instance subscribes to heartbeat AND search
        let mut hb_sub = c.subscribe("nmem.demo.heartbeat").await?;
        let mut search_sub = c.subscribe("nmem.demo.fleet.search").await?;
        let c2 = c.clone();
        let name2 = name.clone();
        handles.push(tokio::spawn(async move {
            // Handle heartbeat
            if let Some(msg) = hb_sub.next().await {
                if let Ok(ping) = serde_json::from_slice::<HeartbeatPing>(&msg.payload) {
                    let pong = HeartbeatPong {
                        responder: name.clone(),
                        echo_ms: ping.sent_ms,
                    };
                    if let Some(reply) = msg.reply {
                        c.publish(reply, serde_json::to_vec(&pong).unwrap().into())
                            .await
                            .unwrap();
                    }
                }
            }
        }));
        handles.push(tokio::spawn(async move {
            // Handle search
            if let Some(msg) = search_sub.next().await {
                let response = SearchResponse {
                    responder: name2,
                    results: vec![],
                    search_ms: (i as u64 + 1) * 2,
                };
                if let Some(reply) = msg.reply {
                    c2.publish(reply, serde_json::to_vec(&response).unwrap().into())
                        .await
                        .unwrap();
                }
            }
        }));
    }

    // Give subscribers time to register
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Step 1: Heartbeat — discover fleet, calibrate timeout
    // Initial timeout is generous (3s) since we don't know fleet geography yet
    let fleet = heartbeat(
        &client,
        "nmem.demo.heartbeat",
        "demo-runner",
        Duration::from_secs(3),
    )
    .await;

    eprintln!(
        "[4/5] Heartbeat: {:?} — {} instances online, calibrated timeout: {:?}",
        t3.elapsed(),
        fleet.instances.len(),
        fleet.calibrated_timeout,
    );
    for (name, rtt) in &fleet.instances {
        eprintln!("       {name} (RTT: {rtt:?})");
    }

    // Step 2: Scatter/Gather with calibrated timeout
    let t4 = Instant::now();
    let responses = scatter_gather(
        &client,
        "nmem.demo.fleet.search",
        serde_json::to_vec(&request)?.into(),
        fleet.calibrated_timeout,
    )
    .await;

    eprintln!(
        "[5/5] Scatter/Gather: {:?} — {} responses (timeout: {:?})",
        t4.elapsed(),
        responses.len(),
        fleet.calibrated_timeout,
    );
    for tm in &responses {
        let r: SearchResponse = serde_json::from_slice(&tm.message.payload).unwrap();
        eprintln!("       {} ({}ms, RTT: {:?})", r.responder, r.search_ms, tm.elapsed);
    }

    for h in handles {
        h.await?;
    }

    eprintln!();
    eprintln!("Total: {:?}", t0.elapsed());
    eprintln!("=== All patterns validated ===");

    Ok(())
}

// ---------------------------------------------------------------------------
// Section 5: Tests (TDD — all require Docker, marked #[ignore])
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use futures::StreamExt;
    use std::time::Duration;

    // --- Group 1: Connection lifecycle ---

    #[tokio::test]
    #[ignore]
    async fn test_connect_to_nats() {
        let server = NatsTestServer::start(14222).await;
        let client = async_nats::connect(&server.url()).await;
        assert!(client.is_ok(), "should connect to NATS");
    }

    #[tokio::test]
    #[ignore]
    async fn test_connect_failure_bad_address() {
        let result = tokio::time::timeout(
            Duration::from_secs(3),
            async_nats::connect("nats://127.0.0.1:19999"),
        )
        .await;

        // Either the connect itself errors or it times out
        match result {
            Ok(Ok(_)) => panic!("should not connect to nonexistent server"),
            Ok(Err(_)) => {} // connect error — expected
            Err(_) => {}     // timeout — also acceptable
        }
    }

    // --- Group 2: Pub/Sub ---

    #[tokio::test]
    #[ignore]
    async fn test_pubsub_simple() {
        let server = NatsTestServer::start(14223).await;
        let client = async_nats::connect(&server.url()).await.unwrap();

        let mut sub = client.subscribe("nmem.viablesys.test").await.unwrap();

        let notification = RagNotification {
            filename: "test.md".into(),
            title: "Test Doc".into(),
            author: "alice".into(),
            hash: "deadbeef".into(),
            tags: vec!["rust".into()],
        };
        client
            .publish(
                "nmem.viablesys.test",
                serde_json::to_vec(&notification).unwrap().into(),
            )
            .await
            .unwrap();
        client.flush().await.unwrap();

        let msg = tokio::time::timeout(Duration::from_secs(2), sub.next())
            .await
            .expect("timeout")
            .expect("no message");

        let received: RagNotification = serde_json::from_slice(&msg.payload).unwrap();
        assert_eq!(received, notification);
    }

    #[tokio::test]
    #[ignore]
    async fn test_pubsub_wildcard_star() {
        let server = NatsTestServer::start(14224).await;
        let client = async_nats::connect(&server.url()).await.unwrap();

        // * matches exactly one token
        let mut sub = client.subscribe("nmem.*.search").await.unwrap();

        client
            .publish("nmem.viablesys.search", "org1".into())
            .await
            .unwrap();
        client
            .publish("nmem.acmecorp.search", "org2".into())
            .await
            .unwrap();
        client.flush().await.unwrap();

        let msg1 = tokio::time::timeout(Duration::from_secs(2), sub.next())
            .await
            .expect("timeout")
            .expect("no msg1");
        let msg2 = tokio::time::timeout(Duration::from_secs(2), sub.next())
            .await
            .expect("timeout")
            .expect("no msg2");

        let payloads: Vec<String> = vec![
            String::from_utf8(msg1.payload.to_vec()).unwrap(),
            String::from_utf8(msg2.payload.to_vec()).unwrap(),
        ];
        assert!(payloads.contains(&"org1".to_string()));
        assert!(payloads.contains(&"org2".to_string()));
    }

    #[tokio::test]
    #[ignore]
    async fn test_pubsub_wildcard_gt() {
        let server = NatsTestServer::start(14225).await;
        let client = async_nats::connect(&server.url()).await.unwrap();

        // > matches one or more tokens at the end
        let mut sub = client.subscribe("nmem.viablesys.>").await.unwrap();

        // These should match
        client
            .publish("nmem.viablesys.search", "a".into())
            .await
            .unwrap();
        client
            .publish("nmem.viablesys.rag.new", "b".into())
            .await
            .unwrap();
        client
            .publish("nmem.viablesys.research.request", "c".into())
            .await
            .unwrap();
        // This should NOT match
        client
            .publish("nmem.acmecorp.search", "d".into())
            .await
            .unwrap();
        client.flush().await.unwrap();

        let mut received = Vec::new();
        for _ in 0..3 {
            match tokio::time::timeout(Duration::from_secs(2), sub.next()).await {
                Ok(Some(msg)) => {
                    received.push(String::from_utf8(msg.payload.to_vec()).unwrap());
                }
                _ => break,
            }
        }

        assert_eq!(received.len(), 3, "should receive exactly 3 messages");
        assert!(received.contains(&"a".to_string()));
        assert!(received.contains(&"b".to_string()));
        assert!(received.contains(&"c".to_string()));

        // Verify the acmecorp message was NOT received
        let extra = tokio::time::timeout(Duration::from_millis(200), sub.next()).await;
        assert!(extra.is_err(), "should not receive acmecorp message");
    }

    #[tokio::test]
    #[ignore]
    async fn test_pubsub_no_subscriber() {
        let server = NatsTestServer::start(14226).await;
        let client = async_nats::connect(&server.url()).await.unwrap();

        // Publishing with no subscribers should not error (fire-and-forget)
        let result = client
            .publish("nmem.nobody.listening", "hello".into())
            .await;
        assert!(result.is_ok(), "publish with no subscribers should succeed");
    }

    // --- Group 3: Request/Reply (single responder) ---

    #[tokio::test]
    #[ignore]
    async fn test_request_reply_single() {
        let server = NatsTestServer::start(14227).await;
        let client = async_nats::connect(&server.url()).await.unwrap();

        let responder_client = client.clone();
        let mut responder_sub = client.subscribe("nmem.viablesys.search").await.unwrap();

        // Spawn responder
        let responder = tokio::spawn(async move {
            let msg = responder_sub.next().await.unwrap();
            let _req: SearchRequest = serde_json::from_slice(&msg.payload).unwrap();
            let response = SearchResponse {
                responder: "alice".into(),
                results: vec![SearchResult {
                    id: 1,
                    timestamp: 1000,
                    obs_type: "file_read".into(),
                    content_preview: "read main.rs".into(),
                    file_path: Some("src/main.rs".into()),
                    session_id: "sess-1".into(),
                }],
                search_ms: 5,
            };
            responder_client
                .publish(
                    msg.reply.unwrap(),
                    serde_json::to_vec(&response).unwrap().into(),
                )
                .await
                .unwrap();
        });

        let request = SearchRequest {
            query: "main.rs".into(),
            project: None,
            obs_type: None,
            limit: 20,
            requester: "bob".into(),
        };
        let reply = client
            .request(
                "nmem.viablesys.search",
                serde_json::to_vec(&request).unwrap().into(),
            )
            .await
            .unwrap();

        let response: SearchResponse = serde_json::from_slice(&reply.payload).unwrap();
        assert_eq!(response.responder, "alice");
        assert_eq!(response.results.len(), 1);
        assert_eq!(response.results[0].id, 1);

        responder.await.unwrap();
    }

    #[tokio::test]
    #[ignore]
    async fn test_request_reply_timeout() {
        let server = NatsTestServer::start(14228).await;
        let client = async_nats::connect(&server.url()).await.unwrap();

        // Subscribe to the subject but never respond — just hold the subscription
        // so NATS doesn't return NoResponders
        let _sub = client.subscribe("nmem.viablesys.timeout").await.unwrap();

        let request = async_nats::Request::new()
            .payload("test".into())
            .timeout(Some(Duration::from_millis(500)));

        let result = client
            .send_request("nmem.viablesys.timeout", request)
            .await;

        assert!(result.is_err(), "should timeout with no response");
        let err = result.unwrap_err();
        assert_eq!(err.kind(), async_nats::RequestErrorKind::TimedOut);
    }

    #[tokio::test]
    #[ignore]
    async fn test_request_reply_no_responders() {
        let server = NatsTestServer::start(14229).await;
        let client = async_nats::connect(&server.url()).await.unwrap();

        // No subscriber at all — NATS 2.2+ returns NoResponders immediately
        let result = client
            .request("nmem.viablesys.ghost", "hello".into())
            .await;

        assert!(result.is_err(), "should get NoResponders error");
        let err = result.unwrap_err();
        assert_eq!(err.kind(), async_nats::RequestErrorKind::NoResponders);
    }

    // --- Group 4: Scatter/Gather (fleet pattern) ---

    #[tokio::test]
    #[ignore]
    async fn test_scatter_gather_multiple_responders() {
        let server = NatsTestServer::start(14230).await;
        let client = async_nats::connect(&server.url()).await.unwrap();

        // Spawn 3 fleet instances
        let mut handles = Vec::new();
        for i in 0..3 {
            let c = client.clone();
            let name = format!("instance-{i}");
            let mut sub = c.subscribe("nmem.viablesys.search").await.unwrap();
            handles.push(tokio::spawn(async move {
                if let Some(msg) = sub.next().await {
                    let response = SearchResponse {
                        responder: name,
                        results: vec![],
                        search_ms: i as u64,
                    };
                    if let Some(reply) = msg.reply {
                        c.publish(reply, serde_json::to_vec(&response).unwrap().into())
                            .await
                            .unwrap();
                    }
                }
            }));
        }

        // Let subscriptions register
        tokio::time::sleep(Duration::from_millis(50)).await;

        let responses = scatter_gather(
            &client,
            "nmem.viablesys.search",
            Bytes::from("test"),
            Duration::from_secs(2),
        )
        .await;

        assert_eq!(responses.len(), 3, "should receive from all 3 instances");

        let mut responders: Vec<String> = responses
            .iter()
            .map(|m| {
                serde_json::from_slice::<SearchResponse>(&m.message.payload)
                    .unwrap()
                    .responder
            })
            .collect();
        responders.sort();
        assert_eq!(responders, vec!["instance-0", "instance-1", "instance-2"]);

        for h in handles {
            h.await.unwrap();
        }
    }

    #[tokio::test]
    #[ignore]
    async fn test_scatter_gather_partial_timeout() {
        let server = NatsTestServer::start(14231).await;
        let client = async_nats::connect(&server.url()).await.unwrap();

        // Instance 0: responds immediately
        let c0 = client.clone();
        let mut sub0 = c0.subscribe("nmem.viablesys.search").await.unwrap();
        let h0 = tokio::spawn(async move {
            if let Some(msg) = sub0.next().await {
                let r = SearchResponse {
                    responder: "fast".into(),
                    results: vec![],
                    search_ms: 1,
                };
                if let Some(reply) = msg.reply {
                    c0.publish(reply, serde_json::to_vec(&r).unwrap().into())
                        .await
                        .unwrap();
                }
            }
        });

        // Instance 1: responds after 200ms
        let c1 = client.clone();
        let mut sub1 = c1.subscribe("nmem.viablesys.search").await.unwrap();
        let h1 = tokio::spawn(async move {
            if let Some(msg) = sub1.next().await {
                tokio::time::sleep(Duration::from_millis(200)).await;
                let r = SearchResponse {
                    responder: "medium".into(),
                    results: vec![],
                    search_ms: 200,
                };
                if let Some(reply) = msg.reply {
                    c1.publish(reply, serde_json::to_vec(&r).unwrap().into())
                        .await
                        .unwrap();
                }
            }
        });

        // Instance 2: sleeps 5 seconds (misses the window)
        let c2 = client.clone();
        let mut sub2 = c2.subscribe("nmem.viablesys.search").await.unwrap();
        let h2 = tokio::spawn(async move {
            if let Some(msg) = sub2.next().await {
                tokio::time::sleep(Duration::from_secs(5)).await;
                let r = SearchResponse {
                    responder: "slow".into(),
                    results: vec![],
                    search_ms: 5000,
                };
                if let Some(reply) = msg.reply {
                    c2.publish(reply, serde_json::to_vec(&r).unwrap().into())
                        .await
                        .unwrap();
                }
            }
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        let responses = scatter_gather(
            &client,
            "nmem.viablesys.search",
            Bytes::from("test"),
            Duration::from_secs(1),
        )
        .await;

        // Should get 2 responses (fast + medium), miss slow
        assert_eq!(
            responses.len(),
            2,
            "should receive 2 (fast + medium), miss slow"
        );

        let mut responders: Vec<String> = responses
            .iter()
            .map(|m| {
                serde_json::from_slice::<SearchResponse>(&m.message.payload)
                    .unwrap()
                    .responder
            })
            .collect();
        responders.sort();
        assert_eq!(responders, vec!["fast", "medium"]);

        h0.await.unwrap();
        h1.await.unwrap();
        h2.abort(); // don't wait 5 seconds
    }

    #[tokio::test]
    #[ignore]
    async fn test_scatter_gather_json_roundtrip() {
        let server = NatsTestServer::start(14232).await;
        let client = async_nats::connect(&server.url()).await.unwrap();

        let c = client.clone();
        let mut sub = c.subscribe("nmem.viablesys.search").await.unwrap();
        let responder = tokio::spawn(async move {
            if let Some(msg) = sub.next().await {
                // Deserialize the request
                let req: SearchRequest = serde_json::from_slice(&msg.payload).unwrap();
                assert_eq!(req.query, "episode detection");
                assert_eq!(req.requester, "bob");

                let response = SearchResponse {
                    responder: "alice".into(),
                    results: vec![SearchResult {
                        id: 99,
                        timestamp: 1773543500,
                        obs_type: "file_edit".into(),
                        content_preview: "Modified s4_memory.rs episode detection".into(),
                        file_path: Some("src/s4_memory.rs".into()),
                        session_id: "def-456".into(),
                    }],
                    search_ms: 7,
                };
                if let Some(reply) = msg.reply {
                    c.publish(reply, serde_json::to_vec(&response).unwrap().into())
                        .await
                        .unwrap();
                }
            }
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        let request = SearchRequest {
            query: "episode detection".into(),
            project: Some("nmem".into()),
            obs_type: Some("file_edit".into()),
            limit: 10,
            requester: "bob".into(),
        };

        let responses = scatter_gather(
            &client,
            "nmem.viablesys.search",
            serde_json::to_vec(&request).unwrap().into(),
            Duration::from_secs(2),
        )
        .await;

        assert_eq!(responses.len(), 1);
        let response: SearchResponse = serde_json::from_slice(&responses[0].message.payload).unwrap();
        assert_eq!(response.responder, "alice");
        assert_eq!(response.results.len(), 1);
        assert_eq!(response.results[0].id, 99);
        assert_eq!(
            response.results[0].file_path,
            Some("src/s4_memory.rs".into())
        );

        responder.await.unwrap();
    }

    // --- Group 5: Subject Hierarchy (ADR-015 specific) ---

    #[tokio::test]
    #[ignore]
    async fn test_subject_hierarchy_org_isolation() {
        let server = NatsTestServer::start(14233).await;
        let client = async_nats::connect(&server.url()).await.unwrap();

        let mut viablesys_sub = client.subscribe("nmem.viablesys.>").await.unwrap();

        // viablesys messages — should be received
        client
            .publish("nmem.viablesys.search", "a".into())
            .await
            .unwrap();
        client
            .publish("nmem.viablesys.rag.new", "b".into())
            .await
            .unwrap();
        // acmecorp message — should NOT be received
        client
            .publish("nmem.acmecorp.search", "c".into())
            .await
            .unwrap();
        client.flush().await.unwrap();

        let mut received = Vec::new();
        for _ in 0..2 {
            match tokio::time::timeout(Duration::from_secs(2), viablesys_sub.next()).await {
                Ok(Some(msg)) => {
                    received.push(String::from_utf8(msg.payload.to_vec()).unwrap());
                }
                _ => break,
            }
        }

        assert_eq!(received.len(), 2);
        assert!(received.contains(&"a".to_string()));
        assert!(received.contains(&"b".to_string()));

        // Verify no extra messages leak through
        let extra = tokio::time::timeout(Duration::from_millis(200), viablesys_sub.next()).await;
        assert!(extra.is_err(), "acmecorp message should not be received");
    }

    #[tokio::test]
    #[ignore]
    async fn test_subject_targeted_query() {
        let server = NatsTestServer::start(14234).await;
        let client = async_nats::connect(&server.url()).await.unwrap();

        // Targeted subscription — only alice
        let mut alice_sub = client
            .subscribe("nmem.viablesys.alice.search")
            .await
            .unwrap();
        // Broadcast subscription
        let mut broadcast_sub = client.subscribe("nmem.viablesys.search").await.unwrap();

        // Send targeted message to alice
        client
            .publish("nmem.viablesys.alice.search", "for-alice".into())
            .await
            .unwrap();
        client.flush().await.unwrap();

        // Alice should receive it
        let msg = tokio::time::timeout(Duration::from_secs(2), alice_sub.next())
            .await
            .expect("timeout")
            .expect("alice should receive");
        assert_eq!(msg.payload.as_ref(), b"for-alice");

        // Broadcast should NOT receive it (different subject)
        let broadcast = tokio::time::timeout(Duration::from_millis(200), broadcast_sub.next()).await;
        assert!(
            broadcast.is_err(),
            "broadcast should not receive targeted message"
        );
    }

    #[tokio::test]
    #[ignore]
    async fn test_subject_research_flow() {
        let server = NatsTestServer::start(14235).await;
        let client = async_nats::connect(&server.url()).await.unwrap();

        let mut request_sub = client
            .subscribe("nmem.viablesys.research.request")
            .await
            .unwrap();
        let mut response_sub = client
            .subscribe("nmem.viablesys.research.response.task123")
            .await
            .unwrap();

        // Publish research request
        client
            .publish("nmem.viablesys.research.request", "research-prompt".into())
            .await
            .unwrap();
        client.flush().await.unwrap();

        let req_msg = tokio::time::timeout(Duration::from_secs(2), request_sub.next())
            .await
            .expect("timeout")
            .expect("should receive request");
        assert_eq!(req_msg.payload.as_ref(), b"research-prompt");

        // Publish research response on the task-specific subject
        client
            .publish(
                "nmem.viablesys.research.response.task123",
                "findings".into(),
            )
            .await
            .unwrap();
        client.flush().await.unwrap();

        let resp_msg = tokio::time::timeout(Duration::from_secs(2), response_sub.next())
            .await
            .expect("timeout")
            .expect("should receive response");
        assert_eq!(resp_msg.payload.as_ref(), b"findings");
    }

    // --- Group 6: Edge cases ---

    #[tokio::test]
    #[ignore]
    async fn test_large_payload() {
        let server = NatsTestServer::start(14236).await;
        let client = async_nats::connect(&server.url()).await.unwrap();

        let c = client.clone();
        let mut sub = c.subscribe("nmem.viablesys.search").await.unwrap();
        let responder = tokio::spawn(async move {
            if let Some(msg) = sub.next().await {
                let results: Vec<SearchResult> = (0..20)
                    .map(|i| SearchResult {
                        id: i,
                        timestamp: 1773543500 + i,
                        obs_type: "file_read".into(),
                        content_preview: format!("Content preview for result {i} with padding to reach approximately two hundred characters of content which is typical for real observations stored in nmem database entries as they contain file paths and excerpts"),
                        file_path: Some(format!("src/module_{i}.rs")),
                        session_id: format!("session-{i}"),
                    })
                    .collect();
                let response = SearchResponse {
                    responder: "large-instance".into(),
                    results,
                    search_ms: 15,
                };
                if let Some(reply) = msg.reply {
                    c.publish(reply, serde_json::to_vec(&response).unwrap().into())
                        .await
                        .unwrap();
                }
            }
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        let responses = scatter_gather(
            &client,
            "nmem.viablesys.search",
            Bytes::from("query"),
            Duration::from_secs(2),
        )
        .await;

        assert_eq!(responses.len(), 1);
        let response: SearchResponse = serde_json::from_slice(&responses[0].message.payload).unwrap();
        assert_eq!(response.results.len(), 20);
        assert_eq!(response.responder, "large-instance");

        responder.await.unwrap();
    }

    #[tokio::test]
    #[ignore]
    async fn test_empty_results() {
        let server = NatsTestServer::start(14237).await;
        let client = async_nats::connect(&server.url()).await.unwrap();

        let c = client.clone();
        let mut sub = c.subscribe("nmem.viablesys.search").await.unwrap();
        let responder = tokio::spawn(async move {
            if let Some(msg) = sub.next().await {
                let response = SearchResponse {
                    responder: "empty-instance".into(),
                    results: vec![],
                    search_ms: 0,
                };
                if let Some(reply) = msg.reply {
                    c.publish(reply, serde_json::to_vec(&response).unwrap().into())
                        .await
                        .unwrap();
                }
            }
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        let responses = scatter_gather(
            &client,
            "nmem.viablesys.search",
            Bytes::from("nothing"),
            Duration::from_secs(2),
        )
        .await;

        assert_eq!(responses.len(), 1);
        let response: SearchResponse = serde_json::from_slice(&responses[0].message.payload).unwrap();
        assert!(response.results.is_empty());
        assert_eq!(response.search_ms, 0);

        responder.await.unwrap();
    }

    #[tokio::test]
    #[ignore]
    async fn test_concurrent_requests() {
        let server = NatsTestServer::start(14238).await;
        let client = async_nats::connect(&server.url()).await.unwrap();

        // Spawn a responder that handles multiple requests
        let c = client.clone();
        let mut sub = c.subscribe("nmem.viablesys.concurrent").await.unwrap();
        let responder = tokio::spawn(async move {
            for _ in 0..10 {
                if let Some(msg) = sub.next().await {
                    let payload = String::from_utf8(msg.payload.to_vec()).unwrap();
                    let response = SearchResponse {
                        responder: format!("reply-to-{payload}"),
                        results: vec![],
                        search_ms: 1,
                    };
                    if let Some(reply) = msg.reply {
                        c.publish(reply, serde_json::to_vec(&response).unwrap().into())
                            .await
                            .unwrap();
                    }
                }
            }
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        // Fire 10 requests concurrently
        let mut handles = Vec::new();
        for i in 0..10 {
            let c = client.clone();
            handles.push(tokio::spawn(async move {
                let result = c
                    .request(
                        "nmem.viablesys.concurrent",
                        Bytes::from(format!("{i}")),
                    )
                    .await;
                assert!(result.is_ok(), "request {i} should succeed");
                let response: SearchResponse =
                    serde_json::from_slice(&result.unwrap().payload).unwrap();
                assert_eq!(response.responder, format!("reply-to-{i}"));
            }));
        }

        for h in handles {
            h.await.unwrap();
        }
        responder.await.unwrap();
    }

    // --- Group 7: Heartbeat + calibrated timeout ---

    #[tokio::test]
    #[ignore]
    async fn test_heartbeat_discovers_fleet() {
        let server = NatsTestServer::start(14239).await;
        let client = async_nats::connect(&server.url()).await.unwrap();

        // Spawn 3 instances that respond to heartbeat
        let mut handles = Vec::new();
        for i in 0..3 {
            let c = client.clone();
            let name = format!("node-{i}");
            let mut sub = c.subscribe("nmem.viablesys.heartbeat").await.unwrap();
            handles.push(tokio::spawn(async move {
                if let Some(msg) = sub.next().await {
                    let ping: HeartbeatPing =
                        serde_json::from_slice(&msg.payload).unwrap();
                    let pong = HeartbeatPong {
                        responder: name,
                        echo_ms: ping.sent_ms,
                    };
                    if let Some(reply) = msg.reply {
                        c.publish(reply, serde_json::to_vec(&pong).unwrap().into())
                            .await
                            .unwrap();
                    }
                }
            }));
        }

        tokio::time::sleep(Duration::from_millis(50)).await;

        let fleet = heartbeat(
            &client,
            "nmem.viablesys.heartbeat",
            "test-runner",
            Duration::from_secs(5),
        )
        .await;

        assert_eq!(fleet.instances.len(), 3, "should discover 3 instances");

        let mut names: Vec<String> = fleet.instances.iter().map(|(n, _)| n.clone()).collect();
        names.sort();
        assert_eq!(names, vec!["node-0", "node-1", "node-2"]);

        // Calibrated timeout should be >= 500ms (floor)
        assert!(
            fleet.calibrated_timeout >= Duration::from_millis(500),
            "calibrated timeout should be at least 500ms"
        );

        for h in handles {
            h.await.unwrap();
        }
    }

    #[tokio::test]
    #[ignore]
    async fn test_heartbeat_empty_fleet() {
        let server = NatsTestServer::start(14240).await;
        let client = async_nats::connect(&server.url()).await.unwrap();

        // No instances online — heartbeat should return empty fleet with default timeout
        let fleet = heartbeat(
            &client,
            "nmem.viablesys.heartbeat",
            "lonely-node",
            Duration::from_millis(500),
        )
        .await;

        assert_eq!(fleet.instances.len(), 0, "no instances should respond");
        // No responses: fallback max_rtt=500ms, ×3 = 1500ms, floor=500ms → 1500ms
        assert_eq!(
            fleet.calibrated_timeout,
            Duration::from_millis(1500),
            "should use fallback timeout (500ms × 3) when no responses"
        );
    }

    #[tokio::test]
    #[ignore]
    async fn test_heartbeat_calibrates_timeout() {
        let server = NatsTestServer::start(14241).await;
        let client = async_nats::connect(&server.url()).await.unwrap();

        // Instance 0: responds immediately
        let c0 = client.clone();
        let mut sub0 = c0.subscribe("nmem.viablesys.heartbeat").await.unwrap();
        let h0 = tokio::spawn(async move {
            if let Some(msg) = sub0.next().await {
                let ping: HeartbeatPing =
                    serde_json::from_slice(&msg.payload).unwrap();
                let pong = HeartbeatPong {
                    responder: "fast".into(),
                    echo_ms: ping.sent_ms,
                };
                if let Some(reply) = msg.reply {
                    c0.publish(reply, serde_json::to_vec(&pong).unwrap().into())
                        .await
                        .unwrap();
                }
            }
        });

        // Instance 1: responds after 100ms delay
        let c1 = client.clone();
        let mut sub1 = c1.subscribe("nmem.viablesys.heartbeat").await.unwrap();
        let h1 = tokio::spawn(async move {
            if let Some(msg) = sub1.next().await {
                tokio::time::sleep(Duration::from_millis(100)).await;
                let ping: HeartbeatPing =
                    serde_json::from_slice(&msg.payload).unwrap();
                let pong = HeartbeatPong {
                    responder: "slow".into(),
                    echo_ms: ping.sent_ms,
                };
                if let Some(reply) = msg.reply {
                    c1.publish(reply, serde_json::to_vec(&pong).unwrap().into())
                        .await
                        .unwrap();
                }
            }
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        let fleet = heartbeat(
            &client,
            "nmem.viablesys.heartbeat",
            "calibrator",
            Duration::from_secs(1),
        )
        .await;

        assert_eq!(fleet.instances.len(), 2);

        // Slow instance has ~100ms delay, so max RTT > 100ms
        // Calibrated = max(max_rtt × 3, 500ms), so should be >= 500ms
        assert!(
            fleet.calibrated_timeout >= Duration::from_millis(500),
            "calibrated timeout ({:?}) should be >= 500ms (floor)",
            fleet.calibrated_timeout
        );

        // But not absurdly large — should be well under 3s
        assert!(
            fleet.calibrated_timeout < Duration::from_secs(3),
            "calibrated timeout ({:?}) should be < 3s",
            fleet.calibrated_timeout
        );

        h0.await.unwrap();
        h1.await.unwrap();
    }

    #[tokio::test]
    #[ignore]
    async fn test_heartbeat_then_scatter_gather() {
        let server = NatsTestServer::start(14242).await;
        let client = async_nats::connect(&server.url()).await.unwrap();

        // Spawn 2 instances that respond to both heartbeat and search
        let mut handles = Vec::new();
        for i in 0..2 {
            let c = client.clone();
            let name = format!("fleet-{i}");
            let mut hb_sub = c.subscribe("nmem.viablesys.heartbeat").await.unwrap();
            let mut search_sub = c.subscribe("nmem.viablesys.search").await.unwrap();
            let c2 = c.clone();
            let name2 = name.clone();

            handles.push(tokio::spawn(async move {
                if let Some(msg) = hb_sub.next().await {
                    let ping: HeartbeatPing =
                        serde_json::from_slice(&msg.payload).unwrap();
                    let pong = HeartbeatPong {
                        responder: name,
                        echo_ms: ping.sent_ms,
                    };
                    if let Some(reply) = msg.reply {
                        c.publish(reply, serde_json::to_vec(&pong).unwrap().into())
                            .await
                            .unwrap();
                    }
                }
            }));
            handles.push(tokio::spawn(async move {
                if let Some(msg) = search_sub.next().await {
                    let response = SearchResponse {
                        responder: name2,
                        results: vec![],
                        search_ms: 5,
                    };
                    if let Some(reply) = msg.reply {
                        c2.publish(reply, serde_json::to_vec(&response).unwrap().into())
                            .await
                            .unwrap();
                    }
                }
            }));
        }

        tokio::time::sleep(Duration::from_millis(50)).await;

        // Step 1: Heartbeat
        let fleet = heartbeat(
            &client,
            "nmem.viablesys.heartbeat",
            "orchestrator",
            Duration::from_secs(5),
        )
        .await;

        assert_eq!(fleet.instances.len(), 2, "heartbeat should find 2 instances");

        // Step 2: Scatter/gather with calibrated timeout
        let responses = scatter_gather(
            &client,
            "nmem.viablesys.search",
            Bytes::from("query"),
            fleet.calibrated_timeout,
        )
        .await;

        assert_eq!(
            responses.len(),
            2,
            "scatter/gather with calibrated timeout should get both responses"
        );

        for h in handles {
            h.await.unwrap();
        }
    }

    // --- Group 8: Latency simulation — fleet geography ---

    /// Helper: spawn a heartbeat responder with simulated network delay.
    async fn spawn_delayed_responder(
        client: &async_nats::Client,
        subject: &str,
        name: String,
        delay: Duration,
    ) -> tokio::task::JoinHandle<()> {
        let c = client.clone();
        let mut sub = c.subscribe(subject.to_string()).await.unwrap();
        tokio::spawn(async move {
            if let Some(msg) = sub.next().await {
                tokio::time::sleep(delay).await; // simulate network latency
                let ping: HeartbeatPing =
                    serde_json::from_slice(&msg.payload).unwrap();
                let pong = HeartbeatPong {
                    responder: name,
                    echo_ms: ping.sent_ms,
                };
                if let Some(reply) = msg.reply {
                    c.publish(reply, serde_json::to_vec(&pong).unwrap().into())
                        .await
                        .unwrap();
                }
            }
        })
    }

    #[tokio::test]
    #[ignore]
    async fn test_latency_same_coast() {
        // Two agents on the same coast: ~10-20ms RTT
        let server = NatsTestServer::start(14243).await;
        let client = async_nats::connect(&server.url()).await.unwrap();

        let h0 = spawn_delayed_responder(
            &client, "nmem.viablesys.heartbeat", "sf-office".into(),
            Duration::from_millis(10),
        ).await;
        let h1 = spawn_delayed_responder(
            &client, "nmem.viablesys.heartbeat", "la-home".into(),
            Duration::from_millis(20),
        ).await;

        tokio::time::sleep(Duration::from_millis(50)).await;

        let fleet = heartbeat(
            &client, "nmem.viablesys.heartbeat", "test",
            Duration::from_secs(3),
        ).await;

        assert_eq!(fleet.instances.len(), 2);
        // 20ms × 3 = 60ms, but floor is 500ms
        assert_eq!(fleet.calibrated_timeout, Duration::from_millis(500));

        h0.await.unwrap();
        h1.await.unwrap();
    }

    #[tokio::test]
    #[ignore]
    async fn test_latency_cross_us() {
        // Agents on opposite coasts: ~60-80ms RTT
        let server = NatsTestServer::start(14244).await;
        let client = async_nats::connect(&server.url()).await.unwrap();

        let h0 = spawn_delayed_responder(
            &client, "nmem.viablesys.heartbeat", "nyc-office".into(),
            Duration::from_millis(10),
        ).await;
        let h1 = spawn_delayed_responder(
            &client, "nmem.viablesys.heartbeat", "sf-home".into(),
            Duration::from_millis(80),
        ).await;

        tokio::time::sleep(Duration::from_millis(50)).await;

        let fleet = heartbeat(
            &client, "nmem.viablesys.heartbeat", "test",
            Duration::from_secs(3),
        ).await;

        assert_eq!(fleet.instances.len(), 2);
        // 80ms × 3 = 240ms, but floor is 500ms
        assert_eq!(fleet.calibrated_timeout, Duration::from_millis(500));

        h0.await.unwrap();
        h1.await.unwrap();
    }

    #[tokio::test]
    #[ignore]
    async fn test_latency_global() {
        // Agents on different continents: ~150-300ms RTT
        let server = NatsTestServer::start(14245).await;
        let client = async_nats::connect(&server.url()).await.unwrap();

        let h0 = spawn_delayed_responder(
            &client, "nmem.viablesys.heartbeat", "nyc".into(),
            Duration::from_millis(10),
        ).await;
        let h1 = spawn_delayed_responder(
            &client, "nmem.viablesys.heartbeat", "london".into(),
            Duration::from_millis(150),
        ).await;
        let h2 = spawn_delayed_responder(
            &client, "nmem.viablesys.heartbeat", "tokyo".into(),
            Duration::from_millis(250),
        ).await;

        tokio::time::sleep(Duration::from_millis(50)).await;

        let fleet = heartbeat(
            &client, "nmem.viablesys.heartbeat", "test",
            Duration::from_secs(3),
        ).await;

        assert_eq!(fleet.instances.len(), 3);
        // 250ms × 3 = 750ms, above 500ms floor
        assert!(
            fleet.calibrated_timeout >= Duration::from_millis(750),
            "global fleet timeout ({:?}) should be >= 750ms",
            fleet.calibrated_timeout
        );
        assert!(
            fleet.calibrated_timeout < Duration::from_secs(2),
            "should be bounded"
        );

        h0.await.unwrap();
        h1.await.unwrap();
        h2.await.unwrap();
    }

    #[tokio::test]
    #[ignore]
    async fn test_latency_degraded() {
        // Worst case: global + loaded machine (model loading, GC, slow network)
        let server = NatsTestServer::start(14246).await;
        let client = async_nats::connect(&server.url()).await.unwrap();

        let h0 = spawn_delayed_responder(
            &client, "nmem.viablesys.heartbeat", "fast-local".into(),
            Duration::from_millis(5),
        ).await;
        let h1 = spawn_delayed_responder(
            &client, "nmem.viablesys.heartbeat", "slow-global".into(),
            Duration::from_millis(500),
        ).await;
        let h2 = spawn_delayed_responder(
            &client, "nmem.viablesys.heartbeat", "degraded".into(),
            Duration::from_millis(800),
        ).await;

        tokio::time::sleep(Duration::from_millis(50)).await;

        let fleet = heartbeat(
            &client, "nmem.viablesys.heartbeat", "test",
            Duration::from_secs(5),
        ).await;

        assert_eq!(fleet.instances.len(), 3);
        // 800ms × 3 = 2400ms
        assert!(
            fleet.calibrated_timeout >= Duration::from_secs(2),
            "degraded fleet timeout ({:?}) should be >= 2s",
            fleet.calibrated_timeout
        );
        assert!(
            fleet.calibrated_timeout < Duration::from_secs(5),
            "should be bounded under initial timeout"
        );

        h0.await.unwrap();
        h1.await.unwrap();
        h2.await.unwrap();
    }

    #[tokio::test]
    #[ignore]
    async fn test_latency_one_drops_out() {
        // Global fleet where the slowest node exceeds heartbeat timeout
        let server = NatsTestServer::start(14247).await;
        let client = async_nats::connect(&server.url()).await.unwrap();

        let h0 = spawn_delayed_responder(
            &client, "nmem.viablesys.heartbeat", "fast".into(),
            Duration::from_millis(10),
        ).await;
        let h1 = spawn_delayed_responder(
            &client, "nmem.viablesys.heartbeat", "medium".into(),
            Duration::from_millis(200),
        ).await;
        // This one is too slow — will miss the 1s heartbeat window
        let h2 = spawn_delayed_responder(
            &client, "nmem.viablesys.heartbeat", "unreachable".into(),
            Duration::from_millis(2000),
        ).await;

        tokio::time::sleep(Duration::from_millis(50)).await;

        let fleet = heartbeat(
            &client, "nmem.viablesys.heartbeat", "test",
            Duration::from_secs(1), // tight initial timeout
        ).await;

        // Only 2 instances responded — unreachable missed the window
        assert_eq!(
            fleet.instances.len(), 2,
            "should only see 2 (unreachable misses the 1s window)"
        );

        let names: Vec<&str> = fleet.instances.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"fast"));
        assert!(names.contains(&"medium"));
        assert!(!names.contains(&"unreachable"));

        // Calibrated from medium's ~200ms: 200ms × 3 = 600ms, above 500ms floor
        assert!(
            fleet.calibrated_timeout >= Duration::from_millis(500),
            "timeout ({:?}) should be >= 500ms",
            fleet.calibrated_timeout
        );

        h0.await.unwrap();
        h1.await.unwrap();
        h2.abort(); // don't wait 2s
    }
}
