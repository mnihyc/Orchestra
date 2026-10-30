//! Words in the languages the interface speaks, with the English text as the fallback.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The language every set of words has: the others fall back to it.
pub const FALLBACK_LANGUAGE: &str = "en";

/// The languages the interface is written in, in the order they are listed.
pub const LANGUAGES: [&str; 2] = [FALLBACK_LANGUAGE, "zh-CN"];

/// A set of words by language tag: `{"en": "Codex", "zh-CN": "Codex"}`. English is always present, so a reader
/// whose language is missing still gets words. A tag this version does not know is kept and never interpreted.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Localized(BTreeMap<String, String>);

impl Localized {
    /// The words in English alone.
    #[must_use]
    pub fn english(words: impl Into<String>) -> Self {
        Self(BTreeMap::from([(
            FALLBACK_LANGUAGE.to_owned(),
            words.into(),
        )]))
    }

    /// The same words with those of `language` added.
    #[must_use]
    pub fn with(mut self, language: &str, words: impl Into<String>) -> Self {
        self.0.insert(language.to_owned(), words.into());
        self
    }

    /// The English words; empty when there are none, which [`Localized::is_complete`] reports.
    #[must_use]
    pub fn en(&self) -> &str {
        self.0.get(FALLBACK_LANGUAGE).map_or("", String::as_str)
    }

    /// The words in `language`, or in English where there are none.
    #[must_use]
    pub fn get(&self, language: &str) -> &str {
        self.0
            .get(language)
            .filter(|words| !words.trim().is_empty())
            .map_or_else(|| self.en(), String::as_str)
    }

    /// Whether English words are there, and every listed language has some.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        !self.en().trim().is_empty() && self.0.values().all(|words| !words.trim().is_empty())
    }

    /// Whether every language the interface speaks has its own words.
    #[must_use]
    pub fn covers_every_language(&self) -> bool {
        LANGUAGES.iter().all(|language| {
            self.0
                .get(*language)
                .is_some_and(|words| !words.trim().is_empty())
        })
    }

    /// The words by language.
    #[must_use]
    pub fn all(&self) -> &BTreeMap<String, String> {
        &self.0
    }

    /// The words of every language but English, for a payload that carries them beside the English.
    #[must_use]
    pub fn others(&self) -> BTreeMap<String, String> {
        self.0
            .iter()
            .filter(|(language, _)| language.as_str() != FALLBACK_LANGUAGE)
            .map(|(language, words)| (language.clone(), words.clone()))
            .collect()
    }
}

impl Serialize for Localized {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Localized {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self(BTreeMap::deserialize(deserializer)?))
    }
}

impl JsonSchema for Localized {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Localized".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "object",
            "description": "Words by language tag; `en` is always present and is the fallback",
            "properties": { "en": { "type": "string" }, "zh-CN": { "type": "string" } },
            "required": ["en"],
            "additionalProperties": { "type": "string" }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_missing_language_reads_as_english() {
        let words = Localized::english("Plan").with("zh-CN", "计划");
        assert_eq!(words.get("zh-CN"), "计划");
        assert_eq!(words.get("fr"), "Plan");
        assert!(words.covers_every_language());
        assert!(!Localized::english("Plan").covers_every_language());
        assert!(Localized::english("Plan").is_complete());
        assert!(!Localized::default().is_complete());
    }

    #[test]
    fn it_is_written_as_a_plain_object() {
        let words = Localized::english("Codex").with("zh-CN", "Codex");
        assert_eq!(
            serde_json::to_value(&words).unwrap(),
            json!({"en": "Codex", "zh-CN": "Codex"})
        );
        let read: Localized =
            serde_json::from_value(json!({"en": "Fake", "de": "Attrappe"})).unwrap();
        assert_eq!(read.get("de"), "Attrappe");
        assert_eq!(read.others().len(), 1);
    }
}
