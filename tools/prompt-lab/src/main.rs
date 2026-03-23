//! Prompt Lab — data-driven prompt engineering for nmem summarization.
//!
//! Extracts real session payloads from the nmem database, runs multiple
//! prompt variants through the local inference engine, and scores outputs
//! with automated metrics. No human judgment needed.
//!
//! Usage:
//!   cargo run --release -- --sessions 5
//!   cargo run --release --features rocm -- --sessions 10 --prompts-dir ./prompts

use nmem::db::open_db_readonly;
use nmem::s1_4_inference::{self, InferenceEngine};
use nmem::s1_4_summarize::gather_session_payload;
use serde::Deserialize;
use std::path::{Path, PathBuf};

// --- Config ---

fn default_db_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join(".nmem").join("nmem.db")
}

fn default_prompts_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("prompts")
}

// --- Prompt variant ---

#[derive(Debug, Deserialize)]
struct PromptVariant {
    name: String,
    #[allow(dead_code)]
    description: String,
    system_prompt: String,
    user_template: String,
}

// --- Scoring ---

#[derive(Debug, Default, Clone)]
struct Score {
    json_valid: bool,
    fields_present: u8,        // out of 7
    intent_words: usize,
    learned_count: usize,
    learned_avg_chars: usize,
    completed_count: usize,
    next_steps_count: usize,
    files_read_count: usize,
    files_edited_count: usize,
    array_compliance: u8,      // out of 5 array fields
    notes_present: bool,
    prompt_tokens: usize,
    generated_tokens: usize,
    generation_ms: u64,
}

fn score_output(raw: &str, result: &s1_4_inference::GenerateResult) -> Score {
    let mut s = Score {
        prompt_tokens: result.prompt_tokens,
        generated_tokens: result.generated_tokens,
        generation_ms: result.total_ms,
        ..Default::default()
    };

    let cleaned = match extract_json(raw) {
        Some(j) => j,
        None => return s,
    };
    let parsed: serde_json::Value = match serde_json::from_str(cleaned) {
        Ok(v) => { s.json_valid = true; v }
        Err(_) => return s,
    };

    let obj = match parsed.as_object() {
        Some(o) => o,
        None => return s,
    };

    // Field presence
    let expected = ["intent", "learned", "completed", "next_steps", "files_read", "files_edited", "notes"];
    s.fields_present = expected.iter().filter(|f| obj.contains_key(**f)).count() as u8;

    // Intent quality
    if let Some(intent) = obj.get("intent").and_then(|v| v.as_str()) {
        s.intent_words = intent.split_whitespace().count();
    }

    // Array fields
    let array_fields = ["learned", "completed", "next_steps", "files_read", "files_edited"];
    for field in &array_fields {
        if let Some(val) = obj.get(*field) {
            if val.is_array() {
                s.array_compliance += 1;
            }
        }
    }

    // learned detail
    if let Some(arr) = obj.get("learned").and_then(|v| v.as_array()) {
        s.learned_count = arr.len();
        let total_chars: usize = arr.iter()
            .filter_map(|v| v.as_str())
            .map(|s| s.len())
            .sum();
        s.learned_avg_chars = if arr.is_empty() { 0 } else { total_chars / arr.len() };
    }

    if let Some(arr) = obj.get("completed").and_then(|v| v.as_array()) {
        s.completed_count = arr.len();
    }
    if let Some(arr) = obj.get("next_steps").and_then(|v| v.as_array()) {
        s.next_steps_count = arr.len();
    }
    if let Some(arr) = obj.get("files_read").and_then(|v| v.as_array()) {
        s.files_read_count = arr.len();
    }
    if let Some(arr) = obj.get("files_edited").and_then(|v| v.as_array()) {
        s.files_edited_count = arr.len();
    }

    // notes presence
    if let Some(notes) = obj.get("notes") {
        s.notes_present = !notes.is_null() && notes.as_str().map(|s| !s.is_empty()).unwrap_or(true);
    }

    s
}

/// Extract the first balanced JSON object from raw model output.
/// Handles: markdown fences, trailing explanation text, leading preamble.
fn extract_json(text: &str) -> Option<&str> {
    // Strip fences first
    let t = text.trim();
    let t = if let Some(rest) = t.strip_prefix("```") {
        let rest = rest.trim_start_matches(|c: char| c != '\n').trim_start_matches('\n');
        rest.strip_suffix("```").unwrap_or(rest).trim()
    } else {
        t
    };

    // Find first '{' and match to its closing '}'
    let start = t.find('{')?;
    let bytes = t.as_bytes();
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape = false;

    for i in start..bytes.len() {
        let b = bytes[i];
        if escape {
            escape = false;
            continue;
        }
        match b {
            b'\\' if in_string => escape = true,
            b'"' => in_string = !in_string,
            b'{' if !in_string => depth += 1,
            b'}' if !in_string => {
                depth -= 1;
                if depth == 0 {
                    return Some(&t[start..=i]);
                }
            }
            _ => {}
        }
    }
    None // unbalanced
}

// --- Aggregation ---

#[derive(Debug, Default)]
struct Aggregate {
    name: String,
    n: usize,
    json_valid: usize,
    avg_fields: f64,
    avg_intent_words: f64,
    avg_learned_count: f64,
    avg_learned_chars: f64,
    avg_completed: f64,
    avg_next_steps: f64,
    avg_array_compliance: f64,
    notes_present: usize,
    avg_generation_ms: f64,
    avg_prompt_tokens: f64,
    avg_generated_tokens: f64,
}

fn aggregate(name: &str, scores: &[Score]) -> Aggregate {
    let n = scores.len();
    if n == 0 {
        return Aggregate { name: name.to_string(), ..Default::default() };
    }
    let nf = n as f64;
    Aggregate {
        name: name.to_string(),
        n,
        json_valid: scores.iter().filter(|s| s.json_valid).count(),
        avg_fields: scores.iter().map(|s| s.fields_present as f64).sum::<f64>() / nf,
        avg_intent_words: scores.iter().map(|s| s.intent_words as f64).sum::<f64>() / nf,
        avg_learned_count: scores.iter().map(|s| s.learned_count as f64).sum::<f64>() / nf,
        avg_learned_chars: scores.iter().map(|s| s.learned_avg_chars as f64).sum::<f64>() / nf,
        avg_completed: scores.iter().map(|s| s.completed_count as f64).sum::<f64>() / nf,
        avg_next_steps: scores.iter().map(|s| s.next_steps_count as f64).sum::<f64>() / nf,
        avg_array_compliance: scores.iter().map(|s| s.array_compliance as f64).sum::<f64>() / nf,
        notes_present: scores.iter().filter(|s| s.notes_present).count(),
        avg_generation_ms: scores.iter().map(|s| s.generation_ms as f64).sum::<f64>() / nf,
        avg_prompt_tokens: scores.iter().map(|s| s.prompt_tokens as f64).sum::<f64>() / nf,
        avg_generated_tokens: scores.iter().map(|s| s.generated_tokens as f64).sum::<f64>() / nf,
    }
}

// --- Session selection ---

struct SessionRow {
    id: String,
    project: String,
    obs_count: i64,
}

fn select_sessions(
    conn: &rusqlite::Connection,
    limit: usize,
) -> Result<Vec<SessionRow>, nmem::NmemError> {
    let mut stmt = conn.prepare(
        "SELECT s.id, s.project,
                (SELECT COUNT(*) FROM observations WHERE session_id = s.id) as obs_count
         FROM sessions s
         WHERE s.summary IS NOT NULL
           AND s.ended_at IS NOT NULL
         ORDER BY s.started_at DESC
         LIMIT ?1",
    )?;

    let rows = stmt
        .query_map([limit as i64], |row| {
            Ok(SessionRow {
                id: row.get(0)?,
                project: row.get(1)?,
                obs_count: row.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(rows)
}

// --- Output ---

fn print_comparison(aggregates: &[Aggregate], details: &[(String, Vec<(String, Score, String)>)]) {
    println!("\n# Prompt Lab Results\n");
    println!("## Aggregate Comparison\n");
    println!("| Metric | {} |",
        aggregates.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(" | "));
    println!("| ------ | {} |",
        aggregates.iter().map(|_| "------").collect::<Vec<_>>().join(" | "));

    macro_rules! row {
        ($label:expr, $field:ident, $fmt:expr) => {
            println!("| {} | {} |", $label,
                aggregates.iter().map(|a| format!($fmt, a.$field)).collect::<Vec<_>>().join(" | "));
        };
    }

    row!("Sessions", n, "{}");
    println!("| JSON valid | {} |",
        aggregates.iter().map(|a| format!("{}/{}", a.json_valid, a.n)).collect::<Vec<_>>().join(" | "));
    row!("Avg fields (of 7)", avg_fields, "{:.1}");
    row!("Avg intent words", avg_intent_words, "{:.1}");
    row!("Avg learned count", avg_learned_count, "{:.1}");
    row!("Avg learned chars", avg_learned_chars, "{:.0}");
    row!("Avg completed", avg_completed, "{:.1}");
    row!("Avg next_steps", avg_next_steps, "{:.1}");
    row!("Array compliance (of 5)", avg_array_compliance, "{:.1}");
    row!("Notes present", notes_present, "{}");
    row!("Avg generation ms", avg_generation_ms, "{:.0}");
    row!("Avg prompt tokens", avg_prompt_tokens, "{:.0}");
    row!("Avg generated tokens", avg_generated_tokens, "{:.0}");

    // Per-session detail
    println!("\n## Per-Session Detail\n");
    for (session_id, variants) in details {
        println!("### Session: {session_id}\n");
        for (variant_name, score, intent) in variants {
            println!("**{variant_name}** — valid={}, fields={}/7, intent_words={}, learned={}, array={}/5, {}ms",
                score.json_valid, score.fields_present, score.intent_words,
                score.learned_count, score.array_compliance, score.generation_ms);
            println!("  intent: {intent}");
        }
        println!();
    }
}

// --- Main ---

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();

    let mut db_path = default_db_path();
    let mut prompts_dir = default_prompts_dir();
    let mut n_sessions: usize = 5;
    let mut output_path: Option<PathBuf> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--db" => { i += 1; db_path = PathBuf::from(&args[i]); }
            "--prompts-dir" => { i += 1; prompts_dir = PathBuf::from(&args[i]); }
            "--sessions" => { i += 1; n_sessions = args[i].parse()?; }
            "--output" | "-o" => { i += 1; output_path = Some(PathBuf::from(&args[i])); }
            "--help" | "-h" => {
                eprintln!("prompt-lab [--db PATH] [--sessions N] [--prompts-dir DIR] [-o FILE]");
                std::process::exit(0);
            }
            other => {
                eprintln!("unknown arg: {other}");
                std::process::exit(1);
            }
        }
        i += 1;
    }

    // Load prompt variants
    let variants = load_variants(&prompts_dir)?;
    eprintln!("loaded {} prompt variants from {}", variants.len(), prompts_dir.display());

    // Open DB read-only
    let conn = open_db_readonly(&db_path)?;
    eprintln!("opened {}", db_path.display());

    // Select sessions
    let sessions = select_sessions(&conn, n_sessions)?;
    eprintln!("selected {} sessions with summaries", sessions.len());
    for s in &sessions {
        eprintln!("  {} ({}, {} obs)", s.id, s.project, s.obs_count);
    }

    // Extract payloads
    let mut payloads: Vec<(String, String)> = Vec::new(); // (session_id, payload)
    for s in &sessions {
        match gather_session_payload(&conn, &s.id)? {
            Some(p) => payloads.push((s.id.clone(), p)),
            None => eprintln!("  skipping {} (too few observations)", s.id),
        }
    }
    eprintln!("{} payloads extracted\n", payloads.len());

    // Load inference engine once (shared across all variants — same model)
    let config = nmem::s5_config::load_config()
        .unwrap_or_default()
        .summarization;
    let inference_params = s1_4_inference::params_from_config(&config)?;
    eprintln!("loading model: {}", inference_params.model_path.display());
    let engine = InferenceEngine::new(inference_params)?;
    eprintln!("model loaded\n");

    // Run each variant against each payload
    let mut all_scores: Vec<(String, Vec<Score>)> = Vec::new(); // (variant_name, scores)
    let mut details: Vec<(String, Vec<(String, Score, String)>)> = Vec::new(); // per-session

    // Initialize detail entries
    for (sid, _) in &payloads {
        details.push((sid.clone(), Vec::new()));
    }

    for variant in &variants {
        eprintln!("--- variant: {} ---", variant.name);
        let mut scores = Vec::new();

        for (idx, (sid, payload)) in payloads.iter().enumerate() {
            let user_content = variant.user_template.replace("{PAYLOAD}", payload);

            let result = engine.generate(&variant.system_prompt, &user_content)?;
            let score = score_output(&result.text, &result);

            // Extract intent for detail view
            let intent = if score.json_valid {
                extract_json(&result.text)
                    .and_then(|j| serde_json::from_str::<serde_json::Value>(j).ok())
                    .and_then(|v| v.get("intent")?.as_str().map(String::from))
                    .unwrap_or_default()
            } else {
                format!("[PARSE FAILED] {}", &result.text[..result.text.len().min(80)])
            };

            eprintln!(
                "  {}: valid={} fields={}/7 intent_words={} learned={} array={}/5 {}ms",
                &sid[..8], score.json_valid, score.fields_present, score.intent_words,
                score.learned_count, score.array_compliance, score.generation_ms,
            );

            scores.push(score.clone());
            details[idx].1.push((variant.name.clone(), score, intent));
        }

        all_scores.push((variant.name.clone(), scores));
    }

    // Aggregate and print
    let aggregates: Vec<Aggregate> = all_scores
        .iter()
        .map(|(name, scores)| aggregate(name, scores))
        .collect();

    print_comparison(&aggregates, &details);

    // Write to file if requested
    if let Some(path) = &output_path {
        // Re-generate to file by capturing via redirect isn't clean,
        // so just write a summary JSON instead
        let summary = serde_json::json!({
            "variants": aggregates.iter().map(|a| serde_json::json!({
                "name": a.name,
                "n": a.n,
                "json_valid": a.json_valid,
                "avg_fields": a.avg_fields,
                "avg_intent_words": a.avg_intent_words,
                "avg_learned_count": a.avg_learned_count,
                "avg_learned_chars": a.avg_learned_chars,
                "avg_array_compliance": a.avg_array_compliance,
                "avg_generation_ms": a.avg_generation_ms,
            })).collect::<Vec<_>>(),
        });
        std::fs::write(path, serde_json::to_string_pretty(&summary)?)?;
        eprintln!("\nresults written to {}", path.display());
    }

    Ok(())
}

fn load_variants(dir: &Path) -> Result<Vec<PromptVariant>, Box<dyn std::error::Error>> {
    let mut variants = Vec::new();
    let mut entries: Vec<_> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "toml"))
        .collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let content = std::fs::read_to_string(entry.path())?;
        let variant: PromptVariant = toml::from_str(&content)?;
        variants.push(variant);
    }
    Ok(variants)
}
