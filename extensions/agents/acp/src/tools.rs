//! What a tool call of an ACP agent is understood as: a command, a change of files, a read, a search, an MCP call, one of Orch's own tools.
//!
//! The protocol gives a tool call a `kind` (read, edit, execute, search, fetch, and more), a title, its raw input
//! and what it produced. Agents differ in how much of that they fill: some give every field, some (DeepSeek
//! Harness's own protocol surface) give every call the kind `other` and its tool's name as the title. So a call is
//! understood from three sources, in this order: the program's own table of tools (`ToolRule`s of its manifest,
//! by name), the kind the protocol says, and, for the tools of MCP servers, the server's name the session was
//! given. A call that none of them places is kept whole as a tool of kind `other`, with its raw form beside, so
//! that nothing is lost for not being understood.

use orch_agent_contract::events::{
    FILE_CHANGE_DIFF_MAX_BYTES, FileChange, FileChangeKind, FileSearchMode, OrchTool, OutputRef,
    RawCall, SubagentState, ToolPayload, WebSearchResult, sub_stream,
};
use orch_types::Json;

use crate::program::{AcpProgram, ToolFamily};

/// How far a call has come, as the protocol says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Announced, not yet running (the input is still arriving, or it waits for approval).
    Pending,
    /// Running.
    InProgress,
    /// Ended well.
    Completed,
    /// Ended badly.
    Failed,
}

impl Status {
    /// Reads the protocol's word for it.
    #[must_use]
    pub fn read(word: &str) -> Option<Self> {
        match word {
            "pending" => Some(Self::Pending),
            "in_progress" => Some(Self::InProgress),
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }

    /// Whether the call is over.
    #[must_use]
    pub const fn ended(self) -> bool {
        matches!(self, Self::Completed | Self::Failed)
    }
}

/// What a call is understood as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Understood {
    /// One of the kinds of the program's table, or of the protocol's.
    Family(ToolFamily),
    /// A call of one of Orch's own tools, through the `orch` server.
    Orch(OrchTool),
    /// A call of a tool of an MCP server.
    Mcp {
        /// The server.
        server: String,
        /// The tool.
        tool: String,
    },
    /// A call of a tool of the executor's `exec` server, understood as the family its name says.
    Exec(ToolFamily),
    /// Nothing the adapter knows.
    Other,
}

/// What the adapter keeps of one tool call while it runs.
#[derive(Debug, Clone)]
pub struct ToolView {
    /// The call's id.
    pub id: String,
    /// The tool's own name, where the protocol gave one; else the title.
    pub name: String,
    /// The title the agent gave.
    pub title: String,
    /// The protocol's kind.
    pub kind: String,
    /// What it is understood as.
    pub understood: Understood,
    /// How far it has come.
    pub status: Status,
    /// The input, raw.
    pub raw_input: Json,
    /// The output, raw, where the agent gave one.
    pub raw_output: Option<Json>,
    /// The content blocks the agent gave.
    pub content: Vec<Json>,
    /// The files it concerns.
    pub locations: Vec<Json>,
}

/// Where the session works, and the servers it has; what a call is read against.
#[derive(Debug, Clone, Copy)]
pub struct Place<'a> {
    /// The manifest.
    pub program: &'a AcpProgram,
    /// The thread's directory.
    pub directory: &'a str,
    /// Where the agent starts.
    pub cwd: &'a str,
    /// The names of the MCP servers the session was given.
    pub servers: &'a [String],
}

fn text_of<'a>(input: &'a Json, member: &str) -> Option<&'a str> {
    input.get(member).and_then(Json::as_str)
}

fn number_of(input: &Json, member: &str) -> Option<i64> {
    input.get(member).and_then(Json::as_i64)
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

/// Splits the name of an MCP tool as an agent writes it into the server and the tool, given the servers the
/// session has: `mcp__server__tool`, `server__tool`, `server.tool`, `server/tool`, `server:tool` or `server_tool`.
#[must_use]
pub fn split_mcp(name: &str, servers: &[String]) -> Option<(String, String)> {
    let bare = name.strip_prefix("mcp__").unwrap_or(name);
    // The longest server name first, so that `tmux_ui` is never mistaken for `tmux`.
    let mut ordered: Vec<&String> = servers.iter().collect();
    ordered.sort_by_key(|server| std::cmp::Reverse(server.len()));
    for server in ordered {
        for separator in ["__", ".", "/", ":", "_"] {
            if let Some(tool) = bare
                .strip_prefix(server.as_str())
                .and_then(|rest| rest.strip_prefix(separator))
                .filter(|tool| !tool.is_empty())
            {
                return Some((server.clone(), tool.to_owned()));
            }
        }
    }
    if let Some(rest) = name.strip_prefix("mcp__")
        && let Some((server, tool)) = rest.split_once("__")
    {
        return Some((server.to_owned(), tool.to_owned()));
    }
    None
}

/// The family of a tool of the executor, whatever its tool set.
fn exec_family(tool: &str) -> Option<ToolFamily> {
    Some(match tool {
        "Bash" | "shell" => ToolFamily::Command,
        "Edit" | "edit" => ToolFamily::Edit,
        "Write" | "write" => ToolFamily::Write,
        "Read" | "read" => ToolFamily::Read,
        "Glob" | "list" => ToolFamily::Glob,
        "Grep" | "search" => ToolFamily::Grep,
        _ => return None,
    })
}

/// What a call is understood as: from the manifest's table by the tool's name, then from the servers' names, then
/// from the protocol's kind.
#[must_use]
pub fn understand(place: &Place<'_>, name: &str, title: &str, kind: &str) -> Understood {
    let wanted = [name, title];
    for rule in &place.program.tools {
        let bare = |candidate: &str| candidate.to_ascii_lowercase();
        if rule.names.iter().any(|known| {
            wanted
                .iter()
                .any(|candidate| !candidate.is_empty() && bare(candidate) == bare(known))
        }) {
            return Understood::Family(rule.family);
        }
    }
    for candidate in wanted {
        if candidate.is_empty() {
            continue;
        }
        if let Some((server, tool)) = split_mcp(candidate, place.servers) {
            return match server.as_str() {
                orch_agent_contract::loopback::ORCH_SERVER => OrchTool::parse(&tool).map_or(
                    Understood::Mcp {
                        server: server.clone(),
                        tool: tool.clone(),
                    },
                    Understood::Orch,
                ),
                orch_agent_contract::loopback::EXEC_SERVER => {
                    exec_family(&tool).map_or(Understood::Mcp { server, tool }, Understood::Exec)
                }
                _ => Understood::Mcp { server, tool },
            };
        }
    }
    match kind {
        "execute" => Understood::Family(ToolFamily::Command),
        "read" => Understood::Family(ToolFamily::Read),
        "edit" => Understood::Family(ToolFamily::Edit),
        "search" => Understood::Family(ToolFamily::Grep),
        "fetch" => Understood::Family(ToolFamily::WebFetch),
        _ => Understood::Other,
    }
}

/// The text the agent gave as the tool's output: every text block, in order.
#[must_use]
pub fn content_output(content: &[Json]) -> String {
    let mut out = String::new();
    for block in content {
        let text = match block.get("type").and_then(Json::as_str) {
            Some("content") => block.get("content").and_then(crate::wire::content_text),
            _ => None,
        };
        if let Some(text) = text {
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(&text);
        }
    }
    out
}

/// The text of a raw output, where it is text, or a text member of it.
#[must_use]
pub fn raw_output_text(raw: &Json) -> Option<String> {
    match raw {
        Json::String(text) => Some(text.clone()),
        Json::Object(map) => ["output", "stdout", "text", "content", "result"]
            .iter()
            .find_map(|member| map.get(*member).and_then(Json::as_str))
            .map(str::to_owned),
        _ => None,
    }
}

/// What the call produced as text: the content blocks, else the raw output.
#[must_use]
pub fn output_of(view: &ToolView) -> String {
    let from_content = content_output(&view.content);
    if !from_content.is_empty() {
        return from_content;
    }
    view.raw_output
        .as_ref()
        .and_then(raw_output_text)
        .unwrap_or_default()
}

/// The exit code a result states, by the marker some agents write (`[exit code: 3]`) or by a member of the raw
/// output, and otherwise by how the call ended.
#[must_use]
pub fn exit_code_of(view: &ToolView, marker: Option<&str>) -> Option<i64> {
    if let Some(raw) = &view.raw_output {
        for member in ["exit_code", "exitCode", "code"] {
            if let Some(code) = raw.get(member).and_then(Json::as_i64) {
                return Some(code);
            }
        }
    }
    if let Some(marker) = marker.filter(|marker| !marker.is_empty()) {
        let text = output_of(view);
        if let Some(start) = text.rfind(marker) {
            let digits: String = text[start + marker.len()..]
                .trim_start()
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '-')
                .collect();
            if let Ok(code) = digits.parse() {
                return Some(code);
            }
        }
    }
    match view.status {
        Status::Completed => Some(0),
        Status::Failed => Some(1),
        _ => None,
    }
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

/// A diff, counted, and cut where it is longer than a diff may be.
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

fn change(path: &str, kind: FileChangeKind, diff: Option<String>) -> FileChange {
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
        binary: false,
        truncated,
    }
}

/// The changes a call makes, from the diffs the protocol carries, else from the input of the tool.
#[must_use]
pub fn file_changes(view: &ToolView, family: ToolFamily, place: &Place<'_>) -> Vec<FileChange> {
    let mut out = Vec::new();
    for block in &view.content {
        if block.get("type").and_then(Json::as_str) != Some("diff") {
            continue;
        }
        let Some(path) = block.get("path").and_then(Json::as_str) else {
            continue;
        };
        let new = block
            .get("newText")
            .and_then(Json::as_str)
            .unwrap_or_default();
        let old = block.get("oldText").and_then(Json::as_str);
        let shown = shown_path(path, place.directory);
        let diff = unified_diff(&shown, old.unwrap_or_default(), new, old.is_none());
        out.push(change(
            &shown,
            if old.is_none() {
                FileChangeKind::Add
            } else {
                FileChangeKind::Modify
            },
            Some(diff),
        ));
    }
    if !out.is_empty() {
        return out;
    }
    let argument = place
        .program
        .tools
        .iter()
        .find(|rule| rule.family == family && rule.argument.is_some())
        .and_then(|rule| rule.argument.as_deref());
    let path = argument
        .and_then(|member| text_of(&view.raw_input, member))
        .or_else(|| {
            ["file_path", "path", "filePath"]
                .iter()
                .find_map(|member| text_of(&view.raw_input, member))
        })
        .or_else(|| {
            view.locations
                .first()
                .and_then(|location| location.get("path"))
                .and_then(Json::as_str)
        });
    let Some(path) = path else {
        return out;
    };
    let shown = shown_path(path, place.directory);
    let input = &view.raw_input;
    match family {
        ToolFamily::Write => {
            let content = ["content", "text", "contents"]
                .iter()
                .find_map(|member| text_of(input, member))
                .unwrap_or_default();
            let diff = unified_diff(&shown, "", content, true);
            out.push(change(&shown, FileChangeKind::Add, Some(diff)));
        }
        _ => {
            let old = ["old_string", "old_str", "oldText", "old"]
                .iter()
                .find_map(|member| text_of(input, member));
            let new = ["new_string", "new_str", "newText", "new"]
                .iter()
                .find_map(|member| text_of(input, member));
            let diff = match (old, new) {
                (Some(old), Some(new)) => Some(unified_diff(&shown, old, new, false)),
                _ => None,
            };
            out.push(change(&shown, FileChangeKind::Modify, diff));
        }
    }
    out
}

fn first_line(text: &str) -> String {
    let line = text.lines().next().unwrap_or_default();
    let mut shown: String = line.chars().take(120).collect();
    if line.chars().count() > 120 {
        shown.push('…');
    }
    shown
}

/// The line shown for a call.
#[must_use]
pub fn call_title(view: &ToolView, place: &Place<'_>) -> String {
    let input = &view.raw_input;
    let path_of = |members: &[&str]| {
        members
            .iter()
            .find_map(|member| text_of(input, member))
            .map(|path| shown_path(path, place.directory))
            .unwrap_or_default()
    };
    let fallback = if view.title.is_empty() {
        view.name.clone()
    } else {
        view.title.clone()
    };
    match &view.understood {
        Understood::Family(family) | Understood::Exec(family) => match family {
            ToolFamily::Command => text_of(input, "description")
                .filter(|description| !description.is_empty())
                .map(first_line)
                .or_else(|| text_of(input, "command").map(first_line))
                .unwrap_or(fallback),
            ToolFamily::Edit => format!("Edit {}", path_of(&["file_path", "path"])),
            ToolFamily::Write => format!("Write {}", path_of(&["file_path", "path"])),
            ToolFamily::Read => format!("Read {}", path_of(&["file_path", "path"])),
            ToolFamily::Glob => format!(
                "Find files {}",
                text_of(input, "pattern").unwrap_or_default()
            ),
            ToolFamily::Grep => format!(
                "Search for {}",
                text_of(input, "pattern")
                    .or_else(|| text_of(input, "query"))
                    .unwrap_or_default()
            ),
            ToolFamily::WebSearch => format!(
                "Search the web: {}",
                text_of(input, "query").unwrap_or_default()
            ),
            ToolFamily::WebFetch => format!("Fetch {}", text_of(input, "url").unwrap_or_default()),
            ToolFamily::Subagent => text_of(input, "description")
                .map(first_line)
                .unwrap_or_else(|| "Sub-agent".to_owned()),
            ToolFamily::Todo => "Todo list".to_owned(),
        },
        Understood::Orch(tool) => tool.name().to_owned(),
        Understood::Mcp { server, tool } => format!("{server}: {tool}"),
        Understood::Other => fallback,
    }
}

/// The call as the harness made it: its name, its arguments as JSON text, and what came back as text.
#[must_use]
pub fn raw_call(view: &ToolView) -> RawCall {
    let output = output_of(view);
    RawCall {
        name: if view.name.is_empty() {
            view.title.clone()
        } else {
            view.name.clone()
        },
        input: OutputRef::of(&view.raw_input.to_string()),
        output: view.status.ended().then(|| OutputRef::of(&output)),
        is_error: view.status == Status::Failed,
    }
}

fn json_or_text(text: &str) -> Json {
    serde_json::from_str(text).unwrap_or_else(|_| Json::String(text.to_owned()))
}

/// The call as Orch understood it, for a display made for its kind.
#[must_use]
pub fn tool_payload(view: &ToolView, place: &Place<'_>) -> ToolPayload {
    let input = &view.raw_input;
    let output = output_of(view);
    let ended = view.status.ended();
    let failed = view.status == Status::Failed;
    let family = match &view.understood {
        Understood::Family(family) | Understood::Exec(family) => Some(*family),
        _ => None,
    };
    let Some(family) = family else {
        return match &view.understood {
            Understood::Orch(tool) => ToolPayload::Orch {
                tool: *tool,
                arguments: input.clone(),
                result: (ended && !failed).then(|| json_or_text(&output)),
            },
            Understood::Mcp { server, tool } => ToolPayload::Mcp {
                server: server.clone(),
                tool: tool.clone(),
                arguments: input.clone(),
                result: (ended && !failed).then(|| json_or_text(&output)),
                error: (ended && failed).then(|| output.clone()),
            },
            _ => ToolPayload::Other {
                name: if view.name.is_empty() {
                    view.title.clone()
                } else {
                    view.name.clone()
                },
                arguments: (!input.is_null()).then(|| input.clone()),
                result: ended.then(|| {
                    view.raw_output
                        .clone()
                        .filter(|raw| !raw.is_string())
                        .unwrap_or_else(|| json_or_text(&output))
                }),
            },
        };
    };
    match family {
        ToolFamily::Command => {
            let marker = place.program.command_exit_marker.as_deref();
            ToolPayload::Command {
                command: ["command", "cmd"]
                    .iter()
                    .find_map(|member| text_of(input, member))
                    .unwrap_or_default()
                    .to_owned(),
                cwd: text_of(input, "workdir")
                    .or_else(|| text_of(input, "cwd"))
                    .unwrap_or(place.cwd)
                    .to_owned(),
                exit_code: ended.then(|| exit_code_of(view, marker)).flatten(),
                output: OutputRef::of(&output),
                process_id: None,
                background: input
                    .get("run_in_background")
                    .or_else(|| input.get("background"))
                    .and_then(Json::as_bool)
                    .unwrap_or(false),
                actions: Vec::new(),
            }
        }
        ToolFamily::Edit | ToolFamily::Write => ToolPayload::FileChange {
            changes: file_changes(view, family, place),
        },
        ToolFamily::Read => {
            let path = ["file_path", "path"]
                .iter()
                .find_map(|member| text_of(input, member))
                .or_else(|| {
                    view.locations
                        .first()
                        .and_then(|location| location.get("path"))
                        .and_then(Json::as_str)
                })
                .unwrap_or_default();
            let start = number_of(input, "offset")
                .or_else(|| number_of(input, "line"))
                .or_else(|| {
                    view.locations
                        .first()
                        .and_then(|location| location.get("line"))
                        .and_then(Json::as_i64)
                });
            let count = number_of(input, "limit");
            ToolPayload::FileRead {
                path: shown_path(path, place.directory),
                start_line: start,
                end_line: match (start, count) {
                    (Some(start), Some(count)) if count > 0 => Some(start + count - 1),
                    _ => None,
                },
                total_lines: None,
                content: OutputRef::of(&output),
                media_type: None,
            }
        }
        ToolFamily::Glob | ToolFamily::Grep => ToolPayload::FileSearch {
            mode: if family == ToolFamily::Glob {
                FileSearchMode::Name
            } else {
                FileSearchMode::Content
            },
            query: text_of(input, "pattern")
                .or_else(|| text_of(input, "query"))
                .unwrap_or_default()
                .to_owned(),
            path: text_of(input, "path").map(|path| shown_path(path, place.directory)),
            matches: Vec::new(),
            total: None,
            truncated: false,
        },
        ToolFamily::WebSearch => ToolPayload::WebSearch {
            query: text_of(input, "query").unwrap_or_default().to_owned(),
            results: web_results(&output),
        },
        ToolFamily::WebFetch => ToolPayload::WebFetch {
            url: text_of(input, "url").unwrap_or_default().to_owned(),
            title: None,
            status: None,
            content: OutputRef::of(&output),
        },
        ToolFamily::Subagent => ToolPayload::Subagent {
            child_stream: sub_stream(&view.id),
            name: text_of(input, "name")
                .or_else(|| text_of(input, "description"))
                .map(str::to_owned),
            role: text_of(input, "subagent_type").map(str::to_owned),
            prompt: text_of(input, "prompt").map(str::to_owned),
            state: if !ended {
                SubagentState::Running
            } else if failed {
                SubagentState::Failed
            } else {
                SubagentState::Completed
            },
            model: text_of(input, "model").map(str::to_owned),
        },
        ToolFamily::Todo => ToolPayload::Other {
            name: view.name.clone(),
            arguments: Some(input.clone()),
            result: ended.then(|| json_or_text(&output)),
        },
    }
}

/// Results of a web search, where the text lists them as Markdown links (`[title](url)`).
fn web_results(text: &str) -> Vec<WebSearchResult> {
    let mut results = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        let after = &rest[open + 1..];
        let Some(close) = after.find("](") else {
            break;
        };
        let title = &after[..close];
        let tail = &after[close + 2..];
        let Some(end) = tail.find(')') else {
            break;
        };
        let url = &tail[..end];
        if url.starts_with("http") && !title.contains('\n') {
            results.push(WebSearchResult {
                title: title.to_owned(),
                url: url.to_owned(),
            });
        }
        rest = &tail[end + 1..];
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn program() -> AcpProgram {
        AcpProgram::from_json(&json!({
            "kind": "demo", "title": "Demo", "supported_range": "*",
            "command_exit_marker": "[exit code:",
            "tools": [
                { "names": ["bash"], "family": "command", "argument": "command" },
                { "names": ["write"], "family": "write", "argument": "file_path" },
                { "names": ["edit"], "family": "edit", "argument": "file_path" },
                { "names": ["read"], "family": "read", "argument": "file_path" },
                { "names": ["grep"], "family": "grep", "argument": "pattern" }
            ]
        }))
        .unwrap()
    }

    fn place<'a>(program: &'a AcpProgram, servers: &'a [String]) -> Place<'a> {
        Place {
            program,
            directory: "/w",
            cwd: "/w/app",
            servers,
        }
    }

    fn view(
        name: &str,
        kind: &str,
        input: Json,
        status: Status,
        text: &str,
        place: &Place<'_>,
    ) -> ToolView {
        ToolView {
            id: "c1".into(),
            name: name.into(),
            title: name.into(),
            kind: kind.into(),
            understood: understand(place, name, name, kind),
            status,
            raw_input: input,
            raw_output: None,
            content: if text.is_empty() {
                Vec::new()
            } else {
                vec![json!({ "type": "content", "content": { "type": "text", "text": text } })]
            },
            locations: Vec::new(),
        }
    }

    #[test]
    fn a_tool_is_placed_by_the_programs_table_then_by_its_servers_then_by_the_protocols_kind() {
        let program = program();
        let servers = vec!["orch".to_owned(), "exec".to_owned(), "github".to_owned()];
        let place = place(&program, &servers);
        assert_eq!(
            understand(&place, "bash", "bash", "other"),
            Understood::Family(ToolFamily::Command)
        );
        assert_eq!(
            understand(&place, "BASH", "", "other"),
            Understood::Family(ToolFamily::Command)
        );
        assert_eq!(
            understand(&place, "mcp__orch__announce", "", "other"),
            Understood::Orch(OrchTool::Announce)
        );
        assert_eq!(
            understand(&place, "orch.read_shared", "", "other"),
            Understood::Orch(OrchTool::ReadShared)
        );
        assert_eq!(
            understand(&place, "mcp__exec__shell", "", "other"),
            Understood::Exec(ToolFamily::Command)
        );
        assert_eq!(
            understand(&place, "github_create_issue", "", "other"),
            Understood::Mcp {
                server: "github".into(),
                tool: "create_issue".into()
            }
        );
        assert_eq!(
            understand(&place, "mystery", "mystery", "other"),
            Understood::Other
        );
        assert_eq!(
            understand(&place, "thing", "thing", "execute"),
            Understood::Family(ToolFamily::Command)
        );
    }

    #[test]
    fn a_command_has_its_text_its_exit_code_from_the_marker_and_its_output() {
        let program = program();
        let place = place(&program, &[]);
        let view = view(
            "bash",
            "other",
            json!({ "command": "ls", "description": "list files", "run_in_background": false }),
            Status::Completed,
            "a\nb\n[exit code: 2]",
            &place,
        );
        let ToolPayload::Command {
            command,
            cwd,
            exit_code,
            output,
            background,
            ..
        } = tool_payload(&view, &place)
        else {
            panic!("a command");
        };
        assert_eq!((command.as_str(), cwd.as_str()), ("ls", "/w/app"));
        assert_eq!(exit_code, Some(2));
        assert!(output.inline.contains("[exit code: 2]") && !background);
        assert_eq!(call_title(&view, &place), "list files");
        let running = ToolView {
            status: Status::InProgress,
            ..view.clone()
        };
        let ToolPayload::Command { exit_code, .. } = tool_payload(&running, &place) else {
            panic!()
        };
        assert_eq!(exit_code, None, "a command that runs has no exit code yet");
    }

    #[test]
    fn a_write_and_an_edit_become_diffs_and_a_protocol_diff_is_used_as_it_is() {
        let program = program();
        let place = place(&program, &[]);
        let write = view(
            "write",
            "other",
            json!({ "file_path": "/w/a.txt", "content": "x\ny\n" }),
            Status::Completed,
            "",
            &place,
        );
        let ToolPayload::FileChange { changes } = tool_payload(&write, &place) else {
            panic!()
        };
        assert_eq!(changes[0].path, "a.txt");
        assert_eq!(
            (changes[0].change, changes[0].added, changes[0].removed),
            (FileChangeKind::Add, 2, 0)
        );
        let edit = view(
            "edit",
            "other",
            json!({ "file_path": "/w/a.txt", "old_string": "x\n", "new_string": "z\n" }),
            Status::Completed,
            "",
            &place,
        );
        let ToolPayload::FileChange { changes } = tool_payload(&edit, &place) else {
            panic!()
        };
        assert_eq!(
            (changes[0].change, changes[0].added, changes[0].removed),
            (FileChangeKind::Modify, 1, 1)
        );
        let mut standard = view("Edit", "edit", json!({}), Status::Completed, "", &place);
        standard.content =
            vec![json!({ "type": "diff", "path": "/w/b.rs", "oldText": "a\n", "newText": "b\n" })];
        let ToolPayload::FileChange { changes } = tool_payload(&standard, &place) else {
            panic!()
        };
        assert_eq!(changes[0].path, "b.rs");
        assert!(
            changes[0].diff.as_deref().unwrap().contains("-a")
                && changes[0].diff.as_deref().unwrap().contains("+b")
        );
    }

    #[test]
    fn a_call_nothing_places_is_kept_whole_and_the_raw_form_is_never_dropped() {
        let program = program();
        let place = place(&program, &[]);
        let view = view(
            "mystery",
            "other",
            json!({ "a": 1 }),
            Status::Failed,
            "boom",
            &place,
        );
        let ToolPayload::Other {
            name,
            arguments,
            result,
        } = tool_payload(&view, &place)
        else {
            panic!()
        };
        assert_eq!(name, "mystery");
        assert_eq!(arguments, Some(json!({ "a": 1 })));
        assert_eq!(result, Some(json!("boom")));
        let raw = raw_call(&view);
        assert!(raw.is_error);
        assert_eq!(raw.input.inline, "{\"a\":1}");
        assert_eq!(raw.output.unwrap().inline, "boom");
    }

    #[test]
    fn mcp_names_are_split_by_the_servers_the_session_has_with_the_longest_first() {
        let servers = vec!["tmux".to_owned(), "tmux_ui".to_owned()];
        assert_eq!(
            split_mcp("tmux_ui_open", &servers),
            Some(("tmux_ui".into(), "open".into()))
        );
        assert_eq!(
            split_mcp("mcp__tmux__send", &servers),
            Some(("tmux".into(), "send".into()))
        );
        assert_eq!(
            split_mcp("mcp__other__thing", &servers),
            Some(("other".into(), "thing".into()))
        );
        assert_eq!(split_mcp("plain", &servers), None);
    }

    #[test]
    fn paths_are_shown_relative_to_the_threads_directory() {
        assert_eq!(shown_path("/w/src/a.rs", "/w"), "src/a.rs");
        assert_eq!(shown_path("/other/a.rs", "/w"), "/other/a.rs");
        assert_eq!(shown_path("/wx/a.rs", "/w"), "/wx/a.rs");
    }
}
