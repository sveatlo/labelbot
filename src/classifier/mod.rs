pub mod anthropic;
pub mod openai;

use crate::error::ClassifyError;

#[async_trait::async_trait]
pub trait LlmClassifier: Send + Sync {
    async fn classify(
        &self,
        subject: &str,
        from_addr: &str,
        body_summary: Option<&str>,
    ) -> Result<Vec<String>, ClassifyError>;
}

#[cfg(test)]
#[expect(dead_code)]
pub(crate) struct MockClassifier {
    pub labels: Vec<String>,
}

#[cfg(test)]
#[async_trait::async_trait]
impl LlmClassifier for MockClassifier {
    async fn classify(
        &self,
        _subject: &str,
        _from_addr: &str,
        _body_summary: Option<&str>,
    ) -> Result<Vec<String>, ClassifyError> {
        Ok(self.labels.clone())
    }
}
