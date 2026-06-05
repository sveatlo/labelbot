use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("configuration load failed: {0}")]
    Load(#[from] figment::Error),

    #[error("missing required configuration value: {0}")]
    Missing(&'static str),
}

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Sqlx(#[from] sqlx::Error),

    #[error("migration error: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Error)]
pub enum ClassifyError {
    #[error("http request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("api returned status {status}: {body}")]
    Api { status: u16, body: String },

    #[error("could not parse classifier response: {0}")]
    Parse(String),

    #[error("rate limited after {attempts} attempts")]
    RateLimited { attempts: u32 },

    #[error("backend not implemented: {0}")]
    Unimplemented(&'static str),
}

#[derive(Debug, Error)]
pub enum ImapError {
    #[error("imap protocol error: {0}")]
    Imap(#[from] async_imap::error::Error),

    #[error("tls error: {0}")]
    Tls(#[from] native_tls::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("unexpected imap state: {0}")]
    State(String),

    #[error("label mailbox not found and could not be created: {0}")]
    LabelMailbox(String),
}

#[derive(Debug, Error)]
pub enum SummarizeError {
    #[error("inference failed: {0}")]
    Inference(String),

    #[error("summarizer task panicked")]
    TaskPanicked,
}

#[derive(Debug, Error)]
pub enum AppError {
    #[error(transparent)]
    Config(#[from] ConfigError),

    #[error(transparent)]
    Store(#[from] StoreError),

    #[error(transparent)]
    Classify(#[from] ClassifyError),

    #[error(transparent)]
    Imap(#[from] ImapError),
}
