use std::sync::Arc;

use anyhow::Context as _;
use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::generation::LogitsProcessor;
use candle_transformers::models::t5;
use hf_hub::api::sync::Api;
use hf_hub::{Repo, RepoType};
use parking_lot::Mutex;
use tokenizers::Tokenizer;

use crate::error::SummarizeError;
use crate::summarizer::Summarizer;

struct T5Inner {
    model: t5::T5ForConditionalGeneration,
    tokenizer: Tokenizer,
}

pub struct T5Summarizer {
    inner: Arc<Mutex<T5Inner>>,
    device: Device,
    t5_config: t5::Config,
    max_input_chars: usize,
    max_output_tokens: usize,
}

impl std::fmt::Debug for T5Summarizer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("T5Summarizer")
            .field("max_input_chars", &self.max_input_chars)
            .field("max_output_tokens", &self.max_output_tokens)
            .finish()
    }
}

impl T5Summarizer {
    pub async fn load(
        model_id: String,
        max_input_chars: usize,
        max_output_tokens: usize,
    ) -> anyhow::Result<Self> {
        tokio::task::spawn_blocking(move || {
            Self::load_sync(model_id, max_input_chars, max_output_tokens)
        })
        .await
        .context("model load task panicked")?
    }

    fn load_sync(
        model_id: String,
        max_input_chars: usize,
        max_output_tokens: usize,
    ) -> anyhow::Result<Self> {
        tracing::info!(%model_id, "downloading/loading T5 model");

        let device = Device::Cpu;

        let api = Api::new().context("create HF Hub API client")?;
        let repo = api.repo(Repo::with_revision(
            model_id.clone(),
            RepoType::Model,
            "main".to_owned(),
        ));

        let config_path = repo.get("config.json").context("fetch config.json")?;
        let tokenizer_path = repo.get("tokenizer.json").context("fetch tokenizer.json")?;
        let weights_path = repo
            .get("model.safetensors")
            .context("fetch model.safetensors")?;

        let config_str = std::fs::read_to_string(&config_path).context("read config.json")?;
        let mut t5_config: t5::Config =
            serde_json::from_str(&config_str).context("parse config.json")?;
        t5_config.use_cache = true;

        let tokenizer = Tokenizer::from_file(&tokenizer_path)
            .map_err(|e| anyhow::anyhow!("load tokenizer: {e}"))?;

        // SAFETY: mmap is safe as long as the file is not modified or truncated while mapped.
        let vb = unsafe {
            VarBuilder::from_mmaped_safetensors(&[weights_path], DType::F32, &device)
                .context("load model weights")?
        };
        let model =
            t5::T5ForConditionalGeneration::load(vb, &t5_config).context("build T5 model")?;

        tracing::info!(%model_id, "T5 summarizer ready");

        Ok(Self {
            inner: Arc::new(Mutex::new(T5Inner { model, tokenizer })),
            device,
            t5_config,
            max_input_chars,
            max_output_tokens,
        })
    }
}

#[async_trait::async_trait]
impl Summarizer for T5Summarizer {
    async fn summarize(&self, text: &str) -> Result<String, SummarizeError> {
        let inner = Arc::clone(&self.inner);
        let device = self.device.clone();
        let t5_config = self.t5_config.clone();
        let max_output_tokens = self.max_output_tokens;
        let truncated: String = text.chars().take(self.max_input_chars).collect();

        tokio::task::spawn_blocking(move || {
            let mut guard = inner.lock();
            let T5Inner {
                ref mut model,
                ref tokenizer,
            } = *guard;
            run_inference(model, tokenizer, &t5_config, &device, &truncated, max_output_tokens)
                .map_err(|e| SummarizeError::Inference(e.to_string()))
        })
        .await
        .map_err(|_| SummarizeError::TaskPanicked)?
    }
}

fn run_inference(
    model: &mut t5::T5ForConditionalGeneration,
    tokenizer: &Tokenizer,
    config: &t5::Config,
    device: &Device,
    text: &str,
    max_output_tokens: usize,
) -> anyhow::Result<String> {
    model.clear_kv_cache();

    let input = format!("summarize: {text}");
    let encoding = tokenizer
        .encode(input, true)
        .map_err(|e| anyhow::anyhow!("tokenize: {e}"))?;
    let input_ids = encoding.get_ids();
    let input_tensor = Tensor::new(input_ids, device)?.unsqueeze(0)?;

    let encoder_output = model.encode(&input_tensor)?;

    let start_token = config.decoder_start_token_id.unwrap_or(0) as u32;
    let eos_token = config.eos_token_id as u32;

    let mut output_token_ids: Vec<u32> = vec![start_token];
    let mut logits_processor = LogitsProcessor::new(0, None, None);

    for _ in 0..max_output_tokens {
        let decoder_input = if config.use_cache {
            let last = *output_token_ids
                .last()
                .ok_or_else(|| anyhow::anyhow!("BUG: empty output token ids"))?;
            Tensor::new(&[last], device)?.unsqueeze(0)?
        } else {
            Tensor::new(output_token_ids.as_slice(), device)?.unsqueeze(0)?
        };

        let logits = model.decode(&decoder_input, &encoder_output)?.squeeze(0)?;
        let next_token = logits_processor.sample(&logits)?;

        if next_token == eos_token {
            break;
        }
        output_token_ids.push(next_token);
    }

    let summary = tokenizer
        .decode(&output_token_ids[1..], true)
        .map_err(|e| anyhow::anyhow!("decode output: {e}"))?;

    Ok(summary)
}
