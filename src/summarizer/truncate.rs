use crate::error::SummarizeError;
use crate::summarizer::Summarizer;

#[derive(Debug)]
pub struct TruncateSummarizer {
    max_chars: usize,
}

impl TruncateSummarizer {
    #[must_use] 
    pub fn new(max_chars: usize) -> Self {
        Self { max_chars }
    }
}

#[async_trait::async_trait]
impl Summarizer for TruncateSummarizer {
    async fn summarize(&self, text: &str) -> Result<String, SummarizeError> {
        let clean = crate::util::strip_html_tags(text);
        Ok(clean.chars().take(self.max_chars).collect())
    }
}
