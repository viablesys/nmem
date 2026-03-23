//! S1's S4 — Generative LLM inference engine.
//!
//! Loads a GGUF model via llama-cpp-2, applies the chat template from GGUF
//! metadata, and generates text. Each call loads the model, infers once, and
//! exits — no persistent model state.
//!
//! This is distinct from `s2_inference.rs` (TF-IDF/LinearSVC statistical
//! classification). Different tools, different VSM layer purpose.

use crate::NmemError;
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaChatMessage, LlamaChatTemplate, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// All inference parameters — constructed by each task from its config section.
/// Model-specific settings (temperature, n_ctx, n_gpu_layers) live here,
/// not hardcoded in the engine.
pub struct InferenceParams {
    pub model_path: PathBuf,
    pub temperature: f32,
    pub n_ctx: u32,
    pub max_tokens: u32,
    pub n_threads: u32,
    pub n_gpu_layers: u32,
}

/// Result of a generation call, includes timing metadata.
pub struct GenerateResult {
    pub text: String,
    pub total_ms: u64,
    pub prompt_tokens: usize,
    pub generated_tokens: usize,
}

/// Resolve a model specifier to a local path.
/// - If the path exists on disk, use it directly.
/// - If it contains ':', treat as `repo:filename` and download from HuggingFace.
/// - Downloads are cached by hf-hub (~/.cache/huggingface/hub/).
pub fn resolve_model(specifier: &str) -> Result<PathBuf, NmemError> {
    let path = Path::new(specifier);
    if path.exists() {
        return Ok(path.to_path_buf());
    }

    let (repo_id, filename) = specifier.split_once(':').ok_or_else(|| {
        NmemError::Config(format!(
            "model not found: {specifier} (use repo:filename for HuggingFace download)"
        ))
    })?;

    log::info!("downloading model from HuggingFace ({repo_id}/{filename})...");

    let api = hf_hub::api::sync::ApiBuilder::new()
        .with_progress(true)
        .build()
        .map_err(|e| NmemError::Config(format!("hf-hub init: {e}")))?;
    let repo = api.model(repo_id.to_string());
    let local_path = repo
        .get(filename)
        .map_err(|e| NmemError::Config(format!("hf-hub download: {e}")))?;

    log::info!("model cached at {}", local_path.display());
    Ok(local_path)
}

/// Loaded model that can generate multiple times without reloading.
/// Backend + model stay in memory; each `generate()` creates a fresh context.
pub struct InferenceEngine {
    backend: LlamaBackend,
    model: LlamaModel,
    template: LlamaChatTemplate,
    params: InferenceParams,
    n_threads: i32,
}

impl InferenceEngine {
    /// Load backend + model from disk. This is the expensive step (~1.5-2s).
    pub fn new(params: InferenceParams) -> Result<Self, NmemError> {
        let backend = LlamaBackend::init()
            .map_err(|e| NmemError::Config(format!("llama backend init: {e}")))?;

        let model_path_str = params.model_path.display().to_string();
        let mut model_params = LlamaModelParams::default();
        if params.n_gpu_layers > 0 {
            model_params = model_params.with_n_gpu_layers(params.n_gpu_layers);
        }
        let model = LlamaModel::load_from_file(&backend, &model_path_str, &model_params)
            .map_err(|e| NmemError::Config(format!("model load: {e}")))?;

        let template = model
            .chat_template(None)
            .map_err(|e| NmemError::Config(format!("no chat template in GGUF: {e:?}")))?;

        let n_threads = if params.n_threads == 0 {
            std::thread::available_parallelism()
                .map(|n| (n.get() as i32 / 2).max(1))
                .unwrap_or(4)
        } else {
            params.n_threads as i32
        };

        Ok(Self { backend, model, template, params, n_threads })
    }

    /// Generate text from a system + user prompt pair.
    /// Creates a fresh context per call (cheap vs model load).
    pub fn generate(
        &self,
        system_prompt: &str,
        user_prompt: &str,
    ) -> Result<GenerateResult, NmemError> {
        let t_total = Instant::now();

        let messages = vec![
            LlamaChatMessage::new("system".into(), system_prompt.into())
                .map_err(|e| NmemError::Config(format!("chat message: {e}")))?,
            LlamaChatMessage::new("user".into(), user_prompt.into())
                .map_err(|e| NmemError::Config(format!("chat message: {e}")))?,
        ];

        let formatted = self.model
            .apply_chat_template(&self.template, &messages, true)
            .map_err(|e| NmemError::Config(format!("apply chat template: {e}")))?;

        let ctx_params = LlamaContextParams::default()
            .with_n_ctx(std::num::NonZeroU32::new(self.params.n_ctx))
            .with_n_threads(self.n_threads)
            .with_n_threads_batch(self.n_threads * 2);
        let mut ctx = self.model
            .new_context(&self.backend, ctx_params)
            .map_err(|e| NmemError::Config(format!("context creation: {e}")))?;

        let tokens = self.model
            .str_to_token(&formatted, AddBos::Always)
            .map_err(|e| NmemError::Config(format!("tokenization: {e}")))?;
        let n_prompt_tokens = tokens.len();

        if n_prompt_tokens as u32 >= self.params.n_ctx {
            return Err(NmemError::Config(format!(
                "prompt ({n_prompt_tokens} tokens) exceeds n_ctx ({})",
                self.params.n_ctx
            )));
        }

        // Decode prompt in chunks of n_batch (2048)
        let n_batch = 2048usize;
        let mut batch = LlamaBatch::new(n_batch, 1);
        let mut decoded = 0usize;

        while decoded < tokens.len() {
            batch.clear();
            let chunk_end = (decoded + n_batch).min(tokens.len());
            for i in decoded..chunk_end {
                let is_last = i == tokens.len() - 1;
                batch
                    .add(tokens[i], i as i32, &[0], is_last)
                    .map_err(|e| NmemError::Config(format!("batch add: {e}")))?;
            }
            ctx.decode(&mut batch)
                .map_err(|e| NmemError::Config(format!("decode: {e}")))?;
            decoded = chunk_end;
        }

        let mut sampler = if self.params.temperature == 0.0 {
            LlamaSampler::greedy()
        } else {
            LlamaSampler::chain_simple([
                LlamaSampler::temp(self.params.temperature),
                LlamaSampler::dist(42),
            ])
        };

        let mut output = String::new();
        let mut n_generated = 0usize;
        let mut pos = n_prompt_tokens as i32;
        let mut decoder = encoding_rs::UTF_8.new_decoder();

        for _ in 0..self.params.max_tokens {
            let new_token = sampler.sample(&ctx, batch.n_tokens() - 1);
            sampler.accept(new_token);

            if self.model.is_eog_token(new_token) {
                break;
            }

            let token_str = self.model
                .token_to_piece(new_token, &mut decoder, true, None)
                .map_err(|e| NmemError::Config(format!("token_to_piece: {e}")))?;
            output.push_str(&token_str);
            n_generated += 1;

            batch.clear();
            batch
                .add(new_token, pos, &[0], true)
                .map_err(|e| NmemError::Config(format!("batch add: {e}")))?;
            pos += 1;
            ctx.decode(&mut batch)
                .map_err(|e| NmemError::Config(format!("decode: {e}")))?;
        }

        let total_ms = t_total.elapsed().as_millis() as u64;

        Ok(GenerateResult {
            text: output,
            total_ms,
            prompt_tokens: n_prompt_tokens,
            generated_tokens: n_generated,
        })
    }
}

/// One-shot generate: load model, infer, drop. Convenience for single-session use.
pub fn generate(
    params: &InferenceParams,
    system_prompt: &str,
    user_prompt: &str,
) -> Result<GenerateResult, NmemError> {
    let engine = InferenceEngine::new(InferenceParams {
        model_path: params.model_path.clone(),
        temperature: params.temperature,
        n_ctx: params.n_ctx,
        max_tokens: params.max_tokens,
        n_threads: params.n_threads,
        n_gpu_layers: params.n_gpu_layers,
    })?;
    engine.generate(system_prompt, user_prompt)
}

/// Build InferenceParams from SummarizationConfig, resolving the model path.
pub fn params_from_config(
    config: &crate::s5_config::SummarizationConfig,
) -> Result<InferenceParams, NmemError> {
    let model_path = resolve_model(&config.model_path)?;
    Ok(InferenceParams {
        model_path,
        temperature: config.temperature,
        n_ctx: config.n_ctx,
        max_tokens: config.max_tokens,
        n_threads: config.n_threads,
        n_gpu_layers: config.n_gpu_layers,
    })
}
