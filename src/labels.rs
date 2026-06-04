use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Name of the auto-applied label for emails matching any user-marked important label.
pub const IMPORTANT_LABEL: &str = "Important";

/// Per-label user configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct LabelConfig {
    #[serde(default)]
    pub important: bool,
}

/// A configured set of labels. Carries the canonical name list (for the LLM)
/// and an `important` flag per label (for the auto-tagging rule).
#[derive(Debug, Clone)]
pub struct LabelSet {
    /// Stable, sorted list of canonical names. Used for prompts/enums.
    names: Vec<String>,
    /// Lookup: canonical-name -> important flag.
    /// Keys are stored verbatim; lookups are case-insensitive (see `find`).
    map: HashMap<String, bool>,
}

impl LabelSet {
    pub fn from_config(map: &HashMap<String, LabelConfig>) -> Self {
        let mut names: Vec<String> = map.keys().cloned().collect();
        names.sort_unstable();
        let map = map
            .iter()
            .map(|(k, v)| (k.clone(), v.important))
            .collect();
        Self { names, map }
    }

    /// Canonical names, sorted, suitable for the classifier enum.
    pub fn names(&self) -> &[String] {
        &self.names
    }

    /// Case-insensitive lookup: returns the canonical name if known.
    pub fn canonical(&self, name: &str) -> Option<&str> {
        self.names
            .iter()
            .find(|n| n.eq_ignore_ascii_case(name.trim()))
            .map(String::as_str)
    }

    pub fn is_important(&self, canonical_name: &str) -> bool {
        self.map.get(canonical_name).copied().unwrap_or(false)
    }

    /// True if any configured label is marked important. When false, the
    /// "Important" auxiliary mailbox is not needed.
    pub fn has_any_important(&self) -> bool {
        self.map.values().any(|v| *v)
    }

    /// The default label set used when the user omits `[labels]` from the
    /// config: the historical ten labels, none marked important.
    pub fn default_names() -> [&'static str; 10] {
        [
            "Work",
            "Finance",
            "Newsletters",
            "Receipts",
            "Personal",
            "Travel",
            "Shopping",
            "Notifications",
            "House",
            "Family",
        ]
    }

    pub fn default_config() -> HashMap<String, LabelConfig> {
        Self::default_names()
            .iter()
            .map(|n| ((*n).to_owned(), LabelConfig::default()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(pairs: &[(&str, bool)]) -> HashMap<String, LabelConfig> {
        pairs
            .iter()
            .map(|(n, i)| ((*n).to_owned(), LabelConfig { important: *i }))
            .collect()
    }

    #[test]
    fn default_set_has_ten_labels_none_important() {
        let set = LabelSet::from_config(&LabelSet::default_config());
        assert_eq!(set.names().len(), 10);
        assert!(!set.has_any_important());
    }

    #[test]
    fn canonical_is_case_insensitive_and_trims() {
        let set = LabelSet::from_config(&cfg(&[("House", true), ("Newsletter", false)]));
        assert_eq!(set.canonical("  house  "), Some("House"));
        assert_eq!(set.canonical("NEWSLETTER"), Some("Newsletter"));
        assert_eq!(set.canonical("Spam"), None);
    }

    #[test]
    fn important_flag_is_per_label() {
        let set = LabelSet::from_config(&cfg(&[("House", true), ("Newsletter", false)]));
        assert!(set.is_important("House"));
        assert!(!set.is_important("Newsletter"));
        assert!(set.has_any_important());
    }

    #[test]
    fn names_are_sorted() {
        let set = LabelSet::from_config(&cfg(&[("Zebra", false), ("Apple", false)]));
        assert_eq!(set.names(), &["Apple".to_owned(), "Zebra".to_owned()]);
    }
}
