//! Engagement, chosen levels, unread state and mark-read.
//!
//! Only the people who take part in a chat follow it, unless somebody chooses
//! otherwise. A person's level in a chat is, in this order: what they set
//! there themself with the bell; otherwise, when they have written there or
//! were named there, or are in the direct or group chat, their default for
//! that kind of chat; otherwise `off`. Whoever has a level other than `off`
//! follows the chat, and it is unread for them when its newest message is
//! beyond the position they have read to. Reading does not make anybody
//! follow, the position only moves forward, and a person's draft is theirs
//! alone.

use crate::attachments;
use crate::chats::Loaded;
use crate::notifications;
use crate::{Acting, Error};
use orch_access_contract::decisions::{Decision, ObjectRef};
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_api_contract::chats::{
    CHAT_MESSAGE_MAX_PARTS, ChatDefaultReason, ChatDraft, ChatLevel, ChatPart, ChatState,
};
use orch_api_contract::threads::ContentPart;
use orch_authz::{Act, Policy, Principal, documents, nodes, resources};
use orch_db::Conn;
use orch_db::queries::chats::{self as rows, ChatRow, StateRow};
use orch_db::queries::{storage, uploads};
use orch_realtime_contract::feeds::UserEvent;
use orch_types::{Hash, Id, Seq, Time};

/// Whether the caller retained these bytes or may currently read a saved source of them.
pub async fn may_use_blob(
    conn: &mut Conn,
    principal: &Principal,
    hash: &Hash,
) -> Result<bool, Error> {
    if uploads::owns(conn, principal.user(), hash).await? {
        return Ok(true);
    }
    for (kind, id) in uploads::sources(conn, principal.user(), hash).await? {
        let allowed = match kind.as_str() {
            "chat" => {
                let origin = orch_db::audit::Origin::default();
                let act = Acting {
                    principal,
                    now: principal.now().get(),
                    origin: &origin,
                };
                match crate::chats::load(conn, &act, id, crate::chats::Mode::Read).await {
                    Ok(_) => true,
                    Err(Error::Db(error)) => return Err(error.into()),
                    Err(Error::Internal(error)) => return Err(Error::Internal(error)),
                    Err(_) => false,
                }
            }
            "thread" => matches!(
                nodes::check(
                    conn,
                    principal,
                    id,
                    Permission::ThreadView,
                    Act::Use,
                    Policy::default()
                )
                .await,
                Ok(Decision::Allowed { .. })
            ),
            "document" => matches!(
                documents::check(conn, principal, id, Permission::DocView, Policy::default()).await,
                Ok(Decision::Allowed { .. })
            ),
            "environment" => matches!(
                resources::check(
                    conn,
                    principal,
                    ObjectRef {
                        kind: ObjectKind::Environment,
                        id
                    },
                    Permission::FilesView,
                    Policy::default()
                )
                .await,
                Ok(Decision::Allowed { .. })
            ),
            _ => false,
        };
        if allowed {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Refuses a hash without an authorized upload or readable saved source, concealing its existence.
pub async fn require_blobs(
    conn: &mut Conn,
    principal: &Principal,
    hashes: &[Hash],
) -> Result<(), Error> {
    for hash in hashes {
        if !may_use_blob(conn, principal, hash).await? {
            return Err(Error::NotFound);
        }
    }
    Ok(())
}

/// The distinct uploaded bytes named by an agent input, including files carried by forwarded copies.
#[must_use]
pub fn content_blobs(content: &[ContentPart]) -> Vec<Hash> {
    let mut hashes = Vec::new();
    for part in content {
        match part {
            ContentPart::PastedText(paste) => hashes.push(paste.blob),
            ContentPart::Image { blob, .. }
            | ContentPart::File {
                blob: Some(blob), ..
            } => hashes.push(*blob),
            ContentPart::Forwarded(copy) => {
                hashes.extend(copy.images.iter().map(|image| image.blob));
                hashes.extend(copy.attachments.iter().map(|file| file.blob));
            }
            _ => {}
        }
    }
    hashes.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
    hashes.dedup();
    hashes
}

/// The word a level is stored as.
#[must_use]
pub const fn level_word(level: ChatLevel) -> &'static str {
    match level {
        ChatLevel::All => "all",
        ChatLevel::Mentions => "mentions",
        ChatLevel::Off => "off",
    }
}

/// The level a stored word means.
#[must_use]
pub fn parse_level(word: &str) -> Option<ChatLevel> {
    match word {
        "all" => Some(ChatLevel::All),
        "mentions" => Some(ChatLevel::Mentions),
        "off" => Some(ChatLevel::Off),
        _ => None,
    }
}

/// What a person's notification preferences give them in a chat that they did
/// not set a level in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Defaults {
    /// In the chat of a node.
    pub node: ChatLevel,
    /// In a direct or a group chat.
    pub direct: ChatLevel,
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            node: parse_level(rows::DEFAULT_NODE_LEVEL).unwrap_or(ChatLevel::Mentions),
            direct: parse_level(rows::DEFAULT_PEOPLE_LEVEL).unwrap_or(ChatLevel::All),
        }
    }
}

impl Defaults {
    /// What the preferences of a person say; the shipped defaults where they
    /// chose none.
    #[must_use]
    pub fn of(stored: Option<&(String, String)>) -> Self {
        stored.map_or_else(Self::default, |(node, direct)| {
            let shipped = Self::default();
            Self {
                node: parse_level(node).unwrap_or(shipped.node),
                direct: parse_level(direct).unwrap_or(shipped.direct),
            }
        })
    }
}

/// A person's level in a chat: what they set; else, when they took part in it
/// (or are in the direct or group chat), the default for that kind of chat;
/// else `off`.
#[must_use]
pub fn effective_level(
    people_chat: bool,
    in_chat: bool,
    state: Option<&StateRow>,
    defaults: Defaults,
) -> ChatLevel {
    if let Some(set) = state
        .and_then(|state| state.level.as_deref())
        .and_then(parse_level)
    {
        return set;
    }
    let engaged = state.is_some_and(|state| state.engaged);
    match (people_chat, in_chat || engaged) {
        (true, true) => defaults.direct,
        (false, true) if engaged => defaults.node,
        _ => ChatLevel::Off,
    }
}

/// A person's state in a chat as the API has it.
#[must_use]
pub fn state_view(chat: Id, state: Option<&StateRow>, effective: ChatLevel) -> ChatState {
    ChatState {
        read_revision: state.map_or(0, |state| state.read_revision),
        default_level: None,
        default_reason: None,
        chat_id: chat,
        level: state
            .and_then(|state| state.level.as_deref())
            .and_then(parse_level),
        engaged: state.is_some_and(|state| state.engaged),
        effective_level: effective,
        last_read_seq: state.map_or(0, |state| state.last_read_seq),
        mentions_unread: state.map_or(0, |state| i64::from(state.mentions_unread)),
        hidden: state.is_some_and(|state| state.hidden),
    }
}

/// The caller's own state in a chat: always there in a direct or a group chat
/// they are in (with the defaults, until they have a row), and in a node's
/// chat once they have a row. Nothing for somebody who only inspects.
pub async fn my_state(
    conn: &mut Conn,
    user: Id,
    chat: &ChatRow,
    in_chat: bool,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<Option<ChatState>, Error> {
    let state = rows::state(conn, user, chat.id).await?;
    let derived = rows::following::derived(conn, Some(user), Some(&[chat.id]), now).await?;
    let derived = derived.first();
    if state.is_none() && derived.is_none() && !(chat.is_people_chat() && in_chat) {
        return Ok(None);
    }
    let defaults = Defaults::of(rows::default_levels(conn, &[user]).await?.get(&user));
    let reason = if derived.is_some_and(|derived| derived.owner) {
        ChatDefaultReason::Owner
    } else if derived.is_some_and(|derived| derived.following) {
        ChatDefaultReason::Following
    } else if chat.is_people_chat() && in_chat {
        ChatDefaultReason::Member
    } else if state.as_ref().is_some_and(|state| state.engaged) {
        ChatDefaultReason::Engaged
    } else {
        ChatDefaultReason::Quiet
    };
    let default = match reason {
        ChatDefaultReason::Owner | ChatDefaultReason::Following => derived
            .and_then(|derived| parse_level(&derived.default_level))
            .unwrap_or(defaults.direct),
        ChatDefaultReason::Member => defaults.direct,
        ChatDefaultReason::Engaged if chat.is_people_chat() => defaults.direct,
        ChatDefaultReason::Engaged => defaults.node,
        ChatDefaultReason::Quiet => ChatLevel::Off,
    };
    let level = state
        .as_ref()
        .and_then(|state| state.level.as_deref())
        .and_then(parse_level)
        .unwrap_or(default);
    let mut shown = state_view(chat.id, state.as_ref(), level);
    shown.default_level = Some(default);
    shown.default_reason = Some(reason);
    Ok(Some(shown))
}

/// Tells the caller's other screens how their state in a chat stands now.
async fn tell_screens(
    conn: &mut Conn,
    act: &Acting<'_>,
    loaded: &Loaded,
) -> Result<ChatState, Error> {
    let state = my_state(
        conn,
        act.user(),
        &loaded.row,
        loaded.standing.in_chat,
        act.now,
    )
    .await?
    .unwrap_or_else(|| state_view(loaded.id(), None, ChatLevel::Off));
    notifications::tell_users(
        conn,
        &[act.user()],
        &UserEvent::ChatState {
            state: state.clone(),
        },
    )
    .await?;
    Ok(state)
}

/// Only somebody who is in the chat, or who reads it by the page of its node,
/// has a position in it, a level and a draft; an admin who reads it as an
/// outsider has none.
fn ensure_own(loaded: &Loaded) -> Result<(), Error> {
    if loaded.standing.inspects() {
        Err(Error::Inspecting)
    } else {
        Ok(())
    }
}

/// Moves the caller's read position forward to `seq` and clears their unread
/// mentions; marks the notifications of the chat as read; tells their other
/// screens. A position beyond what the chat has issued is taken as the last
/// one issued, and a position behind the one held changes nothing but the
/// mentions.
pub async fn mark_read(
    conn: &mut Conn,
    act: &Acting<'_>,
    loaded: &Loaded,
    seq: Seq,
) -> Result<ChatState, Error> {
    ensure_own(loaded)?;
    if seq < 0 {
        return Err(Error::invalid("seq", "is a position in the chat, from 0"));
    }
    let position = seq.min(loaded.row.seq());
    rows::mark_read(conn, act.user(), loaded.id(), position).await?;
    rows::read_notices(conn, act.user(), loaded.id(), act.now).await?;
    tell_screens(conn, act, loaded).await
}

/// Advances or deliberately rewinds the caller's position under the observed state revision.
pub async fn mark_read_observed(
    conn: &mut Conn,
    act: &Acting<'_>,
    loaded: &Loaded,
    seq: Seq,
    revision: i64,
    unread_from: bool,
) -> Result<ChatState, Error> {
    ensure_own(loaded)?;
    if seq < 0 || revision < 0 {
        return Err(Error::invalid(
            "seq",
            "read positions and revisions are nonnegative",
        ));
    }
    let current = rows::following::locked(conn, act.user(), loaded.id()).await?;
    if current.read_revision != revision {
        return Err(Error::StaleRead {
            revision: current.read_revision,
        });
    }
    let (position, next_revision) = if unread_from {
        if !rows::following::has_anchor(conn, loaded.id(), seq).await? {
            return Err(Error::invalid("seq", "is not a live message of this chat"));
        }
        (seq.saturating_sub(1), revision.saturating_add(1))
    } else {
        (
            current.last_read_seq.max(seq.min(loaded.row.seq())),
            revision,
        )
    };
    rows::following::put_read(conn, act.user(), loaded.id(), position, next_revision).await?;
    if !unread_from {
        rows::read_notices_through(conn, act.user(), loaded.id(), Some(position), act.now).await?;
    }
    tell_screens(conn, act, loaded).await
}

/// Current chat followers, including thread owners and whole followers without creating engagement.
pub async fn followers(
    conn: &mut Conn,
    act: &Acting<'_>,
    loaded: &Loaded,
) -> Result<Vec<rows::Follower>, Error> {
    if loaded.row.is_people_chat() {
        return Ok(rows::people_chat_followers(conn, loaded.id()).await?);
    }
    let mut people: std::collections::BTreeMap<Id, String> =
        rows::node_chat_followers(conn, loaded.id())
            .await?
            .into_iter()
            .map(|row| (row.user_id, row.level))
            .collect();
    for derived in rows::following::derived(conn, None, Some(&[loaded.id()]), act.now).await? {
        people.remove(&derived.user_id);
        if derived.level != "off" {
            people.insert(derived.user_id, derived.level);
        }
    }
    Ok(people
        .into_iter()
        .map(|(user_id, level)| rows::Follower { user_id, level })
        .collect())
}

/// Repairs the ordinary chat-state event after a thread's ownership or whole subscription changes.
pub async fn refresh_thread(
    conn: &mut Conn,
    thread: Id,
    previous: &[Id],
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(), Error> {
    if rows::node_kind(conn, thread).await?.as_deref() != Some("thread") {
        return Ok(());
    }
    let Some(chat) = rows::chat_of_node(conn, thread).await? else {
        return Ok(());
    };
    let mut users: Vec<Id> = rows::following::derived(conn, None, Some(&[chat.id]), now)
        .await?
        .into_iter()
        .map(|reason| reason.user_id)
        .collect();
    users.extend_from_slice(previous);
    users.sort_unstable();
    users.dedup();
    refresh_thread_people(conn, thread, &users, now).await
}

/// Refreshes only the named people after a change that cannot alter anybody else's following.
/// The explicit chat grant passes one person; this path never enumerates the chat's followers.
pub async fn refresh_thread_people(
    conn: &mut Conn,
    thread: Id,
    users: &[Id],
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(), Error> {
    if rows::node_kind(conn, thread).await?.as_deref() != Some("thread") {
        return Ok(());
    }
    let Some(chat) = rows::chat_of_node(conn, thread).await? else {
        return Ok(());
    };
    for user in users {
        let user = *user;
        let principal =
            Principal::load(conn, orch_authz::Caller::session(user), Time::new(now)).await?;
        let held = nodes::check(
            conn,
            &principal,
            thread,
            Permission::ChatView,
            Act::Use,
            Policy::default(),
        )
        .await?;
        if !held.is_allowed() {
            continue;
        }
        let state = my_state(conn, user, &chat, false, now)
            .await?
            .unwrap_or_else(|| {
                let mut state = state_view(chat.id, None, ChatLevel::Off);
                state.default_level = Some(ChatLevel::Off);
                state.default_reason = Some(ChatDefaultReason::Quiet);
                state
            });
        notifications::tell_users(conn, &[user], &UserEvent::ChatState { state }).await?;
    }
    Ok(())
}

/// What the caller changes of their state in a chat; what is left out stays.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Change {
    /// The bell: a value follows or mutes; `Some(None)` returns to the default.
    pub level: Option<Option<ChatLevel>>,
    /// Whether a direct or group chat is off their list.
    pub hidden: Option<bool>,
}

/// Changes the caller's level in a chat and whether it is on their list.
pub async fn set_state(
    conn: &mut Conn,
    act: &Acting<'_>,
    loaded: &Loaded,
    change: Change,
) -> Result<ChatState, Error> {
    ensure_own(loaded)?;
    if change.hidden.is_some() && !loaded.row.is_people_chat() {
        return Err(Error::invalid(
            "hidden",
            "only a direct or a group chat is taken off a list",
        ));
    }
    if let Some(level) = change.level {
        rows::set_level(
            conn,
            act.user(),
            loaded.id(),
            level.map(level_word),
            act.now,
        )
        .await?;
    }
    if let Some(hidden) = change.hidden {
        rows::set_hidden(conn, act.user(), loaded.id(), hidden, act.now).await?;
    }
    tell_screens(conn, act, loaded).await
}

/// Marks as read, up to now, every chat of a root org that the caller follows
/// and has something unread in. The chats it marked.
pub async fn mark_all_read(conn: &mut Conn, act: &Acting<'_>, root: Id) -> Result<Vec<Id>, Error> {
    use orch_access_contract::permissions::{Permission, PermissionName};
    use orch_authz::{Act, Policy, nodes};
    let unread = rows::unread_in_org(conn, act.user(), root).await?;
    let node_ids: Vec<Id> = unread.iter().filter_map(|(chat, _)| chat.node_id).collect();
    let readable: Vec<Id> = if node_ids.is_empty() {
        Vec::new()
    } else {
        nodes::holding(
            conn,
            act.principal,
            &node_ids,
            PermissionName::Exact(Permission::ChatView),
            Act::Use,
            Policy::default(),
        )
        .await?
        .into_iter()
        .map(|listed| listed.id)
        .collect()
    };
    let mut marked = Vec::new();
    for (chat, _) in unread {
        let people = chat.is_people_chat();
        if chat.node_id.is_some_and(|node| !readable.contains(&node)) {
            continue;
        }
        if my_state(conn, act.user(), &chat, people, act.now)
            .await?
            .is_none_or(|state| state.effective_level == ChatLevel::Off)
        {
            continue;
        }
        rows::mark_read(conn, act.user(), chat.id, chat.last_message_seq).await?;
        rows::read_notices_through(
            conn,
            act.user(),
            chat.id,
            Some(chat.last_message_seq),
            act.now,
        )
        .await?;
        let state = my_state(conn, act.user(), &chat, people, act.now).await?;
        if let Some(state) = state {
            notifications::tell_users(conn, &[act.user()], &UserEvent::ChatState { state }).await?;
        }
        marked.push(chat.id);
    }
    Ok(marked)
}

// ------------------------------------------------------------------ drafts

/// What the caller has begun to write in a chat and not sent.
pub async fn draft(
    conn: &mut Conn,
    act: &Acting<'_>,
    chat: Id,
) -> Result<Option<ChatDraft>, Error> {
    let Some((parts, reply_to, saved)) = rows::draft(conn, act.user(), chat).await? else {
        return Ok(None);
    };
    Ok(Some(ChatDraft {
        mention_names: Vec::new(),
        parts: serde_json::from_value(parts).unwrap_or_default(),
        reply_to,
        updated_at: Time::new(saved),
    }))
}

/// Keeps what the caller has written, to be there on any of their screens.
pub async fn save_draft(
    conn: &mut Conn,
    act: &Acting<'_>,
    loaded: &Loaded,
    parts: &[ChatPart],
    reply_to: Option<Id>,
) -> Result<ChatDraft, Error> {
    loaded.ensure_send()?;
    ensure_own(loaded)?;
    if parts.len() > usize::try_from(CHAT_MESSAGE_MAX_PARTS).unwrap_or(usize::MAX) {
        return Err(Error::invalid(
            "parts",
            format!("a message holds at most {CHAT_MESSAGE_MAX_PARTS} parts"),
        ));
    }
    if parts
        .iter()
        .any(|part| matches!(part, ChatPart::Forwarded(_)))
    {
        return Err(Error::invalid(
            "parts",
            "a copy of a message is made by forwarding it",
        ));
    }
    if let Some(reply) = reply_to
        && rows::message(conn, loaded.id(), reply).await?.is_none()
    {
        return Err(Error::invalid("reply_to", "is not a message of this chat"));
    }
    let content =
        serde_json::to_value(parts).map_err(|error| Error::Internal(error.to_string()))?;
    uploads::lock_chat_draft(conn, act.user(), loaded.id()).await?;
    let before = rows::draft(conn, act.user(), loaded.id())
        .await?
        .map(|(parts, _, _)| serde_json::from_value::<Vec<ChatPart>>(parts))
        .transpose()
        .map_err(|error| Error::Internal(error.to_string()))?
        .unwrap_or_default();
    let old = attachments::blobs_in(&before);
    let new = attachments::blobs_in(parts);
    let mut all = old.clone();
    all.extend(new.iter().copied());
    uploads::lock_blobs(conn, &all).await?;
    require_blobs(conn, act.principal, &new).await?;
    let largest = attachments::largest(conn, &loaded.row).await?;
    attachments::check(conn, parts, largest).await?;
    let added: Vec<Hash> = new
        .iter()
        .copied()
        .filter(|hash| !old.contains(hash))
        .collect();
    let removed: Vec<Hash> = old
        .iter()
        .copied()
        .filter(|hash| !new.contains(hash))
        .collect();
    if storage::pin_blobs(conn, &added).await? != u64::try_from(added.len()).unwrap_or(u64::MAX) {
        return Err(Error::NotFound);
    }
    for hash in &new {
        uploads::remember(conn, act.user(), hash, act.now).await?;
    }
    rows::save_draft(conn, act.user(), loaded.id(), &content, reply_to, act.now).await?;
    storage::unpin_blobs(conn, &removed, act.now).await?;
    let saved = ChatDraft {
        mention_names: Vec::new(),
        parts: parts.to_vec(),
        reply_to,
        updated_at: Time::new(act.now),
    };
    notifications::tell_users(
        conn,
        &[act.user()],
        &UserEvent::ChatDraft {
            chat_id: loaded.id(),
            draft: Some(saved.clone()),
        },
    )
    .await?;
    Ok(saved)
}

/// Throws the caller's draft away.
pub async fn discard_draft(
    conn: &mut Conn,
    act: &Acting<'_>,
    loaded: &Loaded,
) -> Result<(), Error> {
    loaded.ensure_send()?;
    ensure_own(loaded)?;
    uploads::lock_chat_draft(conn, act.user(), loaded.id()).await?;
    let before = rows::draft(conn, act.user(), loaded.id())
        .await?
        .map(|(parts, _, _)| serde_json::from_value::<Vec<ChatPart>>(parts))
        .transpose()
        .map_err(|error| Error::Internal(error.to_string()))?
        .unwrap_or_default();
    let blobs = attachments::blobs_in(&before);
    uploads::lock_blobs(conn, &blobs).await?;
    if rows::delete_draft(conn, act.user(), loaded.id()).await? {
        storage::unpin_blobs(conn, &blobs, act.now).await?;
        notifications::tell_users(
            conn,
            &[act.user()],
            &UserEvent::ChatDraft {
                chat_id: loaded.id(),
                draft: None,
            },
        )
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use orch_types::new_id;

    fn state(level: Option<&str>, engaged: bool) -> StateRow {
        StateRow {
            read_revision: 0,
            user_id: new_id(),
            chat_id: new_id(),
            level: level.map(str::to_owned),
            engaged,
            last_read_seq: 0,
            mentions_unread: 0,
            draft: None,
            draft_reply_to: None,
            hidden: false,
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn what_a_person_set_wins_over_taking_part_and_over_being_in_the_chat() {
        let defaults = Defaults::default();
        let muted = state(Some("off"), true);
        assert_eq!(
            effective_level(true, true, Some(&muted), defaults),
            ChatLevel::Off
        );
        let followed = state(Some("all"), false);
        assert_eq!(
            effective_level(false, false, Some(&followed), defaults),
            ChatLevel::All,
            "the bell follows a chat whatever happened before"
        );
    }

    #[test]
    fn reading_a_node_chat_alone_follows_nothing_and_taking_part_follows_at_the_default() {
        let defaults = Defaults::default();
        assert_eq!(
            effective_level(false, false, None, defaults),
            ChatLevel::Off
        );
        let reader = state(None, false);
        assert_eq!(
            effective_level(false, false, Some(&reader), defaults),
            ChatLevel::Off
        );
        let engaged = state(None, true);
        assert_eq!(
            effective_level(false, false, Some(&engaged), defaults),
            defaults.node
        );
    }

    #[test]
    fn the_people_of_a_direct_chat_follow_it_at_their_default_without_having_written() {
        let defaults = Defaults::default();
        assert_eq!(effective_level(true, true, None, defaults), defaults.direct);
        let own = Defaults::of(Some(&("off".to_owned(), "mentions".to_owned())));
        assert_eq!(effective_level(true, true, None, own), ChatLevel::Mentions);
        assert_eq!(effective_level(false, false, None, own), ChatLevel::Off);
    }
}
