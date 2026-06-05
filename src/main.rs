use anyhow::Context;
use clap::Parser;
use labelbot::classifier::LlmClassifier;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,labelbot=debug")),
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
        } => Box::new(labelbot::classifier::openai::OpenAiClassifier::new(
            base_url.to_string(),
            api_key.clone(),
            model.clone(),
            label_names,
        )),
        labelbot::config::ClassifierConfig::Anthropic { api_key, model } => {
            Box::new(labelbot::classifier::anthropic::AnthropicClassifier::new(
                api_key.clone(),
                model.clone(),
                label_names,
            ))
        }
    };

    labelbot::daemon::run(cfg, classifier)
        .await
        .context("daemon exited with error")?;

    tracing::info!("labelbot shut down cleanly");
    Ok(())
}
