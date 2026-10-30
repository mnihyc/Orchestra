//! The directives `manage` and `confirm`: the two calls of the tool that deletes, moves and brings back documents, the second made with the word that the first was given.
//!
//! `manage <command> <path> [| <path>…] [=> <to>]` calls `manage_shared`: `command` is `create_folder`, `copy`,
//! `move`, `delete` or `restore`, the paths are separated by `|`, and `=> <to>` names an existing destination.
//! Only delete previews and supplies a token; other commands execute directly. `confirm` repeats deletion with the
//! word that the last answer gave, which the fake remembers for the turn. A `confirm` with no `manage` before it, or
//! after an answer that gave no word, says so and calls nothing; one that follows an answer which was refused as
//! `not_confirmed` takes the new word from the plan that answer holds, so that a script can read the list again and
//! confirm it. The word is kept after it was used, so that a second `confirm` uses it a second time, which is a
//! way to see that a word is good once.

use std::time::Duration;

use orch_agent_contract::events::{ItemStatus, OrchTool, ToolPayload};
use orch_types::Json;
use serde_json::json;
use tokio::sync::oneshot;

use super::{Flow, ORCH_TOOL_TIMEOUT_SECS, Runner, ScriptError, fault, split_unescaped, text_of};
use crate::events::lock;
use crate::sessions::Counter;

/// How many characters a word has: 128 bits in hexadecimal.
const WORD_CHARS: usize = 32;

/// What follows the word in the text of an answer that was refused, in the plan it holds.
const WORD_MARK: &str = "\"confirm\":\"";

/// One of the two directives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// `manage <command> <path> [| <path>…] [=> <to>]`: the first call.
    Manage {
        /// The document-tree command, as the script wrote it; only delete supplies a preview token.
        command: String,
        /// The paths.
        paths: Vec<String>,
        /// Where a copy or move goes.
        to: Option<String>,
    },
    /// `confirm`: the second call.
    Confirm,
}

/// Reads the arguments of `manage`, or `confirm`, which takes none.
pub fn parse(line: usize, keyword: &str, rest: &str) -> Result<Step, ScriptError> {
    if keyword == "confirm" {
        return if rest.is_empty() {
            Ok(Step::Confirm)
        } else {
            Err(fault(line, "confirm takes nothing more"))
        };
    }
    const USAGE: &str = "manage is: manage <command> <path> [| <path>…] [=> <to>]";
    let (head, to) = match rest.rsplit_once("=>") {
        Some((head, to)) => (head.trim(), Some(text_of(to)).filter(|to| !to.is_empty())),
        None => (rest.trim(), None),
    };
    let (command, paths) = head
        .split_once(char::is_whitespace)
        .map_or((head, ""), |(command, paths)| (command, paths.trim()));
    let paths: Vec<String> = split_unescaped(paths, '|')
        .into_iter()
        .map(text_of)
        .filter(|path| !path.is_empty())
        .collect();
    if command.is_empty() || paths.is_empty() {
        return Err(fault(line, USAGE));
    }
    Ok(Step::Manage {
        command: command.to_owned(),
        paths,
        to,
    })
}

/// The last call of `manage_shared` that a script made: what it asked, and the word to confirm it with.
#[derive(Debug, Clone, Default)]
pub struct Memory {
    last: Option<Asked>,
}

#[derive(Debug, Clone)]
struct Asked {
    arguments: Json,
    word: Option<String>,
}

/// What an answer to a call of the tool came to.
enum Reply {
    Result(Json),
    Refused(String),
    Unreachable,
}

impl Reply {
    /// The word that the answer gives: in the result of a first call, in the plan that a refusal as `not_confirmed`
    /// holds.
    fn word(&self) -> Option<String> {
        match self {
            Self::Result(value) => value
                .get("confirm")
                .and_then(Json::as_str)
                .map(str::to_owned),
            Self::Refused(message) => {
                let start = message.find(WORD_MARK)? + WORD_MARK.len();
                let word: String = message[start..]
                    .chars()
                    .take_while(char::is_ascii_hexdigit)
                    .collect();
                (word.len() == WORD_CHARS).then_some(word)
            }
            Self::Unreachable => None,
        }
    }
}

impl Runner {
    /// Carries out one of the two directives.
    pub(super) async fn manage_step(
        &mut self,
        step: &Step,
        stream: &str,
        parent: Option<&str>,
    ) -> Result<(), Flow> {
        match step {
            Step::Manage { command, paths, to } => {
                let mut arguments = json!({ "command": command, "paths": paths });
                if let Some(to) = to {
                    arguments["to"] = json!(to);
                }
                let reply = self.manage_call(&arguments, stream, parent).await?;
                self.manage.last = Some(Asked {
                    word: reply.word(),
                    arguments,
                });
                Ok(())
            }
            Step::Confirm => {
                let Some(asked) = self.manage.last.clone() else {
                    self.emitter
                        .say_whole("confirm: there was no manage before it.", None)
                        .await;
                    return Ok(());
                };
                let Some(word) = asked.word.clone() else {
                    self.emitter.say_whole(
                        "confirm: the last answer of manage_shared gave no word to confirm with.",
                        None,
                    ).await;
                    return Ok(());
                };
                let mut arguments = asked.arguments.clone();
                arguments["confirm"] = json!(word);
                let reply = self.manage_call(&arguments, stream, parent).await?;
                if let Some(new) = reply.word() {
                    self.manage.last = Some(Asked {
                        arguments: asked.arguments,
                        word: Some(new),
                    });
                }
                Ok(())
            }
        }
    }

    /// A call of `manage_shared`, made as the call of any of Orch's tools is, with the answer given back.
    async fn manage_call(
        &mut self,
        arguments: &Json,
        stream: &str,
        parent: Option<&str>,
    ) -> Result<Reply, Flow> {
        let tool = OrchTool::ManageShared;
        let native_ref = self.emitter.next_item_ref();
        let call_ref = format!("call_{}", self.live.next_counter(Counter::Call));
        let begun = |result: Option<Json>| ToolPayload::Orch {
            tool,
            arguments: arguments.clone(),
            result,
        };
        let raw_name = format!("mcp__orch__{}", tool.name());
        self.tool_item(
            &native_ref,
            stream,
            parent,
            begun(None),
            super::title_of(tool),
            &raw_name,
            arguments,
        )
        .await;
        let (reply, result) = oneshot::channel();
        lock(&self.live.tool_calls).insert(call_ref.clone(), reply);
        self.emitter
            .orch_tool_call(&call_ref, tool.name(), arguments.clone())
            .await;
        let waited = self
            .wait(tokio::time::timeout(
                Duration::from_secs(ORCH_TOOL_TIMEOUT_SECS),
                result,
            ))
            .await;
        lock(&self.live.tool_calls).remove(&call_ref);
        match waited {
            Err(flow) => {
                self.end_tool(
                    &native_ref,
                    ItemStatus::Cancelled,
                    begun(None),
                    None,
                    false,
                    None,
                )
                .await;
                Err(flow)
            }
            Ok(Ok(Ok(Ok(value)))) => {
                let text = value.to_string();
                self.end_tool(
                    &native_ref,
                    ItemStatus::Completed,
                    begun(Some(value.clone())),
                    Some(&text),
                    false,
                    None,
                )
                .await;
                Ok(Reply::Result(value))
            }
            Ok(Ok(Ok(Err(refusal)))) => {
                let line = format!("{}: {}", refusal.code, refusal.message);
                self.end_tool(
                    &native_ref,
                    ItemStatus::Failed,
                    begun(Some(json!({
                        "error": { "code": refusal.code, "message": refusal.message }
                    }))),
                    Some(&line),
                    true,
                    None,
                )
                .await;
                Ok(Reply::Refused(refusal.message))
            }
            Ok(Ok(Err(_))) | Ok(Err(_)) => {
                // Nobody answered within the time, or the answer was dropped.
                let message = "Orch is unreachable; try again later";
                self.end_tool(
                    &native_ref,
                    ItemStatus::Failed,
                    begun(Some(json!({ "error": { "message": message } }))),
                    Some(message),
                    true,
                    None,
                )
                .await;
                Ok(Reply::Unreachable)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manage_reads_a_command_its_paths_and_where_a_move_goes() {
        assert_eq!(
            parse(2, "manage", "delete docs/Plan.md").unwrap(),
            Step::Manage {
                command: "delete".into(),
                paths: vec!["docs/Plan.md".into()],
                to: None
            }
        );
        assert_eq!(
            parse(
                2,
                "manage",
                "move docs/Plan.md | docs/Rollout plan.md => ../Other/docs"
            )
            .unwrap(),
            Step::Manage {
                command: "move".into(),
                paths: vec!["docs/Plan.md".into(), "docs/Rollout plan.md".into()],
                to: Some("../Other/docs".into())
            }
        );
        assert_eq!(parse(3, "confirm", "").unwrap(), Step::Confirm);
    }

    #[test]
    fn a_manage_without_a_path_and_a_confirm_with_something_after_it_are_mistakes_of_the_script() {
        for rest in ["", "delete", "delete   ", "delete | "] {
            let error = parse(5, "manage", rest).unwrap_err();
            assert_eq!(error.line, 5, "{rest:?}");
            assert!(error.message.starts_with("manage is:"), "{}", error.message);
        }
        assert_eq!(
            parse(6, "confirm", "yes").unwrap_err().message,
            "confirm takes nothing more"
        );
    }

    #[test]
    fn the_word_is_read_from_a_plan_and_from_the_plan_a_refusal_holds() {
        let word = "0123456789abcdef0123456789abcdef";
        let plan = Reply::Result(json!({ "done": false, "confirm": word, "affects": [] }));
        assert_eq!(plan.word().as_deref(), Some(word));
        let refused = Reply::Refused(format!(
            "Nothing was done. The plan as it stands now: {{\"done\":false,\"confirm\":\"{word}\",\"expires_in\":600}}."
        ));
        assert_eq!(refused.word().as_deref(), Some(word));
        let done = Reply::Result(json!({ "done": true, "results": [] }));
        assert_eq!(done.word(), None, "an act that was done gives no word");
        let short = Reply::Refused("\"confirm\":\"abc\"".into());
        assert_eq!(
            short.word(),
            None,
            "a word has all its characters or it is none"
        );
        assert_eq!(Reply::Unreachable.word(), None);
    }
}
