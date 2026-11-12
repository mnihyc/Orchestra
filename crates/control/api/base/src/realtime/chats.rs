//! Chat feed fan-out, typing and unread signals.
//!
//! A chat's feed is its messages by the chat's own sequence: a new message, an
//! edit, a reaction, a pin and a deletion are each the message's row with the
//! number it was stamped at, and the chat itself arrives when it or its people
//! changed. A subscriber is sent the rows stamped after the last number it was
//! sent, once for all who read from the same number, and what it may see is
//! asked of the access engine at the subscription and every time the epoch
//! moves: someone who is no longer in a group, or no longer holds `chat.view`
//! of the node, is closed with `revoked` before anything more is sent.
//!
//! Whether a chat has something new reaches a person who does not follow it
//! as news on their other feeds, which the writer of the message sends in the
//! transaction that writes it: the `seq` of the node's chat on the tree of its
//! org, and `chat` and `chat_state` on the person's own feed. This module
//! reads and sends only the chat's own.
//!
//! **Typing.** A person who types in a chat's composer sends `typing`, at most
//! once in three seconds for each chat and connection. It is passed to the
//! other subscribers of the chat on every API process (through the database's
//! announcements), never to the sender, never stored and never replayed. A
//! connection that only inspects the chat, as an admin who is not in it, is left
//! out of its presence and cannot type.

use crate::middleware::authorization::refusals;

use crate::realtime::server::Connection;

use crate::realtime::subscriptions::Access;

use crate::realtime::subscriptions::principal_of;

use crate::realtime::sync::Frame;

use crate::realtime::sync::Head;

use crate::realtime::sync::Rows;

use crate::realtime::Fail;

use crate::realtime::Inner;

use crate::realtime::Source;

use crate::routes::chats::read_operation;

use orch_api_contract::chats::CHAT_SYNC_MAX_CHANGES;

use orch_chat::chats;

use orch_chat::chats::Mode;

use orch_chat::Acting;

use orch_chat::Error as ChatError;

use orch_chat::inspection;

use orch_db::audit::Origin;

use orch_db::queries::chats as rows;

use orch_realtime_contract::feeds::EventData;

use orch_realtime_contract::feeds::Feed;

use orch_realtime_contract::feeds::SyncData;

use orch_realtime_contract::frames::EventFrame;

use orch_realtime_contract::frames::ServerFrame;

use orch_realtime_contract::frames::TypingFrame;

use orch_realtime_contract::sync::FeedHead;

use orch_realtime_contract::typing::TypingNotice;

use orch_realtime_contract::typing::shown_to;

use orch_types::Id;

use orch_types::Seq;

use std::sync::Arc;
/// What the chat feed reads a chat as: the connection's person, now.
async fn load_for(
    inner: &Inner,
    conn: &mut orch_db::Conn,
    user: Id,
    chat: Id,
) -> Result<Option<(orch_authz::Principal, chats::Loaded)>, Fail> {
    let Some(principal) = principal_of(inner, conn, user).await? else {
        return Ok(None);
    };
    let origin = Origin::default();
    let act = Acting {
        principal: &principal,
        now: inner.clock.now(),
        origin: &origin,
    };
    match chats::load(conn, &act, chat, Mode::Read).await {
        Ok(loaded) => Ok(Some((principal, loaded))),
        Err(ChatError::NotFound | ChatError::Lacks { .. } | ChatError::Suspended { .. }) => {
            Ok(None)
        }
        Err(error) => Err(Fail::other(error)),
    }
}

/// Whether `user` may follow the chat: in it, or reading it by the page of its
/// node, or as an admin who reads it without being in it (which is told, so
/// that such a connection is left out of the chat's presence).
pub(crate) async fn decide(inner: &Inner, user: Id, chat: Id) -> Result<Access, Fail> {
    let mut conn = inner.db.acquire().await?;
    if rows::chat(&mut conn, chat).await?.is_none() {
        return Ok(Access::NotFound);
    }
    Ok(match load_for(inner, &mut conn, user, chat).await? {
        Some((_, loaded)) if loaded.standing.inspects() => Access::Inspecting,
        Some(_) => Access::Allowed,
        None => Access::Forbidden,
    })
}

/// Writes the one audit row of an admin who opened a chat they are not in, when
/// they subscribed to it, on that subscription's transaction connection.
pub(crate) async fn opened(
    inner: &Inner,
    conn: &mut orch_db::Conn,
    user: Id,
    chat: Id,
    origin: &Origin,
) -> Result<(), Fail> {
    if let Some((principal, loaded)) = load_for(inner, conn, user, chat).await? {
        let act = Acting {
            principal: &principal,
            now: inner.clock.now(),
            origin,
        };
        inspection::note_opened(conn, &act, &loaded)
            .await
            .map_err(Fail::other)?;
    }
    Ok(())
}

/// Where the chat's feed stands, and how many rows lie above `since`.
pub(crate) async fn head(
    inner: &Inner,
    chat: Id,
    since: Option<Seq>,
) -> Result<Option<Head>, Fail> {
    let mut conn = inner.db.acquire().await?;
    let Some(row) = rows::chat(&mut conn, chat).await? else {
        return Ok(None);
    };
    let seq = row.seq();
    let rows_after = match since {
        Some(since) if since < seq => {
            rows::count_changed(&mut conn, chat, since).await? + i64::from(row.seq_state > since)
        }
        _ => 0,
    };
    Ok(Some(Head {
        feed: FeedHead {
            seq,
            purged_seq: 0,
            incremental_limit: CHAT_SYNC_MAX_CHANGES,
        },
        rows_after,
    }))
}

/// The rows of the chat stamped in `(after, upto]`, as `person` sees them: the
/// messages that changed, and the chat itself when it or its people did.
pub(crate) async fn read(
    inner: &Inner,
    person: Id,
    chat: Id,
    after: Seq,
    upto: Seq,
) -> Result<Rows, Fail> {
    let mut conn = orch_db::queries::feeds::begin_reading(&inner.db).await?;
    let Some(principal) = principal_of(inner, &mut conn, person).await? else {
        return Ok(Rows::Revoked);
    };
    let Some(row) = rows::chat(&mut conn, chat).await? else {
        return Ok(Rows::Gone);
    };
    let origin = Origin::default();
    let act = Acting {
        principal: &principal,
        now: inner.clock.now(),
        origin: &origin,
    };
    let loaded = match chats::load(&mut conn, &act, chat, Mode::Read).await {
        Ok(loaded) => loaded,
        Err(ChatError::NotFound) => {
            // Root scope was admitted before this read. A retained chat row whose governing
            // place was deleted is gone; a live private chat that withdrew access is revoked.
            let place = row.home();
            let graph = orch_authz::engine::Graph::load(&mut conn, &[place]).await?;
            return Ok(if graph.site_of(place).is_none() {
                Rows::Gone
            } else {
                Rows::Revoked
            });
        }
        Err(ChatError::Lacks { .. } | ChatError::Suspended { .. }) => return Ok(Rows::Revoked),
        Err(error) => return Err(Fail::other(error)),
    };
    let messages = rows::changed(
        &mut conn,
        chat,
        after,
        Some(upto),
        CHAT_SYNC_MAX_CHANGES + 1,
    )
    .await?;
    if i64::try_from(messages.len()).unwrap_or(i64::MAX) > CHAT_SYNC_MAX_CHANGES {
        return Ok(Rows::TooMany);
    }
    let state_moved = row.seq_state > after && row.seq_state <= upto;
    let highest = messages
        .iter()
        .map(|message| message.seq_updated)
        .chain(state_moved.then_some(row.seq_state))
        .max();
    let Some(highest) = highest else {
        return Ok(Rows::Nothing);
    };
    let projected = match read_operation::read_for(
        &mut conn,
        &principal,
        &loaded,
        &messages,
        state_moved,
    )
    .await
    {
        Ok(projected) => projected,
        Err(error)
            if matches!(
                error.problem.code,
                orch_api_contract::common::ErrorCode::AuthzDenied
                    | orch_api_contract::common::ErrorCode::NotFound
                    | orch_api_contract::common::ErrorCode::AuthSuspended
                    | orch_api_contract::common::ErrorCode::OrgSuspended
                    | orch_api_contract::common::ErrorCode::AuthInvalidToken
                    | orch_api_contract::common::ErrorCode::TokenRefusedHere
                    | orch_api_contract::common::ErrorCode::TokenLifetimeExceeded
            ) =>
        {
            return Ok(Rows::Revoked);
        }
        Err(error) => return Err(Fail::other(error)),
    };
    let shown_messages = (!messages.is_empty()).then_some(projected.messages);
    conn.commit().await.map_err(orch_db::DbError::from)?;
    Ok(Rows::Frame(Box::new(Frame {
        data: SyncData::Chat(orch_realtime_contract::feeds::ChatData {
            chat: projected.chat,
            messages: shown_messages,
        }),
        highest,
        held: Vec::new(),
    })))
}

// ---------------------------------------------------------------- typing

/// A `typing` frame: passed on if the connection follows that chat, is not
/// only inspecting it, and has not sent one in the last three seconds.
pub(crate) async fn typing(inner: &Arc<Inner>, connection: &Arc<Connection>, frame: &TypingFrame) {
    let Ok(feed) = frame.feed() else { return };
    let Feed::Chat(chat) = &feed else { return };
    let Some(own) = connection
        .subscriptions()
        .into_iter()
        .find(|sub| sub.feed() == &feed && sub.is_live())
    else {
        return;
    };
    if own.inspecting() || !own.admit_typing(inner.clock.time()) {
        return;
    }
    let sent = async {
        let mut conn = inner.db.acquire().await?;
        rows::notify_typing(&mut conn, *chat, connection.user).await
    }
    .await;
    if let Err(error) = sent {
        tracing::warn!(%error, "somebody's typing could not be passed on");
    }
}

/// `user` is typing in `chat`: every subscriber of the chat on this process
/// but the person themself is told, with who it is.
pub(crate) async fn deliver_typing(inner: &Arc<Inner>, chat: Id, user: Id) {
    let subscribers = inner.subscribers_of(Source::Chat(chat));
    if subscribers.is_empty() {
        return;
    }
    let who = async {
        let mut conn = inner.db.acquire().await.ok()?;
        refusals::user_refs(&mut conn, &[user])
            .await
            .ok()?
            .remove(&user)
    }
    .await;
    let Some(who) = who else { return };
    let feed = Feed::Chat(chat);
    let notice = EventData::Typing(TypingNotice::Typing {
        user: who,
        at: None,
    });
    for sub in subscribers {
        if !sub.is_live() || !shown_to(user, sub.connection().user) {
            continue;
        }
        match super::watches::reader_guard(inner, &sub).await {
            Ok(Some(())) => {}
            Ok(None) => continue,
            Err(error) => {
                tracing::warn!(%error, "chat typing could not authorize its reader");
                continue;
            }
        };
        let Ok(frame) = EventFrame::new(sub.tag().id, feed.clone(), &notice) else {
            continue;
        };
        if let Ok(text) = ServerFrame::Event(frame).encode() {
            sub.connection().outbox.push_event(sub.tag(), &feed, text);
        }
    }
}
