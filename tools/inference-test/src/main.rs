//! Micro-prototype for ADR-016: Direct Inference.
//!
//! Validates llama-cpp-2 integration against GGUF models for nmem summarization.
//!
//! Usage:
//!   # Local model file
//!   cargo run --release -- model.gguf
//!
//!   # Auto-download from HuggingFace (repo:file format)
//!   cargo run --release -- lmstudio-community/granite-4.0-h-tiny-GGUF:granite-4.0-h-tiny-Q4_K_M.gguf
//!
//!   # With GPU and custom context
//!   cargo run --release -- model.gguf --gpu --n-ctx 32768 --temp 0.3 --payload session.txt

use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaChatMessage, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;
use std::io::Write;
use std::time::Instant;

// --- Prompts (same as nmem's s1_4_summarize.rs) ---

const SYSTEM_PROMPT: &str =
    "You produce structured JSON summaries of coding sessions for an AI agent's cross-session memory. The consumer is the next AI session, not a human. Optimize for context reconstruction. Return ONLY valid JSON, no markdown, no explanation.";

const USER_PROMPT_TEMPLATE: &str = r#"Summarize this coding session for the next AI agent session. The summary will be injected as context so the next session can continue the work without re-deriving conclusions.

Return JSON with these fields:

- "intent": What was being accomplished (string)
- "learned": Decisions made, trade-offs evaluated, constraints discovered (array of strings)
- "completed": What was done (array of strings)
- "next_steps": What logically follows (array of strings)
- "files_read": File paths that were read (array of strings)
- "files_edited": File paths that were modified (array of strings)
- "notes": Errors encountered, failed approaches (string or null)

Session data:
{PAYLOAD}

Return ONLY the JSON object."#;

// --- Simulated session payload ---

const SAMPLE_PAYLOAD: &str = r#"User prompts:
- fix the unsummarized sessions bug in nmem
- check which sessions are missing summaries
- the stop hook should trigger summarization

Agent reasoning:
- The user wants to investigate why 13 sessions have no summaries. The Stop hook spawns a background maintain process that calls LM Studio for summarization. If LM Studio isn't running, summarization fails silently.
- Found the root cause: spawn_deferred_maintain discards the spawn result with `let _ = cmd.spawn()`. No retry, no record of failure.

Actions:
[command|think|diverge|internal|routine] nmem status
[command|act|converge|internal|routine] sqlite3 ~/.nmem/nmem.db "SELECT COUNT(*) FROM sessions WHERE summary IS NULL"
[file_read|think|diverge|internal|routine] src/s1_record.rs
[file_read|think|diverge|internal|routine] src/s3_maintain.rs
[file_read|think|diverge|internal|routine] src/s1_4_summarize.rs
[search|think|diverge|internal|routine] summariz in src/
[command|think|converge|internal|routine] curl -s http://localhost:1234/v1/models
[file_write|act|converge|internal|novel] design/ADR/ADR-016-Direct-Inference.md
[file_edit|act|converge|internal|novel] design/ADR/ADR-016-Direct-Inference.md
[git_commit|act|converge|internal|routine] Add ADR-016: Direct Inference
"#;

// --- Deserialization target (matches nmem's SessionSummary) ---

#[derive(Debug, serde::Deserialize)]
struct SessionSummary {
    intent: String,
    #[serde(default, deserialize_with = "string_or_vec")]
    learned: Vec<String>,
    #[serde(default, deserialize_with = "string_or_vec")]
    completed: Vec<String>,
    #[serde(default, deserialize_with = "string_or_vec")]
    next_steps: Vec<String>,
    #[serde(default, deserialize_with = "string_or_vec")]
    files_read: Vec<String>,
    #[serde(default, deserialize_with = "string_or_vec")]
    files_edited: Vec<String>,
    #[serde(default)]
    notes: serde_json::Value,
}

/// Accept either a JSON string or array of strings.
fn string_or_vec<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de;

    struct StringOrVec;
    impl<'de> de::Visitor<'de> for StringOrVec {
        type Value = Vec<String>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("string or array of strings")
        }
        fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
            Ok(vec![v.to_owned()])
        }
        fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut out = Vec::new();
            while let Some(s) = seq.next_element()? {
                out.push(s);
            }
            Ok(out)
        }
    }
    deserializer.deserialize_any(StringOrVec)
}

/// Strip markdown code fences from LLM response.
fn strip_fences(text: &str) -> &str {
    let t = text.trim();
    if let Some(rest) = t.strip_prefix("```") {
        let rest = rest.trim_start_matches(|c: char| c != '\n').trim_start_matches('\n');
        if let Some(inner) = rest.strip_suffix("```") {
            return inner.trim();
        }
        return rest.trim();
    }
    t
}

/// Resolve a model specifier to a local path.
/// - If the path exists on disk, use it directly.
/// - If it contains ':', treat as `repo:filename` and download from HuggingFace.
/// - Downloads are cached by hf-hub (~/.cache/huggingface/hub/).
fn resolve_model(specifier: &str) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    let path = std::path::Path::new(specifier);
    if path.exists() {
        return Ok(path.to_path_buf());
    }

    // Parse repo:filename format
    let (repo_id, filename) = specifier.split_once(':')
        .ok_or_else(|| format!("Model not found: {specifier}\nUse repo:filename for HuggingFace download (e.g. lmstudio-community/granite-4.0-h-tiny-GGUF:granite-4.0-h-tiny-Q4_K_M.gguf)"))?;

    eprintln!("Model not found locally, downloading from HuggingFace...");
    eprintln!("  Repo: {repo_id}");
    eprintln!("  File: {filename}");

    let api = hf_hub::api::sync::ApiBuilder::new()
        .with_progress(true)
        .build()?;
    let repo = api.model(repo_id.to_string());
    let local_path = repo.get(filename)?;

    eprintln!("  Cached: {}", local_path.display());
    Ok(local_path)
}

fn parse_arg<T: std::str::FromStr>(args: &[String], flag: &str, default: T) -> T {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: inference-test <model.gguf> [--gpu] [--n-ctx N] [--max-tokens N] [--temp F] [--payload FILE]");
        std::process::exit(1);
    }

    let model_specifier = &args[1];
    let model_path = resolve_model(model_specifier)?;
    let model_path_str = model_path.display().to_string();
    let use_gpu = args.iter().any(|a| a == "--gpu");
    let n_ctx: u32 = parse_arg(&args, "--n-ctx", 4096);
    let max_tokens: usize = parse_arg(&args, "--max-tokens", 1024);
    let temperature: f32 = parse_arg(&args, "--temp", 0.0);
    let payload_path: Option<String> = args.iter()
        .position(|a| a == "--payload")
        .and_then(|i| args.get(i + 1))
        .cloned();

    eprintln!("=== ADR-016 Inference Prototype ===");
    eprintln!("Model:      {model_path_str}");
    eprintln!("GPU:        {use_gpu}");
    eprintln!("n_ctx:      {n_ctx}");
    eprintln!("max_tokens: {max_tokens}");
    eprintln!("temperature:{temperature}");
    eprintln!();

    // --- Step 1: Backend ---
    let t_total = Instant::now();
    let backend = LlamaBackend::init()?;
    eprintln!("[1/6] Backend init: {:?}", t_total.elapsed());

    // --- Step 2: Model ---
    let t1 = Instant::now();
    let mut model_params = LlamaModelParams::default();
    if use_gpu {
        model_params = model_params.with_n_gpu_layers(999);
        eprintln!("       GPU: offloading all layers");
    }
    let model = LlamaModel::load_from_file(&backend, &model_path_str, &model_params)?;
    eprintln!("[2/6] Model load:   {:?}", t1.elapsed());

    // --- Step 3: Chat template + prompt ---
    let t2 = Instant::now();
    let template = model.chat_template(None)
        .map_err(|e| format!("No chat template in GGUF: {e:?}"))?;

    let payload = match &payload_path {
        Some(path) => {
            eprintln!("       Payload: {path}");
            std::fs::read_to_string(path)?
        }
        None => {
            eprintln!("       Payload: built-in sample");
            SAMPLE_PAYLOAD.to_string()
        }
    };
    let user_content = USER_PROMPT_TEMPLATE.replace("{PAYLOAD}", &payload);
    let messages = vec![
        LlamaChatMessage::new("system".into(), SYSTEM_PROMPT.into())?,
        LlamaChatMessage::new("user".into(), user_content)?,
    ];

    let formatted = model.apply_chat_template(&template, &messages, true)?;
    eprintln!("[3/6] Chat template + format: {:?} ({} chars)", t2.elapsed(), formatted.len());

    // --- Step 4: Context ---
    let t3 = Instant::now();
    let n_threads = std::thread::available_parallelism()
        .map(|n| (n.get() as i32 / 2).max(1))
        .unwrap_or(4);
    let ctx_params = LlamaContextParams::default()
        .with_n_ctx(std::num::NonZeroU32::new(n_ctx))
        .with_n_threads(n_threads)
        .with_n_threads_batch(n_threads * 2);
    let mut ctx = model.new_context(&backend, ctx_params)?;
    eprintln!("[4/6] Context:      {:?} (n_threads={n_threads}, n_ctx={n_ctx})", t3.elapsed());

    // --- Step 5: Tokenize + decode prompt ---
    let t4 = Instant::now();
    let tokens = model.str_to_token(&formatted, AddBos::Always)?;
    let n_prompt_tokens = tokens.len();

    if n_prompt_tokens as u32 >= n_ctx {
        return Err(format!(
            "Prompt ({n_prompt_tokens} tokens) exceeds n_ctx ({n_ctx}). Increase --n-ctx."
        ).into());
    }

    // Decode prompt in chunks of n_batch (2048) to avoid exceeding batch limit
    let n_batch = 2048usize;
    let mut batch = LlamaBatch::new(n_batch, 1);
    let mut decoded = 0usize;

    while decoded < tokens.len() {
        batch.clear();
        let chunk_end = (decoded + n_batch).min(tokens.len());
        for i in decoded..chunk_end {
            let is_last = i == tokens.len() - 1;
            batch.add(tokens[i], i as i32, &[0], is_last)?;
        }
        ctx.decode(&mut batch)?;
        decoded = chunk_end;
    }
    let prompt_eval_time = t4.elapsed();
    eprintln!("[5/6] Prompt eval:  {prompt_eval_time:?} ({n_prompt_tokens} tokens, {:.1} tok/s)",
        n_prompt_tokens as f64 / prompt_eval_time.as_secs_f64());

    // --- Step 6: Generate ---
    let t5 = Instant::now();
    let mut sampler = if temperature == 0.0 {
        eprintln!("       Sampling: greedy (temp=0.0)");
        LlamaSampler::greedy()
    } else {
        eprintln!("       Sampling: temp={temperature}");
        LlamaSampler::chain_simple([
            LlamaSampler::temp(temperature),
            LlamaSampler::dist(42),
        ])
    };

    let mut output = String::new();
    let mut n_generated = 0usize;
    let mut pos = n_prompt_tokens as i32;
    let mut decoder = encoding_rs::UTF_8.new_decoder();

    for _ in 0..max_tokens {
        let new_token = sampler.sample(&ctx, batch.n_tokens() - 1);
        sampler.accept(new_token);

        if model.is_eog_token(new_token) {
            break;
        }

        let token_str = model.token_to_piece(new_token, &mut decoder, true, None)?;
        output.push_str(&token_str);
        n_generated += 1;

        // Stream to stderr for progress visibility
        eprint!("{token_str}");
        std::io::stderr().flush().ok();

        batch.clear();
        batch.add(new_token, pos, &[0], true)?;
        pos += 1;
        ctx.decode(&mut batch)?;
    }

    let gen_time = t5.elapsed();
    eprintln!();
    eprintln!("[6/6] Generation:   {gen_time:?} ({n_generated} tokens, {:.1} tok/s)",
        n_generated as f64 / gen_time.as_secs_f64());

    let total = t_total.elapsed();
    let total_ms = total.as_millis();

    // --- Results ---
    eprintln!();
    eprintln!("=== Timing ===");
    eprintln!("Model load:  {:?}", t1.elapsed());
    eprintln!("Prompt eval: {prompt_eval_time:?} ({n_prompt_tokens} tokens)");
    eprintln!("Generation:  {gen_time:?} ({n_generated} tokens)");
    eprintln!("Total:       {total:?} ({total_ms}ms)");
    eprintln!();

    // --- Parse output ---
    let cleaned = strip_fences(&output);

    match serde_json::from_str::<SessionSummary>(cleaned) {
        Ok(summary) => {
            eprintln!("=== Parse: SUCCESS ===");
            eprintln!("intent:       {}", summary.intent);
            eprintln!("learned:      {} entries", summary.learned.len());
            eprintln!("completed:    {} entries", summary.completed.len());
            eprintln!("next_steps:   {} entries", summary.next_steps.len());
            eprintln!("files_read:   {:?}", summary.files_read);
            eprintln!("files_edited: {:?}", summary.files_edited);
            for l in &summary.learned {
                eprintln!("  learned: {l}");
            }

            // Write valid JSON to stdout (pipeable)
            println!("{}", serde_json::to_string_pretty(&serde_json::json!({
                "intent": summary.intent,
                "learned": summary.learned,
                "completed": summary.completed,
                "next_steps": summary.next_steps,
                "files_read": summary.files_read,
                "files_edited": summary.files_edited,
                "notes": summary.notes,
                "_meta": {
                    "model": model_path_str,
                    "gpu": use_gpu,
                    "n_ctx": n_ctx,
                    "temperature": temperature,
                    "prompt_tokens": n_prompt_tokens,
                    "generated_tokens": n_generated,
                    "total_ms": total_ms,
                    "prompt_eval_ms": prompt_eval_time.as_millis(),
                    "generation_ms": gen_time.as_millis(),
                }
            }))?);
        }
        Err(e) => {
            eprintln!("=== Parse: FAILED ===");
            eprintln!("Error: {e}");
            eprintln!();
            eprintln!("Raw output:");
            eprintln!("{cleaned}");
            std::process::exit(1);
        }
    }

    Ok(())
}
