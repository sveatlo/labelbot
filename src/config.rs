use crate::error::ConfigError;
use crate::labels::{LabelConfig, LabelSet};
use figment::Figment;
use figment::providers::{Env, Format, Serialized, Toml};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub imap_host: String,
    pub imap_port: u16,
    pub imap_user: String,
    pub imap_password: String,
    pub anthropic_api_key: String,
    #[serde(default = "default_anthropic_model")]
    pub anthropic_model: String,
    #[serde(default = "default_db_path")]
    pub db_path: String,
    #[serde(default = "default_tls_insecure")]
    pub tls_insecure: bool,
    #[serde(default = "default_poll_timeout")]
    pub poll_idle_timeout_secs: u64,
    #[serde(default = "default_backfill_days")]
    pub backfill_max_age_days: u64,
    #[serde(default = "LabelSet::default_config")]
    pub labels: HashMap<String, LabelConfig>,
}

impl Config {
    pub fn label_set(&self) -> LabelSet {
        LabelSet::from_config(&self.labels)
    }
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
    365
}

impl Default for Config {
    fn default() -> Self {
        Config {
            imap_host: "host.docker.internal".into(),
            imap_port: 1143,
            imap_user: String::new(),
            imap_password: String::new(),
            anthropic_api_key: String::new(),
            anthropic_model: default_anthropic_model(),
            db_path: default_db_path(),
            tls_insecure: default_tls_insecure(),
            poll_idle_timeout_secs: default_poll_timeout(),
            backfill_max_age_days: default_backfill_days(),
            labels: LabelSet::default_config(),
        }
    }
}

impl Config {
    pub fn load() -> Result<Self, ConfigError> {
        let config_path =
            std::env::var("LABELBOT_CONFIG").unwrap_or_else(|_| "config.toml".into());

        let figment = Figment::from(Serialized::defaults(Config::default()))
            .merge(Toml::file(&config_path))
            .merge(Env::raw().only(&["ANTHROPIC_API_KEY"]))
            .merge(Env::prefixed("APP_"));

        Config::from_figment(figment)
    }

    #[expect(clippy::needless_pass_by_value)]
    pub fn from_figment(figment: Figment) -> Result<Self, ConfigError> {
        let cfg: Config = figment.extract()?;
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> Result<(), ConfigError> {
        if self.imap_user.is_empty() {
            return Err(ConfigError::Missing("imap_user"));
        }
        if self.imap_password.is_empty() {
            return Err(ConfigError::Missing("imap_password"));
        }
        if self.anthropic_api_key.is_empty() {
            return Err(ConfigError::Missing("anthropic_api_key"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use figment::providers::Toml as TomlProv;

    fn provider_figment() -> Figment {
        Figment::from(Serialized::defaults(Config::default()))
    }

    #[test]
    fn defaults_apply_when_only_required_provided() {
        let f = provider_figment().merge(TomlProv::string(
            "imap_user='u'\nimap_password='p'\nanthropic_api_key='k'",
        ));
        let cfg = Config::from_figment(f).unwrap();
        assert_eq!(cfg.imap_port, 1143);
        assert_eq!(cfg.anthropic_model, "claude-haiku-4-5");
        assert_eq!(cfg.poll_idle_timeout_secs, 300);
        assert_eq!(cfg.db_path, "./local.db");
    }

    #[test]
    fn toml_overrides_defaults() {
        let f = provider_figment()
            .merge(TomlProv::string(
                "imap_host='mail.local'\nimap_port=1993\nimap_user='u'\nimap_password='p'\nanthropic_api_key='k'",
            ));
        let cfg = Config::from_figment(f).unwrap();
        assert_eq!(cfg.imap_host, "mail.local");
        assert_eq!(cfg.imap_port, 1993);
    }

    #[test]
    fn missing_required_field_errors() {
        let f = provider_figment().merge(TomlProv::string(
            "imap_user=''\nimap_password='p'\nanthropic_api_key='k'",
        ));
        let err = Config::from_figment(f).unwrap_err();
        assert!(matches!(&err, ConfigError::Missing(f) if *f == "imap_user"));

        let f = provider_figment().merge(TomlProv::string(
            "anthropic_api_key=''\nimap_user='u'\nimap_password='p'",
        ));
        let err = Config::from_figment(f).unwrap_err();
        assert!(matches!(&err, ConfigError::Missing(f) if *f == "anthropic_api_key"));
    }
}
