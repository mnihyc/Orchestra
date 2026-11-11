//! The target family keeps one schema through settings and summary requests.
#![allow(clippy::unwrap_used, reason = "a test fails by panicking")]

use orch_api_contract::summaries::{SummaryKind, SummaryTargets};
use orch_settings_contract::registry::{self, Scope, keys};
use serde_json::json;

#[test]
fn target_options_match_the_setting_schema_and_reach_an_individual_thread() {
    let definition = registry::get(keys::SUMMARY_TARGETS).unwrap();
    assert!(definition.scopes.contains(&Scope::Thread));
    assert_eq!(definition.default_value(), Some(json!({})));
    let raw = json!({
        "thread":{"automatic":true,"on":["turn_completed","milestone"],"output_chars":1200},
        "document":{"automatic":true,"on":["content_changed"],"context_tokens":40000},
        "chat":{"automatic":false,"on":["message_posted"]},"topic":{"automatic":false,"on":["child_changed"]},
        "help":{},"search_results":{}
    });
    registry::validate_at(keys::SUMMARY_TARGETS, Scope::Thread, &raw).unwrap();
    let targets: SummaryTargets = serde_json::from_value(raw).unwrap();
    assert!(targets.generated(SummaryKind::Thread).unwrap().automatic);
    assert_eq!(
        targets
            .generated(SummaryKind::Document)
            .unwrap()
            .context_tokens
            .unwrap()
            .get(),
        40000
    );
    for key in [
        keys::AUTO_TITLE,
        keys::AUTO_TAGS,
        keys::AUTO_SUMMARIZER,
        keys::AUTO_TRIGGERS,
    ] {
        assert!(
            registry::get(key).unwrap().scopes.contains(&Scope::Thread),
            "{key}"
        );
    }
    for raw in [
        json!({"document":{"automatic":false,"context_tokens":0}}),
        json!({"help":{"automatic":true}}),
        json!({"search_results":{"output_chars":500}}),
    ] {
        assert!(registry::validate(keys::SUMMARY_TARGETS, &raw).is_err());
        assert!(serde_json::from_value::<SummaryTargets>(raw).is_err());
    }
}
