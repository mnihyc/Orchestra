//! Every fake directive and interruptible execution, with one directive for each of Orch's tools and one for a step of each kind.
//!
//! An input whose first line is `#!fake` is a script: each following line is a directive, run in
//! order. [`parse`] reads it whole before anything runs, so that a mistake in line 7 stops the turn
//! before line 1 has done anything. `run_turn` carries it out: every wait (a pause between pieces
//! of text, a `sleep`, a question that waits for a person, a call of Orch's tools that waits for
//! its result, a credential that waits to be given) ends at once when the turn is interrupted, and
//! what was in the making is ended as it stands.
//!
//! The manual of the directives is the module documentation of the crate.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use orch_agent_contract::adapter::{AdapterEvent, EventSink};
use orch_agent_contract::credentials::CredentialNeed;
use orch_agent_contract::descriptor::MemoryKind;
use orch_agent_contract::events::{
    AgentMessagePayload, AgentMessagePhase, ApprovalOption, CommandAction, DeltaField,
    FILE_CHANGE_DIFF_MAX_BYTES, HarnessRequest, HarnessRequestDetail, HarnessRequestKind,
    ItemContent, ItemStatus, MAIN_STREAM, OptionScope, OptionStyle, OrchTool, OutputRef,
    PlanStepStatus, Question, ReasoningPayload, SubagentState, TokenUsage, ToolPayload, TurnError,
    TurnOutcome, TurnPhaseKind, sub_stream,
};
use orch_agent_contract::facts::HarnessState;
use orch_agent_contract::session::{ApprovalDecision, NoteKeeper, TurnStart};
use orch_types::{Json, Time};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::Shared;
use crate::credentials::WINDOW_MINUTES;
use crate::events::{
    Emitter, OUTPUT_PIECE_BYTES, PIECE_CHARS, PIECE_MILLIS, byte_pieces, char_count, file_change,
    lock, output_hash, pieces, tool_call, turn_error,
};
use crate::sessions::{Counter, LiveSession, PendingApproval, TurnRecord, append_note};

pub mod browser;
pub mod display;
pub mod manage;

/// What the first line of an input is when the input is a script.
pub const MARKER: &str = "#!fake";
/// How long a call of one of Orch's tools may wait for its result: the connector's rule for a tool it
/// relays, and so the fake's.
pub const ORCH_TOOL_TIMEOUT_SECS: u64 = 120;
/// The most pieces the output of a command is streamed in: a longer output is streamed in larger
/// pieces, so that no command takes more than five seconds to print.
pub const MAX_OUTPUT_PIECES: usize = 50;
/// How many pieces the answer to an input without a script is streamed in: three, one every hundred
/// milliseconds, so that it takes 300 ms.
pub const DEFAULT_PIECES: usize = 3;
/// The longest a limit may be made to last, in minutes: a year, which is longer than any window a plan has,
/// and short enough that the moment it ends is a time the clock can say.
pub const MAX_LIMIT_MINUTES: i64 = 366 * 24 * 60;
/// What the fake says when a script gives no words of the vendor for a provider that is busy.
pub const BUSY_SAID: &str = "fake provider: overloaded, try again in a moment";
/// What the fake says when a script gives no words of the vendor for a provider that cannot be reached.
pub const UNREACHABLE_SAID: &str = "fake provider: connection refused";
/// A `cut` lets one part in this many of its text through before the stream breaks: half, so that what was
/// said and what was lost are both something to see.
const CUT_KEEPS_ONE_IN: usize = 2;
/// The most lines a side of an `edit` may have: fifty thousand lines of a diff stay under any limit a
/// payload has but pass the cut of a diff, which a test of the cut needs.
pub const MAX_EDIT_LINES: u32 = 50_000;

// ---------------------------------------------------------------------------------------------------------
// The directives

/// A call of one of Orch's tools, as a directive makes it.
#[derive(Debug, Clone, PartialEq)]
pub struct OrchCall {
    /// Which tool.
    pub tool: OrchTool,
    /// What it is called with.
    pub arguments: Json,
}

/// One line of a script.
#[derive(Debug, Clone, PartialEq)]
pub enum Directive {
    /// `say <text>`
    Say(String),
    /// `think <text>`
    Think(String),
    /// `plan <a> | <b> | <c>`
    Plan(Vec<String>),
    /// `run <command> => <exit code> <output>`
    Run {
        /// The command line.
        command: String,
        /// How it ends.
        exit_code: i64,
        /// What it prints.
        output: String,
    },
    /// `background <command>` retains a simulated command until exact End or its own turn's Stop.
    Background(String),
    /// `edit <path> +<added> -<removed>`
    Edit {
        /// The file.
        path: String,
        /// Lines added.
        added: u32,
        /// Lines removed.
        removed: u32,
    },
    /// `mcp <server> <tool> <json>`
    Mcp {
        /// The server.
        server: String,
        /// The tool.
        tool: String,
        /// What it is called with.
        arguments: Json,
    },
    /// `ask command <command>`
    AskCommand(String),
    /// `ask input <question>`
    AskInput(String),
    /// `sub <name>: <directive>; <directive>…`
    Sub {
        /// The sub-agent.
        name: String,
        /// What it runs.
        body: Vec<Directive>,
    },
    /// One of the tools.
    Orch(OrchCall),
    /// `manage …` and `confirm`: the two calls of `manage_shared`.
    Manage(manage::Step),
    /// `remember <text>`
    Remember(String),
    /// `effort`
    Effort,
    /// `model`
    Model,
    /// `mode`
    Mode,
    /// `params`
    Params,
    /// `where`
    Where,
    /// `notes`
    Notes,
    /// `sleep <milliseconds>`
    Sleep(u64),
    /// `usage <input> <output>`
    Usage {
        /// Tokens in.
        input: i64,
        /// Tokens out.
        output: i64,
    },
    /// `exhaust [<minutes>]`
    Exhaust(Option<i64>),
    /// `limit <part> [<minutes>]`
    Limit {
        /// The part of the plan that is used up: a family of models, as the vendor names it.
        part: String,
        /// For how long, in minutes; the plan's window without a number.
        minutes: Option<i64>,
    },
    /// `busy [<what the vendor said>]`
    Busy(String),
    /// `unreachable [<what the vendor said>]`
    Unreachable(String),
    /// `cut <text>`
    Cut(String),
    /// `fail <code> <message> [| <what the vendor said>]`
    Fail {
        /// The code.
        code: String,
        /// The message.
        message: String,
        /// What the vendor said, if the script says it.
        said: Option<String>,
    },
    /// `lose`
    Lose,
    /// `expire`
    Expire,
    /// `title <text>`
    Title(String),
    /// `compact [<milliseconds>]`
    Compact(u64),
    /// `compacted`
    Compacted,
    /// `crash`
    Crash,
}

/// A script that was read.
#[derive(Debug, Clone, PartialEq)]
pub struct Script {
    /// The directives with the number of the line each is on.
    pub lines: Vec<(usize, Directive)>,
}

/// What is wrong with a script.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("line {line}: {message}")]
pub struct ScriptError {
    /// The line, from 1.
    pub line: usize,
    /// What is wrong with it.
    pub message: String,
}

fn fault(line: usize, message: impl Into<String>) -> ScriptError {
    ScriptError {
        line,
        message: message.into(),
    }
}

/// Splits `text` at every `separator` that is not escaped with a backslash. The pieces keep their
/// escapes, for [`unescape`].
fn split_unescaped(text: &str, separator: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut escaped = false;
    for (at, c) in text.char_indices() {
        if escaped {
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == separator {
            parts.push(&text[start..at]);
            start = at + c.len_utf8();
        }
    }
    parts.push(&text[start..]);
    parts
}

/// `\n` is a line break, `\\` a backslash, `\|` and `\;` the characters that would separate. A
/// directive is one line; this is how a text gets more.
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some(next @ ('\\' | '|' | ';')) => out.push(next),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

fn text_of(rest: &str) -> String {
    unescape(rest.trim())
}

fn number<T: std::str::FromStr>(line: usize, what: &str, text: &str) -> Result<T, ScriptError> {
    text.parse()
        .map_err(|_| fault(line, format!("{what} is a whole number, not {text:?}")))
}

/// How long a limit lasts, in minutes: at least one, and at most [`MAX_LIMIT_MINUTES`].
fn minutes_of(line: usize, text: &str) -> Result<i64, ScriptError> {
    let minutes: i64 = number(line, "the minutes", text)?;
    if (1..=MAX_LIMIT_MINUTES).contains(&minutes) {
        Ok(minutes)
    } else {
        Err(fault(
            line,
            format!("a limit lasts from 1 to {MAX_LIMIT_MINUTES} minutes, not {minutes}"),
        ))
    }
}

/// Reads an input. `None` when it is no script: its first line is not `#!fake`. The context Orch puts before
/// the first input of a session (`<orch>` … `</orch>`) is not the person's text and is passed over.
/// What the person wrote: the input without the context Orch puts before a session's first input (`<orch>` ...
/// `</orch>`), which is the agent's to read and no part of what was said.
pub fn without_context(input: &str) -> &str {
    input
        .trim_start()
        .strip_prefix("<orch>")
        .and_then(|rest| rest.split_once("</orch>"))
        .map_or(input, |(_, after)| after.trim_start())
}

pub fn parse(input: &str) -> Result<Option<Script>, ScriptError> {
    let input = without_context(input);
    let mut lines = input.lines();
    if lines.next().map(str::trim_end) != Some(MARKER) {
        return Ok(None);
    }
    let mut directives = Vec::new();
    for (index, raw) in lines.enumerate() {
        let number = index + 2;
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        directives.push((number, directive(number, line, true)?));
    }
    Ok(Some(Script { lines: directives }))
}

#[allow(
    clippy::too_many_lines,
    reason = "one arm for each directive of the manual is the clearest form of the table"
)]
fn directive(line_no: usize, line: &str, top: bool) -> Result<Directive, ScriptError> {
    let (keyword, rest) = line
        .split_once(char::is_whitespace)
        .map_or((line, ""), |(k, r)| (k, r.trim()));
    let none = |what: &str| -> Result<(), ScriptError> {
        if rest.is_empty() {
            Ok(())
        } else {
            Err(fault(line_no, format!("{what} takes nothing more")))
        }
    };
    let need = |what: &str| -> Result<(), ScriptError> {
        if rest.is_empty() {
            Err(fault(line_no, format!("{what} needs more: {line:?}")))
        } else {
            Ok(())
        }
    };
    Ok(match keyword {
        "say" => Directive::Say(text_of(rest)),
        "think" => Directive::Think(text_of(rest)),
        "plan" => {
            if !top {
                return Err(fault(line_no, "plan is not for a sub-agent"));
            }
            let steps: Vec<String> = split_unescaped(rest, '|')
                .into_iter()
                .map(text_of)
                .filter(|step| !step.is_empty())
                .collect();
            if steps.is_empty() {
                return Err(fault(
                    line_no,
                    "plan needs at least one step: plan a | b | c",
                ));
            }
            Directive::Plan(steps)
        }
        "background" => {
            need("background")?;
            Directive::Background(text_of(rest))
        }
        "run" => {
            need("run")?;
            let (command, outcome) = rest.split_once(" => ").unwrap_or((rest, "0"));
            let (code, output) = outcome
                .trim()
                .split_once(char::is_whitespace)
                .map_or((outcome.trim(), ""), |(c, o)| (c, o.trim_start()));
            Directive::Run {
                command: text_of(command),
                exit_code: number(line_no, "an exit code", code)?,
                output: unescape(output),
            }
        }
        "edit" => {
            let tokens: Vec<&str> = rest.split_whitespace().collect();
            let [path @ .., added, removed] = tokens.as_slice() else {
                return Err(fault(line_no, "edit is: edit <path> +<added> -<removed>"));
            };
            let added = added
                .strip_prefix('+')
                .ok_or_else(|| fault(line_no, "edit is: edit <path> +<added> -<removed>"))?;
            let removed = removed
                .strip_prefix('-')
                .ok_or_else(|| fault(line_no, "edit is: edit <path> +<added> -<removed>"))?;
            if path.is_empty() {
                return Err(fault(line_no, "edit needs a path"));
            }
            let (added, removed): (u32, u32) = (
                number(line_no, "the lines added", added)?,
                number(line_no, "the lines removed", removed)?,
            );
            if added > MAX_EDIT_LINES || removed > MAX_EDIT_LINES {
                return Err(fault(
                    line_no,
                    format!("an edit has at most {MAX_EDIT_LINES} lines on a side"),
                ));
            }
            Directive::Edit {
                path: path.join(" "),
                added,
                removed,
            }
        }
        "mcp" => {
            let mut words = rest.splitn(3, char::is_whitespace);
            let server = words.next().unwrap_or_default();
            let tool = words.next().unwrap_or_default();
            if server.is_empty() || tool.is_empty() {
                return Err(fault(line_no, "mcp is: mcp <server> <tool> <json>"));
            }
            let json = words.next().unwrap_or("{}").trim();
            let arguments = if json.is_empty() {
                Json::Object(serde_json::Map::new())
            } else {
                serde_json::from_str(json)
                    .map_err(|e| fault(line_no, format!("the arguments are not JSON: {e}")))?
            };
            Directive::Mcp {
                server: server.to_owned(),
                tool: tool.to_owned(),
                arguments,
            }
        }
        "ask" => {
            let (what, text) = rest
                .split_once(char::is_whitespace)
                .map_or((rest, ""), |(w, t)| (w, t.trim()));
            match what {
                "command" if !text.is_empty() => Directive::AskCommand(text_of(text)),
                "input" if !text.is_empty() => Directive::AskInput(text_of(text)),
                _ => {
                    return Err(fault(
                        line_no,
                        "ask is: ask command <command>, or ask input <question>",
                    ));
                }
            }
        }
        "sub" => {
            if !top {
                return Err(fault(line_no, "a sub-agent does not start another"));
            }
            let (name, body) = rest
                .split_once(':')
                .ok_or_else(|| fault(line_no, "sub is: sub <name>: <directive>; <directive>"))?;
            let name = name.trim();
            if name.is_empty()
                || !name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                return Err(fault(
                    line_no,
                    "the name of a sub-agent is letters, digits, - and _",
                ));
            }
            let mut inside = Vec::new();
            for part in split_unescaped(body, ';') {
                let part = part.trim();
                if !part.is_empty() {
                    inside.push(directive(line_no, part, false)?);
                }
            }
            if inside.is_empty() {
                return Err(fault(line_no, "a sub-agent runs at least one directive"));
            }
            Directive::Sub {
                name: name.to_owned(),
                body: inside,
            }
        }
        "announce" => {
            need("announce")?;
            let parts = split_unescaped(rest, '|');
            let mut arguments = serde_json::json!({ "title": text_of(parts[0]) });
            if let Some(body) = parts.get(1) {
                arguments["body"] = Json::from(text_of(body));
            }
            orch(OrchTool::Announce, arguments)
        }
        "milestone" => {
            need("milestone")?;
            orch(
                OrchTool::Announce,
                serde_json::json!({ "title": text_of(rest), "category": "milestone" }),
            )
        }
        "capability" => {
            let (key, reason) = rest
                .split_once(char::is_whitespace)
                .map_or((rest, ""), |(k, r)| (k, r.trim()));
            if key.is_empty() || reason.is_empty() {
                return Err(fault(line_no, "capability is: capability <key> <reason>"));
            }
            orch(
                OrchTool::RequestCapability,
                serde_json::json!({ "capability": key, "reason": text_of(reason) }),
            )
        }
        "shared" => {
            let arguments = if rest.is_empty() {
                serde_json::json!({})
            } else {
                serde_json::json!({ "path": text_of(rest) })
            };
            orch(OrchTool::ListShared, arguments)
        }
        "read" => {
            need("read")?;
            let tokens: Vec<&str> = rest.split_whitespace().collect();
            let range = match tokens.as_slice() {
                [path @ .., from, to, detail]
                    if !path.is_empty()
                        && matches!(*detail, "brief" | "normal" | "full")
                        && from.parse::<i64>().is_ok()
                        && to.parse::<i64>().is_ok() =>
                {
                    Some((path.join(" "), *from, *to, *detail))
                }
                _ => None,
            };
            let arguments = match range {
                Some((path, from, to, detail)) => serde_json::json!({
                    "path": path,
                    "from": number::<i64>(line_no, "from", from)?,
                    "to": number::<i64>(line_no, "to", to)?,
                    "detail": detail
                }),
                None => serde_json::json!({ "path": text_of(rest) }),
            };
            orch(OrchTool::ReadShared, arguments)
        }
        "search" => {
            need("search")?;
            orch(
                OrchTool::SearchShared,
                serde_json::json!({ "query": text_of(rest) }),
            )
        }
        "download" => {
            need("download")?;
            orch(
                OrchTool::DownloadShared,
                serde_json::json!({ "path": text_of(rest) }),
            )
        }
        "doc" => {
            let (command, after) = rest
                .split_once(char::is_whitespace)
                .map_or((rest, ""), |(c, a)| (c, a.trim()));
            let parts = split_unescaped(after, '|');
            let path = text_of(parts[0]);
            if command.is_empty() || path.is_empty() {
                return Err(fault(line_no, "doc is: doc <command> <path> | <markdown>"));
            }
            let mut arguments = serde_json::json!({ "command": command, "path": path });
            if let Some(markdown) = parts.get(1) {
                arguments["markdown"] = Json::from(unescape(markdown.trim_start()));
            }
            orch(OrchTool::EditDocument, arguments)
        }
        "chat" | "tell" => {
            let parts = split_unescaped(rest, '|');
            let target = text_of(parts[0]);
            if target.is_empty() || parts.len() < 2 {
                return Err(fault(
                    line_no,
                    format!("{keyword} is: {keyword} <path> | <text> [| <file>…]"),
                ));
            }
            let kind = if keyword == "chat" { "chat" } else { "thread" };
            let mut arguments = serde_json::json!({ "target": { "kind": kind, "path": target }, "text": text_of(parts[1]) });
            let files: Vec<Json> = parts[2..]
                .iter()
                .map(|file| Json::from(text_of(file)))
                .filter(|file| file.as_str().is_some_and(|f| !f.is_empty()))
                .collect();
            if !files.is_empty() {
                arguments["files"] = Json::Array(files);
            }
            orch(OrchTool::SendMessage, arguments)
        }
        "browser" => Directive::Orch(browser::parse(line_no, rest)?),
        "display" => Directive::Orch(display::parse(line_no, rest)?),
        "manage" | "confirm" => Directive::Manage(manage::parse(line_no, keyword, rest)?),
        "remember" => {
            need("remember")?;
            Directive::Remember(text_of(rest))
        }
        "effort" => {
            none("effort")?;
            Directive::Effort
        }
        "model" => {
            none("model")?;
            Directive::Model
        }
        "mode" => {
            none("mode")?;
            Directive::Mode
        }
        "params" => {
            none("params")?;
            Directive::Params
        }
        "where" => {
            none("where")?;
            Directive::Where
        }
        "notes" => {
            none("notes")?;
            Directive::Notes
        }
        "sleep" => Directive::Sleep(number(line_no, "the milliseconds", rest)?),
        "usage" => {
            let mut words = rest.split_whitespace();
            let (Some(input), Some(output), None) = (words.next(), words.next(), words.next())
            else {
                return Err(fault(line_no, "usage is: usage <input> <output>"));
            };
            let (input, output): (i64, i64) = (
                number(line_no, "the input tokens", input)?,
                number(line_no, "the output tokens", output)?,
            );
            if input < 0 || output < 0 {
                return Err(fault(line_no, "tokens are not negative"));
            }
            Directive::Usage { input, output }
        }
        "exhaust" => Directive::Exhaust(if rest.is_empty() {
            None
        } else {
            Some(minutes_of(line_no, rest)?)
        }),
        "limit" => {
            let mut words = rest.split_whitespace();
            let (Some(part), minutes, None) = (words.next(), words.next(), words.next()) else {
                return Err(fault(line_no, "limit is: limit <part> [<minutes>]"));
            };
            Directive::Limit {
                part: part.to_owned(),
                minutes: minutes.map(|text| minutes_of(line_no, text)).transpose()?,
            }
        }
        "busy" => Directive::Busy(text_of(rest)),
        "unreachable" => Directive::Unreachable(text_of(rest)),
        "cut" => {
            need("cut")?;
            Directive::Cut(text_of(rest))
        }
        "fail" => {
            let (code, tail) = rest
                .split_once(char::is_whitespace)
                .map_or((rest, ""), |(c, m)| (c, m.trim()));
            if code.is_empty() {
                return Err(fault(line_no, "fail is: fail <code> <message> [| <said>]"));
            }
            let mut parts = split_unescaped(tail, '|').into_iter();
            let message = parts.next().map(text_of).unwrap_or_default();
            let said = parts.next().map(text_of).filter(|said| !said.is_empty());
            if parts.next().is_some() {
                return Err(fault(line_no, "fail is: fail <code> <message> [| <said>]"));
            }
            Directive::Fail {
                code: code.to_owned(),
                message,
                said,
            }
        }
        "lose" => {
            none("lose")?;
            Directive::Lose
        }
        "expire" => {
            none("expire")?;
            Directive::Expire
        }
        "title" => {
            need("title")?;
            Directive::Title(text_of(rest))
        }
        "compact" => Directive::Compact(if rest.is_empty() {
            PIECE_MILLIS
        } else {
            number(line_no, "the milliseconds", rest)?
        }),
        "compacted" => {
            none("compacted")?;
            Directive::Compacted
        }
        "crash" => {
            none("crash")?;
            Directive::Crash
        }
        other => return Err(fault(line_no, format!("{other:?} is not a directive"))),
    })
}

fn orch(tool: OrchTool, arguments: Json) -> Directive {
    Directive::Orch(OrchCall { tool, arguments })
}

fn count_top_level_says(script: &Script) -> usize {
    script
        .lines
        .iter()
        .filter(|(_, directive)| matches!(directive, Directive::Say(_)))
        .count()
}

// ---------------------------------------------------------------------------------------------------------
// Running

/// How a turn ends other than by reaching the end of its script.
#[derive(Debug)]
enum Flow {
    /// An interrupt, a close or a stop.
    Interrupted,
    /// A failure of the turn.
    Failed(Box<TurnError>),
    /// The harness no longer holds the session.
    Lost,
    /// The harness process died.
    Crashed,
}

impl Flow {
    // Only the exceptional path needs the rich failure record; normal script flow stays small.
    fn failed(error: TurnError) -> Self {
        Self::Failed(Box::new(error))
    }
}

/// An item that grows by text, as [`Runner::stream_text`] streams it.
struct TextItem<'a> {
    /// The whole text.
    text: &'a str,
    /// How many characters come in a piece.
    size: usize,
    /// What grows: the text of a message or the summary of a reasoning.
    field: DeltaField,
    /// The stream the item is in.
    stream: &'a str,
    /// The item it belongs to, for a sub-agent's.
    parent: Option<&'a str>,
    /// The phase of an agent message.
    phase: Option<AgentMessagePhase>,
    /// How the item ends once all of its text has come.
    ending: ItemStatus,
}

struct PlanState {
    native_ref: String,
    steps: Vec<(String, PlanStepStatus)>,
}

struct Runner {
    shared: Arc<Shared>,
    live: Arc<LiveSession>,
    emitter: Emitter,
    cancel: CancellationToken,
    turn: TurnStart,
    ordinal: u32,
    plan: Option<PlanState>,
    manage: manage::Memory,
    usage: TokenUsage,
    says_left: usize,
    started: Time,
}

/// Runs a turn from its start to its end. The caller has put it in the session as the active turn.
pub(crate) async fn run_turn(
    shared: Arc<Shared>,
    live: Arc<LiveSession>,
    turn: TurnStart,
    ordinal: u32,
    cancel: CancellationToken,
) {
    let emitter = Emitter::new(
        shared.sink.clone(),
        Arc::clone(&live),
        Arc::clone(&shared.clock),
        turn.turn_id,
        ordinal,
        turn.model.clone(),
    );
    let started = shared.clock.time();
    let mut runner = Runner {
        shared: Arc::clone(&shared),
        live: Arc::clone(&live),
        emitter,
        cancel,
        turn,
        ordinal,
        plan: None,
        manage: manage::Memory::default(),
        usage: TokenUsage::default(),
        says_left: 0,
        started,
    };
    let outcome = runner.go().await;
    runner.finish(outcome).await;
    let mut active = lock(&live.turn);
    if active
        .as_ref()
        .is_some_and(|turn| turn.turn_id == runner.turn.turn_id)
    {
        *active = None;
    }
}

impl Runner {
    async fn go(&mut self) -> Result<(), Flow> {
        self.emitter
            .turn_started(&format!("turn_{}", self.ordinal))
            .await;
        let text = self.turn.text.clone();
        let parsed = match crate::script::parse(&text) {
            Err(error) => {
                self.emitter
                    .error_item("fake.script", &error.to_string(), false)
                    .await;
                return Err(Flow::failed(turn_error(
                    "harness_error",
                    &format!("the script has a mistake: {error}"),
                    false,
                )));
            }
            Ok(parsed) => parsed,
        };
        self.shared.credentials_lock().turns += 1;
        self.check_credential().await?;
        self.emitter.request_asked().await;
        match parsed {
            None => self.default_answer(&text).await,
            Some(script) => {
                self.says_left = count_top_level_says(&script);
                for (_, directive) in &script.lines {
                    self.emitter.resume_request().await;
                    self.execute(directive, MAIN_STREAM, None).await?;
                    if !matches!(directive, Directive::Plan(_)) {
                        self.advance_plan().await;
                    }
                }
                Ok(())
            }
        }
    }

    // ---- ending

    async fn finish(&mut self, outcome: Result<(), Flow>) {
        let (status, error) = match outcome {
            Ok(()) => {
                self.end_plan(ItemStatus::Completed).await;
                (TurnOutcome::Completed, None)
            }
            Err(Flow::Interrupted) => {
                self.close_requests().await;
                self.end_plan(ItemStatus::Cancelled).await;
                (TurnOutcome::Interrupted, None)
            }
            Err(Flow::Failed(error)) => {
                self.close_requests().await;
                self.end_plan(ItemStatus::Failed).await;
                (TurnOutcome::Failed, Some(*error))
            }
            Err(Flow::Lost) => {
                self.close_requests().await;
                self.end_plan(ItemStatus::Failed).await;
                self.emitter
                    .lost("the fake harness was told to lose the session")
                    .await;
                lock(&self.shared.sessions).remove(&self.live.thread_id);
                self.record(
                    TurnOutcome::Failed,
                    Some(turn_error("session_lost", "the session was lost", true)),
                );
                return;
            }
            Err(Flow::Crashed) => {
                self.emitter
                    .request_ended("harness_error", None, false)
                    .await;
                self.end_plan(ItemStatus::Failed).await;
                self.record(
                    TurnOutcome::Failed,
                    Some(turn_error("harness_error", "the harness crashed", true)),
                );
                self.shared.crash().await;
                return;
            }
        };
        self.record(status, error.clone());
        self.emitter.turn_completed(status, error).await;
    }

    fn record(&self, status: TurnOutcome, error: Option<TurnError>) {
        self.live.record_turn(
            TurnRecord {
                ordinal: self.ordinal,
                turn_id: self.turn.turn_id,
                native_turn_ref: format!("turn_{}", self.ordinal),
                input: self.turn.text.clone(),
                status,
                error,
                usage: self.usage,
            },
            self.shared.clock.time(),
        );
    }

    async fn close_requests(&self) {
        let pending: Vec<String> = lock(&self.live.approvals).keys().cloned().collect();
        for request_ref in pending {
            lock(&self.live.approvals).remove(&request_ref);
            self.emitter.approval_closed(&request_ref).await;
        }
    }

    // ---- waiting

    /// Waits for `future`, taking in what is added to the turn meanwhile, and ends at once if the turn is
    /// interrupted.
    async fn wait<F: std::future::Future>(&self, future: F) -> Result<F::Output, Flow> {
        tokio::pin!(future);
        loop {
            tokio::select! {
                biased;
                () = self.cancel.cancelled() => return Err(Flow::Interrupted),
                () = self.shared.sink.closed() => return Err(Flow::Interrupted),
                output = &mut future => return Ok(output),
                () = self.live.steer_notify.notified() => self.take_steers().await,
            }
        }
    }

    async fn take_steers(&self) {
        let steers: Vec<String> = lock(&self.live.steers).drain(..).collect();
        for text in steers {
            self.emitter
                .say_whole(
                    &format!("Steered: {text}"),
                    Some(AgentMessagePhase::Commentary),
                )
                .await;
        }
    }

    async fn pause(&self, millis: u64) -> Result<(), Flow> {
        self.wait(tokio::time::sleep(Duration::from_millis(millis)))
            .await
    }

    fn alive(&self) -> Result<(), Flow> {
        if self.cancel.is_cancelled() || self.emitter.is_closed() {
            return Err(Flow::Interrupted);
        }
        if !self.shared.alive.load(Ordering::Relaxed) {
            return Err(Flow::Crashed);
        }
        Ok(())
    }

    // ---- the credential

    async fn check_credential(&mut self) -> Result<(), Flow> {
        let arrangement = self.live.spec.settings.arrangement;
        loop {
            // Interest in a change is registered before the credential is looked at, so that a change
            // between the two is not missed.
            let changed = self.shared.credentials_changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let need = self
                .shared
                .credentials_lock()
                .need(arrangement, self.shared.clock.time());
            match need {
                None => break,
                Some(CredentialNeed::Expiring) => {
                    self.emitter
                        .credential_needed(CredentialNeed::Expiring)
                        .await;
                    break;
                }
                Some(reason) => {
                    self.emitter.credential_needed(reason).await;
                    self.wait(changed).await?;
                }
            }
        }
        self.shared.credentials_lock().note_turn_on_held();
        Ok(())
    }

    // ---- the plan

    async fn advance_plan(&mut self) {
        let Some(plan) = self.plan.as_mut() else {
            return;
        };
        let Some(open) = plan
            .steps
            .iter()
            .position(|(_, status)| *status == PlanStepStatus::InProgress)
        else {
            return;
        };
        plan.steps[open].1 = PlanStepStatus::Completed;
        if let Some(next) = plan
            .steps
            .iter_mut()
            .find(|(_, status)| *status == PlanStepStatus::Pending)
        {
            next.1 = PlanStepStatus::InProgress;
        }
        let payload =
            serde_json::to_value(Emitter::plan_payload(&plan.steps)).unwrap_or(Json::Null);
        let all_done = plan
            .steps
            .iter()
            .all(|(_, status)| *status == PlanStepStatus::Completed);
        let native_ref = plan.native_ref.clone();
        self.emitter
            .item_updated(&native_ref, payload.clone())
            .await;
        if all_done {
            self.emitter
                .item_completed(&native_ref, ItemStatus::Completed, payload, None)
                .await;
            self.plan = None;
        }
    }

    /// Ends the plan as it stands: the steps that are left are done if the turn is, and stay as they are
    /// if it is not.
    async fn end_plan(&mut self, status: ItemStatus) {
        let Some(mut plan) = self.plan.take() else {
            return;
        };
        if status == ItemStatus::Completed {
            for (_, step) in &mut plan.steps {
                *step = PlanStepStatus::Completed;
            }
        }
        let payload =
            serde_json::to_value(Emitter::plan_payload(&plan.steps)).unwrap_or(Json::Null);
        self.emitter
            .item_completed(&plan.native_ref, status, payload, None)
            .await;
    }

    // ---- the answer to an input that is no script

    async fn default_answer(&mut self, text: &str) -> Result<(), Flow> {
        let answer = format!("You said: {}", without_context(text));
        let size = usize::try_from(char_count(&answer))
            .unwrap_or(usize::MAX)
            .div_ceil(DEFAULT_PIECES)
            .max(1);
        self.stream_text(TextItem {
            text: &answer,
            size,
            field: DeltaField::Text,
            stream: MAIN_STREAM,
            parent: None,
            phase: Some(AgentMessagePhase::Final),
            ending: ItemStatus::Completed,
        })
        .await
    }

    // ---- directives

    async fn execute(
        &mut self,
        directive: &Directive,
        stream: &str,
        parent: Option<&str>,
    ) -> Result<(), Flow> {
        self.alive()?;
        match directive {
            Directive::Say(text) => {
                let phase = if stream == MAIN_STREAM {
                    self.says_left = self.says_left.saturating_sub(1);
                    Some(if self.says_left == 0 {
                        AgentMessagePhase::Final
                    } else {
                        AgentMessagePhase::Commentary
                    })
                } else {
                    None
                };
                self.stream_text(TextItem {
                    text,
                    size: PIECE_CHARS,
                    field: DeltaField::Text,
                    stream,
                    parent,
                    phase,
                    ending: ItemStatus::Completed,
                })
                .await
            }
            Directive::Think(text) => {
                self.stream_text(TextItem {
                    text,
                    size: PIECE_CHARS,
                    field: DeltaField::Summary,
                    stream,
                    parent,
                    phase: None,
                    ending: ItemStatus::Completed,
                })
                .await
            }
            Directive::Plan(steps) => {
                self.start_plan(steps).await;
                Ok(())
            }
            Directive::Run {
                command,
                exit_code,
                output,
            } => {
                self.run_command(command, *exit_code, output, stream, parent)
                    .await
            }
            Directive::Background(command) => {
                let reference = self.emitter.next_item_ref();
                let cwd = self.live.spec.settings.cwd.clone();
                self.tool_item(
                    &reference,
                    stream,
                    parent,
                    ToolPayload::Command {
                        command: command.clone(),
                        cwd: cwd.clone(),
                        exit_code: None,
                        output: OutputRef::default(),
                        process_id: Some(reference.clone()),
                        background: true,
                        actions: vec![CommandAction::Other],
                    },
                    command,
                    "fake_shell",
                    &serde_json::json!({"command":command}),
                )
                .await;
                lock(&self.live.background).insert(
                    reference.clone(),
                    orch_agent_contract::background::NativeBackgroundCommand {
                        id: reference.clone(),
                        native_item_ref: Some(reference),
                        turn_id: Some(self.turn.turn_id),
                        command: Some(command.clone()),
                        command_truncated: false,
                        cwd: Some(cwd),
                        place: orch_agent_contract::background::BackgroundPlace::Environment,
                        since: Some(self.shared.clock.time()),
                        cpu_percent: None,
                        memory_bytes: None,
                    },
                );
                Ok(())
            }
            Directive::Edit {
                path,
                added,
                removed,
            } => self.edit(path, *added, *removed, stream, parent).await,
            Directive::Mcp {
                server,
                tool,
                arguments,
            } => self.mcp(server, tool, arguments, stream, parent).await,
            Directive::AskCommand(command) => self.ask_command(command, stream, parent).await,
            Directive::AskInput(question) => self.ask_input(question, stream, parent).await,
            Directive::Sub { name, body } => Box::pin(self.sub_agent(name, body)).await,
            Directive::Orch(call) => self.orch_call(call, stream, parent).await,
            Directive::Manage(step) => self.manage_step(step, stream, parent).await,
            Directive::Remember(note) => self.remember(note, stream, parent).await,
            Directive::Effort => {
                let effort = self.turn.effort.as_deref().unwrap_or("none");
                self.emitter
                    .say_whole(&format!("effort: {effort}"), None)
                    .await;
                Ok(())
            }
            Directive::Model => {
                self.emitter
                    .say_whole(&format!("model: {}", self.turn.model), None)
                    .await;
                Ok(())
            }
            Directive::Mode => {
                let approval = serde_json::to_value(self.turn.approval_mode).unwrap_or(Json::Null);
                let sandbox = serde_json::to_value(self.turn.sandbox_mode).unwrap_or(Json::Null);
                self.emitter
                    .say_whole(
                        &format!(
                            "approval: {}, sandbox: {}",
                            approval.as_str().unwrap_or("?"),
                            sandbox.as_str().unwrap_or("?")
                        ),
                        None,
                    )
                    .await;
                Ok(())
            }
            Directive::Params => {
                self.emitter
                    .say_whole(&format!("params: {}", self.turn.params), None)
                    .await;
                Ok(())
            }
            Directive::Where => {
                let settings = &self.live.spec.settings;
                self.emitter
                    .say_whole(
                        &format!("directory: {}, cwd: {}", settings.directory, settings.cwd),
                        None,
                    )
                    .await;
                Ok(())
            }
            Directive::Notes => {
                let notes = &self.live.loaded_notes;
                let line = if notes.is_empty() {
                    "notes: none".to_owned()
                } else {
                    format!("notes: {} loaded: {}", notes.len(), notes.join("; "))
                };
                self.emitter.say_whole(&line, None).await;
                Ok(())
            }
            Directive::Sleep(millis) => self.pause(*millis).await,
            Directive::Usage { input, output } => {
                self.usage = TokenUsage {
                    input_tokens: *input,
                    output_tokens: *output,
                    total_tokens: input + output,
                    ..TokenUsage::default()
                };
                let thread = self.live.snapshot().usage.plus(self.usage);
                self.emitter
                    .usage(
                        self.usage,
                        thread,
                        *input,
                        crate::events::context_in_force(&self.turn.params),
                    )
                    .await;
                Ok(())
            }
            Directive::Exhaust(minutes) => {
                let until = self
                    .shared
                    .credentials_lock()
                    .use_up_for(self.shared.clock.time(), minutes.unwrap_or(WINDOW_MINUTES));
                self.shared.facts_changed().await;
                Err(Flow::failed(
                    turn_error(
                        "provider_exhausted",
                        "the provider's usage limit is reached",
                        false,
                    )
                    .saying(&format!(
                        "fake plan: the usage limit is reached; it resets at {until}"
                    ))
                    .resetting_at(Some(until)),
                ))
            }
            Directive::Limit { part, minutes } => {
                // Part of the plan only: the plan is not used up, so the facts stay as they were and another
                // model may still answer, which is what `retryable` says.
                let until = Time::new(
                    self.shared.clock.time().get()
                        + chrono::Duration::minutes(minutes.unwrap_or(WINDOW_MINUTES)),
                );
                Err(Flow::failed(
                    turn_error(
                        "provider_exhausted",
                        &format!("the plan's limit for {part} is reached"),
                        true,
                    )
                    .saying(&format!(
                        "fake plan: the {part} limit is reached; it resets at {until}"
                    ))
                    .resetting_at(Some(until)),
                ))
            }
            Directive::Busy(said) => Err(Flow::failed(
                turn_error(
                    "provider_busy",
                    "the provider is busy; the turn may be tried again",
                    true,
                )
                .saying(if said.is_empty() { BUSY_SAID } else { said }),
            )),
            Directive::Unreachable(said) => Err(Flow::failed(
                turn_error(
                    "provider_unreachable",
                    "the provider could not be reached; the turn may be tried again",
                    true,
                )
                .saying(if said.is_empty() {
                    UNREACHABLE_SAID
                } else {
                    said
                }),
            )),
            Directive::Cut(text) => {
                // The stream breaks in the middle of what is being said: the item is left as an interrupt
                // would leave it, with what came, and the turn says its answer may be cut short.
                let kept: String = text
                    .chars()
                    .take(text.chars().count() / CUT_KEEPS_ONE_IN)
                    .collect();
                self.stream_text(TextItem {
                    text: &kept,
                    size: PIECE_CHARS,
                    field: DeltaField::Text,
                    stream,
                    parent,
                    phase: None,
                    ending: ItemStatus::Cancelled,
                })
                .await?;
                Err(Flow::failed(
                    turn_error(
                        "provider_unreachable",
                        "the connection broke in the middle of an answer",
                        true,
                    )
                    .saying(UNREACHABLE_SAID)
                    .cut_short(true),
                ))
            }
            Directive::Fail {
                code,
                message,
                said,
            } => {
                let error = turn_error(
                    code,
                    if message.is_empty() {
                        "the fake harness was told to fail the turn"
                    } else {
                        message
                    },
                    false,
                );
                Err(Flow::failed(match said {
                    Some(said) => error.saying(said),
                    None => error,
                }))
            }
            Directive::Lose => Err(Flow::Lost),
            Directive::Expire => {
                self.shared.credentials_lock().reject_held();
                self.emitter
                    .credential_needed(CredentialNeed::Rejected)
                    .await;
                loop {
                    let changed = self.shared.credentials_changed.notified();
                    tokio::pin!(changed);
                    changed.as_mut().enable();
                    let rejected = self
                        .shared
                        .credentials_lock()
                        .held
                        .as_ref()
                        .is_none_or(|held| held.rejected);
                    if !rejected {
                        break Ok(());
                    }
                    self.wait(changed).await?;
                }
            }
            Directive::Title(title) => {
                self.live.update(|file| file.title = Some(title.clone()));
                self.emitter.title(title).await;
                Ok(())
            }
            Directive::Compact(millis) => {
                let reports = self.shared.descriptor.capabilities.phases;
                if reports {
                    self.emitter.phase(TurnPhaseKind::Compacting).await;
                }
                self.pause(*millis).await?;
                if reports {
                    self.emitter.phase(TurnPhaseKind::Working).await;
                } else {
                    self.emitter.compacted_marker(None).await;
                }
                Ok(())
            }
            Directive::Compacted => {
                self.emitter.compacted_marker(None).await;
                Ok(())
            }
            Directive::Crash => Err(Flow::Crashed),
        }
    }

    /// An item that grows by text, in pieces of `size` characters, one every [`PIECE_MILLIS`].
    async fn stream_text(&mut self, item: TextItem<'_>) -> Result<(), Flow> {
        let TextItem {
            text,
            size,
            field,
            stream,
            parent,
            phase,
            ending,
        } = item;
        let native_ref = self.emitter.next_item_ref();
        let (content, key) = match field {
            DeltaField::Summary => (
                ItemContent::Reasoning(ReasoningPayload {
                    summary: String::new(),
                    text_len: 0,
                    hidden: false,
                }),
                "summary",
            ),
            _ => (
                ItemContent::AgentMessage(AgentMessagePayload {
                    text: String::new(),
                    text_len: 0,
                    phase,
                }),
                "text",
            ),
        };
        self.emitter
            .item_started(&native_ref, stream, parent, content)
            .await;
        let payload_of = |so_far: &str| -> Json {
            let mut payload = match field {
                DeltaField::Summary => serde_json::to_value(ReasoningPayload {
                    summary: String::new(),
                    text_len: 0,
                    hidden: false,
                }),
                _ => serde_json::to_value(AgentMessagePayload {
                    text: String::new(),
                    text_len: 0,
                    phase,
                }),
            }
            .unwrap_or(Json::Null);
            payload[key] = Json::from(so_far);
            payload["text_len"] = Json::from(char_count(so_far));
            payload
        };
        let mut so_far = String::new();
        for piece in pieces(text, size) {
            self.emitter
                .item_delta(&native_ref, field, char_count(&so_far), &piece)
                .await;
            so_far.push_str(&piece);
            if let Err(flow) = self.pause(PIECE_MILLIS).await {
                self.emitter
                    .item_completed(
                        &native_ref,
                        ItemStatus::Cancelled,
                        payload_of(&so_far),
                        None,
                    )
                    .await;
                return Err(flow);
            }
        }
        self.emitter
            .item_completed(&native_ref, ending, payload_of(&so_far), None)
            .await;
        Ok(())
    }

    async fn start_plan(&mut self, steps: &[String]) {
        let native_ref = self.emitter.next_item_ref();
        let steps: Vec<(String, PlanStepStatus)> = steps
            .iter()
            .enumerate()
            .map(|(at, text)| {
                (
                    text.clone(),
                    if at == 0 {
                        PlanStepStatus::InProgress
                    } else {
                        PlanStepStatus::Pending
                    },
                )
            })
            .collect();
        self.emitter
            .item_started(
                &native_ref,
                MAIN_STREAM,
                None,
                ItemContent::Plan(Emitter::plan_payload(&steps)),
            )
            .await;
        self.plan = Some(PlanState { native_ref, steps });
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "an item of a tool call is told by its place (reference, stream, parent), its tool and how the harness named it (title, name, input); a struct for them would be written out at each call and read nowhere else"
    )]
    async fn tool_item(
        &self,
        native_ref: &str,
        stream: &str,
        parent: Option<&str>,
        tool: ToolPayload,
        title: &str,
        raw_name: &str,
        raw_input: &Json,
    ) {
        self.emitter
            .item_started(
                native_ref,
                stream,
                parent,
                ItemContent::ToolCall(tool_call(
                    tool,
                    title,
                    self.shared.clock.time(),
                    raw_name,
                    &raw_input.to_string(),
                )),
            )
            .await;
    }

    /// Ends a tool call: the same payload as it began, with what the call came to.
    async fn end_tool(
        &self,
        native_ref: &str,
        status: ItemStatus,
        tool: ToolPayload,
        raw_output: Option<&str>,
        is_error: bool,
        blob: Option<orch_agent_contract::events::BlobRef>,
    ) {
        let mut payload = self.emitter.payload_of(native_ref).unwrap_or(Json::Null);
        payload["tool"] = serde_json::to_value(tool).unwrap_or(Json::Null);
        payload["duration_ms"] =
            Json::from(self.shared.clock.time().unix_millis() - self.started_of(&payload));
        if let Some(output) = raw_output {
            payload["raw"]["output"] =
                serde_json::to_value(OutputRef::of(output)).unwrap_or(Json::Null);
        }
        payload["raw"]["is_error"] = Json::from(is_error);
        self.emitter
            .item_completed(native_ref, status, payload, blob)
            .await;
    }

    fn started_of(&self, payload: &Json) -> i64 {
        payload
            .get("started_at")
            .and_then(Json::as_str)
            .and_then(|at| at.parse::<Time>().ok())
            .map_or_else(|| self.started.unix_millis(), Time::unix_millis)
    }

    async fn run_command(
        &mut self,
        command: &str,
        exit_code: i64,
        output: &str,
        stream: &str,
        parent: Option<&str>,
    ) -> Result<(), Flow> {
        let native_ref = self.emitter.next_item_ref();
        let cwd = self.live.spec.settings.cwd.clone();
        let started = |output: &OutputRef, code: Option<i64>| ToolPayload::Command {
            command: command.to_owned(),
            cwd: cwd.clone(),
            exit_code: code,
            output: output.clone(),
            process_id: None,
            background: false,
            actions: vec![CommandAction::Other],
        };
        self.tool_item(
            &native_ref,
            stream,
            parent,
            started(&OutputRef::default(), None),
            command,
            "fake_shell",
            &serde_json::json!({ "command": command }),
        )
        .await;
        let mut shown = OutputRef::default();
        let size = OUTPUT_PIECE_BYTES.max(output.len().div_ceil(MAX_OUTPUT_PIECES));
        let mut offset = 0i64;
        for piece in byte_pieces(output, size) {
            self.emitter
                .item_delta(&native_ref, DeltaField::Output, offset, &piece)
                .await;
            offset += i64::try_from(piece.len()).unwrap_or(i64::MAX);
            shown.push(&piece);
            if let Err(flow) = self.pause(PIECE_MILLIS).await {
                self.end_tool(
                    &native_ref,
                    ItemStatus::Cancelled,
                    started(&shown, None),
                    Some(output),
                    false,
                    None,
                )
                .await;
                return Err(flow);
            }
        }
        let blob = if shown.truncated {
            let hash = output_hash(output);
            let kept = self.shared.home.root().join("outputs");
            if std::fs::create_dir_all(&kept).is_ok() {
                let _ = std::fs::write(kept.join(hash.to_string()), output);
            }
            shown = shown.with_blob(hash);
            Some(orch_agent_contract::events::BlobRef {
                hash,
                size: i64::try_from(output.len()).unwrap_or(i64::MAX),
            })
        } else {
            None
        };
        let status = if exit_code == 0 {
            ItemStatus::Completed
        } else {
            ItemStatus::Failed
        };
        self.end_tool(
            &native_ref,
            status,
            started(&shown, Some(exit_code)),
            Some(output),
            exit_code != 0,
            blob,
        )
        .await;
        Ok(())
    }

    async fn edit(
        &mut self,
        path: &str,
        added: u32,
        removed: u32,
        stream: &str,
        parent: Option<&str>,
    ) -> Result<(), Flow> {
        let native_ref = self.emitter.next_item_ref();
        let limit = usize::try_from(FILE_CHANGE_DIFF_MAX_BYTES).unwrap_or(usize::MAX);
        let begun = || {
            let mut change = file_change(path, 0, 0, limit);
            change.diff = None;
            ToolPayload::FileChange {
                changes: vec![change],
            }
        };
        self.tool_item(
            &native_ref,
            stream,
            parent,
            begun(),
            &format!("Edit {path}"),
            "fake_edit",
            &serde_json::json!({ "path": path, "added": added, "removed": removed }),
        )
        .await;
        if let Err(flow) = self.pause(PIECE_MILLIS).await {
            self.end_tool(
                &native_ref,
                ItemStatus::Cancelled,
                begun(),
                None,
                false,
                None,
            )
            .await;
            return Err(flow);
        }
        let done = ToolPayload::FileChange {
            changes: vec![file_change(path, added, removed, limit)],
        };
        self.end_tool(
            &native_ref,
            ItemStatus::Completed,
            done,
            Some(&format!("edited {path}: +{added} -{removed}")),
            false,
            None,
        )
        .await;
        Ok(())
    }

    async fn mcp(
        &mut self,
        server: &str,
        tool: &str,
        arguments: &Json,
        stream: &str,
        parent: Option<&str>,
    ) -> Result<(), Flow> {
        let native_ref = self.emitter.next_item_ref();
        let begun = || ToolPayload::Mcp {
            server: server.to_owned(),
            tool: tool.to_owned(),
            arguments: arguments.clone(),
            result: None,
            error: None,
        };
        self.tool_item(
            &native_ref,
            stream,
            parent,
            begun(),
            &format!("{server}.{tool}"),
            &format!("mcp__{server}__{tool}"),
            arguments,
        )
        .await;
        if let Err(flow) = self.pause(PIECE_MILLIS).await {
            self.end_tool(
                &native_ref,
                ItemStatus::Cancelled,
                begun(),
                None,
                false,
                None,
            )
            .await;
            return Err(flow);
        }
        let given = self.live.spec.mcp.iter().any(|name| name == server);
        if given {
            let result = serde_json::json!({
                "content": [ { "type": "text", "text": format!("{server}.{tool} ok") } ],
                "isError": false
            });
            let text = result.to_string();
            self.end_tool(
                &native_ref,
                ItemStatus::Completed,
                ToolPayload::Mcp {
                    server: server.to_owned(),
                    tool: tool.to_owned(),
                    arguments: arguments.clone(),
                    result: Some(result),
                    error: None,
                },
                Some(&text),
                false,
                None,
            )
            .await;
        } else {
            let message = format!("this session was given no MCP server named {server:?}");
            self.end_tool(
                &native_ref,
                ItemStatus::Failed,
                ToolPayload::Mcp {
                    server: server.to_owned(),
                    tool: tool.to_owned(),
                    arguments: arguments.clone(),
                    result: None,
                    error: Some(message.clone()),
                },
                Some(&message),
                true,
                None,
            )
            .await;
        }
        Ok(())
    }

    // ---- questions

    /// Asks, and waits for the answer for as long as it takes.
    async fn ask(
        &mut self,
        title: &str,
        request: HarnessRequest,
    ) -> Result<ApprovalDecision, Flow> {
        let person_answers = request.request_kind == HarnessRequestKind::UserInput;
        let request_ref = format!("req_{}", self.live.next_counter(Counter::Request));
        let (reply, answer) = oneshot::channel();
        lock(&self.live.approvals).insert(
            request_ref.clone(),
            PendingApproval {
                request: request.clone(),
                reply,
            },
        );
        self.emitter
            .approval_requested(&request_ref, title, request)
            .await;
        match self.wait(answer).await {
            Ok(Ok(decision)) => {
                if person_answers {
                    self.emitter.input_returned();
                }
                Ok(decision)
            }
            Ok(Err(_)) => Err(Flow::Interrupted),
            Err(flow) => {
                if lock(&self.live.approvals).remove(&request_ref).is_some() {
                    self.emitter.approval_closed(&request_ref).await;
                }
                Err(flow)
            }
        }
    }

    async fn ask_command(
        &mut self,
        command: &str,
        stream: &str,
        parent: Option<&str>,
    ) -> Result<(), Flow> {
        let cwd = self.live.spec.settings.cwd.clone();
        let allowed_for_session = lock(&self.live.allowed_commands).contains(command);
        let allowed = if allowed_for_session {
            true
        } else {
            let request = HarnessRequest {
                request_kind: HarnessRequestKind::Command,
                options: vec![
                    ApprovalOption {
                        id: "once".to_owned(),
                        label: "Allow once".to_owned(),
                        style: OptionStyle::Allow,
                        scope: Some(OptionScope::Once),
                        rule: None,
                    },
                    ApprovalOption {
                        id: "session".to_owned(),
                        label: "Allow for this session".to_owned(),
                        style: OptionStyle::Allow,
                        scope: Some(OptionScope::Session),
                        rule: None,
                    },
                    ApprovalOption {
                        id: "deny".to_owned(),
                        label: "Deny".to_owned(),
                        style: OptionStyle::Deny,
                        scope: None,
                        rule: None,
                    },
                ],
                detail: HarnessRequestDetail::Command {
                    command: command.to_owned(),
                    cwd,
                    reason: None,
                },
            };
            let options = request.options.clone();
            let decision = self.ask(&format!("Run {command}"), request).await?;
            let chosen = match &decision {
                ApprovalDecision::Option { option_id, .. } => {
                    options.iter().find(|option| &option.id == option_id)
                }
                _ => None,
            };
            if chosen.is_some_and(|option| option.scope == Some(OptionScope::Session)) {
                lock(&self.live.allowed_commands).insert(command.to_owned());
            }
            chosen.is_some_and(|option| option.style == OptionStyle::Allow)
        };
        if allowed {
            self.run_command(command, 0, "", stream, parent).await
        } else {
            self.refused_command(command, stream, parent).await;
            Ok(())
        }
    }

    /// The call of a command that a person refused: it did not run.
    async fn refused_command(&self, command: &str, stream: &str, parent: Option<&str>) {
        let native_ref = self.emitter.next_item_ref();
        let tool = ToolPayload::Command {
            command: command.to_owned(),
            cwd: self.live.spec.settings.cwd.clone(),
            exit_code: None,
            output: OutputRef::default(),
            process_id: None,
            background: false,
            actions: vec![CommandAction::Other],
        };
        self.tool_item(
            &native_ref,
            stream,
            parent,
            tool.clone(),
            command,
            "fake_shell",
            &serde_json::json!({ "command": command }),
        )
        .await;
        self.end_tool(
            &native_ref,
            ItemStatus::Cancelled,
            tool,
            Some("denied"),
            true,
            None,
        )
        .await;
    }

    async fn ask_input(
        &mut self,
        question: &str,
        _stream: &str,
        _parent: Option<&str>,
    ) -> Result<(), Flow> {
        let request = HarnessRequest {
            request_kind: HarnessRequestKind::UserInput,
            options: Vec::new(),
            detail: HarnessRequestDetail::UserInput {
                native_item_ref: None,
                questions: vec![Question {
                    id: "q1".to_owned(),
                    header: "Question".to_owned(),
                    question: question.to_owned(),
                    multiple: false,
                    allow_other: true,
                    options: None,
                }],
            },
        };
        let decision = self.ask(question, request).await?;
        let answer = match decision {
            ApprovalDecision::Answers { answers, .. } => answers
                .values()
                .flatten()
                .cloned()
                .collect::<Vec<_>>()
                .join(", "),
            _ => String::new(),
        };
        self.emitter
            .say_whole(&format!("You answered: {answer}"), None)
            .await;
        Ok(())
    }

    // ---- sub-agents

    async fn sub_agent(&mut self, name: &str, body: &[Directive]) -> Result<(), Flow> {
        let native_ref = self.emitter.next_item_ref();
        let child = sub_stream(name);
        let state = |state: SubagentState| ToolPayload::Subagent {
            child_stream: child.clone(),
            name: Some(name.to_owned()),
            role: None,
            prompt: None,
            state,
            model: None,
        };
        self.tool_item(
            &native_ref,
            MAIN_STREAM,
            None,
            state(SubagentState::Running),
            &format!("Sub-agent {name}"),
            "fake_agent",
            &serde_json::json!({ "name": name }),
        )
        .await;
        let mut outcome = Ok(());
        for directive in body {
            if let Err(flow) = self.execute(directive, &child, Some(&native_ref)).await {
                outcome = Err(flow);
                break;
            }
        }
        let (status, ended) = match &outcome {
            Ok(()) => (ItemStatus::Completed, SubagentState::Completed),
            Err(Flow::Interrupted) => (ItemStatus::Cancelled, SubagentState::Interrupted),
            Err(_) => (ItemStatus::Failed, SubagentState::Failed),
        };
        self.emitter
            .item_updated(&native_ref, serde_json::json!({ "tool": state(ended) }))
            .await;
        self.end_tool(&native_ref, status, state(ended), Some("done"), false, None)
            .await;
        outcome
    }

    // ---- Orch's tools

    async fn orch_call(
        &mut self,
        call: &OrchCall,
        stream: &str,
        parent: Option<&str>,
    ) -> Result<(), Flow> {
        let native_ref = self.emitter.next_item_ref();
        let call_ref = format!("call_{}", self.live.next_counter(Counter::Call));
        let begun = |result: Option<Json>| ToolPayload::Orch {
            tool: call.tool,
            arguments: call.arguments.clone(),
            result,
        };
        let raw_name = format!("mcp__orch__{}", call.tool.name());
        self.tool_item(
            &native_ref,
            stream,
            parent,
            begun(None),
            title_of(call.tool),
            &raw_name,
            &call.arguments,
        )
        .await;
        let (reply, result) = oneshot::channel();
        lock(&self.live.tool_calls).insert(call_ref.clone(), reply);
        self.emitter
            .orch_tool_call(&call_ref, call.tool.name(), call.arguments.clone())
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
                if matches!(call.tool, OrchTool::UseBrowser | OrchTool::UseDisplay) {
                    self.emitter.say_whole(&text, None).await;
                }
                self.end_tool(
                    &native_ref,
                    ItemStatus::Completed,
                    begun(Some(value)),
                    Some(&text),
                    false,
                    None,
                )
                .await;
                Ok(())
            }
            Ok(Ok(Ok(Err(refusal)))) => {
                let line = format!("{}: {}", refusal.code, refusal.message);
                if matches!(call.tool, OrchTool::UseBrowser | OrchTool::UseDisplay) {
                    self.emitter.say_whole(&line, None).await;
                }
                self.end_tool(
                    &native_ref,
                    ItemStatus::Failed,
                    begun(Some(serde_json::json!({
                        "error": { "code": refusal.code, "message": refusal.message }
                    }))),
                    Some(&line),
                    true,
                    None,
                )
                .await;
                Ok(())
            }
            Ok(Ok(Err(_))) | Ok(Err(_)) => {
                // Nobody answered within the time, or the answer was dropped.
                let message = "Orch is unreachable; try again later";
                self.end_tool(
                    &native_ref,
                    ItemStatus::Failed,
                    begun(Some(serde_json::json!({ "error": { "message": message } }))),
                    Some(message),
                    true,
                    None,
                )
                .await;
                Ok(())
            }
        }
    }

    // ---- notes

    async fn remember(
        &mut self,
        note: &str,
        stream: &str,
        parent: Option<&str>,
    ) -> Result<(), Flow> {
        let kind = self.shared.profile.memory;
        match (&self.live.spec.settings.memory, kind) {
            (None, _) => {
                self.emitter
                    .say_whole("Not kept: notes are off for this session.", None)
                    .await;
            }
            (Some(_), MemoryKind::Home) => {
                self.emitter.say_whole(
                    "Not kept: this harness has one memory for its whole home, and that is off.",
                    None,
                ).await;
            }
            (Some(memory), MemoryKind::None) if memory.kept_by == NoteKeeper::Harness => {
                self.emitter
                    .say_whole("Not kept: this harness has no memory.", None)
                    .await;
            }
            (Some(memory), _) => {
                let directory = memory.directory.clone();
                let by_agent = memory.kept_by == NoteKeeper::Agent;
                match append_note(&directory, note) {
                    Err(error) => {
                        return Err(Flow::failed(turn_error(
                            "harness_error",
                            &error.detail(),
                            false,
                        )));
                    }
                    Ok(()) if by_agent => {
                        let path = format!(
                            "{directory}/{}",
                            orch_agent_contract::session::MEMORY_INDEX_FILE
                        );
                        let native_ref = self.emitter.next_item_ref();
                        let tool = ToolPayload::FileChange {
                            changes: vec![file_change(&path, 1, 0, usize::MAX)],
                        };
                        self.tool_item(
                            &native_ref,
                            stream,
                            parent,
                            tool.clone(),
                            &format!("Edit {path}"),
                            "fake_edit",
                            &serde_json::json!({ "path": path, "note": note }),
                        )
                        .await;
                        self.end_tool(
                            &native_ref,
                            ItemStatus::Completed,
                            tool,
                            Some("noted"),
                            false,
                            None,
                        )
                        .await;
                    }
                    Ok(()) => {
                        self.emitter
                            .say_whole(&format!("Noted: {note}"), None)
                            .await;
                    }
                }
            }
        }
        Ok(())
    }
}

/// What a call of each of Orch's tools is titled in the timeline.
#[must_use]
pub const fn title_of(tool: OrchTool) -> &'static str {
    match tool {
        OrchTool::ListShared => "List what was shared",
        OrchTool::ReadShared => "Read a shared entry",
        OrchTool::SearchShared => "Search what was shared",
        OrchTool::DownloadShared => "Copy a shared entry into the workspace",
        OrchTool::EditDocument => "Edit a document",
        OrchTool::ManageShared => "Organize documents and folders",
        OrchTool::SendMessage | OrchTool::SendChatMessage | OrchTool::SendThreadMessage => {
            "Send a message"
        }
        OrchTool::Announce => "Announce",
        OrchTool::RequestCapability => "Ask for a capability",
        OrchTool::UseBrowser => "Open or announce a browser",
        OrchTool::UseDisplay => "Open or announce a display",
        OrchTool::ListPorts => "List the environment's ports",
        OrchTool::OpenPort => "Ask to open a port",
        OrchTool::ClosePort => "Close a port's opening",
        OrchTool::ListPortConnections => "List who reached a port",
        OrchTool::Goal => "Report on the goal",
    }
}

/// Reports to the host that the harness died, as the adapter does when its process does.
pub(crate) async fn report_crash(sink: &EventSink, harness_version: &str) {
    let _ = sink
        .emit(AdapterEvent::Status(
            orch_agent_contract::facts::HarnessStatus {
                status: HarnessState::Error,
                detail: Some("the fake harness crashed".to_owned()),
                harness_version: Some(harness_version.to_owned()),
                sessions_open: 0,
            },
        ))
        .await;
}

/// Whether a decision fits the request it answers: an option for a request that has options, answers
/// for questions, an action for an MCP server's request for input.
pub fn decision_fits(request: &HarnessRequest, decision: &ApprovalDecision) -> Result<(), String> {
    match (request.request_kind, decision) {
        (
            HarnessRequestKind::Command
            | HarnessRequestKind::FileChange
            | HarnessRequestKind::Permissions
            | HarnessRequestKind::Other,
            ApprovalDecision::Option { option_id, .. },
        ) => {
            if request.options.iter().any(|option| &option.id == option_id) {
                Ok(())
            } else {
                Err(format!("{option_id:?} is none of the options"))
            }
        }
        (HarnessRequestKind::UserInput, ApprovalDecision::Answers { .. })
        | (HarnessRequestKind::McpElicitation, ApprovalDecision::Elicitation { .. }) => Ok(()),
        _ => Err("that kind of decision does not answer this kind of request".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(script: &str) -> Vec<Directive> {
        parse(&format!("{MARKER}\n{script}"))
            .unwrap()
            .unwrap()
            .lines
            .into_iter()
            .map(|(_, directive)| directive)
            .collect()
    }

    fn mistake(script: &str) -> ScriptError {
        parse(&format!("{MARKER}\n{script}")).unwrap_err()
    }

    #[test]
    fn an_input_is_a_script_only_when_its_first_line_is_the_marker() {
        assert_eq!(parse("hello").unwrap(), None);
        assert_eq!(parse("say hi\n#!fake").unwrap(), None);
        assert_eq!(parse("#!fake  \nsay hi").unwrap().unwrap().lines.len(), 1);
        assert_eq!(parse("#!fake").unwrap().unwrap().lines.len(), 0);
    }

    #[test]
    fn the_echo_says_back_what_the_person_wrote_without_orchs_context() {
        assert_eq!(
            without_context("<orch>\nWhere you are\n</orch>\nhello"),
            "hello"
        );
        assert_eq!(without_context("hello"), "hello");
        assert_eq!(
            without_context("<orch> never closed"),
            "<orch> never closed"
        );
    }

    #[test]
    fn orchs_context_before_the_first_input_is_passed_over() {
        let context = "<orch>\nWhere you are\n- Topic \"login\".\n</orch>\n\n";
        assert_eq!(
            parse(&format!("{context}#!fake\nsay hi"))
                .unwrap()
                .unwrap()
                .lines
                .len(),
            1
        );
        assert_eq!(parse(&format!("{context}hello")).unwrap(), None);
        assert_eq!(parse("<orch> never closed\n#!fake\nsay hi").unwrap(), None);
    }

    #[test]
    fn blank_lines_and_comments_are_skipped_and_lines_are_counted_from_the_marker() {
        let script = parse("#!fake\n\n# a comment\nsay hi\n\nthink hmm")
            .unwrap()
            .unwrap();
        assert_eq!(
            script.lines,
            [
                (4, Directive::Say("hi".into())),
                (6, Directive::Think("hmm".into()))
            ]
        );
    }

    #[test]
    fn the_directives_of_the_table_are_read() {
        let directives = lines(concat!(
            "say Hello there\n",
            "think let me see\n",
            "plan read | write | test\n",
            "run cargo test => 101 3 failed\n",
            "run ls\n",
            "edit src/main.rs +12 -3\n",
            "mcp github create_issue {\"title\": \"x\"}\n",
            "ask command rm -rf target\n",
            "ask input Which database?\n",
            "usage 1200 340\n",
            "sleep 250\n",
            "exhaust\n",
            "fail rate_limited slow down\n",
            "lose\n",
            "expire\n",
            "effort\n",
            "remember the build needs libssl\n",
        ));
        assert_eq!(directives[0], Directive::Say("Hello there".into()));
        assert_eq!(
            directives[2],
            Directive::Plan(vec!["read".into(), "write".into(), "test".into()])
        );
        assert_eq!(
            directives[3],
            Directive::Run {
                command: "cargo test".into(),
                exit_code: 101,
                output: "3 failed".into()
            }
        );
        assert_eq!(
            directives[4],
            Directive::Run {
                command: "ls".into(),
                exit_code: 0,
                output: String::new()
            }
        );
        assert_eq!(
            directives[5],
            Directive::Edit {
                path: "src/main.rs".into(),
                added: 12,
                removed: 3
            }
        );
        assert_eq!(
            directives[6],
            Directive::Mcp {
                server: "github".into(),
                tool: "create_issue".into(),
                arguments: serde_json::json!({ "title": "x" })
            }
        );
        assert_eq!(directives[7], Directive::AskCommand("rm -rf target".into()));
        assert_eq!(directives[8], Directive::AskInput("Which database?".into()));
        assert_eq!(
            directives[9],
            Directive::Usage {
                input: 1200,
                output: 340
            }
        );
        assert_eq!(directives[10], Directive::Sleep(250));
        assert_eq!(directives[11], Directive::Exhaust(None));
        assert_eq!(
            directives[12],
            Directive::Fail {
                code: "rate_limited".into(),
                message: "slow down".into(),
                said: None
            }
        );
        assert_eq!(directives[13], Directive::Lose);
        assert_eq!(directives[14], Directive::Expire);
        assert_eq!(directives[15], Directive::Effort);
        assert_eq!(
            directives[16],
            Directive::Remember("the build needs libssl".into())
        );
    }

    #[test]
    fn a_directive_that_names_a_tool_of_orch_makes_the_call_with_its_arguments() {
        let calls: Vec<OrchCall> = lines(concat!(
            "announce Root cause | It was the cache\n",
            "milestone Tests green\n",
            "capability kvm to boot the image\n",
            "shared\n",
            "shared docs\n",
            "read docs/plan.md 10 20 brief\n",
            "read docs/plan.md\n",
            "search retry budget\n",
            "download files/report.pdf\n",
            "doc create docs/Week 41.md | # Week 41\\nAll quiet.\n",
            "chat chats/team | hello all | notes.txt | b.png\n",
            "tell api | please rebase\n",
        ))
        .into_iter()
        .map(|d| match d {
            Directive::Orch(call) => call,
            other => panic!("{other:?}"),
        })
        .collect();
        let tools: Vec<&str> = calls.iter().map(|c| c.tool.name()).collect();
        assert_eq!(
            tools,
            [
                "announce",
                "announce",
                "request_capability",
                "list_shared",
                "list_shared",
                "read_shared",
                "read_shared",
                "search_shared",
                "download_shared",
                "edit_document",
                "send_message",
                "send_message"
            ]
        );
        assert_eq!(
            calls[0].arguments,
            serde_json::json!({ "title": "Root cause", "body": "It was the cache" })
        );
        assert_eq!(
            calls[1].arguments,
            serde_json::json!({ "title": "Tests green", "category": "milestone" })
        );
        assert_eq!(
            calls[2].arguments,
            serde_json::json!({ "capability": "kvm", "reason": "to boot the image" })
        );
        assert_eq!(calls[3].arguments, serde_json::json!({}));
        assert_eq!(calls[4].arguments, serde_json::json!({ "path": "docs" }));
        assert_eq!(
            calls[5].arguments,
            serde_json::json!({ "path": "docs/plan.md", "from": 10, "to": 20, "detail": "brief" })
        );
        assert_eq!(
            calls[6].arguments,
            serde_json::json!({ "path": "docs/plan.md" })
        );
        assert_eq!(
            calls[9].arguments,
            serde_json::json!({
                "command": "create", "path": "docs/Week 41.md", "markdown": "# Week 41\nAll quiet."
            })
        );
        assert_eq!(
            calls[10].arguments,
            serde_json::json!({ "target": { "kind": "chat", "path": "chats/team" }, "text": "hello all", "files": ["notes.txt", "b.png"] })
        );
        assert_eq!(
            calls[11].arguments,
            serde_json::json!({ "target": { "kind": "thread", "path": "api" }, "text": "please rebase" })
        );
    }

    #[test]
    fn a_sub_agent_runs_the_directives_after_its_colon_and_does_not_start_another() {
        let directives = lines("sub scout: say looking; run ls => 0 a b; think done");
        let Directive::Sub { name, body } = &directives[0] else {
            panic!("{directives:?}");
        };
        assert_eq!(name, "scout");
        assert_eq!(body.len(), 3);
        assert_eq!(
            mistake("sub a: sub b: say x").message,
            "a sub-agent does not start another"
        );
        assert!(mistake("sub a: plan x | y").message.contains("plan"));
        assert!(mistake("sub a b: say x").message.contains("name"));
        assert!(mistake("sub a:").message.contains("at least one"));
    }

    #[test]
    fn a_text_gets_more_lines_and_the_characters_that_separate_with_a_backslash() {
        let directives =
            lines("say one\\ntwo\\\\three\nplan a \\| b | c\nsub x: say a\\; b; say c");
        assert_eq!(directives[0], Directive::Say("one\ntwo\\three".into()));
        assert_eq!(
            directives[1],
            Directive::Plan(vec!["a | b".into(), "c".into()])
        );
        let Directive::Sub { body, .. } = &directives[2] else {
            panic!()
        };
        assert_eq!(body[0], Directive::Say("a; b".into()));
        assert_eq!(body[1], Directive::Say("c".into()));
    }

    #[test]
    fn a_mistake_names_its_line_and_nothing_of_the_script_is_read_past_it() {
        let error = mistake("say fine\nrun ls => seven\nsay never");
        assert_eq!(error.line, 3);
        assert!(error.message.contains("exit code"), "{error}");
        assert_eq!(mistake("dance").message, "\"dance\" is not a directive");
        assert!(mistake("edit file +1").message.contains("edit is"));
        assert!(mistake("edit file +a -1").message.contains("whole number"));
        assert!(mistake("mcp github").message.contains("mcp is"));
        assert!(mistake("mcp a b {not json").message.contains("not JSON"));
        assert!(mistake("usage 1").message.contains("usage is"));
        assert!(mistake("usage -1 2").message.contains("negative"));
        assert!(mistake("ask maybe now").message.contains("ask is"));
        assert!(mistake("plan").message.contains("at least one step"));
        assert!(mistake("effort now").message.contains("takes nothing more"));
        assert!(mistake("fail").message.contains("fail is"));
        assert!(mistake("fail code a | b | c").message.contains("fail is"));
        assert!(mistake("limit").message.contains("limit is"));
        assert!(mistake("limit opus 5 6").message.contains("limit is"));
        assert!(mistake("limit opus soon").message.contains("whole number"));
        assert!(mistake("exhaust 0").message.contains("from 1 to"));
        assert!(mistake("exhaust 999999999").message.contains("from 1 to"));
        assert!(mistake("cut").message.contains("cut needs more"));
        assert!(mistake("tell api").message.contains("tell is"));
        assert!(mistake("edit f +100000 -1").message.contains("at most"));
    }

    #[test]
    fn the_directives_that_fail_a_turn_in_each_of_its_kinds_are_read() {
        let directives = lines(concat!(
            "exhaust 90\n",
            "limit opus\n",
            "limit sonnet 45\n",
            "busy\n",
            "busy 529 overloaded\n",
            "unreachable connection refused\n",
            "cut The answer began and\n",
            "fail rate_limited slow down | You've hit your limit\n",
            "fail some_code\n",
        ));
        assert_eq!(directives[0], Directive::Exhaust(Some(90)));
        assert_eq!(
            directives[1],
            Directive::Limit {
                part: "opus".into(),
                minutes: None
            }
        );
        assert_eq!(
            directives[2],
            Directive::Limit {
                part: "sonnet".into(),
                minutes: Some(45)
            }
        );
        assert_eq!(directives[3], Directive::Busy(String::new()));
        assert_eq!(directives[4], Directive::Busy("529 overloaded".into()));
        assert_eq!(
            directives[5],
            Directive::Unreachable("connection refused".into())
        );
        assert_eq!(directives[6], Directive::Cut("The answer began and".into()));
        assert_eq!(
            directives[7],
            Directive::Fail {
                code: "rate_limited".into(),
                message: "slow down".into(),
                said: Some("You've hit your limit".into())
            }
        );
        assert_eq!(
            directives[8],
            Directive::Fail {
                code: "some_code".into(),
                message: String::new(),
                said: None
            }
        );
    }

    #[test]
    fn the_extras_beyond_the_table_are_read_too() {
        let directives = lines(
            "title A name\ncompact\ncompact 500\ncompacted\ncrash\nmodel\nmode\nparams\nwhere\nnotes",
        );
        assert_eq!(directives[0], Directive::Title("A name".into()));
        assert_eq!(directives[1], Directive::Compact(PIECE_MILLIS));
        assert_eq!(directives[2], Directive::Compact(500));
        assert_eq!(directives[4], Directive::Crash);
        assert_eq!(directives.len(), 10);
    }
}
