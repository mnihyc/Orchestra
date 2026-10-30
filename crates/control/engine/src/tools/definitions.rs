//! Orch's tools as an agent is offered them: names, descriptions, closed schemas, titles and hints; a refusal as an answer with a code and what to do instead; and one rate for all of them.
//!
//! The tools are the `orch` server's, and every thread's agent is offered the same by their definitions here, which
//! are one for each of [`OrchTool::ALL`], of which a thread has those its settings enable and the engine has built
//! ([`BUILT`]). Each definition is what the
//! protocol the tools are offered through lets a tool declare: a name that says the verb and the kind, a title, a
//! description written for somebody new, a schema that keeps out every call it can ([`ToolDefinition::input_schema`]
//! is a plain object that lists its arguments and allows no other), and the hints of its annotations, which say
//! what is true of it: whether it only reads, whether it can destroy, whether doing it twice is doing it once. None
//! of them reaches outside Orch.
//!
//! **A description is a text like any other that Orch says to an agent**, and a place may replace it: the key of the
//! text is `tool_` and the tool's name in `agent.messages` ([`message_key`]). Nothing else about a definition is a
//! setting.
//!
//! **A call that cannot be done is answered, not failed.** A refusal is a result marked as an error, so that the
//! model sees it and can correct itself, and it is one line that begins with its code and says what to do instead
//! ([`Refusal`]). What a schema cannot say (which arguments a `command` takes, how long a list may be before it is
//! too long) is checked at the call by [`check_arguments`], which reads the schema the model was shown, so that the
//! model is never refused for a rule it was not told.
//!
//! **One rate for all of them.** One thread's agent may make `orch_tools.calls_per_minute` calls of these tools in
//! a minute; beyond it a call answers `rate_limited` with the seconds to wait ([`CallRate`]). The window is kept in
//! memory: it exists to stop a loop, and a process that starts anew forgets at most a minute of it.

use crate::tools::texts;
use chrono::{DateTime, Duration, Utc};
use orch_agent_contract::events::OrchTool;
use orch_api_contract::documents::DOCUMENT_DESCRIPTION_MAX_CHARS;
use orch_types::{Id, Json};
use serde_json::json;
use std::collections::{BTreeMap, HashMap, VecDeque};

/// The name of the MCP server the tools are the tools of.
pub const SERVER_NAME: &str = orch_agent_contract::loopback::ORCH_SERVER;

/// The tools the engine has built that a thread may enable. A thread is offered those of `orch_tools.enabled` that are
/// among them, and no others: a tool whose behaviour another part writes joins this list when that part does. `goal` is
/// not among them, since a goal gives it and a setting does not.
pub const BUILT: &[OrchTool] = &[
    OrchTool::ListShared,
    OrchTool::ReadShared,
    OrchTool::SearchShared,
    OrchTool::DownloadShared,
    OrchTool::EditDocument,
    OrchTool::ManageShared,
    OrchTool::SendMessage,
    OrchTool::Announce,
    OrchTool::RequestCapability,
    OrchTool::UseBrowser,
    OrchTool::UseDisplay,
    OrchTool::ListPorts,
    OrchTool::OpenPort,
    OrchTool::ClosePort,
    OrchTool::ListPortConnections,
];

/// What the key of a tool's description in `agent.messages` begins with.
pub const MESSAGE_KEY_PREFIX: &str = "tool_";

/// The most characters of an announcement's title: what the column holds.
pub const TITLE_MAX_CHARS: usize = 120;
/// The most characters of an announcement's body. The column holds more, for a message to one thread; an
/// announcement is kept shorter so that what a model is handed from another agent stays short.
pub const BODY_MAX_CHARS: usize = 1_500;
/// The most characters of the text of a message to a chat or a thread.
pub const TEXT_MAX_CHARS: usize = 8_000;
/// The most files one message carries.
pub const FILES_MAX: usize = 10;
/// The longest reason that goes with a request for a capability: one sentence for a person.
pub const REASON_MAX_CHARS: usize = 1_000;
/// The longest query a search takes.
pub const QUERY_MAX_CHARS: usize = 1_000;
/// The deepest a listing goes.
pub const DEPTH_MAX: i64 = 4;
/// The most edits one `batch` of `edit_document` holds: a change of more places than this is a rewrite.
pub const BATCH_EDITS_MAX: usize = 100;
/// The most paths one call of `manage_shared` names: removing is rarely wanted in bulk, and a mistake in bulk is
/// the costly one.
pub const MANAGE_PATHS_MAX: usize = 20;
/// The fewest and the most bytes a read of an entry returns.
pub const READ_BYTES_MIN: i64 = 1_000;
/// The most bytes a read of an entry returns: under what a harness passes on whole.
pub const READ_BYTES_MAX: i64 = 40_000;
/// The most hits a search returns.
pub const RESULTS_MAX: i64 = 200;
/// The most lines around a hit.
pub const CONTEXT_MAX: i64 = 5;
/// The highest port number.
pub const PORT_MAX: i64 = 65_535;
/// The most rows a page of an agent list holds.
pub const PAGE_MAX: i64 = 200;
/// The most characters of a link's label.
pub const PORT_LABEL_MAX_CHARS: usize = 80;

/// What a definition says is true of a tool, in the terms of the protocol: the hints of its annotations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hints {
    /// It changes nothing.
    pub read_only: bool,
    /// It can destroy: said of a tool that changes something.
    pub destructive: Option<bool>,
    /// Doing it twice is doing it once: said of a tool that changes something.
    pub idempotent: Option<bool>,
}

/// One tool as an agent is offered it.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolDefinition {
    /// Which tool.
    pub tool: OrchTool,
    /// Its name.
    pub name: &'static str,
    /// What it is called for a person.
    pub title: &'static str,
    /// What it is told to the model: the shipped text, or the place's own.
    pub description: String,
    /// The schema of its arguments: a plain object that allows no other.
    pub input_schema: Json,
    /// What is true of it.
    pub hints: Hints,
}

impl ToolDefinition {
    /// The definition as the protocol lists a tool.
    #[must_use]
    pub fn to_mcp(&self) -> Json {
        let mut annotations = serde_json::Map::new();
        annotations.insert("title".to_owned(), json!(self.title));
        annotations.insert("readOnlyHint".to_owned(), json!(self.hints.read_only));
        if let Some(destructive) = self.hints.destructive {
            annotations.insert("destructiveHint".to_owned(), json!(destructive));
        }
        if let Some(idempotent) = self.hints.idempotent {
            annotations.insert("idempotentHint".to_owned(), json!(idempotent));
        }
        annotations.insert(
            "openWorldHint".to_owned(),
            json!(self.tool == OrchTool::UseBrowser),
        );
        json!({
            "name": self.name,
            "title": self.title,
            "description": self.description,
            "inputSchema": self.input_schema,
            "annotations": Json::Object(annotations),
        })
    }
}

/// The key under which a place may replace the description of a tool in `agent.messages`.
#[must_use]
pub fn message_key(tool: OrchTool) -> String {
    format!("{MESSAGE_KEY_PREFIX}{}", tool.name())
}

/// The shipped description of a tool.
#[must_use]
pub const fn shipped_description(tool: OrchTool) -> &'static str {
    match tool {
        OrchTool::ListShared => texts::LIST_SHARED,
        OrchTool::ReadShared => texts::READ_SHARED,
        OrchTool::SearchShared => texts::SEARCH_SHARED,
        OrchTool::DownloadShared => texts::DOWNLOAD_SHARED,
        OrchTool::EditDocument => texts::EDIT_DOCUMENT,
        OrchTool::ManageShared => texts::MANAGE_SHARED,
        OrchTool::SendMessage | OrchTool::SendChatMessage | OrchTool::SendThreadMessage => {
            texts::SEND_MESSAGE
        }
        OrchTool::Announce => texts::ANNOUNCE,
        OrchTool::RequestCapability => texts::REQUEST_CAPABILITY,
        OrchTool::UseBrowser => super::browser_definition::DESCRIPTION,
        OrchTool::UseDisplay => super::display::DESCRIPTION,
        OrchTool::ListPorts => texts::LIST_PORTS,
        OrchTool::OpenPort => texts::OPEN_PORT,
        OrchTool::ClosePort => texts::CLOSE_PORT,
        OrchTool::ListPortConnections => texts::LIST_PORT_CONNECTIONS,
        OrchTool::Goal => texts::GOAL,
    }
}

const fn title_of(tool: OrchTool) -> &'static str {
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

/// What is true of each tool. The first three and the two lists of ports change nothing; a download adds a file and
/// replaces none that differs, and can be asked again; an edit replaces what it touches, though an earlier version
/// stays; managing removes or moves a document, though it can be brought back; the two that send add; a grant
/// recreates the environment and stops what runs in it; asking to open a port again gives the same opening, and
/// closing one takes people's way in away, once.
const fn hints_of(tool: OrchTool) -> Hints {
    const READS: Hints = Hints {
        read_only: true,
        destructive: None,
        idempotent: None,
    };
    match tool {
        OrchTool::ListShared
        | OrchTool::ReadShared
        | OrchTool::SearchShared
        | OrchTool::ListPorts
        | OrchTool::ListPortConnections => READS,
        OrchTool::OpenPort => writes(false, true),
        OrchTool::ClosePort => writes(true, true),
        OrchTool::DownloadShared => writes(false, true),
        OrchTool::EditDocument
        | OrchTool::ManageShared
        | OrchTool::RequestCapability
        | OrchTool::UseBrowser
        | OrchTool::UseDisplay => writes(true, false),
        OrchTool::SendMessage
        | OrchTool::SendChatMessage
        | OrchTool::SendThreadMessage
        | OrchTool::Goal
        | OrchTool::Announce => writes(false, false),
    }
}

/// The hints of a tool that changes something.
const fn writes(destructive: bool, idempotent: bool) -> Hints {
    Hints {
        read_only: false,
        destructive: Some(destructive),
        idempotent: Some(idempotent),
    }
}

// ---------------------------------------------------------------- the schemas

fn string(description: &str) -> Json {
    json!({ "type": "string", "description": description })
}

fn bounded_string(description: &str, min: usize, max: usize) -> Json {
    json!({ "type": "string", "description": description, "minLength": min, "maxLength": max })
}

fn integer(description: &str) -> Json {
    json!({ "type": "integer", "description": description })
}

fn bounded_integer(description: &str, min: i64, max: i64) -> Json {
    json!({ "type": "integer", "description": description, "minimum": min, "maximum": max })
}

fn boolean(description: &str) -> Json {
    json!({ "type": "boolean", "description": description })
}

fn choice(description: &str, words: &[&str]) -> Json {
    json!({ "type": "string", "description": description, "enum": words })
}

fn list_of(description: &str, items: Json, max: Option<usize>) -> Json {
    let mut schema = json!({ "type": "array", "description": description, "items": items });
    if let Some(max) = max {
        schema["maxItems"] = json!(max);
    }
    schema
}

/// A list whose entries differ.
fn unique(mut schema: Json) -> Json {
    schema["uniqueItems"] = json!(true);
    schema
}

/// How many rows a page of an agent list holds.
fn page_limit() -> Json {
    bounded_integer("How many rows: 50 when left out.", 1, PAGE_MAX)
}

/// Where a list that was cut goes on.
fn page_cursor() -> Json {
    string("Where a list that was cut goes on: the cursor it gave.")
}

/// A list of at least `min` and at most `max` entries.
fn list_between(description: &str, items: Json, min: usize, max: usize) -> Json {
    let mut schema = list_of(description, items, Some(max));
    schema["minItems"] = json!(min);
    schema
}

fn object(properties: Vec<(&str, Json)>, required: &[&str]) -> Json {
    let mut members = serde_json::Map::new();
    for (name, schema) in properties {
        members.insert(name.to_owned(), schema);
    }
    members.entry("max_bytes".to_owned()).or_insert_with(|| {
        bounded_integer(
            "The most UTF-8 bytes in the complete answer: 20000 when left out, at most 40000.",
            READ_BYTES_MIN,
            READ_BYTES_MAX,
        )
    });
    members.insert("ids".to_owned(), boolean("Include optional opaque object ids; false when left out. Required references and positions remain."));
    json!({
        "type": "object",
        "properties": Json::Object(members),
        "required": required,
        "additionalProperties": false,
    })
}

/// The schema of `goal`: a command, and for an update the status with what shows it and what comes next.
fn goal_schema() -> Json {
    json!({
        "type": "object",
        "properties": {
            "command": choice("get reads the goal; update reports where you stand.", &["get", "update"]),
            "status": choice(
                "With update: working while you go on, met when the condition holds, blocked when only a person can go on.",
                &["working", "met", "blocked"],
            ),
            "evidence": bounded_string(
                "With update and status met or blocked, required: what you ran or read and what it showed, or what stops you.",
                0,
                crate::goals::EVIDENCE_MAX_CHARS,
            ),
            "next": bounded_string(
                "With update: what you will do next, or what you need from a person.",
                0,
                crate::goals::NEXT_MAX_CHARS,
            ),
        },
        "required": ["command"],
        "additionalProperties": false,
    })
}

fn input_schema_of(tool: OrchTool) -> Json {
    match tool {
        OrchTool::ListShared => object(
            vec![
                ("limit", page_limit()),
                ("cursor", page_cursor()),
                (
                    "sort",
                    choice(
                        "The order within each directory: active work first, then most recently changed (the default), or names.",
                        &["changed", "name"],
                    ),
                ),
                (
                    "include_archived",
                    boolean(
                        "Include archived entries and their readable contents; otherwise one count says how many were folded.",
                    ),
                ),
                (
                    "path",
                    string(
                        "The directory to list: a path of the tree, from \"/\" or from your own topic, or id: and an id. Your own topic when left out.",
                    ),
                ),
                (
                    "depth",
                    bounded_integer(
                        "How many levels down to list. 1 when left out.",
                        1,
                        DEPTH_MAX,
                    ),
                ),
                (
                    "kinds",
                    list_of(
                        "Keep only entries of these kinds.",
                        json!({ "type": "string", "enum": ["org", "project", "topic", "thread", "conversation", "chat", "folder", "document", "version", "file"] }),
                        None,
                    ),
                ),
                (
                    "names",
                    string(
                        "A pattern over entry names, as a shell has them: * ? [...]. Only entries whose name matches are listed.",
                    ),
                ),
            ],
            &[],
        ),
        OrchTool::ReadShared => object(
            vec![
                (
                    "path",
                    string("The entry to read: its path in the tree, or id: and an id."),
                ),
                (
                    "from",
                    integer("The first part to read. A negative number counts from the end."),
                ),
                (
                    "to",
                    integer("The last part to read. A negative number counts from the end."),
                ),
                (
                    "detail",
                    choice(
                        "How much of each part: brief, normal (the default) or full.",
                        &["brief", "normal", "full"],
                    ),
                ),
                (
                    "max_bytes",
                    bounded_integer(
                        "The most bytes of text to return. 20000 when left out.",
                        READ_BYTES_MIN,
                        READ_BYTES_MAX,
                    ),
                ),
            ],
            &["path"],
        ),
        // One of query, related_to and referenced_by is given, and the call says so when it is not: no union at the root
        // of a schema, which not every program that offers a tool to its model reads.
        OrchTool::SearchShared => object(
            vec![
                (
                    "related_to",
                    string(
                        "Find items related to a readable document, by shared path or id:<id>. Use instead of query; for example related_to: \"docs/Rollout plan.md\".",
                    ),
                ),
                (
                    "referenced_by",
                    string(
                        "List what refers to a readable document, by shared path or id:<id>, with the lines around the mentions. Use instead of query; for example referenced_by: \"docs/Rollout plan.md\".",
                    ),
                ),
                (
                    "query",
                    bounded_string(
                        "Words in any order; double quotes keep a phrase together. With regex, a regular expression instead. Needed unless related_to or referenced_by is given.",
                        1,
                        QUERY_MAX_CHARS,
                    ),
                ),
                (
                    "regex",
                    boolean(
                        "Whether query is a regular expression, without look-around and without back-references.",
                    ),
                ),
                (
                    "path",
                    string("A directory to look under. Everything you can reach when left out."),
                ),
                (
                    "kinds",
                    list_of(
                        "Which kinds to look in. All four when left out.",
                        json!({ "type": "string", "enum": ["documents", "chats", "conversations", "files"] }),
                        None,
                    ),
                ),
                (
                    "names",
                    string(
                        "A pattern over entry names: only entries whose name matches are looked at.",
                    ),
                ),
                (
                    "max_results",
                    bounded_integer(
                        "At most this many entries. Ordinary search defaults to 50; related lists also respect related.count.",
                        1,
                        RESULTS_MAX,
                    ),
                ),
                ("cursor", page_cursor()),
                (
                    "snippets",
                    boolean(
                        "Include text snippets with native positions. False when left out; summaries always include match counts and first positions.",
                    ),
                ),
                (
                    "context",
                    bounded_integer(
                        "How many lines around a match a snippet carries. 1 when left out.",
                        0,
                        CONTEXT_MAX,
                    ),
                ),
            ],
            &[],
        ),
        OrchTool::DownloadShared => object(
            vec![
                (
                    "path",
                    string(
                        "The entry or attachment to copy: the exact shared path from list_shared or read_shared, or id: and an id.",
                    ),
                ),
                (
                    "save_to",
                    string(
                        "Where to write it under your directory. When left out, .orch/shared/downloads/<canonical filename>; a file keeps its original bytes and extension.",
                    ),
                ),
                (
                    "detail",
                    choice(
                        "For an entry that is no file: brief, normal (the default) or full.",
                        &["brief", "normal", "full"],
                    ),
                ),
                (
                    "format",
                    choice(
                        "For an entry that is no file: markdown (the default), or json for a program to read.",
                        &["markdown", "json"],
                    ),
                ),
            ],
            &["path"],
        ),
        OrchTool::EditDocument => object(
            vec![
                (
                    "command",
                    choice(
                        "What to do: create, rewrite, replace, insert, rename, describe, attach, detach, block_replace, block_insert_after, block_delete, block_move_after, batch or import.",
                        &[
                            "create",
                            "rewrite",
                            "replace",
                            "insert",
                            "rename",
                            "describe",
                            "attach",
                            "detach",
                            "block_replace",
                            "block_insert_after",
                            "block_delete",
                            "block_move_after",
                            "batch",
                            "import",
                        ],
                    ),
                ),
                (
                    "path",
                    string(
                        "One text document path or id. For create: a new path in an existing writable docs root or document folder, ending in the title and .md. For import: a writable docs directory or document folder the documents go into. Use manage_shared for folders and copies.",
                    ),
                ),
                (
                    "markdown",
                    string(
                        "The text, as Orch Markdown: for create, rewrite, insert, block_replace and block_insert_after; for create, rewrite and insert, file may be given in its place. Orch Markdown is GitHub Markdown with $x$ formulas, $$ blocks, ```mermaid diagrams, callouts as alerts (a line \"> [!WARNING]\" and then the words as a quote; NOTE, TIP, WARNING or CAUTION), and a closed set of tags, each on a line of its own between blank lines: a toggle is \"<details>\", \"<summary>Title</summary>\", its blocks, \"</details>\"; columns are \"<columns>\" holding \"<column ratio=\"2\">\" and \"<column>\" with their blocks, each closed; inside a line <span color=\"red\" background=\"amber\">words</span> and <u>words</u> (colours: red, orange, amber, lime, green, teal, cyan, blue, indigo, purple, pink, grey); \"<video src=\"https://example.org/a.mp4\">caption</video>\", the same for <audio>; and \"<page-break/>\". Any other tag stays text. Keep exported <!-- orch:block ... --> and <!-- orch:children --> comments as they are: they keep layout, cell spans, blank blocks and nesting. A picture is ![caption](images/a.png), its path relative to your file, or ![caption](<Title.files/a.png>) for one the document holds; a path that is not found is listed as missing and kept as words.",
                    ),
                ),
                (
                    "from",
                    string(
                        "For create instead of markdown: a current readable document path or id: reference from the shared tree; copies its current blocks and files with fresh identities.",
                    ),
                ),
                (
                    "old",
                    string(
                        "For replace: the text to put something in place of, exactly as read_shared gave it.",
                    ),
                ),
                ("new", string("For replace: the text to put there.")),
                (
                    "all",
                    boolean("For replace: change every place old occurs, not only a single one."),
                ),
                (
                    "after",
                    string(
                        "For insert and attach: text in the block the new block or text follows. At the end when left out. For block_move_after: the address of the block it goes after, or #0.",
                    ),
                ),
                ("title", string("For rename: the new title.")),
                (
                    "description",
                    bounded_string(
                        "For describe, and optionally for create: what the document is about, shown beside its title where documents are listed (its first line). Empty takes it away.",
                        0,
                        usize::try_from(DOCUMENT_DESCRIPTION_MAX_CHARS).unwrap_or(usize::MAX),
                    ),
                ),
                (
                    "file",
                    string(
                        "A file relative to your thread's directory, without .. or an absolute path; it must remain inside the environment's roots. For attach: the one whole file to store, which must fit the document's upload limits. For create, rewrite and insert: the text, in place of markdown (with format). For import: a directory or a .zip.",
                    ),
                ),
                (
                    "name",
                    string(
                        "For attach: the stored name, defaulting to the source's last path component; for detach: the stored file name to remove. At most 200 characters, without slashes or control characters.",
                    ),
                ),
                (
                    "overwrite",
                    boolean(
                        "For attach: replace a file of the same name, retaining its identity and links; otherwise a name conflict refuses the call.",
                    ),
                ),
                (
                    "seq",
                    integer(
                        "The seq of the read the change was written on, so that what somebody wrote meanwhile is not undone. rewrite (unless its file carries one) and batch require it.",
                    ),
                ),
                (
                    "format",
                    choice(
                        "What file is written in: markdown, html, docx or text. Left out, its ending says.",
                        &["markdown", "html", "docx", "text"],
                    ),
                ),
                (
                    "block",
                    string(
                        "For block_replace, block_insert_after, block_delete and block_move_after: the address of a block, `#n` for the nth block as read_shared counts them and `#n.m` for the mth block directly under it; `#0` (the start) for block_insert_after.",
                    ),
                ),
                (
                    "place",
                    boolean(
                        "For attach: show the file in a block (the default); false stores it without one.",
                    ),
                ),
                (
                    "caption",
                    string("For attach: the words under the block that shows the file."),
                ),
                (
                    "cancel",
                    boolean(
                        "For import: stop the job for this same path and file after the current file, keeping committed documents. Does not start a job.",
                    ),
                ),
                (
                    "resume",
                    boolean(
                        "For import: restart a cancelled job through current permissions, keeping already imported documents. Use separately from cancel.",
                    ),
                ),
                (
                    "edits",
                    list_between(
                        "For batch: the edits in order, each a replace, insert, block_replace, block_insert_after, block_delete or block_move_after with the arguments that command takes. Every address and passage is read as the document stood at seq.",
                        json!({
                            "type": "object",
                            "additionalProperties": false,
                            "properties": {
                                "command": {
                                    "type": "string",
                                    "enum": ["replace", "insert", "block_replace", "block_insert_after", "block_delete", "block_move_after"]
                                },
                                "block": { "type": "string" },
                                "markdown": { "type": "string" },
                                "old": { "type": "string" },
                                "new": { "type": "string" },
                                "all": { "type": "boolean" },
                                "after": { "type": "string" }
                            },
                            "required": ["command"]
                        }),
                        1,
                        BATCH_EDITS_MAX,
                    ),
                ),
            ],
            &["command", "path"],
        ),
        OrchTool::ManageShared => object(
            vec![
                (
                    "command",
                    choice(
                        "What to do: create_folder, copy, move, delete or restore. Only delete uses preview and confirm.",
                        &["create_folder", "copy", "move", "delete", "restore"],
                    ),
                ),
                (
                    "paths",
                    list_between(
                        "One to twenty document or document-folder paths or id references from list_shared. create_folder takes exactly one new folder path whose parent exists. At most fifty documents and folders, including all descendants, may be affected; overlapping selections are refused. restore also accepts a deleted item or one earlier document version.",
                        json!({ "type": "string" }),
                        1,
                        MANAGE_PATHS_MAX,
                    ),
                ),
                (
                    "to",
                    string(
                        "Required for copy and move: an existing writable document directory. Parent directories are never created implicitly.",
                    ),
                ),
                (
                    "name",
                    string(
                        "Optional new basename for copy or move with exactly one source; keep .md for a text document. No separators, overwrite or automatic renaming.",
                    ),
                ),
                (
                    "confirm",
                    string(
                        "For delete only: repeat the same arguments with the confirmation token from the complete deletion preview. All other commands execute directly without confirm.",
                    ),
                ),
            ],
            &["command", "paths"],
        ),
        OrchTool::SendMessage | OrchTool::SendChatMessage | OrchTool::SendThreadMessage => object(
            vec![
                (
                    "target",
                    json!({
                        "type": "object",
                        "description": "The intended recipient. Chat paths name chat.md; thread paths name a thread or its conversation.md. Each kind keeps its own write permissions.",
                        "properties": {
                            "kind": choice("Who receives the message: people in a chat, or the agent of another thread.", &["chat", "thread"]),
                            "path": string("The shared path or id: and an id of the chat or thread. A thread of your own topic may also be named by its unique name."),
                        },
                        "required": ["kind", "path"],
                        "additionalProperties": false,
                    }),
                ),
                (
                    "text",
                    bounded_string("The message, as Markdown.", 0, TEXT_MAX_CHARS),
                ),
                (
                    "files",
                    list_of(
                        "Files in your directory to attach, as relative paths without ..; at most 10. The whole message is refused if a file cannot be read or exceeds the attachment limit.",
                        json!({ "type": "string" }),
                        Some(FILES_MAX),
                    ),
                ),
                (
                    "reply_to",
                    integer(
                        "For a chat target only: the number of the message this answers, as read_shared shows it. Omit it for a thread.",
                    ),
                ),
            ],
            &["target"],
        ),
        OrchTool::Announce => object(
            vec![
                (
                    "title",
                    bounded_string("What happened, in one line.", 1, TITLE_MAX_CHARS),
                ),
                (
                    "body",
                    bounded_string(
                        "The rest: what the others need in order to act.",
                        0,
                        BODY_MAX_CHARS,
                    ),
                ),
                (
                    "category",
                    choice(
                        "notice (the default) or milestone.",
                        &["notice", "milestone"],
                    ),
                ),
                (
                    "audience",
                    choice(
                        "Who hears it: followers, topic, project or the nearest org, within the place's announcement reach. A notice goes to the topic and a milestone to the followers unless this says otherwise.",
                        &["followers", "topic", "project", "org"],
                    ),
                ),
                (
                    "severity",
                    choice(
                        "info (the default), or important, which also reaches the people who asked to hear only of what matters: keep it for what stops their work.",
                        &["info", "important"],
                    ),
                ),
                (
                    "delivery",
                    choice(
                        "How it reaches a thread at work: interrupt, steer or next_turn. Honoured only where the topic lets agents choose.",
                        &["interrupt", "steer", "next_turn"],
                    ),
                ),
                (
                    "wake_idle",
                    boolean(
                        "Whether it starts a turn on a thread that is idle. Honoured only where the topic lets agents choose.",
                    ),
                ),
            ],
            &["title"],
        ),
        OrchTool::UseBrowser => super::browser_definition::schema(),
        OrchTool::UseDisplay => super::display::schema(),
        OrchTool::RequestCapability => object(
            vec![
                (
                    "capability",
                    string("The key of the capability, as your instructions list it."),
                ),
                (
                    "reason",
                    bounded_string(
                        "What you need it for, in a sentence; a person may read it before deciding.",
                        1,
                        REASON_MAX_CHARS,
                    ),
                ),
            ],
            &["capability", "reason"],
        ),
        OrchTool::ListPorts => object(
            vec![
                (
                    "port",
                    bounded_integer(
                        "A port of the environment, also one no program listens on yet, to learn its address.",
                        1,
                        PORT_MAX,
                    ),
                ),
                ("limit", page_limit()),
                ("cursor", page_cursor()),
            ],
            &[],
        ),
        OrchTool::OpenPort => object(
            vec![
                (
                    "port",
                    bounded_integer(
                        "The port of the environment the program listens on: never a process or an outside address.",
                        1,
                        PORT_MAX,
                    ),
                ),
                (
                    "kind",
                    choice(
                        "Who is let in: the viewers of a place, whoever holds a link, or whoever reaches the host.",
                        &["place", "link", "host"],
                    ),
                ),
                (
                    "place_id",
                    string(
                        "With kind place: the place whose viewers are let in, by an id list_shared gave; this thread's topic when left out.",
                    ),
                ),
                (
                    "label",
                    bounded_string(
                        "With kind link: a readable name for the link, which is no secret.",
                        1,
                        PORT_LABEL_MAX_CHARS,
                    ),
                ),
                (
                    "transports",
                    unique(list_between(
                        "tcp, udp or both: both for host and helper, tcp for a browser, when left out.",
                        choice("A transport.", &["tcp", "udp"]),
                        1,
                        2,
                    )),
                ),
                (
                    "entry_methods",
                    unique(list_between(
                        "With kind link only: browser, helper or both; both where the domain exists, helper alone otherwise, when left out.",
                        choice("A way in.", &["browser", "helper"]),
                        1,
                        2,
                    )),
                ),
                (
                    "until",
                    string(
                        "With kind link, and then required: when the link ends, an absolute time within the allowed maximum.",
                    ),
                ),
                (
                    "host_port",
                    bounded_integer(
                        "With kind host: a fixed outside port, in the range the host allows.",
                        1,
                        PORT_MAX,
                    ),
                ),
                (
                    "program",
                    json!({
                        "type": "object",
                        "description": "How the program inside is spoken to, all three members as list_ports gives them; the current ones when left out.",
                        "properties": {
                            "http_scheme": choice(
                                "Whether the program speaks HTTP or HTTPS inside; its certificate is not checked.",
                                &["http", "https"],
                            ),
                            "host_header": {
                                "type": ["string", "null"],
                                "description": "The Host header the program needs, where a development server asks for one; null otherwise.",
                            },
                            "proxy_protocol": boolean(
                                "Whether TCP connections begin with the PROXY protocol, for a program that reads it.",
                            ),
                        },
                        "required": ["http_scheme", "host_header", "proxy_protocol"],
                        "additionalProperties": false,
                    }),
                ),
                (
                    "reason",
                    bounded_string(
                        "One sentence the deciding person reads: what it is for.",
                        1,
                        REASON_MAX_CHARS,
                    ),
                ),
            ],
            &["port", "kind", "reason"],
        ),
        OrchTool::ClosePort => object(
            vec![(
                "opening",
                string("The opening to close, as list_ports or open_port named it."),
            )],
            &["opening"],
        ),
        OrchTool::Goal => goal_schema(),
        OrchTool::ListPortConnections => object(
            vec![
                (
                    "opening",
                    string(
                        "Only the connections through this opening, as list_ports or open_port named it.",
                    ),
                ),
                (
                    "port",
                    bounded_integer(
                        "Only the connections to this port of the environment.",
                        1,
                        PORT_MAX,
                    ),
                ),
                (
                    "inside_port",
                    bounded_integer(
                        "Only the connection your program saw come from this port, to match a line of its log.",
                        1,
                        PORT_MAX,
                    ),
                ),
                (
                    "transport",
                    choice("Only tcp or only udp.", &["tcp", "udp"]),
                ),
                ("limit", page_limit()),
                ("cursor", page_cursor()),
            ],
            &[],
        ),
    }
}

/// The definition of a tool, with its description as `messages` (the resolved `agent.messages`) have it.
#[must_use]
pub fn definition(tool: OrchTool, messages: &BTreeMap<String, String>) -> ToolDefinition {
    // Stored create(from) calls keep their private validation and use the common copy domain.
    // New models receive the single copy workflow through manage_shared.
    let mut input_schema = input_schema_of(tool);
    if tool == OrchTool::EditDocument
        && let Some(properties) = input_schema["properties"].as_object_mut()
    {
        properties.remove("from");
    }
    ToolDefinition {
        tool,
        name: tool.name(),
        title: title_of(tool),
        description: messages
            .get(&message_key(tool))
            .map(|own| orch_agent_contract::tool_answers::supplied(own))
            .unwrap_or_else(|| shipped_description(tool).to_owned()),
        input_schema,
        hints: hints_of(tool),
    }
}

/// Advertises only the current place's announcement choices. Actual calls still recheck current policy.
#[must_use]
pub fn definition_at(
    tool: OrchTool,
    messages: &BTreeMap<String, String>,
    reach: &str,
    options: &[String],
) -> ToolDefinition {
    let mut definition = definition(tool, messages);
    if tool == OrchTool::Announce {
        definition.input_schema["properties"]["audience"]["enum"] = match reach {
            "org" => json!(["followers", "topic", "project", "org"]),
            "project" => json!(["followers", "topic", "project"]),
            _ => json!(["followers", "topic"]),
        };
        let deliveries: Vec<_> = ["interrupt", "steer", "next_turn"]
            .into_iter()
            .filter(|value| options.iter().any(|allowed| allowed.as_str() == *value))
            .collect();
        if let Some(properties) = definition.input_schema["properties"].as_object_mut() {
            if deliveries.is_empty() {
                properties.remove("delivery");
            } else if let Some(delivery) = properties.get_mut("delivery") {
                delivery["enum"] = json!(deliveries);
            }
            if !options.iter().any(|value| value == "wake_idle") {
                properties.remove("wake_idle");
            }
        }
    }
    definition
}

/// The definitions of all the tools, in their fixed order.
#[must_use]
pub fn all(messages: &BTreeMap<String, String>) -> Vec<ToolDefinition> {
    OrchTool::ALL
        .iter()
        .copied()
        .map(|tool| definition(tool, messages))
        .collect()
}

/// The tools a thread is offered: those of `enabled` (the resolved `orch_tools.enabled`) that are built, in
/// their fixed order. A side thread is a quick aside from another thread and tells nobody: it has no
/// `announce`.
#[must_use]
pub fn offered(enabled: &[String], side_thread: bool) -> Vec<OrchTool> {
    OrchTool::ALL
        .iter()
        .copied()
        .filter(|tool| BUILT.contains(tool))
        .filter(|tool| {
            enabled.iter().any(|name| {
                name == tool.name()
                    || (*tool == OrchTool::SendMessage
                        && matches!(name.as_str(), "send_chat_message" | "send_thread_message"))
            })
        })
        .filter(|tool| !(side_thread && *tool == OrchTool::Announce))
        .collect()
}

// ---------------------------------------------------------------- the answer to a call that cannot be done

/// Why a call was refused. The codes are the same for all the tools, so that a model learns them once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Code {
    /// Another thread or a person opened it.
    NotYours,
    /// The environment has reached its browser limit.
    TooMany,
    /// No browser answers there.
    NotReachable,
    /// The endpoint is not loopback.
    NotLocal,
    /// No installed program can open a browser.
    Unsupported,
    /// The environment has switched browsers off.
    Disabled,
    /// Nothing the agent may read is at that path.
    NotFound,
    /// The agent may read it and not write it.
    NotWritable,
    /// `manage_shared` on a document or folder the agent may read or write but lacks the required Manage.
    NotManageable,
    /// `manage_shared` with a word that is not the one of a first call with these arguments, was used, or is older
    /// than ten minutes; or what would be affected is no longer what that call listed.
    NotConfirmed,
    /// The path is not of the kind the tool works on.
    WrongKind,
    /// `create` at a path that is a document.
    Exists,
    /// A block the change touches changed after `seq`.
    Changed,
    /// `old` or `after` does not occur.
    NoMatch,
    /// Text occurs more than once without `all`, or a write name identifies several reachable entries.
    Ambiguous,
    /// A file that is not text was read.
    NotText,
    /// A file larger than a preview was read.
    TooLarge,
    /// `download_shared` of something the agent may read where the share gave no download (the export of a place).
    NotDownloadable,
    /// An argument is missing, unknown or out of range, or one that its command does not take.
    Invalid,
    /// A change would pass a limit of the place.
    Limit,
    /// A chat that takes no messages.
    Closed,
    /// Over a rate.
    RateLimited,
}

impl Code {
    /// The word a refusal begins with.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Unsupported => "unsupported",
            Self::NotLocal => "not_local",
            Self::NotReachable => "not_reachable",
            Self::TooMany => "too_many",
            Self::NotYours => "not_yours",
            Self::NotFound => "not_found",
            Self::NotWritable => "not_writable",
            Self::NotManageable => "not_manageable",
            Self::NotConfirmed => "not_confirmed",
            Self::WrongKind => "wrong_kind",
            Self::Exists => "exists",
            Self::Changed => "changed",
            Self::NoMatch => "no_match",
            Self::Ambiguous => "ambiguous",
            Self::NotText => "not_text",
            Self::TooLarge => "too_large",
            Self::NotDownloadable => "not_downloadable",
            Self::Invalid => "invalid",
            Self::Limit => "limit",
            Self::Closed => "closed",
            Self::RateLimited => "rate_limited",
        }
    }
}

/// A call that was refused, with what to do instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    /// Why.
    pub code: Code,
    /// What to do instead, in a sentence or two.
    pub message: String,
}

impl Refusal {
    /// A refusal with its code and what to do.
    #[must_use]
    pub fn new(code: Code, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// The line a harness shows: the code, then what to do.
    #[must_use]
    pub fn line(&self) -> String {
        format!("{}: {}", self.code.as_str(), self.message)
    }

    /// `invalid`, which names the argument and what it takes.
    #[must_use]
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(Code::Invalid, message)
    }
}

// ---------------------------------------------------------------- checking a call against the schema

/// Looks at the arguments of a call and refuses the first that the tool's schema does not allow, in words that
/// name it and say what is taken.
pub fn check_arguments(tool: OrchTool, arguments: &Json) -> Result<(), Refusal> {
    let schema = input_schema_of(tool);
    let Some(given) = arguments.as_object() else {
        if arguments.is_null() {
            return check_arguments(tool, &Json::Object(serde_json::Map::new()));
        }
        return Err(Refusal::invalid(format!(
            "The arguments of {} are an object. {}",
            tool.name(),
            takes(&schema)
        )));
    };
    let properties = schema["properties"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    for name in given.keys() {
        if !properties.contains_key(name) {
            return Err(Refusal::invalid(format!(
                "\"{name}\" is not an argument of {}. {}",
                tool.name(),
                takes(&schema)
            )));
        }
    }
    for name in schema["required"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Json::as_str)
    {
        if !given.contains_key(name) || given[name].is_null() {
            return Err(Refusal::invalid(format!(
                "The argument \"{name}\" is required. {}",
                takes(&schema)
            )));
        }
    }
    for (name, value) in given {
        if value.is_null() {
            continue;
        }
        if let Some(wrong) = misfit(&properties[name], name, value) {
            return Err(Refusal::invalid(wrong));
        }
    }
    if tool == OrchTool::SendMessage
        && given["target"]["kind"] == "thread"
        && given.contains_key("reply_to")
    {
        return Err(Refusal::invalid(
            "\"reply_to\" answers a chat message; it is only allowed when target.kind is \"chat\".",
        ));
    }
    Ok(())
}

/// The sentence that says what a tool takes: its arguments, the required ones marked.
fn takes(schema: &Json) -> String {
    let required: Vec<&str> = schema["required"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Json::as_str)
        .collect();
    let names: Vec<String> = schema["properties"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(name, _)| {
            if required.contains(&name.as_str()) {
                format!("{name} (required)")
            } else {
                name.clone()
            }
        })
        .collect();
    if names.is_empty() {
        "It takes no arguments.".to_owned()
    } else {
        format!("It takes: {}.", names.join(", "))
    }
}

fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

fn object_misfit(schema: &Json, name: &str, value: &Json) -> Option<String> {
    let Some(members) = value.as_object() else {
        return Some(format!("\"{name}\" is an object."));
    };
    let properties = schema["properties"].as_object()?;
    if schema["additionalProperties"] == false
        && let Some(extra) = members.keys().find(|key| !properties.contains_key(*key))
    {
        return Some(format!(
            "\"{name}.{extra}\" is not an argument. It takes: {}.",
            properties.keys().cloned().collect::<Vec<_>>().join(", ")
        ));
    }
    for required in schema["required"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Json::as_str)
    {
        if members.get(required).is_none_or(Json::is_null) {
            return Some(format!("The argument \"{name}.{required}\" is required."));
        }
    }
    members.iter().find_map(|(key, entry)| {
        properties
            .get(key)
            .and_then(|property| misfit(property, &format!("{name}.{key}"), entry))
    })
}

/// What is wrong with `value` as the argument `name` of the schema `schema`, if anything.
fn misfit(schema: &Json, name: &str, value: &Json) -> Option<String> {
    match schema["type"].as_str() {
        Some("object") => object_misfit(schema, name, value),
        Some("string") => {
            let Some(text) = value.as_str() else {
                return Some(format!("\"{name}\" is text."));
            };
            if let Some(words) = schema["enum"].as_array() {
                let words: Vec<&str> = words.iter().filter_map(Json::as_str).collect();
                if !words.contains(&text) {
                    return Some(format!("\"{name}\" is one of: {}.", words.join(", ")));
                }
            }
            let length = text.chars().count();
            let (min, max) = (
                schema["minLength"].as_u64().map(|n| n as usize),
                schema["maxLength"].as_u64().map(|n| n as usize),
            );
            if min.is_some_and(|min| length < min) || max.is_some_and(|max| length > max) {
                return Some(match (min, max) {
                    (Some(min), Some(max)) => {
                        format!("\"{name}\" is {min} to {max} characters; yours has {length}.")
                    }
                    (None, Some(max)) => {
                        format!("\"{name}\" is at most {max} characters; yours has {length}.")
                    }
                    _ => format!("\"{name}\" is at least {} characters.", min.unwrap_or(1)),
                });
            }
            None
        }
        Some("integer") => {
            let Some(number) = value.as_i64() else {
                return Some(format!("\"{name}\" is a whole number."));
            };
            let (min, max) = (schema["minimum"].as_i64(), schema["maximum"].as_i64());
            if min.is_some_and(|min| number < min) || max.is_some_and(|max| number > max) {
                return Some(match (min, max) {
                    (Some(min), Some(max)) => format!("\"{name}\" is from {min} to {max}."),
                    (Some(min), None) => format!("\"{name}\" is at least {min}."),
                    (None, Some(max)) => format!("\"{name}\" is at most {max}."),
                    (None, None) => format!("\"{name}\" is a whole number."),
                });
            }
            None
        }
        Some("boolean") => (!value.is_boolean()).then(|| format!("\"{name}\" is true or false.")),
        Some("array") => {
            let Some(entries) = value.as_array() else {
                return Some(format!("\"{name}\" is a list."));
            };
            if let Some(max) = schema["maxItems"].as_u64().map(|n| n as usize)
                && entries.len() > max
            {
                return Some(format!(
                    "\"{name}\" takes at most {}; yours has {}.",
                    plural(max, "entry", "entries"),
                    entries.len()
                ));
            }
            if let Some(min) = schema["minItems"].as_u64().map(|n| n as usize)
                && entries.len() < min
            {
                return Some(format!(
                    "\"{name}\" takes at least {}; yours has {}.",
                    plural(min, "entry", "entries"),
                    entries.len()
                ));
            }
            entries
                .iter()
                .enumerate()
                .find_map(|(at, entry)| misfit(&schema["items"], &format!("{name}[{at}]"), entry))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------- one rate for all of them

/// The calls that threads' agents made lately, for the rate: one window for each thread.
#[derive(Debug, Default)]
pub struct CallRate {
    windows: HashMap<Id, VecDeque<(DateTime<Utc>, String)>>,
}

/// How long a call counts against the rate: a minute.
const RATE_WINDOW: Duration = Duration::seconds(60);
/// How many threads' windows are kept before the empty ones are swept out: far more than threads that call in a
/// minute, and few enough that a sweep costs nothing.
const WINDOWS_SWEPT_ABOVE: usize = 1_024;

impl CallRate {
    /// Counts a call of `thread` made at `now`, or says how many seconds to wait before one would be counted.
    /// A call that is asked again under the same `call_ref` is the same call and counts once.
    pub fn admit(
        &mut self,
        thread: Id,
        call_ref: &str,
        now: DateTime<Utc>,
        per_minute: i64,
    ) -> Result<(), u64> {
        if self.windows.len() > WINDOWS_SWEPT_ABOVE {
            self.windows
                .retain(|_, window| window.back().is_some_and(|(at, _)| now - *at < RATE_WINDOW));
        }
        let window = self.windows.entry(thread).or_default();
        while window
            .front()
            .is_some_and(|(at, _)| now - *at >= RATE_WINDOW)
        {
            window.pop_front();
        }
        if window.iter().any(|(_, seen)| seen == call_ref) {
            return Ok(());
        }
        let allowed = usize::try_from(per_minute).unwrap_or(0);
        if window.len() >= allowed {
            // The call that would be counted next is the one that leaves the window first.
            let wait = window
                .front()
                .map_or(RATE_WINDOW, |(at, _)| RATE_WINDOW - (now - *at));
            return Err(u64::try_from(wait.num_seconds() + 1).unwrap_or(1));
        }
        window.push_back((now, call_ref.to_owned()));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_types::new_id;

    fn at(seconds: i64) -> DateTime<Utc> {
        DateTime::<Utc>::UNIX_EPOCH + Duration::seconds(seconds)
    }

    #[test]
    fn every_tool_is_declared_with_a_closed_schema_whose_arguments_are_all_described_and_hints_that_are_true()
     {
        let all = all(&BTreeMap::new());
        assert_eq!(
            all.iter().map(|each| each.name).collect::<Vec<_>>(),
            OrchTool::ALL
                .iter()
                .map(|tool| tool.name())
                .collect::<Vec<_>>(),
            "one definition for each tool of the list, in the list's order"
        );
        for each in &all {
            let schema = &each.input_schema;
            assert_eq!(schema["type"], "object", "{}", each.name);
            assert_eq!(schema["additionalProperties"], false, "{}", each.name);
            assert!(
                schema.get("anyOf").is_none() && schema.get("oneOf").is_none(),
                "no union at the root of {}",
                each.name
            );
            for (name, argument) in schema["properties"].as_object().unwrap() {
                assert!(
                    argument["description"]
                        .as_str()
                        .is_some_and(|text| !text.is_empty()),
                    "{}.{name} says what it is",
                    each.name
                );
            }
            for needed in schema["required"].as_array().unwrap() {
                assert!(schema["properties"].get(needed.as_str().unwrap()).is_some());
            }
            assert!(
                each.description.split(". ").count() >= 3,
                "{} is described in three sentences or more",
                each.name
            );
            let mcp = each.to_mcp();
            assert_eq!(
                mcp["annotations"]["openWorldHint"],
                each.tool == OrchTool::UseBrowser,
                "only the browser tool reaches the open web"
            );
            // A tool that only reads says nothing of destroying; one that changes something says both.
            assert_eq!(
                mcp["annotations"].get("destructiveHint").is_some(),
                !each.hints.read_only
            );
        }
        let read_only: Vec<&str> = all
            .iter()
            .filter(|each| each.hints.read_only)
            .map(|each| each.name)
            .collect();
        for reads in ["list_shared", "read_shared", "search_shared"] {
            assert!(read_only.contains(&reads), "{reads} changes nothing");
        }
        for changes in [
            "download_shared",
            "edit_document",
            "manage_shared",
            "send_message",
            "announce",
            "request_capability",
        ] {
            assert!(!read_only.contains(&changes), "{changes} changes something");
        }
    }

    #[test]
    fn the_tool_that_manages_documents_is_declared_as_the_specification_gives_it() {
        let tool = definition(OrchTool::ManageShared, &BTreeMap::new());
        assert_eq!(tool.name, "manage_shared");
        assert_eq!(tool.title, "Organize documents and folders");
        assert_eq!(tool.description, texts::MANAGE_SHARED);
        assert!(
            tool.description.starts_with(
                "Organize shared documents and document folders using paths from list_shared."
            ) && tool.description.ends_with(
                "These operations organize Orch documents, not your execution workspace."
            )
        );
        assert!(tool.description.contains("Only delete requires two calls"));
        assert!(
            tool.description
                .contains("create_folder, copy, move and restore execute directly")
        );
        // It removes or moves, though it can be brought back; and doing it twice is not doing it once.
        assert_eq!(
            tool.hints,
            Hints {
                read_only: false,
                destructive: Some(true),
                idempotent: Some(false)
            }
        );
        let schema = &tool.input_schema;
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(schema["required"], json!(["command", "paths"]));
        assert_eq!(
            schema["properties"]["command"]["enum"],
            json!(["create_folder", "copy", "move", "delete", "restore"])
        );
        assert_eq!(schema["properties"]["paths"]["minItems"], 1);
        assert_eq!(schema["properties"]["paths"]["maxItems"], 20);
        assert_eq!(schema["properties"]["paths"]["items"]["type"], "string");
        assert_eq!(schema["properties"]["to"]["type"], "string");
        assert_eq!(schema["properties"]["name"]["type"], "string");
        assert_eq!(schema["properties"]["confirm"]["type"], "string");
        assert_eq!(
            schema["properties"].as_object().map(serde_json::Map::len),
            Some(7),
            "command, paths, to, name and confirm accompany the common answer options"
        );
    }

    #[test]
    fn an_announcement_lists_only_the_current_places_delivery_and_audience_choices() {
        let messages = BTreeMap::new();
        let stock = definition(OrchTool::Announce, &messages);
        let default = definition_at(OrchTool::Announce, &messages, "topic", &[]);
        assert_eq!(
            default.input_schema["properties"]["audience"]["enum"],
            json!(["followers", "topic"])
        );
        assert!(default.input_schema["properties"].get("delivery").is_none());
        assert!(
            default.input_schema["properties"]
                .get("wake_idle")
                .is_none()
        );
        let allowed = definition_at(
            OrchTool::Announce,
            &messages,
            "org",
            &["steer".to_owned(), "wake_idle".to_owned()],
        );
        assert_eq!(
            allowed.input_schema["properties"]["delivery"]["enum"],
            json!(["steer"])
        );
        assert_eq!(
            allowed.input_schema["properties"]["audience"]["enum"],
            stock.input_schema["properties"]["audience"]["enum"]
        );
        assert_eq!(
            allowed.description,
            texts::ANNOUNCE,
            "policy narrows the arguments, not the exact T11 description"
        );
        assert_eq!(
            allowed.input_schema["properties"]["body"]["maxLength"],
            1500
        );
        assert!(
            allowed.input_schema["properties"]
                .get("wake_idle")
                .is_some()
        );
        assert!(
            stock.input_schema["properties"].get("delivery").is_some(),
            "the shared catalogue was not mutated"
        );
    }

    #[test]
    fn the_schema_of_managing_keeps_out_what_it_can() {
        let ok = json!({ "command": "delete", "paths": ["docs/Plan.md"] });
        assert!(check_arguments(OrchTool::ManageShared, &ok).is_ok());
        let none = check_arguments(
            OrchTool::ManageShared,
            &json!({ "command": "delete", "paths": [] }),
        )
        .unwrap_err();
        assert!(
            none.message
                .contains("\"paths\" takes at least 1 entry; yours has 0"),
            "{}",
            none.message
        );
        let many = check_arguments(
            OrchTool::ManageShared,
            &json!({ "command": "delete", "paths": vec!["a.md"; 21] }),
        )
        .unwrap_err();
        assert!(
            many.message
                .contains("\"paths\" takes at most 20 entries; yours has 21"),
            "{}",
            many.message
        );
        let word = check_arguments(
            OrchTool::ManageShared,
            &json!({ "command": "destroy", "paths": ["a.md"] }),
        )
        .unwrap_err();
        assert!(
            word.message
                .contains("create_folder, copy, move, delete, restore"),
            "{}",
            word.message
        );
        let unknown = check_arguments(
            OrchTool::ManageShared,
            &json!({ "command": "delete", "paths": ["a.md"], "confirmed": true }),
        )
        .unwrap_err();
        assert!(
            unknown
                .message
                .contains("\"confirmed\" is not an argument of manage_shared"),
            "{}",
            unknown.message
        );
    }

    #[test]
    fn document_folders_have_one_copy_workflow_and_only_delete_asks_for_confirmation() {
        let edit = definition(OrchTool::EditDocument, &BTreeMap::new());
        assert!(edit.input_schema["properties"].get("from").is_none());
        assert!(
            input_schema_of(OrchTool::EditDocument)["properties"]
                .get("from")
                .is_some()
        );
        assert!(
            check_arguments(
                OrchTool::EditDocument,
                &json!({"command":"create","path":"docs/Copy.md","from":"docs/Original.md"})
            )
            .is_ok(),
            "stored calls keep their original meaning through the same Copy domain"
        );
        assert!(!edit.description.contains("For create, from"));
        assert!(edit.description.contains("manage_shared copy"));
        for arguments in [
            json!({"command":"create_folder","paths":["docs/Research"]}),
            json!({"command":"copy","paths":["docs/Research"],"to":"docs/Archive","name":"Snapshot"}),
            json!({"command":"move","paths":["docs/Research"],"to":"docs/Archive"}),
            json!({"command":"restore","paths":["docs/Research"]}),
            json!({"command":"delete","paths":["docs/Research"]}),
            json!({"command":"delete","paths":["docs/Research"],"confirm":"the-complete-preview-token"}),
        ] {
            assert!(
                check_arguments(OrchTool::ManageShared, &arguments).is_ok(),
                "{arguments}"
            );
        }
        let description = shipped_description(OrchTool::ManageShared);
        assert!(description.contains("Only delete requires two calls"));
        assert!(
            description
                .contains("Every operation authorizes the whole tree and executes all or none")
        );
        assert!(texts::LIST_SHARED.contains("nested document folders"));
        assert!(
            texts::LIST_SHARED
                .contains("recursive operations separately authorize every descendant")
        );
        assert!(texts::READ_SHARED.contains("A folder is listed with list_shared"));
        assert!(texts::READ_SHARED.contains("Attachments, including text files"));
        assert!(
            edit.input_schema["properties"]["path"]["description"]
                .as_str()
                .is_some_and(|description| description
                    .contains("existing writable docs root or document folder"))
        );
        assert_eq!(
            definition_at(OrchTool::EditDocument, &BTreeMap::new(), "topic", &[]).input_schema,
            edit.input_schema,
            "offered definitions have no second create(from) workflow at any place"
        );
    }

    #[test]
    fn a_place_may_replace_the_description_of_a_tool_and_nothing_else_of_it() {
        let own = BTreeMap::from([(
            message_key(OrchTool::Announce),
            "Tell the others.".to_owned(),
        )]);
        let replaced = definition(OrchTool::Announce, &own);
        let shipped = definition(OrchTool::Announce, &BTreeMap::new());
        assert_eq!(replaced.description, "Tell the others.");
        assert_eq!(replaced.input_schema, shipped.input_schema);
        assert_eq!(replaced.title, shipped.title);
        assert_eq!(
            definition(OrchTool::ListShared, &own).description,
            shipped_description(OrchTool::ListShared)
        );
    }

    #[test]
    fn every_shipped_description_stays_under_two_thousand_and_forty_eight_characters_and_writers_are_taught_references()
     {
        for each in all(&BTreeMap::new()) {
            let length = each.description.chars().count();
            assert!(length < 2_048, "{}: {length} characters", each.name);
        }
        for tool in [
            OrchTool::ListShared,
            OrchTool::EditDocument,
            OrchTool::SendMessage,
        ] {
            assert!(
                shipped_description(tool).contains("[Rollout plan](<docs/Rollout plan.md#3>)"),
                "{tool:?} shows the one example of a reference"
            );
        }
    }

    #[test]
    fn a_thread_is_offered_what_it_enables_that_is_built_and_a_side_thread_has_no_announce() {
        let enabled: Vec<String> = OrchTool::ALL
            .iter()
            .map(|tool| tool.name().to_owned())
            .collect();
        let everything = offered(&enabled, false);
        assert_eq!(
            everything, BUILT,
            "all enabled: the ones that are built, in their order"
        );
        let aside = offered(&enabled, true);
        assert!(!aside.contains(&OrchTool::Announce));
        assert!(aside.contains(&OrchTool::SendMessage));
        assert!(offered(&[], false).is_empty());
        assert_eq!(
            offered(&["list_shared".to_owned()], false),
            [OrchTool::ListShared],
            "only what is enabled is offered"
        );
    }

    #[test]
    fn arguments_are_checked_against_the_schema_the_model_was_shown() {
        assert!(check_arguments(OrchTool::Announce, &json!({ "title": "x" })).is_ok());
        assert!(
            check_arguments(OrchTool::Announce, &Json::Null).is_err(),
            "a title is required"
        );
        let wrong = check_arguments(
            OrchTool::SendMessage,
            &json!({ "target": { "kind": "thread", "path": "A" }, "files": vec!["a"; 11] }),
        )
        .unwrap_err();
        assert_eq!(wrong.code, Code::Invalid);
        assert!(
            wrong
                .message
                .contains("\"files\" takes at most 10 entries; yours has 11"),
            "{}",
            wrong.message
        );
        let range = check_arguments(OrchTool::ListShared, &json!({ "depth": 9 })).unwrap_err();
        assert!(
            range.message.contains("\"depth\" is from 1 to 4"),
            "{}",
            range.message
        );
        let kind = check_arguments(
            OrchTool::RequestCapability,
            &json!({ "capability": 3, "reason": "r" }),
        )
        .unwrap_err();
        assert!(
            kind.message.contains("\"capability\" is text"),
            "{}",
            kind.message
        );
    }

    #[test]
    fn one_message_tool_has_a_closed_typed_target_and_chat_only_replies() {
        let tool = definition(OrchTool::SendMessage, &BTreeMap::new());
        assert_eq!(tool.name, "send_message");
        assert_eq!(tool.input_schema["required"], json!(["target"]));
        let target = &tool.input_schema["properties"]["target"];
        assert_eq!(target["additionalProperties"], false);
        assert_eq!(target["required"], json!(["kind", "path"]));
        assert_eq!(
            target["properties"]["kind"]["enum"],
            json!(["chat", "thread"])
        );
        assert_eq!(target["properties"].as_object().unwrap().len(), 2);
        for arguments in [
            json!({"target":{"kind":"chat","path":"chat.md"},"text":"Report","reply_to":1}),
            json!({"target":{"kind":"thread","path":"Validate"},"files":["out/report.pdf"]}),
        ] {
            assert!(check_arguments(OrchTool::SendMessage, &arguments).is_ok());
        }
        for (arguments, cause) in [
            (json!({"target":"chat.md","text":"Report"}), "target"),
            (
                json!({"target":{"kind":"chat"},"text":"Report"}),
                "target.path",
            ),
            (
                json!({"target":{"path":"chat.md"},"text":"Report"}),
                "target.kind",
            ),
            (
                json!({"target":{"kind":"email","path":"private"},"text":"Report"}),
                "target.kind",
            ),
            (
                json!({"target":{"kind":"chat","path":3},"text":"Report"}),
                "target.path",
            ),
            (
                json!({"target":{"kind":"chat","path":"chat.md","as_user":"owner"},"text":"Report"}),
                "target.as_user",
            ),
            (
                json!({"target":{"kind":"thread","path":"Validate"},"text":"Report","reply_to":1}),
                "reply_to",
            ),
            (
                json!({"target":{"kind":"thread","path":"Validate"},"text":"Report","reply_to":null}),
                "reply_to",
            ),
            (json!({"chat":"chat.md","text":"Report"}), "chat"),
        ] {
            let refusal = check_arguments(OrchTool::SendMessage, &arguments).unwrap_err();
            assert_eq!(refusal.code, Code::Invalid);
            assert!(refusal.message.contains(cause), "{}", refusal.message);
        }
        let names: Vec<_> = all(&BTreeMap::new())
            .iter()
            .map(|definition| definition.name)
            .collect();
        assert_eq!(
            names.iter().filter(|name| **name == "send_message").count(),
            1
        );
        for historical in ["send_chat_message", "send_thread_message"] {
            assert!(!names.contains(&historical));
            assert_eq!(
                offered(&[historical.to_owned()], false),
                [OrchTool::SendMessage]
            );
        }
    }

    #[test]
    fn a_call_beyond_the_rate_waits_for_the_oldest_one_to_leave_the_minute() {
        let mut rate = CallRate::default();
        let thread = new_id();
        for second in 0..3 {
            assert!(
                rate.admit(thread, &format!("c{second}"), at(second), 3)
                    .is_ok()
            );
        }
        assert_eq!(rate.admit(thread, "c3", at(10), 3), Err(51));
        assert!(rate.admit(thread, "c3", at(60), 3).is_ok());
    }

    #[test]
    fn a_call_asked_twice_under_one_reference_is_counted_once() {
        let mut rate = CallRate::default();
        let thread = new_id();
        for _ in 0..5 {
            assert!(rate.admit(thread, "same", at(0), 1).is_ok());
        }
        assert!(rate.admit(thread, "other", at(1), 1).is_err());
    }

    #[test]
    fn the_rate_is_one_threads_own() {
        let mut rate = CallRate::default();
        let (a, b) = (new_id(), new_id());
        assert!(rate.admit(a, "x", at(0), 1).is_ok());
        assert!(rate.admit(a, "y", at(0), 1).is_err());
        assert!(rate.admit(b, "y", at(0), 1).is_ok());
    }
}
