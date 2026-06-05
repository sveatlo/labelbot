pub mod t5;
pub mod truncate;

use crate::error::SummarizeError;

#[async_trait::async_trait]
pub trait Summarizer: Send + Sync {
    async fn summarize(&self, text: &str) -> Result<String, SummarizeError>;
}
