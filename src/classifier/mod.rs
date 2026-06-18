pub mod anthropic;
pub mod openai;

/// The outcome of a classification attempt, from the caller's perspective.
/// Internal error details (HTTP status, JSON parse failures) are absorbed by
/// the adapter and do not cross this seam.
#[derive(Debug)]
pub enum ClassifyOutcome {
    /// Classifier returned a (possibly empty) label list.
    Labels(Vec<String>),
    /// Permanent failure — record the message as processed to avoid looping.
    Terminal,
    /// Temporary failure — skip recording; retry on the next notification.
    Transient,
}

#[async_trait::async_trait]
pub trait LlmClassifier: Send + Sync {
    async fn classify(
        &self,
        subject: &str,
        from_addr: &str,
        body_summary: Option<&str>,
    ) -> ClassifyOutcome;
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
    ) -> ClassifyOutcome {
        ClassifyOutcome::Labels(self.labels.clone())
    }
}
