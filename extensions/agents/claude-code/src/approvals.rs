//! The permission tool, approval modes and the shell sandbox mapping.
//!
//! When the harness would act on something its mode does not already allow, it asks the program that
//! drives it on its own standard streams: a request that names the tool and its input, answered with an
//! allowance or a refusal. This module turns such a request into an approval of the neutral kind (a command,
//! a change of files, a question, or more permissions), and a person's decision back into the answer the
//! harness reads. It also holds the two halves of what confines an agent: the harness's permission mode that
//! follows Orch's approval mode, and the settings of the harness's shell sandbox that follow Orch's sandbox
//! mode.

use std::collections::BTreeMap;

use orch_agent_contract::adapter::AdapterError;
use orch_agent_contract::descriptor::{ApprovalMode, SandboxMode};
use orch_agent_contract::events::{
    ApprovalOption, ApprovalRequested, HarnessRequest, HarnessRequestDetail, HarnessRequestKind,
    OptionScope, OptionStyle, Question, QuestionOption,
};
use orch_agent_contract::session::{ApprovalDecision, TurnId};
use orch_types::Json;
use serde_json::json;

use crate::stream::Frame;
use crate::tools::{Family, ToolContext, call_title, family_of, tool_payload};

/// The option that lets the one thing happen.
pub const OPTION_ALLOW_ONCE: &str = "allow_once";
/// The option that refuses it.
pub const OPTION_DENY: &str = "deny";

/// What the harness is told when a person refuses and says nothing more.
pub const DEFAULT_REFUSAL: &str = "The person did not allow this.";

/// The option that proceeds with a proposed plan.
pub const OPTION_PROCEED: &str = "proceed";
/// The option that keeps planning, with the person's words.
pub const OPTION_KEEP_PLANNING: &str = "keep_planning";
/// What the harness is told when a person keeps planning and says nothing more.
pub const DEFAULT_KEEP_PLANNING: &str =
    "The person is not ready to proceed. Keep planning, and propose the plan again.";

/// The tool with which the harness proposes its plan and asks to leave plan mode.
pub const EXIT_PLAN_TOOL: &str = "ExitPlanMode";
/// The harness's own permission mode in which it reads and plans and changes nothing.
pub const PLAN_MODE: &str = "plan";

/// The launch variable that lets bypass mode skip the native dangerous-delete prompt.
pub const BYPASS_ENV: &str = "CLAUDE_CODE_EVEN_MORE_DANGEROUSLY_SKIP_PERMISSIONS";

/// The permission mode of the harness for an approval mode of Orch. The harness is always told one: left
/// to itself it chooses a starting mode, and in a non-interactive run that may be a mode that decides by a
/// classifier.
#[must_use]
pub const fn permission_mode(mode: ApprovalMode) -> &'static str {
    match mode {
        ApprovalMode::Ask => "default",
        ApprovalMode::OnRequest => "acceptEdits",
        ApprovalMode::Auto => "auto",
        ApprovalMode::Bypass => "bypassPermissions",
    }
}

/// The settings of the harness's shell sandbox for a sandbox mode of Orch, to be merged into the session's
/// settings. `workspace` turns the sandbox on and makes it a hard requirement, so that a sandbox that cannot
/// start fails the session and never runs a command unconfined; with the approval mode `on_request`, commands
/// that run inside the sandbox run without asking. `open` asks for nothing: the environment is the boundary.
#[must_use]
pub fn sandbox_settings(approval: ApprovalMode, sandbox: SandboxMode) -> Option<Json> {
    match sandbox {
        SandboxMode::Open | SandboxMode::ReadOnly => None,
        SandboxMode::Workspace => Some(json!({
            "sandbox": {
                "enabled": true,
                "failIfUnavailable": true,
                "allowUnsandboxedCommands": false,
                "autoAllowBashIfSandboxed": matches!(approval, ApprovalMode::OnRequest),
            }
        })),
    }
}

/// A request of the harness for permission.
#[derive(Debug, Clone, PartialEq)]
pub struct PermissionAsk {
    /// The harness's id for the request, which the answer names.
    pub control_id: String,
    /// The tool that wants to act.
    pub tool_name: String,
    /// What it wants to do.
    pub input: Json,
    /// The tool use the request is about.
    pub tool_use_id: Option<String>,
    /// The sub-agent that asks, when one does.
    pub agent_id: Option<String>,
    /// The harness's sentence on it.
    pub description: Option<String>,
}

impl PermissionAsk {
    /// Reads a control request of the kind `can_use_tool`; `None` for any other request.
    #[must_use]
    pub fn from_frame(frame: &Frame) -> Option<Self> {
        let request = frame.get("request")?;
        if request.get("subtype")?.as_str()? != "can_use_tool" {
            return None;
        }
        Some(Self {
            control_id: frame.text("request_id")?.to_owned(),
            tool_name: request.get("tool_name")?.as_str()?.to_owned(),
            input: request.get("input").cloned().unwrap_or_else(|| json!({})),
            tool_use_id: request
                .get("tool_use_id")
                .and_then(Json::as_str)
                .map(str::to_owned),
            agent_id: request
                .get("agent_id")
                .and_then(Json::as_str)
                .map(str::to_owned),
            description: request
                .get("description")
                .and_then(Json::as_str)
                .map(str::to_owned),
        })
    }

    /// What kind of approval this is.
    #[must_use]
    pub fn kind(&self) -> HarnessRequestKind {
        if self.tool_name == EXIT_PLAN_TOOL {
            return HarnessRequestKind::Plan;
        }
        match family_of(&self.tool_name) {
            Family::Command => HarnessRequestKind::Command,
            Family::Edit | Family::Write | Family::NotebookEdit => HarnessRequestKind::FileChange,
            Family::Question => HarnessRequestKind::UserInput,
            _ => HarnessRequestKind::Permissions,
        }
    }

    /// Whether this native tool only reads or searches while a plan is being prepared.
    #[must_use]
    pub fn read_only(&self) -> bool {
        matches!(
            family_of(&self.tool_name),
            Family::Read | Family::Glob | Family::Grep | Family::WebSearch | Family::WebFetch
        )
    }

    /// The approval a person is asked to decide.
    #[must_use]
    pub fn approval(&self, turn_id: TurnId, ctx: &ToolContext<'_>) -> ApprovalRequested {
        let family = family_of(&self.tool_name);
        let kind = self.kind();
        let reason = self.description.clone();
        let (options, detail) = match kind {
            HarnessRequestKind::Command => (
                allow_or_deny(),
                HarnessRequestDetail::Command {
                    command: self
                        .input
                        .get("command")
                        .and_then(Json::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    cwd: ctx.cwd.to_owned(),
                    reason,
                },
            ),
            HarnessRequestKind::FileChange => {
                let changes =
                    match tool_payload(&family, &self.tool_name, "", &self.input, None, ctx) {
                        orch_agent_contract::events::ToolPayload::FileChange { changes } => changes,
                        _ => Vec::new(),
                    };
                (
                    allow_or_deny(),
                    HarnessRequestDetail::FileChange { changes, reason },
                )
            }
            HarnessRequestKind::UserInput => (
                Vec::new(),
                HarnessRequestDetail::UserInput {
                    native_item_ref: self.tool_use_id.clone(),
                    questions: questions_of(&self.input),
                },
            ),
            HarnessRequestKind::Plan => (
                proceed_or_keep_planning(),
                HarnessRequestDetail::Plan {
                    plan: self
                        .input
                        .get("plan")
                        .and_then(Json::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                },
            ),
            _ => (
                allow_or_deny(),
                HarnessRequestDetail::Permissions {
                    permissions: json!({ "tool": self.tool_name, "input": self.input }),
                    reason,
                },
            ),
        };
        ApprovalRequested {
            turn_id,
            request_ref: self.control_id.clone(),
            title: approval_title(self, &family, ctx),
            request: HarnessRequest {
                request_kind: kind,
                options,
                detail,
            },
        }
    }

    /// The answer to give the harness for a person's decision.
    pub fn answer(&self, decision: &ApprovalDecision) -> Result<Json, AdapterError> {
        match decision {
            ApprovalDecision::Option { option_id, note } => match option_id.as_str() {
                OPTION_ALLOW_ONCE | OPTION_PROCEED => Ok(self.allow()),
                OPTION_KEEP_PLANNING => Ok(json!({
                    "behavior": "deny",
                    "message": note.clone().filter(|note| !note.is_empty()).unwrap_or_else(|| DEFAULT_KEEP_PLANNING.to_owned()),
                })),
                OPTION_DENY => Ok(json!({
                    "behavior": "deny",
                    "message": note.clone().filter(|note| !note.is_empty()).unwrap_or_else(|| DEFAULT_REFUSAL.to_owned()),
                })),
                other => Err(AdapterError::NotFound(format!(
                    "the request has no option {other:?}"
                ))),
            },
            ApprovalDecision::Answers { answers, .. } => {
                if self.kind() != HarnessRequestKind::UserInput {
                    return Err(AdapterError::Unsupported(
                        "answers are given to a question only".to_owned(),
                    ));
                }
                Ok(
                    json!({ "behavior": "allow", "updatedInput": answered_input(&self.input, answers) }),
                )
            }
            ApprovalDecision::Elicitation { .. } => {
                Err(AdapterError::Unsupported("mcp_elicitation".to_owned()))
            }
        }
    }

    /// Allows this exact tool input without changing it.
    #[must_use]
    pub fn allow(&self) -> Json {
        json!({ "behavior": "allow", "updatedInput": self.input })
    }

    /// The answer when nobody can decide: the process is going away, or the mode says never to ask.
    #[must_use]
    pub fn refusal(&self, why: &str) -> Json {
        json!({ "behavior": "deny", "message": why })
    }
}

/// Proceeding leaves plan mode and the turn goes on with the work; keeping planning tells the agent what to change.
fn proceed_or_keep_planning() -> Vec<ApprovalOption> {
    vec![
        ApprovalOption {
            id: OPTION_PROCEED.to_owned(),
            label: "Proceed".to_owned(),
            style: OptionStyle::Allow,
            scope: Some(OptionScope::Once),
            rule: None,
        },
        ApprovalOption {
            id: OPTION_KEEP_PLANNING.to_owned(),
            label: "Keep planning".to_owned(),
            style: OptionStyle::Deny,
            scope: None,
            rule: None,
        },
    ]
}

fn allow_or_deny() -> Vec<ApprovalOption> {
    vec![
        ApprovalOption {
            id: OPTION_ALLOW_ONCE.to_owned(),
            label: "Allow once".to_owned(),
            style: OptionStyle::Allow,
            scope: Some(OptionScope::Once),
            rule: None,
        },
        ApprovalOption {
            id: OPTION_DENY.to_owned(),
            label: "Deny".to_owned(),
            style: OptionStyle::Deny,
            scope: None,
            rule: None,
        },
    ]
}

fn approval_title(ask: &PermissionAsk, family: &Family, ctx: &ToolContext<'_>) -> String {
    let title = call_title(family, &ask.tool_name, &ask.input, ctx);
    match family {
        Family::Command => format!(
            "Run {}",
            ask.input
                .get("command")
                .and_then(Json::as_str)
                .map_or(title, |command| {
                    command
                        .lines()
                        .next()
                        .unwrap_or_default()
                        .chars()
                        .take(120)
                        .collect()
                })
        ),
        Family::Question => "The agent asks a question".to_owned(),
        _ if ask.tool_name == EXIT_PLAN_TOOL => "The agent proposes a plan".to_owned(),
        Family::Other | Family::Mcp { .. } | Family::Orch(_) | Family::Plan(_) => {
            format!("Use {}", ask.tool_name)
        }
        _ => title,
    }
}

/// The questions of the harness's question tool, in the neutral form. The tool always lets a person answer
/// in words of their own.
fn questions_of(input: &Json) -> Vec<Question> {
    input
        .get("questions")
        .and_then(Json::as_array)
        .map(|questions| {
            questions
                .iter()
                .enumerate()
                .map(|(position, question)| Question {
                    id: format!("q{}", position + 1),
                    header: question
                        .get("header")
                        .and_then(Json::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    question: question
                        .get("question")
                        .and_then(Json::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    multiple: question
                        .get("multiSelect")
                        .and_then(Json::as_bool)
                        .unwrap_or(false),
                    allow_other: true,
                    options: question
                        .get("options")
                        .and_then(Json::as_array)
                        .map(|options| {
                            options
                                .iter()
                                .map(|option| QuestionOption {
                                    label: option
                                        .get("label")
                                        .and_then(Json::as_str)
                                        .unwrap_or_default()
                                        .to_owned(),
                                    description: option
                                        .get("description")
                                        .and_then(Json::as_str)
                                        .unwrap_or_default()
                                        .to_owned(),
                                })
                                .collect()
                        }),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The input of the question tool with the answers added, each under the text of its question and several
/// choices joined by commas, as the tool reads them.
fn answered_input(input: &Json, answers: &BTreeMap<String, Vec<String>>) -> Json {
    let mut by_question = serde_json::Map::new();
    for (position, question) in input
        .get("questions")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        let id = format!("q{}", position + 1);
        let text = question
            .get("question")
            .and_then(Json::as_str)
            .unwrap_or_default();
        if let Some(chosen) = answers.get(&id) {
            by_question.insert(text.to_owned(), Json::String(chosen.join(", ")));
        }
    }
    let mut updated = input.clone();
    if let Some(object) = updated.as_object_mut() {
        object.insert("answers".to_owned(), Json::Object(by_question));
    }
    updated
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_agent_contract::events::{FileChangeKind, HarnessRequestKind};
    use orch_types::new_id;

    fn ctx() -> ToolContext<'static> {
        ToolContext {
            directory: "/workspace/web",
            cwd: "/workspace/web",
        }
    }

    fn ask(tool: &str, input: Json) -> PermissionAsk {
        PermissionAsk::from_frame(&Frame::from_value(json!({
            "type": "control_request", "request_id": "req-1",
            "request": { "subtype": "can_use_tool", "tool_name": tool, "input": input,
                         "tool_use_id": "toolu_1", "description": "Say hello" }
        })))
        .unwrap()
    }

    #[test]
    fn a_proposed_plan_is_a_plan_to_proceed_with_or_to_keep_planning_in_the_persons_words() {
        let plan = ask(
            EXIT_PLAN_TOOL,
            json!({ "plan": "1. Read the handler\n2. Fix the redirect" }),
        );
        assert_eq!(plan.kind(), HarnessRequestKind::Plan);
        let approval = plan.approval(new_id(), &ctx());
        assert_eq!(approval.title, "The agent proposes a plan");
        assert_eq!(
            approval.request.detail,
            HarnessRequestDetail::Plan {
                plan: "1. Read the handler\n2. Fix the redirect".into()
            }
        );
        assert_eq!(
            approval
                .request
                .options
                .iter()
                .map(|option| option.id.as_str())
                .collect::<Vec<_>>(),
            [OPTION_PROCEED, OPTION_KEEP_PLANNING]
        );
        let proceed = plan
            .answer(&ApprovalDecision::Option {
                option_id: OPTION_PROCEED.into(),
                note: None,
            })
            .unwrap();
        assert_eq!(proceed["behavior"], "allow");
        let keep = plan
            .answer(&ApprovalDecision::Option {
                option_id: OPTION_KEEP_PLANNING.into(),
                note: Some("Split the second step".into()),
            })
            .unwrap();
        assert_eq!(
            keep,
            json!({ "behavior": "deny", "message": "Split the second step" })
        );
        let bare = plan
            .answer(&ApprovalDecision::Option {
                option_id: OPTION_KEEP_PLANNING.into(),
                note: None,
            })
            .unwrap();
        assert_eq!(bare["message"], DEFAULT_KEEP_PLANNING);
    }

    #[test]
    fn the_approval_mode_of_orch_is_always_a_permission_mode_of_the_harness() {
        assert_eq!(permission_mode(ApprovalMode::Ask), "default");
        assert_eq!(permission_mode(ApprovalMode::OnRequest), "acceptEdits");
        assert_eq!(permission_mode(ApprovalMode::Auto), "auto");
        assert_eq!(permission_mode(ApprovalMode::Bypass), "bypassPermissions");
    }

    #[test]
    fn the_sandbox_is_a_hard_requirement_in_workspace_mode_and_absent_in_open_mode() {
        let ask_each = sandbox_settings(ApprovalMode::Ask, SandboxMode::Workspace).unwrap();
        assert_eq!(ask_each["sandbox"]["enabled"], true);
        assert_eq!(ask_each["sandbox"]["failIfUnavailable"], true);
        assert_eq!(ask_each["sandbox"]["allowUnsandboxedCommands"], false);
        assert_eq!(ask_each["sandbox"]["autoAllowBashIfSandboxed"], false);
        let on_request = sandbox_settings(ApprovalMode::OnRequest, SandboxMode::Workspace).unwrap();
        assert_eq!(on_request["sandbox"]["autoAllowBashIfSandboxed"], true);
        assert!(sandbox_settings(ApprovalMode::Bypass, SandboxMode::Open).is_none());
    }

    #[test]
    fn a_request_for_a_command_is_an_approval_of_that_command_that_can_be_allowed_once_or_denied() {
        let ask = ask(
            "Bash",
            json!({ "command": "mkdir -p out && echo hi > out/x", "description": "Say hello" }),
        );
        let turn = new_id();
        let approval = ask.approval(turn, &ctx());
        assert_eq!(
            (approval.turn_id, approval.request_ref.as_str()),
            (turn, "req-1")
        );
        assert_eq!(approval.title, "Run mkdir -p out && echo hi > out/x");
        assert_eq!(approval.request.request_kind, HarnessRequestKind::Command);
        assert_eq!(
            approval.request.first_allowing().map(|o| o.id.as_str()),
            Some(OPTION_ALLOW_ONCE)
        );
        let HarnessRequestDetail::Command {
            command,
            cwd,
            reason,
        } = &approval.request.detail
        else {
            panic!("a command");
        };
        assert_eq!(
            (command.as_str(), cwd.as_str()),
            ("mkdir -p out && echo hi > out/x", "/workspace/web")
        );
        assert_eq!(reason.as_deref(), Some("Say hello"));
    }

    #[test]
    fn a_request_to_change_a_file_shows_the_change() {
        let ask = ask(
            "Edit",
            json!({ "file_path": "/workspace/web/a.txt", "old_string": "one", "new_string": "two" }),
        );
        let approval = ask.approval(new_id(), &ctx());
        assert_eq!(
            approval.request.request_kind,
            HarnessRequestKind::FileChange
        );
        let HarnessRequestDetail::FileChange { changes, .. } = &approval.request.detail else {
            panic!("a change");
        };
        assert_eq!(
            (changes[0].path.as_str(), changes[0].change),
            ("a.txt", FileChangeKind::Modify)
        );
        assert_eq!((changes[0].added, changes[0].removed), (1, 1));
    }

    #[test]
    fn a_request_for_anything_else_asks_for_the_permission_to_use_the_tool() {
        let ask = ask("mcp__github__create_issue", json!({ "title": "x" }));
        let approval = ask.approval(new_id(), &ctx());
        assert_eq!(
            approval.request.request_kind,
            HarnessRequestKind::Permissions
        );
        assert_eq!(approval.title, "Use mcp__github__create_issue");
    }

    #[test]
    fn allowing_gives_the_tool_its_input_and_denying_gives_the_agent_the_reason() {
        let ask = ask("Bash", json!({ "command": "ls" }));
        let allowed = ask
            .answer(&ApprovalDecision::Option {
                option_id: OPTION_ALLOW_ONCE.into(),
                note: None,
            })
            .unwrap();
        assert_eq!(
            allowed,
            json!({ "behavior": "allow", "updatedInput": { "command": "ls" } })
        );
        let denied = ask
            .answer(&ApprovalDecision::Option {
                option_id: OPTION_DENY.into(),
                note: Some("not now".into()),
            })
            .unwrap();
        assert_eq!(denied, json!({ "behavior": "deny", "message": "not now" }));
        let silent = ask
            .answer(&ApprovalDecision::Option {
                option_id: OPTION_DENY.into(),
                note: None,
            })
            .unwrap();
        assert_eq!(silent["message"], DEFAULT_REFUSAL);
        assert!(matches!(
            ask.answer(&ApprovalDecision::Option {
                option_id: "always".into(),
                note: None
            }),
            Err(AdapterError::NotFound(_))
        ));
    }

    #[test]
    fn a_question_is_put_to_the_person_and_the_answers_go_back_in_the_input_of_the_tool() {
        let input = json!({ "questions": [
            { "question": "Which database?", "header": "DB", "multiSelect": false,
              "options": [{ "label": "Postgres", "description": "default" }, { "label": "SQLite", "description": "small" }] },
            { "question": "Which features?", "header": "Features", "multiSelect": true, "options": [] }
        ] });
        let ask = ask("AskUserQuestion", input.clone());
        let approval = ask.approval(new_id(), &ctx());
        assert_eq!(approval.request.request_kind, HarnessRequestKind::UserInput);
        assert!(approval.request.options.is_empty());
        let HarnessRequestDetail::UserInput { questions, .. } = &approval.request.detail else {
            panic!("questions");
        };
        assert_eq!(questions[0].id, "q1");
        assert_eq!(questions[0].options.as_ref().unwrap()[1].label, "SQLite");
        assert!(questions[1].multiple && questions[1].allow_other);
        let answers = BTreeMap::from([
            ("q1".to_owned(), vec!["Postgres".to_owned()]),
            ("q2".to_owned(), vec!["a".to_owned(), "b".to_owned()]),
        ]);
        let answer = ask
            .answer(&ApprovalDecision::Answers {
                answers,
                note: None,
            })
            .unwrap();
        assert_eq!(answer["behavior"], "allow");
        assert_eq!(
            answer["updatedInput"]["answers"]["Which database?"],
            "Postgres"
        );
        assert_eq!(answer["updatedInput"]["answers"]["Which features?"], "a, b");
        assert_eq!(
            answer["updatedInput"]["questions"], input["questions"],
            "the questions stay"
        );
    }

    #[test]
    fn a_control_request_that_is_not_a_permission_is_not_an_ask() {
        let frame = Frame::from_value(json!({
            "type": "control_request", "request_id": "r", "request": { "subtype": "hook_callback" }
        }));
        assert!(PermissionAsk::from_frame(&frame).is_none());
    }
}
