use crate::classifier::{ClassifyOutcome, LlmClassifier};
use crate::error::ClassifyError;
use rand::RngExt;
use serde_json::Value;
use std::time::Duration;

const ANTHROPIC_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const MAX_RETRIES: u32 = 5;

#[derive(Debug)]
pub struct AnthropicClassifier {
    client: reqwest::Client,
    api_key: String,
    model: String,
    base_url: String,
    labels: Vec<String>,
}

impl AnthropicClassifier {
    #[must_use] 
    pub fn new(api_key: String, model: String, labels: Vec<String>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("reqwest client build");

        AnthropicClassifier {
            client,
            api_key,
            model,
            base_url: ANTHROPIC_URL.to_owned(),
            labels,
        }
    }

    #[cfg(test)]
    pub fn with_base_url(
        api_key: String,
        model: String,
        base_url: String,
        labels: Vec<String>,
    ) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("reqwest client build");

        AnthropicClassifier {
            client,
            api_key,
            model,
            base_url,
            labels,
        }
    }
}

#[async_trait::async_trait]
impl LlmClassifier for AnthropicClassifier {
    async fn classify(
        &self,
        subject: &str,
        from_addr: &str,
        body_summary: Option<&str>,
    ) -> ClassifyOutcome {
        let body = build_request_body(&self.model, subject, from_addr, body_summary, &self.labels);

        for attempt in 1..=MAX_RETRIES {
            match self.execute(&body, &self.labels).await {
                Ok(labels) => return ClassifyOutcome::Labels(labels),
                Err(ClassifyError::RateLimited { .. }) if attempt < MAX_RETRIES => {
                    tokio::time::sleep(backoff_duration(attempt)).await;
                }
                Err(ClassifyError::Api { status, .. })
                    if status >= 500 && attempt < MAX_RETRIES =>
                {
                    tokio::time::sleep(backoff_duration(attempt)).await;
                }
                Err(e @ (ClassifyError::Parse(_) | ClassifyError::Unimplemented(_))) => {
                    tracing::warn!(error=%e, "permanent classification failure");
                    return ClassifyOutcome::Terminal;
                }
                Err(e) => {
                    tracing::warn!(error=%e, attempt, "transient classification failure");
                    return ClassifyOutcome::Transient;
                }
            }
        }

        tracing::warn!(attempts = MAX_RETRIES, "all retries exhausted");
        ClassifyOutcome::Transient
    }
}

impl AnthropicClassifier {
    async fn execute(
        &self,
        body: &Value,
        known_labels: &[String],
    ) -> Result<Vec<String>, ClassifyError> {
        let resp = self
            .client
            .post(&self.base_url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .json(body)
            .send()
            .await?;

        let status = resp.status();
        if status == 429 {
            return Err(ClassifyError::RateLimited { attempts: 0 });
        }
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            return Err(ClassifyError::Api {
                status: status.as_u16(),
                body: body_text,
            });
        }

        let value: Value = resp.json().await?;
        parse_tool_use_response(&value, known_labels)
    }
}

fn build_request_body(
    model: &str,
    subject: &str,
    from_addr: &str,
    body_summary: Option<&str>,
    labels: &[String],
) -> Value {
    let label_enum: Vec<Value> = labels.iter().map(|l| Value::String(l.clone())).collect();

    let user_content = if let Some(summary) = body_summary {
        format!("From: {from_addr}\nSubject: {subject}\nBody summary: {summary}\n\nClassify this email into one or more of the defined labels.")
    } else {
        format!("From: {from_addr}\nSubject: {subject}\n\nClassify this email into one or more of the defined labels.")
    };

    serde_json::json!({
        "model": model,
        "max_tokens": 256,
        "messages": [
            {
                "role": "user",
                "content": user_content
            }
        ],
        "tools": [
            {
                "name": "classify_email",
                "description": "Classify an email into zero or more predefined labels",
                "strict": true,
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "labels": {
                            "type": "array",
                            "items": {
                                "type": "string",
                                "enum": label_enum
                            },
                            "description": "Zero or more labels from the fixed set"
                        }
                    },
                    "required": ["labels"],
                    "additionalProperties": false
                }
            }
        ],
        "tool_choice": {
            "type": "tool",
            "name": "classify_email"
        }
    })
}

fn parse_tool_use_response(
    value: &Value,
    known_labels: &[String],
) -> Result<Vec<String>, ClassifyError> {
    let content = value
        .get("content")
        .and_then(|c| c.as_array())
        .ok_or_else(|| ClassifyError::Parse("missing content array".into()))?;

    for block in content {
        if block.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
            if block.get("name").and_then(|n| n.as_str()) != Some("classify_email") {
                continue;
            }
            let input = block
                .get("input")
                .ok_or_else(|| ClassifyError::Parse("tool_use missing input".into()))?;
            let label_names = input
                .get("labels")
                .and_then(|l| l.as_array())
                .ok_or_else(|| {
                    ClassifyError::Parse("tool_use input missing labels array".into())
                })?;

            return label_names
                .iter()
                .filter_map(|v| v.as_str())
                .map(|s| canonicalize(s, known_labels))
                .collect();
        }
    }

    Err(ClassifyError::Parse("no tool_use block in response".into()))
}

fn canonicalize(s: &str, known: &[String]) -> Result<String, ClassifyError> {
    let trimmed = s.trim();
    known
        .iter()
        .find(|n| n.eq_ignore_ascii_case(trimmed))
        .cloned()
        .ok_or_else(|| ClassifyError::Parse(format!("unknown label: {s}")))
}

fn backoff_duration(attempt: u32) -> Duration {
    // Exponential backoff: 1s, 2s, 4s, 8s, 16s, ... plus jitter up to 100%
    let base_secs = 2_u64.pow(attempt.saturating_sub(1));
    let jitter: u64 = rand::rng().random_range(0..=base_secs);
    Duration::from_secs(base_secs + jitter)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn default_labels() -> Vec<String> {
        ["Work", "Finance", "Personal", "House"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect()
    }

    fn make_classifier(server: &MockServer) -> AnthropicClassifier {
        AnthropicClassifier::with_base_url(
            "test-key".into(),
            "claude-haiku-4-5".into(),
            format!("{}/v1/messages", server.uri()),
            default_labels(),
        )
    }

    fn tool_use_response(labels: &[&str]) -> Value {
        let labels_val: Vec<Value> = labels
            .iter()
            .map(|l| Value::String((*l).to_owned()))
            .collect();
        serde_json::json!({
            "id": "msg_123",
            "type": "message",
            "role": "assistant",
            "content": [
                {
                    "type": "tool_use",
                    "id": "tu_123",
                    "name": "classify_email",
                    "input": {
                        "labels": labels_val
                    }
                }
            ],
            "model": "claude-haiku-4-5",
            "stop_reason": "tool_use",
            "usage": {
                "input_tokens": 50,
                "output_tokens": 10
            }
        })
    }

    #[tokio::test]
    async fn classify_returns_labels() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(tool_use_response(&["Work", "Finance"])),
            )
            .mount(&server)
            .await;

        let classifier = make_classifier(&server);
        let body = build_request_body("haiku", "test", "from@x.com", None, &default_labels());
        let labels = classifier.execute(&body, &default_labels()).await.unwrap();
        assert_eq!(labels, vec!["Work".to_owned(), "Finance".to_owned()]);
    }

    #[tokio::test]
    async fn classify_returns_empty_list() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(tool_use_response(&[])))
            .mount(&server)
            .await;

        let classifier = make_classifier(&server);
        let body = build_request_body("haiku", "test", "from@x.com", None, &default_labels());
        let labels = classifier.execute(&body, &default_labels()).await.unwrap();
        assert!(labels.is_empty());
    }

    #[tokio::test]
    async fn api_error_returns_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(400).set_body_string("bad request"))
            .mount(&server)
            .await;

        let classifier = make_classifier(&server);
        let body = build_request_body("haiku", "test", "from@x.com", None, &default_labels());
        let err = classifier
            .execute(&body, &default_labels())
            .await
            .unwrap_err();
        assert!(matches!(err, ClassifyError::Api { status: 400, .. }));
    }

    #[tokio::test]
    async fn parse_tool_use_extracts_labels() {
        let value = tool_use_response(&["Work", "Personal"]);
        let labels = parse_tool_use_response(&value, &default_labels()).unwrap();
        assert_eq!(labels, vec!["Work".to_owned(), "Personal".to_owned()]);
    }

    #[tokio::test]
    async fn parse_empty_labels() {
        let value = tool_use_response(&[]);
        let labels = parse_tool_use_response(&value, &default_labels()).unwrap();
        assert!(labels.is_empty());
    }

    #[test]
    fn parse_invalid_response_missing_content() {
        let value = serde_json::json!({"id": "msg_1"});
        let err = parse_tool_use_response(&value, &default_labels()).unwrap_err();
        assert!(matches!(err, ClassifyError::Parse(_)));
    }

    #[test]
    fn parse_rejects_unknown_label() {
        let value = tool_use_response(&["Spam"]);
        let err = parse_tool_use_response(&value, &default_labels()).unwrap_err();
        assert!(matches!(err, ClassifyError::Parse(_)));
    }
}
