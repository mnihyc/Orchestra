//! The written form of a conversation: turns as parts, a range of them, and three depths (in brief, as it was said with a line for each step, and in full with what each step was given and what came back); one renderer for what an agent reads, a download, a copy, an export, a forward and the file a switch hands on.
//!
//! A conversation is a sequence of turns, counted by their numbers in the thread. What is written of one is what
//! the people who follow the thread see on its screen, in the depth they choose between there: a turn **in brief**
//! (when it was, who asked and the opening of what, how it ended, the opening of the agent's last message, how many
//! steps it took and how many files it changed), **normal** (what each person sent under their name, what the
//! agent said, and a line for each step of its work) and **full** (each step with what it was given and what came
//! back, the plan, each approval and how it was decided, and the lines of the thread's history).
//!
//! **Never written, at any depth:** the agent's reasoning; a call as its harness made it; what happened inside a
//! sub-agent, which is one step with what it was asked and what it answered; the thread's chat, which is a thing
//! of its own. Everything was masked when it was stored, so nothing is masked again here and nothing is unmasked.
//!
//! The text is Markdown in UTC, with names where the screen has names: a person by their display name, an agent
//! by the name of its profile, a thread by its name. A path appears only where the next request needs one: the
//! files of a turn lie in `<turn, four digits>/` of the directory beside the text that holds the files (see
//! [`files_beside`]: `conversation.files/` where the text is `conversation.md`), what its inputs carried as
//! `in-<n>-<name>` and what its steps left by the step's place in the turn.
//!
//! This module only writes. [`turn_text`] writes one turn, [`conversation`] puts written turns under the head
//! with the line that says which turns they are, and [`window`] says which turns fit a bound of bytes. The rows
//! are read into the types of the API elsewhere (`views`), so that the text an agent reads, the text a person
//! exports and the text that is forwarded come from the same code and are the same text.

use orch_api_contract::threads::{
    AnnouncementCategory, AnnouncementFrom, ContentPart, ExportDetail, Forwarded, ForwardedAuthor,
    ForwardedFrom, Item, ItemPayload, ItemStatus, MarkerPayload, OutputRef, PlanStepStatus,
    ToolCallPayload, ToolPayload, Turn, TurnState, UserMessagePayload,
};
use orch_chat::written::size_text;
use orch_documents::references::{self, Typed, Writing};
use orch_types::{Id, Json, Time};
use std::collections::HashMap;
use std::fmt::Write as _;

/// The directory the files of a conversation lie in, beside its written form, where the text lies as
/// `conversation.md`: what the tree an agent is given has.
pub const FILES_DIRECTORY: &str = "conversation.files";

/// What follows the name of a text, without its extension, in the name of the directory its files lie in.
pub const FILES_SUFFIX: &str = ".files";

/// What follows the name of a document, without its extension, in the name of the directory its earlier versions
/// lie in.
pub const HISTORY_SUFFIX: &str = ".history";

/// The extensions of the files a written form is kept in, which are not part of the name its files go by.
const TEXT_EXTENSIONS: [&str; 2] = [".md", ".json"];

/// The characters of what was asked that a turn in brief shows.
pub const BRIEF_ASKED_CHARS: usize = 200;

/// The characters of the agent's last message that a turn in brief shows.
pub const BRIEF_ANSWER_CHARS: usize = 300;

/// The most characters of text from one source that is written under one step: the end of an output, the beginning
/// of anything else. What was left out is said in one line, with the path of the file that holds it whole.
pub const STEP_TEXT_CHARS: usize = 2_000;

/// The most characters of a difference that is written under one step.
pub const STEP_DIFF_CHARS: usize = 4_000;

/// The most matches of a search in files that are written under its step.
pub const SEARCH_MATCHES_SHOWN: usize = 20;

/// The most results of a search of the web that are written under its step.
pub const WEB_RESULTS_SHOWN: usize = 10;

/// The most characters of the reason a failed turn names in its heading.
pub const REASON_CHARS: usize = 120;

/// The most characters of what a step was asked or told that its line repeats where the title says less.
pub const NAME_CHARS: usize = 80;

/// How many characters one marker of the history takes at most.
const MARKER_CHARS: usize = 300;

/// The fewest backticks of a fence around a block of code: the Markdown minimum.
const MIN_FENCE: usize = 3;

/// The most characters of the line of one match of a search in files.
const MATCH_TEXT_CHARS: usize = 160;

/// How many seconds make a minute, and a minute an hour, in the time a turn took.
const MINUTE: i64 = 60;
const HOUR: i64 = 3_600;

/// How much of each part is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Detail {
    /// A few lines for each turn.
    Brief,
    /// What was said, with a line for each step of the agent's work.
    #[default]
    Normal,
    /// Also what each step was given and what came back, approvals and markers.
    Full,
}

impl Detail {
    /// The depth a word names: `brief`, `normal` or `full`.
    #[must_use]
    pub fn parse(word: &str) -> Option<Self> {
        match word {
            "brief" => Some(Self::Brief),
            "normal" => Some(Self::Normal),
            "full" => Some(Self::Full),
            _ => None,
        }
    }

    /// The word that names the depth.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Brief => "brief",
            Self::Normal => "normal",
            Self::Full => "full",
        }
    }
}

impl From<ExportDetail> for Detail {
    fn from(detail: ExportDetail) -> Self {
        match detail {
            ExportDetail::Brief => Self::Brief,
            ExportDetail::Normal => Self::Normal,
            ExportDetail::Full => Self::Full,
        }
    }
}

/// What an approval says, which the item of an approval does not: what was asked, and who decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalFacts {
    /// What was asked, in the words of the request.
    pub title: String,
    /// The display name of whoever decided it; none for one that nobody has decided, or that the mode decided.
    pub decided_by: Option<String>,
}

/// What is known beside the items of a conversation, to write them.
#[derive(Debug, Clone, Default)]
pub struct Beside {
    /// The approvals the items name, by identifier.
    pub approvals: HashMap<Id, ApprovalFacts>,
    /// The last thing each sub-agent said, by the name of its stream.
    pub sub_agents: HashMap<String, String>,
    /// The directory the files of the turns lie in, as the text names it ([`files_beside`] of the file the text is
    /// kept in); [`FILES_DIRECTORY`] where none is given.
    pub files: Option<String>,
}

impl Beside {
    /// The directory the text names the files of its turns in.
    fn directory(&self) -> &str {
        self.files.as_deref().unwrap_or(FILES_DIRECTORY)
    }
}

/// The thread whose conversation is written.
#[derive(Debug, Clone, Copy)]
pub struct Head<'a> {
    /// Its name.
    pub name: &'a str,
    /// The name of the agent profile it runs, which the head names.
    pub agent: &'a str,
    /// Where it stands now: `idle`, `running`, and the rest of the words of a thread's state.
    pub state: &'a str,
}

/// One turn with the items of its main stream, oldest first.
#[derive(Debug, Clone)]
pub struct Part<'a> {
    /// The turn.
    pub turn: &'a Turn,
    /// The name of the agent that answered in it: the profile of its segment.
    pub agent: &'a str,
    /// Its items; those of sub-agents and the agent's reasoning are not written, whatever is given.
    pub items: Vec<&'a Item>,
}

// ---------------------------------------------------------------- small pieces of text

/// A moment as the written forms give it: UTC, to the minute.
#[must_use]
pub fn minute(time: Time) -> String {
    time.get().format("%Y-%m-%dT%H:%MZ").to_string()
}

/// How long a turn took, in the words of a person: `40 s`, `3 min`, `2 h 5 min`.
#[must_use]
pub fn duration_text(seconds: i64) -> String {
    let seconds = seconds.max(0);
    if seconds < MINUTE {
        return format!("{seconds} s");
    }
    if seconds < HOUR {
        return format!("{} min", seconds / MINUTE);
    }
    let hours = seconds / HOUR;
    let minutes = (seconds % HOUR) / MINUTE;
    if minutes == 0 {
        format!("{hours} h")
    } else {
        format!("{hours} h {minutes} min")
    }
}

/// The opening of a text in one line: white space collapsed, cut at `chars` characters and marked when it was.
#[must_use]
pub fn opening(text: &str, chars: usize) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= chars {
        return collapsed;
    }
    let mut cut: String = collapsed.chars().take(chars).collect();
    cut.push('…');
    cut
}

/// The first `chars` characters of a text, and whether more was left.
fn beginning(text: &str, chars: usize) -> (String, bool) {
    if text.chars().count() <= chars {
        return (text.to_owned(), false);
    }
    (text.chars().take(chars).collect(), true)
}

/// The last `chars` characters of a text, and whether more was left before them.
fn ending(text: &str, chars: usize) -> (String, bool) {
    let count = text.chars().count();
    if count <= chars {
        return (text.to_owned(), false);
    }
    (text.chars().skip(count - chars).collect(), true)
}

/// A name that can lie in a path: whatever would break a path is replaced.
#[must_use]
pub fn clean_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | '\0') || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect()
}

/// The directory of the files of a turn, relative to the conversation's, where its text lies as `conversation.md`.
#[must_use]
pub fn files_of(turn: i64) -> String {
    files_in(FILES_DIRECTORY, turn)
}

/// The directory of the files of a turn, in the directory `directory` that holds the conversation's files.
#[must_use]
pub fn files_in(directory: &str, turn: i64) -> String {
    format!("{directory}/{turn:04}")
}

/// The directory that holds the files of the written form kept in the file `path`, as a link to one of them names it
/// from the place the text lies in. This is the one rule of such a link, for a document, a conversation and a chat
/// alike, and for every place a text and its files lie together: the tree an agent reads, the copy it is given, a
/// single export with its files and the archive of a place.
///
/// The directory is the name of the text without its extension, and `.files`: `Handbook.files` beside
/// `…/Handbook.md`, `conversation.files` beside `…/conversation.md`, `chat.files` beside `…/chat.md`. An earlier
/// version of a document lies in `<title>.history/`, one directory below the document's own place, and links to the
/// files of the document one level up: `../Handbook.files`.
#[must_use]
pub fn files_beside(path: &str) -> String {
    let mut parts = path.rsplit('/');
    let name = parts.next().unwrap_or_default();
    if let Some(stem) = parts
        .next()
        .and_then(|dir| dir.strip_suffix(HISTORY_SUFFIX))
    {
        return format!("../{stem}{FILES_SUFFIX}");
    }
    let stem = TEXT_EXTENSIONS
        .iter()
        .find_map(|extension| name.strip_suffix(extension))
        .unwrap_or(name);
    format!("{stem}{FILES_SUFFIX}")
}

/// A fence for a block of code that holds `text`: longer than any run of backticks in it.
fn fence_for(text: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for c in text.chars() {
        if c == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    "`".repeat(longest.max(MIN_FENCE - 1) + 1)
}

/// `text` as a block of code of a language, every line indented by `indent`.
fn fenced(out: &mut String, indent: &str, language: &str, text: &str) {
    let fence = fence_for(text);
    let _ = writeln!(out, "{indent}{fence}{language}");
    for line in text.trim_end_matches('\n').lines() {
        let _ = writeln!(out, "{indent}{line}");
    }
    let _ = writeln!(out, "{indent}{fence}");
}

/// Writes how much was cut from a source, and where the whole is.
fn left_out(out: &mut String, indent: &str, path: &str) {
    let _ = writeln!(out, "{indent}(cut; the whole is in {path})");
}

fn plural(count: usize, one: &str, many: &str) -> String {
    if count == 1 {
        format!("1 {one}")
    } else {
        format!("{count} {many}")
    }
}

// ---------------------------------------------------------------- the pieces of a turn

/// A block of the text of a turn: a paragraph that stands alone, or lines of a list that run together.
enum Block {
    Paragraph(String),
    Lines(String),
}

/// What a person sent, written: their words under their name, then one line for each file or image it carried.
/// `directory` is where the files of this turn lie; `carried` counts the files of the turn's inputs so far, which is
/// what `in-<n>-<name>` numbers.
fn said_by_person(
    who: &str,
    message: &UserMessagePayload,
    directory: &str,
    carried: &mut usize,
) -> String {
    let mut texts: Vec<String> = Vec::new();
    let mut lines: Vec<String> = Vec::new();
    for part in &message.parts {
        match part {
            ContentPart::Text { text } => {
                if !text.trim().is_empty() {
                    texts.push(text.trim().to_owned());
                }
            }
            ContentPart::Image { name, .. } => {
                *carried += 1;
                lines.push(format!(
                    "[image: {name} · {directory}/in-{}-{}]",
                    *carried,
                    clean_name(name)
                ));
            }
            ContentPart::PastedText(paste) => {
                *carried += 1;
                lines.push(format!(
                    "[Pasted text: {} characters · {} · {directory}/in-{}-{}]",
                    paste.characters,
                    size_text(paste.size),
                    *carried,
                    clean_name(&paste.name)
                ));
            }
            ContentPart::File { name, size, .. } => {
                *carried += 1;
                lines.push(format!(
                    "[file: {name} · {} · {directory}/in-{}-{}]",
                    size_text(*size),
                    *carried,
                    clean_name(name)
                ));
            }
            ContentPart::Forwarded(copy) => lines.push(forwarded_quote(copy)),
        }
    }
    let mut out = format!("**{who}:**");
    if !texts.is_empty() {
        out.push(' ');
        out.push_str(&texts.join("\n\n"));
    }
    for line in lines {
        out.push('\n');
        out.push_str(&line);
    }
    out
}

fn source_of(from: &ForwardedFrom) -> String {
    match from {
        ForwardedFrom::Chat { chat } => chat
            .node
            .as_ref()
            .and_then(|node| node.name.clone())
            .or_else(|| chat.name.clone())
            .map_or_else(|| "a chat".to_owned(), |name| format!("{name} chat")),
        ForwardedFrom::Thread { thread } => thread
            .name
            .clone()
            .map_or_else(|| "a thread".to_owned(), |name| format!("thread {name}")),
    }
}

/// A forwarded message: a quote under a line that says whose it was and where from.
fn forwarded_quote(copy: &Forwarded) -> String {
    let author = match &copy.author {
        Some(ForwardedAuthor::User(user)) => human(user),
        Some(ForwardedAuthor::Agent(agent)) => agent.agent.clone(),
        None => "someone".to_owned(),
    };
    let mut lines = vec![format!(
        "> Forwarded from {} · {author} · {}",
        source_of(&copy.from),
        minute(copy.at)
    )];
    for line in copy.text.lines() {
        lines.push(format!("> {line}"));
    }
    for image in &copy.images {
        lines.push(format!("> [image: {}]", image.name));
    }
    for file in &copy.attachments {
        lines.push(format!(
            "> [file: {} · {}]",
            file.name,
            size_text(file.size)
        ));
    }
    lines.join("\n")
}

fn announcement_text(item: &Item) -> Option<String> {
    let ItemPayload::Announcement(payload) = &item.payload else {
        return None;
    };
    let from = match &payload.from {
        AnnouncementFrom::Thread(thread) => thread.name.clone(),
        AnnouncementFrom::User(user) => human(user),
    };
    if payload.to_thread {
        return Some(format!("**{from} (message):** {}", payload.body.trim()));
    }
    let kind = match payload.category {
        AnnouncementCategory::Notice => "announcement",
        AnnouncementCategory::Milestone => "milestone",
    };
    let body = payload.body.trim();
    if body.is_empty() {
        Some(format!("**{from} ({kind}):** {}", payload.title.trim()))
    } else {
        Some(format!(
            "**{from} ({kind}):** {}: {body}",
            payload.title.trim()
        ))
    }
}

/// Whether a step ended in a way its title does not say, and the words for it.
fn ending_of(item: &Item, call: &ToolCallPayload) -> String {
    if let ToolPayload::Command {
        exit_code: Some(code),
        ..
    } = &call.tool
        && *code != 0
    {
        return format!(" · exit {code}");
    }
    let mcp_failed = matches!(&call.tool, ToolPayload::Mcp { error: Some(_), .. });
    match item.status {
        ItemStatus::Failed => " · failed".to_owned(),
        ItemStatus::InProgress => " · still running".to_owned(),
        ItemStatus::Cancelled => " · cancelled".to_owned(),
        _ if mcp_failed => " · failed".to_owned(),
        _ => String::new(),
    }
}

/// The line of a step when its title says nothing.
fn kind_title(tool: &ToolPayload) -> String {
    match tool {
        ToolPayload::Command { command, .. } => format!("Ran `{}`", opening(command, NAME_CHARS)),
        ToolPayload::FileChange { changes } => {
            plural(changes.len(), "file changed", "files changed")
        }
        ToolPayload::FileRead { path, .. } => format!("Read `{path}`"),
        ToolPayload::FileSearch { query, .. } => format!("Searched for `{query}`"),
        ToolPayload::Mcp { server, tool, .. } => format!("Called {server}.{tool}"),
        ToolPayload::WebSearch { query, .. } => format!("Searched the web for `{query}`"),
        ToolPayload::WebFetch { url, .. } => format!("Fetched {url}"),
        ToolPayload::Image { name, .. } => format!(
            "Looked at an image{}",
            name.as_ref()
                .map(|name| format!(" `{name}`"))
                .unwrap_or_default()
        ),
        ToolPayload::Subagent { name, .. } => format!(
            "Handed work to a sub-agent{}",
            name.as_ref()
                .map(|name| format!(" `{name}`"))
                .unwrap_or_default()
        ),
        ToolPayload::Orch { tool, .. } => format!(
            "Called {}",
            serde_json::to_value(tool)
                .ok()
                .and_then(|word| word.as_str().map(str::to_owned))
                .unwrap_or_default()
        ),
        ToolPayload::Other { name, .. } => format!("Called {name}"),
    }
}

fn json_text(value: &Json) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default()
}

/// What an output holds that the text under a step may show, and the line that says what was left out.
fn output_block(out: &mut String, output: &OutputRef, path_of_whole: &str, indent: &str) {
    let (shown, cut_here) = ending(&output.inline, STEP_TEXT_CHARS);
    let more = cut_here
        || output.truncated
        || output.total_len > i64::try_from(output.inline.len()).unwrap_or(i64::MAX);
    if shown.trim().is_empty() && !more {
        return;
    }
    if !shown.trim().is_empty() {
        fenced(out, indent, "text", &shown);
    }
    if more {
        left_out(out, indent, path_of_whole);
    }
}

/// What a step was given and what came back, at the depth that shows it.
fn step_details(out: &mut String, call: &ToolCallPayload, turn: i64, step: usize, beside: &Beside) {
    const INDENT: &str = "  ";
    let directory = files_in(beside.directory(), turn);
    match &call.tool {
        ToolPayload::Command { cwd, output, .. } => {
            if !cwd.is_empty() {
                let _ = writeln!(out, "{INDENT}in `{cwd}`");
            }
            output_block(
                out,
                output,
                &format!("{directory}/{step:02}-output.txt"),
                INDENT,
            );
        }
        ToolPayload::FileChange { changes } => {
            for change in changes {
                let kind = serde_json::to_value(change.change)
                    .ok()
                    .and_then(|word| word.as_str().map(str::to_owned))
                    .unwrap_or_default();
                let _ = writeln!(
                    out,
                    "{INDENT}`{}` ({kind}, +{} −{})",
                    change.path, change.added, change.removed
                );
                match &change.diff {
                    Some(diff) => {
                        let (shown, cut) = beginning(diff, STEP_DIFF_CHARS);
                        fenced(out, INDENT, "diff", &shown);
                        if cut || change.truncated {
                            left_out(out, INDENT, &format!("{directory}/{step:02}.diff"));
                        }
                    }
                    None if change.binary => {
                        let _ = writeln!(out, "{INDENT}(not text)");
                    }
                    None => {}
                }
            }
        }
        ToolPayload::FileRead {
            start_line,
            end_line,
            total_lines,
            ..
        } => {
            let of = total_lines
                .map(|total| format!(" of {total}"))
                .unwrap_or_default();
            match (start_line, end_line) {
                (Some(start), Some(end)) => {
                    let _ = writeln!(out, "{INDENT}lines {start}–{end}{of}");
                }
                _ => {
                    let _ = writeln!(out, "{INDENT}the whole file{of}");
                }
            }
        }
        ToolPayload::FileSearch {
            query,
            path,
            matches,
            total,
            truncated,
            ..
        } => {
            let place = path
                .as_ref()
                .map(|path| format!(" in `{path}`"))
                .unwrap_or_default();
            let _ = writeln!(out, "{INDENT}looked for `{query}`{place}");
            for found in matches.iter().take(SEARCH_MATCHES_SHOWN) {
                let line = found
                    .line
                    .map(|line| format!(":{line}"))
                    .unwrap_or_default();
                let text = found
                    .text
                    .as_ref()
                    .map(|text| format!(" {}", opening(text, MATCH_TEXT_CHARS)))
                    .unwrap_or_default();
                let _ = writeln!(out, "{INDENT}- `{}{line}`{text}", found.path);
            }
            let found = total.unwrap_or_else(|| i64::try_from(matches.len()).unwrap_or(i64::MAX));
            if found > i64::try_from(SEARCH_MATCHES_SHOWN).unwrap_or(i64::MAX) || *truncated {
                let _ = writeln!(
                    out,
                    "{INDENT}({found} matches in all; the first {SEARCH_MATCHES_SHOWN} are written)"
                );
            }
        }
        ToolPayload::WebSearch { query, results } => {
            let _ = writeln!(out, "{INDENT}looked for `{query}`");
            for result in results.iter().take(WEB_RESULTS_SHOWN) {
                let _ = writeln!(out, "{INDENT}- {} <{}>", result.title, result.url);
            }
        }
        ToolPayload::WebFetch {
            url, title, status, ..
        } => {
            let status = status
                .map(|code| format!(" · status {code}"))
                .unwrap_or_default();
            let title = title
                .as_ref()
                .map(|title| format!(" · {title}"))
                .unwrap_or_default();
            let _ = writeln!(out, "{INDENT}<{url}>{status}{title}");
        }
        ToolPayload::Image { name, .. } => {
            let name = name.clone().unwrap_or_else(|| "image".to_owned());
            let _ = writeln!(
                out,
                "{INDENT}[image: {name} · {directory}/{step:02}-{}]",
                clean_name(&name)
            );
        }
        ToolPayload::Subagent {
            child_stream,
            prompt,
            ..
        } => {
            if let Some(prompt) = prompt {
                let (shown, cut) = beginning(prompt, STEP_TEXT_CHARS);
                let _ = writeln!(
                    out,
                    "{INDENT}asked: {}{}",
                    opening(&shown, STEP_TEXT_CHARS),
                    if cut { "…" } else { "" }
                );
            }
            if let Some(last) = beside.sub_agents.get(child_stream) {
                let (shown, cut) = beginning(last, STEP_TEXT_CHARS);
                let _ = writeln!(
                    out,
                    "{INDENT}answered: {}{}",
                    opening(&shown, STEP_TEXT_CHARS),
                    if cut { "…" } else { "" }
                );
            }
        }
        ToolPayload::Mcp {
            arguments,
            result,
            error,
            ..
        } => {
            json_step(out, "arguments", Some(arguments));
            json_step(out, "result", result.as_ref());
            if let Some(error) = error {
                let _ = writeln!(out, "{INDENT}error: {}", opening(error, STEP_TEXT_CHARS));
            }
        }
        ToolPayload::Orch {
            arguments, result, ..
        } => {
            json_step(out, "arguments", Some(arguments));
            json_step(out, "result", result.as_ref());
        }
        ToolPayload::Other {
            arguments, result, ..
        } => {
            json_step(out, "arguments", arguments.as_ref());
            json_step(out, "result", result.as_ref());
        }
    }
}

fn json_step(out: &mut String, label: &str, value: Option<&Json>) {
    const INDENT: &str = "  ";
    let Some(value) = value else { return };
    if value.is_null() {
        return;
    }
    let (shown, cut) = beginning(&json_text(value), STEP_TEXT_CHARS);
    let _ = writeln!(out, "{INDENT}{label}:");
    fenced(out, INDENT, "json", &shown);
    if cut {
        let _ = writeln!(out, "{INDENT}(cut)");
    }
}

fn human(user: &orch_api_contract::users::UserRef) -> String {
    orch_chat::written::person_name(&user.display_name, &user.username)
}

/// What the history says, in the words the screen has for it.
#[must_use]
pub fn marker_words(marker: &MarkerPayload) -> String {
    use MarkerPayload as M;
    let node = |node: &orch_api_contract::nodes::NodeRef| {
        node.name.clone().unwrap_or_else(|| {
            format!(
                "a removed {}",
                match node.kind {
                    orch_api_contract::nodes::NodeKind::Org => "org",
                    orch_api_contract::nodes::NodeKind::Project => "project",
                    orch_api_contract::nodes::NodeKind::Topic => "topic",
                    orch_api_contract::nodes::NodeKind::Thread => "thread",
                }
            )
        })
    };
    let words = match marker {
        M::ResourceResumed(m) => format!(
            "{} enabled {}: {}{}",
            human(&m.by),
            m.name,
            if m.input_id.is_some() {
                "the original task was continued"
            } else {
                "the original task stays stopped"
            },
            m.reason
                .as_ref()
                .map(|reason| format!(" ({reason})"))
                .unwrap_or_default(),
        ),
        M::ResourceStopped(m) => format!(
            "Stopped: {} was {} by {}{}",
            m.name,
            if m.deleted { "removed" } else { "switched off" },
            human(&m.by),
            m.note
                .as_ref()
                .map(|note| format!(" · {note}"))
                .unwrap_or_default()
        ),
        M::CallRecovery(m) => format!(
            "Automatic {} after {} (rule {}, attempt {}){}",
            m.action,
            m.kind,
            m.rule,
            m.attempt,
            m.model
                .as_ref()
                .map(|model| format!(" · {model}"))
                .unwrap_or_default()
        ),
        M::OrchContext(_) => "Orch supplied context to the agent".to_owned(),
        M::Created(m) => format!("{} created this thread", human(&m.by)),
        M::AgentHistory(m) => {
            let size = m
                .bytes
                .map(|bytes| format!(" · {bytes} bytes"))
                .unwrap_or_default();
            if let Some(mode) = m.recovered {
                let history = match mode {
                    orch_api_contract::threads::AgentHistoryRecovery::Native => {
                        "recorded turns imported natively"
                    }
                    orch_api_contract::threads::AgentHistoryRecovery::Carry => {
                        "recent turns carried in"
                    }
                    orch_api_contract::threads::AgentHistoryRecovery::Summary => {
                        "summary carried in"
                    }
                    orch_api_contract::threads::AgentHistoryRecovery::Brief => {
                        "brief recent turns carried in"
                    }
                    orch_api_contract::threads::AgentHistoryRecovery::File => {
                        "conversation provided as a file"
                    }
                    orch_api_contract::threads::AgentHistoryRecovery::None => "history set to None",
                };
                format!("{} starts a new session · {history}{size}", m.program)
            } else {
                let by =
                    m.by.as_ref()
                        .map(human)
                        .unwrap_or_else(|| "Automatic retention".into());
                format!("{by} removed {} agent history{size}", m.program)
            }
        }
        M::Renamed(m) => match &m.by {
            Some(by) => format!("{} renamed it from {} to {}", human(by), m.from, m.to),
            None => format!("It was renamed from {} to {}", m.from, m.to),
        },
        M::Described(m) => format!("{} wrote its description", human(&m.by)),
        M::ModelChanged(m) => format!(
            "{} changed the model to {}{}",
            human(&m.by),
            m.to,
            m.effort
                .to
                .as_ref()
                .map(|effort| format!(" at {effort}"))
                .unwrap_or_default()
        ),
        M::SettingChanged(m) => format!("{} changed {}", human(&m.by), m.label),
        M::SourcePrepared(m) => format!(
            "{} prepared code in {}",
            human(&m.by),
            if m.directory.is_empty() {
                "the workspace"
            } else {
                m.directory.as_str()
            }
        ),
        M::FilesRestored(m) => format!(
            "{} restored {}",
            human(&m.by),
            plural(usize::try_from(m.files).unwrap_or(0), "file", "files")
        ),
        M::LinksChanged(m) => format!(
            "{} changed the thread's {}",
            human(&m.by),
            plural(usize::try_from(m.count).unwrap_or(0), "link", "links")
        ),
        M::AccessChanged(m) => format!("{} changed who may use this thread", human(&m.by)),
        M::OwnerChanged(m) => format!("{} made {} its first owner", human(&m.by), human(&m.to)),
        M::SegmentStarted(m) => format!(
            "{} {} with {}",
            m.agent.name,
            if m.resumed {
                "continued the conversation"
            } else {
                "began the conversation"
            },
            m.provider.name
        ),
        M::Switched(m) => format!("{} switched what it runs on", human(&m.by)),
        M::ForkedFrom(m) => format!("{} forked it from {}", human(&m.by), node(&m.thread)),
        M::Forked(m) => format!("{} forked {} from it", human(&m.by), node(&m.into)),
        M::EnvironmentRecreated(m) => {
            format!("Its environment was made again from {}", m.template.name)
        }
        M::EnvironmentMoved(m) => format!("{} moved its environment", human(&m.by)),
        M::EnvironmentStopped(_) => "Its environment was stopped".to_owned(),
        M::EnvironmentStarted(_) => "Its environment was started".to_owned(),
        M::EnvironmentRestarted(m) => match &m.by {
            Some(by) => format!("{} restarted its environment", human(by)),
            None => "Its environment was restarted".to_owned(),
        },
        M::EnvironmentTools(m) => format!("{} changed the tools available here", human(&m.by)),
        M::CapabilityGranted(m) => format!("The capability {} was granted", m.capability),
        M::CapabilityRevoked(m) => format!("The capability {} was taken back", m.capability),
        M::Compacted(_) => "The agent shortened its own conversation".to_owned(),
        M::InstructionsDeferred(m) => format!(
            "{} keeps this session's original instructions; changes apply to the next new session",
            m.program
        ),
        M::LimitBypassed(m) => format!("{} let a turn past a limit", human(&m.by)),
        M::Archived(m) => format!("{} archived {}", human(&m.by), node(&m.node)),
        M::Unarchived(m) => format!("{} took {} out of the archive", human(&m.by), node(&m.node)),
        M::Restored(m) => format!("{} brought {} back", human(&m.by), node(&m.node)),
        M::Shared(m) => format!("{} shared something with its agent", human(&m.by)),
        M::Unshared(m) => format!("{} ended a share with its agent", human(&m.by)),
        M::SideStarted(m) => format!("{} started a side thread", human(&m.by)),
        M::SideClosed(m) => format!(
            "A side thread was closed after {}",
            plural(usize::try_from(m.turns).unwrap_or(0), "turn", "turns")
        ),
        M::QueueHeld(m) => format!(
            "{} stopped and held {}",
            m.by.as_ref().map_or_else(|| "Somebody".to_owned(), human),
            plural(
                usize::try_from(m.inputs).unwrap_or(0),
                "waiting message",
                "waiting messages"
            )
        ),
        M::QueueResumed(m) => format!(
            "{} resumed {}",
            m.by.as_ref().map_or_else(|| "Somebody".to_owned(), human),
            plural(
                usize::try_from(m.inputs).unwrap_or(0),
                "held message",
                "held messages"
            )
        ),
        M::ThreadMoved(m) => format!(
            "{} moved this thread from {} to {}",
            human(&m.by),
            node(&m.from),
            node(&m.to)
        ),
        M::Adopted(m) => format!(
            "{} took in a session begun outside Orch; {} earlier turns of it are here as its harness recorded them",
            human(&m.by),
            m.turns
        ),
        M::GoalSet(m) => format!(
            "{} set a goal: keep working until {}",
            human(&m.by),
            m.condition
        ),
        M::GoalChanged(m) => format!(
            "{} changed the goal: keep working until {}",
            human(&m.by),
            m.condition
        ),
        M::GoalPaused(m) => {
            use orch_api_contract::goals::GoalReason;
            let by = || m.by.as_ref().map_or_else(|| "Somebody".to_owned(), human);
            match m.reason {
                GoalReason::Interrupted => {
                    format!("{} stopped a turn, which paused the goal", by())
                }
                GoalReason::Blocked => format!(
                    "The goal was paused: the agent is blocked{}",
                    m.detail
                        .as_ref()
                        .map(|detail| format!(" ({detail})"))
                        .unwrap_or_default()
                ),
                GoalReason::Failed => "A turn failed, which paused the goal".to_owned(),
                _ => format!("{} paused the goal", by()),
            }
        }
        M::GoalResumed(m) => format!("{} set the goal going again", human(&m.by)),
        M::GoalMet(m) => format!(
            "The goal was met after {}: {} ({})",
            plural(usize::try_from(m.turns).unwrap_or(0), "turn", "turns"),
            m.condition,
            match m.checked_by {
                orch_api_contract::goals::GoalCheckedBy::Helper => "checked by the summary model",
                orch_api_contract::goals::GoalCheckedBy::Agent => "the agent's report",
            }
        ),
        M::GoalStopped(m) => format!(
            "The goal stopped after {}{}",
            plural(usize::try_from(m.turns).unwrap_or(0), "turn", "turns"),
            m.limit
                .as_ref()
                .map(|limit| format!(" at its limit of {limit}"))
                .unwrap_or_default()
        ),
        M::GoalCleared(m) => format!("{} cleared the goal", human(&m.by)),
        M::ApprovalRulesChanged(m) => match (m.action, m.by.as_ref()) {
            (orch_api_contract::threads::ApprovalRuleAction::Allowed, by) => format!(
                "{} allowed an action for this thread",
                by.map_or_else(|| "Somebody".to_owned(), human)
            ),
            (orch_api_contract::threads::ApprovalRuleAction::Revoked, Some(by)) => {
                format!("{} took back an action allowed for this thread", human(by))
            }
            (orch_api_contract::threads::ApprovalRuleAction::Revoked, None) => {
                "An action allowed for this thread ended with a change of what it runs on"
                    .to_owned()
            }
        },
    };
    opening(&words, MARKER_CHARS)
}

/// What an approval comes to, in one line.
fn approval_line(item: &Item, beside: &Beside) -> Option<String> {
    let ItemPayload::Approval(payload) = &item.payload else {
        return None;
    };
    let asked = beside
        .approvals
        .get(&payload.approval_id)
        .map_or_else(|| "a request".to_owned(), |facts| facts.title.clone());
    let by = beside
        .approvals
        .get(&payload.approval_id)
        .and_then(|facts| facts.decided_by.clone());
    let outcome = match (item.status, by) {
        (ItemStatus::Approved, Some(by)) => format!("approved by {by}"),
        (ItemStatus::Approved, None) => "approved".to_owned(),
        (ItemStatus::Denied, Some(by)) => format!("denied by {by}"),
        (ItemStatus::Denied, None) => "denied".to_owned(),
        (ItemStatus::Expired, _) => "expired".to_owned(),
        _ => "waiting".to_owned(),
    };
    Some(format!("- Approval: {asked} · {outcome}"))
}

fn plan_text(item: &Item) -> Option<String> {
    let ItemPayload::Plan(plan) = &item.payload else {
        return None;
    };
    let mut out = String::from("Plan:");
    if let Some(explanation) = plan
        .explanation
        .as_ref()
        .filter(|text| !text.trim().is_empty())
    {
        let _ = write!(out, " {}", explanation.trim());
    }
    for step in &plan.steps {
        let mark = match step.status {
            PlanStepStatus::Completed => "x",
            PlanStepStatus::Pending | PlanStepStatus::InProgress => " ",
        };
        let note = if step.status == PlanStepStatus::InProgress {
            " (under way)"
        } else {
            ""
        };
        let _ = write!(out, "\n- [{mark}] {}{note}", step.text.trim());
    }
    Some(out)
}

fn ended_words(turn: &Turn) -> String {
    match turn.state {
        TurnState::Completed => "completed".to_owned(),
        TurnState::Interrupted => "interrupted".to_owned(),
        TurnState::Failed => {
            let reason = turn
                .error
                .as_ref()
                .map(|error| opening(&error.message, REASON_CHARS))
                .filter(|reason| !reason.is_empty())
                .unwrap_or_else(|| "it failed".to_owned());
            format!("failed: {reason}")
        }
        TurnState::Starting | TurnState::Running => "running".to_owned(),
    }
}

/// The heading of a turn.
fn heading(part: &Part<'_>) -> String {
    let turn = part.turn;
    let began = turn
        .started_at
        .or_else(|| part.items.first().map(|item| item.created_at));
    let mut out = format!("## Turn {}", turn.ordinal);
    if let Some(began) = began {
        let _ = write!(out, " · {}", minute(began));
    }
    let _ = write!(out, " · {}", ended_words(turn));
    if let (Some(started), Some(done)) = (turn.started_at, turn.completed_at) {
        let _ = write!(
            out,
            " · {}",
            duration_text((done.get() - started.get()).num_seconds())
        );
    }
    out
}

/// A turn in brief: when it was and how it ended in the heading, who asked and the opening of what, the opening of
/// the agent's last message, and how much it did.
fn brief_lines(part: &Part<'_>) -> Vec<String> {
    let mut lines = Vec::new();
    let asked = part.items.iter().find_map(|item| match &item.payload {
        ItemPayload::UserMessage(message) => {
            let who = item
                .author
                .as_ref()
                .map_or_else(|| "Orch".to_owned(), human);
            let text: Vec<&str> = message
                .parts
                .iter()
                .filter_map(|part| match part {
                    ContentPart::Text { text } => Some(text.as_str()),
                    ContentPart::Forwarded(copy) => Some(copy.text.as_str()),
                    _ => None,
                })
                .collect();
            Some((who, text.join(" ")))
        }
        _ => None,
    });
    if let Some((who, text)) = asked {
        lines.push(format!("**{who}:** {}", opening(&text, BRIEF_ASKED_CHARS)));
    }
    let answered = part
        .items
        .iter()
        .rev()
        .find_map(|item| match &item.payload {
            ItemPayload::AgentMessage(message) if !message.text.trim().is_empty() => {
                Some(message.text.clone())
            }
            _ => None,
        });
    if let Some(text) = answered {
        lines.push(format!(
            "**{}:** {}",
            part.agent,
            opening(&text, BRIEF_ANSWER_CHARS)
        ));
    }
    let steps = part
        .items
        .iter()
        .filter(|item| matches!(item.payload, ItemPayload::ToolCall(_)))
        .count();
    let mut changed: Vec<&str> = Vec::new();
    for item in &part.items {
        if let ItemPayload::ToolCall(call) = &item.payload
            && let ToolPayload::FileChange { changes } = &call.tool
        {
            for change in changes {
                if !changed.contains(&change.path.as_str()) {
                    changed.push(&change.path);
                }
            }
        }
    }
    if steps > 0 || !changed.is_empty() {
        let mut counts = plural(steps, "step", "steps");
        if !changed.is_empty() {
            let _ = write!(
                counts,
                " · {}",
                plural(changed.len(), "file changed", "files changed")
            );
        }
        lines.push(counts);
    }
    lines
}

/// Whether an item is of the main stream: what is written of a conversation. Sub-agents' items and the agent's
/// reasoning are never.
fn is_written(item: &Item) -> bool {
    item.stream == orch_api_contract::threads::MAIN_STREAM
        && !matches!(item.payload, ItemPayload::Reasoning(_))
}

/// One turn, written at a depth: its heading, then what was said and done in it.
#[must_use]
pub fn turn_text(part: &Part<'_>, beside: &Beside, detail: Detail) -> String {
    let items: Vec<&Item> = part
        .items
        .iter()
        .copied()
        .filter(|item| is_written(item))
        .collect();
    let part = Part {
        turn: part.turn,
        agent: part.agent,
        items,
    };
    let mut out = heading(&part);
    out.push('\n');
    if detail == Detail::Brief {
        for line in brief_lines(&part) {
            out.push('\n');
            out.push_str(&line);
        }
        out.push('\n');
        return out;
    }
    let turn = part.turn.ordinal;
    let last_plan = part
        .items
        .iter()
        .rposition(|item| matches!(item.payload, ItemPayload::Plan(_)));
    let mut blocks: Vec<Block> = Vec::new();
    let mut carried = 0_usize;
    let mut step = 0_usize;
    let push_line = |blocks: &mut Vec<Block>, text: String| {
        if let Some(Block::Lines(lines)) = blocks.last_mut() {
            lines.push('\n');
            lines.push_str(text.trim_end_matches('\n'));
        } else {
            blocks.push(Block::Lines(text.trim_end_matches('\n').to_owned()));
        }
    };
    for (at, item) in part.items.iter().enumerate() {
        match &item.payload {
            ItemPayload::UserMessage(message) => {
                let who = match (&item.author, message.source) {
                    (Some(user), _) => human(user),
                    (None, _) => "Orch".to_owned(),
                };
                blocks.push(Block::Paragraph(said_by_person(
                    &who,
                    message,
                    &files_in(beside.directory(), turn),
                    &mut carried,
                )));
            }
            ItemPayload::AgentMessage(message) => {
                if !message.text.trim().is_empty() {
                    blocks.push(Block::Paragraph(format!(
                        "**{}:** {}",
                        part.agent,
                        message.text.trim()
                    )));
                }
            }
            ItemPayload::Plan(_) => {
                if Some(at) == last_plan
                    && let Some(text) = plan_text(item)
                {
                    blocks.push(Block::Paragraph(text));
                }
            }
            ItemPayload::ToolCall(call) => {
                step += 1;
                let title = if call.title.trim().is_empty() {
                    kind_title(&call.tool)
                } else {
                    call.title.trim().to_owned()
                };
                let mut line = format!("- {title}{}", ending_of(item, call));
                if detail == Detail::Full {
                    let mut details = String::new();
                    step_details(&mut details, call, turn, step, beside);
                    if !details.is_empty() {
                        line.push('\n');
                        line.push_str(details.trim_end_matches('\n'));
                    }
                }
                push_line(&mut blocks, line);
            }
            ItemPayload::Approval(_) => {
                if detail == Detail::Full
                    && let Some(line) = approval_line(item, beside)
                {
                    push_line(&mut blocks, line);
                }
            }
            ItemPayload::Announcement(_) => {
                if let Some(text) = announcement_text(item) {
                    blocks.push(Block::Paragraph(text));
                }
            }
            ItemPayload::Marker(marker) => {
                if detail == Detail::Full {
                    push_line(&mut blocks, format!("- — {} —", marker_words(marker)));
                }
            }
            ItemPayload::Error(error) => {
                blocks.push(Block::Paragraph(format!(
                    "**Error:** {}",
                    error.message.trim()
                )));
            }
            ItemPayload::Reasoning(_) => {}
        }
    }
    for block in blocks {
        out.push('\n');
        match block {
            Block::Paragraph(text) | Block::Lines(text) => {
                out.push_str(&text);
                out.push('\n');
            }
        }
    }
    out
}

/// A conversation written: its head, the line that says which turns these are, and the turns as they were
/// written. `first` and `to` are the numbers of the first and last turn given, `last` the number of the last
/// turn there is.
#[must_use]
pub fn conversation(head: &Head<'_>, first: i64, to: i64, last: i64, turns: &[String]) -> String {
    let mut out = format!(
        "# {}\nThread · {} · {} · turns {}–{} of {}\n",
        head.name, head.agent, head.state, first, to, last
    );
    for text in turns {
        out.push('\n');
        out.push_str(text);
    }
    out
}

/// A conversation with no turn in it yet.
#[must_use]
pub fn empty(head: &Head<'_>) -> String {
    format!(
        "# {}\nThread · {} · {} · no turns yet\n",
        head.name, head.agent, head.state
    )
}

/// Every turn of a conversation written at one depth, under one head: what an export, a copy and a hand-over are.
#[must_use]
pub fn whole(head: &Head<'_>, parts: &[Part<'_>], beside: &Beside, detail: Detail) -> String {
    let (Some(first), Some(last)) = (parts.first(), parts.last()) else {
        return empty(head);
    };
    let texts: Vec<String> = parts
        .iter()
        .map(|part| turn_text(part, beside, detail))
        .collect();
    conversation(
        head,
        first.turn.ordinal,
        last.turn.ordinal,
        last.turn.ordinal,
        &texts,
    )
}

// ---------------------------------------------------------------- which turns fit

/// The turns that are given, of those asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    /// The first turn given.
    pub from: i64,
    /// The last turn given.
    pub to: i64,
    /// Whether the first turn alone is larger than the bound, so that it is given cut.
    pub cut: bool,
}

/// The turns `from` to `to` (both counted in; a negative number counts from the end of a conversation of `last`
/// turns) that fit in `budget` bytes, `size_of(n)` being the bytes of turn `n` written.
///
/// With neither, as many of the latest turns as fit; with `from` alone, turns forward from there; with `to`
/// alone, turns ending there. Parts are given whole: the turns are taken one by one while they fit, and a first
/// turn larger than everything is given cut. Nothing where the range holds no turn.
pub fn window(
    from: Option<i64>,
    to: Option<i64>,
    last: i64,
    budget: usize,
    mut size_of: impl FnMut(i64) -> usize,
) -> Option<Window> {
    if last < 1 {
        return None;
    }
    let number = |n: i64| if n < 0 { last + 1 + n } else { n };
    let (first, end, backwards) = match (from.map(number), to.map(number)) {
        (None, None) => (1, last, true),
        (Some(first), None) => (first, last, false),
        (None, Some(end)) => (1, end, true),
        (Some(first), Some(end)) => (first, end, false),
    };
    let (first, end) = (first.max(1), end.min(last));
    if first > end {
        return None;
    }
    let mut used = 0_usize;
    let mut taken = 0_i64;
    let mut cut = false;
    if backwards {
        let mut at = end;
        while at >= first {
            let size = size_of(at);
            if taken > 0 && used.saturating_add(size) > budget {
                break;
            }
            if taken == 0 && size > budget {
                cut = true;
            }
            used = used.saturating_add(size);
            taken += 1;
            at -= 1;
        }
        Some(Window {
            from: end - taken + 1,
            to: end,
            cut,
        })
    } else {
        let mut at = first;
        while at <= end {
            let size = size_of(at);
            if taken > 0 && used.saturating_add(size) > budget {
                break;
            }
            if taken == 0 && size > budget {
                cut = true;
            }
            used = used.saturating_add(size);
            taken += 1;
            at += 1;
        }
        Some(Window {
            from: first,
            to: first + taken - 1,
            cut,
        })
    }
}

/// A text cut to a bound of bytes at a character boundary, and the line that says it was.
#[must_use]
pub fn cut_to_bytes(text: &str, budget: usize) -> (String, bool) {
    if text.len() <= budget {
        return (text.to_owned(), false);
    }
    let mut end = budget;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_owned(), true)
}

// ---------------------------------------------------------------- reading it from the database

/// The most turns looked at to find the ones that fit a bound: the bound is bytes, and the smallest turn is
/// some hundred of them, so this reaches further than the largest bound does.
pub const CANDIDATE_TURNS: i64 = 200;

/// The most messages looked at to find the ones that fit a bound.
pub const CANDIDATE_MESSAGES: i64 = 300;

/// What was read of a conversation or a chat: the text and which parts it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Read {
    /// The written form.
    pub text: String,
    /// The first part it holds; 0 where it holds none.
    pub from: i64,
    /// The last part it holds.
    pub to: i64,
    /// How many parts there are.
    pub last: i64,
    /// Whether a part was cut to fit the bound.
    pub truncated: bool,
}

/// An ordinary reading names its own range. An export names its complete range
/// once and then appends windows without repeating that heading.
#[derive(Clone, Copy)]
enum WrittenHead {
    Range,
    Export(i64, i64),
    None,
}

#[derive(Clone, Copy)]
struct ConversationLayout<'a> {
    view: Option<Id>,
    files: Option<&'a str>,
    heading: WrittenHead,
}

fn conversation_piece(
    head: &Head<'_>,
    first: i64,
    to: i64,
    last: i64,
    turns: &[String],
    heading: WrittenHead,
) -> String {
    match heading {
        WrittenHead::Range => conversation(head, first, to, last, turns),
        WrittenHead::Export(first, to) => conversation(head, first, to, last, turns),
        WrittenHead::None => {
            let mut out = String::new();
            for text in turns {
                out.push('\n');
                out.push_str(text);
            }
            out
        }
    }
}

/// The parts asked for, as the numbers of the first and the last to look at: all of them when the text is not
/// bounded, else at most `candidates` from the end the range is read from.
fn candidates(
    range: (Option<i64>, Option<i64>),
    last: i64,
    bound: Option<i64>,
) -> Option<(i64, i64)> {
    let number = |n: i64| if n < 0 { last + 1 + n } else { n };
    let (from, to) = (range.0.map(number), range.1.map(number));
    let (low, high) = match (from, to) {
        (None, None) => (1, last),
        (Some(f), None) => (f, last),
        (None, Some(t)) => (1, t),
        (Some(f), Some(t)) => (f, t),
    };
    let (low, high) = (low.max(1), high.min(last));
    if low > high {
        return None;
    }
    let Some(most) = bound else {
        return Some((low, high));
    };
    Some(if from.is_none() {
        ((high - most + 1).max(low), high)
    } else {
        (low, (low + most - 1).min(high))
    })
}

/// The complete range of an export, with the same negative-number reading as
/// the written forms. The producer divides this fixed range into windows.
pub(crate) fn export_range(range: (Option<i64>, Option<i64>), last: i64) -> Option<(i64, i64)> {
    candidates(range, last, None)
}

fn user_ref(person: &orch_db::queries::threads::PersonRow) -> orch_api_contract::users::UserRef {
    orch_api_contract::users::UserRef {
        id: person.id,
        kind: if person.kind == "service" {
            orch_api_contract::users::UserKind::Service
        } else {
            orch_api_contract::users::UserKind::Human
        },
        username: person.username.clone(),
        address: Some(format!("@{}", person.username)),
        display_name: person.display_name.clone(),
        avatar: orch_api_contract::users::Avatar {
            text: person
                .display_name
                .chars()
                .next()
                .map(|c| c.to_uppercase().to_string())
                .unwrap_or_default(),
            color: u32::try_from(person.avatar_color).unwrap_or(0),
        },
    }
}

/// The text a conversation is written as, `range` of its turns (a negative number counts from the end) at
/// `detail`, within `budget` bytes where that is given, for a text that lies as `conversation.md` (its files are
/// named in [`FILES_DIRECTORY`]). Nothing where the thread is not there.
pub async fn read_conversation(
    conn: &mut orch_db::Conn,
    thread: Id,
    range: (Option<i64>, Option<i64>),
    detail: Detail,
    budget: Option<usize>,
) -> Result<Option<Read>, crate::error::EngineError> {
    read_conversation_in(conn, thread, range, detail, budget, None).await
}

/// The same for a text that is kept in another file than `conversation.md`: `files` is the directory beside it
/// that holds the files of the conversation, as the text names it ([`files_beside`] of that file).
pub async fn read_conversation_in(
    conn: &mut orch_db::Conn,
    thread: Id,
    range: (Option<i64>, Option<i64>),
    detail: Detail,
    budget: Option<usize>,
    files: Option<&str>,
) -> Result<Option<Read>, crate::error::EngineError> {
    read_conversation_piece(
        conn,
        thread,
        range,
        detail,
        budget,
        ConversationLayout {
            view: None,
            files,
            heading: WrittenHead::Range,
        },
    )
    .await
}

/// The same renderer over an explicit retained history; reading never selects its native lane.
pub async fn read_conversation_view(
    conn: &mut orch_db::Conn,
    thread: Id,
    view: Id,
    range: (Option<i64>, Option<i64>),
    detail: Detail,
    budget: Option<usize>,
    files: Option<&str>,
) -> Result<Option<Read>, crate::error::EngineError> {
    read_conversation_piece(
        conn,
        thread,
        range,
        detail,
        budget,
        ConversationLayout {
            view: Some(view),
            files,
            heading: WrittenHead::Range,
        },
    )
    .await
}

/// An archive fixes its history once, then streams all of its windows under that identity.
pub(crate) async fn read_conversation_window_view(
    conn: &mut orch_db::Conn,
    thread: Id,
    view: Id,
    range: (i64, i64),
    detail: Detail,
    files: &str,
    heading: Option<(i64, i64)>,
) -> Result<Option<Read>, crate::error::EngineError> {
    read_conversation_piece(
        conn,
        thread,
        (Some(range.0), Some(range.1)),
        detail,
        None,
        ConversationLayout {
            view: Some(view),
            files: Some(files),
            heading: heading.map_or(WrittenHead::None, |(first, to)| {
                WrittenHead::Export(first, to)
            }),
        },
    )
    .await
}

/// Other suffixes leave permanent ordinal gaps; only actual retained parts consume the byte budget.
fn visible_window(
    parts: &[Part<'_>],
    texts: &HashMap<i64, String>,
    forward: bool,
    budget: usize,
) -> Option<Window> {
    let mut ordinals: Vec<_> = parts.iter().map(|part| part.turn.ordinal).collect();
    if !forward {
        ordinals.reverse();
    }
    let mut used = 0_usize;
    let mut chosen = None::<Window>;
    for ordinal in ordinals {
        let Some(text) = texts.get(&ordinal) else {
            continue;
        };
        let next = used.saturating_add(text.len().saturating_add(1));
        if next > budget {
            if chosen.is_none() {
                chosen = Some(Window {
                    from: ordinal,
                    to: ordinal,
                    cut: true,
                });
            }
            break;
        }
        used = next;
        chosen = Some(match chosen {
            None => Window {
                from: ordinal,
                to: ordinal,
                cut: false,
            },
            Some(window) => Window {
                from: window.from.min(ordinal),
                to: window.to.max(ordinal),
                cut: false,
            },
        });
    }
    chosen
}

async fn read_conversation_piece(
    conn: &mut orch_db::Conn,
    thread: Id,
    range: (Option<i64>, Option<i64>),
    detail: Detail,
    budget: Option<usize>,
    layout: ConversationLayout<'_>,
) -> Result<Option<Read>, crate::error::EngineError> {
    use crate::views::{People, item_view, turn_view};
    use orch_db::queries::shares as rows;
    use orch_db::queries::thread_history as selected;
    use orch_db::queries::threads;
    let Some(row) = threads::thread(conn, thread).await? else {
        return Ok(None);
    };
    let Some(projection) =
        orch_db::queries::thread_continuations::projection(conn, thread, layout.view).await?
    else {
        return Ok(None);
    };
    let view = projection.viewed_continuation_id;
    let profile = match row.agent_id {
        Some(agent) => rows::agent_names(conn, &[agent])
            .await?
            .remove(&agent)
            .unwrap_or_default(),
        None => String::new(),
    };
    let agent_name = if profile.is_empty() {
        "Agent".to_owned()
    } else {
        profile
    };
    let head = Head {
        name: &row.name,
        agent: &agent_name,
        state: if view == row.active_continuation_id {
            &row.state
        } else {
            "retained"
        },
    };
    let last = selected::last_turn(conn, thread, view).await?;
    if last == 0 {
        return Ok(Some(Read {
            text: empty(&head),
            from: 0,
            to: 0,
            last: 0,
            truncated: false,
        }));
    }
    let bound = budget.map(|_| CANDIDATE_TURNS);
    let Some((low, high)) = candidates(range, last, bound) else {
        return Ok(Some(Read {
            text: format!(
                "# {}\nThread · {} · {} · no turns in that range (it has {last})\n",
                head.name, head.agent, head.state
            ),
            from: 0,
            to: 0,
            last,
            truncated: false,
        }));
    };
    let turn_rows = selected::turns_between(conn, thread, view, low, high).await?;
    let turn_ids: Vec<Id> = turn_rows.iter().map(|turn| turn.id).collect();
    let item_rows = selected::items_of_turns(conn, thread, view, &turn_ids).await?;
    // The lines of the history that belong to no turn go with the turn that follows them.
    let after = if low > 1 {
        selected::turn_high(conn, thread, view, low - 1)
            .await?
            .unwrap_or(0)
    } else {
        0
    };
    let up_to = if high == last {
        i64::MAX
    } else {
        selected::turn_high(conn, thread, view, high)
            .await?
            .unwrap_or(i64::MAX)
    };
    let loose = if detail == Detail::Full {
        let mut loose =
            selected::items_between(conn, thread, view, after, up_to, Some("main")).await?;
        loose.retain(|row| row.turn_id.is_none() && row.kind != "reasoning");
        loose
    } else {
        Vec::new()
    };
    let mut who: Vec<Id> = Vec::new();
    for item in item_rows.iter().chain(&loose) {
        who.extend(item.author_id);
    }
    for turn in &turn_rows {
        who.extend(turn.interrupted_by);
    }
    let people = People::load(conn, who).await?;
    let mut turns: Vec<Turn> = Vec::with_capacity(turn_rows.len());
    for turn in &turn_rows {
        turns.push(turn_view(turn, "", &people)?);
    }
    let all_items: Vec<Item> = item_rows
        .iter()
        .chain(&loose)
        .map(|item| item_view(item, &people))
        .collect();
    let mut beside = Beside {
        files: layout.files.map(str::to_owned),
        ..Beside::default()
    };
    let approvals: Vec<Id> = all_items
        .iter()
        .filter_map(|item| match &item.payload {
            ItemPayload::Approval(approval) => Some(approval.approval_id),
            _ => None,
        })
        .collect();
    if !approvals.is_empty() {
        for line in rows::approval_lines(conn, &approvals).await? {
            beside.approvals.insert(
                line.id,
                ApprovalFacts {
                    title: line.title,
                    decided_by: line.decided_by,
                },
            );
        }
    }
    let streams: Vec<String> = all_items
        .iter()
        .filter_map(|item| match &item.payload {
            ItemPayload::ToolCall(call) => match &call.tool {
                ToolPayload::Subagent { child_stream, .. } => Some(child_stream.clone()),
                _ => None,
            },
            _ => None,
        })
        .collect();
    if !streams.is_empty() {
        beside.sub_agents = selected::last_sub_messages(conn, thread, view, &streams).await?;
    }
    let segment_ids: Vec<Id> = turns.iter().map(|turn| turn.segment_id).collect();
    let agents = rows::segment_agents(conn, &segment_ids).await?;
    // Which turn each item is written in: its own, or for a loose line the first turn that comes after it.
    let mut highs: Vec<(Id, i64)> = Vec::new();
    for turn in &turns {
        let high = all_items
            .iter()
            .filter(|item| item.turn_id == Some(turn.id))
            .map(|item| item.seq_created)
            .max();
        if let Some(high) = high {
            highs.push((turn.id, high));
        }
    }
    let home_of = |item: &Item| -> Option<Id> {
        item.turn_id.or_else(|| {
            highs
                .iter()
                .find(|(_, high)| *high >= item.seq_created)
                .or_else(|| highs.last())
                .map(|(turn, _)| *turn)
        })
    };
    let mut parts: Vec<Part<'_>> = Vec::with_capacity(turns.len());
    for turn in &turns {
        let mut items: Vec<&Item> = all_items
            .iter()
            .filter(|item| home_of(item) == Some(turn.id))
            .collect();
        items.sort_by_key(|item| item.seq_created);
        parts.push(Part {
            turn,
            agent: agents
                .get(&turn.segment_id)
                .map_or(agent_name.as_str(), String::as_str),
            items,
        });
    }
    let texts: HashMap<i64, String> = parts
        .iter()
        .map(|part| (part.turn.ordinal, turn_text(part, &beside, detail)))
        .collect();
    let Some(budget) = budget else {
        let ordered: Vec<String> = parts
            .iter()
            .filter_map(|part| texts.get(&part.turn.ordinal).cloned())
            .collect();
        return Ok(Some(Read {
            text: conversation_piece(&head, low, high, last, &ordered, layout.heading),
            from: low,
            to: high,
            last,
            truncated: false,
        }));
    };
    let room = budget.saturating_sub(conversation(&head, low, high, last, &[]).len());
    let Some(window) = visible_window(&parts, &texts, range.0.is_some(), room) else {
        return Ok(Some(Read {
            text: format!(
                "# {}\nThread · {} · {} · no turns in that range (it has {last})\n",
                head.name, head.agent, head.state
            ),
            from: 0,
            to: 0,
            last,
            truncated: false,
        }));
    };
    let mut given: Vec<String> = parts
        .iter()
        .map(|part| part.turn.ordinal)
        .filter(|n| *n >= window.from && *n <= window.to)
        .filter_map(|n| texts.get(&n).cloned())
        .collect();
    let mut truncated = false;
    if window.cut
        && let Some(first) = given.first_mut()
    {
        let (cut, was) = cut_to_bytes(first, room.saturating_sub(1));
        *first = cut;
        truncated = was;
    }
    Ok(Some(Read {
        text: conversation_piece(&head, window.from, window.to, last, &given, layout.heading),
        from: window.from,
        to: window.to,
        last,
        truncated,
    }))
}

/// The text a chat is written as, `range` of its messages at `detail`, within `budget` bytes where that is
/// given. `title` is the first line of it. The text lies as `chat.md`: its files are named in
/// [`orch_chat::written::FILES_DIRECTORY`].
pub async fn read_chat(
    conn: &mut orch_db::Conn,
    chat: Id,
    title: &str,
    range: (Option<i64>, Option<i64>),
    detail: Detail,
    budget: Option<usize>,
) -> Result<Read, crate::error::EngineError> {
    read_chat_in(conn, chat, title, range, detail, budget, None).await
}

/// The same for a text that is kept in another file than `chat.md`: `files` is the directory beside it that holds
/// the files of the chat, as the text names it ([`files_beside`] of that file).
pub async fn read_chat_in(
    conn: &mut orch_db::Conn,
    chat: Id,
    title: &str,
    range: (Option<i64>, Option<i64>),
    detail: Detail,
    budget: Option<usize>,
    files: Option<&str>,
) -> Result<Read, crate::error::EngineError> {
    read_chat_piece(
        conn,
        chat,
        title,
        range,
        detail,
        budget,
        files,
        WrittenHead::Range,
        None,
    )
    .await
}

/// The same for one reader: each reference the messages name is written as `writings` has it for that reader, and
/// one it has not (one the reader may not read) as words that name nothing.
#[allow(
    clippy::too_many_arguments,
    reason = "a read of a chat is its range, depth, bound and files, and here the reader's terms"
)]
pub(crate) async fn read_chat_for(
    conn: &mut orch_db::Conn,
    chat: Id,
    title: &str,
    range: (Option<i64>, Option<i64>),
    detail: Detail,
    budget: Option<usize>,
    files: Option<&str>,
    writings: Option<&HashMap<Typed, Writing>>,
) -> Result<Read, crate::error::EngineError> {
    read_chat_piece(
        conn,
        chat,
        title,
        range,
        detail,
        budget,
        files,
        WrittenHead::Range,
        writings,
    )
    .await
}

/// The typed references that the messages a read of `range` looks at name in their text, in order.
pub(crate) async fn chat_references(
    conn: &mut orch_db::Conn,
    chat: Id,
    range: (Option<i64>, Option<i64>),
    budget: Option<usize>,
) -> Result<Vec<Typed>, crate::error::EngineError> {
    use orch_db::queries::shares as rows;
    let last = rows::chat_total(conn, chat).await?;
    let Some((low, high)) = candidates(range, last, budget.map(|_| CANDIDATE_MESSAGES)) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for each in rows::chat_messages_numbered(conn, chat, low, high).await? {
        let parts: Vec<orch_api_contract::chats::ChatPart> =
            serde_json::from_value(each.row.content).unwrap_or_default();
        for part in parts {
            if let orch_api_contract::chats::ChatPart::Text { text } = part {
                out.extend(references::typed_in(&text));
            }
        }
    }
    Ok(out)
}

/// One window of an archive's chat under the heading of the whole export.
pub(crate) async fn read_chat_window(
    conn: &mut orch_db::Conn,
    chat: Id,
    title: &str,
    range: (i64, i64),
    files: &str,
    heading: Option<(i64, i64)>,
) -> Result<Read, crate::error::EngineError> {
    read_chat_piece(
        conn,
        chat,
        title,
        (Some(range.0), Some(range.1)),
        Detail::Normal,
        None,
        Some(files),
        heading.map_or(WrittenHead::None, |(first, to)| {
            WrittenHead::Export(first, to)
        }),
        None,
    )
    .await
}

#[allow(
    clippy::too_many_arguments,
    reason = "one projection serves bounded reads and export windows"
)]
async fn read_chat_piece(
    conn: &mut orch_db::Conn,
    chat: Id,
    title: &str,
    range: (Option<i64>, Option<i64>),
    detail: Detail,
    budget: Option<usize>,
    files: Option<&str>,
    heading: WrittenHead,
    writings: Option<&HashMap<Typed, Writing>>,
) -> Result<Read, crate::error::EngineError> {
    use orch_chat::views::{Lookups, messages as message_views};
    use orch_chat::written::{Detail as ChatDetail, Written as ChatWritten, render, render_body};
    use orch_db::queries::{chats as chat_rows, shares as rows, threads};
    let last = rows::chat_total(conn, chat).await?;
    let none = |text: String| Read {
        text,
        from: 0,
        to: 0,
        last,
        truncated: false,
    };
    if last == 0 {
        return Ok(none(format!("# {title}\nChat · no messages yet\n")));
    }
    let bound = budget.map(|_| CANDIDATE_MESSAGES);
    let Some((low, high)) = candidates(range, last, bound) else {
        return Ok(none(format!(
            "# {title}\nChat · no messages in that range (it has {last})\n"
        )));
    };
    let numbered = rows::chat_messages_numbered(conn, chat, low, high).await?;
    let list: Vec<chat_rows::MessageRow> = numbered.iter().map(|each| each.row.clone()).collect();
    let reply_ids: Vec<Id> = list.iter().filter_map(|row| row.reply_to_id).collect();
    let reply_rows = if reply_ids.is_empty() {
        Vec::new()
    } else {
        chat_rows::messages_by_ids(conn, chat, &reply_ids).await?
    };
    let numbers = rows::chat_numbers(conn, chat, &reply_ids).await?;
    let replies: HashMap<Id, chat_rows::MessageRow> =
        reply_rows.into_iter().map(|row| (row.id, row)).collect();
    let refs: Vec<&chat_rows::MessageRow> = list.iter().chain(replies.values()).collect();
    let mut lookups = Lookups::default();
    for person in threads::people(conn, &orch_chat::views::people_of(&refs)).await? {
        lookups.people.insert(person.id, user_ref(&person));
    }
    let agent_threads = orch_chat::views::threads_of(&refs);
    if !agent_threads.is_empty() {
        for place in rows::places(conn, &agent_threads).await? {
            lookups.nodes.insert(
                place.id,
                orch_api_contract::nodes::NodeRef {
                    thread_display: None,
                    id: place.id,
                    kind: orch_api_contract::nodes::NodeKind::Thread,
                    name: Some(place.name),
                    redacted: false,
                },
            );
        }
    }
    let mut views = message_views(&list, &replies, &lookups);
    if let Some(writings) = writings {
        // The hidden ones of all the messages read are numbered as one text.
        let mut numbered = 0;
        for view in &mut views {
            for part in &mut view.parts {
                if let orch_api_contract::chats::ChatPart::Text { text } = part {
                    *text = references::for_reader(
                        text,
                        &|typed| writings.get(typed).cloned(),
                        &mut numbered,
                    );
                }
            }
        }
    }
    let by_number: HashMap<i64, usize> = numbered
        .iter()
        .enumerate()
        .map(|(at, each)| (each.number, at))
        .collect();
    let chat_detail = match detail {
        Detail::Brief => ChatDetail::Brief,
        Detail::Normal => ChatDetail::Normal,
        Detail::Full => ChatDetail::Full,
    };
    let render_range = |first: i64, end: i64| -> String {
        let slice: Vec<_> = (first..=end)
            .filter_map(|n| by_number.get(&n).and_then(|at| views.get(*at)).cloned())
            .collect();
        let written = ChatWritten {
            title,
            first,
            last,
            numbers: &numbers,
            files,
        };
        match heading {
            WrittenHead::Range => render(&written, &slice, chat_detail),
            WrittenHead::Export(first, to) => {
                let mut out = format!("# {title}\nChat · messages {first}–{to} of {last}\n");
                out.push_str(&render_body(&written, &slice, chat_detail));
                out
            }
            WrittenHead::None => render_body(&written, &slice, chat_detail),
        }
    };
    let Some(budget) = budget else {
        return Ok(Read {
            text: render_range(low, high),
            from: low,
            to: high,
            last,
            truncated: false,
        });
    };
    // A message weighs what it adds to the text: the head is the same for any of them, give or take digits.
    let head_len = render_range(low, low)
        .split_once("\n\n")
        .map_or(0, |(head, _)| head.len() + 2);
    let sizes: HashMap<i64, usize> = (low..=high)
        .map(|n| {
            let one = render_range(n, n);
            (n, one.len().saturating_sub(head_len) + 1)
        })
        .collect();
    let room = budget.saturating_sub(head_len);
    let size_of = |n: i64| sizes.get(&n).copied().unwrap_or(usize::MAX);
    let Some(window) = window(range.0, range.1, last, room, size_of) else {
        return Ok(none(format!(
            "# {title}\nChat · no messages in that range (it has {last})\n"
        )));
    };
    let text = render_range(window.from, window.to);
    let (text, truncated) = if window.cut {
        cut_to_bytes(&text, budget)
    } else {
        (text, false)
    };
    Ok(Read {
        text,
        from: window.from,
        to: window.to,
        last,
        truncated,
    })
}

// ---------------------------------------------------------------- a chat as a file (the export of a place)

/// The first line of the written form of a chat when it is taken away: `Chat of` and the name of the node it
/// belongs to, or the name of a group, or the people in a direct chat.
pub async fn chat_title(
    conn: &mut orch_db::Conn,
    chat: Id,
) -> Result<String, crate::error::EngineError> {
    use orch_db::queries::{chats as chat_rows, shares as rows, threads};
    let Some(row) = chat_rows::chat(conn, chat).await? else {
        return Ok("Chat".to_owned());
    };
    if let Some(node) = row.node_id {
        let name = threads::node_name(conn, node)
            .await?
            .unwrap_or_else(|| "a place".to_owned());
        return Ok(format!("Chat of {name}"));
    }
    if let Some(name) = row.name {
        return Ok(name);
    }
    Ok(rows::member_names(conn, &[chat])
        .await?
        .remove(&chat)
        .map_or_else(|| "Chat".to_owned(), |names| names.join(", ")))
}

/// The messages of a chat, `range` of them by their numbers, as the API types them: what a chat is written as for
/// a program. Those deleted are left out, as the written form leaves them out.
pub async fn chat_records(
    conn: &mut orch_db::Conn,
    chat: Id,
    range: (Option<i64>, Option<i64>),
) -> Result<Vec<orch_api_contract::chats::ChatMessage>, crate::error::EngineError> {
    use orch_chat::views::{Lookups, messages as message_views};
    use orch_db::queries::{chats as chat_rows, shares as rows, threads};
    let last = rows::chat_total(conn, chat).await?;
    if last == 0 {
        return Ok(Vec::new());
    }
    let Some((low, high)) = candidates(range, last, None) else {
        return Ok(Vec::new());
    };
    let numbered = rows::chat_messages_numbered(conn, chat, low, high).await?;
    let list: Vec<chat_rows::MessageRow> = numbered
        .into_iter()
        .map(|each| each.row)
        .filter(|row| !row.is_deleted())
        .collect();
    let reply_ids: Vec<Id> = list.iter().filter_map(|row| row.reply_to_id).collect();
    let reply_rows = if reply_ids.is_empty() {
        Vec::new()
    } else {
        chat_rows::messages_by_ids(conn, chat, &reply_ids).await?
    };
    let replies: HashMap<Id, chat_rows::MessageRow> =
        reply_rows.into_iter().map(|row| (row.id, row)).collect();
    let refs: Vec<&chat_rows::MessageRow> = list.iter().chain(replies.values()).collect();
    let mut lookups = Lookups::default();
    for person in threads::people(conn, &orch_chat::views::people_of(&refs)).await? {
        lookups.people.insert(person.id, user_ref(&person));
    }
    let agent_threads = orch_chat::views::threads_of(&refs);
    if !agent_threads.is_empty() {
        for place in rows::places(conn, &agent_threads).await? {
            lookups.nodes.insert(
                place.id,
                orch_api_contract::nodes::NodeRef {
                    thread_display: None,
                    id: place.id,
                    kind: orch_api_contract::nodes::NodeKind::Thread,
                    name: Some(place.name),
                    redacted: false,
                },
            );
        }
    }
    Ok(message_views(&list, &replies, &lookups))
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_types::new_id;
    use serde_json::json;

    fn person(name: &str) -> Json {
        json!({
            "id": new_id(), "kind": "human", "username": name.to_lowercase(),
            "display_name": name, "avatar": { "text": &name[..1], "color": 1 }
        })
    }

    fn turn_with(
        ordinal: i64,
        state: &str,
        started: Option<&str>,
        done: Option<&str>,
        error: Option<Json>,
    ) -> Turn {
        serde_json::from_value(json!({
            "id": new_id(), "thread_id": new_id(), "segment_id": new_id(), "ordinal": ordinal,
            "state": state, "input_id": new_id(), "model": "gpt-5", "model_display": "GPT-5",
            "effort": null, "params": {}, "error": error, "usage": null, "interrupted_by": null,
            "limit_bypass": [], "phase": null, "started_at": started, "completed_at": done,
            "seq_updated": 1
        }))
        .unwrap()
    }

    fn the_turn() -> Turn {
        turn_with(
            4,
            "completed",
            Some("2026-10-09T14:02:00.000Z"),
            Some("2026-10-09T14:05:10.000Z"),
            None,
        )
    }

    fn item(seq: i64, kind: &str, status: &str, payload: Json, author: Option<Json>) -> Item {
        serde_json::from_value(json!({
            "id": new_id(), "thread_id": new_id(), "segment_id": new_id(), "turn_id": new_id(),
            "stream": "main", "parent_item_id": null, "kind": kind, "payload": payload,
            "status": status, "author": author, "pinned": null, "chat_refs": 0,
            "seq_created": seq, "seq_updated": seq,
            "created_at": "2026-10-09T14:02:00.000Z", "updated_at": "2026-10-09T14:02:00.000Z"
        }))
        .unwrap()
    }

    fn said(seq: i64, text: &str, files: Json) -> Item {
        let mut parts = vec![json!({ "type": "text", "text": text })];
        parts.extend(files.as_array().cloned().unwrap_or_default());
        item(
            seq,
            "user_message",
            "completed",
            json!({ "parts": parts, "input_id": new_id(), "delivery": "queue", "source": "user" }),
            Some(person("Dana")),
        )
    }

    fn answer(seq: i64, text: &str) -> Item {
        item(
            seq,
            "agent_message",
            "completed",
            json!({ "text": text, "text_len": text.len(), "phase": null }),
            None,
        )
    }

    fn call(seq: i64, title: &str, status: &str, tool: Json) -> Item {
        item(
            seq,
            "tool_call",
            status,
            json!({
                "tool": tool, "title": title, "started_at": "2026-10-09T14:02:30.000Z",
                "duration_ms": 900,
                "raw": { "name": "x", "input": { "inline": "{}", "total_len": 2, "truncated": false, "blob": null },
                         "output": null, "is_error": false }
            }),
            None,
        )
    }

    fn output(text: &str) -> Json {
        json!({ "inline": text, "total_len": text.len(), "truncated": false, "blob": null })
    }

    fn command(line: &str, exit: i64, printed: &str) -> Json {
        json!({
            "kind": "command", "command": line, "cwd": "/work/app", "exit_code": exit,
            "output": output(printed), "process_id": null, "background": false, "actions": []
        })
    }

    fn the_example() -> Vec<Item> {
        vec![
            said(
                1,
                "Please reproduce the login failure on staging.",
                json!([{ "type": "file", "path": ".orch/uploads/trace.har", "name": "trace.har",
                         "size": 217_088, "blob": null }]),
            ),
            answer(2, "I will start with the SSO tests."),
            call(
                3,
                "Ran `pytest tests/login -k sso`",
                "completed",
                command("pytest tests/login -k sso", 1, "1 failed"),
            ),
            call(
                4,
                "Read `docs/sso.md`",
                "completed",
                json!({ "kind": "file_read", "path": "docs/sso.md", "start_line": 1, "end_line": 40,
                        "total_lines": 120, "content": output("# SSO"), "media_type": null }),
            ),
            call(
                5,
                "Edited `src/auth/session.py` (+12 −3)",
                "completed",
                json!({ "kind": "file_change", "changes": [{
                    "path": "src/auth/session.py", "change": "modify", "old_path": null,
                    "diff": "@@ -1,2 +1,2 @@\n-old\n+new\n", "added": 12, "removed": 3,
                    "binary": false, "truncated": false }] }),
            ),
            answer(
                6,
                "The failure reproduces: the session cookie is dropped when …",
            ),
        ]
    }

    fn head() -> Head<'static> {
        Head {
            name: "Repro",
            agent: "Codex",
            state: "idle",
        }
    }

    fn written(items: &[Item], detail: Detail) -> String {
        let turn = the_turn();
        let part = Part {
            turn: &turn,
            agent: "Codex",
            items: items.iter().collect(),
        };
        turn_text(&part, &Beside::default(), detail)
    }

    #[test]
    fn a_turn_is_written_as_what_was_said_and_one_line_for_each_step() {
        assert_eq!(
            written(&the_example(), Detail::Normal),
            "## Turn 4 · 2026-10-09T14:02Z · completed · 3 min\n\
             \n\
             **Dana (@dana):** Please reproduce the login failure on staging.\n\
             [file: trace.har · 212 KiB · conversation.files/0004/in-1-trace.har]\n\
             \n\
             **Codex:** I will start with the SSO tests.\n\
             \n\
             - Ran `pytest tests/login -k sso` · exit 1\n\
             - Read `docs/sso.md`\n\
             - Edited `src/auth/session.py` (+12 −3)\n\
             \n\
             **Codex:** The failure reproduces: the session cookie is dropped when …\n"
        );
    }

    #[test]
    fn a_conversation_has_a_head_that_says_which_turns_are_given_of_how_many() {
        let turn = the_turn();
        let items = the_example();
        let part = Part {
            turn: &turn,
            agent: "Codex",
            items: items.iter().collect(),
        };
        let text = whole(&head(), &[part], &Beside::default(), Detail::Brief);
        assert!(
            text.starts_with("# Repro\nThread · Codex · idle · turns 4–4 of 4\n\n## Turn 4 ·"),
            "{text}"
        );
        assert_eq!(
            empty(&head()),
            "# Repro\nThread · Codex · idle · no turns yet\n"
        );
    }

    #[test]
    fn in_brief_a_turn_is_a_few_lines_that_say_who_asked_how_it_ended_and_how_much_it_did() {
        assert_eq!(
            written(&the_example(), Detail::Brief),
            "## Turn 4 · 2026-10-09T14:02Z · completed · 3 min\n\
             \n\
             **Dana (@dana):** Please reproduce the login failure on staging.\n\
             **Codex:** The failure reproduces: the session cookie is dropped when …\n\
             3 steps · 1 file changed\n"
        );
        let long = answer(2, &"word ".repeat(200));
        let text = written(
            &[said(1, &"ask ".repeat(100), json!([])), long],
            Detail::Brief,
        );
        let lines: Vec<&str> = text.lines().collect();
        assert!(
            lines[2].chars().count()
                <= "**Dana (@dana):** ".chars().count() + BRIEF_ASKED_CHARS + 1
        );
        assert!(lines[3].chars().count() <= "**Codex:** ".chars().count() + BRIEF_ANSWER_CHARS + 1);
        assert!(lines[2].ends_with('…') && lines[3].ends_with('…'));
    }

    #[test]
    fn in_full_every_kind_of_step_carries_what_it_was_given_and_what_came_back() {
        let items = vec![
            said(1, "Look into it.", json!([])),
            call(
                2,
                "Ran `ls`",
                "completed",
                command("ls", 0, "a.txt\nb.txt\n"),
            ),
            call(
                3,
                "Searched for `TODO`",
                "completed",
                json!({ "kind": "file_search", "mode": "content", "query": "TODO", "path": "src",
                        "matches": [{ "path": "src/a.rs", "line": 3, "text": "// TODO fix" }],
                        "total": 25, "truncated": true }),
            ),
            call(
                4,
                "Searched the web for `rust regex`",
                "completed",
                json!({ "kind": "web_search", "query": "rust regex",
                        "results": [{ "title": "regex crate", "url": "https://docs.rs/regex" }] }),
            ),
            call(
                5,
                "Fetched https://docs.rs/regex",
                "completed",
                json!({ "kind": "web_fetch", "url": "https://docs.rs/regex", "title": "regex",
                        "status": 200, "content": output("...") }),
            ),
            call(
                6,
                "Looked at a screenshot",
                "completed",
                json!({ "kind": "image", "blob": "0".repeat(64), "media_type": "image/png",
                        "name": "shot.png", "caption": null }),
            ),
            call(
                7,
                "Asked a sub-agent",
                "completed",
                json!({ "kind": "subagent", "child_stream": "sub:1", "name": "checker", "role": null,
                        "prompt": "Check the tests", "state": "completed" }),
            ),
            call(
                8,
                "Called github.create_issue",
                "failed",
                json!({ "kind": "mcp", "server": "github", "tool": "create_issue",
                        "arguments": { "title": "x" }, "result": null, "error": "no token" }),
            ),
            call(
                9,
                "Called announce",
                "completed",
                json!({ "kind": "orch", "tool": "announce", "arguments": { "title": "Built" },
                        "result": { "announcement_id": "a" } }),
            ),
            item(
                10,
                "approval",
                "approved",
                json!({ "approval_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5001" }),
                None,
            ),
            item(
                11,
                "marker",
                "completed",
                json!({ "marker": "compacted", "data": { "duration_ms": null } }),
                None,
            ),
            item(
                12,
                "error",
                "failed",
                json!({ "code": "x", "message": "The tool broke.", "retryable": false }),
                None,
            ),
            item(
                13,
                "plan",
                "completed",
                json!({ "explanation": null, "steps": [
                    { "text": "Find it", "status": "completed" },
                    { "text": "Fix it", "status": "in_progress" },
                    { "text": "Test it", "status": "pending" }] }),
                None,
            ),
        ];
        let turn = the_turn();
        let mut beside = Beside::default();
        beside.approvals.insert(
            "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5001".parse().unwrap(),
            ApprovalFacts {
                title: "Run `rm -rf build`".to_owned(),
                decided_by: Some("Bo".to_owned()),
            },
        );
        beside
            .sub_agents
            .insert("sub:1".to_owned(), "All green.".to_owned());
        let part = Part {
            turn: &turn,
            agent: "Codex",
            items: items.iter().collect(),
        };
        let text = turn_text(&part, &beside, Detail::Full);
        let expected = "\
## Turn 4 · 2026-10-09T14:02Z · completed · 3 min

**Dana (@dana):** Look into it.

- Ran `ls`
  in `/work/app`
  ```text
  a.txt
  b.txt
  ```
- Searched for `TODO`
  looked for `TODO` in `src`
  - `src/a.rs:3` // TODO fix
  (25 matches in all; the first 20 are written)
- Searched the web for `rust regex`
  looked for `rust regex`
  - regex crate <https://docs.rs/regex>
- Fetched https://docs.rs/regex
  <https://docs.rs/regex> · status 200 · regex
- Looked at a screenshot
  [image: shot.png · conversation.files/0004/05-shot.png]
- Asked a sub-agent
  asked: Check the tests
  answered: All green.
- Called github.create_issue · failed
  arguments:
  ```json
  {
    \"title\": \"x\"
  }
  ```
  error: no token
- Called announce
  arguments:
  ```json
  {
    \"title\": \"Built\"
  }
  ```
  result:
  ```json
  {
    \"announcement_id\": \"a\"
  }
  ```
- Approval: Run `rm -rf build` · approved by Bo
- — The agent shortened its own conversation —

**Error:** The tool broke.

Plan:
- [x] Find it
- [ ] Fix it (under way)
- [ ] Test it
";
        assert_eq!(text, expected);
    }

    #[test]
    fn a_change_is_written_as_its_difference_and_a_read_by_its_lines_not_its_text() {
        let text = written(&the_example(), Detail::Full);
        assert!(
            text.contains(
                "- Read `docs/sso.md`\n  lines 1–40 of 120\n- Edited `src/auth/session.py` (+12 −3)\n  `src/auth/session.py` (modify, +12 −3)\n  ```diff\n  @@ -1,2 +1,2 @@\n  -old\n  +new\n  ```\n"
            ),
            "{text}"
        );
        assert!(
            !text.contains("# SSO"),
            "the text of a file that was read is not written"
        );
    }

    #[test]
    fn what_a_step_printed_is_cut_at_its_end_and_the_cut_names_the_file_that_holds_the_whole() {
        let printed: String = (0..STEP_TEXT_CHARS + 500)
            .map(|n| char::from(b'a' + u8::try_from(n % 26).unwrap()))
            .collect();
        let items = vec![call(
            3,
            "Ran `make`",
            "completed",
            command("make", 0, &printed),
        )];
        let text = written(&items, Detail::Full);
        let kept: String = printed.chars().skip(500).collect();
        assert!(text.contains(&kept), "the end of the output is kept");
        assert!(!text.contains(&printed), "and not all of it");
        assert!(
            text.contains("(cut; the whole is in conversation.files/0004/01-output.txt)"),
            "{text}"
        );
    }

    #[test]
    fn a_difference_is_cut_at_its_beginning_and_names_the_file_that_holds_it_whole() {
        let diff = format!("@@\n{}", "+x\n".repeat(STEP_DIFF_CHARS));
        let items = vec![call(
            3,
            "Edited `a`",
            "completed",
            json!({ "kind": "file_change", "changes": [{
                "path": "a", "change": "add", "old_path": null, "diff": diff,
                "added": 1, "removed": 0, "binary": false, "truncated": false }] }),
        )];
        let text = written(&items, Detail::Full);
        assert!(text.contains("(cut; the whole is in conversation.files/0004/01.diff)"));
        assert!(text.chars().count() < STEP_DIFF_CHARS * 2);
    }

    #[test]
    fn reasoning_and_what_sub_agents_did_are_never_written() {
        let mut inside = answer(5, "A sub-agent's words");
        inside.stream = "sub:1".to_owned();
        let thought = item(
            4,
            "reasoning",
            "completed",
            json!({ "summary": "I think the cookie is dropped", "text_len": 30, "hidden": false }),
            None,
        );
        let items = vec![said(1, "Go", json!([])), thought, inside, answer(6, "Done")];
        for detail in [Detail::Brief, Detail::Normal, Detail::Full] {
            let text = written(&items, detail);
            assert!(!text.contains("cookie"), "{detail:?}: {text}");
            assert!(!text.contains("A sub-agent's words"), "{detail:?}: {text}");
        }
    }

    #[test]
    fn a_turn_says_how_it_ended_and_a_failed_one_why() {
        let failed = turn_with(
            5,
            "failed",
            Some("2026-10-09T14:00:00.000Z"),
            Some("2026-10-09T14:00:40.000Z"),
            Some(
                json!({ "code": "provider_exhausted", "message": "The provider has no credit left.\nTry later.", "retryable": true }),
            ),
        );
        let part = Part {
            turn: &failed,
            agent: "Codex",
            items: Vec::new(),
        };
        assert_eq!(
            turn_text(&part, &Beside::default(), Detail::Normal)
                .lines()
                .next(),
            Some(
                "## Turn 5 · 2026-10-09T14:00Z · failed: The provider has no credit left. Try later. · 40 s"
            )
        );
        let interrupted = turn_with(
            6,
            "interrupted",
            Some("2026-10-09T14:00:00.000Z"),
            None,
            None,
        );
        let running = turn_with(7, "running", Some("2026-10-09T14:00:00.000Z"), None, None);
        for (turn, word) in [(&interrupted, "interrupted"), (&running, "running")] {
            let part = Part {
                turn,
                agent: "Codex",
                items: Vec::new(),
            };
            let head = turn_text(&part, &Beside::default(), Detail::Normal);
            assert_eq!(
                head.lines().next().unwrap().rsplit(" · ").next(),
                Some(word)
            );
        }
    }

    #[test]
    fn an_announcement_and_a_message_from_another_thread_are_written_with_their_senders() {
        let notice = item(
            2,
            "announcement",
            "completed",
            json!({ "announcement_id": new_id(), "category": "notice", "to_thread": false,
                    "title": "API changed", "body": "v2 is out", "severity": "info",
                    "from": { "thread": { "id": new_id(), "name": "Docs" } } }),
            None,
        );
        let message = item(
            3,
            "announcement",
            "completed",
            json!({ "announcement_id": new_id(), "category": "notice", "to_thread": true,
                    "title": "", "body": "The build is in ./out", "severity": "info",
                    "from": { "thread": { "id": new_id(), "name": "Build" } } }),
            None,
        );
        let text = written(&[notice, message], Detail::Normal);
        assert!(text.contains("**Docs (announcement):** API changed: v2 is out"));
        assert!(text.contains("**Build (message):** The build is in ./out"));
    }

    #[test]
    fn the_files_of_a_text_lie_in_the_directory_its_file_names_and_a_version_names_the_documents() {
        for (file, directory) in [
            ("conversation.md", "conversation.files"),
            ("Acme/Web/chat.md", "chat.files"),
            ("docs/Handbook.md", "Handbook.files"),
            ("docs/Release v1.2.md", "Release v1.2.files"),
            ("docs/Plan~ab12.md", "Plan~ab12.files"),
            ("conversations/Fix login.json", "Fix login.files"),
            ("README", "README.files"),
            // An earlier version lies one directory below the document, and its links go one up.
            (
                "docs/Plan.history/0003 2026-10-09T14-02Z.md",
                "../Plan.files",
            ),
            ("Plan.history/0001 2026-10-09T14-02Z.json", "../Plan.files"),
        ] {
            assert_eq!(files_beside(file), directory, "{file}");
        }
    }

    #[test]
    fn a_conversation_names_the_files_of_its_turns_in_the_directory_it_is_given() {
        let files =
            json!([{ "type": "file", "path": "a", "name": "a.txt", "size": 10, "blob": null }]);
        let item = said(1, "Here", files);
        let turn = the_turn();
        let part = Part {
            turn: &turn,
            agent: "Codex",
            items: vec![&item],
        };
        let beside = Beside {
            files: Some(files_beside("conversations/Fix login.md")),
            ..Beside::default()
        };
        let text = turn_text(&part, &beside, Detail::Normal);
        assert!(
            text.contains("[file: a.txt · 10 B · Fix login.files/0004/in-1-a.txt]"),
            "{text}"
        );
        let text = turn_text(&part, &Beside::default(), Detail::Normal);
        assert!(
            text.contains("[file: a.txt · 10 B · conversation.files/0004/in-1-a.txt]"),
            "and as `conversation.md` where none is given: {text}"
        );
    }

    #[test]
    fn the_files_an_input_carried_are_numbered_in_the_order_they_came() {
        let files = json!([
            { "type": "file", "path": "a", "name": "a.txt", "size": 10, "blob": null },
            { "type": "image", "blob": "0".repeat(64), "media_type": "image/png", "name": "b/c.png" }
        ]);
        let text = written(&[said(1, "Here", files)], Detail::Normal);
        assert!(text.contains("[file: a.txt · 10 B · conversation.files/0004/in-1-a.txt]"));
        assert!(text.contains("[image: b/c.png · conversation.files/0004/in-2-b_c.png]"));
    }

    #[test]
    fn retained_ordinal_gaps_do_not_hide_an_earlier_part_that_fits_the_written_window() {
        let first = turn_with(1, "completed", None, None, None);
        let last = turn_with(3, "completed", None, None, None);
        let before = answer(1, "Retained first answer");
        let after = answer(30, "Selected later answer");
        let parts = [
            Part {
                turn: &first,
                agent: "Agent",
                items: vec![&before],
            },
            Part {
                turn: &last,
                agent: "Agent",
                items: vec![&after],
            },
        ];
        let texts: HashMap<_, _> = parts
            .iter()
            .map(|part| {
                (
                    part.turn.ordinal,
                    turn_text(part, &Beside::default(), Detail::Normal),
                )
            })
            .collect();
        let selected = visible_window(&parts, &texts, false, usize::MAX).unwrap();
        assert_eq!(
            selected,
            Window {
                from: 1,
                to: 3,
                cut: false
            }
        );
        let text = conversation_piece(
            &head(),
            selected.from,
            selected.to,
            3,
            &parts
                .iter()
                .map(|part| texts[&part.turn.ordinal].clone())
                .collect::<Vec<_>>(),
            WrittenHead::Range,
        );
        assert!(text.contains("Retained first answer") && text.contains("Selected later answer"));
        assert!(
            !text.contains("## Turn 2"),
            "another suffix's turn is neither invented nor renumbered"
        );
    }

    #[test]
    fn the_turns_that_fit_are_the_latest_ones_whole_and_a_first_turn_too_large_is_given_cut() {
        let sizes = [100_usize, 100, 100, 100];
        let size = |n: i64| sizes[usize::try_from(n - 1).unwrap()];
        assert_eq!(
            window(None, None, 4, 250, size),
            Some(Window {
                from: 3,
                to: 4,
                cut: false
            })
        );
        assert_eq!(
            window(Some(2), None, 4, 250, size),
            Some(Window {
                from: 2,
                to: 3,
                cut: false
            })
        );
        assert_eq!(
            window(None, Some(3), 4, 250, size),
            Some(Window {
                from: 2,
                to: 3,
                cut: false
            })
        );
        assert_eq!(
            window(Some(-3), Some(-2), 4, 1_000, size),
            Some(Window {
                from: 2,
                to: 3,
                cut: false
            })
        );
        assert_eq!(
            window(Some(1), Some(4), 4, 50, size),
            Some(Window {
                from: 1,
                to: 1,
                cut: true
            })
        );
        assert_eq!(window(Some(9), None, 4, 250, size), None);
        assert_eq!(window(None, None, 0, 250, size), None);
    }

    #[test]
    fn a_text_is_cut_at_a_character_and_never_inside_one() {
        let (cut, was) = cut_to_bytes("héllo wörld", 2);
        assert_eq!((cut.as_str(), was), ("h", true));
        let (whole, was) = cut_to_bytes("héllo", 100);
        assert_eq!((whole.as_str(), was), ("héllo", false));
    }

    #[test]
    fn a_duration_is_said_in_the_words_of_a_person() {
        assert_eq!(duration_text(40), "40 s");
        assert_eq!(duration_text(190), "3 min");
        assert_eq!(duration_text(7_500), "2 h 5 min");
        assert_eq!(duration_text(7_200), "2 h");
    }

    #[test]
    fn history_words_do_not_repeat_raw_identifiers_from_a_page_change_and_name_a_removed_thread() {
        let id = new_id();
        let marker:MarkerPayload=serde_json::from_value(json!({"marker":"access_changed","data":{"by":person("Dana"),"summary":format!("named the agents of {id} in a row")}})).unwrap();
        let text = marker_words(&marker);
        assert!(text.contains("changed who may use this thread"));
        assert!(!text.contains(&id.to_string()));
        let fork:MarkerPayload=serde_json::from_value(json!({"marker":"forked_from","data":{"by":person("Dana"),"thread":{"id":id,"kind":"thread","name":null,"redacted":true},
            "turn_id":new_id(),"report":{"fidelity":"lossless","items_total":0,"by_kind":{},"dropped":[],"warnings":[]}}})).unwrap();
        assert!(marker_words(&fork).contains("a removed thread"));
        assert!(!marker_words(&fork).contains(&id.to_string()));
    }
}

// ---------------------------------------------------------------- items picked out of a conversation

/// The most bytes of the text of one part of a forward: what a part may hold.
pub const FORWARD_PART_BYTES: usize = orch_api_contract::chats::FORWARD_PART_MAX_BYTES as usize;

/// What marks a part that was cut to fit.
const CUT_MARK: &str = "\n(cut to fit)\n";

/// One part of a forward of items of a conversation: what one person said, or what the agent said and did in one
/// turn, its items in their order, written as the conversation is written in full.
#[derive(Debug, Clone, PartialEq)]
pub struct ForwardPart {
    /// The person who said it, if a person did.
    pub person: Option<orch_api_contract::users::UserRef>,
    /// The name of the agent that said and did it, if the agent did.
    pub agent: Option<String>,
    /// When the first of its items was made.
    pub at: Time,
    /// The first of its items.
    pub source: Id,
    /// What it comes to in words.
    pub text: String,
    /// The images a person's message carried.
    pub images: Vec<orch_api_contract::threads::ForwardedImage>,
}

/// The items `selected` of the conversation of `thread` as the parts of a forward: each message of a person is a
/// part, and what the agent said and did in one turn is one part, in the order they were made.
pub async fn forward_parts(
    conn: &mut orch_db::Conn,
    thread: Id,
    selected: &[Id],
) -> Result<Vec<ForwardPart>, crate::error::EngineError> {
    use crate::views::{People, item_view, turn_view};
    use orch_db::queries::{shares as rows, threads};
    let item_rows = rows::items_by_ids(conn, thread, selected).await?;
    let mut who: Vec<Id> = Vec::new();
    for row in &item_rows {
        who.extend(row.author_id);
    }
    let people = People::load(conn, who).await?;
    let mut turn_ids: Vec<Id> = item_rows.iter().filter_map(|row| row.turn_id).collect();
    turn_ids.sort_unstable();
    turn_ids.dedup();
    let mut turns: HashMap<Id, Turn> = HashMap::new();
    for id in &turn_ids {
        if let Some(turn) = threads::turn(conn, *id).await? {
            turns.insert(*id, turn_view(&turn, "", &people)?);
        }
    }
    let segments: Vec<Id> = turns.values().map(|turn| turn.segment_id).collect();
    let agents = rows::segment_agents(conn, &segments).await?;
    let items: Vec<Item> = item_rows
        .iter()
        .map(|row| item_view(row, &people))
        .collect();
    let beside = Beside::default();
    let mut parts: Vec<ForwardPart> = Vec::new();
    let mut at = 0;
    while at < items.len() {
        let item = &items[at];
        match (
            &item.payload,
            item.turn_id.and_then(|turn| turns.get(&turn)),
        ) {
            (ItemPayload::UserMessage(message), _) => {
                let mut texts: Vec<&str> = Vec::new();
                let mut images = Vec::new();
                for part in &message.parts {
                    match part {
                        ContentPart::Text { text } if !text.trim().is_empty() => {
                            texts.push(text.trim());
                        }
                        ContentPart::Image {
                            blob,
                            media_type,
                            name,
                            ..
                        } => images.push(orch_api_contract::threads::ForwardedImage {
                            blob: *blob,
                            media_type: serde_json::to_value(media_type)
                                .ok()
                                .and_then(|word| word.as_str().map(str::to_owned))
                                .unwrap_or_default(),
                            name: name.clone(),
                        }),
                        _ => {}
                    }
                }
                parts.push(ForwardPart {
                    person: item.author.clone(),
                    agent: None,
                    at: item.created_at,
                    source: item.id,
                    text: texts.join("\n\n"),
                    images,
                });
                at += 1;
            }
            (_, Some(turn)) => {
                let name = agents
                    .get(&turn.segment_id)
                    .cloned()
                    .unwrap_or_else(|| "Agent".to_owned());
                let first = at;
                while at < items.len()
                    && items[at].turn_id == Some(turn.id)
                    && !matches!(items[at].payload, ItemPayload::UserMessage(_))
                {
                    at += 1;
                }
                let group: Vec<&Item> = items[first..at].iter().collect();
                let part = Part {
                    turn,
                    agent: &name,
                    items: group,
                };
                let full = turn_text(&part, &beside, Detail::Full);
                // The heading of the turn says which turn it was; a part of a forward says who and when.
                let body = full
                    .split_once('\n')
                    .map_or(full.as_str(), |(_, rest)| rest)
                    .trim_start_matches('\n')
                    .to_owned();
                let (text, was_cut) = cut_to_bytes(&body, FORWARD_PART_BYTES - CUT_MARK.len());
                parts.push(ForwardPart {
                    person: None,
                    agent: Some(name),
                    at: items[first].created_at,
                    source: items[first].id,
                    text: if was_cut {
                        format!("{text}{CUT_MARK}")
                    } else {
                        text
                    },
                    images: Vec::new(),
                });
            }
            (_, None) => {
                // An item that belongs to no turn is a line of its own.
                let text = match &item.payload {
                    ItemPayload::Announcement(_) => announcement_text(item).unwrap_or_default(),
                    ItemPayload::Error(error) => format!("**Error:** {}", error.message.trim()),
                    _ => String::new(),
                };
                if !text.is_empty() {
                    parts.push(ForwardPart {
                        person: None,
                        agent: None,
                        at: item.created_at,
                        source: item.id,
                        text,
                        images: Vec::new(),
                    });
                }
                at += 1;
            }
        }
    }
    Ok(parts)
}
