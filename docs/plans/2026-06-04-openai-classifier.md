# OpenAI-Compatible Classifier Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add an OpenAI-compatible classifier backend so labelbot can use local AI (Ollama / Open WebUI) for email classification instead of requiring Anthropic Claude.

**Architecture:** Add an `OpenAiClassifier` implementing the existing `LlmClassifier` trait, targeting OpenAI chat completions API format (function calling). Add config fields for backend selection (`classifier_backend`), OpenAI base URL, API key, and model. Backwards compatible — existing Anthropic config continues to work unchanged. The daemon constructs whichever classifier is configured at startup.

**Tech Stack:** Rust, reqwest, serde_json, tokio

---

### Task 1: Add OpenAiClassifier (`src/classifier/openai.rs`)

**Files:**
- Create: `src/classifier/openai.rs`
- Modify: `src/classifier/mod.rs` (add `pub mod openai;`)

**Step 1: Create the classifier module**

Create `src/classifier/openai.rs` with:

- `OpenAiClassifier` struct holding `reqwest::Client`, `base_url`, `api_key`, `model`, `labels`
- `new()` constructor
- `impl LlmClassifier for OpenAiClassifier`
- Private `execute()` method that sends request and parses response
- `build_request_body()` — constructs OpenAI chat completions JSON with function calling for `classify_email`
- `parse_response()` — extracts labels from the function call response or falls back to JSON mode parsing
- Uses the same retry/backoff logic as `AnthropicClassifier` (shared constants or duplicated — duplication > premature abstraction per pragmatism)
- Same `canonicalize()` logic for case-insensitive label matching

The API call targets `{base_url}/chat/completions` (OpenAI-compatible endpoint).

Request body (OpenAI function calling format):
```json
{
    "model": "the-model-name",
    "messages": [
        {"role": "user", "content": "From: sender@example.com\nSubject: Invoice attached\n\nClassify this email into one or more of the defined labels."}
    ],
    "tools": [
        {
            "type": "function",
            "function": {
                "name": "classify_email",
                "description": "Classify an email into zero or more predefined labels",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "labels": {
                            "type": "array",
                            "items": {"type": "string", "enum": ["Work", "Finance", ...]},
                            "description": "Zero or more labels from the fixed set"
                        }
                    },
                    "required": ["labels"],
                    "additionalProperties": false
                }
            }
        }
    ],
    "tool_choice": {"type": "function", "function": {"name": "classify_email"}}
}
```

Response parsing extracts from `choices[0].message.tool_calls[0].function.arguments` JSON.

Headers: `Authorization: Bearer {api_key}`, `Content-Type: application/json`

Reuse existing `ClassifyError` variants from `src/error.rs` — no new error types needed.

Signed-off: `impl LlmClassifier for OpenAiClassifier { ... }`

**Step 2: Register module in mod.rs**

Add `pub mod openai;` after `pub mod anthropic;` in `src/classifier/mod.rs`.

**Step 3: Run build to check compilation**

```bash
cargo check
```
Expected: clean compile (warnings allowed per project lint config).

**Step 4: Commit**

```bash
git add src/classifier/openai.rs src/classifier/mod.rs
git commit -m "feat: add OpenAiClassifier skeleton"
```

---

### Task 2: Add config fields for OpenAI backend

**Files:**
- Modify: `src/config.rs`

**Step 1: Add config fields**

Add to `Config` struct:
- `classifier_backend: String` — defaults to `"anthropic"` (backwards compatible)
- `openai_base_url: String` — defaults to `"http://localhost:3000/api/v1"`
- `openai_api_key: String` — defaults to empty
- `openai_model: String` — defaults to `"llama3"`

Update validation in `validate()`: if `classifier_backend == "openai"` AND `openai_api_key` is empty, it's still ok (local AI often doesn't need real keys). If `classifier_backend == "anthropic"`, keep existing `anthropic_api_key` check.

Default functions for each new field.

**Step 2: Run build**

```bash
cargo check
```
Expected: clean compile.

**Step 3: Commit**

```bash
git add src/config.rs
git commit -m "feat: add openai config fields (base_url, api_key, model, backend selector)"
```

---

### Task 3: Wire backend selection in main.rs

**Files:**
- Modify: `src/main.rs`

**Step 1: Update main.rs to select classifier by config**

Replace direct `AnthropicClassifier::new(...)` with backend selection:

```rust
let label_names: Vec<String> = cfg.label_set().names().to_vec();

let classifier: Box<dyn LlmClassifier> = match cfg.classifier_backend.as_str() {
    "openai" => Box::new(
        labelbot::classifier::openai::OpenAiClassifier::new(
            cfg.openai_base_url.clone(),
            cfg.openai_api_key.clone(),
            cfg.openai_model.clone(),
            label_names,
        )
    ),
    _ => Box::new(
        labelbot::classifier::anthropic::AnthropicClassifier::new(
            cfg.anthropic_api_key.clone(),
            cfg.anthropic_model.clone(),
            label_names,
        )
    ),
};
```

Note: `daemon::run()` takes `impl LlmClassifier`, so `Box<dyn LlmClassifier>` won't work directly as it expects a concrete type. Need to adjust `daemon::run` to accept `Box<dyn LlmClassifier>` or use `impl LlmClassifier` with a generic. The simplest approach: change `daemon::run()` to accept `Box<dyn LlmClassifier>`.

Actually, looking at `daemon.rs`, `run` already takes `classifier: impl LlmClassifier`. To support runtime-selected backends, we need either:
- Change to `Box<dyn LlmClassifier>` — requires updating all call sites and impl trait methods
- Or use an enum wrapper

The Box approach is simpler. Update `daemon.rs`:

```rust
pub async fn run(cfg: Config, classifier: Box<dyn LlmClassifier>) -> anyhow::Result<()> {
```

And update references in `run_session`, `backfill`, `classify_and_record` to take `&dyn LlmClassifier` instead of `&impl LlmClassifier`.

Actually, `&impl LlmClassifier` and `&dyn LlmClassifier` have the same call semantics. Let me check — `trait LlmClassifier` has `fn classify(&self, ...)`. Since it's an async trait, it's using `impl Future` in return type. This is where the `Send + Sync` bound comes in. `Box<dyn LlmClassifier>` should work fine.

Let me trace through the changes:
- `daemon::run(cfg, classifier: impl LlmClassifier)` → `classifier: Box<dyn LlmClassifier>`
- `run_session(cfg, labels, classifier: &impl LlmClassifier, ...)` → `classifier: &dyn LlmClassifier`
- `backfill(session, store, classifier: &impl LlmClassifier, ...)` → `classifier: &dyn LlmClassifier`
- `classify_and_record(classifier: &impl LlmClassifier, ...)` → `classifier: &dyn LlmClassifier`

This works because `&dyn Trait` can call trait methods just like `&impl Trait`.

**Step 2: Run build**

```bash
cargo check
```
Expected: clean compile.

**Step 3: Commit**

```bash
git add src/main.rs src/daemon.rs
git commit -m "feat: wire backend selection in main (anthropic vs openai)"
```

---

### Task 4: Update example config

**Files:**
- Modify: `config.example.toml`

**Step 1: Document new config options**

Add documented section for the classifier backend choice:

```toml
# ── Classifier Backend ────────────────────────────────────────────
# Which LLM backend to use: "anthropic" (default) or "openai"
# classifer_backend = "anthropic"

# Anthropic settings (used when classifier_backend = "anthropic")
# anthropic_api_key = "sk-ant-..."
# anthropic_model = "claude-haiku-4-5"

# OpenAI-compatible settings (used when classifier_backend = "openai")
# Point at any OpenAI-compatible API (Open WebUI, Ollama, LiteLLM, etc.)
# openai_base_url = "http://192.168.1.100:3000/api/v1"
# openai_api_key = "sk-..."        # may be optional for local deployments
# openai_model = "llama3"
```

Keep existing lines, just add the new section.

**Step 2: Commit**

```bash
git add config.example.toml
git commit -m "docs: document openai classifier config in example"
```

---

### Task 5: Write tests for OpenAiClassifier

**Files:**
- Modify: `src/classifier/openai.rs` (add `#[cfg(test)] mod tests`)

**Step 1: Add comprehensive tests**

Add test module with:
- `classify_returns_labels()` — mock returns valid function call response with labels, verify they're returned
- `classify_returns_empty_labels()` — mock returns empty labels array
- `api_error_returns_error()` — mock returns 400, verify Api error returned
- `parse_function_call_extracts_labels()` — unit test parsing function call JSON
- `parse_empty_labels()` — unit test empty labels array
- `parse_invalid_response_missing_choices()` — missing choices returns Parse error
- `parse_rejects_unknown_label()` — unknown label returns Parse error
- `openai_chat_completions_path()` — verify URL is `{base_url}/chat/completions`

Use `wiremock` for HTTP mocking (same as existing Anthropic tests).

**Step 2: Run tests**

```bash
cargo test -p labelbot --test '*' 2>&1; cargo test classifier::openai::tests 2>&1
```
Expected: all tests pass.

**Step 3: Commit**

```bash
git add src/classifier/openai.rs
git commit -m "test: add OpenAiClassifier tests"
```

---

### Task 6: Integration smoke test

**Step 1: Build release**

```bash
cargo build --release
```
Expected: binary builds cleanly.

**Step 2: Verify daemon compiles and runs with backend selection**

No actual IMAP/Ollama needed — the daemon will fail to connect to IMAP which is fine. The goal is to confirm:
- Backend selection compiles
- Config parsing works with `classifier_backend = "openai"`

Create a minimal config test:
```bash
LABELBOT_CONFIG=/dev/null ./target/release/labelbot 2>&1 || true
```
Expected: exits with config error (missing fields), not a panic or compilation issue.

**Step 3: No commit (validation only)**

---

## Summary of changes

| File | Action |
|---|---|
| `src/classifier/openai.rs` | **Create** — OpenAI-compatible classifier |
| `src/classifier/mod.rs` | **Modify** — register `openai` module |
| `src/config.rs` | **Modify** — add `classifier_backend`, `openai_*` fields |
| `src/main.rs` | **Modify** — backend selection logic |
| `src/daemon.rs` | **Modify** — `run()` accepts `Box<dyn LlmClassifier>` |
| `config.example.toml` | **Modify** — document new options |
