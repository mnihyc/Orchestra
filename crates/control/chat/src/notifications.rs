//! Mention and per-chat notifications written with the message.
//!
//! In the transaction that writes a message, the people it concerns are told.
//! A `mention` goes to each person who is named and can read the chat, unless
//! they muted it; a `chat.message` goes to each person who follows the chat at
//! level `all`, as one row for the chat that each new message renews until the
//! chat is read; both start the outbox rows of the person's routes. Nobody is
//! told of what they did themself. The people in a direct or a group chat are
//! told on their screens that the chat has something new, and a person who was
//! named, or whose chat came back to their list, is told how their state in it
//! stands now.

use crate::chats::{self, Loaded};
use crate::following::{self, Defaults};
use crate::mentions::Named;
use crate::views;
use crate::{Acting, Error};
use orch_api_contract::chats::ChatLevel;
use orch_db::queries::chats::{self as rows, MessageRow, Notice};
use orch_db::queries::identity;
use orch_db::queries::notifications::outbox::Snapshot;
use orch_db::queries::notifications::routes;
use orch_db::{Conn, sequences};
use orch_realtime_contract::feeds::UserEvent;
use orch_settings::Place;
use orch_types::{Id, Time};
use serde_json::json;
use std::collections::HashMap;

/// Tells people, on every connection they hold on every API process, news
/// that is no row of a feed. It is delivered when the transaction commits.
pub async fn tell_users(conn: &mut Conn, users: &[Id], event: &UserEvent) -> Result<(), Error> {
    if users.is_empty() {
        return Ok(());
    }
    let news = serde_json::to_value(event).map_err(|error| Error::Internal(error.to_string()))?;
    sequences::notify_user_event(conn, users, &news).await?;
    Ok(())
}

/// Who a message told, for whoever wants to know.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Told {
    /// The people it named, who can read the chat, `@all` included.
    pub named: Vec<Id>,
    /// The people who got a `mention` notification.
    pub mentioned: Vec<Id>,
    /// The people whose `chat.message` notification it renewed.
    pub renewed: Vec<Id>,
}

/// The title of the chat as a notification names it.
async fn title_of(conn: &mut Conn, loaded: &Loaded, author: &str) -> Result<String, Error> {
    Ok(match (loaded.row.node_id, loaded.row.name.as_deref()) {
        (Some(node), _) => rows::node_name(conn, node).await?.unwrap_or_default(),
        (None, Some(name)) => name.to_owned(),
        (None, None) if loaded.row.kind == rows::KIND_DIRECT => author.to_owned(),
        (None, None) => "a group chat".to_owned(),
    })
}

/// What a message told, once written: engagement, unread mentions, the rows of
/// the Inbox, the outbox rows of routes that tell at once, and the news for
/// the screens of the people concerned. `by_agent` is a message an agent wrote
/// where the chat was shared with it: it makes nobody engaged.
pub async fn after_post(
    conn: &mut Conn,
    act: &Acting<'_>,
    loaded: &Loaded,
    message: &MessageRow,
    named: &Named,
    by_agent: bool,
) -> Result<Told, Error> {
    let chat = &loaded.row;
    let author = message.author_id;
    let people_chat = chat.is_people_chat();
    if let Some(author) = author
        && !by_agent
    {
        rows::engage(conn, chat.id, &[author], act.now).await?;
    }
    // Everyone who follows the chat now, among them those the message names
    // with @all.
    // Every deliberate audience, including @all, was frozen before the message was stored.
    let named_users = named.users.clone();
    if !named_users.is_empty() {
        rows::raise_mentions(conn, chat.id, &named_users, act.now).await?;
    }
    let brought_back = rows::unhide_all(conn, chat.id).await?;

    // Levels after the message engaged and named people.
    let followers = following::followers(conn, act, loaded).await?;
    let level_of: HashMap<Id, ChatLevel> = followers
        .iter()
        .filter_map(|follower| {
            following::parse_level(&follower.level).map(|level| (follower.user_id, level))
        })
        .collect();
    let wants_all: Vec<Id> = level_of
        .iter()
        .filter(|(user, level)| {
            **level == ChatLevel::All && Some(**user) != author && !named_users.contains(user)
        })
        .map(|(user, _)| *user)
        .collect();
    let renewed = orch_authz::chats::readers_among(
        conn,
        chat,
        &loaded.members,
        &wants_all,
        Time::new(act.now),
    )
    .await?;
    let mentioned: Vec<Id> = named_users
        .iter()
        .copied()
        .filter(|user| level_of.contains_key(user))
        .collect();

    if !mentioned.is_empty() || !renewed.is_empty() {
        write_notices(conn, act, loaded, message, &mentioned, &renewed).await?;
    }

    // The screens.
    if people_chat {
        let present = loaded.present();
        chats::tell_members(conn, chat.id, message.seq_created, act.now, &present).await?;
    }
    let mut changed: Vec<Id> = named_users.clone();
    changed.extend(brought_back);
    changed.sort_unstable();
    changed.dedup();
    for user in changed {
        let in_chat = loaded.has(user);
        if let Some(state) = following::my_state(conn, user, chat, in_chat, act.now).await? {
            tell_users(conn, &[user], &UserEvent::ChatState { state }).await?;
        }
    }
    Ok(Told {
        named: named_users,
        mentioned,
        renewed,
    })
}

async fn write_notices(
    conn: &mut Conn,
    act: &Acting<'_>,
    loaded: &Loaded,
    message: &MessageRow,
    mentioned: &[Id],
    renewed: &[Id],
) -> Result<(), Error> {
    let chat = &loaded.row;
    let (name, username) = match message.author_id {
        Some(author) => identity::users_brief(conn, &[author])
            .await?
            .into_iter()
            .next()
            .map_or_else(
                || (String::new(), String::new()),
                |brief| (brief.display_name, brief.username),
            ),
        None => (
            message.author_agent.clone().unwrap_or_default(),
            String::new(),
        ),
    };
    let title = title_of(conn, loaded, &name).await?;
    let excerpt = views::excerpt_of(message);
    let path = if chat.is_people_chat() {
        rows::org_slug(conn, chat.org_id).await?.map(|slug| {
            format!(
                "/org/{slug}/messages?chat={}&message={}",
                chat.id, message.id
            )
        })
    } else {
        None
    };
    let mut data = json!({
        "actor": { "id": message.author_id, "username": username, "display_name": name },
        "message_id": message.id,
    });
    if let Some(path) = path
        && let Some(map) = data.as_object_mut()
    {
        map.insert("path".to_owned(), json!(path));
    }
    let mail_shared = orch_settings::get::<bool>(conn, Place::System, "mail.shared")
        .await?
        .unwrap_or(true);
    let plan = mentioned
        .iter()
        .map(|user| {
            (
                *user,
                rows::NOTICE_MENTION,
                format!("mention:{}", message.id),
            )
        })
        .chain(renewed.iter().map(|user| {
            (
                *user,
                rows::NOTICE_MESSAGE,
                format!("{}:{}", rows::NOTICE_MESSAGE, chat.id),
            )
        }));
    for (user, kind, key) in plan {
        let (heading, body) = if kind == rows::NOTICE_MENTION {
            (format!("{name} mentioned you in {title}"), excerpt.clone())
        } else {
            (title.clone(), format!("{name}: {excerpt}"))
        };
        let written = rows::renew_notice(
            conn,
            &Notice {
                user_id: user,
                kind,
                node_id: chat.node_id,
                chat_id: chat.id,
                title: &heading,
                body: &body,
                data: &data,
                dedupe_key: &key,
                at: act.now,
            },
        )
        .await?;
        if let Some(written) = written {
            routes::tell_routes(
                conn,
                user,
                &Snapshot {
                    id: Some(written.id),
                    kind: kind.to_owned(),
                    at: Time::new(act.now),
                    title: heading,
                    body,
                    node_id: chat.node_id,
                    thread_id: None,
                    approval_id: None,
                    chat_id: Some(chat.id),
                    data: data.clone(),
                },
                Some(chat.org_id),
                mail_shared,
                act.now,
            )
            .await?;
        }
    }
    Ok(())
}

/// Tells the people newly named by an edit, the same way a new message tells
/// them: they are counted, engaged and notified, once each.
pub async fn after_edit(
    conn: &mut Conn,
    act: &Acting<'_>,
    loaded: &Loaded,
    message: &MessageRow,
    newly: &[Id],
) -> Result<(), Error> {
    if newly.is_empty() {
        return Ok(());
    }
    rows::raise_mentions(conn, loaded.id(), newly, act.now).await?;
    let followers = following::followers(conn, act, loaded).await?;
    let mentioned: Vec<Id> = newly
        .iter()
        .copied()
        .filter(|user| followers.iter().any(|follower| follower.user_id == *user))
        .collect();
    if !mentioned.is_empty() {
        write_notices(conn, act, loaded, message, &mentioned, &[]).await?;
    }
    for user in newly {
        let in_chat = loaded.has(*user);
        if let Some(state) = following::my_state(conn, *user, &loaded.row, in_chat, act.now).await?
        {
            tell_users(conn, &[*user], &UserEvent::ChatState { state }).await?;
        }
    }
    Ok(())
}

/// The defaults a person has for the levels of chats, for whoever needs them
/// to say what a state comes to.
pub async fn defaults_of(conn: &mut Conn, user: Id) -> Result<Defaults, Error> {
    Ok(Defaults::of(
        rows::default_levels(conn, &[user]).await?.get(&user),
    ))
}
