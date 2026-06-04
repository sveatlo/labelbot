pub mod anthropic;
pub mod openai;

use crate::error::ClassifyError;

pub trait LlmClassifier: Send + Sync {
    fn classify(
        &self,
        subject: &str,
        from_addr: &str,
    ) -> impl std::future::Future<Output = Result<Vec<String>, ClassifyError>> + Send;
}

#[cfg(test)]
#[expect(dead_code)]
pub(crate) struct MockClassifier {
    pub labels: Vec<String>,
}

#[cfg(test)]
impl LlmClassifier for MockClassifier {
    fn classify(
        &self,
        _subject: &str,
        _from_addr: &str,
    ) -> impl std::future::Future<Output = Result<Vec<String>, ClassifyError>> + Send {
        let labels = self.labels.clone();
        async move { Ok(labels) }
    }
}
