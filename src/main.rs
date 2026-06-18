use anyhow::Context;
use clap::Parser;
use labelbot::classifier::LlmClassifier;
use labelbot::classifier::anthropic::AnthropicClassifier;
use labelbot::classifier::openai::OpenAiClassifier;
use labelbot::summarizer::Summarizer;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,labelbot=info")),
        )
        .init();

    tracing::info!("labelbot starting");

    let cli = labelbot::cli::Cli::parse();

    let cfg = labelbot::config::Config::load(cli.config)?;
    tracing::info!(
        imap_host = %cfg.imap.host,
        imap_port = %cfg.imap.port,
        backfill_days = %cfg.backfill_max_age_days,
        labels = ?cfg.label_set().names(),
        "config loaded",
    );

    let label_names: Vec<String> = cfg.label_set().names().to_vec();

    let classifier: Box<dyn LlmClassifier> = match &cfg.classifier {
        labelbot::config::ClassifierConfig::OpenAI {
            api_key,
            base_url,
            model,
        } => Box::new(OpenAiClassifier::new(
            base_url.to_string(),
            api_key.clone(),
            model.clone(),
            label_names,
        )),
        labelbot::config::ClassifierConfig::Anthropic { api_key, model } => Box::new(
            AnthropicClassifier::new(api_key.clone(), model.clone(), label_names),
        ),
    };

    let summarizer: Box<dyn Summarizer> = match &cfg.summarizer {
        labelbot::config::SummarizerConfig::Truncate { max_chars } => {
            tracing::info!(%max_chars, "using truncate summarizer");
            Box::new(labelbot::summarizer::truncate::TruncateSummarizer::new(
                *max_chars,
            )) as Box<dyn Summarizer>
        }
    };

    labelbot::daemon::run(cfg, classifier, summarizer)
        .await
        .context("daemon exited with error")?;

    tracing::info!("labelbot shut down cleanly");
    Ok(())
}
