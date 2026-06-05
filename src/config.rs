use crate::error::ConfigError;
use crate::labels::{LabelConfig, LabelSet};
use figment::Figment;
use figment::providers::{Env, Format, Toml};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use url::Url;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub imap: ImapConfig,
    pub classifier: ClassifierConfig,

    #[serde(default)]
    pub summarizer: SummarizerConfig,

    #[serde(default = "default_db_path")]
    pub db_path: String,

    #[serde(default = "default_poll_timeout")]
    pub poll_idle_timeout_secs: u64,
    #[serde(default = "default_backfill_days")]
    pub backfill_max_age_days: u64,
    #[serde(default = "LabelSet::default_config")]
    labels: HashMap<String, LabelConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "summarizer_backend")]
pub enum SummarizerConfig {
    T5 {
        #[serde(default = "default_summarizer_model")]
        model_id: String,
        #[serde(default = "default_max_input_chars")]
        max_input_chars: usize,
        #[serde(default = "default_max_output_tokens")]
        max_output_tokens: usize,
    },
    Truncate {
        #[serde(default = "default_max_input_chars")]
        max_chars: usize,
    },
}

impl Default for SummarizerConfig {
    fn default() -> Self {
        Self::Truncate {
            max_chars: default_max_input_chars(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImapConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    #[serde(default = "default_tls_insecure")]
    pub tls_insecure: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "classifier_backend")]
pub enum ClassifierConfig {
    Anthropic {
        api_key: String,
        #[serde(default = "default_anthropic_model")]
        model: String,
    },
    OpenAI {
        api_key: String,
        #[serde(default = "default_openai_base_url")]
        base_url: Url,
        #[serde(default = "default_openai_model")]
        model: String,
    },
}

impl Default for ClassifierConfig {
    fn default() -> Self {
        ClassifierConfig::Anthropic {
            api_key: String::new(),
            model: default_anthropic_model(),
        }
    }
}

impl Config {
    pub fn label_set(&self) -> LabelSet {
        LabelSet::from_config(&self.labels)
    }
}

fn default_summarizer_model() -> String {
    "t5-base".into()
}
fn default_max_input_chars() -> usize {
    2000
}
fn default_max_output_tokens() -> usize {
    100
}
fn default_anthropic_model() -> String {
    "claude-haiku-4-5".into()
}
fn default_db_path() -> String {
    "./local.db".into()
}
fn default_poll_timeout() -> u64 {
    300
}
fn default_tls_insecure() -> bool {
    false
}
fn default_backfill_days() -> u64 {
    0
}
fn default_openai_base_url() -> Url {
    "https://api.openai.com/v1/"
        .parse()
        .expect("BUG: invalid hardcoded OpenAI URL")
}
fn default_openai_model() -> String {
    "llama3".into()
}

impl Default for Config {
    fn default() -> Self {
        Config {
            imap: ImapConfig {
                host: "host.docker.internal".into(),
                port: 1143,
                user: String::new(),
                password: String::new(),
                tls_insecure: default_tls_insecure(),
            },
            classifier: ClassifierConfig::default(),
            summarizer: SummarizerConfig::default(),
            db_path: default_db_path(),
            poll_idle_timeout_secs: default_poll_timeout(),
            backfill_max_age_days: default_backfill_days(),
            labels: LabelSet::default_config(),
        }
    }
}

impl Config {
    pub fn load(config_file: Option<PathBuf>) -> Result<Self, ConfigError> {
        let mut figment = Figment::new();

        if let Some(config_file) = config_file {
            figment = figment.merge(Toml::file(config_file));
        }

        figment = figment.merge(Env::prefixed("LABELBOT_").split("__"));

        Config::from_figment(figment)
    }

    #[expect(clippy::needless_pass_by_value)]
    pub fn from_figment(figment: Figment) -> Result<Self, ConfigError> {
        let cfg: Config = figment.extract()?;
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> Result<(), ConfigError> {
        if self.imap.user.is_empty() {
            return Err(ConfigError::Missing("imap.user"));
        }
        if self.imap.password.is_empty() {
            return Err(ConfigError::Missing("imap.password"));
        }
        match &self.classifier {
            ClassifierConfig::OpenAI { .. } => {
                // api_key is optional for local deployments
            }
            ClassifierConfig::Anthropic { api_key, .. } => {
                if api_key.is_empty() {
                    return Err(ConfigError::Missing("classifier.api_key"));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use figment::providers::Serialized;
    use figment::providers::Toml as TomlProv;

    fn provider_figment() -> Figment {
        Figment::from(Serialized::defaults(Config::default()))
    }

    #[test]
    fn defaults_apply_when_only_required_provided() {
        let f = provider_figment().merge(TomlProv::string(
            "[imap]\nuser='u'\npassword='p'\n[classifier]\nclassifier_backend='Anthropic'\napi_key='k'",
        ));
        let cfg = Config::from_figment(f).unwrap();
        assert_eq!(cfg.imap.port, 1143);
        assert_eq!(cfg.poll_idle_timeout_secs, 300);
        assert_eq!(cfg.db_path, "./local.db");
        assert!(matches!(
            &cfg.classifier,
            ClassifierConfig::Anthropic { model, .. } if model == "claude-haiku-4-5"
        ));
    }

    #[test]
    fn toml_overrides_defaults() {
        let f = provider_figment().merge(TomlProv::string(
            "[imap]\nhost='mail.local'\nport=1993\nuser='u'\npassword='p'\n[classifier]\nclassifier_backend='Anthropic'\napi_key='k'",
        ));
        let cfg = Config::from_figment(f).unwrap();
        assert_eq!(cfg.imap.host, "mail.local");
        assert_eq!(cfg.imap.port, 1993);
    }

    #[test]
    fn missing_required_field_errors() {
        let f = provider_figment().merge(TomlProv::string(
            "[imap]\nuser=''\npassword='p'\n[classifier]\nclassifier_backend='Anthropic'\napi_key='k'",
        ));
        let err = Config::from_figment(f).unwrap_err();
        assert!(matches!(&err, ConfigError::Missing(f) if *f == "imap.user"));

        let f = provider_figment().merge(TomlProv::string(
            "[imap]\nuser='u'\npassword='p'\n[classifier]\nclassifier_backend='Anthropic'\napi_key=''",
        ));
        let err = Config::from_figment(f).unwrap_err();
        assert!(matches!(&err, ConfigError::Missing(f) if *f == "classifier.api_key"));
    }
}
