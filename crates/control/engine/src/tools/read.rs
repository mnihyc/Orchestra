//! Read one entry of the shared tree: a range of its parts at one of three depths, in its written form, within a bound of bytes.
//!
//! An entry is counted in parts: a conversation in turns, a chat in messages, a document in top-level blocks. `from`
//! and `to` name a range of them, and a negative number counts from the end. Without a range a conversation or a
//! chat is given from its end, as many of its latest parts as fit, and a document from its beginning. `max_bytes`
//! bounds the text in bytes of UTF-8: 20 000 unless given, at most 40 000. Parts are given whole; one that is
//! larger than everything asked for is cut, and says so. The answer says which parts it holds and how many there
//! are, so that what is left is asked for with the next `from`.
//!
//! What is read is the written form (`written`), the one text the server makes of a conversation, a chat and a
//! document for a reader that is not the application's own screen; the text was masked when it was stored. Reading
//! changes nothing and tells nobody, and is audited.

use crate::engine::Engine;
use crate::error::EngineError;
use crate::tools::definitions::{Code, READ_BYTES_MAX, READ_BYTES_MIN, Refusal};
use crate::tools::documents;
use crate::tools::shared::{Resolved, Target, audit_by_agent};
use crate::tools::{Answer, CallContext};
use crate::written::{self, Detail};
use orch_access_contract::audit::AuditAction;
use orch_api_contract::share::{SharedEntryKind, SharedEntryUnit};
use orch_db::queries::documents::{self as document_rows, history};
use orch_db::queries::threads::ThreadRow;
use orch_documents::markdown;
use orch_types::{Id, Json, Time};
use serde::Deserialize;
use serde_json::json;

/// The bytes a read returns when it is not told.
pub const READ_BYTES_DEFAULT: i64 = 20_000;

/// The arguments of `read_shared`, once the schema has let them through.
#[derive(Debug, Deserialize)]
struct Arguments {
    path: String,
    from: Option<i64>,
    to: Option<i64>,
    detail: Option<String>,
    max_bytes: Option<i64>,
}

/// What was read, ready to be an answer.
pub(crate) struct Got {
    pub text: String,
    pub from: i64,
    pub to: i64,
    pub last: i64,
    pub truncated: bool,
    pub unit: SharedEntryUnit,
    pub kind: SharedEntryKind,
    pub name: String,
    pub id: Id,
    pub seq: Option<i64>,
    pub files: Vec<Json>,
    /// Writers of the blocks actually included by this reading, separate from
    /// their editable Markdown. Each name includes its mention handle.
    pub writers: Vec<Json>,
}

fn name_of(path: &str) -> String {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_owned()
}

impl Engine {
    /// Reads what a resolved path names: the text at a depth, within a bound.
    pub(crate) async fn read_target(
        &self,
        conn: &mut orch_db::Conn,
        resolved: &Resolved,
        range: (Option<i64>, Option<i64>),
        detail: Detail,
        budget: Option<usize>,
    ) -> Result<Result<Got, Refusal>, EngineError> {
        let path = resolved.path.clone();
        let tree = &resolved.tree;
        Ok(Ok(match &resolved.target {
            Target::DocumentFile { .. } | Target::ChatFile { .. } => {
                return Ok(Err(Refusal::new(
                    Code::WrongKind,
                    "This path names an attachment. Use download_shared to copy its bytes into your workspace.",
                )));
            }
            Target::Directory(_) => {
                return Ok(Err(Refusal::new(
                    Code::WrongKind,
                    format!(
                        "{path} is a directory, not something to read. List it with list_shared."
                    ),
                )));
            }
            Target::Conversation { thread } => {
                let files = written::files_beside(&path);
                let Some(read) = written::read_conversation_in(
                    conn,
                    *thread,
                    range,
                    detail,
                    budget,
                    Some(&files),
                )
                .await?
                else {
                    return Ok(Err(Refusal::new(Code::NotFound, "That thread is gone.")));
                };
                Got {
                    text: read.text,
                    from: read.from,
                    to: read.to,
                    last: read.last,
                    truncated: read.truncated,
                    unit: SharedEntryUnit::Turn,
                    kind: SharedEntryKind::Conversation,
                    name: name_of(&path),
                    id: *thread,
                    seq: None,
                    files: Vec::new(),
                    writers: Vec::new(),
                }
            }
            Target::NodeChat { chat, .. } | Target::PeopleChat { chat } => {
                let title = tree.chat_title(*chat).unwrap_or_else(|| "Chat".to_owned());
                let files = written::files_beside(&path);
                // What the messages name is written in this reader's terms.
                let named = written::chat_references(conn, *chat, range, budget).await?;
                let writings = self
                    .writings_for(conn, resolved.asking.agent.thread, &named)
                    .await?;
                let read = written::read_chat_for(
                    conn,
                    *chat,
                    &title,
                    range,
                    detail,
                    budget,
                    Some(&files),
                    Some(&writings),
                )
                .await?;
                Got {
                    text: read.text,
                    from: read.from,
                    to: read.to,
                    last: read.last,
                    truncated: read.truncated,
                    unit: SharedEntryUnit::Message,
                    kind: SharedEntryKind::Chat,
                    name: name_of(&path),
                    id: *chat,
                    seq: None,
                    files: Vec::new(),
                    writers: Vec::new(),
                }
            }
            Target::Document { document } => {
                let Some(row) = document_rows::get(conn, *document).await? else {
                    return Ok(Err(Refusal::new(Code::NotFound, "That document is gone.")));
                };
                if row.deleted_at.is_some() {
                    return Ok(Err(Refusal::new(Code::NotFound, "That document is gone.")));
                }
                let stored = documents::live(conn, *document).await?;
                let changed_by = documents::last_changed_by(conn, &row).await?;
                let files = self.document_files(conn, &row, &path).await?;
                let known = self
                    .known_for_reader(
                        conn,
                        resolved.asking.agent.thread,
                        &stored,
                        &written::files_beside(&path),
                    )
                    .await?;
                document_got(
                    known,
                    &stored,
                    &row.title,
                    &changed_by,
                    Time::new(row.updated_at),
                    range,
                    detail,
                    budget,
                    Got {
                        text: String::new(),
                        from: 0,
                        to: 0,
                        last: 0,
                        truncated: false,
                        unit: SharedEntryUnit::Block,
                        kind: SharedEntryKind::Document,
                        name: name_of(&path),
                        id: *document,
                        seq: Some(row.doc_seq),
                        files,
                        writers: Vec::new(),
                    },
                )
                .await?
            }
            Target::Version { document, number } => {
                let Some(row) = document_rows::get(conn, *document).await? else {
                    return Ok(Err(Refusal::new(Code::NotFound, "That document is gone.")));
                };
                let (Some(version), Some(stored)) = (
                    history::get(conn, *document, *number).await?,
                    history::snapshot(conn, *document, *number).await?,
                ) else {
                    return Ok(Err(Refusal::new(
                        Code::NotFound,
                        format!(
                            "The document has no version {number}. Its versions are in the directory beside it that ends in .history."
                        ),
                    )));
                };
                let writers: Vec<_> = version
                    .authors
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(orch_documents::versions::Writer::from_json)
                    .collect();
                let users: Vec<_> = writers
                    .iter()
                    .filter_map(|writer| match writer {
                        orch_documents::versions::Writer::User(user) => Some(*user),
                        orch_documents::versions::Writer::Agent { .. } => None,
                    })
                    .collect();
                let people: std::collections::HashMap<_, _> =
                    orch_db::queries::identity::users_brief(conn, &users)
                        .await?
                        .into_iter()
                        .map(|person| {
                            (
                                person.id,
                                orch_chat::written::person_name(
                                    &person.display_name,
                                    &person.username,
                                ),
                            )
                        })
                        .collect();
                let changed_by = if writers.is_empty() {
                    "Orch".to_owned()
                } else {
                    writers
                        .iter()
                        .map(|writer| match writer {
                            orch_documents::versions::Writer::Agent { agent, .. } => agent.clone(),
                            orch_documents::versions::Writer::User(user) => people
                                .get(user)
                                .cloned()
                                .unwrap_or_else(|| "a removed person".to_owned()),
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                let known = self
                    .known_for_reader(
                        conn,
                        resolved.asking.agent.thread,
                        &stored,
                        &written::files_beside(&path),
                    )
                    .await?;
                document_got(
                    known,
                    &stored,
                    &version.title,
                    &changed_by,
                    Time::new(version.to_at),
                    range,
                    detail,
                    budget,
                    Got {
                        text: String::new(),
                        from: 0,
                        to: 0,
                        last: 0,
                        truncated: false,
                        unit: SharedEntryUnit::Block,
                        kind: SharedEntryKind::Version,
                        name: name_of(&path),
                        id: *document,
                        seq: Some(row.doc_seq),
                        files: Vec::new(),
                        writers: Vec::new(),
                    },
                )
                .await?
            }
        }))
    }

    /// The files a document holds, as the tree names them.
    async fn document_files(
        &self,
        conn: &mut orch_db::Conn,
        row: &document_rows::DocumentRow,
        path: &str,
    ) -> Result<Vec<Json>, EngineError> {
        // The files lie where a link in the text leads to them: beside it, in the directory the one rule names.
        let directory = path
            .rsplit_once('/')
            .map_or(String::new(), |(directory, _)| format!("{directory}/"));
        let beside = written::files_beside(path);
        Ok(document_rows::files(conn, row.id)
            .await?
            .into_iter()
            .map(|file| {
                json!({
                    // The name of a file is already fit for a path: it holds no slash, and is none of `.` and `..`.
                    "path": format!("{directory}{beside}/{}", file.name),
                    "name": file.name,
                    "size": file.size,
                    "media_type": file.media_type,
                })
            })
            .collect())
    }

    /// `read_shared`: the call of an agent.
    pub(crate) async fn read_shared_call(
        &self,
        call: &CallContext,
        thread: &ThreadRow,
    ) -> Result<Answer, EngineError> {
        let arguments: Arguments = match serde_json::from_value(call.arguments.clone()) {
            Ok(arguments) => arguments,
            Err(error) => return Ok(Refusal::invalid(error.to_string()).into()),
        };
        let detail = match arguments.detail.as_deref() {
            None => Detail::Normal,
            Some(word) => match Detail::parse(word) {
                Some(detail) => detail,
                None => {
                    return Ok(Refusal::invalid("\"detail\" is brief, normal or full.").into());
                }
            },
        };
        let max_bytes = arguments.max_bytes.unwrap_or(READ_BYTES_DEFAULT);
        if !(READ_BYTES_MIN..=READ_BYTES_MAX).contains(&max_bytes) {
            return Ok(Refusal::invalid(format!(
                "\"max_bytes\" is from {READ_BYTES_MIN} to {READ_BYTES_MAX}."
            ))
            .into());
        }
        let budget = usize::try_from(max_bytes).unwrap_or(usize::MAX);
        let mut conn = self.inner.db.acquire().await?;
        let resolved = match self
            .resolve(&mut conn, thread.node_id, &arguments.path, true)
            .await?
        {
            Ok(resolved) => resolved,
            Err(refusal) => return Ok(refusal.into()),
        };
        let got = match self
            .read_target(
                &mut conn,
                &resolved,
                (arguments.from, arguments.to),
                detail,
                Some(budget),
            )
            .await?
        {
            Ok(got) => got,
            Err(refusal) => return Ok(refusal.into()),
        };
        self.audit_read(&mut conn, &resolved, &got).await?;
        Ok(Answer::Done(resolved.annotate(answer_json(
            &resolved.path,
            detail,
            &got,
        ))))
    }

    /// Writes down that an agent read something.
    pub(crate) async fn audit_read(
        &self,
        conn: &mut orch_db::Conn,
        resolved: &Resolved,
        got: &Got,
    ) -> Result<(), EngineError> {
        let (action, kind, node, summary) = match &resolved.target {
            Target::Conversation { thread } => (
                orch_db::audit::action!(AuditAction::ThreadReadByAgent),
                "thread",
                Some(*thread),
                "An agent read the conversation of another thread.",
            ),
            Target::NodeChat { node, .. } => (
                orch_db::audit::action!(AuditAction::ChatReadByAgent),
                "chat",
                Some(*node),
                "An agent read a chat.",
            ),
            Target::PeopleChat { .. } => (
                AuditAction::ChatReadByAgent,
                "chat",
                None,
                "An agent read a chat.",
            ),
            Target::Document { .. } | Target::Version { .. } => (
                orch_db::audit::action!(AuditAction::DocumentReadByAgent),
                "document",
                None,
                "An agent read a document.",
            ),
            Target::Directory(_) | Target::DocumentFile { .. } | Target::ChatFile { .. } => {
                return Ok(());
            }
        };
        audit_by_agent(
            conn,
            &resolved.asking,
            action,
            kind,
            got.id,
            node,
            summary,
            json!({ "path": resolved.path, "from": got.from, "to": got.to }),
        )
        .await
    }
}

/// A document, or one of its versions, read: its blocks as Markdown under the line that says whose it is.
#[allow(
    clippy::too_many_arguments,
    reason = "a document is read by its blocks, who wrote it last, the range, the depth and the bound"
)]
async fn document_got(
    known: documents::Known,
    stored: &[orch_db::queries::documents::BlockRow],
    title: &str,
    changed_by: &str,
    at: Time,
    range: (Option<i64>, Option<i64>),
    detail: Detail,
    budget: Option<usize>,
    mut got: Got,
) -> Result<Got, EngineError> {
    let blocks: Vec<_> = stored.iter().map(documents::block_of).collect();
    let line = markdown::context_line(title, changed_by, at);
    let pieces = documents::pieces_of(&blocks, &known);
    got.last = pieces.count;
    if pieces.count == 0 {
        got.text = format!("{line}\n");
        return Ok(got);
    }
    if detail == Detail::Brief {
        got.text = format!("{line}\n\n{}\n", markdown::headings(&blocks));
        got.from = 1;
        got.to = pieces.count;
        return Ok(got);
    }
    let room = budget.map(|budget| budget.saturating_sub(line.len() + 2));
    let from = range.0.or(if range.1.is_none() { Some(1) } else { None });
    let window = match room {
        Some(room) if pieces.cut => written::window(from, range.1, pieces.count, room, |n| {
            usize::try_from(n - 1)
                .ok()
                .and_then(|at| pieces.pieces.get(at))
                .map_or(usize::MAX, |piece| {
                    piece.text.len() + piece.gap.text().len()
                })
        }),
        _ => {
            let number = |n: i64| if n < 0 { pieces.count + 1 + n } else { n };
            let (first, last) = (
                from.map_or(1, number).max(1),
                range.1.map_or(pieces.count, number).min(pieces.count),
            );
            (first <= last).then_some(written::Window {
                from: first,
                to: last,
                cut: false,
            })
        }
    };
    let Some(window) = window else {
        got.text = format!(
            "{line}\n\n(no blocks in that range; the document has {})\n",
            pieces.count
        );
        return Ok(got);
    };
    let mut text = documents::range_text(&pieces, window.from, window.to);
    got.from = window.from;
    got.to = window.to;
    // The selected top-level blocks and everything under them. Parent links
    // come only from the authorized document, never a general people lookup.
    let roots = documents::top_level(&blocks);
    let first = usize::try_from(window.from - 1).unwrap_or(0);
    let count = usize::try_from(window.to - window.from + 1).unwrap_or(0);
    let selected: std::collections::HashSet<_> = roots
        .iter()
        .skip(first)
        .take(count)
        .map(|block| block.id)
        .collect();
    let parents: std::collections::HashMap<_, _> = stored
        .iter()
        .map(|block| (block.id, block.parent_id))
        .collect();
    for block in stored {
        let mut root = block.id;
        // Stored blocks were validated as a tree; this bound also makes a
        // damaged legacy cycle contribute no writer instead of looping.
        for _ in 0..stored.len() {
            let Some(parent) = parents.get(&root).copied().flatten() else {
                break;
            };
            root = parent;
        }
        if selected.contains(&root)
            && let Some(writer) = known.block_writer(block)
        {
            got.writers
                .push(json!({ "block_id": block.id, "writer": writer }));
        }
    }
    if let Some(room) = room {
        let (cut, was) = written::cut_to_bytes(&text, room);
        if was {
            text = cut;
            got.truncated = true;
        }
    }
    got.text = format!("{line}\n\n{text}\n");
    Ok(got)
}

/// A document in its written form, whole: the text that a read of all of it gives, for a copy and a download to
/// hold the same words that the tree reads.
pub(crate) async fn document_written(
    conn: &mut orch_db::Conn,
    row: &document_rows::DocumentRow,
    beside: &str,
) -> Result<String, EngineError> {
    let stored = documents::live(conn, row.id).await?;
    let changed_by = documents::last_changed_by(conn, row).await?;
    let known = documents::known_for(conn, &stored, beside).await?;
    let got = document_got(
        known,
        &stored,
        &row.title,
        &changed_by,
        Time::new(row.updated_at),
        (None, None),
        Detail::Normal,
        None,
        Got {
            text: String::new(),
            from: 0,
            to: 0,
            last: 0,
            truncated: false,
            unit: SharedEntryUnit::Block,
            kind: SharedEntryKind::Document,
            name: row.title.clone(),
            id: row.id,
            seq: Some(row.doc_seq),
            files: Vec::new(),
            writers: Vec::new(),
        },
    )
    .await?;
    Ok(got.text)
}

/// The answer to a read.
pub(crate) fn answer_json(path: &str, detail: Detail, got: &Got) -> Json {
    let mut entry = serde_json::Map::new();
    entry.insert("path".to_owned(), json!(path));
    entry.insert(
        "kind".to_owned(),
        serde_json::to_value(got.kind).unwrap_or(Json::Null),
    );
    entry.insert("name".to_owned(), json!(got.name));
    entry.insert("id".to_owned(), json!(got.id));
    entry.insert(
        "unit".to_owned(),
        serde_json::to_value(got.unit).unwrap_or(Json::Null),
    );
    if let Some(seq) = got.seq {
        entry.insert("seq".to_owned(), json!(seq));
    }
    json!({
        "entry": Json::Object(entry),
        "detail": detail.word(),
        "from": got.from,
        "to": got.to,
        "last": got.last,
        "text": got.text,
        "files": got.files,
        "writers": got.writers,
        "truncated": got.truncated,
    })
}

/// The one line that says what was read and what is left, which goes before the text where the text is given as it
/// is.
#[must_use]
pub fn summary_line(answer: &Json) -> Option<String> {
    let entry = answer.get("entry")?;
    let path = entry.get("path")?.as_str()?;
    let unit = entry.get("unit")?.as_str()?;
    let (from, to, last) = (
        answer.get("from")?.as_i64()?,
        answer.get("to")?.as_i64()?,
        answer.get("last")?.as_i64()?,
    );
    let detail = answer.get("detail")?.as_str()?;
    let mut line = format!("{path} · {detail} · {unit}s {from}–{to} of {last}");
    if answer.get("truncated").and_then(Json::as_bool) == Some(true) {
        line.push_str(" · the last part is cut to fit");
    }
    if from > 1 {
        line.push_str(&format!(" · earlier: to {}", from - 1));
    }
    if to < last {
        line.push_str(&format!(" · later: from {}", to + 1));
    }
    Some(line)
}
