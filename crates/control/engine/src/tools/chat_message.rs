//! Write in a chat that was shared with the agent for writing: under its name, with files, and as a reply to a message by its number.
//!
//! `send_message` with a chat target writes to the people there. It is allowed when a share lets the agent
//! post there (`chat.interact`) at the moment of the call, and the chat takes messages (`chat.enabled`, not
//! archived). The message is a message of that chat authored by the agent: it is shown as the agent's, with the
//! thread it came from one click away; mentions notify as a person's would, and the people who follow the chat are
//! told as for any message. Text is masked like any text stored in a chat, with the patterns and the secrets of the
//! place it goes to. The call is audited as `chat.posted_by_agent`.
//!
//! An agent reaches people no other way: not in a chat nobody shared with it for writing, not in a direct message,
//! not by mail. Files use the shared workspace reader before the message transaction, then the same attachment
//! validation and pinning as a person's message. A refused file leaves the whole message unsent.
//!
//! A shared-tree path or `id:` that the message names in a link or in backquotes, with `#n` for a part, is resolved
//! from the sending thread and kept as a link to its typed reference, so that a rename moves nothing: people see a
//! chip, and an agent that reads the chat is given it in its own terms. The answer lists each reference that
//! resolved and each kept as written.

use crate::engine::Engine;
use crate::error::EngineError;
use crate::ids;
use crate::tools::definitions::{Code, FILES_MAX, Refusal, TEXT_MAX_CHARS};
use crate::tools::message::Message;
use crate::tools::references::Resolution;
use crate::tools::shared::{Resolved, Target, audit_by_agent};
use crate::tools::workspace_files::WorkspaceFile;
use crate::tools::{Answer, CallContext};
use orch_access_contract::audit::AuditAction;
use orch_api_contract::chats::ChatPart;
use orch_authz::chats::{ChatPolicy, Reading, Standing};
use orch_authz::{Caller, Principal};
use orch_chat::Acting;
use orch_chat::chats::Loaded;
use orch_chat::mentions::{self, MENTIONS_MAX, Named};
use orch_chat::{messages, notifications, redaction};
use orch_db::audit::Origin;
use orch_db::queries::chats::{self as chat_rows, ChatRow, NewMessage};
use orch_db::queries::hierarchy;
use orch_db::queries::shares as share_rows;
use orch_db::queries::threads::ThreadRow;
use orch_db::secrets::MasterKey;
use orch_settings::Place;
use orch_settings_contract::registry::keys;
use orch_types::{Id, Time};
use serde_json::json;
use std::sync::OnceLock;

static MASTER_KEY: OnceLock<MasterKey> = OnceLock::new();

/// The shipped rolling-hour broadcast bound, used when no stored override is present.
const DEFAULT_POSTS_PER_HOUR: i64 = 60;

/// Gives the engine the key that opens the secrets of a place, so that what an agent writes in a chat is masked
/// by them as what a person writes is. Installed once, by the program that holds the key; false when one was.
pub fn install_master_key(key: MasterKey) -> bool {
    MASTER_KEY.set(key).is_ok()
}

fn master_key() -> Result<&'static MasterKey, String> {
    MASTER_KEY
        .get()
        .ok_or_else(|| "the key that opens secrets is not installed".to_owned())
}

struct Posting {
    parts: Vec<ChatPart>,
    reply: Option<Id>,
}

fn message_id(thread: &ThreadRow, call: &CallContext) -> Id {
    ids::derived(
        "chat_message",
        &[
            thread.node_id.as_bytes().as_slice(),
            call.call_ref.as_bytes(),
        ],
    )
}

fn sent(path: &str, number: i64, parts: &[ChatPart], references: &Resolution) -> Answer {
    let files: Vec<_> = parts
        .iter()
        .filter_map(|part| match part {
            ChatPart::Attachment {
                name, blob, size, ..
            } => Some(json!({ "name": name, "blob": blob, "size": size })),
            _ => None,
        })
        .collect();
    let mut answer = json!({ "target": { "kind": "chat", "path": path }, "status": "posted", "message": number, "files": files });
    if !references.is_empty() {
        answer["references"] = references.feedback();
    }
    Answer::Done(answer)
}

/// Why a chat takes no message, said to the agent that tried.
fn closed(path: &str, why: &str) -> Refusal {
    Refusal::new(
        Code::Closed,
        format!("{path} takes no messages: {why}. Nothing can be written there."),
    )
}

impl Engine {
    /// The chat route of `send_message`: the call of an agent.
    #[allow(
        clippy::too_many_lines,
        reason = "a call is checked in the order the refusals are told in, and then written"
    )]
    pub(super) async fn send_chat_message_call(
        &self,
        call: &CallContext,
        thread: &ThreadRow,
        arguments: Message,
    ) -> Result<Answer, EngineError> {
        let text = arguments.text.unwrap_or_default();
        let files = arguments.files.unwrap_or_default();
        if text.chars().count() > TEXT_MAX_CHARS || files.len() > FILES_MAX {
            return Ok(Refusal::invalid(
                "\"text\" is at most 8000 characters and \"files\" takes at most 10 entries.",
            )
            .into());
        }
        if text.trim().is_empty() && files.is_empty() {
            return Ok(
                Refusal::invalid("Give text, files, or both: there is nothing to send.").into(),
            );
        }
        let mut conn = self.inner.db.acquire().await?;
        let resolved = match self
            .resolve_write(&mut conn, thread.node_id, &arguments.path, false)
            .await?
        {
            Ok(resolved) => resolved,
            Err(refusal) => return Ok(refusal.into()),
        };
        let (chat_id, node) = match &resolved.target {
            Target::NodeChat { node, chat } => (*chat, Some(*node)),
            Target::PeopleChat { chat } => (*chat, None),
            Target::Conversation { .. } => {
                return Ok(Refusal::new(
                    Code::WrongKind,
                    format!("{} is a conversation, which nothing is written in. To hand something to its agent, use send_message with target.kind \"thread\" and that thread's path.", resolved.path),
                )
                .into());
            }
            Target::Document { .. } | Target::Version { .. } => {
                return Ok(Refusal::new(
                    Code::WrongKind,
                    format!(
                        "{} is a document, not a chat. A document is changed with edit_document.",
                        resolved.path
                    ),
                )
                .into());
            }
            Target::DocumentFile { .. } | Target::ChatFile { .. } => {
                return Ok(Refusal::new(Code::WrongKind,
                    "That path names an attachment, not a chat. Use download_shared to copy its file, or send_message with the chat.md path to post in the chat.").into());
            }
            Target::Directory(_) => {
                return Ok(Refusal::new(
                    Code::WrongKind,
                    format!("{} is a directory, not a chat. Name the chat.md in it, as list_shared shows it.", resolved.path),
                )
                .into());
            }
        };
        let standing = match node {
            Some(node) => resolved
                .tree
                .node_chat_of(node)
                .map(|(_, standing, _)| standing),
            None => resolved
                .tree
                .people_chat_of(chat_id)
                .map(|entry| entry.standing),
        }
        .unwrap_or_default();
        if !standing.writes() {
            if chat_rows::posting::is_stopped(&mut conn, chat_id, thread.node_id).await? {
                return Ok(closed(
                    &resolved.path,
                    "a person stopped your posting here; ask this chat's manager to allow it again",
                )
                .into());
            }
            return Ok(Refusal::new(
                Code::NotWritable,
                format!(
                    "You may read {} and not write in it: posting takes the right chat.interact there, which a person can give you by sharing the chat with you for writing.",
                    resolved.path
                ),
            )
            .into());
        }
        if let Some(node) = node {
            if resolved.tree.reach().archived(node) {
                return Ok(closed(&resolved.path, "it is archived").into());
            }
            let enabled = orch_settings::get::<bool>(&mut conn, Place::Node(node), "chat.enabled")
                .await?
                .unwrap_or(true);
            if !enabled {
                return Ok(
                    closed(&resolved.path, "chats are switched off here (chat.enabled)").into(),
                );
            }
        }
        // The chat as it is now.
        let Some(row) = chat_rows::chat(&mut conn, chat_id).await? else {
            return Ok(Refusal::new(Code::NotFound, "That chat is gone.").into());
        };
        if node.is_none() {
            let direct = orch_chat::chats::direct_allowed(&mut conn, row.org_id)
                .await
                .map_err(|error| EngineError::unreadable(error.to_string()))?;
            if !direct {
                return Ok(closed(
                    &resolved.path,
                    "direct and group chats are switched off here (chat.direct)",
                )
                .into());
            }
        }
        // A completed write remains the same message even if its source file changed or disappeared.
        if let Some(found) =
            chat_rows::message(&mut conn, chat_id, message_id(thread, call)).await?
        {
            let number = share_rows::chat_numbers(&mut conn, chat_id, &[found.id])
                .await?
                .get(&found.id)
                .copied()
                .unwrap_or_default();
            let parts: Vec<ChatPart> = serde_json::from_value(found.content)?;
            return Ok(sent(&resolved.path, number, &parts, &Resolution::default()));
        }
        let reply = match arguments.reply_to {
            None => None,
            Some(number) => {
                let found =
                    share_rows::chat_messages_numbered(&mut conn, chat_id, number, number).await?;
                match found.into_iter().next() {
                    Some(found)
                        if !found.row.is_deleted() && found.row.kind == chat_rows::MESSAGE =>
                    {
                        Some(found.row.id)
                    }
                    _ => {
                        return Ok(Refusal::invalid(format!(
                            "There is no message #{number} to answer in {}. Messages are numbered as read_shared shows them.",
                            resolved.path
                        ))
                        .into());
                    }
                }
            }
        };
        let largest = orch_chat::attachments::largest(&mut conn, &row)
            .await
            .map_err(EngineError::unreadable)?;
        // What the message names is resolved from the sending thread and kept typed.
        let resolution = self
            .resolve_references(&mut conn, thread.node_id, &text)
            .await?;
        let text = resolution.store(&text);
        drop(conn);
        let attached: Vec<WorkspaceFile> =
            match self.read_workspace_files(thread, &files, largest).await? {
                Ok(files) => files,
                Err(refusal) => return Ok(refusal.into()),
            };
        let mut parts = Vec::with_capacity(attached.len() + 1);
        if !text.is_empty() {
            parts.push(ChatPart::Text { text });
        }
        parts.extend(attached.into_iter().map(|file| ChatPart::Attachment {
            blob: file.blob,
            name: file.name,
            size: file.size,
            media_type: file.media_type,
        }));
        let outcome = self
            .post_as_agent(thread, &resolved, row, Posting { parts, reply }, call)
            .await?;
        let Ok((number, parts)) = outcome else {
            return Ok(outcome.err().map(Answer::from).unwrap_or_else(|| {
                Answer::Refused(Refusal::new(Code::NotFound, "That chat is gone."))
            }));
        };
        Ok(sent(&resolved.path, number, &parts, &resolution))
    }

    /// Writes and pins the message in one transaction; a repeated call returns the original stored parts.
    async fn post_as_agent(
        &self,
        thread: &ThreadRow,
        resolved: &Resolved,
        chat: ChatRow,
        posting: Posting,
        call: &CallContext,
    ) -> Result<Result<(i64, Vec<ChatPart>), Refusal>, EngineError> {
        let id = message_id(thread, call);
        let original = thread;
        let mut tx = self.inner.db.begin().await?;
        // The action gate is taken by Db::begin; serialize this origin before any destination chat.
        let Some(thread) = orch_db::queries::threads::lock(&mut tx, thread.node_id).await? else {
            return Ok(Err(Refusal::new(
                Code::NotFound,
                "The originating thread is gone.",
            )));
        };
        let locked = chat_rows::lock(&mut tx, chat.id).await?;
        let Some(chat) = locked else {
            return Ok(Err(Refusal::new(Code::NotFound, "That chat is gone.")));
        };
        if let Some(found) = chat_rows::message(&mut tx, chat.id, id).await? {
            let number = share_rows::chat_numbers(&mut tx, chat.id, &[found.id])
                .await?
                .get(&found.id)
                .copied()
                .unwrap_or_default();
            return Ok(Ok((number, serde_json::from_value(found.content)?)));
        }
        if !crate::tools::workspace_files::same_workspace(original, &thread)
            && !orch_chat::attachments::blobs_in(&posting.parts).is_empty()
        {
            return Ok(Err(crate::tools::workspace_files::changed()));
        }
        if chat_rows::posting::is_stopped(&mut tx, chat.id, thread.node_id).await? {
            return Ok(Err(closed(
                &resolved.path,
                "a person stopped this origin's posting here",
            )));
        }
        // The identity and share are resolved again inside the transaction that writes.
        let fresh = match self
            .resolve_write(&mut tx, thread.node_id, &format!("id:{}", chat.id), false)
            .await?
        {
            Ok(fresh) => fresh,
            Err(refusal) => return Ok(Err(refusal)),
        };
        let now = fresh.asking.now.get();
        let current = match fresh.target {
            Target::NodeChat { node, .. } => fresh
                .tree
                .node_chat_of(node)
                .map(|(_, standing, _)| standing),
            Target::PeopleChat { chat } => {
                fresh.tree.people_chat_of(chat).map(|entry| entry.standing)
            }
            _ => None,
        }
        .unwrap_or_default();
        if !current.writes() {
            return Ok(Err(Refusal::new(
                Code::NotWritable,
                "This chat is no longer shared with you for writing.",
            )));
        }
        if let Some(node) = chat.node_id {
            if fresh.tree.reach().archived(node)
                || !orch_settings::get::<bool>(&mut tx, Place::Node(node), "chat.enabled")
                    .await?
                    .unwrap_or(true)
            {
                return Ok(Err(closed(&fresh.path, "the chat is archived or disabled")));
            }
        } else if !orch_chat::chats::direct_allowed(&mut tx, chat.org_id)
            .await
            .map_err(|error| EngineError::unreadable(error.to_string()))?
        {
            return Ok(Err(closed(
                &fresh.path,
                "direct and group chats are disabled",
            )));
        }
        let limit = orch_settings::get::<i64>(
            &mut tx,
            Place::Node(thread.node_id),
            keys::BROADCAST_CHAT_MESSAGES_PER_HOUR,
        )
        .await?
        .unwrap_or(DEFAULT_POSTS_PER_HOUR);
        if limit <= 0 {
            return Ok(Err(Refusal::new(
                Code::RateLimited,
                "Agent chat posting is disabled by broadcast.chat_messages_per_hour; ask the place's managers to change it.",
            )));
        }
        if let Some(next) =
            chat_rows::posting::next_allowed(&mut tx, thread.node_id, limit, now).await?
        {
            return Ok(Err(Refusal::new(
                Code::RateLimited,
                format!(
                    "The rolling-hour limit of {limit} chat posts is reached. The next post is allowed at {}.",
                    next.to_rfc3339()
                ),
            )));
        }
        let members = if chat.is_people_chat() {
            chat_rows::members(&mut tx, chat.id).await?
        } else {
            Vec::new()
        };
        let loaded = Loaded {
            row: chat.clone(),
            members,
            standing: Standing {
                // An agent reaches a chat only through what was shared with it, never through an admin's reach.
                admin_access: None,
                admin_place: None,
                admin_only: Vec::new(),
                participating: true,
                reading: Some(Reading::Page),
                sending: Ok(()),
                moderate: false,
                export: false,
                in_chat: false,
                names_node: false,
            },
            policy: ChatPolicy::default(),
        };
        let keys = &master_key;
        let redactor = redaction::for_chat(&mut tx, &chat, keys)
            .await
            .map_err(|error| EngineError::unreadable(error.to_string()))?;
        let mut parts = posting.parts;
        redactor.mask_parts(&mut parts);
        if let Err(error) = messages::check_parts(&parts, None) {
            return Ok(Err(Refusal::invalid(error.to_string())));
        }
        let largest = orch_chat::attachments::largest(&mut tx, &chat)
            .await
            .map_err(EngineError::unreadable)?;
        if parts
            .iter()
            .any(|part| matches!(part, ChatPart::Attachment { size, .. } if *size > largest))
        {
            return Ok(Err(Refusal::invalid(
                orch_api_contract::messages::for_failure("workspace_file_too_large"),
            )));
        }
        orch_db::queries::uploads::lock_blobs(&mut tx, &orch_chat::attachments::blobs_in(&parts))
            .await?;
        orch_chat::attachments::check(&mut tx, &parts, largest)
            .await
            .map_err(EngineError::unreadable)?;
        let owner =
            Principal::load(&mut tx, Caller::session(thread.owner_id), Time::new(now)).await?;
        let origin = Origin::default();
        let act = Acting {
            principal: &owner,
            now,
            origin: &origin,
        };
        let named = resolve_mentions(&mut tx, &act, &loaded, &parts).await?;
        let seq = chat_rows::next_seq(&mut tx, chat.id)
            .await?
            .ok_or_else(|| EngineError::unreadable("the chat has no sequence"))?;
        let content = serde_json::to_value(&parts)?;
        let search = messages::search_text_of(&parts);
        let agent = agent_name(&mut tx, &thread).await?;
        let message = chat_rows::insert_message(
            &mut tx,
            &NewMessage {
                id,
                chat_id: chat.id,
                kind: chat_rows::MESSAGE,
                author_id: None,
                author_thread_id: Some(thread.node_id),
                author_agent: Some(&agent),
                content: &content,
                reply_to_id: posting.reply,
                mentions: &named.users,
                mentions_all: named.everyone,
                client_id: None,
                search_text: search.as_deref(),
                seq,
                at: now,
            },
        )
        .await?;
        orch_chat::attachments::pin(&mut tx, &parts)
            .await
            .map_err(EngineError::unreadable)?;
        chat_rows::note_message(&mut tx, chat.id, seq, now).await?;
        if let Some(node) = chat.node_id {
            hierarchy::stamp(&mut tx, node).await?;
        }
        notifications::after_post(&mut tx, &act, &loaded, &message, &named, true)
            .await
            .map_err(|error| EngineError::unreadable(error.to_string()))?;
        chat_rows::notify(&mut tx, chat.id, seq).await?;
        let number = share_rows::chat_total(&mut tx, chat.id).await?;
        audit_by_agent(
            &mut tx,
            &fresh.asking,
            orch_db::audit::action!(AuditAction::ChatPostedByAgent),
            "chat",
            chat.id,
            chat.node_id,
            "An agent wrote in a chat.",
            json!({ "path": fresh.path, "message": number, "thread": thread.node_id }),
        )
        .await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        metrics::counter!("orch_chat_messages_total","kind"=>loaded.row.kind.clone()).increment(1);
        Ok(Ok((number, parts)))
    }
}

/// The name an agent's message is shown under: its profile's.
async fn agent_name(conn: &mut orch_db::Conn, thread: &ThreadRow) -> Result<String, EngineError> {
    Ok(match thread.agent_id {
        Some(agent) => share_rows::agent_names(conn, &[agent])
            .await?
            .remove(&agent)
            .unwrap_or_else(|| "Agent".to_owned()),
        None => "Agent".to_owned(),
    })
}

/// Who an agent's message names: individual usernames of current readers.
/// `@all` and role/group words remain ordinary text for an agent.
async fn resolve_mentions(
    conn: &mut orch_db::Conn,
    act: &Acting<'_>,
    chat: &Loaded,
    parts: &[ChatPart],
) -> Result<Named, EngineError> {
    let mut names: Vec<String> = Vec::new();
    for part in parts {
        if let ChatPart::Text { text } = part {
            let (found, _all) = mentions::scan(text);
            for name in found {
                if !names.contains(&name) {
                    names.push(name);
                }
            }
        }
    }
    names.truncate(MENTIONS_MAX);
    if names.is_empty() {
        return Ok(Named {
            users: Vec::new(),
            everyone: false,
        });
    }
    let found = chat_rows::standing_by_username(conn, chat.row.org_id, &names).await?;
    let candidates: Vec<Id> = names
        .iter()
        .filter_map(|name| {
            found
                .iter()
                .find(|person| person.username.to_lowercase() == *name)
        })
        .map(|person| person.id)
        .collect();
    let readers = orch_authz::chats::readers_among(
        conn,
        &chat.row,
        &chat.members,
        &candidates,
        Time::new(act.now),
    )
    .await?;
    Ok(Named {
        users: candidates
            .into_iter()
            .filter(|id| readers.contains(id))
            .collect(),
        everyone: false,
    })
}
