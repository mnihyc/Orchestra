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
    tool_call_payload: events::ToolCallPayload => threads::ToolCallPayload,
    tool_payload: events::ToolPayload => threads::ToolPayload,
    turn_phase_kind: events::TurnPhaseKind => threads::TurnPhaseKind,
    web_search_result: events::WebSearchResult => threads::WebSearchResult,
);

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
