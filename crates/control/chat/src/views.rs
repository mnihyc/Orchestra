//! Rows as the types of the API: a chat as one person reads it, and a message as it is shown.
//!
//! A row names people and nodes by identifier. What a reader may know of each
//! is looked up by whoever serves them and handed in as [`Lookups`], so that
//! the shapes made here depend on nothing but the rows and those names. A
//! message that was deleted shows nothing of what it said, and nothing of what
//! was said to it either: the excerpt a reply carries of it is empty.

use orch_api_contract::chats::{
    CHAT_REPLY_EXCERPT_CHARS, Chat, ChatCan, ChatCanReason, ChatKind, ChatMessage, ChatMessageKind,
    ChatMessageReply, ChatMessageSystem, ChatPart, ChatPin, ChatPreview, ChatRef, ChatState,
    Reaction,
};
use orch_api_contract::documents::{Author, AuthorAgent};
use orch_api_contract::nodes::{NodeKind, NodeRef};
use orch_api_contract::users::UserRef;
use orch_authz::chats::{Cannot, Standing};
use orch_db::queries::chats::{self as rows, ChatRow, MemberRow, MessageRow};
use orch_types::{Id, Json, Time};
use std::collections::HashMap;

/// What the reader may know of the people and the nodes the rows name.
#[derive(Debug, Clone, Default)]
pub struct Lookups {
    /// People by identifier, as others see them.
    pub people: HashMap<Id, UserRef>,
    /// Nodes by identifier, each by name where the reader may know it.
    pub nodes: HashMap<Id, NodeRef>,
    /// Current readable group names, including their visible place.
    pub groups: HashMap<Id, String>,
}

/// A deliberate mention as current human text, with an unseen target kept unnamed.
#[must_use]
pub fn mention_text(
    target: &orch_api_contract::chats::ChatMentionTarget,
    lookups: &Lookups,
) -> String {
    use orch_api_contract::access::Role;
    use orch_api_contract::chats::ChatMentionTarget as Target;
    match target {
        Target::User { id } => lookups.user(*id).map_or_else(
            || "@a person".to_owned(),
            |user| format!("@{}", user.username),
        ),
        Target::Group { id } => lookups
            .groups
            .get(id)
            .map_or_else(|| "@a group".to_owned(), |name| format!("@{name}")),
        Target::Agent { thread_id } => lookups
            .nodes
            .get(thread_id)
            .and_then(|node| node.name.as_deref())
            .map_or_else(|| "@an agent".to_owned(), |name| format!("@{name}")),
        Target::Role { node_id, role } => {
            let role = match role {
                Role::Viewer => "Viewers",
                Role::Interactor => "Interactors",
                Role::Developer => "Developers",
                Role::Admin => "Admins",
            };
            let place = lookups
                .nodes
                .get(node_id)
                .and_then(|node| node.name.as_deref())
                .unwrap_or("a private place");
            format!("@{role} of {place}")
        }
    }
}

/// The safe current display labels of stable targets, including a private fallback after access loss.
#[must_use]
pub fn mention_names(
    targets: &[orch_api_contract::chats::ChatMentionTarget],
    lookups: &Lookups,
) -> Vec<orch_api_contract::chats::ChatMentionName> {
    targets
        .iter()
        .map(|target| orch_api_contract::chats::ChatMentionName {
            target: target.clone(),
            name: mention_text(target, lookups)
                .trim_start_matches('@')
                .to_owned(),
        })
        .collect()
}

pub(crate) fn project_parts(parts: Vec<ChatPart>, lookups: &Lookups) -> Vec<ChatPart> {
    let mut shown = Vec::new();
    let mut join_next = false;
    for part in parts {
        let mention = matches!(&part, ChatPart::Mention { .. });
        let part = match part {
            ChatPart::Mention { target } => ChatPart::Text {
                text: mention_text(&target, lookups),
            },
            part => part,
        };
        if (mention || join_next)
            && let ChatPart::Text { text } = &part
            && let Some(ChatPart::Text { text: previous }) = shown.last_mut()
        {
            previous.push_str(text);
            join_next = mention;
            continue;
        }
        join_next = mention;
        shown.push(part);
    }
    shown
}

/// Groups deliberately named in the source rows, once each.
#[must_use]
pub fn groups_of(messages: &[&MessageRow]) -> Vec<Id> {
    let mut ids: Vec<_> = messages
        .iter()
        .flat_map(|row| parts_of(&row.content))
        .filter_map(|part| match part {
            ChatPart::Mention {
                target: orch_api_contract::chats::ChatMentionTarget::Group { id },
            } => Some(id),
            _ => None,
        })
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

/// Places whose role audiences are deliberately named in the source rows, once each.
#[must_use]
pub fn role_places_of(messages: &[&MessageRow]) -> Vec<Id> {
    let mut ids: Vec<_> = messages
        .iter()
        .flat_map(|row| parts_of(&row.content))
        .filter_map(|part| match part {
            ChatPart::Mention {
                target: orch_api_contract::chats::ChatMentionTarget::Role { node_id, .. },
            } => Some(node_id),
            _ => None,
        })
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

impl Lookups {
    /// The person, if they are known.
    #[must_use]
    pub fn user(&self, id: Id) -> Option<UserRef> {
        self.people.get(&id).cloned()
    }

    /// The node `id`, or a reference to one the reader may not know, of the
    /// kind given.
    #[must_use]
    pub fn node(&self, id: Id, kind: NodeKind) -> NodeRef {
        self.nodes.get(&id).cloned().unwrap_or(NodeRef {
            thread_display: None,
            id,
            kind,
            name: None,
            redacted: true,
        })
    }
}

/// What a system message holds, as it is stored in its content.
fn stored_event(content: &Json) -> Option<(ChatMessageSystemParts, Option<String>)> {
    let event = serde_json::from_value(content.get("event")?.clone()).ok()?;
    let id = |member: &str| -> Option<Id> { content.get(member)?.as_str()?.parse().ok() };
    Some((
        ChatMessageSystemParts {
            event,
            by: id("by"),
            user: id("user"),
        },
        content
            .get("name")
            .and_then(Json::as_str)
            .map(str::to_owned),
    ))
}

struct ChatMessageSystemParts {
    event: orch_api_contract::chats::ChatSystemEvent,
    by: Option<Id>,
    user: Option<Id>,
}

/// The identifiers of the people a group of messages names, so that their
/// names can be looked up once: authors, the people mentioned, who pinned,
/// the people a system line is about, and the authors of the messages replied
/// to.
#[must_use]
pub fn people_of(messages: &[&MessageRow]) -> Vec<Id> {
    let mut found: Vec<Id> = Vec::new();
    for row in messages {
        found.extend(row.author_id);
        found.extend(row.mentions.iter().copied());
        found.extend(row.pinned_by);
        if row.kind == rows::SYSTEM
            && let Some((parts, _)) = stored_event(&row.content)
        {
            found.extend(parts.by);
            found.extend(parts.user);
        }
    }
    found.sort_unstable();
    found.dedup();
    found
}

/// The identifiers of the threads whose agents wrote messages.
#[must_use]
pub fn threads_of(messages: &[&MessageRow]) -> Vec<Id> {
    let mut found: Vec<Id> = messages
        .iter()
        .filter_map(|row| row.author_thread_id)
        .collect();
    for row in messages {
        found.extend(
            parts_of(&row.content)
                .into_iter()
                .filter_map(|part| match part {
                    ChatPart::Mention {
                        target: orch_api_contract::chats::ChatMentionTarget::Agent { thread_id },
                    } => Some(thread_id),
                    _ => None,
                }),
        );
        found.extend(asked_of(row).into_iter().map(|(thread, _)| thread));
    }
    found.sort_unstable();
    found.dedup();
    found
}

fn asked_of(row: &MessageRow) -> Vec<(Id, Id)> {
    row.asked_agents
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|asked| {
            Some((
                asked.get("thread_id")?.as_str()?.parse().ok()?,
                asked.get("input_id")?.as_str()?.parse().ok()?,
            ))
        })
        .collect()
}

/// The parts a stored message holds. Content that does not read as parts, which
/// only a fault of an earlier version could leave, shows as no parts.
#[must_use]
pub fn parts_of(content: &Json) -> Vec<ChatPart> {
    serde_json::from_value(content.clone()).unwrap_or_default()
}

/// The first characters of what a message says, for a reply to show: its text,
/// else the names of what it carries. Nothing for a message that was deleted.
#[must_use]
pub fn excerpt_of(row: &MessageRow) -> String {
    if row.is_deleted() || row.kind == rows::SYSTEM {
        return String::new();
    }
    let parts = parts_of(&row.content);
    let mut said: Vec<String> = Vec::new();
    for part in &parts {
        match part {
            ChatPart::Text { text } if !text.trim().is_empty() => said.push(text.trim().to_owned()),
            ChatPart::Forwarded(copy) if !copy.text.trim().is_empty() => {
                said.push(copy.text.trim().to_owned());
            }
            _ => {}
        }
    }
    if said.is_empty() {
        for part in &parts {
            match part {
                ChatPart::Image { name, .. } => said.push(format!("[image: {name}]")),
                ChatPart::Attachment { name, .. } => said.push(format!("[file: {name}]")),
                ChatPart::PastedText(paste) => said.push(format!("[file: {}]", paste.name)),
                _ => {}
            }
        }
    }
    said.join(" ")
        .chars()
        .take(usize::try_from(CHAT_REPLY_EXCERPT_CHARS).unwrap_or(usize::MAX))
        .collect()
}

fn reactions_of(row: &MessageRow) -> Vec<Reaction> {
    if row.is_deleted() {
        return Vec::new();
    }
    row.reactions
        .as_object()
        .into_iter()
        .flatten()
        .map(|(emoji, users)| Reaction {
            emoji: emoji.clone(),
            users: users
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|user| user.as_str()?.parse().ok())
                .collect(),
        })
        .filter(|reaction| !reaction.users.is_empty())
        .collect()
}

/// A message as it is shown. `reply` is the message it replies to, if it does.
#[must_use]
pub fn message(row: &MessageRow, reply: Option<&MessageRow>, lookups: &Lookups) -> ChatMessage {
    let system = (row.kind == rows::SYSTEM).then(|| {
        stored_event(&row.content).map(|(parts, name)| ChatMessageSystem {
            event: parts.event,
            by: parts.by.and_then(|id| lookups.user(id)),
            user: parts.user.and_then(|id| lookups.user(id)),
            name,
        })
    });
    let author = match (row.author_id, &row.author_agent) {
        (Some(id), _) => lookups.user(id).map(Author::User),
        (None, Some(agent)) => Some(Author::Agent(AuthorAgent {
            agent: agent.clone(),
            thread: row.author_thread_id.map_or_else(
                || NodeRef {
                    thread_display: None,
                    id: Id::nil(),
                    kind: NodeKind::Thread,
                    name: None,
                    redacted: true,
                },
                |thread| lookups.node(thread, NodeKind::Thread),
            ),
        })),
        (None, None) => None,
    };
    let deleted = row.is_deleted();
    ChatMessage {
        mention_names: if deleted {
            Vec::new()
        } else {
            mention_names(
                &serde_json::from_value::<Vec<_>>(row.mention_targets.clone()).unwrap_or_default(),
                lookups,
            )
        },
        id: row.id,
        chat_id: row.chat_id,
        kind: if row.kind == rows::SYSTEM {
            ChatMessageKind::System
        } else {
            ChatMessageKind::Message
        },
        author,
        parts: if deleted || row.kind == rows::SYSTEM {
            Vec::new()
        } else {
            project_parts(parts_of(&row.content), lookups)
        },
        system: system.flatten(),
        reply_to: reply.map(|replied| ChatMessageReply {
            id: replied.id,
            author: message(replied, None, lookups).author,
            excerpt: excerpt_of(replied),
            deleted: replied.is_deleted(),
        }),
        mentions: if deleted {
            Vec::new()
        } else {
            row.mentions
                .iter()
                .filter_map(|id| lookups.user(*id))
                .collect()
        },
        mentions_all: row.mentions_all && !deleted,
        mention_targets: if deleted {
            Vec::new()
        } else {
            serde_json::from_value(row.mention_targets.clone()).unwrap_or_default()
        },
        asked_agents: if deleted {
            Vec::new()
        } else {
            asked_of(row)
                .into_iter()
                .map(
                    |(thread, input_id)| orch_api_contract::chats::ChatAskedAgent {
                        thread: lookups.node(thread, NodeKind::Thread),
                        input_id,
                    },
                )
                .collect()
        },
        reactions: reactions_of(row),
        pinned: match (row.pinned_at, row.pinned_by.and_then(|id| lookups.user(id))) {
            (Some(at), Some(by)) if !deleted => Some(ChatPin {
                at: Time::new(at),
                by,
            }),
            _ => None,
        },
        edited_at: row.edited_at.map(Time::new),
        deleted,
        seq_created: row.seq_created,
        seq_updated: row.seq_updated,
        created_at: Time::new(row.created_at),
    }
}

/// The messages as they are shown, each with the message it replies to taken
/// from `replies`.
#[must_use]
pub fn messages(
    list: &[MessageRow],
    replies: &HashMap<Id, MessageRow>,
    lookups: &Lookups,
) -> Vec<ChatMessage> {
    list.iter()
        .map(|row| {
            message(
                row,
                row.reply_to_id.and_then(|id| replies.get(&id)),
                lookups,
            )
        })
        .collect()
}

/// What the caller may do in a chat, as the API says it.
#[must_use]
pub fn can_of(standing: &Standing, lookups: &Lookups) -> ChatCan {
    let reason = match standing.sending {
        Ok(()) => None,
        Err(Cannot::Archived { .. }) => Some(ChatCanReason::Archived),
        Err(Cannot::Disabled | Cannot::DirectOff) => Some(ChatCanReason::Disabled),
        Err(Cannot::Closed) => Some(ChatCanReason::Closed),
        Err(Cannot::NoPermission) => Some(ChatCanReason::NoPermission),
        Err(Cannot::Inspecting) => Some(ChatCanReason::Inspecting),
    };
    ChatCan {
        admin_place: (!standing.admin_only.is_empty())
            .then_some(standing.admin_place)
            .flatten()
            .map(|place| match place {
                orch_access_contract::roles::PlaceRef::System => {
                    orch_api_contract::nodes::NodeOrSystem::SYSTEM
                }
                orch_access_contract::roles::PlaceRef::Node(id) => {
                    orch_api_contract::nodes::NodeOrSystem::Node(lookups.node(id, NodeKind::Org))
                }
            }),
        admin_only: standing
            .admin_only
            .iter()
            .map(|permission| permission.as_str().to_owned())
            .collect(),
        admin_access: standing
            .admin_access
            .map(|reach| orch_api_contract::access::AdminAccess {
                private: reach.private,
                place: match reach.place {
                    orch_access_contract::roles::PlaceRef::System => {
                        orch_api_contract::nodes::NodeOrSystem::SYSTEM
                    }
                    orch_access_contract::roles::PlaceRef::Node(id) => {
                        orch_api_contract::nodes::NodeOrSystem::Node(
                            lookups.node(id, NodeKind::Org),
                        )
                    }
                },
            }),
        participating: standing.participating,
        send: standing.sending.is_ok(),
        moderate: standing.moderate,
        export: standing.export,
        reason,
    }
}

/// The kind of a chat as the API names it.
#[must_use]
pub fn kind_of(chat: &ChatRow) -> ChatKind {
    match chat.kind.as_str() {
        rows::KIND_DIRECT => ChatKind::Direct,
        rows::KIND_GROUP => ChatKind::Group,
        _ => ChatKind::Node,
    }
}

/// The people a chat view names: who made it and who is in it.
#[must_use]
pub fn people_in(chat: &ChatRow, members: &[MemberRow]) -> Vec<Id> {
    let mut found: Vec<Id> = chat.created_by.into_iter().collect();
    found.extend(
        members
            .iter()
            .filter(|row| row.is_in())
            .map(|row| row.user_id),
    );
    found.sort_unstable();
    found.dedup();
    found
}

/// What a chat view is made of.
#[derive(Debug, Clone, Copy)]
pub struct Seen<'a> {
    /// The chat.
    pub chat: &'a ChatRow,
    /// Its people, those who left among them.
    pub members: &'a [MemberRow],
    /// What the reader may do in it.
    pub standing: &'a Standing,
    /// The reader's own state in it; none when they have none.
    pub my: Option<&'a ChatState>,
}

/// A chat as the reader sees it.
#[must_use]
pub fn chat(seen: &Seen<'_>, lookups: &Lookups) -> Chat {
    let row = seen.chat;
    let people = !matches!(kind_of(row), ChatKind::Node);
    let present: Vec<&MemberRow> = seen.members.iter().filter(|each| each.is_in()).collect();
    Chat {
        inspection_holders: None,
        agents: None,
        base: ChatRef {
            id: row.id,
            kind: kind_of(row),
            org_id: row.org_id,
            node: row.node_id.map(|node| lookups.node(node, NodeKind::Org)),
            name: row.name.clone(),
        },
        members: people.then(|| {
            present
                .iter()
                .filter_map(|each| lookups.user(each.user_id))
                .collect()
        }),
        membership_expiries: present
            .iter()
            .filter_map(|member| {
                member
                    .expires_at
                    .map(|end| (member.user_id, Time::new(end)))
            })
            .collect(),
        guests: if people {
            present
                .iter()
                .filter(|each| each.guest)
                .map(|each| each.user_id)
                .collect()
        } else {
            Vec::new()
        },
        created_by: row.created_by.and_then(|id| lookups.user(id)),
        created_at: Time::new(row.created_at),
        seq: row.seq(),
        last_message_seq: row.last_message_seq,
        last_message_at: row.last_message_at.map(Time::new),
        preview: None,
        can: can_of(seen.standing, lookups),
        my: seen.my.cloned(),
    }
}

/// The same author and deletion rules as a message, without exposing the body
/// or dereferencing an item quote into a thread the reader may not read.
#[must_use]
pub fn preview(row: &MessageRow, lookups: &Lookups) -> ChatPreview {
    ChatPreview {
        id: row.id,
        author: message(row, None, lookups).author,
        text: excerpt_of(row),
        deleted: row.is_deleted(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use orch_types::new_id;
    use serde_json::json;

    fn row(content: Json) -> MessageRow {
        MessageRow {
            mention_targets: serde_json::json!([]),
            asked_agents: serde_json::json!([]),
            id: new_id(),
            chat_id: new_id(),
            kind: rows::MESSAGE.to_owned(),
            author_id: Some(new_id()),
            author_thread_id: None,
            author_agent: None,
            content,
            reply_to_id: None,
            mentions: Vec::new(),
            mentions_all: false,
            reactions: json!({}),
            client_id: None,
            search_text: None,
            edited_at: None,
            pinned_at: None,
            pinned_by: None,
            deleted_at: None,
            deleted_by: None,
            seq_created: 1,
            seq_updated: 1,
            created_at: Utc
                .with_ymd_and_hms(2026, 10, 8, 14, 0, 0)
                .single()
                .unwrap(),
        }
    }

    #[test]
    fn a_reply_shows_the_first_hundred_and_twenty_characters_of_what_it_answers() {
        let long = "x".repeat(300);
        let replied = row(json!([{ "type": "text", "text": long }]));
        assert_eq!(excerpt_of(&replied).chars().count(), 120);
        let picture = row(
            json!([{ "type": "image", "blob": "a".repeat(64), "media_type": "image/png", "name": "chart.png" }]),
        );
        assert_eq!(excerpt_of(&picture), "[image: chart.png]");
    }

    #[test]
    fn a_deleted_message_shows_nothing_of_what_it_said_and_nothing_replying_to_it_does_either() {
        let mut gone = row(json!([]));
        gone.deleted_at = Some(gone.created_at);
        gone.reactions = json!({ "x": [new_id().to_string()] });
        gone.mentions = vec![new_id()];
        gone.mentions_all = true;
        let shown = message(&gone, None, &Lookups::default());
        assert!(shown.deleted);
        assert!(shown.parts.is_empty() && shown.reactions.is_empty() && shown.mentions.is_empty());
        assert!(!shown.mentions_all);
        // What was written before it was emptied is not what the reply shows.
        let mut kept = row(json!([{ "type": "text", "text": "a secret I regret" }]));
        kept.deleted_at = Some(kept.created_at);
        let answer = message(&row(json!([])), Some(&kept), &Lookups::default());
        let reply = answer.reply_to.unwrap();
        assert!(reply.deleted);
        assert_eq!(reply.excerpt, "");
    }

    #[test]
    fn a_system_message_shows_its_event_and_no_parts() {
        let (by, user) = (new_id(), new_id());
        let mut line =
            row(json!({ "event": "member_added", "by": by, "user": user, "name": null }));
        line.kind = rows::SYSTEM.to_owned();
        line.author_id = None;
        let shown = message(&line, None, &Lookups::default());
        assert_eq!(shown.kind, ChatMessageKind::System);
        assert!(shown.parts.is_empty() && shown.author.is_none());
        let event = shown.system.unwrap();
        assert_eq!(
            event.event,
            orch_api_contract::chats::ChatSystemEvent::MemberAdded
        );
        assert_eq!(people_of(&[&line]), {
            let mut both = vec![by, user];
            both.sort_unstable();
            both
        });
    }
}
