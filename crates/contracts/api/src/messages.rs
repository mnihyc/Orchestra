//! Public error sentences and recovery from the same catalogue and native aliases as the interface.

use crate::common::ErrorCode;
use orch_types::Json;
use std::sync::OnceLock;

const ENGLISH: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../frontend/src/i18n/en.json"
));
const ALIASES: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../frontend/src/i18n/error-aliases.json"
));

fn catalogue() -> &'static Json {
    static KEPT: OnceLock<Json> = OnceLock::new();
    KEPT.get_or_init(|| serde_json::from_str(ENGLISH).unwrap_or(Json::Null))
}

fn at<'a>(catalogue: &'a Json, key: &str) -> Option<&'a str> {
    key.split('.')
        .try_fold(catalogue, |value, part| value.get(part))?
        .as_str()
}

/// The contract's English wire fallback; browsers localize the same key, not a program's diagnostics.
#[must_use]
pub fn for_code(code: ErrorCode) -> String {
    at(catalogue(), &format!("errors.{}", code.as_str()))
        .unwrap_or("The server could not complete this request. Try again.")
        .to_owned()
}

/// A native or job failure retains its code while its human message has a known recovery sentence.
#[must_use]
pub fn for_failure(code: &str) -> String {
    if let Ok(code) = serde_json::from_value::<ErrorCode>(Json::String(code.to_owned())) {
        return for_code(code);
    }
    static KEPT: OnceLock<Json> = OnceLock::new();
    let aliases = KEPT.get_or_init(|| serde_json::from_str(ALIASES).unwrap_or(Json::Null));
    aliases
        .get(code)
        .and_then(Json::as_str)
        .and_then(|key| at(catalogue(), key))
        .or_else(|| at(catalogue(), "ui.error.operationFailed"))
        .unwrap_or("The server could not complete this request. Try again.")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_history_import_refusal_has_its_own_public_recovery() {
        let message = for_failure("history_import_failed");
        assert!(message.contains("Summary or None"));
        assert!(message.contains("could not import the carried history"));
        assert_ne!(message, for_failure("harness_unavailable"));
    }

    #[test]
    fn every_native_alias_resolves_in_both_languages_and_unknown_codes_have_recovery() {
        let aliases: Json =
            serde_json::from_str(ALIASES).unwrap_or_else(|error| panic!("aliases: {error}"));
        let chinese = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../frontend/src/i18n/zh-CN.json"
        ));
        for text in [ENGLISH, chinese] {
            let words: Json =
                serde_json::from_str(text).unwrap_or_else(|error| panic!("words: {error}"));
            for (code, key) in aliases
                .as_object()
                .unwrap_or_else(|| panic!("aliases need names"))
            {
                let key = key
                    .as_str()
                    .unwrap_or_else(|| panic!("{code} needs a sentence key"));
                assert!(at(&words, key).is_some(), "{code}: missing {key}");
            }
        }
        assert_eq!(
            for_failure("future_constraint_fkey"),
            at(catalogue(), "ui.error.operationFailed").unwrap_or_else(|| panic!("fallback words"))
        );
    }
}
