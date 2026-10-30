//! Map harness approval and user-input requests.
//!
//! The harness asks, as a request of its own that waits for an answer: to run a command, to change files, to
//! be given more permissions, to put questions to the person, and, for an MCP server, to ask for input. Each
//! is reported as an approval with the options a person may choose among ([`translate`]); the id of an option
//! is the adapter's, and the answer that comes back names one of them ([`answer`]).
//!
//! * A command offers what the harness lists as its decisions (allow once; allow for the session; allow
//!   commands like this one from now on; allow or block a host), and always a plain **Deny**, which is
//!   `decline`: the harness takes it though it does not list it, the command is not run, and the turn goes
//!   on with the refusal told to the model. A second refusal, **Deny and stop the turn**, is `cancel`.
//! * A change of files offers the same four. The request itself names no file; the changes are those of the
//!   item it is about, which the mapper holds.
//! * A request for more permissions can be granted for this turn or for the session, or refused.
//! * Questions are answered by choosing among their options (or writing an answer of one's own where the
//!   question allows it), and an MCP server's request for input is accepted with what was asked for,
//!   declined or cancelled.
//!
//! A request the adapter does not know is refused at once, so that the harness is not left waiting for it.

use std::collections::BTreeMap;

use orch_agent_contract::adapter::AdapterError;
use orch_agent_contract::events::{
    ApprovalOption, FileChange, HarnessRequest, HarnessRequestDetail, HarnessRequestKind,
    OptionScope, OptionStyle, Question, QuestionOption,
};
use orch_agent_contract::session::{ApprovalDecision, ElicitationAction};
use orch_types::Json;
use serde_json::json;

use crate::events::{display_command, first_line};

/// The id of the option that allows once.
pub const ONCE: &str = "once";
/// The id of the option that allows for the rest of the session.
pub const SESSION: &str = "session";
/// The id of the option that allows commands like this one from now on.
pub const ALWAYS: &str = "always";
/// The id of the option that allows a host from now on.
pub const HOST_ALLOW: &str = "host_allow";
/// The id of the option that blocks a host from now on.
pub const HOST_DENY: &str = "host_deny";
/// The id of the option that refuses and lets the turn go on.
pub const DENY: &str = "deny";
/// The id of the option that refuses and stops the turn.
pub const ABORT: &str = "abort";

/// What the adapter keeps of a request until it is answered.
#[derive(Debug, Clone, PartialEq)]
pub struct Pending {
    /// The kind of request, which decides the shape of the answer.
    pub kind: PendingKind,
    /// The harness's thread the request is about.
    pub native_thread: String,
    /// The item the request is about, where it names one.
    pub item: Option<String>,
    /// For the options of a command or a change: the decision each option stands for.
    pub decisions: BTreeMap<String, Json>,
    /// Whether it has been answered, so that the harness's note that it is settled is not a withdrawal.
    pub answered: bool,
    /// Whether it was reported as withdrawn, so that the note that follows is not reported again and an
    /// answer that comes too late is refused.
    pub closed: bool,
}

/// The kinds of request the adapter answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingKind {
    /// To run a command.
    Command,
    /// To change files.
    FileChange,
    /// To be given more permissions; holds what was asked for.
    Permissions(Json),
    /// Questions for the person; holds their ids.
    UserInput(Vec<String>),
    /// An MCP server's request for input.
    Elicitation,
}

/// A request of the harness, as the neutral model has it.
#[derive(Debug, Clone, PartialEq)]
pub struct Translated {
    /// The line shown for it.
    pub title: String,
    /// What is asked.
    pub request: HarnessRequest,
    /// What the adapter keeps.
    pub pending: Pending,
}

fn option(id: &str, label: &str, style: OptionStyle, scope: Option<OptionScope>) -> ApprovalOption {
    ApprovalOption {
        id: id.to_owned(),
        label: label.to_owned(),
        style,
        scope,
        // No matcher of this harness is verified on a later request yet: nothing is offered for the thread.
        rule: None,
    }
}

fn text<'a>(value: &'a Json, key: &str) -> &'a str {
    value.get(key).and_then(Json::as_str).unwrap_or_default()
}

fn opt_text(value: &Json, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Json::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

/// The decisions a command or a change offers, in the order they are shown: what allows first, then what
/// refuses.
fn decision_options(
    listed: Option<&Vec<Json>>,
    with_amendments: bool,
) -> (Vec<ApprovalOption>, BTreeMap<String, Json>) {
    let default = [
        json!("accept"),
        json!("acceptForSession"),
        json!("decline"),
        json!("cancel"),
    ];
    let listed: Vec<&Json> = match listed {
        Some(listed) => listed.iter().collect(),
        None => default.iter().collect(),
    };
    let mut options = Vec::new();
    let mut decisions = BTreeMap::new();
    let mut add = |id: &str, label: &str, style, scope, decision: Json| {
        options.push(option(id, label, style, scope));
        decisions.insert(id.to_owned(), decision);
    };
    for decision in listed {
        match decision {
            Json::String(word) if word == "accept" => {
                add(
                    ONCE,
                    "Allow once",
                    OptionStyle::Allow,
                    Some(OptionScope::Once),
                    decision.clone(),
                );
            }
            Json::String(word) if word == "acceptForSession" => add(
                SESSION,
                "Allow for this session",
                OptionStyle::Allow,
                Some(OptionScope::Session),
                decision.clone(),
            ),
            Json::Object(members)
                if with_amendments && members.contains_key("acceptWithExecpolicyAmendment") =>
            {
                add(
                    ALWAYS,
                    "Always allow commands like this",
                    OptionStyle::Allow,
                    Some(OptionScope::Always),
                    decision.clone(),
                )
            }
            Json::Object(members) if with_amendments => {
                if let Some(amendment) = members.get("applyNetworkPolicyAmendment") {
                    let allow = amendment
                        .get("network_policy_amendment")
                        .and_then(|amendment| amendment.get("action"))
                        .and_then(Json::as_str)
                        != Some("deny");
                    let host = amendment
                        .get("network_policy_amendment")
                        .map(|amendment| text(amendment, "host"))
                        .unwrap_or_default();
                    if allow {
                        add(
                            HOST_ALLOW,
                            &format!("Allow {host} from now on"),
                            OptionStyle::Allow,
                            Some(OptionScope::Always),
                            decision.clone(),
                        );
                    } else {
                        add(
                            HOST_DENY,
                            &format!("Block {host} from now on"),
                            OptionStyle::Deny,
                            Some(OptionScope::Always),
                            decision.clone(),
                        );
                    }
                }
            }
            _ => {}
        }
    }
    add(DENY, "Deny", OptionStyle::Deny, None, json!("decline"));
    add(
        ABORT,
        "Deny and stop the turn",
        OptionStyle::Deny,
        None,
        json!("cancel"),
    );
    (options, decisions)
}

fn mcp_schema(params: &Json) -> (String, Json) {
    let message = match text(params, "mode") {
        "openai/userVerification" => {
            format!("{}\n{}", text(params, "title"), text(params, "description"))
        }
        _ => text(params, "message").to_owned(),
    };
    let mut schema = json!({ "mode": text(params, "mode") });
    for key in ["requestedSchema", "url", "elicitationId", "challenge"] {
        if let Some(value) = params.get(key) {
            schema[key] = value.clone();
        }
    }
    (message, schema)
}

/// The approval a request of the harness is, or `None` for a request that is not one of those the adapter
/// answers. `changes` are the changes of the item the request is about, where it is a change of files.
#[must_use]
pub fn translate(method: &str, params: &Json, changes: Vec<FileChange>) -> Option<Translated> {
    let native_thread = text(params, "threadId").to_owned();
    let item = opt_text(params, "itemId");
    match method {
        "item/commandExecution/requestApproval" => {
            let command = text(params, "command");
            let shown = display_command(command);
            let listed = params.get("availableDecisions").and_then(Json::as_array);
            let (options, decisions) = decision_options(listed, true);
            Some(Translated {
                title: format!("Run {}", first_line(&shown)),
                request: HarnessRequest {
                    request_kind: HarnessRequestKind::Command,
                    options,
                    detail: HarnessRequestDetail::Command {
                        command: shown,
                        cwd: text(params, "cwd").to_owned(),
                        reason: opt_text(params, "reason"),
                    },
                },
                pending: Pending {
                    kind: PendingKind::Command,
                    native_thread,
                    item,
                    decisions,
                    answered: false,
                    closed: false,
                },
            })
        }
        "item/fileChange/requestApproval" => {
            let (options, decisions) = decision_options(None, false);
            let title = match changes.as_slice() {
                [] => "Change files".to_owned(),
                [one] => format!("Change {}", one.path),
                many => format!("Change {} files", many.len()),
            };
            Some(Translated {
                title,
                request: HarnessRequest {
                    request_kind: HarnessRequestKind::FileChange,
                    options,
                    detail: HarnessRequestDetail::FileChange {
                        changes,
                        reason: opt_text(params, "reason"),
                    },
                },
                pending: Pending {
                    kind: PendingKind::FileChange,
                    native_thread,
                    item,
                    decisions,
                    answered: false,
                    closed: false,
                },
            })
        }
        "item/permissions/requestApproval" => {
            let requested = params.get("permissions").cloned().unwrap_or(Json::Null);
            let options = vec![
                option(
                    ONCE,
                    "Allow for this turn",
                    OptionStyle::Allow,
                    Some(OptionScope::Once),
                ),
                option(
                    SESSION,
                    "Allow for this session",
                    OptionStyle::Allow,
                    Some(OptionScope::Session),
                ),
                option(DENY, "Deny", OptionStyle::Deny, None),
            ];
            Some(Translated {
                title: "Be given more permissions".to_owned(),
                request: HarnessRequest {
                    request_kind: HarnessRequestKind::Permissions,
                    options,
                    detail: HarnessRequestDetail::Permissions {
                        permissions: requested.clone(),
                        reason: opt_text(params, "reason"),
                    },
                },
                pending: Pending {
                    kind: PendingKind::Permissions(requested),
                    native_thread,
                    item,
                    decisions: BTreeMap::new(),
                    answered: false,
                    closed: false,
                },
            })
        }
        "item/tool/requestUserInput" => {
            let questions: Vec<Question> = params
                .get("questions")
                .and_then(Json::as_array)
                .map(|questions| {
                    questions
                        .iter()
                        .map(|question| Question {
                            id: text(question, "id").to_owned(),
                            header: text(question, "header").to_owned(),
                            question: text(question, "question").to_owned(),
                            multiple: false,
                            allow_other: question
                                .get("isOther")
                                .and_then(Json::as_bool)
                                .unwrap_or(false),
                            options: question.get("options").and_then(Json::as_array).map(
                                |options| {
                                    options
                                        .iter()
                                        .map(|choice| QuestionOption {
                                            label: text(choice, "label").to_owned(),
                                            description: text(choice, "description").to_owned(),
                                        })
                                        .collect()
                                },
                            ),
                        })
                        .collect()
                })
                .unwrap_or_default();
            let ids = questions
                .iter()
                .map(|question| question.id.clone())
                .collect();
            Some(Translated {
                title: questions.first().map_or_else(
                    || "A question for you".to_owned(),
                    |question| first_line(&question.question),
                ),
                request: HarnessRequest {
                    request_kind: HarnessRequestKind::UserInput,
                    options: Vec::new(),
                    detail: HarnessRequestDetail::UserInput {
                        questions,
                        native_item_ref: item.clone(),
                    },
                },
                pending: Pending {
                    kind: PendingKind::UserInput(ids),
                    native_thread,
                    item,
                    decisions: BTreeMap::new(),
                    answered: false,
                    closed: false,
                },
            })
        }
        "mcpServer/elicitation/request" => {
            let (message, schema) = mcp_schema(params);
            let server = text(params, "serverName").to_owned();
            Some(Translated {
                title: format!("{server} asks for input"),
                request: HarnessRequest {
                    request_kind: HarnessRequestKind::McpElicitation,
                    options: Vec::new(),
                    detail: HarnessRequestDetail::McpElicitation {
                        native_item_ref: item.clone(),
                        server,
                        message,
                        schema,
                    },
                },
                pending: Pending {
                    kind: PendingKind::Elicitation,
                    native_thread,
                    item,
                    decisions: BTreeMap::new(),
                    answered: false,
                    closed: false,
                },
            })
        }
        _ => None,
    }
}

fn unusable(what: &str) -> AdapterError {
    AdapterError::Other {
        code: "invalid_decision".to_owned(),
        message: what.to_owned(),
        retryable: false,
    }
}

/// The answer to give the harness for the decision a person made.
pub fn answer(pending: &Pending, decision: &ApprovalDecision) -> Result<Json, AdapterError> {
    match (&pending.kind, decision) {
        (
            PendingKind::Command | PendingKind::FileChange,
            ApprovalDecision::Option { option_id, .. },
        ) => {
            let chosen = pending
                .decisions
                .get(option_id)
                .ok_or_else(|| unusable(&format!("the request has no option {option_id:?}")))?;
            Ok(json!({ "decision": chosen }))
        }
        (PendingKind::Permissions(requested), ApprovalDecision::Option { option_id, .. }) => {
            match option_id.as_str() {
                ONCE => Ok(json!({ "permissions": requested, "scope": "turn" })),
                SESSION => Ok(json!({ "permissions": requested, "scope": "session" })),
                DENY => Ok(json!({ "permissions": {}, "scope": "turn" })),
                other => Err(unusable(&format!("the request has no option {other:?}"))),
            }
        }
        (PendingKind::UserInput(ids), ApprovalDecision::Answers { answers, .. }) => {
            let mut given = serde_json::Map::new();
            for id in ids {
                let chosen = answers.get(id).cloned().unwrap_or_default();
                given.insert(id.clone(), json!({ "answers": chosen }));
            }
            Ok(json!({ "answers": given }))
        }
        (PendingKind::Elicitation, ApprovalDecision::Elicitation { action, content }) => {
            let action = match action {
                ElicitationAction::Accept => "accept",
                ElicitationAction::Decline => "decline",
                ElicitationAction::Cancel => "cancel",
            };
            Ok(json!({
                "action": action,
                "content": if action == "accept" { content.clone().unwrap_or(Json::Null) } else { Json::Null },
                "_meta": Json::Null,
            }))
        }
        _ => Err(unusable(
            "that kind of answer does not fit that kind of request",
        )),
    }
}

/// Allows a residual tool permission request for this turn; questions and forms still need a person.
#[must_use]
pub fn bypass_answer(kind: &PendingKind) -> Option<Json> {
    match kind {
        PendingKind::Command | PendingKind::FileChange => Some(json!({ "decision": "accept" })),
        PendingKind::Permissions(requested) => {
            Some(json!({ "permissions": requested, "scope": "turn" }))
        }
        PendingKind::UserInput(_) | PendingKind::Elicitation => None,
    }
}

/// The answer that says no to a request of the given kind, for one nobody is left to answer.
#[must_use]
pub fn refusal(kind: &PendingKind) -> Json {
    match kind {
        PendingKind::Command | PendingKind::FileChange => json!({ "decision": "cancel" }),
        PendingKind::Permissions(_) => json!({ "permissions": {}, "scope": "turn" }),
        PendingKind::UserInput(ids) => {
            let empty: serde_json::Map<String, Json> = ids
                .iter()
                .map(|id| (id.clone(), json!({ "answers": [] })))
                .collect();
            json!({ "answers": empty })
        }
        PendingKind::Elicitation => json!({ "action": "cancel", "content": null, "_meta": null }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command_request() -> Json {
        json!({
            "kind": "command", "threadId": "thr_1", "turnId": "turn_1", "itemId": "call_1",
            "startedAtMs": 1, "environmentId": "local",
            "command": "/bin/bash -lc 'npm test && echo done'", "cwd": "/workspace/web",
            "commandActions": [{ "type": "unknown", "command": "npm test && echo done" }],
            "proposedExecpolicyAmendment": ["/bin/bash", "-lc", "npm test"],
            "availableDecisions": ["accept", { "acceptWithExecpolicyAmendment": { "execpolicy_amendment": ["npm", "test"] } }, "cancel"]
        })
    }

    fn translated(method: &str, params: &Json) -> Translated {
        translate(method, params, Vec::new()).unwrap()
    }

    #[test]
    fn a_command_is_shown_as_the_agent_wrote_it_with_the_options_the_harness_lists_and_a_deny() {
        let approval = translated("item/commandExecution/requestApproval", &command_request());
        assert_eq!(approval.title, "Run npm test && echo done");
        let HarnessRequestDetail::Command {
            command,
            cwd,
            reason,
        } = &approval.request.detail
        else {
            panic!("a command");
        };
        assert_eq!(
            (command.as_str(), cwd.as_str(), reason),
            ("npm test && echo done", "/workspace/web", &None)
        );
        let ids: Vec<_> = approval
            .request
            .options
            .iter()
            .map(|option| option.id.as_str())
            .collect();
        assert_eq!(
            ids,
            [ONCE, ALWAYS, DENY, ABORT],
            "the allowing ones first, then the refusing ones"
        );
        assert_eq!(approval.request.first_allowing().unwrap().id, ONCE);
        let deny = &approval.request.options[2];
        assert_eq!((deny.style, deny.scope), (OptionStyle::Deny, None));
        assert_eq!(approval.pending.kind, PendingKind::Command);
    }

    #[test]
    fn a_decision_names_an_option_and_becomes_the_harnesss_own_word_for_it() {
        let approval = translated("item/commandExecution/requestApproval", &command_request());
        let pick = |id: &str| {
            answer(
                &approval.pending,
                &ApprovalDecision::Option {
                    option_id: id.into(),
                    note: None,
                },
            )
            .unwrap()
        };
        assert_eq!(pick(ONCE), json!({ "decision": "accept" }));
        assert_eq!(pick(DENY), json!({ "decision": "decline" }));
        assert_eq!(pick(ABORT), json!({ "decision": "cancel" }));
        assert_eq!(
            pick(ALWAYS),
            json!({ "decision": { "acceptWithExecpolicyAmendment": { "execpolicy_amendment": ["npm", "test"] } } })
        );
        assert!(
            answer(
                &approval.pending,
                &ApprovalDecision::Option {
                    option_id: "nonsense".into(),
                    note: None
                }
            )
            .is_err(),
            "an option the request does not have is refused"
        );
    }

    #[test]
    fn without_a_list_of_decisions_the_four_the_harness_always_has_are_offered() {
        let mut params = command_request();
        params.as_object_mut().unwrap().remove("availableDecisions");
        let approval = translated("item/commandExecution/requestApproval", &params);
        let ids: Vec<_> = approval
            .request
            .options
            .iter()
            .map(|option| option.id.as_str())
            .collect();
        assert_eq!(ids, [ONCE, SESSION, DENY, ABORT]);
    }

    #[test]
    fn a_change_of_files_is_shown_with_the_changes_of_the_item_it_is_about() {
        let change = FileChange {
            path: "src/main.rs".into(),
            change: orch_agent_contract::events::FileChangeKind::Modify,
            old_path: None,
            diff: Some("@@ -1 +1 @@\n-a\n+b\n".into()),
            added: 1,
            removed: 1,
            binary: false,
            truncated: false,
        };
        let approval = translate(
            "item/fileChange/requestApproval",
            &json!({ "threadId": "thr_1", "turnId": "t", "itemId": "call_2", "reason": "outside the workspace", "grantRoot": null }),
            vec![change],
        )
        .unwrap();
        assert_eq!(approval.title, "Change src/main.rs");
        assert!(matches!(
            &approval.request.detail,
            HarnessRequestDetail::FileChange { changes, reason } if changes.len() == 1 && reason.as_deref() == Some("outside the workspace")
        ));
        let ids: Vec<_> = approval
            .request
            .options
            .iter()
            .map(|option| option.id.as_str())
            .collect();
        assert_eq!(ids, [ONCE, SESSION, DENY, ABORT]);
    }

    #[test]
    fn more_permissions_can_be_granted_for_a_turn_for_a_session_or_refused() {
        let asked = json!({ "network": { "enabled": true }, "fileSystem": null });
        let approval = translated(
            "item/permissions/requestApproval",
            &json!({ "threadId": "thr_1", "turnId": "t", "itemId": "i", "cwd": "/w", "reason": "to fetch", "permissions": asked }),
        );
        let pick = |id: &str| {
            answer(
                &approval.pending,
                &ApprovalDecision::Option {
                    option_id: id.into(),
                    note: None,
                },
            )
            .unwrap()
        };
        assert_eq!(pick(ONCE), json!({ "permissions": asked, "scope": "turn" }));
        assert_eq!(
            pick(SESSION),
            json!({ "permissions": asked, "scope": "session" })
        );
        assert_eq!(pick(DENY), json!({ "permissions": {}, "scope": "turn" }));
    }

    #[test]
    fn questions_are_answered_by_their_ids() {
        let approval = translated(
            "item/tool/requestUserInput",
            &json!({ "threadId": "thr_1", "turnId": "t", "itemId": "i", "isBlocking": true, "autoResolutionMs": null,
                     "questions": [ { "id": "db", "header": "Database", "question": "Which database?", "isOther": true,
                                      "isSecret": false, "options": [ { "label": "Postgres", "description": "The default" } ] },
                                    { "id": "name", "header": "Name", "question": "What name?", "isOther": false,
                                      "isSecret": false, "options": null } ] }),
        );
        assert_eq!(approval.title, "Which database?");
        let HarnessRequestDetail::UserInput { questions, .. } = &approval.request.detail else {
            panic!("questions");
        };
        assert!(questions[0].allow_other && questions[0].options.as_ref().unwrap().len() == 1);
        assert!(questions[1].options.is_none(), "a free answer");
        assert!(approval.request.options.is_empty());
        let given = answer(
            &approval.pending,
            &ApprovalDecision::Answers {
                answers: BTreeMap::from([("db".to_owned(), vec!["Postgres".to_owned()])]),
                note: None,
            },
        )
        .unwrap();
        assert_eq!(
            given,
            json!({ "answers": { "db": { "answers": ["Postgres"] }, "name": { "answers": [] } } })
        );
    }

    #[test]
    fn an_mcp_servers_request_for_input_is_accepted_declined_or_cancelled() {
        let approval = translated(
            "mcpServer/elicitation/request",
            &json!({ "threadId": "thr_1", "turnId": null, "serverName": "github", "mode": "form",
                     "message": "Which repository?", "requestedSchema": { "type": "object" }, "_meta": null }),
        );
        let HarnessRequestDetail::McpElicitation {
            server,
            message,
            schema,
            ..
        } = &approval.request.detail
        else {
            panic!("an elicitation");
        };
        assert_eq!(
            (server.as_str(), message.as_str()),
            ("github", "Which repository?")
        );
        assert_eq!(schema["requestedSchema"], json!({ "type": "object" }));
        let accepted = answer(
            &approval.pending,
            &ApprovalDecision::Elicitation {
                action: ElicitationAction::Accept,
                content: Some(json!({ "repo": "x" })),
            },
        )
        .unwrap();
        assert_eq!(
            accepted,
            json!({ "action": "accept", "content": { "repo": "x" }, "_meta": null })
        );
        let declined = answer(
            &approval.pending,
            &ApprovalDecision::Elicitation {
                action: ElicitationAction::Decline,
                content: Some(json!({ "repo": "x" })),
            },
        )
        .unwrap();
        assert_eq!(
            declined["content"],
            Json::Null,
            "nothing is given with a refusal"
        );
    }

    #[test]
    fn a_kind_of_answer_that_does_not_fit_the_request_is_refused_and_an_unknown_request_is_not_an_approval()
     {
        let approval = translated("item/commandExecution/requestApproval", &command_request());
        assert!(
            answer(
                &approval.pending,
                &ApprovalDecision::Elicitation {
                    action: ElicitationAction::Accept,
                    content: None
                }
            )
            .is_err()
        );
        assert!(translate("item/tool/call", &json!({}), Vec::new()).is_none());
    }

    #[test]
    fn a_request_nobody_is_left_to_answer_is_refused_in_its_own_shape() {
        assert_eq!(
            refusal(&PendingKind::Command),
            json!({ "decision": "cancel" })
        );
        assert_eq!(
            refusal(&PendingKind::Permissions(Json::Null)),
            json!({ "permissions": {}, "scope": "turn" })
        );
        assert_eq!(
            refusal(&PendingKind::UserInput(vec!["q".into()])),
            json!({ "answers": { "q": { "answers": [] } } })
        );
        assert_eq!(refusal(&PendingKind::Elicitation)["action"], "cancel");
    }
}
