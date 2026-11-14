//! The target family keeps one schema through settings and summary requests.
#![allow(clippy::unwrap_used, reason = "a test fails by panicking")]

use orch_api_contract::summaries::{SummaryKind, SummaryTargets};
use orch_settings_contract::registry::{self, Scope, keys};
use orch_summarizer_contract::summarizer::{SUMMARY_CHARS, TITLE_MAX_CHARS};
use orch_summarizer_model::descriptor::CONTEXT_TOKENS;
use serde_json::json;

#[test]
fn target_options_match_the_setting_schema_and_reach_an_individual_thread() {
    let definition = registry::get(keys::SUMMARY_TARGETS).unwrap();
    assert!(definition.scopes.contains(&Scope::Thread));
    registry::validate_at(
        keys::SUMMARY_TARGETS,
        Scope::Thread,
        &definition.default_value().unwrap(),
    )
    .unwrap();
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

#[test]
fn every_generated_target_ships_off_with_the_numbers_the_summaries_use() {
    let definition = registry::get(keys::SUMMARY_TARGETS).unwrap();
    let shipped: SummaryTargets =
        serde_json::from_value(definition.default_value().unwrap()).unwrap();
    for (kind, seconds, chars) in [
        (SummaryKind::Thread, 120, SUMMARY_CHARS),
        (SummaryKind::Topic, 900, SUMMARY_CHARS),
        (SummaryKind::Document, 300, SUMMARY_CHARS),
        (SummaryKind::Chat, 300, SUMMARY_CHARS),
        (SummaryKind::ChatTitle, 300, TITLE_MAX_CHARS),
    ] {
        let options = shipped.generated(kind).unwrap();
        assert!(!options.automatic, "{kind:?} ships off");
        assert_eq!(options.min_interval_secs, Some(seconds), "{kind:?}");
        assert_eq!(
            options.context_tokens.unwrap().get(),
            CONTEXT_TOKENS,
            "{kind:?}"
        );
        assert_eq!(options.output_chars.unwrap().get(), chars, "{kind:?}");
        assert!(!options.on.as_ref().unwrap().is_empty(), "{kind:?}");
    }
    assert!(shipped.help.is_none() && shipped.search_results.is_none());
}
