//! Posting by people, and by agents where a chat was shared with them for writing; editing, deleting, replies, reactions, pins and masking.
//!
//! Every change to a message is a row stamped with the next number of the
//! chat's own sequence in the transaction that makes it, so that everything
//! changed after a number describes every change: an edit, a reaction, a pin
//! and a deletion each stamp the message they change. A deletion empties what
//! the message said, its mentions, reactions and the words a search finds it
//! by, takes away its pin and its quotes, and keeps the row. Text is masked
//! before it is stored.

use crate::chats::{self, Loaded};
use crate::mentions;
use crate::notifications::{self, Told};
use crate::quotes;
use crate::redaction::Redactor;
use crate::views;
use crate::{Acting, Error, attachments};
use chrono::{DateTime, Utc};
use orch_access_contract::audit::AuditAction;
use orch_access_contract::permissions::Permission;
use orch_api_contract::chats::{
    CHAT_MESSAGE_MAX_EMOJI, CHAT_MESSAGE_MAX_PARTS, CHAT_MESSAGE_MAX_TEXT_CHARS,
    CHAT_MESSAGES_DEFAULT_LIMIT, CHAT_MESSAGES_MAX_LIMIT, CHAT_SYNC_MAX_CHANGES, ChatPart,
    ChatSystemEvent,
};
use orch_db::Conn;
use orch_db::audit::{self, Actor, Entry};
use orch_db::queries::chats::{self as rows, ChatRow, Edit, MessageRow, NewMessage};
use orch_db::queries::hierarchy;
use orch_types::{Id, Json, Seq};
use serde_json::json;
use std::collections::HashMap;

/// The most characters of the value a client sends to make sending a message
/// again harmless: a UUID is 36, and the schema indexes what is kept.
pub const CLIENT_ID_MAX_CHARS: usize = 200;

/// The most characters of the emoji a reaction is named by: the longest
/// sequences of joined emoji are a dozen or so, and this leaves room for them.
pub const EMOJI_MAX_CHARS: usize = 16;

/// What a message is made of, as a person sends it.
#[derive(Debug, Clone)]
pub struct Post {
    /// What it says.
    pub parts: Vec<ChatPart>,
    /// The message it replies to.
    pub reply_to: Option<Id>,
    /// What makes sending it again harmless.
    pub client_id: String,
}

/// A message that was written, or found again.
#[derive(Debug, Clone)]
pub struct Written {
    /// The message as it stands.
    pub message: MessageRow,
    /// False when its author had sent it before.
    pub created: bool,
    /// The chat's highest sequence number now.
    pub seq: Seq,
    /// Whom it told.
    pub told: Told,
}

/// What a system line of a chat says.
#[derive(Debug, Clone, Copy)]
pub struct Line<'a> {
    /// What happened.
    pub event: ChatSystemEvent,
    /// Who did it.
    pub by: Option<Id>,
    /// To whom, where it was done to somebody.
    pub user: Option<Id>,
    /// The new name, for a rename.
    pub name: Option<&'a str>,
}

/// Writes a line the system says about the chat, at `seq`, which the caller has
/// taken. It is not a message anybody wrote: it makes no chat unread.
pub async fn system(
    conn: &mut Conn,
    chat: &ChatRow,
    seq: Seq,
    line: &Line<'_>,
    at: DateTime<Utc>,
) -> Result<MessageRow, Error> {
    let content =
        json!({ "event": line.event, "by": line.by, "user": line.user, "name": line.name });
    Ok(rows::insert_message(
        conn,
        &NewMessage {
            id: orch_types::new_id(),
            chat_id: chat.id,
            kind: rows::SYSTEM,
            author_id: None,
            author_thread_id: None,
            author_agent: None,
            content: &content,
            reply_to_id: None,
            mentions: &[],
            mentions_all: false,
            client_id: None,
            search_text: None,
            seq,
            at,
        },
    )
    .await?)
}

/// The words a search finds a message by: its text, and the names of what it
/// carries. None when it has neither.
#[must_use]
pub fn search_text_of(parts: &[ChatPart]) -> Option<String> {
    let mut found: Vec<&str> = Vec::new();
    for part in parts {
        match part {
            ChatPart::PastedText(paste) => found.push(&paste.name),
            ChatPart::Text { text } => found.push(text),
            ChatPart::Image { name, .. } | ChatPart::Attachment { name, .. } => found.push(name),
            ChatPart::Forwarded(copy) => {
                found.push(&copy.text);
                found.extend(copy.images.iter().map(|image| image.name.as_str()));
                found.extend(copy.attachments.iter().map(|file| file.name.as_str()));
            }
            ChatPart::ItemRef { .. } | ChatPart::Mention { .. } => {}
        }
    }
    let joined = found
        .into_iter()
        .filter(|text| !text.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    (!joined.is_empty()).then_some(joined)
}

/// Checks the parts of a message a person sends: at most
/// [`CHAT_MESSAGE_MAX_PARTS`] of them, at most [`CHAT_MESSAGE_MAX_TEXT_CHARS`]
/// characters of text across them, and something said. A copy of a message is
/// made by forwarding it and is never accepted from a person, except that an
/// edit keeps the copies the message holds (`kept`).
pub fn check_parts(parts: &[ChatPart], kept: Option<&[ChatPart]>) -> Result<(), Error> {
    if parts.is_empty() {
        return Err(Error::invalid("parts", "a message has at least one part"));
    }
    if parts.len() > usize::try_from(CHAT_MESSAGE_MAX_PARTS).unwrap_or(usize::MAX) {
        return Err(Error::invalid(
            "parts",
            format!("a message has at most {CHAT_MESSAGE_MAX_PARTS} parts"),
        ));
    }
    let mut characters = 0_usize;
    let mut says_something = false;
    for (at, part) in parts.iter().enumerate() {
        match part {
            ChatPart::Text { text } => {
                characters += text.chars().count();
                says_something |= !text.trim().is_empty();
            }
            ChatPart::Forwarded(_) => {
                if !kept.is_some_and(|kept| kept.contains(part)) {
                    return Err(Error::invalid(
                        format!("parts[{at}]"),
                        "a copy of a message is made by forwarding it",
                    ));
                }
                says_something = true;
            }
            ChatPart::PastedText(paste) => {
                if paste.inline_text.is_some() {
                    return Err(Error::invalid(
                        format!("parts[{at}].inline_text"),
                        "inline text is prepared from its upload when sending to an agent",
                    ));
                }
                if paste.source.is_some() && !kept.is_some_and(|kept| kept.contains(part)) {
                    return Err(Error::invalid(
                        format!("parts[{at}].source"),
                        "a copy of a message is made by forwarding it",
                    ));
                }
                says_something = true;
            }
            ChatPart::Image { .. }
            | ChatPart::Attachment { .. }
            | ChatPart::ItemRef { .. }
            | ChatPart::Mention { .. } => {
                says_something = true;
            }
        }
    }
    if characters > usize::try_from(CHAT_MESSAGE_MAX_TEXT_CHARS).unwrap_or(usize::MAX) {
        return Err(Error::invalid(
            "parts",
            format!("a message has at most {CHAT_MESSAGE_MAX_TEXT_CHARS} characters of text"),
        ));
    }
    if !says_something {
        return Err(Error::invalid("parts", "a message says something"));
    }
    Ok(())
}

fn content_of(parts: &[ChatPart]) -> Result<Json, Error> {
    serde_json::to_value(parts).map_err(|error| Error::Internal(error.to_string()))
}

/// Retains deliberate audience identities beside the frozen people accepted for the message.
async fn save_targets(
    conn: &mut Conn,
    message: &mut MessageRow,
    parts: &[ChatPart],
) -> Result<(), Error> {
    let targets: Vec<_> = parts
        .iter()
        .filter_map(|part| match part {
            ChatPart::Mention { target } => Some(target),
            _ => None,
        })
        .collect();
    let targets =
        serde_json::to_value(targets).map_err(|error| Error::Internal(error.to_string()))?;
    rows::people::targets(conn, message.id, &targets).await?;
    message.mention_targets = targets;
    Ok(())
}

/// Writes a message of the caller's in the chat.
///
/// The caller may write in the chat, the parts are fit ([`check_parts`]), what
/// they carry is known to the store and what they quote is of the thread whose
/// chat it is. A value of `client_id` that this author used before answers
/// with the message they wrote then. People named who can read the chat are
/// counted, engaged and told, and the author is engaged.
pub async fn post(
    conn: &mut Conn,
    act: &Acting<'_>,
    loaded: &mut Loaded,
    input: Post,
    redactor: &Redactor,
) -> Result<Written, Error> {
    write(conn, act, loaded, input, redactor, false).await
}

/// Writes a message that holds copies of other messages, which only
/// forwarding does.
pub(crate) async fn post_copies(
    conn: &mut Conn,
    act: &Acting<'_>,
    loaded: &mut Loaded,
    input: Post,
    redactor: &Redactor,
) -> Result<Written, Error> {
    write(conn, act, loaded, input, redactor, true).await
}

async fn write(
    conn: &mut Conn,
    act: &Acting<'_>,
    loaded: &mut Loaded,
    input: Post,
    redactor: &Redactor,
    copies: bool,
) -> Result<Written, Error> {
    let me = act.user();
    loaded.ensure_send()?;
    let client_id = input.client_id.trim().to_owned();
    if client_id.is_empty() || client_id.chars().count() > CLIENT_ID_MAX_CHARS {
        return Err(Error::invalid(
            "client_id",
            format!("is a value of 1 to {CLIENT_ID_MAX_CHARS} characters"),
        ));
    }
    let chat = loaded.id();
    if let Some(found) = rows::message_by_client(conn, chat, me, &client_id).await? {
        return Ok(Written {
            seq: loaded.row.seq(),
            message: found,
            created: false,
            told: Told::default(),
        });
    }
    check_parts(&input.parts, copies.then_some(input.parts.as_slice()))?;
    let mut parts = input.parts;
    redactor.mask_parts(&mut parts);
    let largest = attachments::largest(conn, &loaded.row).await?;
    crate::following::require_blobs(conn, act.principal, &attachments::blobs_in(&parts)).await?;
    attachments::check(conn, &parts, largest).await?;
    let quoted = quotes::check(conn, act, &loaded.row, &parts).await?;
    if let Some(reply) = input.reply_to
        && rows::message(conn, chat, reply).await?.is_none()
    {
        return Err(Error::invalid("reply_to", "is not a message of this chat"));
    }
    let named = mentions::resolve(conn, act, loaded, &parts, Some(me)).await?;
    // The thread's number first, the chat's second.
    let thread_seq = match &quoted {
        Some(quoted) => Some(quotes::begin(conn, quoted.thread).await?),
        None => None,
    };
    loaded.lock(conn).await?;
    if let Some(found) = rows::message_by_client(conn, chat, me, &client_id).await? {
        return Ok(Written {
            seq: loaded.row.seq(),
            message: found,
            created: false,
            told: Told::default(),
        });
    }
    let seq = chats::take(conn, chat).await?;
    let content = content_of(&parts)?;
    let search = search_text_of(&parts);
    let mut message = rows::insert_message(
        conn,
        &NewMessage {
            id: orch_types::new_id(),
            chat_id: chat,
            kind: rows::MESSAGE,
            author_id: Some(me),
            author_thread_id: None,
            author_agent: None,
            content: &content,
            reply_to_id: input.reply_to,
            mentions: &named.users,
            mentions_all: named.everyone,
            client_id: Some(&client_id),
            search_text: search.as_deref(),
            seq,
            at: act.now,
        },
    )
    .await?;
    save_targets(conn, &mut message, &parts).await?;
    rows::note_message(conn, chat, seq, act.now).await?;
    if let (Some(quoted), Some(thread_seq)) = (&quoted, thread_seq) {
        quotes::record(
            conn,
            quoted.thread,
            thread_seq,
            message.id,
            &quotes::Changes {
                added: &quoted.items,
                removed: &[],
            },
            act.now,
        )
        .await?;
    }
    attachments::pin(conn, &parts).await?;
    if let Some(node) = loaded.row.node_id {
        hierarchy::stamp(conn, node).await?;
    }
    let told = notifications::after_post(conn, act, loaded, &message, &named, false).await?;
    rows::notify(conn, chat, seq).await?;
    loaded.row.next_seq = seq + 1;
    Ok(Written {
        message,
        created: true,
        seq,
        told,
    })
}

fn live(found: Option<MessageRow>) -> Result<MessageRow, Error> {
    let found = found.ok_or(Error::NotFound)?;
    if found.kind != rows::MESSAGE {
        return Err(Error::invalid(
            "message_id",
            "is a line the system wrote, which nobody changes",
        ));
    }
    if found.is_deleted() {
        return Err(Error::invalid("message_id", "was deleted"));
    }
    Ok(found)
}

fn difference(of: &[Id], from: &[Id]) -> Vec<Id> {
    of.iter().copied().filter(|id| !from.contains(id)).collect()
}

/// Replaces what a message says. Only its author does, while they can still
/// write in the chat. People who are named now and were not before are counted
/// and told; the quotes and the files it carries are counted again.
pub async fn edit(
    conn: &mut Conn,
    act: &Acting<'_>,
    loaded: &mut Loaded,
    id: Id,
    parts: Vec<ChatPart>,
    redactor: &Redactor,
) -> Result<MessageRow, Error> {
    loaded.ensure_send()?;
    let chat = loaded.id();
    let found = live(rows::message(conn, chat, id).await?)?;
    if found.author_id != Some(act.user()) {
        return Err(Error::NotTheAuthor);
    }
    let observed = found.seq_updated;
    let before = views::parts_of(&found.content);
    check_parts(&parts, Some(&before))?;
    let mut parts = parts;
    redactor.mask_parts(&mut parts);
    let named = mentions::resolve(conn, act, loaded, &parts, Some(act.user())).await?;
    let mut previous = found.mentions.clone();
    previous.sort_unstable();
    previous.dedup();
    if parts == before && named.users == previous && named.everyone == found.mentions_all {
        return Ok(found);
    }
    let largest = attachments::largest(conn, &loaded.row).await?;
    crate::following::require_blobs(conn, act.principal, &attachments::blobs_in(&parts)).await?;
    attachments::check(conn, &parts, largest).await?;
    let quoted = quotes::check(conn, act, &loaded.row, &parts).await?;
    let old_items: Vec<Id> = quotes::refs_in(&before)
        .into_iter()
        .map(|(_, item)| item)
        .collect();
    let new_items: Vec<Id> = quoted
        .as_ref()
        .map(|quoted| quoted.items.clone())
        .unwrap_or_default();
    let (added, removed) = (
        difference(&new_items, &old_items),
        difference(&old_items, &new_items),
    );
    let thread = loaded.row.node_id;
    let thread_seq = match (thread, added.is_empty() && removed.is_empty()) {
        (Some(thread), false) => Some(quotes::begin(conn, thread).await?),
        _ => None,
    };
    loaded.lock(conn).await?;
    let found = live(rows::message(conn, chat, id).await?)?;
    if found.seq_updated != observed {
        return Err(Error::StaleMessage {
            revision: found.seq_updated,
        });
    }
    let seq = chats::take(conn, chat).await?;
    let content = content_of(&parts)?;
    let search = search_text_of(&parts);
    let mut message = rows::edit_message(
        conn,
        &Edit {
            id,
            content: &content,
            mentions: &named.users,
            mentions_all: named.everyone,
            search_text: search.as_deref(),
            seq,
            at: act.now,
        },
    )
    .await?;
    save_targets(conn, &mut message, &parts).await?;
    if let (Some(thread), Some(thread_seq)) = (thread, thread_seq) {
        quotes::record(
            conn,
            thread,
            thread_seq,
            id,
            &quotes::Changes {
                added: &added,
                removed: &removed,
            },
            act.now,
        )
        .await?;
    }
    attachments::unpin(conn, &before).await?;
    attachments::pin(conn, &parts).await?;
    let newly = difference(&named.users, &found.mentions);
    notifications::after_edit(conn, act, loaded, &message, &newly).await?;
    rows::notify(conn, chat, seq).await?;
    loaded.row.next_seq = seq + 1;
    Ok(message)
}

/// Deletes a message: its author, who can still write in the chat; or, in a
/// node's chat, whoever holds `chat.manage` there. The row stays, emptied. A
/// message deleted by somebody other than its author leaves an audit row.
pub async fn delete(
    conn: &mut Conn,
    act: &Acting<'_>,
    loaded: &mut Loaded,
    id: Id,
) -> Result<MessageRow, Error> {
    let chat = loaded.id();
    let found = rows::message(conn, chat, id)
        .await?
        .ok_or(Error::NotFound)?;
    if found.kind != rows::MESSAGE {
        return Err(Error::invalid(
            "message_id",
            "is a line the system wrote, which nobody changes",
        ));
    }
    if found.is_deleted() {
        return Ok(found);
    }
    let observed = found.seq_updated;
    let own = found.author_id == Some(act.user());
    if own {
        loaded.ensure_send()?;
    } else if !loaded.standing.moderate {
        return Err(match loaded.row.node_id {
            Some(node) if !loaded.standing.inspects() => Error::Lacks {
                node,
                permission: Permission::ChatManage,
            },
            _ => Error::NotTheAuthor,
        });
    }
    let quoted = rows::item_refs(conn, id).await?;
    let thread = loaded.row.node_id;
    let thread_seq = match (thread, quoted.is_empty()) {
        (Some(thread), false) => Some(quotes::begin(conn, thread).await?),
        _ => None,
    };
    loaded.lock(conn).await?;
    let found = rows::message(conn, chat, id)
        .await?
        .ok_or(Error::NotFound)?;
    if found.is_deleted() {
        return Ok(found);
    }
    if found.seq_updated != observed {
        return Err(Error::StaleMessage {
            revision: found.seq_updated,
        });
    }
    let seq = chats::take(conn, chat).await?;
    let gone = rows::delete_message(conn, id, act.user(), seq, act.now).await?;
    if let (Some(thread), Some(thread_seq)) = (thread, thread_seq) {
        quotes::record(
            conn,
            thread,
            thread_seq,
            id,
            &quotes::Changes {
                added: &[],
                removed: &quoted,
            },
            act.now,
        )
        .await?;
    }
    attachments::unpin(conn, &views::parts_of(&found.content)).await?;
    if !own {
        let mut entry = Entry::new(
            Actor::User(act.user()),
            orch_db::audit::action!(AuditAction::ChatMessageDeleted).as_str(),
            "chat",
            Some(chat),
            "deleted a message that somebody else wrote",
        );
        entry.node_id = loaded.row.node_id;
        entry.org_id = Some(loaded.row.org_id);
        entry.details = json!({ "message_id": id, "author": found.author_id });
        audit::append(conn, &entry, act.origin, act.now).await?;
    }
    rows::notify(conn, chat, seq).await?;
    loaded.row.next_seq = seq + 1;
    Ok(gone)
}

fn check_emoji(emoji: &str) -> Result<(), Error> {
    let fit = !emoji.is_empty()
        && emoji.chars().count() <= EMOJI_MAX_CHARS
        && !emoji.chars().any(|c| c.is_whitespace() || c.is_control());
    if fit {
        Ok(())
    } else {
        Err(Error::invalid(
            "emoji",
            format!("is an emoji of at most {EMOJI_MAX_CHARS} characters"),
        ))
    }
}

/// Adds the caller's reaction to a message, or takes it away. A reaction the
/// caller has not made cannot be taken away and one they made is not made
/// twice: neither changes the message. A message holds at most
/// [`CHAT_MESSAGE_MAX_EMOJI`] different emoji.
pub async fn react(
    conn: &mut Conn,
    act: &Acting<'_>,
    loaded: &mut Loaded,
    id: Id,
    emoji: &str,
    add: bool,
) -> Result<MessageRow, Error> {
    loaded.ensure_send()?;
    check_emoji(emoji)?;
    let chat = loaded.id();
    loaded.lock(conn).await?;
    let found = live(rows::message(conn, chat, id).await?)?;
    let me = act.user().to_string();
    let mut reactions = found.reactions.as_object().cloned().unwrap_or_default();
    let mut people: Vec<Json> = reactions
        .get(emoji)
        .and_then(Json::as_array)
        .cloned()
        .unwrap_or_default();
    let reacted = people.iter().any(|each| each.as_str() == Some(me.as_str()));
    if add == reacted {
        return Ok(found);
    }
    if add {
        if !reactions.contains_key(emoji)
            && reactions.len() >= usize::try_from(CHAT_MESSAGE_MAX_EMOJI).unwrap_or(usize::MAX)
        {
            return Err(Error::invalid(
                "emoji",
                format!("a message holds at most {CHAT_MESSAGE_MAX_EMOJI} different reactions"),
            ));
        }
        people.push(Json::String(me));
    } else {
        people.retain(|each| each.as_str() != Some(me.as_str()));
    }
    if people.is_empty() {
        reactions.remove(emoji);
    } else {
        reactions.insert(emoji.to_owned(), Json::Array(people));
    }
    let seq = chats::take(conn, chat).await?;
    let changed = rows::set_reactions(conn, id, &Json::Object(reactions), seq).await?;
    rows::notify(conn, chat, seq).await?;
    loaded.row.next_seq = seq + 1;
    Ok(changed)
}

/// Pins a message, or takes the pin off. Pinning a pinned message and
/// unpinning one that is not pinned change nothing.
pub async fn pin(
    conn: &mut Conn,
    act: &Acting<'_>,
    loaded: &mut Loaded,
    id: Id,
    pinned: bool,
) -> Result<MessageRow, Error> {
    loaded.ensure_send()?;
    let chat = loaded.id();
    loaded.lock(conn).await?;
    let found = live(rows::message(conn, chat, id).await?)?;
    if found.pinned_at.is_some() == pinned {
        return Ok(found);
    }
    let seq = chats::take(conn, chat).await?;
    let changed = rows::set_pin(conn, id, pinned.then_some(act.user()), act.now, seq).await?;
    rows::notify(conn, chat, seq).await?;
    loaded.row.next_seq = seq + 1;
    Ok(changed)
}

// ------------------------------------------------------------------ reading

/// Which messages are asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Window {
    /// The newest ones.
    Latest,
    /// Those written before a number.
    Before(Seq),
    /// Those written after a number.
    After(Seq),
    /// Those around a message.
    Around(Id),
    /// Those that are pinned.
    Pinned,
    /// Those that quote an item.
    Quoting(Id),
}

/// A stretch of a chat's messages, oldest first.
#[derive(Debug, Clone)]
pub struct Slice {
    /// The messages.
    pub messages: Vec<MessageRow>,
    /// Whether older ones exist.
    pub has_older: bool,
    /// Whether newer ones exist.
    pub has_newer: bool,
}

/// How many messages a read of a chat returns: what is asked for, at most
/// [`CHAT_MESSAGES_MAX_LIMIT`], and [`CHAT_MESSAGES_DEFAULT_LIMIT`] when nothing
/// is asked. None is not a number to ask for.
pub fn limit_of(asked: Option<u32>) -> Result<i64, Error> {
    match asked {
        None => Ok(i64::from(CHAT_MESSAGES_DEFAULT_LIMIT)),
        Some(0) => Err(Error::invalid("limit", "must be at least 1")),
        Some(limit) => Ok(i64::from(limit.min(CHAT_MESSAGES_MAX_LIMIT))),
    }
}

/// Reads a stretch of the chat's messages.
pub async fn window(
    conn: &mut Conn,
    chat: &ChatRow,
    window: Window,
    limit: i64,
) -> Result<Slice, Error> {
    let id = chat.id;
    match window {
        Window::Latest => {
            let messages = rows::latest(conn, id, limit).await?;
            let has_older = match messages.first() {
                Some(first) => rows::any_before(conn, id, first.seq_created).await?,
                None => false,
            };
            Ok(Slice {
                messages,
                has_older,
                has_newer: false,
            })
        }
        Window::Before(seq) => {
            let messages = rows::before(conn, id, seq, limit).await?;
            let has_older = match messages.first() {
                Some(first) => rows::any_before(conn, id, first.seq_created).await?,
                None => false,
            };
            Ok(Slice {
                messages,
                has_older,
                has_newer: rows::any_after(conn, id, seq.saturating_sub(1)).await?,
            })
        }
        Window::After(seq) => {
            let messages = rows::after(conn, id, seq, limit).await?;
            let has_newer = match messages.last() {
                Some(last) => rows::any_after(conn, id, last.seq_created).await?,
                None => false,
            };
            Ok(Slice {
                messages,
                has_older: rows::any_before(conn, id, seq.saturating_add(1)).await?,
                has_newer,
            })
        }
        Window::Around(message) => {
            let centre = rows::message(conn, id, message)
                .await?
                .ok_or(Error::NotFound)?;
            let half = (limit / 2).max(1);
            let mut messages = rows::before(conn, id, centre.seq_created, half).await?;
            let has_older = match messages.first() {
                Some(first) => rows::any_before(conn, id, first.seq_created).await?,
                None => rows::any_before(conn, id, centre.seq_created).await?,
            };
            let seq = centre.seq_created;
            messages.push(centre);
            let newer = rows::after(conn, id, seq, half).await?;
            let has_newer = match newer.last() {
                Some(last) => rows::any_after(conn, id, last.seq_created).await?,
                None => false,
            };
            messages.extend(newer);
            Ok(Slice {
                messages,
                has_older,
                has_newer,
            })
        }
        Window::Pinned => {
            let mut messages = rows::pinned(conn, id, limit + 1).await?;
            Ok(trimmed(&mut messages, limit))
        }
        Window::Quoting(item) => {
            let mut messages = rows::quoting(conn, id, item, limit + 1).await?;
            Ok(trimmed(&mut messages, limit))
        }
    }
}

/// A read that asked for one more than it shows, to learn whether older ones
/// exist: the oldest is dropped when it did come.
fn trimmed(messages: &mut Vec<MessageRow>, limit: i64) -> Slice {
    let over = i64::try_from(messages.len()).unwrap_or(i64::MAX) > limit;
    if over {
        messages.remove(0);
    }
    Slice {
        messages: std::mem::take(messages),
        has_older: over,
        has_newer: false,
    }
}

/// What a sync of a chat holds.
#[derive(Debug, Clone)]
pub struct Synced {
    /// The messages: the latest ones, or those that changed after a number.
    pub messages: Vec<MessageRow>,
    /// False when more changed than a sync holds.
    pub complete: bool,
    /// Whether older messages exist; on a first load only.
    pub has_older: bool,
}

/// Reads a chat for a client: its latest `limit` messages; or, from a
/// sequence, every message that changed after it, unless more than
/// [`CHAT_SYNC_MAX_CHANGES`] did, which the client answers by loading again.
pub async fn sync(
    conn: &mut Conn,
    chat: &ChatRow,
    since: Option<Seq>,
    limit: i64,
) -> Result<Synced, Error> {
    match since {
        None => {
            let slice = window(conn, chat, Window::Latest, limit).await?;
            Ok(Synced {
                messages: slice.messages,
                complete: true,
                has_older: slice.has_older,
            })
        }
        Some(since) => {
            if rows::count_changed(conn, chat.id, since).await? > CHAT_SYNC_MAX_CHANGES {
                return Ok(Synced {
                    messages: Vec::new(),
                    complete: false,
                    has_older: false,
                });
            }
            Ok(Synced {
                messages: rows::changed(conn, chat.id, since, None, CHAT_SYNC_MAX_CHANGES).await?,
                complete: true,
                has_older: false,
            })
        }
    }
}

/// The messages that `list` replies to, by identifier.
pub async fn replies_of(
    conn: &mut Conn,
    chat: Id,
    list: &[MessageRow],
) -> Result<HashMap<Id, MessageRow>, Error> {
    let mut ids: Vec<Id> = list.iter().filter_map(|row| row.reply_to_id).collect();
    ids.sort_unstable();
    ids.dedup();
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    Ok(rows::messages_by_ids(conn, chat, &ids)
        .await?
        .into_iter()
        .map(|row| (row.id, row))
        .collect())
}
