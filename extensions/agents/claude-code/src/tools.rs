//! The harness's own tools; with its provider, those kept that act on no machine, and the rest replaced by a tool set of the environment's executor.
//!
//! Two things about tools live here. The first is which tools a session is given: with the harness in the
//! environment, its own except the few that would make it act outside a turn; with the harness in its
//! provider's runtime, only those that touch no machine, and the environment's executor offered over MCP in
//! place of the others ([`tool_arguments`]). The second is what a call of a tool is, in Orch's terms: every
//! tool use becomes a [`ToolPayload`] of the most particular kind it can be (a command, a change of files, a
//! read, a search, a fetch, a sub-agent, one of Orch's tools, an MCP call), and is also kept as the harness
//! made it ([`RawCall`]), whole.

use orch_agent_contract::events::{
    FILE_CHANGE_DIFF_MAX_BYTES, FILE_SEARCH_MAX_MATCHES, FileChange, FileChangeKind,
    FileSearchMatch, FileSearchMode, OrchTool, OutputRef, RawCall, SubagentState, ToolPayload,
    WebSearchResult, sub_stream,
};
use orch_agent_contract::loopback::{EXEC_SERVER, ORCH_SERVER};
use orch_types::Json;

/// The tool that starts a sub-agent, by the name the harness's requests use.
pub const AGENT_TOOL: &str = "Agent";
/// The older name of that tool, which the harness still lists for programs that know it.
pub const AGENT_TOOL_ALIAS: &str = "Task";
/// The tool through which the harness asks a person a question.
pub const QUESTION_TOOL: &str = "AskUserQuestion";
/// The prefix of every tool that belongs to an MCP server.
pub const MCP_PREFIX: &str = "mcp__";

/// The tool sets of Orch's executor, by name.
pub mod tool_set {
    /// The harness's own shell and file tools, by name and input.
    pub const CLAUDE_CODE: &str = "claude-code";
    /// Orch's eight plain tools.
    pub const PLAIN: &str = "plain";
    /// No tools of the executor; the environment's MCP servers alone.
    pub const CUSTOM: &str = "custom";
}

/// What a plan tool does to the harness's list of things to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanOp {
    /// Adds a task.
    Create,
    /// Changes one.
    Update,
    /// Reads one; changes nothing.
    Get,
    /// Reads all; changes nothing.
    List,
    /// Replaces the whole list.
    Write,
}

/// What kind of thing a tool is, by its name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Family {
    /// A shell command.
    Command,
    /// An edit of a file by replacing text.
    Edit,
    /// A whole file written.
    Write,
    /// An edit of a notebook cell.
    NotebookEdit,
    /// A read of a file.
    Read,
    /// A search of files by name.
    Glob,
    /// A search of files by content.
    Grep,
    /// A search or a fetch on the web.
    WebSearch,
    /// A fetch of a page.
    WebFetch,
    /// Work handed to a sub-agent.
    Subagent,
    /// One of Orch's own tools.
    Orch(OrchTool),
    /// A tool of some other MCP server.
    Mcp {
        /// The server's name as the harness spells it.
        server: String,
        /// The tool's name.
        tool: String,
    },
    /// The harness's list of things to do.
    Plan(PlanOp),
    /// A question for the person.
    Question,
    /// Anything else, kept whole.
    Other,
}

/// What kind of tool `name` is. The tools of Orch's executor are known by the names its tool sets give
/// them, which for the set `claude-code` are the harness's own.
#[must_use]
pub fn family_of(name: &str) -> Family {
    if let Some(rest) = name.strip_prefix(MCP_PREFIX) {
        let Some((server, tool)) = rest.split_once("__") else {
            return Family::Other;
        };
        return match server {
            ORCH_SERVER => OrchTool::parse(tool).map_or_else(
                || Family::Mcp {
                    server: server.to_owned(),
                    tool: tool.to_owned(),
                },
                Family::Orch,
            ),
            EXEC_SERVER => exec_family(tool),
            _ => Family::Mcp {
                server: server.to_owned(),
                tool: tool.to_owned(),
            },
        };
    }
    match name {
        "Bash" | "PowerShell" => Family::Command,
        "Edit" => Family::Edit,
        "Write" => Family::Write,
        "NotebookEdit" => Family::NotebookEdit,
        "Read" => Family::Read,
        "Glob" => Family::Glob,
        "Grep" => Family::Grep,
        "WebSearch" => Family::WebSearch,
        "WebFetch" => Family::WebFetch,
        AGENT_TOOL | AGENT_TOOL_ALIAS => Family::Subagent,
        QUESTION_TOOL => Family::Question,
        "TaskCreate" => Family::Plan(PlanOp::Create),
        "TaskUpdate" => Family::Plan(PlanOp::Update),
        "TaskGet" => Family::Plan(PlanOp::Get),
        "TaskList" => Family::Plan(PlanOp::List),
        "TodoWrite" => Family::Plan(PlanOp::Write),
        _ => Family::Other,
    }
}

/// The kind of a tool of the executor's `exec` server, whatever tool set it comes from.
fn exec_family(tool: &str) -> Family {
    match tool {
        "Bash" | "shell" => Family::Command,
        "Edit" | "edit" => Family::Edit,
        "Write" | "write" => Family::Write,
        "Read" | "read" => Family::Read,
        "Glob" | "list" => Family::Glob,
        "Grep" | "search" => Family::Grep,
        _ => Family::Other,
    }
}

/// What came back from a tool: the text the model was given, whether the harness marked it as an error,
/// and the structured form the harness also reports for some tools.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ToolOutcome {
    /// The result as text; image blocks are named, not included.
    pub text: String,
    /// Whether the harness marked the result as an error.
    pub is_error: bool,
    /// The structured result (`tool_use_result`), where the harness gave one.
    pub structured: Option<Json>,
}

impl ToolOutcome {
    /// The outcome carried by a `tool_result` block and the structured result beside it.
    #[must_use]
    pub fn from_block(block: &Json, structured: Option<&Json>) -> Self {
        let text = match block.get("content") {
            Some(Json::String(text)) => text.clone(),
            Some(Json::Array(parts)) => parts
                .iter()
                .map(|part| match part.get("type").and_then(Json::as_str) {
                    Some("text") => part
                        .get("text")
                        .and_then(Json::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    Some("image") => format!(
                        "[image: {}]",
                        part.pointer("/source/media_type")
                            .and_then(Json::as_str)
                            .unwrap_or("unknown type")
                    ),
                    Some(other) => format!("[{other}]"),
                    None => String::new(),
                })
                .collect::<Vec<_>>()
                .join("\n"),
            _ => String::new(),
        };
        Self {
            text,
            is_error: block
                .get("is_error")
                .and_then(Json::as_bool)
                .unwrap_or(false),
            structured: structured.cloned(),
        }
    }
}

/// Where a call happened: the paths that are shown relative to it.
#[derive(Debug, Clone, Copy)]
pub struct ToolContext<'a> {
    /// The thread's directory.
    pub directory: &'a str,
    /// Where the agent starts, inside it.
    pub cwd: &'a str,
}

/// A path as it is shown: relative to the thread's directory where it lies in it, else as it is.
#[must_use]
pub fn shown_path(path: &str, directory: &str) -> String {
    let root = directory.trim_end_matches('/');
    match path.strip_prefix(root) {
        Some(rest) if !root.is_empty() && rest.starts_with('/') => {
            rest.trim_start_matches('/').to_owned()
        }
        _ => path.to_owned(),
    }
}

fn text_of<'a>(input: &'a Json, member: &str) -> Option<&'a str> {
    input.get(member).and_then(Json::as_str)
}

fn number_of(input: &Json, member: &str) -> Option<i64> {
    input.get(member).and_then(Json::as_i64)
}

/// The exit status of a command: absent from a result that went well (0), and the first line of one that
/// failed (`Exit code 3`).
#[must_use]
pub fn exit_code_of(outcome: &ToolOutcome) -> i64 {
    if !outcome.is_error {
        return 0;
    }
    outcome
        .text
        .strip_prefix("Exit code ")
        .and_then(|rest| {
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            digits.parse().ok()
        })
        .unwrap_or(1)
}

/// The line shown for a call.
#[must_use]
pub fn call_title(family: &Family, name: &str, input: &Json, ctx: &ToolContext<'_>) -> String {
    let first_line = |text: &str| -> String {
        let line = text.lines().next().unwrap_or_default();
        let mut shown: String = line.chars().take(120).collect();
        if line.chars().count() > 120 {
            shown.push('…');
        }
        shown
    };
    let path_of = |member: &str| {
        text_of(input, member)
            .map(|path| shown_path(path, ctx.directory))
            .unwrap_or_default()
    };
    match family {
        Family::Command => text_of(input, "description")
            .filter(|description| !description.is_empty())
            .map(first_line)
            .or_else(|| text_of(input, "command").map(first_line))
            .unwrap_or_else(|| name.to_owned()),
        Family::Edit => format!("Edit {}", path_of("file_path")),
        Family::Write => format!("Write {}", path_of("file_path")),
        Family::NotebookEdit => format!("Edit {}", path_of("notebook_path")),
        Family::Read => format!("Read {}", path_of("file_path")),
        Family::Glob => format!(
            "Find files {}",
            text_of(input, "pattern").unwrap_or_default()
        ),
        Family::Grep => format!(
            "Search for {}",
            text_of(input, "pattern").unwrap_or_default()
        ),
        Family::WebSearch => format!(
            "Search the web: {}",
            text_of(input, "query").unwrap_or_default()
        ),
        Family::WebFetch => format!("Fetch {}", text_of(input, "url").unwrap_or_default()),
        Family::Subagent => text_of(input, "description")
            .map(first_line)
            .unwrap_or_else(|| "Sub-agent".to_owned()),
        Family::Orch(tool) => tool.name().to_owned(),
        Family::Mcp { server, tool } => format!("{server}: {tool}"),
        Family::Plan(_) => "Plan".to_owned(),
        Family::Question => "Question".to_owned(),
        Family::Other => name.to_owned(),
    }
}

// ---------------------------------------------------------------------------------------------------------
// Differences

/// A change of one file as a unified diff with three lines of context, counted, and cut where it is longer
/// than a diff may be.
fn finish_diff(diff: String) -> (Option<String>, i64, i64, bool) {
    let (mut added, mut removed) = (0, 0);
    for line in diff.lines().skip(2) {
        if line.starts_with('+') {
            added += 1;
        } else if line.starts_with('-') {
            removed += 1;
        }
    }
    let most = usize::try_from(FILE_CHANGE_DIFF_MAX_BYTES).unwrap_or(usize::MAX);
    if diff.len() <= most {
        return (Some(diff), added, removed, false);
    }
    let mut cut = most;
    while !diff.is_char_boundary(cut) {
        cut -= 1;
    }
    let kept = diff[..cut].rfind('\n').map_or(cut, |end| end + 1);
    (Some(diff[..kept].to_owned()), added, removed, true)
}

/// The unified diff from the old text to the new.
#[must_use]
pub fn unified_diff(path: &str, old: &str, new: &str, created: bool) -> String {
    let old_name = if created {
        "/dev/null".to_owned()
    } else {
        format!("a/{path}")
    };
    similar::TextDiff::from_lines(old, new)
        .unified_diff()
        .context_radius(3)
        .header(&old_name, &format!("b/{path}"))
        .to_string()
}

/// The unified diff from the hunks the harness reports, which already carry their context.
#[must_use]
pub fn diff_from_hunks(path: &str, hunks: &Json, created: bool) -> Option<String> {
    let hunks = hunks.as_array()?;
    if hunks.is_empty() {
        return None;
    }
    let old_name = if created {
        "/dev/null".to_owned()
    } else {
        format!("a/{path}")
    };
    let mut diff = format!("--- {old_name}\n+++ b/{path}\n");
    for hunk in hunks {
        let number = |member: &str| hunk.get(member).and_then(Json::as_i64);
        diff.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            number("oldStart")?,
            number("oldLines")?,
            number("newStart")?,
            number("newLines")?
        ));
        for line in hunk.get("lines")?.as_array()? {
            diff.push_str(line.as_str()?);
            diff.push('\n');
        }
    }
    Some(diff)
}

fn change_of(path: &str, kind: FileChangeKind, diff: Option<String>, binary: bool) -> FileChange {
    let (diff, added, removed, truncated) = match diff {
        Some(diff) => finish_diff(diff),
        None => (None, 0, 0, false),
    };
    FileChange {
        path: path.to_owned(),
        change: kind,
        old_path: None,
        diff,
        added,
        removed,
        binary,
        truncated,
    }
}

/// The changes a call of an editing tool makes, from what it was asked and, once it ran, what the harness
/// reports it did.
fn file_changes(
    family: &Family,
    input: &Json,
    outcome: Option<&ToolOutcome>,
    ctx: &ToolContext<'_>,
) -> Vec<FileChange> {
    let structured = outcome.and_then(|outcome| outcome.structured.as_ref());
    let failed = outcome.is_some_and(|outcome| outcome.is_error);
    match family {
        Family::Edit => {
            let Some(path) = text_of(input, "file_path") else {
                return Vec::new();
            };
            let shown = shown_path(path, ctx.directory);
            let old = text_of(input, "old_string").unwrap_or_default();
            let new = text_of(input, "new_string").unwrap_or_default();
            let diff = structured
                .filter(|_| !failed)
                .and_then(|result| result.get("structuredPatch"))
                .and_then(|hunks| diff_from_hunks(&shown, hunks, false))
                .unwrap_or_else(|| unified_diff(&shown, old, new, false));
            vec![change_of(&shown, FileChangeKind::Modify, Some(diff), false)]
        }
        Family::Write => {
            let Some(path) = text_of(input, "file_path") else {
                return Vec::new();
            };
            let shown = shown_path(path, ctx.directory);
            let content = text_of(input, "content").unwrap_or_default();
            let created = structured
                .and_then(|result| result.get("type"))
                .and_then(Json::as_str)
                .is_none_or(|kind| kind == "create");
            let original = structured
                .and_then(|result| result.get("originalFile"))
                .and_then(Json::as_str);
            let diff = match (created, original) {
                (false, Some(original)) => unified_diff(&shown, original, content, false),
                (false, None) => unified_diff(&shown, "", content, false),
                (true, _) => unified_diff(&shown, "", content, true),
            };
            let kind = if created {
                FileChangeKind::Add
            } else {
                FileChangeKind::Modify
            };
            vec![change_of(&shown, kind, Some(diff), false)]
        }
        Family::NotebookEdit => {
            let Some(path) = text_of(input, "notebook_path") else {
                return Vec::new();
            };
            let shown = shown_path(path, ctx.directory);
            let new = text_of(input, "new_source").unwrap_or_default();
            let old = structured
                .and_then(|result| result.get("old_source"))
                .and_then(Json::as_str)
                .unwrap_or_default();
            vec![change_of(
                &shown,
                FileChangeKind::Modify,
                Some(unified_diff(&shown, old, new, false)),
                false,
            )]
        }
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------------------------------------------------
// Searches

fn search_matches(
    family: &Family,
    outcome: &ToolOutcome,
) -> (Vec<FileSearchMatch>, Option<i64>, bool) {
    let limit = usize::try_from(FILE_SEARCH_MAX_MATCHES).unwrap_or(usize::MAX);
    let mut found: Vec<FileSearchMatch> = Vec::new();
    let names = outcome
        .structured
        .as_ref()
        .and_then(|result| result.get("filenames"))
        .and_then(Json::as_array);
    if let Some(names) = names {
        found.extend(
            names
                .iter()
                .filter_map(Json::as_str)
                .map(|path| FileSearchMatch {
                    path: path.to_owned(),
                    line: None,
                    text: None,
                }),
        );
    } else if !outcome.is_error {
        for line in outcome.text.lines().filter(|line| !line.trim().is_empty()) {
            if matches!(family, Family::Grep) {
                let mut parts = line.splitn(3, ':');
                let path = parts.next().unwrap_or_default();
                let number = parts.next().and_then(|n| n.parse::<i64>().ok());
                let text = parts.next();
                if let (Some(number), Some(text)) = (number, text) {
                    found.push(FileSearchMatch {
                        path: path.to_owned(),
                        line: Some(number),
                        text: Some(text.to_owned()),
                    });
                    continue;
                }
            }
            found.push(FileSearchMatch {
                path: line.to_owned(),
                line: None,
                text: None,
            });
        }
    }
    let total = i64::try_from(found.len()).ok();
    let truncated = found.len() > limit;
    found.truncate(limit);
    (found, total, truncated)
}

// ---------------------------------------------------------------------------------------------------------
// The payload

fn json_or_text(text: &str) -> Json {
    serde_json::from_str(text).unwrap_or_else(|_| Json::String(text.to_owned()))
}

/// The call as Orch understands it.
#[must_use]
pub fn tool_payload(
    family: &Family,
    name: &str,
    id: &str,
    input: &Json,
    outcome: Option<&ToolOutcome>,
    ctx: &ToolContext<'_>,
) -> ToolPayload {
    let text = outcome.map(|outcome| outcome.text.as_str());
    match family {
        Family::Command => ToolPayload::Command {
            command: text_of(input, "command").unwrap_or_default().to_owned(),
            cwd: ctx.cwd.to_owned(),
            exit_code: outcome.map(exit_code_of),
            output: OutputRef::of(
                text.map(|text| {
                    if outcome.is_some_and(|outcome| outcome.is_error) {
                        text.strip_prefix("Exit code ")
                            .and_then(|rest| rest.split_once('\n'))
                            .map_or(text, |(_, output)| output)
                    } else {
                        text
                    }
                })
                .unwrap_or_default(),
            ),
            process_id: outcome
                .and_then(|outcome| outcome.structured.as_ref())
                .and_then(|result| result.get("backgroundTaskId"))
                .and_then(Json::as_str)
                .map(str::to_owned),
            background: input
                .get("run_in_background")
                .and_then(Json::as_bool)
                .unwrap_or(false),
            actions: Vec::new(),
        },
        Family::Edit | Family::Write | Family::NotebookEdit => ToolPayload::FileChange {
            changes: file_changes(family, input, outcome, ctx),
        },
        Family::Read => {
            let path = text_of(input, "file_path").unwrap_or_default();
            let file = outcome
                .and_then(|outcome| outcome.structured.as_ref())
                .and_then(|result| result.get("file"));
            let image = outcome
                .and_then(|outcome| outcome.structured.as_ref())
                .and_then(|result| result.get("type"))
                .and_then(Json::as_str)
                == Some("image");
            let start = file
                .and_then(|file| file.get("startLine"))
                .and_then(Json::as_i64)
                .or_else(|| number_of(input, "offset"));
            let count = file
                .and_then(|file| file.get("numLines"))
                .and_then(Json::as_i64);
            ToolPayload::FileRead {
                path: shown_path(path, ctx.directory),
                start_line: start,
                end_line: match (start, count) {
                    (Some(start), Some(count)) if count > 0 => Some(start + count - 1),
                    _ => None,
                },
                total_lines: file
                    .and_then(|file| file.get("totalLines"))
                    .and_then(Json::as_i64),
                content: OutputRef::of(
                    file.and_then(|file| file.get("content"))
                        .and_then(Json::as_str)
                        .or(if image { Some("") } else { text })
                        .unwrap_or_default(),
                ),
                media_type: image
                    .then(|| {
                        file.and_then(|file| file.get("type"))
                            .and_then(Json::as_str)
                            .map(str::to_owned)
                    })
                    .flatten(),
            }
        }
        Family::Glob | Family::Grep => {
            let (matches, total, truncated) = outcome
                .map_or((Vec::new(), None, false), |outcome| {
                    search_matches(family, outcome)
                });
            ToolPayload::FileSearch {
                mode: if matches!(family, Family::Glob) {
                    FileSearchMode::Name
                } else {
                    FileSearchMode::Content
                },
                query: text_of(input, "pattern")
                    .or_else(|| text_of(input, "query"))
                    .unwrap_or_default()
                    .to_owned(),
                path: text_of(input, "path").map(|path| shown_path(path, ctx.directory)),
                matches,
                total,
                truncated,
            }
        }
        Family::WebSearch => {
            let mut results = Vec::new();
            if let Some(groups) = outcome
                .and_then(|outcome| outcome.structured.as_ref())
                .and_then(|result| result.get("results"))
                .and_then(Json::as_array)
            {
                for group in groups {
                    for hit in group
                        .get("content")
                        .and_then(Json::as_array)
                        .into_iter()
                        .flatten()
                    {
                        if let (Some(title), Some(url)) = (
                            hit.get("title").and_then(Json::as_str),
                            hit.get("url").and_then(Json::as_str),
                        ) {
                            results.push(WebSearchResult {
                                title: title.to_owned(),
                                url: url.to_owned(),
                            });
                        }
                    }
                }
            }
            ToolPayload::WebSearch {
                query: text_of(input, "query").unwrap_or_default().to_owned(),
                results,
            }
        }
        Family::WebFetch => {
            let structured = outcome.and_then(|outcome| outcome.structured.as_ref());
            ToolPayload::WebFetch {
                url: text_of(input, "url").unwrap_or_default().to_owned(),
                title: None,
                status: structured
                    .and_then(|result| result.get("code"))
                    .and_then(Json::as_u64)
                    .and_then(|code| u16::try_from(code).ok()),
                content: OutputRef::of(
                    structured
                        .and_then(|result| result.get("result"))
                        .and_then(Json::as_str)
                        .or(text)
                        .unwrap_or_default(),
                ),
            }
        }
        Family::Subagent => ToolPayload::Subagent {
            child_stream: sub_stream(id),
            name: text_of(input, "name")
                .or_else(|| text_of(input, "description"))
                .map(str::to_owned),
            role: text_of(input, "subagent_type").map(str::to_owned),
            prompt: text_of(input, "prompt").map(str::to_owned),
            state: match outcome {
                None => SubagentState::Running,
                Some(outcome) if outcome.is_error => SubagentState::Failed,
                Some(_) => SubagentState::Completed,
            },
            // The model the call asks for (an alias such as `haiku`); without one the sub-agent runs on its
            // definition's model or the session's, which the turn's use by model names.
            model: text_of(input, "model").map(str::to_owned),
        },
        Family::Orch(tool) => ToolPayload::Orch {
            tool: *tool,
            arguments: input.clone(),
            result: outcome
                .filter(|outcome| !outcome.is_error)
                .map(|outcome| json_or_text(&outcome.text)),
        },
        Family::Mcp { server, tool } => ToolPayload::Mcp {
            server: server.clone(),
            tool: tool.clone(),
            arguments: input.clone(),
            result: outcome
                .filter(|outcome| !outcome.is_error)
                .map(|outcome| json_or_text(&outcome.text)),
            error: outcome
                .filter(|outcome| outcome.is_error)
                .map(|outcome| outcome.text.clone()),
        },
        Family::Plan(_) | Family::Question | Family::Other => ToolPayload::Other {
            name: name.to_owned(),
            arguments: Some(input.clone()),
            result: outcome.map(|outcome| {
                outcome
                    .structured
                    .clone()
                    .filter(|structured| !structured.is_string())
                    .unwrap_or_else(|| json_or_text(&outcome.text))
            }),
        },
    }
}

/// The call as the harness made it: its name, its arguments as JSON text, and what came back as text.
#[must_use]
pub fn raw_call(name: &str, input: &Json, outcome: Option<&ToolOutcome>) -> RawCall {
    RawCall {
        name: name.to_owned(),
        input: OutputRef::of(&input.to_string()),
        output: outcome.map(|outcome| OutputRef::of(&outcome.text)),
        is_error: outcome.is_some_and(|outcome| outcome.is_error),
    }
}

// ---------------------------------------------------------------------------------------------------------
// The tools a session is given

/// The built-in tools that are never offered. Each would make the harness act outside a turn of Orch:
/// scheduled prompts and wake-ups start turns nobody asked for, a message to a background agent has no
/// agent to reach, and a remote or push trigger leaves the machine. Entering plan mode is a person's
/// switch, never the agent's, and worktrees change what the harness is allowed to do in ways the approval
/// and sandbox modes of Orch do not know. Leaving plan mode is offered: it is how a plan is proposed.
pub const NEVER_OFFERED: [&str; 13] = [
    "CronCreate",
    "CronDelete",
    "CronList",
    "ScheduleWakeup",
    "RemoteTrigger",
    "PushNotification",
    "Monitor",
    "Workflow",
    "SendMessage",
    "ListAgents",
    "EnterPlanMode",
    "EnterWorktree",
    "ExitWorktree",
];

/// The built-in tools that act on no machine: starting a sub-agent, asking a person, proposing a plan, the
/// list of things to do. These stay when the harness works through Orch's executor, because they need
/// nothing of the machine the harness is on.
pub const MACHINE_FREE: [&str; 9] = [
    AGENT_TOOL,
    QUESTION_TOOL,
    "ExitPlanMode",
    "TaskCreate",
    "TaskGet",
    "TaskList",
    "TaskUpdate",
    "TodoWrite",
    "Skill",
];

/// The arguments that give a session its tools. With the harness in the environment it keeps all its own
/// except [`NEVER_OFFERED`]. With the harness in its provider's runtime only [`MACHINE_FREE`] stay, so that
/// everything that touches a machine goes through the executor's `exec` server.
#[must_use]
pub fn tool_arguments(through_executor: bool) -> Vec<String> {
    if through_executor {
        vec!["--tools".to_owned(), MACHINE_FREE.join(",")]
    } else {
        let mut arguments = vec!["--disallowed-tools".to_owned()];
        arguments.extend(NEVER_OFFERED.iter().map(|tool| (*tool).to_owned()));
        arguments
    }
}

/// Whether a tool belongs to the harness's own set of this kind (used to tell its calls from an MCP
/// server's of the same look).
#[must_use]
pub fn is_exec_tool(name: &str) -> bool {
    name.strip_prefix(MCP_PREFIX)
        .and_then(|rest| rest.split_once("__"))
        .is_some_and(|(server, _)| server == EXEC_SERVER)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ctx() -> ToolContext<'static> {
        ToolContext {
            directory: "/workspace/web",
            cwd: "/workspace/web/api",
        }
    }

    #[test]
    fn a_tool_is_known_by_its_own_name_and_by_the_name_the_executor_gives_it() {
        assert_eq!(family_of("Bash"), Family::Command);
        assert_eq!(family_of("mcp__exec__Bash"), Family::Command);
        assert_eq!(family_of("mcp__exec__shell"), Family::Command);
        assert_eq!(family_of("mcp__exec__edit"), Family::Edit);
        assert_eq!(family_of("mcp__exec__write"), Family::Write);
        assert_eq!(family_of("mcp__exec__read"), Family::Read);
        assert_eq!(family_of("mcp__exec__search"), Family::Grep);
        assert_eq!(family_of("mcp__exec__list"), Family::Glob);
        assert_eq!(family_of("Agent"), Family::Subagent);
        assert_eq!(family_of("Task"), Family::Subagent);
        assert_eq!(
            family_of("mcp__orch__announce"),
            Family::Orch(OrchTool::Announce)
        );
        assert_eq!(
            family_of("mcp__github__create_issue"),
            Family::Mcp {
                server: "github".into(),
                tool: "create_issue".into()
            }
        );
        assert_eq!(family_of("TaskCreate"), Family::Plan(PlanOp::Create));
        assert_eq!(family_of("SomethingNew"), Family::Other);
        assert_eq!(family_of("mcp__broken"), Family::Other);
    }

    #[test]
    fn a_command_that_failed_has_its_exit_status_from_the_first_line_of_what_came_back() {
        let failed = ToolOutcome {
            text: "Exit code 3\nout-line\nerr-line".into(),
            is_error: true,
            structured: None,
        };
        assert_eq!(exit_code_of(&failed), 3);
        let payload = tool_payload(
            &Family::Command,
            "Bash",
            "t1",
            &json!({ "command": "exit 3" }),
            Some(&failed),
            &ctx(),
        );
        let ToolPayload::Command {
            exit_code,
            output,
            cwd,
            ..
        } = payload
        else {
            panic!("a command");
        };
        assert_eq!(exit_code, Some(3));
        assert_eq!(output.inline, "out-line\nerr-line");
        assert_eq!(cwd, "/workspace/web/api");
        let fine = ToolOutcome {
            text: "ok".into(),
            ..ToolOutcome::default()
        };
        assert_eq!(exit_code_of(&fine), 0);
    }

    #[test]
    fn an_edit_is_shown_as_the_difference_the_harness_reports() {
        let outcome = ToolOutcome {
            text: "The file has been updated successfully.".into(),
            is_error: false,
            structured: Some(json!({
                "filePath": "/workspace/web/existing.txt",
                "structuredPatch": [{ "oldStart": 1, "oldLines": 3, "newStart": 1, "newLines": 3,
                    "lines": [" line one", "-line two", "+line 2 changed", " line three"] }]
            })),
        };
        let payload = tool_payload(
            &Family::Edit,
            "Edit",
            "t1",
            &json!({ "file_path": "/workspace/web/existing.txt", "old_string": "line two", "new_string": "line 2 changed" }),
            Some(&outcome),
            &ctx(),
        );
        let ToolPayload::FileChange { changes } = payload else {
            panic!("a change of files");
        };
        let change = &changes[0];
        assert_eq!(change.path, "existing.txt");
        assert_eq!(change.change, FileChangeKind::Modify);
        assert_eq!((change.added, change.removed), (1, 1));
        assert_eq!(
            change.diff.as_deref(),
            Some(
                "--- a/existing.txt\n+++ b/existing.txt\n@@ -1,3 +1,3 @@\n line one\n-line two\n+line 2 changed\n line three\n"
            )
        );
    }

    #[test]
    fn a_file_that_was_written_is_all_added_and_one_that_was_replaced_is_a_difference() {
        let created = tool_payload(
            &Family::Write,
            "Write",
            "t1",
            &json!({ "file_path": "/workspace/web/new.txt", "content": "alpha\nbeta\n" }),
            Some(&ToolOutcome {
                text: "created".into(),
                is_error: false,
                structured: Some(json!({ "type": "create", "originalFile": null })),
            }),
            &ctx(),
        );
        let ToolPayload::FileChange { changes } = created else {
            panic!("a change of files");
        };
        assert_eq!(changes[0].change, FileChangeKind::Add);
        assert_eq!((changes[0].added, changes[0].removed), (2, 0));
        assert!(
            changes[0]
                .diff
                .as_deref()
                .unwrap()
                .starts_with("--- /dev/null\n+++ b/new.txt\n")
        );

        let replaced = tool_payload(
            &Family::Write,
            "Write",
            "t1",
            &json!({ "file_path": "/workspace/web/a.txt", "content": "one\nTWO\n" }),
            Some(&ToolOutcome {
                text: "updated".into(),
                is_error: false,
                structured: Some(json!({ "type": "update", "originalFile": "one\ntwo\n" })),
            }),
            &ctx(),
        );
        let ToolPayload::FileChange { changes } = replaced else {
            panic!("a change of files");
        };
        assert_eq!(changes[0].change, FileChangeKind::Modify);
        assert_eq!((changes[0].added, changes[0].removed), (1, 1));
    }

    #[test]
    fn a_change_that_is_not_done_yet_is_shown_from_what_was_asked() {
        let payload = tool_payload(
            &Family::Edit,
            "Edit",
            "t1",
            &json!({ "file_path": "/elsewhere/x.txt", "old_string": "a", "new_string": "b" }),
            None,
            &ctx(),
        );
        let ToolPayload::FileChange { changes } = payload else {
            panic!("a change of files");
        };
        assert_eq!(
            changes[0].path, "/elsewhere/x.txt",
            "outside the directory the path stays whole"
        );
        assert_eq!((changes[0].added, changes[0].removed), (1, 1));
    }

    #[test]
    fn a_read_reports_the_lines_it_returned() {
        let payload = tool_payload(
            &Family::Read,
            "Read",
            "t1",
            &json!({ "file_path": "/workspace/web/src/main.rs", "offset": 10, "limit": 5 }),
            Some(&ToolOutcome {
                text: "10\tfn main() {}".into(),
                is_error: false,
                structured: Some(json!({ "type": "text", "file": {
                    "filePath": "/workspace/web/src/main.rs", "content": "fn main() {}",
                    "numLines": 5, "startLine": 10, "totalLines": 120 } })),
            }),
            &ctx(),
        );
        let ToolPayload::FileRead {
            path,
            start_line,
            end_line,
            total_lines,
            content,
            media_type,
        } = payload
        else {
            panic!("a read");
        };
        assert_eq!(path, "src/main.rs");
        assert_eq!(
            (start_line, end_line, total_lines),
            (Some(10), Some(14), Some(120))
        );
        assert_eq!(content.inline, "fn main() {}");
        assert_eq!(media_type, None);
    }

    #[test]
    fn a_tool_of_an_mcp_server_is_a_call_with_its_arguments_and_its_result_or_its_error() {
        let family = family_of("mcp__github__create_issue");
        let done = tool_payload(
            &family,
            "mcp__github__create_issue",
            "t1",
            &json!({ "title": "x" }),
            Some(&ToolOutcome {
                text: r#"{"number":7}"#.into(),
                ..ToolOutcome::default()
            }),
            &ctx(),
        );
        assert!(
            matches!(done, ToolPayload::Mcp { result: Some(ref r), error: None, .. } if r["number"] == 7)
        );
        let failed = tool_payload(
            &family,
            "mcp__github__create_issue",
            "t1",
            &json!({}),
            Some(&ToolOutcome {
                text: "denied".into(),
                is_error: true,
                structured: None,
            }),
            &ctx(),
        );
        assert!(
            matches!(failed, ToolPayload::Mcp { result: None, error: Some(ref e), .. } if e == "denied")
        );
    }

    #[test]
    fn the_raw_form_keeps_the_name_the_arguments_and_the_result_whole() {
        let raw = raw_call(
            "Bash",
            &json!({ "command": "ls" }),
            Some(&ToolOutcome {
                text: "a\nb".into(),
                is_error: false,
                structured: None,
            }),
        );
        assert_eq!(raw.name, "Bash");
        assert_eq!(raw.input.inline, r#"{"command":"ls"}"#);
        assert_eq!(raw.output.unwrap().inline, "a\nb");
        assert!(!raw.is_error);
    }

    #[test]
    fn a_session_in_the_environment_keeps_its_tools_less_those_that_start_turns_and_one_with_its_provider_keeps_only_those_that_touch_no_machine()
     {
        let own = tool_arguments(false);
        assert_eq!(own[0], "--disallowed-tools");
        assert!(
            own.contains(&"CronCreate".to_owned()) && own.contains(&"ScheduleWakeup".to_owned())
        );
        assert!(!own.contains(&"Bash".to_owned()));
        let through = tool_arguments(true);
        assert_eq!(through[0], "--tools");
        let kept: Vec<&str> = through[1].split(',').collect();
        assert!(kept.contains(&"Agent") && !kept.contains(&"Bash") && !kept.contains(&"Write"));
    }

    #[test]
    fn a_path_inside_the_thread_directory_is_shown_without_it() {
        assert_eq!(shown_path("/w/web/a/b.rs", "/w/web"), "a/b.rs");
        assert_eq!(shown_path("/w/web", "/w/web"), "/w/web");
        assert_eq!(shown_path("/w/webby/x", "/w/web"), "/w/webby/x");
        assert_eq!(shown_path("rel.txt", "/w/web/"), "rel.txt");
    }
}
