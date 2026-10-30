//! Copies of selected messages, or of selected items of a thread with the agent's work of a turn as one part, with their images and files, for chats and people; thread destinations become inputs.
//!
//! Forwarding is a deliberate copy by somebody who can read the original: the
//! people at the destination need no access to where it came from, exactly as
//! if the forwarder had retyped it. What arrives in a chat is one new message
//! by the forwarder: a text part with their comment when they gave one, then
//! one `forwarded` part for each message, in their original order. A copy holds
//! the text, the images and the files as they were at that moment, with the
//! author and time of the original, and does not change when the original is
//! edited or deleted. A message that itself holds copies carries them as they
//! are, so that the first author and source stay. Into the chat of the very
//! thread the items come from, the items arrive as `item_ref` parts, so that
//! they stay live quotes.
//!
//! One request writes to every destination in one transaction, each kind in
//! ascending order of identifier; if one refuses, nothing is written anywhere
//! and the error names that destination.
//!
//! **Routing.** The API sends agent-thread destinations to the thread engine as
//! the forwarder's input. This writer handles chats and direct messages. Live
//! item quotes go to their source thread's chat; other destinations receive
//! copies in the conversation's written form.

use crate::chats::{self, Loaded, Mode};
use crate::messages::{self, Post, Written};
use crate::redaction::{self, KeyProvider};
use crate::views::{self, Lookups};
use crate::{Acting, Error};
use orch_api_contract::chats::{
    CHAT_MESSAGE_MAX_PARTS, ChatPart, ChatRef, FORWARD_MAX_DESTINATIONS, FORWARD_MAX_ITEMS,
    FORWARD_MAX_MESSAGES,
};
use orch_api_contract::threads::{
    AgentName, Forwarded, ForwardedAttachment, ForwardedAuthor, ForwardedFrom, ForwardedImage,
};
use orch_db::Conn;
use orch_db::queries::chats::{self as rows, MessageRow};
use orch_types::{Id, Time};
use serde_json::json;

/// Where a forward goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Destination {
    /// A chat the caller can write in.
    Chat(Id),
    /// A person: the direct chat with them in the org the source belongs to,
    /// opened if it does not exist yet.
    User(Id),
    /// An agent's thread.
    Thread(Id),
}

impl Destination {
    /// The destination as the request gave it, for an error to name it.
    #[must_use]
    pub fn named(&self) -> serde_json::Value {
        match self {
            Self::Chat(chat) => json!({ "chat_id": chat }),
            Self::User(user) => json!({ "user_id": user }),
            Self::Thread(thread) => json!({ "thread_id": thread }),
        }
    }
}

/// The messages to forward, read: the chat they are in, and the messages.
#[derive(Debug, Clone)]
pub struct Prepared {
    /// The chat they come from, as the caller stands towards it.
    pub chat: Loaded,
    /// The messages, in their original order.
    pub messages: Vec<MessageRow>,
}

/// Reads the messages `ids` of the chat `chat` for forwarding: the caller can
/// read the chat, at most [`FORWARD_MAX_MESSAGES`] are chosen, every one is
/// there and was not deleted, and none is a line the system wrote.
pub async fn prepare(
    conn: &mut Conn,
    act: &Acting<'_>,
    chat: Id,
    ids: &[Id],
) -> Result<Prepared, Error> {
    if ids.is_empty() || ids.len() > usize::try_from(FORWARD_MAX_MESSAGES).unwrap_or(usize::MAX) {
        return Err(Error::invalid(
            "from.message_ids",
            format!("name 1 to {FORWARD_MAX_MESSAGES} messages"),
        ));
    }
    let loaded = chats::load(conn, act, chat, Mode::Read).await?;
    let messages = rows::messages_by_ids(conn, chat, ids).await?;
    for id in ids {
        match messages.iter().find(|row| row.id == *id) {
            None => {
                return Err(Error::invalid(
                    "from.message_ids",
                    format!("{id} is not a message of that chat"),
                ));
            }
            Some(row) if row.is_deleted() || row.kind != rows::MESSAGE => {
                return Err(Error::invalid(
                    "from.message_ids",
                    format!("{id} says nothing to forward"),
                ));
            }
            Some(_) => {}
        }
    }
    Ok(Prepared {
        chat: loaded,
        messages,
    })
}

/// The people the copies of these messages name, so that their names can be
/// looked up.
#[must_use]
pub fn people_of(prepared: &Prepared) -> Vec<Id> {
    let rows: Vec<&MessageRow> = prepared.messages.iter().collect();
    views::people_of(&rows)
}

fn media_type_text(media_type: &orch_api_contract::threads::ImageMediaType) -> String {
    serde_json::to_value(media_type)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// The parts that copy these messages: for each, one `forwarded` part holding
/// what it said and carried, and then the copies it held, as they were.
#[must_use]
pub fn copies_of(prepared: &Prepared, chat: &ChatRef, lookups: &Lookups) -> Vec<ChatPart> {
    let mut out: Vec<ChatPart> = Vec::new();
    for row in &prepared.messages {
        let parts = views::project_parts(views::parts_of(&row.content), lookups);
        let mut text: Vec<String> = Vec::new();
        let mut images: Vec<ForwardedImage> = Vec::new();
        let mut attachments: Vec<ForwardedAttachment> = Vec::new();
        let mut carried: Vec<ChatPart> = Vec::new();
        let author = match (row.author_id, &row.author_agent) {
            (Some(id), _) => lookups.user(id).map(ForwardedAuthor::User),
            (None, Some(agent)) => Some(ForwardedAuthor::Agent(AgentName {
                agent: agent.clone(),
            })),
            (None, None) => None,
        };
        let mut pasted = false;
        for part in parts {
            match part {
                ChatPart::PastedText(mut paste) => {
                    if paste.source.is_none() {
                        pasted = true;
                        paste.source = Some(Box::new(Forwarded {
                            r#type: orch_api_contract::threads::ForwardedType::Forwarded,
                            from: ForwardedFrom::Chat { chat: chat.clone() },
                            source_id: row.id,
                            author: author.clone(),
                            at: Time::new(row.created_at),
                            text: String::new(),
                            images: Vec::new(),
                            attachments: Vec::new(),
                        }));
                    }
                    carried.push(ChatPart::PastedText(paste));
                }
                ChatPart::Mention { target } => text.push(views::mention_text(&target, lookups)),
                ChatPart::Text { text: said } => text.push(said),
                ChatPart::Image {
                    blob,
                    media_type,
                    name,
                } => images.push(ForwardedImage {
                    blob,
                    media_type: media_type_text(&media_type),
                    name,
                }),
                ChatPart::Attachment {
                    blob,
                    name,
                    size,
                    media_type,
                } => attachments.push(ForwardedAttachment {
                    blob,
                    name,
                    size,
                    media_type,
                }),
                copy @ ChatPart::Forwarded(_) => carried.push(copy),
                // A live quote is valid only where it was made.
                ChatPart::ItemRef { .. } => {}
            }
        }
        let text = text.join("\n\n");
        if !text.trim().is_empty() || !images.is_empty() || !attachments.is_empty() || pasted {
            out.push(ChatPart::Forwarded(Box::new(Forwarded {
                r#type: orch_api_contract::threads::ForwardedType::Forwarded,
                from: ForwardedFrom::Chat { chat: chat.clone() },
                source_id: row.id,
                author,
                at: Time::new(row.created_at),
                text,
                images,
                attachments,
            })));
        }
        out.extend(carried);
    }
    out
}

/// What a forward writes to each destination.
#[derive(Debug, Clone)]
pub struct Delivery<'a> {
    /// The root org the source belongs to: where a person as a destination is
    /// reached by a direct chat.
    pub root: Id,
    /// The parts that arrive after the comment: copies, or quotes of items.
    pub content: Vec<ChatPart>,
    /// The only chat the content may go to: the chat of the thread whose items
    /// are quoted.
    pub only: Option<Id>,
    /// Where it goes.
    pub to: &'a [Destination],
    /// What the forwarder says first.
    pub comment: Option<&'a str>,
    /// What makes forwarding again harmless.
    pub client_id: &'a str,
}

/// What was written in a chat by a forward.
#[derive(Debug, Clone)]
pub struct Sent {
    /// The chat.
    pub chat: Id,
    /// The message, by the caller.
    pub written: Written,
}

/// Writes the message of a forward to each destination, in one transaction:
/// the caller's, which the caller does not commit if this refuses.
pub async fn deliver(
    conn: &mut Conn,
    act: &Acting<'_>,
    delivery: &Delivery<'_>,
    keys: KeyProvider<'_>,
) -> Result<Vec<Sent>, Error> {
    if delivery.to.is_empty()
        || delivery.to.len() > usize::try_from(FORWARD_MAX_DESTINATIONS).unwrap_or(usize::MAX)
    {
        return Err(Error::invalid(
            "to",
            format!("name 1 to {FORWARD_MAX_DESTINATIONS} destinations"),
        ));
    }
    let mut destinations: Vec<(Id, Destination)> = Vec::new();
    for to in delivery.to {
        let chat = match to {
            Destination::Chat(chat) => *chat,
            Destination::User(user) => {
                chats::open(conn, act, delivery.root, &[*user], None)
                    .await
                    .map_err(|error| at(to, error))?
                    .chat
                    .id
            }
            Destination::Thread(_) => {
                return Err(at(
                    to,
                    Error::Unsupported(
                        "an agent thread receives forwarded messages as conversation input",
                    ),
                ));
            }
        };
        if !destinations.iter().any(|(each, _)| *each == chat) {
            destinations.push((chat, *to));
        }
    }
    // Ascending order of identifier, so that two forwards never wait for each
    // other's chats in a different order.
    let mut order = destinations.clone();
    order.sort_by_key(|(chat, _)| *chat);
    let mut written: Vec<Sent> = Vec::new();
    for (chat, to) in order {
        if delivery.only.is_some_and(|only| only != chat) {
            return Err(at(
                &to,
                Error::Unsupported(
                    "live item quotes can only be sent to their source thread's chat",
                ),
            ));
        }
        let mut loaded = chats::load(conn, act, chat, Mode::Read)
            .await
            .map_err(|error| at(&to, error))?;
        let redactor = redaction::for_chat(conn, &loaded.row, keys)
            .await
            .map_err(|error| at(&to, error))?;
        let mut parts: Vec<ChatPart> = Vec::new();
        if let Some(comment) = delivery
            .comment
            .filter(|comment| !comment.trim().is_empty())
        {
            parts.push(ChatPart::Text {
                text: comment.to_owned(),
            });
        }
        parts.extend(delivery.content.iter().cloned());
        if parts.len() > usize::try_from(CHAT_MESSAGE_MAX_PARTS).unwrap_or(usize::MAX) {
            return Err(at(
                &to,
                Error::invalid(
                    "from",
                    format!("come to more than the {CHAT_MESSAGE_MAX_PARTS} parts of a message"),
                ),
            ));
        }
        let sent = messages::post_copies(
            conn,
            act,
            &mut loaded,
            Post {
                parts,
                reply_to: None,
                client_id: delivery.client_id.to_owned(),
            },
            &redactor,
        )
        .await
        .map_err(|error| at(&to, error))?;
        written.push(Sent {
            chat,
            written: sent,
        });
    }
    // In the order the request named them.
    let mut ordered: Vec<Sent> = Vec::new();
    for (chat, _) in destinations {
        if let Some(position) = written.iter().position(|sent| sent.chat == chat) {
            ordered.push(written.remove(position));
        }
    }
    Ok(ordered)
}

fn at(to: &Destination, error: Error) -> Error {
    match error {
        already @ Error::Destination { .. } => already,
        other => Error::Destination {
            to: to.named(),
            error: Box::new(other),
        },
    }
}

/// The items to forward must be at most [`FORWARD_MAX_ITEMS`].
pub fn check_items(items: &[Id]) -> Result<(), Error> {
    if items.is_empty() || items.len() > usize::try_from(FORWARD_MAX_ITEMS).unwrap_or(usize::MAX) {
        return Err(Error::invalid(
            "from.item_ids",
            format!("name 1 to {FORWARD_MAX_ITEMS} items"),
        ));
    }
    Ok(())
}

/// The parts that quote items of a thread into the chat of that thread:
/// quotes stay live, which a copy would not.
pub async fn quote_items(
    conn: &mut Conn,
    thread: Id,
    items: &[Id],
) -> Result<Vec<ChatPart>, Error> {
    check_items(items)?;
    let found = rows::items_with_kinds(conn, thread, items).await?;
    for item in items {
        match found.iter().find(|(id, _)| id == item) {
            None => {
                return Err(Error::invalid(
                    "from.item_ids",
                    format!("{item} is not an item of that thread"),
                ));
            }
            Some((_, kind)) if kind == "approval" || kind == "marker" => {
                return Err(Error::invalid(
                    "from.item_ids",
                    format!("{item} is an approval or a line of history, which is not forwarded"),
                ));
            }
            Some(_) => {}
        }
    }
    Ok(items
        .iter()
        .map(|item| ChatPart::ItemRef {
            thread_id: thread,
            item_id: *item,
        })
        .collect())
}
