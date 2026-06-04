use anyhow::Context;
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

    let cfg = labelbot::config::Config::load()?;
    tracing::info!(
        imap_host = %cfg.imap_host,
        imap_port = %cfg.imap_port,
        classifier_backend = %cfg.classifier_backend,
        "config loaded",
    );

    let label_names: Vec<String> = cfg.label_set().names().to_vec();

    let classifier: Box<dyn LlmClassifier> = match cfg.classifier_backend.as_str() {
        "openai" => Box::new(labelbot::classifier::openai::OpenAiClassifier::new(
            cfg.openai_base_url.clone(),
            cfg.openai_api_key.clone(),
            cfg.openai_model.clone(),
            label_names,
        )),
        _ => Box::new(labelbot::classifier::anthropic::AnthropicClassifier::new(
            cfg.anthropic_api_key.clone(),
            cfg.anthropic_model.clone(),
            label_names,
        )),
    };

    labelbot::daemon::run(cfg, classifier)
        .await
        .context("daemon exited with error")?;

    tracing::info!("labelbot shut down cleanly");
    Ok(())
}
