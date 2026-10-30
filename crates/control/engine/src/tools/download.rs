//! Copy an entry of the shared tree into the agent's workspace: a file as it is, anything else in its written form or as records; large ones as visible jobs.
//!
//! `download_shared` writes one entry into the directory of the agent's thread: a conversation, a chat, a document or
//! one of its versions in its written form, whole, at the depth asked (the text `read_shared` gives, only all of
//! it), or with `format: "json"` as records for a program. It lands under `.orch/shared/downloads/` unless `save_to`
//! names another place under the thread's directory. A file that is there already with the same bytes is left
//! alone, so that asking again is harmless; one that differs is not replaced, and the refusal says so.
//!
//! The bytes are written by what writes files into workspaces ([`crate::sharing::Parcels`]), the writer that
//! delivers the files of a message; with none connected a download is refused and says why. The files that a chat
//! or a document carries are read as their original bytes from the attached blob store. They retain the owner's
//! Read and Export gates, are bounded by storage.max_blob_bytes, and are rechecked before workspace delivery.

use crate::engine::Engine;
use crate::error::EngineError;
use crate::sharing::FileWritten;
use crate::tools::definitions::{Code, Refusal};
use crate::tools::shared::{Resolved, Target, audit_by_agent};
use crate::tools::tree::{attachments::carried_files, tree_name};
use crate::tools::{Answer, CallContext, documents};
use crate::written::{self, Detail};
use futures::StreamExt;
use orch_access_contract::audit::AuditAction;
use orch_api_contract::chats::ChatPart;
use orch_blob_contract::store::{BlobError, Verifier};
use orch_db::queries::chats as chat_rows;
use orch_db::queries::documents as document_rows;
use orch_db::queries::hierarchy;
use orch_db::queries::shares as share_rows;
use orch_db::queries::threads::ThreadRow;
use orch_settings::Place;
use orch_settings_contract::registry::keys;
use orch_types::{Hash, Json};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Where a download is written when `save_to` does not say: under the thread's directory.
pub const DOWNLOADS_DIRECTORY: &str = ".orch/shared/downloads";

/// The longest `save_to` that is taken: more than any path a workspace holds.
const TO_MAX_CHARS: usize = 512;

/// The arguments of `download_shared`, once the schema has let them through.
#[derive(Debug, Deserialize)]
struct Arguments {
    path: String,
    save_to: Option<String>,
    detail: Option<String>,
    format: Option<String>,
}

/// A single live attachment, bound to the exact owner/slot and its canonical safe filename.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub(super) struct Attachment {
    name: String,
    blob: Hash,
    size: Option<i64>,
}

/// Whether `save_to` names a place under the thread's directory: relative, and going nowhere above it.
fn under_the_directory(to: &str) -> bool {
    !to.is_empty()
        && to.chars().count() <= TO_MAX_CHARS
        && !to.starts_with('/')
        && !to.contains('\0')
        && to.split('/').all(|part| part != "..")
}

impl Engine {
    /// `download_shared`: the call of an agent.
    #[allow(
        clippy::too_many_lines,
        reason = "one call, in the order the agent is answered: the arguments, the entry, its bytes, then the writing"
    )]
    pub(crate) async fn download_shared_call(
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
        let records = match arguments.format.as_deref() {
            None | Some("markdown") => false,
            Some("json") => true,
            Some(_) => {
                return Ok(Refusal::invalid("\"format\" is markdown or json.").into());
            }
        };
        if let Some(to) = arguments.save_to.as_deref()
            && !under_the_directory(to)
        {
            return Ok(Refusal::invalid(
                "\"save_to\" is a path under your directory: it does not begin with / and does not go up with ..",
            )
            .into());
        }
        if let Some(answer) = self.rejoin_native_download(call).await? {
            return Ok(answer);
        }
        let Some(writer) = self.parcels() else {
            return Ok(Refusal::new(
                Code::Closed,
                "Nothing is connected that writes files into your workspace from here, so nothing can be downloaded. Read it with read_shared instead.",
            )
            .into());
        };
        let mut conn = self.inner.db.acquire().await?;
        let mut resolved = match self
            .resolve(&mut conn, thread.node_id, &arguments.path, true)
            .await?
        {
            Ok(resolved) => resolved,
            Err(refusal) if refusal.code == Code::NotFound => {
                return Ok(Refusal::new(
                    Code::NotFound,
                    "That shared entry is no longer available.",
                )
                .into());
            }
            Err(refusal) => return Ok(refusal.into()),
        };
        if matches!(resolved.target, Target::Directory(_)) {
            return Ok(Refusal::new(
                Code::WrongKind,
                format!(
                    "{} is a directory. Download a conversation, a chat or a document, or a file in one.",
                    resolved.path
                ),
            )
            .into());
        }
        let attachment = if matches!(
            resolved.target,
            Target::DocumentFile { .. } | Target::ChatFile { .. }
        ) {
            match attachment_to_download(&mut conn, &resolved).await? {
                Ok(file) => Some(file),
                Err(refusal) => return Ok(refusal.into()),
            }
        } else {
            None
        };
        // The export of a place: downloading is taking away as a file, which the Export row lets the agents do.
        if !may_take_away(&resolved) {
            return Ok(not_downloadable(&resolved).into());
        }
        let expected = super::source::download_admission::stamp(&mut conn, &resolved).await?;
        let (name, bytes) = if let Some(attachment) = attachment {
            let largest =
                orch_settings::get::<i64>(&mut conn, Place::System, keys::STORAGE_MAX_BLOB_BYTES)
                    .await?
                    .ok_or_else(|| {
                        EngineError::unreadable("the download file limit has no resolved value")
                    })?;
            // No transaction or metadata connection is held across blob I/O.
            drop(conn);
            let bytes = match self.attachment_bytes(&attachment, largest).await {
                Ok(bytes) => bytes,
                Err(refusal) => return Ok(refusal.into()),
            };
            conn = self.inner.db.acquire().await?;
            let fresh = match self
                .resolve(&mut conn, thread.node_id, &arguments.path, true)
                .await?
            {
                Ok(fresh) => fresh,
                Err(_) => return Ok(missing_attachment().into()),
            };
            if !same_attachment_target(&resolved.target, &fresh.target) {
                return Ok(missing_attachment().into());
            }
            let current = match attachment_to_download(&mut conn, &fresh).await? {
                Ok(file) => file,
                Err(refusal) => return Ok(refusal.into()),
            };
            if !may_take_away(&fresh) {
                return Ok(not_downloadable(&fresh).into());
            }
            if current != attachment {
                return Ok(changed_attachment().into());
            }
            let current_limit =
                orch_settings::get::<i64>(&mut conn, Place::System, keys::STORAGE_MAX_BLOB_BYTES)
                    .await?
                    .ok_or_else(|| {
                        EngineError::unreadable("the download file limit has no resolved value")
                    })?;
            if i64::try_from(bytes.len()).unwrap_or(i64::MAX) > current_limit {
                return Ok(attachment_too_large(current_limit).into());
            }
            resolved = fresh;
            (attachment.name, bytes)
        } else {
            match self
                .bytes_to_download(&mut conn, &resolved, detail, records)
                .await?
            {
                Ok(found) => found,
                Err(refusal) => return Ok(refusal.into()),
            }
        };
        let to = arguments
            .save_to
            .unwrap_or_else(|| format!("{DOWNLOADS_DIRECTORY}/{name}"));
        let size = i64::try_from(bytes.len()).unwrap_or(i64::MAX);
        // The returned destination is necessary to use this file. Check it before the write, including
        // rendered marks and the longest status/counters; an effect must never lose its destination.
        if !orch_agent_contract::tool_answers::fits(
            "download_shared",
            &call.arguments,
            &resolved.annotate(
                json!({"status":"downloading","path":to,"size":size,"received":i64::MAX}),
            ),
        ) {
            return Ok(Refusal::new(Code::TooLarge, "The destination and complete download outcome do not fit max_bytes. Nothing was written. Use a shorter save_to or request more max_bytes (at most 40000).").into());
        }
        let proof = super::source::download_admission::proof(
            call,
            thread,
            &resolved,
            expected,
            to.clone(),
            Hash::of(&bytes),
            size,
        )?;
        let checked = super::source::download_admission::scope(
            proof,
            writer.write_file(&mut conn, thread.node_id, &to, &bytes),
        )
        .await;
        if let Some(refusal) = checked.refused {
            return Ok(refusal.into());
        }
        let written = checked.result.map_err(|fault| {
            EngineError::unreadable(format!(
                "a file could not be written into a workspace: {fault}"
            ))
        })?;
        if let Some(delivery) = checked.delivery {
            return super::source::download_admission::outcome(self, call, &delivery, written)
                .await;
        }
        let status = match written {
            FileWritten::Ready => {
                json!({ "status": "ready", "path": to, "size": size, "received": size })
            }
            FileWritten::Downloading { received } => {
                json!({ "status": "downloading", "path": to, "size": size, "received": received })
            }
            FileWritten::Differs => {
                return Ok(Refusal::new(
                    Code::Exists,
                    format!(
                        "{to} is there already with other bytes, and a download replaces nothing. Give another \"save_to\", or remove that file first."
                    ),
                )
                .into());
            }
        };
        let (action, kind, object, node) = match &resolved.target {
            Target::Conversation { thread: other } => (
                AuditAction::ThreadReadByAgent,
                "thread",
                *other,
                Some(*other),
            ),
            Target::NodeChat { chat, node } => {
                (AuditAction::ChatReadByAgent, "chat", *chat, Some(*node))
            }
            Target::PeopleChat { chat } => (AuditAction::ChatReadByAgent, "chat", *chat, None),
            Target::ChatFile { chat, node, .. } => {
                (AuditAction::ChatReadByAgent, "chat", *chat, *node)
            }
            Target::Document { document }
            | Target::Version { document, .. }
            | Target::DocumentFile { document, .. } => (
                AuditAction::DocumentReadByAgent,
                "document",
                *document,
                None,
            ),
            Target::Directory(_) => return Ok(status_answer(status)),
        };
        audit_by_agent(
            &mut conn,
            &resolved.asking,
            action,
            kind,
            object,
            node,
            "An agent read something to copy it into its workspace.",
            json!({ "path": resolved.path, "downloaded": true }),
        )
        .await?;
        audit_by_agent(
            &mut conn,
            &resolved.asking,
            orch_db::audit::action!(AuditAction::ShareFileDownloaded),
            kind,
            object,
            node,
            "An agent copied something it was shared into its workspace.",
            json!({ "path": resolved.path, "to": to, "bytes": size }),
        )
        .await?;
        Ok(status_answer(resolved.annotate(status)))
    }

    /// Rejoin the already admitted native command and its frozen reply context, never render/author another copy.
    async fn rejoin_native_download(
        &self,
        call: &CallContext,
    ) -> Result<Option<Answer>, EngineError> {
        use crate::commands::Outcome;
        let Some((command, metadata)) = super::source::download_state(self).await? else {
            return Ok(None);
        };
        let delivery: super::source::download_admission::Delivery =
            serde_json::from_value(metadata)?;
        let command = super::source::issue(self, command).await?;
        let outcome = tokio::time::timeout(
            crate::parcels::DOWNLOAD_WAIT,
            self.inner.commands.wait(command.id),
        )
        .await;
        let written=match outcome {
            Ok(Ok(Outcome::Succeeded(_)))=>FileWritten::Ready,
            Ok(Ok(Outcome::Failed(error))) if error.code=="conflict"=>{
                let environment=serde_json::from_value(command.body["environment_id"].clone())?;
                let path=command.body["path"].as_str().ok_or(EngineError::ToolCallIdentity)?;
                self.same_file_there(command.machine,environment,path,&delivery.blob).await?
            }
            Err(_)=>FileWritten::Downloading {received:0},
            Ok(Err(error))=>return Err(error.into()),
            Ok(Ok(_))=>return Ok(Some(Refusal::new(Code::Closed,
                "This call's admitted download did not confirm its result. Its effect may have occurred; inspect the destination before a new call.").into())),
        };
        super::source::download_admission::outcome(self, call, &delivery, written)
            .await
            .map(Some)
    }

    /// Reads a whole original blob, refusing a short, damaged or oversized stream before any workspace effect.
    async fn attachment_bytes(
        &self,
        attachment: &Attachment,
        largest: i64,
    ) -> Result<Vec<u8>, Refusal> {
        if attachment.size.is_some_and(|size| size < 0) {
            return Err(missing_attachment());
        }
        if attachment.size.is_some_and(|size| size > largest) {
            return Err(attachment_too_large(largest));
        }
        let store = self.blobs().ok_or_else(|| {
            Refusal::new(
                Code::Closed,
                "The file store is unavailable. Nothing was written. Try again.",
            )
        })?;
        let read = store
            .get(&attachment.blob, None)
            .await
            .map_err(blob_refusal)?;
        if read.meta.size > largest {
            return Err(attachment_too_large(largest));
        }
        if read.meta.size < 0
            || read.meta.hash != attachment.blob
            || attachment.size.is_some_and(|size| size != read.meta.size)
        {
            return Err(changed_attachment());
        }
        let mut verifier = Verifier::new(attachment.blob, Some(read.meta.size));
        let mut stream = read.body;
        let mut bytes = Vec::new();
        while let Some(piece) = stream.next().await {
            let piece = piece.map_err(blob_refusal)?;
            let next = i64::try_from(bytes.len().saturating_add(piece.len()))
                .map_err(|_| attachment_too_large(largest))?;
            if next > largest {
                return Err(attachment_too_large(largest));
            }
            bytes.try_reserve(piece.len()).map_err(|_| Refusal::new(Code::Closed,
                "There is not enough memory to read this attachment. Nothing was written. Try again."))?;
            verifier.update(&piece);
            bytes.extend_from_slice(&piece);
        }
        verifier.finish().map_err(|_| changed_attachment())?;
        Ok(bytes)
    }

    /// The name and the bytes of a non-file download: the whole written form, or records.
    async fn bytes_to_download(
        &self,
        conn: &mut orch_db::Conn,
        resolved: &Resolved,
        detail: Detail,
        records: bool,
    ) -> Result<Result<(String, Vec<u8>), Refusal>, EngineError> {
        let stem = tree_name(
            resolved
                .path
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or("entry")
                .trim_end_matches(".md"),
        );
        if !records {
            let got = match self
                .read_target(conn, resolved, (None, None), detail, None)
                .await?
            {
                Ok(got) => got,
                Err(refusal) => return Ok(Err(refusal)),
            };
            let name = format!("{}.md", written::clean_name(&stem));
            let mut text = got.text;
            // A document that is copied whole is checked out: its file says which document it is and at which
            // sequence, so that a commit of the file (`edit_document` `rewrite` with `file`) knows where it was written.
            if let (Target::Document { document }, Detail::Normal) = (&resolved.target, detail)
                && let Some(row) = document_rows::get(conn, *document).await?
            {
                let body = text.split_once("\n\n").map_or("", |(_, rest)| rest);
                text = orch_documents::checkout::write(
                    &orch_documents::checkout::Checkout {
                        document: row.id,
                        seq: row.doc_seq,
                        title: Some(row.title.clone()),
                    },
                    body,
                );
            }
            return Ok(Ok((name, text.into_bytes())));
        }
        let value: Json = match &resolved.target {
            Target::Conversation { thread } => {
                // Read on this call's own connection, as its written form is: a reading transaction of its
                // own would set its isolation after the call's guard has already spoken, which the database
                // refuses.
                let data = self
                    .export_view_in(
                        conn,
                        *thread,
                        crate::views::ExportSelection {
                            range: (None, None),
                            reasoning: false,
                            sub_streams: false,
                            continuation: None,
                            revision: None,
                        },
                    )
                    .await
                    .map_err(|error| EngineError::unreadable(error.to_string()))?;
                json!({ "segments": data.segments, "turns": data.turns, "items": data.items })
            }
            Target::NodeChat { chat, .. } | Target::PeopleChat { chat } => {
                let total = share_rows::chat_total(conn, *chat).await?;
                let messages =
                    share_rows::chat_messages_numbered(conn, *chat, 1, total.max(1)).await?;
                json!(
                    messages
                        .into_iter()
                        .map(|message| json!({
                            "number": message.number,
                            "id": message.row.id,
                            "author": message.row.author_id,
                            "agent": message.row.author_agent,
                            "at": message.row.created_at,
                            "deleted": message.row.is_deleted(),
                            "parts": message.row.content,
                        }))
                        .collect::<Vec<_>>()
                )
            }
            Target::Document { document } => {
                let Some(row) = document_rows::get(conn, *document).await? else {
                    return Ok(Err(Refusal::new(Code::NotFound, "That document is gone.")));
                };
                let blocks: Vec<_> = documents::live(conn, row.id)
                    .await?
                    .iter()
                    .map(documents::block_of)
                    .collect();
                json!({ "id": row.id, "title": row.title, "seq": row.doc_seq, "blocks": blocks })
            }
            Target::Version { .. } => {
                return Ok(Err(Refusal::new(
                    Code::WrongKind,
                    "An earlier version of a document is downloaded in its written form; records are of the document as it is.",
                )));
            }
            Target::Directory(_) => {
                return Ok(Err(Refusal::new(Code::WrongKind, "That is a directory.")));
            }
            Target::DocumentFile { .. } | Target::ChatFile { .. } => {
                return Ok(Err(Refusal::new(
                    Code::WrongKind,
                    "An attachment is downloaded as its original file bytes.",
                )));
            }
        };
        let bytes = serde_json::to_vec_pretty(&value)?;
        Ok(Ok((format!("{}.json", written::clean_name(&stem)), bytes)))
    }
}

fn status_answer(status: Json) -> Answer {
    Answer::Done(status)
}

fn missing_attachment() -> Refusal {
    Refusal::new(Code::NotFound, "That attachment is no longer available.")
}

fn changed_attachment() -> Refusal {
    Refusal::new(
        Code::Changed,
        "The attachment changed while it was being read. Nothing was written. List its files and try again.",
    )
}

fn attachment_too_large(largest: i64) -> Refusal {
    Refusal::new(
        Code::TooLarge,
        format!(
            "The attachment exceeds storage.max_blob_bytes ({largest} bytes). Nothing was written."
        ),
    )
}

fn blob_refusal(error: BlobError) -> Refusal {
    match error {
        BlobError::NotFound(_) => missing_attachment(),
        BlobError::HashMismatch { .. } | BlobError::SizeMismatch { .. } | BlobError::Corrupt(_) => {
            changed_attachment()
        }
        _ => Refusal::new(
            Code::Closed,
            "The attachment could not be read from file storage. Nothing was written. Try again.",
        ),
    }
}

fn not_downloadable(resolved: &Resolved) -> Refusal {
    Refusal::new(
        Code::NotDownloadable,
        format!(
            "You may read {} with read_shared, and not copy it into your workspace: whoever shared it may not take it away as a file, or left that out. A person who may export it can share it for download.",
            resolved.path
        ),
    )
}

fn same_attachment_target(first: &Target, second: &Target) -> bool {
    match (first, second) {
        (
            Target::DocumentFile {
                document: a,
                file: x,
            },
            Target::DocumentFile {
                document: b,
                file: y,
            },
        ) => a == b && x == y,
        (
            Target::ChatFile {
                chat: a,
                node: n,
                message: x,
                carried: i,
            },
            Target::ChatFile {
                chat: b,
                node: m,
                message: y,
                carried: j,
            },
        ) => a == b && n == m && x == y && i == j,
        _ => false,
    }
}

/// Re-read precisely the live document file or message slot, never a file belonging to another owner.
pub(super) async fn attachment_to_download(
    conn: &mut orch_db::Conn,
    resolved: &Resolved,
) -> Result<Result<Attachment, Refusal>, EngineError> {
    let Some(name) = resolved
        .path
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty() && *name != "." && *name != "..")
    else {
        return Ok(Err(missing_attachment()));
    };
    let (blob, size) = match &resolved.target {
        Target::DocumentFile { document, file } => {
            let Some(owner) = document_rows::get(conn, *document)
                .await?
                .filter(|owner| owner.deleted_at.is_none())
            else {
                return Ok(Err(missing_attachment()));
            };
            if !live_parent(conn, owner.node_id).await? {
                return Ok(Err(missing_attachment()));
            }
            let Some(file) = document_rows::file(conn, owner.id, *file).await? else {
                return Ok(Err(missing_attachment()));
            };
            if file.file.name != name {
                return Ok(Err(missing_attachment()));
            }
            (file.blob, Some(file.file.size))
        }
        Target::ChatFile {
            chat,
            node,
            message,
            carried,
        } => {
            let Some(owner) = chat_rows::chat(conn, *chat).await? else {
                return Ok(Err(missing_attachment()));
            };
            if owner.node_id != *node {
                return Ok(Err(missing_attachment()));
            }
            if !live_parent(conn, owner.node_id.unwrap_or(owner.org_id)).await? {
                return Ok(Err(missing_attachment()));
            }
            let Some(row) = chat_rows::message(conn, *chat, *message)
                .await?
                .filter(|row| !row.is_deleted() && row.kind == chat_rows::MESSAGE)
            else {
                return Ok(Err(missing_attachment()));
            };
            let Ok(parts) = serde_json::from_value::<Vec<ChatPart>>(row.content) else {
                return Ok(Err(missing_attachment()));
            };
            let files = carried_files(&parts);
            let Some(file) = carried.checked_sub(1).and_then(|at| files.get(at)) else {
                return Ok(Err(missing_attachment()));
            };
            let numbers = share_rows::chat_numbers(conn, *chat, &[*message]).await?;
            let Some(number) = numbers.get(message) else {
                return Ok(Err(missing_attachment()));
            };
            if orch_chat::written::file_name(*number, *carried, &file.name) != name {
                return Ok(Err(missing_attachment()));
            }
            (file.blob, file.size)
        }
        _ => return Ok(Err(missing_attachment())),
    };
    Ok(Ok(Attachment {
        name: written::clean_name(name),
        blob,
        size,
    }))
}

async fn live_parent(
    conn: &mut orch_db::Conn,
    parent: orch_types::Id,
) -> Result<bool, EngineError> {
    let path = hierarchy::with_ancestors(conn, &[parent]).await?;
    Ok(path.iter().any(|node| node.id == parent)
        && path.iter().all(|node| node.deleted_at.is_none()))
}

/// Whether what a path names may be taken away as a file by the agent that reads it: the Export row of its area
/// names the agents (the export of a place).
pub(super) fn may_take_away(resolved: &Resolved) -> bool {
    let reach = resolved.tree.reach();
    match &resolved.target {
        Target::Directory(_) => false,
        Target::Conversation { thread } => reach.may_download_conversation(*thread, false),
        Target::NodeChat { node, .. } => reach.may_download_node_chat(*node),
        Target::PeopleChat { chat } => reach.may_download_people_chat(*chat),
        Target::ChatFile { chat, node, .. } => match node {
            Some(node) => reach.may_download_node_chat(*node),
            None => reach.may_download_people_chat(*chat),
        },
        Target::Document { document }
        | Target::Version { document, .. }
        | Target::DocumentFile { document, .. } => resolved
            .tree
            .document(*document)
            .is_some_and(|entry| reach.may_download_document(&entry.document)),
    }
}
