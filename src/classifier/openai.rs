use crate::classifier::LlmClassifier;
use crate::error::ClassifyError;
use rand::Rng;
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

    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
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

    let tool_calls = message
        .get("tool_calls")
        .and_then(|t| t.as_array())
        .ok_or_else(|| ClassifyError::Parse("message missing tool_calls array".into()))?;

    let first_call = tool_calls
        .first()
        .ok_or_else(|| ClassifyError::Parse("empty tool_calls array".into()))?;

    let function = first_call
        .get("function")
        .ok_or_else(|| ClassifyError::Parse("tool_call missing function".into()))?;

    let name = function
        .get("name")
        .and_then(|n| n.as_str())
        .ok_or_else(|| ClassifyError::Parse("function missing name".into()))?;

    if name != "classify_email" {
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
    let jitter: u64 = rand::thread_rng().gen_range(0..=base_secs);
    Duration::from_secs(base_secs + jitter)
}
