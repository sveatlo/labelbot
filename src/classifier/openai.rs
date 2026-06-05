use crate::classifier::LlmClassifier;
use crate::error::ClassifyError;
use rand::RngExt;
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

const MAX_RETRIES: u32 = 5;

#[derive(Debug)]
pub struct OpenAiClassifier {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
    labels: Vec<String>,
}

impl OpenAiClassifier {
    pub fn new(base_url: String, api_key: String, model: String, labels: Vec<String>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("reqwest client build");

        OpenAiClassifier {
            client,
            base_url,
            api_key,
            model,
            labels,
        }
    }
}

impl LlmClassifier for OpenAiClassifier {
    fn classify<'a>(
        &'a self,
        subject: &'a str,
        from_addr: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, ClassifyError>> + Send + 'a>> {
        let body = build_request_body(&self.model, subject, from_addr, &self.labels);
        let known: Vec<String> = self.labels.clone();
        let base_url = self.base_url.clone();
        let api_key = self.api_key.clone();
        let client = self.client.clone();

        Box::pin(async move {
            for attempt in 1..=MAX_RETRIES {
                match execute(&client, &base_url, &api_key, &body, &known).await {
                    Ok(labels) => return Ok(labels),
                    Err(ClassifyError::RateLimited { .. }) if attempt < MAX_RETRIES => {
                        tokio::time::sleep(backoff_duration(attempt)).await;
                    }
                    Err(ClassifyError::Api { status, .. })
                        if status >= 500 && attempt < MAX_RETRIES =>
                    {
                        tokio::time::sleep(backoff_duration(attempt)).await;
                    }
                    Err(e) => return Err(e),
                }
            }

            Err(ClassifyError::RateLimited {
                attempts: MAX_RETRIES,
            })
        })
    }
}

async fn execute(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    body: &Value,
    known_labels: &[String],
) -> Result<Vec<String>, ClassifyError> {
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));

    tracing::debug!(%url, "sending classification request");

    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .json(body)
        .send()
        .await?;

    let status = resp.status();
    tracing::debug!(status = %status, "classification response status");

    if status == 429 {
        return Err(ClassifyError::RateLimited { attempts: 0 });
    }
    if !status.is_success() {
        let body_text = resp.text().await.unwrap_or_default();
        tracing::warn!(%status, body = %body_text, "classification API error");
        return Err(ClassifyError::Api {
            status: status.as_u16(),
            body: body_text,
        });
    }

    let value: Value = resp.json().await?;
    parse_response(&value, known_labels)
}

fn build_request_body(model: &str, subject: &str, from_addr: &str, labels: &[String]) -> Value {
    let label_enum: Vec<Value> = labels.iter().map(|l| Value::String(l.clone())).collect();

    let user_content = format!(
        "From: {from_addr}\nSubject: {subject}\n\nClassify this email into one or more of the defined labels."
    );

    serde_json::json!({
        "model": model,
        "messages": [
            {
                "role": "user",
                "content": user_content
            }
        ],
        "tools": [
            {
                "type": "function",
                "function": {
                    "name": "classify_email",
                    "description": "Classify an email into zero or more predefined labels",
                    "strict": true,
                    "parameters": {
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
            }
        ],
        "tool_choice": {
            "type": "function",
            "function": {
                "name": "classify_email"
            }
        }
    })
}

fn parse_response(value: &Value, known_labels: &[String]) -> Result<Vec<String>, ClassifyError> {
    let choices = value
        .get("choices")
        .and_then(|c| c.as_array())
        .ok_or_else(|| ClassifyError::Parse("missing choices array".into()))?;

    let first_choice = choices
        .first()
        .ok_or_else(|| ClassifyError::Parse("empty choices array".into()))?;

    let message = first_choice
        .get("message")
        .ok_or_else(|| ClassifyError::Parse("choice missing message".into()))?;

    // Try tool_calls first (native function calling)
    if let Some(tool_calls) = message.get("tool_calls").and_then(|t| t.as_array()) {
        return parse_tool_calls(tool_calls, known_labels);
    }

    // Fallback: parse content text as JSON (for models that don't support native tool calls)
    #[expect(clippy::collapsible_if)]
    if let Some(content) = message.get("content").and_then(|c| c.as_str()) {
        if let Ok(parsed) = serde_json::from_str::<Value>(content)
            && let Some(labels) = parsed.get("labels").and_then(|l| l.as_array())
        {
            return labels
                .iter()
                .filter_map(|v| v.as_str())
                .map(|s| canonicalize(s, known_labels))
                .collect();
        }
    }

    Err(ClassifyError::Parse(
        "response has no tool_calls and content did not contain valid JSON labels".into(),
    ))
}

fn parse_tool_calls(
    tool_calls: &[Value],
    known_labels: &[String],
) -> Result<Vec<String>, ClassifyError> {
    let first_call = tool_calls
        .first()
        .ok_or_else(|| ClassifyError::Parse("empty tool_calls array".into()))?;

    let function = first_call
        .get("function")
        .ok_or_else(|| ClassifyError::Parse("tool_call missing function".into()))?;

    if let Some(name) = function.get("name").and_then(|n| n.as_str())
        && name != "classify_email"
    {
        return Err(ClassifyError::Parse(format!(
            "unexpected function name: {name}"
        )));
    }

    let arguments_str = function
        .get("arguments")
        .and_then(|a| a.as_str())
        .ok_or_else(|| ClassifyError::Parse("function missing arguments string".into()))?;

    let arguments_value: Value = serde_json::from_str(arguments_str)
        .map_err(|e| ClassifyError::Parse(format!("invalid arguments JSON: {e}")))?;

    let label_names = arguments_value
        .get("labels")
        .and_then(|l| l.as_array())
        .ok_or_else(|| ClassifyError::Parse("arguments missing labels array".into()))?;

    label_names
        .iter()
        .filter_map(|v| v.as_str())
        .map(|s| canonicalize(s, known_labels))
        .collect()
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

    fn make_classifier(server: &MockServer) -> OpenAiClassifier {
        OpenAiClassifier::new(
            server.uri(),
            "test-key".into(),
            "gpt-4o-mini".into(),
            default_labels(),
        )
    }

    fn function_call_response(labels: &[&str]) -> Value {
        let labels_val: Vec<Value> = labels
            .iter()
            .map(|l| Value::String((*l).to_owned()))
            .collect();
        let args = serde_json::json!({ "labels": labels_val }).to_string();
        serde_json::json!({
            "id": "chatcmpl_123",
            "object": "chat.completion",
            "created": 1234567890,
            "model": "gpt-4o-mini",
            "choices": [
                {
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [
                            {
                                "id": "call_123",
                                "type": "function",
                                "function": {
                                    "name": "classify_email",
                                    "arguments": args
                                }
                            }
                        ]
                    },
                    "finish_reason": "tool_calls"
                }
            ],
            "usage": {
                "prompt_tokens": 50,
                "completion_tokens": 10,
                "total_tokens": 60
            }
        })
    }

    #[tokio::test]
    async fn classify_returns_labels() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(function_call_response(&["Work", "Finance"])),
            )
            .mount(&server)
            .await;

        let classifier = make_classifier(&server);
        let body = build_request_body("gpt-4o-mini", "test", "from@x.com", &default_labels());
        let labels = execute(
            &classifier.client,
            &server.uri(),
            "test-key",
            &body,
            &default_labels(),
        )
        .await
        .unwrap();
        assert_eq!(labels, vec!["Work".to_owned(), "Finance".to_owned()]);
    }

    #[tokio::test]
    async fn classify_returns_empty_labels() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(function_call_response(&[])))
            .mount(&server)
            .await;

        let classifier = make_classifier(&server);
        let body = build_request_body("gpt-4o-mini", "test", "from@x.com", &default_labels());
        let labels = execute(
            &classifier.client,
            &server.uri(),
            "test-key",
            &body,
            &default_labels(),
        )
        .await
        .unwrap();
        assert!(labels.is_empty());
    }

    #[tokio::test]
    async fn api_error_returns_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(400).set_body_string("bad request"))
            .mount(&server)
            .await;

        let classifier = make_classifier(&server);
        let body = build_request_body("gpt-4o-mini", "test", "from@x.com", &default_labels());
        let err = execute(
            &classifier.client,
            &server.uri(),
            "test-key",
            &body,
            &default_labels(),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ClassifyError::Api { status: 400, .. }));
    }

    #[tokio::test]
    async fn parse_function_call_extracts_labels() {
        let value = function_call_response(&["Work", "Personal"]);
        let labels = parse_response(&value, &default_labels()).unwrap();
        assert_eq!(labels, vec!["Work".to_owned(), "Personal".to_owned()]);
    }

    #[tokio::test]
    async fn parse_empty_labels() {
        let value = function_call_response(&[]);
        let labels = parse_response(&value, &default_labels()).unwrap();
        assert!(labels.is_empty());
    }

    #[test]
    fn parse_invalid_response_missing_choices() {
        let value = serde_json::json!({"id": "chatcmpl_1"});
        let err = parse_response(&value, &default_labels()).unwrap_err();
        assert!(matches!(err, ClassifyError::Parse(_)));
    }

    #[test]
    fn parse_rejects_unknown_label() {
        let value = function_call_response(&["Spam"]);
        let err = parse_response(&value, &default_labels()).unwrap_err();
        assert!(matches!(err, ClassifyError::Parse(_)));
    }

    #[test]
    fn parse_json_content_fallback() {
        // Some models (e.g., llama3.2:1b) return JSON in content text instead of tool_calls
        let value = serde_json::json!({
            "id": "chatcmpl_1",
            "object": "chat.completion",
            "created": 123,
            "model": "llama3.2:1b",
            "choices": [
                {
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "{\"labels\": [\"Work\", \"Finance\"]}"
                    },
                    "finish_reason": "stop"
                }
            ]
        });
        let labels = parse_response(&value, &default_labels()).unwrap();
        assert_eq!(labels, vec!["Work".to_owned(), "Finance".to_owned()]);
    }

    #[test]
    fn parse_json_content_empty_labels() {
        let value = serde_json::json!({
            "id": "chatcmpl_1",
            "object": "chat.completion",
            "created": 123,
            "model": "llama3.2:1b",
            "choices": [
                {
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "{\"labels\": []}"
                    },
                    "finish_reason": "stop"
                }
            ]
        });
        let labels = parse_response(&value, &default_labels()).unwrap();
        assert!(labels.is_empty());
    }

    #[test]
    fn parse_json_content_not_json_falls_through() {
        let value = serde_json::json!({
            "id": "chatcmpl_1",
            "object": "chat.completion",
            "created": 123,
            "model": "llama3.2:1b",
            "choices": [
                {
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "I don't know how to classify this"
                    },
                    "finish_reason": "stop"
                }
            ]
        });
        let err = parse_response(&value, &default_labels()).unwrap_err();
        assert!(matches!(err, ClassifyError::Parse(_)));
    }

    #[tokio::test]
    async fn rate_limited_returns_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(429))
            .mount(&server)
            .await;

        let classifier = make_classifier(&server);
        let body = build_request_body("gpt-4o-mini", "test", "from@x.com", &default_labels());
        let err = execute(
            &classifier.client,
            &server.uri(),
            "test-key",
            &body,
            &default_labels(),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ClassifyError::RateLimited { .. }));
    }

    #[tokio::test]
    async fn server_error_returns_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(500).set_body_string("internal error"))
            .mount(&server)
            .await;

        let classifier = make_classifier(&server);
        let body = build_request_body("gpt-4o-mini", "test", "from@x.com", &default_labels());
        let err = execute(
            &classifier.client,
            &server.uri(),
            "test-key",
            &body,
            &default_labels(),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ClassifyError::Api { status: 500, .. }));
    }

    #[tokio::test]
    async fn uses_correct_url_path() {
        let server = MockServer::start().await;

        // The URI from wiremock is http://127.0.0.1:PORT, without a path.
        // execute() appends /chat/completions, so our mock must match that.
        let base_url = format!("{}/v1", server.uri());
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(function_call_response(&[])))
            .mount(&server)
            .await;

        let classifier = OpenAiClassifier::new(
            base_url.clone(),
            "test-key".into(),
            "gpt-4o-mini".into(),
            default_labels(),
        );
        let body = build_request_body("gpt-4o-mini", "test", "from@x.com", &default_labels());
        let result = execute(
            &classifier.client,
            &base_url,
            "test-key",
            &body,
            &default_labels(),
        )
        .await;
        assert!(result.is_ok());
    }
}
