//! The agent's requests to be allowed something and to ask a person something, as Orch's approvals, and the person's answers as the agent's.
//!
//! **Permission.** `session/request_permission` names a tool call and offers options. The adapter shows what the
//! call is (a command, a change of files, a plan to proceed with, or something else) with the agent's options as
//! they are: each is allowed or refused, once or for good. The answer is the option's id, with the person's words
//! in `_meta.orch.note` for an agent that reads them.
//!
//! **Questions.** `elicitation/create` in form mode becomes questions: a field with a fixed set of values is a
//! choice, a switch is Yes or No, a list of such values allows several, anything else is answered freely. What
//! the person chooses is mapped back to the values the schema names. A companion plugin's `_orch/question/ask`
//! is the same card with the plugin's own questions and answers.
//!
//! **A plan to review.** A companion plugin's `_orch/plan/review` shows the plan with Proceed and Keep planning;
//! the person's words with Keep planning go back as the feedback.

use std::collections::BTreeMap;

use orch_agent_contract::adapter::AdapterError;
use orch_agent_contract::events::{
    ApprovalOption, HarnessRequest, HarnessRequestDetail, HarnessRequestKind, OptionScope,
    OptionStyle, Question, QuestionOption,
};
use orch_agent_contract::session::{ApprovalDecision, ElicitationAction};
use orch_types::Json;
use serde_json::json;

use crate::program::ToolFamily;
use crate::tools::{Place, ToolView, Understood, call_title, content_output, file_changes};

/// One option of a permission request as the agent offered it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfferedOption {
    /// The agent's id for it.
    pub id: String,
    /// Whether it allows.
    pub allows: bool,
}

/// What the code of a question that was declined is: the person did not answer.
pub const DECLINED: i64 = -32001;

/// Whose words a request is answered in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// The protocol's `session/request_permission`.
    Protocol,
    /// A companion plugin's `_orch/plan/review`.
    PlanReview,
}

/// A permission request as it is shown, and what is needed to answer it.
#[derive(Debug, Clone)]
pub struct PermissionRequest {
    /// How it is answered.
    pub style: Style,
    /// The line shown.
    pub title: String,
    /// What is asked.
    pub request: HarnessRequest,
    /// The options the agent offered.
    pub offered: Vec<OfferedOption>,
    /// The call it is about.
    pub tool_call_id: String,
}

/// Reads the options of a permission request.
fn options_of(params: &Json) -> (Vec<ApprovalOption>, Vec<OfferedOption>) {
    let mut shown = Vec::new();
    let mut offered = Vec::new();
    for option in params
        .get("options")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
    {
        let Some(id) = option.get("optionId").and_then(Json::as_str) else {
            continue;
        };
        let kind = option
            .get("kind")
            .and_then(Json::as_str)
            .unwrap_or("allow_once");
        let allows = kind.starts_with("allow");
        shown.push(ApprovalOption {
            id: id.to_owned(),
            label: option
                .get("name")
                .and_then(Json::as_str)
                .unwrap_or(id)
                .to_owned(),
            style: if allows {
                OptionStyle::Allow
            } else {
                OptionStyle::Deny
            },
            scope: Some(if kind.ends_with("always") {
                OptionScope::Always
            } else {
                OptionScope::Once
            }),
            rule: None,
        });
        offered.push(OfferedOption {
            id: id.to_owned(),
            allows,
        });
    }
    (shown, offered)
}

/// What a `session/request_permission` asks, shown for the person.
///
/// `view` is the call the request names, as the adapter knows it: from the updates that came before, with what
/// the request itself says laid over it.
#[must_use]
pub fn permission(view: &ToolView, params: &Json, place: &Place<'_>) -> PermissionRequest {
    let (options, offered) = options_of(params);
    let reason = ["justification", "reason", "description"]
        .iter()
        .find_map(|member| view.raw_input.get(*member).and_then(Json::as_str))
        .filter(|text| !text.is_empty())
        .map(str::to_owned);
    let title = call_title(view, place);
    let (request_kind, detail) = if view.kind == "switch_mode" {
        let plan = content_output(&view.content);
        (
            HarnessRequestKind::Plan,
            HarnessRequestDetail::Plan {
                plan: if plan.is_empty() { title.clone() } else { plan },
            },
        )
    } else {
        match &view.understood {
            Understood::Family(ToolFamily::Command) | Understood::Exec(ToolFamily::Command) => (
                HarnessRequestKind::Command,
                HarnessRequestDetail::Command {
                    command: ["command", "cmd"]
                        .iter()
                        .find_map(|member| view.raw_input.get(*member).and_then(Json::as_str))
                        .unwrap_or(&title)
                        .to_owned(),
                    cwd: view
                        .raw_input
                        .get("workdir")
                        .or_else(|| view.raw_input.get("cwd"))
                        .and_then(Json::as_str)
                        .unwrap_or(place.cwd)
                        .to_owned(),
                    reason,
                },
            ),
            Understood::Family(family @ (ToolFamily::Edit | ToolFamily::Write))
            | Understood::Exec(family @ (ToolFamily::Edit | ToolFamily::Write)) => (
                HarnessRequestKind::FileChange,
                HarnessRequestDetail::FileChange {
                    changes: file_changes(view, *family, place),
                    reason,
                },
            ),
            _ => (
                HarnessRequestKind::Other,
                HarnessRequestDetail::Other {
                    summary: title.clone(),
                    raw: params.get("toolCall").cloned().unwrap_or(Json::Null),
                },
            ),
        }
    };
    PermissionRequest {
        style: Style::Protocol,
        title,
        request: HarnessRequest {
            request_kind,
            options,
            detail,
        },
        offered,
        tool_call_id: view.id.clone(),
    }
}

/// The plan a companion plugin put up for review, shown with Proceed and Keep planning.
#[must_use]
pub fn plan_review(params: &Json) -> Option<PermissionRequest> {
    let call = params.get("callId").and_then(Json::as_str)?;
    let plan = params.get("plan").and_then(Json::as_str)?;
    let title: String = plan
        .lines()
        .map(|line| line.trim_start_matches('#').trim())
        .find(|line| !line.is_empty())
        .unwrap_or("Plan")
        .chars()
        .take(120)
        .collect();
    let option = |id: &str, label: &str, style: OptionStyle| ApprovalOption {
        id: id.to_owned(),
        label: label.to_owned(),
        style,
        scope: Some(OptionScope::Once),
        rule: None,
    };
    Some(PermissionRequest {
        style: Style::PlanReview,
        title,
        request: HarnessRequest {
            request_kind: HarnessRequestKind::Plan,
            options: vec![
                option("proceed", "Proceed", OptionStyle::Allow),
                option("keep_planning", "Keep planning", OptionStyle::Deny),
            ],
            detail: HarnessRequestDetail::Plan {
                plan: plan.to_owned(),
            },
        },
        offered: vec![
            OfferedOption {
                id: "proceed".to_owned(),
                allows: true,
            },
            OfferedOption {
                id: "keep_planning".to_owned(),
                allows: false,
            },
        ],
        tool_call_id: call.to_owned(),
    })
}

/// The answer of a person to a permission request, as the agent is answered.
pub fn permission_answer(
    pending: &PermissionRequest,
    decision: &ApprovalDecision,
) -> Result<Json, AdapterError> {
    let ApprovalDecision::Option { option_id, note } = decision else {
        return Err(invalid(
            "a request to be allowed something is answered by one of its options",
        ));
    };
    if !pending
        .offered
        .iter()
        .any(|offered| &offered.id == option_id)
    {
        return Err(invalid(&format!(
            "{option_id:?} is not one of the options the agent offered"
        )));
    }
    let note = note.as_deref().filter(|note| !note.trim().is_empty());
    if pending.style == Style::PlanReview {
        let mut answer = json!({ "decision": option_id });
        if let Some(note) = note {
            answer["feedback"] = json!(note);
        }
        return Ok(answer);
    }
    let mut outcome = json!({ "outcome": "selected", "optionId": option_id });
    if let Some(note) = note {
        outcome["_meta"] = crate::extensions::meta(json!({ "note": note }));
    }
    Ok(json!({ "outcome": outcome }))
}

/// The answer to a permission request that was withdrawn: the turn was cancelled before anybody answered.
#[must_use]
pub fn permission_cancelled() -> Json {
    json!({ "outcome": { "outcome": "cancelled" } })
}

/// The answer to a request of `pending`'s kind that was withdrawn.
#[must_use]
pub fn withdrawn(pending: &PermissionRequest) -> Json {
    match pending.style {
        Style::Protocol => permission_cancelled(),
        Style::PlanReview => json!({ "decision": "dismiss" }),
    }
}

fn invalid(message: &str) -> AdapterError {
    AdapterError::Other {
        code: "invalid_input".to_owned(),
        message: message.to_owned(),
        retryable: false,
    }
}

// -- questions ----------------------------------------------------------------------------------------------

/// How a field of a form is answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    /// A text, or one of a set of texts.
    Text,
    /// A number with a fraction.
    Number,
    /// A whole number.
    Integer,
    /// Yes or no.
    Boolean,
    /// Several of a set.
    Several,
}

/// One field of a form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    /// Its name, which the answer names.
    pub name: String,
    /// How it is answered.
    pub kind: FieldKind,
    /// The values it may take, each with the label a person is shown.
    pub choices: Vec<(String, String)>,
}

/// Whose words a form is answered in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormStyle {
    /// The protocol's `elicitation/create`.
    Elicitation,
    /// A companion plugin's `_orch/question/ask`.
    Questions,
}

/// How a form is answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    /// The result of the request.
    Result(Json),
    /// The person did not answer: the request is answered with [`DECLINED`].
    Declined,
}

/// A form as it is shown, and what is needed to answer it.
#[derive(Debug, Clone)]
pub struct Form {
    /// How it is answered.
    pub style: FormStyle,
    /// The call that asked, where the plugin names one.
    pub call_id: Option<String>,
    /// The line shown.
    pub title: String,
    /// What is asked.
    pub request: HarnessRequest,
    /// The fields.
    pub fields: Vec<Field>,
}

fn choices_of(schema: &Json) -> Vec<(String, String)> {
    let mut choices = Vec::new();
    for entry in schema
        .get("oneOf")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(value) = entry.get("const").and_then(Json::as_str) {
            let label = entry.get("title").and_then(Json::as_str).unwrap_or(value);
            choices.push((value.to_owned(), label.to_owned()));
        }
    }
    if choices.is_empty() {
        for value in schema
            .get("enum")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
            .filter_map(Json::as_str)
        {
            choices.push((value.to_owned(), value.to_owned()));
        }
    }
    choices
}

/// What an `elicitation/create` in form mode asks, shown as questions. A form that names `_meta.orch.questions`
/// is shown as the questions it names.
#[must_use]
pub fn elicitation(params: &Json) -> Option<Form> {
    let message = params
        .get("message")
        .and_then(Json::as_str)
        .unwrap_or_default();
    let schema = params.get("requestedSchema")?;
    let properties = schema.get("properties").and_then(Json::as_object)?;
    let mut fields = Vec::new();
    let mut questions = Vec::new();
    let single = properties.len() == 1;
    for (name, property) in properties {
        let kind_word = property
            .get("type")
            .and_then(Json::as_str)
            .unwrap_or("string");
        let (kind, choices) = match kind_word {
            "boolean" => (FieldKind::Boolean, Vec::new()),
            "number" => (FieldKind::Number, Vec::new()),
            "integer" => (FieldKind::Integer, Vec::new()),
            "array" => (
                FieldKind::Several,
                property.get("items").map(choices_of).unwrap_or_default(),
            ),
            _ => (FieldKind::Text, choices_of(property)),
        };
        let title = property.get("title").and_then(Json::as_str).unwrap_or(name);
        let description = property.get("description").and_then(Json::as_str);
        let question_text = match (single, description) {
            (true, _) if !message.is_empty() => message.to_owned(),
            (_, Some(description)) => description.to_owned(),
            _ => title.to_owned(),
        };
        let options = match kind {
            FieldKind::Boolean => Some(vec![
                QuestionOption {
                    label: "Yes".to_owned(),
                    description: String::new(),
                },
                QuestionOption {
                    label: "No".to_owned(),
                    description: String::new(),
                },
            ]),
            _ if !choices.is_empty() => Some(
                choices
                    .iter()
                    .map(|(_, label)| QuestionOption {
                        label: label.clone(),
                        description: String::new(),
                    })
                    .collect(),
            ),
            _ => None,
        };
        questions.push(Question {
            id: name.clone(),
            header: title.chars().take(30).collect(),
            question: question_text,
            multiple: kind == FieldKind::Several,
            allow_other: options.is_none(),
            options,
        });
        fields.push(Field {
            name: name.clone(),
            kind,
            choices,
        });
    }
    Some(Form {
        style: FormStyle::Elicitation,
        call_id: None,
        title: if message.is_empty() {
            "Question".to_owned()
        } else {
            message
                .lines()
                .next()
                .unwrap_or_default()
                .chars()
                .take(120)
                .collect()
        },
        request: HarnessRequest {
            request_kind: HarnessRequestKind::UserInput,
            options: Vec::new(),
            detail: HarnessRequestDetail::UserInput {
                native_item_ref: params
                    .get("toolCallId")
                    .and_then(Json::as_str)
                    .map(|id| format!("tool:{id}")),
                questions,
            },
        },
        fields,
    })
}

/// The questions a companion plugin asks, shown as one card.
#[must_use]
pub fn questions(params: &Json) -> Option<Form> {
    let call = params.get("callId").and_then(Json::as_str)?;
    let mut fields = Vec::new();
    let mut shown = Vec::new();
    for question in params.get("questions").and_then(Json::as_array)? {
        let id = question.get("id").and_then(Json::as_str)?;
        let text = question
            .get("question")
            .and_then(Json::as_str)
            .unwrap_or(id);
        let detail = question
            .get("detail")
            .and_then(Json::as_str)
            .filter(|detail| !detail.is_empty());
        let header = question.get("header").and_then(Json::as_str).unwrap_or(id);
        let many = question
            .get("multiSelect")
            .and_then(Json::as_bool)
            .unwrap_or(false);
        let choices: Vec<(String, String)> = question
            .get("options")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
            .filter_map(|option| {
                let label = option.get("label").and_then(Json::as_str)?;
                Some((label.to_owned(), label.to_owned()))
            })
            .collect();
        let options = (!choices.is_empty()).then(|| {
            question
                .get("options")
                .and_then(Json::as_array)
                .into_iter()
                .flatten()
                .filter_map(|option| {
                    Some(QuestionOption {
                        label: option.get("label").and_then(Json::as_str)?.to_owned(),
                        description: option
                            .get("description")
                            .and_then(Json::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                    })
                })
                .collect::<Vec<_>>()
        });
        shown.push(Question {
            id: id.to_owned(),
            header: header.chars().take(30).collect(),
            question: match detail {
                Some(detail) => format!("{text}\n\n{detail}"),
                None => text.to_owned(),
            },
            multiple: many,
            allow_other: true,
            options,
        });
        fields.push(Field {
            name: id.to_owned(),
            kind: if many {
                FieldKind::Several
            } else {
                FieldKind::Text
            },
            choices,
        });
    }
    let title = shown
        .first()?
        .question
        .lines()
        .next()
        .unwrap_or("Question")
        .chars()
        .take(120)
        .collect();
    Some(Form {
        style: FormStyle::Questions,
        call_id: Some(call.to_owned()),
        title,
        request: HarnessRequest {
            request_kind: HarnessRequestKind::UserInput,
            options: Vec::new(),
            detail: HarnessRequestDetail::UserInput {
                native_item_ref: Some(format!("tool:{call}")),
                questions: shown,
            },
        },
        fields,
    })
}

fn value_of(field: &Field, shown: &str) -> Json {
    let by_label = field
        .choices
        .iter()
        .find(|(value, label)| label == shown || value == shown)
        .map(|(value, _)| value.clone());
    match field.kind {
        FieldKind::Boolean => Json::Bool(matches!(
            shown.trim().to_ascii_lowercase().as_str(),
            "yes" | "true" | "y" | "1"
        )),
        FieldKind::Integer => shown
            .trim()
            .parse::<i64>()
            .map_or_else(|_| Json::String(shown.to_owned()), Json::from),
        FieldKind::Number => shown
            .trim()
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map_or_else(|| Json::String(shown.to_owned()), Json::Number),
        FieldKind::Text | FieldKind::Several => {
            Json::String(by_label.unwrap_or_else(|| shown.to_owned()))
        }
    }
}

/// The answer of a person to a form, as the agent is answered.
pub fn elicitation_answer(form: &Form, decision: &ApprovalDecision) -> Result<Json, AdapterError> {
    match decision {
        ApprovalDecision::Answers { answers, .. } => {
            let mut content = serde_json::Map::new();
            for field in &form.fields {
                let Some(given) = answers.get(&field.name).filter(|given| !given.is_empty()) else {
                    continue;
                };
                let value = if field.kind == FieldKind::Several {
                    Json::Array(given.iter().map(|shown| value_of(field, shown)).collect())
                } else {
                    value_of(field, &given[0])
                };
                content.insert(field.name.clone(), value);
            }
            Ok(json!({ "action": "accept", "content": content }))
        }
        ApprovalDecision::Elicitation { action, content } => Ok(match action {
            ElicitationAction::Accept => {
                json!({ "action": "accept", "content": content.clone().unwrap_or_else(|| json!({})) })
            }
            ElicitationAction::Decline => json!({ "action": "decline" }),
            ElicitationAction::Cancel => json!({ "action": "cancel" }),
        }),
        ApprovalDecision::Option { option_id, .. }
            if matches!(option_id.as_str(), "decline" | "cancel") =>
        {
            Ok(json!({ "action": option_id }))
        }
        ApprovalDecision::Option { .. } => Err(invalid("a question is answered by its answers")),
    }
}

/// The answer of a person to a form, in the words the form's own request is answered in.
pub fn reply(form: &Form, decision: &ApprovalDecision) -> Result<Reply, AdapterError> {
    if form.style == FormStyle::Elicitation {
        return elicitation_answer(form, decision).map(Reply::Result);
    }
    match decision {
        ApprovalDecision::Answers { answers, .. } => {
            let mut given = Vec::new();
            for field in &form.fields {
                let shown = answers
                    .get(&field.name)
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                let selected: Vec<&String> = shown
                    .iter()
                    .filter(|shown| field.choices.iter().any(|(label, _)| label == *shown))
                    .collect();
                let own = shown
                    .iter()
                    .find(|shown| !field.choices.iter().any(|(label, _)| label == *shown));
                let mut answer = json!({ "id": field.name, "selected": selected });
                if let Some(own) = own {
                    answer["custom"] = json!(own);
                }
                given.push(answer);
            }
            Ok(Reply::Result(json!({ "answers": given })))
        }
        ApprovalDecision::Elicitation {
            action: ElicitationAction::Decline | ElicitationAction::Cancel,
            ..
        } => Ok(Reply::Declined),
        ApprovalDecision::Option { option_id, .. }
            if matches!(option_id.as_str(), "decline" | "cancel") =>
        {
            Ok(Reply::Declined)
        }
        _ => Err(invalid("a question is answered by its answers")),
    }
}

/// The answers a person gave, by the labels they chose, as `{ question id: [labels] }`.
#[must_use]
pub fn answers_of(pairs: &[(&str, &[&str])]) -> BTreeMap<String, Vec<String>> {
    pairs
        .iter()
        .map(|(id, values)| {
            (
                (*id).to_owned(),
                values.iter().map(|value| (*value).to_owned()).collect(),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::program::AcpProgram;
    use crate::tools::{Status, understand};

    fn program() -> AcpProgram {
        AcpProgram::from_json(&json!({
            "kind": "demo", "title": "Demo", "supported_range": "*",
            "tools": [ { "names": ["bash"], "family": "command" }, { "names": ["write"], "family": "write" } ]
        }))
        .unwrap()
    }

    fn view(name: &str, kind: &str, input: Json, place: &Place<'_>) -> ToolView {
        ToolView {
            id: "call_1".into(),
            name: name.into(),
            title: name.into(),
            kind: kind.into(),
            understood: understand(place, name, name, kind),
            status: Status::InProgress,
            raw_input: input,
            raw_output: None,
            content: Vec::new(),
            locations: Vec::new(),
        }
    }

    fn place(program: &AcpProgram) -> Place<'_> {
        Place {
            program,
            directory: "/w",
            cwd: "/w",
            servers: &[],
        }
    }

    fn params() -> Json {
        json!({
            "sessionId": "s", "toolCall": { "toolCallId": "call_1" },
            "options": [
                { "optionId": "allow-once", "name": "Allow once", "kind": "allow_once" },
                { "optionId": "reject-once", "name": "Reject", "kind": "reject_once" } ]
        })
    }

    #[test]
    fn a_command_that_asks_is_shown_with_its_text_its_reason_and_the_agents_own_options() {
        let program = program();
        let place = place(&program);
        let view = view(
            "bash",
            "other",
            json!({ "command": "rm -rf build", "description": "clean", "justification": "the build is stale" }),
            &place,
        );
        let asked = permission(&view, &params(), &place);
        assert_eq!(asked.request.request_kind, HarnessRequestKind::Command);
        let HarnessRequestDetail::Command {
            command, reason, ..
        } = &asked.request.detail
        else {
            panic!()
        };
        assert_eq!(command, "rm -rf build");
        assert_eq!(reason.as_deref(), Some("the build is stale"));
        assert_eq!(asked.request.options.len(), 2);
        assert_eq!(asked.request.options[0].style, OptionStyle::Allow);
        assert_eq!(asked.request.options[1].style, OptionStyle::Deny);
        assert_eq!(asked.request.first_allowing().unwrap().id, "allow-once");
    }

    #[test]
    fn the_answer_is_the_option_chosen_with_the_persons_words_beside_it() {
        let program = program();
        let place = place(&program);
        let asked = permission(
            &view("bash", "other", json!({ "command": "ls" }), &place),
            &params(),
            &place,
        );
        let allowed = permission_answer(
            &asked,
            &ApprovalDecision::Option {
                option_id: "allow-once".into(),
                note: None,
            },
        )
        .unwrap();
        assert_eq!(
            allowed,
            json!({ "outcome": { "outcome": "selected", "optionId": "allow-once" } })
        );
        let refused = permission_answer(
            &asked,
            &ApprovalDecision::Option {
                option_id: "reject-once".into(),
                note: Some("not that".into()),
            },
        )
        .unwrap();
        assert_eq!(refused["outcome"]["_meta"]["orch"]["note"], "not that");
        assert!(
            permission_answer(
                &asked,
                &ApprovalDecision::Option {
                    option_id: "other".into(),
                    note: None
                }
            )
            .is_err()
        );
        assert_eq!(
            permission_cancelled(),
            json!({ "outcome": { "outcome": "cancelled" } })
        );
    }

    #[test]
    fn a_request_to_leave_a_mode_is_a_plan_to_proceed_with() {
        let program = program();
        let place = place(&program);
        let mut plan = view("ExitPlanMode", "switch_mode", json!({}), &place);
        plan.content = vec![
            json!({ "type": "content", "content": { "type": "text", "text": "# Plan\n1. a" } }),
        ];
        let asked = permission(&plan, &params(), &place);
        assert_eq!(asked.request.request_kind, HarnessRequestKind::Plan);
        let HarnessRequestDetail::Plan { plan } = &asked.request.detail else {
            panic!()
        };
        assert!(plan.starts_with("# Plan"));
    }

    #[test]
    fn a_write_that_asks_shows_the_change() {
        let program = program();
        let place = place(&program);
        let asked = permission(
            &view(
                "write",
                "other",
                json!({ "file_path": "/w/a.txt", "content": "x\n" }),
                &place,
            ),
            &params(),
            &place,
        );
        let HarnessRequestDetail::FileChange { changes, .. } = &asked.request.detail else {
            panic!()
        };
        assert_eq!(changes[0].path, "a.txt");
        assert_eq!(changes[0].added, 1);
    }

    fn form() -> Json {
        json!({
            "message": "Which database?", "mode": "form",
            "requestedSchema": { "type": "object", "required": ["db"], "properties": {
                "db": { "type": "string", "title": "Database", "oneOf": [
                    { "const": "pg", "title": "Postgres" }, { "const": "my", "title": "MySQL" } ] },
                "tests": { "type": "boolean", "title": "Run the tests" },
                "tags": { "type": "array", "title": "Tags", "items": { "enum": ["a", "b"] } },
                "note": { "type": "string", "title": "A note" },
                "retries": { "type": "integer", "title": "Retries" } } }
        })
    }

    #[test]
    fn a_form_is_shown_as_questions_and_answered_with_the_values_it_names() {
        let form = elicitation(&form()).unwrap();
        let HarnessRequestDetail::UserInput { questions, .. } = &form.request.detail else {
            panic!()
        };
        assert_eq!(questions.len(), 5);
        let db = questions.iter().find(|q| q.id == "db").unwrap();
        assert_eq!(db.options.as_ref().unwrap()[0].label, "Postgres");
        assert!(!db.allow_other);
        assert!(
            questions
                .iter()
                .find(|q| q.id == "note")
                .unwrap()
                .allow_other
        );
        assert!(questions.iter().find(|q| q.id == "tags").unwrap().multiple);
        let answered = elicitation_answer(
            &form,
            &ApprovalDecision::Answers {
                answers: answers_of(&[
                    ("db", &["Postgres"]),
                    ("tests", &["Yes"]),
                    ("tags", &["a", "b"]),
                    ("note", &["hi"]),
                    ("retries", &["3"]),
                ]),
                note: None,
            },
        )
        .unwrap();
        assert_eq!(
            answered,
            json!({ "action": "accept", "content": { "db": "pg", "tests": true, "tags": ["a", "b"], "note": "hi", "retries": 3 } })
        );
        assert_eq!(
            elicitation_answer(
                &form,
                &ApprovalDecision::Elicitation {
                    action: ElicitationAction::Decline,
                    content: None
                }
            )
            .unwrap(),
            json!({ "action": "decline" })
        );
    }

    fn asked() -> Json {
        json!({
            "sessionId": "s", "callId": "call_7",
            "questions": [
                { "id": "db", "question": "Which database?", "detail": "It decides the driver.", "header": "Database",
                  "options": [ { "label": "Postgres", "description": "the default" }, { "label": "MySQL" } ] },
                { "id": "tags", "question": "Which tags?", "multiSelect": true, "options": [ { "label": "a" }, { "label": "b" } ] },
                { "id": "why", "question": "Why?" }
            ]
        })
    }

    #[test]
    fn the_questions_of_a_plugin_are_one_card_and_answered_by_label_or_in_the_persons_own_words() {
        let form = questions(&asked()).unwrap();
        assert_eq!(form.style, FormStyle::Questions);
        assert_eq!(form.call_id.as_deref(), Some("call_7"));
        let HarnessRequestDetail::UserInput {
            questions,
            native_item_ref,
        } = &form.request.detail
        else {
            panic!()
        };
        assert_eq!(native_item_ref.as_deref(), Some("tool:call_7"));
        assert_eq!(
            questions[0].question,
            "Which database?\n\nIt decides the driver."
        );
        assert_eq!(
            questions[0].options.as_ref().unwrap()[0].description,
            "the default"
        );
        assert!(questions[1].multiple && questions[2].options.is_none());
        let Reply::Result(answer) = reply(
            &form,
            &ApprovalDecision::Answers {
                answers: answers_of(&[
                    ("db", &["MySQL"]),
                    ("tags", &["a", "b"]),
                    ("why", &["because"]),
                ]),
                note: None,
            },
        )
        .unwrap() else {
            panic!()
        };
        assert_eq!(
            answer["answers"][0],
            json!({ "id": "db", "selected": ["MySQL"] })
        );
        assert_eq!(
            answer["answers"][1],
            json!({ "id": "tags", "selected": ["a", "b"] })
        );
        assert_eq!(
            answer["answers"][2],
            json!({ "id": "why", "selected": [], "custom": "because" })
        );
        let Reply::Result(own) = reply(
            &form,
            &ApprovalDecision::Answers {
                answers: answers_of(&[("db", &["SQLite"])]),
                note: None,
            },
        )
        .unwrap() else {
            panic!()
        };
        assert_eq!(
            own["answers"][0],
            json!({ "id": "db", "selected": [], "custom": "SQLite" })
        );
        assert_eq!(
            reply(
                &form,
                &ApprovalDecision::Elicitation {
                    action: ElicitationAction::Cancel,
                    content: None
                }
            )
            .unwrap(),
            Reply::Declined
        );
    }

    #[test]
    fn a_plan_a_plugin_puts_up_is_reviewed_with_proceed_and_keep_planning() {
        let asked = plan_review(
            &json!({ "sessionId": "s", "callId": "c1", "plan": "# Move the tests\n1. a\n2. b" }),
        )
        .unwrap();
        assert_eq!(asked.title, "Move the tests");
        assert_eq!(asked.request.request_kind, HarnessRequestKind::Plan);
        let ids: Vec<&str> = asked
            .request
            .options
            .iter()
            .map(|option| option.id.as_str())
            .collect();
        assert_eq!(ids, ["proceed", "keep_planning"]);
        assert_eq!(
            permission_answer(
                &asked,
                &ApprovalDecision::Option {
                    option_id: "proceed".into(),
                    note: None
                }
            )
            .unwrap(),
            json!({ "decision": "proceed" })
        );
        assert_eq!(
            permission_answer(
                &asked,
                &ApprovalDecision::Option {
                    option_id: "keep_planning".into(),
                    note: Some("use sqlite".into())
                }
            )
            .unwrap(),
            json!({ "decision": "keep_planning", "feedback": "use sqlite" })
        );
        assert_eq!(withdrawn(&asked), json!({ "decision": "dismiss" }));
        assert!(plan_review(&json!({ "callId": "c1" })).is_none());
    }

    #[test]
    fn a_form_with_one_field_asks_in_the_words_of_its_message() {
        let one = elicitation(&json!({
            "message": "What should the file be called?",
            "requestedSchema": { "type": "object", "properties": { "name": { "type": "string" } } }
        }))
        .unwrap();
        let HarnessRequestDetail::UserInput { questions, .. } = &one.request.detail else {
            panic!()
        };
        assert_eq!(questions[0].question, "What should the file be called?");
    }
}
