pub mod anthropic;
pub mod openai;

use crate::error::ClassifyError;
use std::future::Future;
use std::pin::Pin;

pub trait LlmClassifier: Send + Sync {
    fn classify<'a>(
        &'a self,
        subject: &'a str,
        from_addr: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, ClassifyError>> + Send + 'a>>;
}

#[cfg(test)]
#[expect(dead_code)]
pub(crate) struct MockClassifier {
    pub labels: Vec<String>,
}

#[cfg(test)]
impl LlmClassifier for MockClassifier {
    fn classify<'a>(
        &'a self,
        _subject: &'a str,
        _from_addr: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, ClassifyError>> + Send + 'a>> {
        let labels = self.labels.clone();
        Box::pin(async move { Ok(labels) })
    }
}
