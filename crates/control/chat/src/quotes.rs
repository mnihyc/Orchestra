//! Timeline items quoted into a thread's chat, and their counts.
//!
//! A message in a thread's chat may quote items of that thread. Each quote is
//! a row that ties the message to the item, and the item counts the messages
//! that quote it: the count is updated, and the item stamped in the thread's
//! own sequence so that whoever has the thread open learns of it, in the
//! transaction that writes or deletes the message. A writer that needs both
//! sequences takes the thread's first and the chat's second.

use crate::{Acting, Error};
use chrono::{DateTime, Utc};
use orch_access_contract::decisions::Decision;
use orch_access_contract::permissions::Permission;
use orch_api_contract::chats::ChatPart;
use orch_authz::{Act, Policy, nodes};
use orch_db::queries::chats::{self as rows, ChatRow};
use orch_db::{Conn, sequences};
use orch_types::{Id, Seq};

/// The items of a thread that a message quotes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Quoted {
    /// The thread whose chat it is.
    pub thread: Id,
    /// The items, each once.
    pub items: Vec<Id>,
}

/// The items the parts quote, each once, in the order they are quoted, and the
/// threads they say they are of.
#[must_use]
pub fn refs_in(parts: &[ChatPart]) -> Vec<(Id, Id)> {
    let mut found: Vec<(Id, Id)> = Vec::new();
    for part in parts {
        if let ChatPart::ItemRef { thread_id, item_id } = part
            && !found.iter().any(|(_, item)| item == item_id)
        {
            found.push((*thread_id, *item_id));
        }
    }
    found
}

/// Checks the quotes of parts that are about to be written in `chat` by the
/// caller: they are in the chat of a thread, the items are of that thread, and
/// the caller can read the thread. None when the parts quote nothing.
pub async fn check(
    conn: &mut Conn,
    act: &Acting<'_>,
    chat: &ChatRow,
    parts: &[ChatPart],
) -> Result<Option<Quoted>, Error> {
    let refs = refs_in(parts);
    let Some((first, _)) = refs.first().copied() else {
        return Ok(None);
    };
    let thread = match chat.node_id {
        Some(node) if rows::node_kind(conn, node).await?.as_deref() == Some("thread") => node,
        _ => {
            return Err(Error::invalid(
                "parts",
                "an item is quoted only in the chat of its thread",
            ));
        }
    };
    if first != thread || refs.iter().any(|(of, _)| *of != thread) {
        return Err(Error::invalid(
            "parts",
            "an item is quoted only in the chat of its thread",
        ));
    }
    let items: Vec<Id> = refs.iter().map(|(_, item)| *item).collect();
    let real = rows::items_of_thread(conn, thread, &items).await?;
    if let Some(missing) = items.iter().find(|item| !real.contains(item)) {
        return Err(Error::invalid(
            "parts",
            format!("{missing} is not an item of this thread"),
        ));
    }
    let seen = nodes::check(
        conn,
        act.principal,
        thread,
        Permission::ThreadView,
        Act::Use,
        Policy::default(),
    )
    .await?;
    if matches!(seen, Decision::Refused(_)) {
        return Err(Error::Lacks {
            node: thread,
            permission: Permission::ThreadView,
        });
    }
    Ok(Some(Quoted { thread, items }))
}

/// Takes the thread's next number. It is taken before the chat's, whenever a
/// write needs both.
pub async fn begin(conn: &mut Conn, thread: Id) -> Result<Seq, Error> {
    sequences::next(conn, thread)
        .await?
        .ok_or_else(|| Error::Internal("the thread of a chat is not there".to_owned()))
}

/// Records that a message quotes `added` and no longer quotes `removed`, counts
/// them on the items, stamps the items at `seq` of the thread's sequence and
/// announces it.
pub async fn record(
    conn: &mut Conn,
    thread: Id,
    seq: Seq,
    message: Id,
    changes: &Changes<'_>,
    at: DateTime<Utc>,
) -> Result<(), Error> {
    if !changes.added.is_empty() {
        rows::add_item_refs(conn, message, changes.added).await?;
        rows::count_item_quotes(conn, changes.added, 1, seq, at).await?;
    }
    if !changes.removed.is_empty() {
        rows::drop_item_refs(conn, message, changes.removed).await?;
        rows::count_item_quotes(conn, changes.removed, -1, seq, at).await?;
    }
    sequences::notify(conn, thread, seq).await?;
    Ok(())
}

/// The items a message begins to quote and those it stops quoting.
#[derive(Debug, Clone, Copy)]
pub struct Changes<'a> {
    /// Quoted now and not before.
    pub added: &'a [Id],
    /// Quoted before and not now.
    pub removed: &'a [Id],
}
