//! The item, tool and request payloads the agent contract writes beside the API's own have one wire shape with them.
//!
//! A harness's events are written with the agent contract's types and shown with the API's. Where the two write
//! the same payload, each is its own type (the agent contract is the harness's, with its own words), and this
//! test holds them to one shape: every member, its type, which are required, the values an enumeration takes.
//! A plan card also has optional API decision metadata, checked separately from its native proposal.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_agent_contract::{events, session};
use orch_api_contract::{approvals, threads};
use schemars::JsonSchema;
use serde_json::{Map, Value as Json};

/// How many references one schema may follow inside another before it is taken for a loop.
const MOST_REFERENCES: usize = 64;

/// The schema of `T` as it constrains what is written: each reference replaced by what it names, the words for
/// people (descriptions, titles) left out, and an enumeration written one way, so that two copies compare by
/// their shape alone.
fn shape<T: JsonSchema>() -> Json {
    let root = serde_json::to_value(schemars::schema_for!(T)).unwrap();
    let definitions = root.get("$defs").cloned().unwrap_or(Json::Null);
    resolved(&root, &definitions, 0)
}

/// Whether a member of a schema is one of the words for people, or the list of definitions resolved in place.
fn for_people(key: &str) -> bool {
    matches!(key, "description" | "title" | "$schema" | "$defs")
}

fn resolved(schema: &Json, definitions: &Json, followed: usize) -> Json {
    match schema {
        Json::Object(members) => {
            let mut whole = Map::new();
            if let Some(Json::String(target)) = members.get("$ref") {
                assert!(
                    followed < MOST_REFERENCES,
                    "a payload's schema refers to itself: {target}"
                );
                let name = target.trim_start_matches("#/$defs/");
                match resolved(&definitions[name], definitions, followed + 1) {
                    Json::Object(named) => whole = named,
                    other => return other,
                }
            }
            for (key, value) in members {
                if key != "$ref" && !for_people(key) {
                    whole.insert(key.clone(), resolved(value, definitions, followed));
                }
            }
            one_enumeration(whole)
        }
        Json::Array(each) => Json::Array(
            each.iter()
                .map(|value| resolved(value, definitions, followed))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// An enumeration of words is written as one `enum` where none of its values has words for people, and as one
/// `const` for each value where some have; both are written here as the `enum`.
fn one_enumeration(mut schema: Map<String, Json>) -> Json {
    let words = schema
        .get("oneOf")
        .and_then(Json::as_array)
        .and_then(|choices| {
            choices
                .iter()
                .map(|choice| {
                    let choice = choice.as_object()?;
                    let word =
                        choice
                            .get("const")
                            .cloned()
                            .or_else(|| match choice.get("enum") {
                                Some(Json::Array(one)) if one.len() == 1 => one.first().cloned(),
                                _ => None,
                            })?;
                    choice
                        .keys()
                        .all(|key| matches!(key.as_str(), "type" | "const" | "enum"))
                        .then_some(word)
                })
                .collect::<Option<Vec<Json>>>()
        });
    if let Some(words) = words {
        schema.remove("oneOf");
        schema.insert("type".to_owned(), Json::String("string".to_owned()));
        schema.insert("enum".to_owned(), Json::Array(words));
    }
    Json::Object(schema)
}

/// One test for each pair: it fails when the two copies' schemas differ in shape.
macro_rules! one_shape {
    ($($test:ident: $agent:ty => $api:ty),+ $(,)?) => {
        $(
            #[test]
            fn $test() {
                assert_eq!(
                    shape::<$agent>(),
                    shape::<$api>(),
                    concat!(stringify!($agent), " no longer has the shape of ", stringify!($api))
                );
            }
        )+
    };
}

one_shape!(
    agent_message_payload: events::AgentMessagePayload => threads::AgentMessagePayload,
    agent_message_phase: events::AgentMessagePhase => threads::AgentMessagePhase,
    approval_matcher: events::ApprovalMatcher => approvals::ApprovalMatcher,
    command_action: events::CommandAction => threads::CommandAction,
    elicitation_action: session::ElicitationAction => approvals::ElicitationAction,
    error_payload: events::ErrorPayload => threads::ErrorPayload,
    file_change: events::FileChange => threads::FileChange,
    file_change_image: events::FileChangeImage => orch_api_contract::source::FileChangeImage,
    file_change_kind: events::FileChangeKind => threads::FileChangeKind,
    file_search_match: events::FileSearchMatch => threads::FileSearchMatch,
    file_search_mode: events::FileSearchMode => threads::FileSearchMode,
    harness_request_kind: events::HarnessRequestKind => approvals::HarnessRequestKind,
    limit_scope: events::LimitScope => threads::LimitScope,
    memory_keeper: session::MemoryKeeper => threads::MemoryKeeper,
    memory_why: session::MemoryWhy => threads::MemoryWhy,
    option_scope: events::OptionScope => approvals::OptionScope,
    option_style: events::OptionStyle => approvals::OptionStyle,
    orch_tool: events::OrchTool => threads::OrchTool,
    plan_step: events::PlanStep => threads::PlanStep,
    plan_step_status: events::PlanStepStatus => threads::PlanStepStatus,
    question: events::Question => approvals::Question,
    question_option: events::QuestionOption => approvals::QuestionOption,
    reasoning_payload: events::ReasoningPayload => threads::ReasoningPayload,
    subagent_state: events::SubagentState => threads::SubagentState,
    tool_payload: events::ToolPayload => threads::ToolPayload,
    turn_phase_kind: events::TurnPhaseKind => threads::TurnPhaseKind,
    web_action: events::WebAction => threads::WebAction,
    web_search_result: events::WebSearchResult => threads::WebSearchResult,
);

/// A tool call is written with its raw form and shown without it, so that the thread's timeline does not carry the same
/// content twice: the API's member is optional where the agent's is required, and in every other member the two agree.
#[test]
fn tool_call_payload_has_the_shape_of_the_apis_but_for_its_optional_raw_form() {
    let without_raw = |mut schema: Json| {
        schema["properties"].as_object_mut().unwrap().remove("raw");
        schema["required"]
            .as_array_mut()
            .unwrap()
            .retain(|name| name != "raw");
        schema
    };
    assert_eq!(
        without_raw(shape::<events::ToolCallPayload>()),
        without_raw(shape::<threads::ToolCallPayload>()),
        "ToolCallPayload no longer has the shape of the API's but for its raw form"
    );
    let written = shape::<events::ToolCallPayload>();
    assert!(
        written["required"]
            .as_array()
            .unwrap()
            .iter()
            .any(|name| name == "raw"),
        "a harness's call always says what it was"
    );
    assert!(
        !shape::<threads::ToolCallPayload>()["required"]
            .as_array()
            .unwrap()
            .iter()
            .any(|name| name == "raw"),
        "the timeline's call may leave it out"
    );
}

#[test]
fn web_actions_preserve_legacy_rows_and_unknown_actions_are_not_searches()
-> Result<(), Box<dyn std::error::Error>> {
    let old = serde_json::json!({"kind":"web_search", "query":"orchards", "results":[]});
    let native: events::ToolPayload = serde_json::from_value(old.clone())?;
    let api: threads::ToolPayload = serde_json::from_value(old.clone())?;
    assert_eq!(serde_json::to_value(native)?, old);
    assert_eq!(serde_json::to_value(api)?, old);
    for action in [
        serde_json::json!({"type":"open_page", "url":"https://docs.example.invalid/guide"}),
        serde_json::json!({"type":"find_in_page", "url":null, "pattern":"harvest"}),
        serde_json::json!({"type":"other"}),
    ] {
        let row =
            serde_json::json!({"kind":"web_search", "query":"", "results":[], "action":action});
        let native: events::ToolPayload = serde_json::from_value(row.clone())?;
        let api: threads::ToolPayload = serde_json::from_value(row.clone())?;
        assert_eq!(serde_json::to_value(native)?, row);
        assert_eq!(serde_json::to_value(api)?, row);
    }
    let future: events::WebAction =
        serde_json::from_value(serde_json::json!({"type":"future_action", "new_member":true}))?;
    assert_eq!(future, events::WebAction::Other);
    Ok(())
}

#[test]
fn recorded_file_images_survive_both_payload_copies_and_old_changes_still_read() {
    let old = serde_json::json!({
        "path":"code.txt", "change":"modify", "old_path":null,
        "diff":null, "added":1, "removed":1, "binary":false, "truncated":false
    });
    for images in [
        None,
        Some(serde_json::json!({
            "before":{"state":"text", "text":"before"},
            "after":{"state":"text", "text":"after"}
        })),
    ] {
        let mut recorded = old.clone();
        if let Some(images) = images {
            recorded
                .as_object_mut()
                .unwrap()
                .extend(images.as_object().unwrap().clone());
        }
        let native: events::FileChange = serde_json::from_value(recorded.clone()).unwrap();
        let shown: threads::FileChange = serde_json::from_value(recorded.clone()).unwrap();
        assert_eq!(serde_json::to_value(native).unwrap(), recorded);
        assert_eq!(serde_json::to_value(shown).unwrap(), recorded);
    }
}

#[test]
fn allowance_details_are_optional_and_survive_the_agent_to_api_boundary() {
    let previous = serde_json::json!({
        "code": "harness_error", "message": "An earlier notice", "retryable": false
    });
    let native: events::ErrorPayload = serde_json::from_value(previous.clone()).unwrap();
    let shown: threads::ErrorPayload = serde_json::from_value(previous.clone()).unwrap();
    assert_eq!(serde_json::to_value(native).unwrap(), previous);
    assert_eq!(serde_json::to_value(shown).unwrap(), previous);

    let limited = serde_json::json!({
        "code": "image_generation_limit", "message": "Image generation reached its usage limit.",
        "retryable": false, "details": {"reported":true},
        "window": "imagegen_premium", "resets_at": "2026-10-12T12:35:52Z"
    });
    let native: events::ErrorPayload = serde_json::from_value(limited).unwrap();
    let wire = serde_json::to_value(native).unwrap();
    let shown: threads::ErrorPayload = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(shown.window.as_deref(), Some("imagegen_premium"));
    assert!(shown.resets_at.is_some());
    assert_eq!(shown.details, Some(serde_json::json!({"reported":true})));
    assert_eq!(serde_json::to_value(shown).unwrap(), wire);
}

#[test]
fn plan_payload() {
    let proposed = shape::<events::PlanPayload>();
    let mut shown = shape::<threads::PlanPayload>();
    let fields = shown["properties"].as_object_mut().unwrap();
    assert_eq!(
        fields.remove("decision").unwrap(),
        shape::<Option<approvals::RequestDecisionStatus>>()
    );
    assert_eq!(
        fields.remove("approval_id").unwrap(),
        shape::<Option<orch_types::Id>>()
    );
    assert_eq!(
        proposed, shown,
        "the API preserves every native plan member and adds only its typed decision metadata"
    );
}

#[test]
fn words_for_people_are_not_the_shape_and_a_member_more_is() {
    #[derive(JsonSchema)]
    #[allow(dead_code, reason = "only the schema is read")]
    struct One {
        /// Said one way.
        id: String,
        state: Word,
    }
    #[derive(JsonSchema)]
    #[allow(dead_code, reason = "only the schema is read")]
    struct Other {
        /// Said another way.
        id: String,
        state: SaidWord,
    }
    #[derive(JsonSchema)]
    #[allow(dead_code, reason = "only the schema is read")]
    struct Wider {
        id: String,
        state: Word,
        label: Option<String>,
    }
    #[derive(JsonSchema)]
    #[serde(rename_all = "snake_case")]
    #[allow(dead_code, reason = "only the schema is read")]
    enum Word {
        Open,
        Closed,
    }
    #[derive(JsonSchema)]
    #[serde(rename_all = "snake_case")]
    #[allow(dead_code, reason = "only the schema is read")]
    enum SaidWord {
        /// It is open.
        Open,
        /// It is closed.
        Closed,
    }
    assert_eq!(
        shape::<One>(),
        shape::<Other>(),
        "the words for people are not the shape"
    );
    assert_ne!(shape::<One>(), shape::<Wider>());
}

#[test]
fn error_details_are_optional_and_round_trip_through_both_contracts() {
    let before = serde_json::json!({"code":"harness_diverged","message":"A native setting changed","retryable":false});
    let agent: events::ErrorPayload = serde_json::from_value(before.clone()).unwrap();
    assert!(agent.details.is_none());
    assert_eq!(serde_json::to_value(agent).unwrap(), before);
    let with_details = serde_json::json!({"code":"mcp_server_failed","message":"A server failed","retryable":false,"details":{"status":"failed"}});
    let agent: events::ErrorPayload = serde_json::from_value(with_details.clone()).unwrap();
    let api: threads::ErrorPayload =
        serde_json::from_value(serde_json::to_value(agent).unwrap()).unwrap();
    assert_eq!(serde_json::to_value(api).unwrap(), with_details);
}
