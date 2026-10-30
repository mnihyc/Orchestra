//! The document feed: blocks that changed by sequence, discussions, presence, and who is writing where.
//!
//! A document's feed carries the rows stamped from the document's own
//! sequence: the blocks a batch touched, as they now stand (the removed ones
//! with `removed: true`), the discussions in which something was said, and
//! the document itself when its row moved. They are the rows the document's
//! own request for them (`GET /documents/{id}/sync`) returns, read once for
//! all who read from the same number and put into names for each subscriber.
//! Who may follow it is `doc.view`.
//!
//! What a person types in a block is written over HTTP and comes back as a
//! row. Who is writing, and in which block, is told apart from that and never
//! stored: a client sends `typing` at most every three seconds, the server
//! announces it to every API process through the database, and each tells the
//! subscribers of the document who are not that person. A viewer who cannot
//! edit is never shown as writing. Who has the document open is the
//! presence of the feed, as for a thread's.

use crate::realtime::server::Connection;
use crate::realtime::subscriptions::{Access, principal_of};
use crate::realtime::sync::{Cache, Frame, Head, Rows};
use crate::realtime::{Fail, Inner, Source};
use crate::routes::documents::build;
use orch_access_contract::decisions::Decision;
use orch_access_contract::permissions::Permission;
use orch_api_contract::documents::DOCUMENT_SYNC_MAX_CHANGES;
use orch_authz::{Policy, documents};
use orch_db::queries::documents::discussions::{self, Full};
use orch_db::queries::documents::{self as rows, BlockRow, DocumentRow};
use orch_db::sequences;
use orch_realtime_contract::documents::DocData;
use orch_realtime_contract::feeds::{EventData, Feed, SyncData};
use orch_realtime_contract::frames::{EventFrame, ServerFrame};
use orch_realtime_contract::sync::FeedHead;
use orch_realtime_contract::typing::{TypingNotice, shown_to};
use orch_types::{Id, Seq};
use std::sync::Arc;

/// What a typing announcement starts with, on the document channel, to tell it
/// from the announcement of a sequence.
const TYPING: &str = "t";
/// What stands in an announcement of typing where no block was named.
const NO_BLOCK: &str = "-";

/// What was read of a document for all who read from one number.
#[derive(Debug)]
pub(crate) struct Shared {
    gone: bool,
    too_many: bool,
    blocks: Vec<BlockRow>,
    discussions: Vec<Full>,
    /// The row of the document, when it moved in the range.
    row: Option<DocumentRow>,
    highest: Option<Seq>,
}

impl Shared {
    fn nothing() -> Self {
        Self {
            gone: false,
            too_many: false,
            blocks: Vec::new(),
            discussions: Vec::new(),
            row: None,
            highest: None,
        }
    }

    fn gone() -> Self {
        Self {
            gone: true,
            ..Self::nothing()
        }
    }

    fn too_many() -> Self {
        Self {
            too_many: true,
            ..Self::nothing()
        }
    }
}

/// Whether `user` may follow the document: whoever may read it.
pub(crate) async fn decide(inner: &Inner, user: Id, document: Id) -> Result<Access, Fail> {
    let mut conn = inner.db.acquire().await?;
    if rows::get(&mut conn, document).await?.is_none() {
        return Ok(Access::NotFound);
    }
    if !rows::gone(&mut conn, &[document]).await?.is_empty() {
        return Ok(Access::Gone);
    }
    let Some(principal) = principal_of(inner, &mut conn, user).await? else {
        return Ok(Access::Forbidden);
    };
    match documents::check(
        &mut conn,
        &principal,
        document,
        Permission::DocView,
        Policy::default(),
    )
    .await
    {
        Ok(Decision::Allowed { .. }) => Ok(Access::Allowed),
        Ok(Decision::Refused(_)) => Ok(Access::Forbidden),
        Err(orch_authz::Error::NotFound(_)) => Ok(Access::NotFound),
        // Held back by a suspension: not theirs to follow now.
        Err(orch_authz::Error::Suspended { .. } | orch_authz::Error::Archived { .. }) => {
            Ok(Access::Forbidden)
        }
        Err(error) => Err(error.into()),
    }
}

/// Where the document's feed stands, and how many rows lie above `since`.
pub(crate) async fn head(
    inner: &Inner,
    document: Id,
    since: Option<Seq>,
) -> Result<Option<Head>, Fail> {
    let mut conn = inner.db.acquire().await?;
    let Some(row) = rows::get(&mut conn, document)
        .await?
        .filter(|row| row.deleted_at.is_none())
    else {
        return Ok(None);
    };
    let rows_after = match since {
        Some(since) if since <= row.doc_seq && since >= row.purged_seq => {
            rows::count_blocks_changed(&mut conn, document, since).await?
                + discussions::count_changed(&mut conn, document, since).await?
        }
        _ => 0,
    };
    Ok(Some(Head {
        feed: FeedHead {
            seq: row.doc_seq,
            purged_seq: row.purged_seq,
            incremental_limit: DOCUMENT_SYNC_MAX_CHANGES,
        },
        rows_after,
    }))
}

/// Reads the rows in `(after, upto]`, as the document's own request returns them.
async fn load(inner: &Inner, document: Id, after: Seq, upto: Seq) -> Result<Shared, Fail> {
    let mut conn = inner.db.acquire().await?;
    let Some(row) = rows::get(&mut conn, document)
        .await?
        .filter(|row| row.deleted_at.is_none())
    else {
        return Ok(Shared::gone());
    };
    if !rows::gone(&mut conn, &[document]).await?.is_empty() {
        return Ok(Shared::gone());
    }
    // One more than a frame holds, to know that there is more.
    let blocks = rows::blocks_changed(
        &mut conn,
        document,
        after,
        upto,
        DOCUMENT_SYNC_MAX_CHANGES + 1,
    )
    .await?;
    if i64::try_from(blocks.len()).unwrap_or(i64::MAX) > DOCUMENT_SYNC_MAX_CHANGES {
        return Ok(Shared::too_many());
    }
    let discussions = discussions::changed_previews(&mut conn, document, after, upto, 51).await?;
    if discussions.len() > 50 {
        return Ok(Shared::too_many());
    }
    // What moved past the number named waits for the frame that follows.
    let own = ((!discussions.is_empty()) || (row.seq_updated > after && row.seq_updated <= upto))
        .then_some(row);
    let highest = blocks
        .iter()
        .map(|block| block.seq_updated)
        .chain(discussions.iter().map(|each| each.discussion.seq_updated))
        .chain(own.iter().map(|row| row.seq_updated))
        .max();
    Ok(Shared {
        gone: false,
        too_many: false,
        blocks,
        discussions,
        row: own,
        highest,
    })
}

fn some<T>(rows: Vec<T>) -> Option<Vec<T>> {
    if rows.is_empty() { None } else { Some(rows) }
}

/// The rows of the document stamped in `(after, upto]`, for one subscriber.
pub(crate) async fn read(
    inner: &Arc<Inner>,
    person: Id,
    document: Id,
    after: Seq,
    upto: Seq,
    cache: &mut Cache,
) -> Result<Rows, Fail> {
    let shared = match cache.documents.get(&(after, upto)) {
        Some(shared) => Arc::clone(shared),
        None => {
            let shared = Arc::new(load(inner, document, after, upto).await?);
            cache.documents.insert((after, upto), Arc::clone(&shared));
            shared
        }
    };
    if shared.gone {
        return Ok(Rows::Gone);
    }
    if shared.too_many {
        return Ok(Rows::TooMany);
    }
    let Some(highest) = shared.highest else {
        return Ok(Rows::Nothing);
    };
    let mut conn = inner.db.acquire().await?;
    let Some(principal) = principal_of(inner, &mut conn, person).await? else {
        return Ok(Rows::Nothing);
    };
    let policy = Policy::default();
    let blocks = build::blocks_for(&mut conn, &principal, &shared.blocks, policy)
        .await
        .map_err(Fail::other)?;
    let discussions =
        build::discussions_for(&mut conn, &principal, document, &shared.discussions, policy)
            .await
            .map_err(Fail::other)?;
    let reference = match &shared.row {
        Some(row) => {
            let listed = build::standing(&mut conn, &principal, document, policy)
                .await
                .map_err(Fail::other)?;
            Some(
                build::reference(
                    &mut conn,
                    &principal,
                    row,
                    &listed,
                    inner.clock.now(),
                    policy,
                )
                .await
                .map_err(Fail::other)?,
            )
        }
        None => None,
    };
    Ok(Rows::Frame(Box::new(Frame {
        data: SyncData::Doc(DocData {
            document: reference,
            blocks: some(blocks),
            discussions: some(discussions),
        }),
        highest,
        held: Vec::new(),
    })))
}

// ---------------------------------------------------------------- who is writing where

/// A `typing` frame of a connection on a document's feed: the person is
/// writing, in the block `at` if they said. Ignored for a feed the connection
/// does not follow, and for one that comes sooner than the protocol lets a
/// client send.
pub(crate) fn typing(
    inner: &Arc<Inner>,
    connection: &Arc<Connection>,
    feed: &Feed,
    at: Option<Id>,
) {
    let Feed::Doc(document) = feed else {
        return;
    };
    if !connection.follows(feed) || !connection.may_type(feed, inner.clock.time()) {
        return;
    }
    let (task, document, user) = (Arc::clone(inner), *document, connection.user);
    inner.tasks.spawn(async move {
        if let Err(error) = announce_typing(&task, document, user, at).await {
            tracing::warn!(%error, "who is writing could not be told");
        }
    });
}

/// Tells every process, through the database, that `user` is writing in the
/// document; nothing is stored. Whoever may not edit it is not writing.
async fn announce_typing(
    inner: &Inner,
    document: Id,
    user: Id,
    at: Option<Id>,
) -> Result<(), Fail> {
    let mut conn = inner.db.acquire().await?;
    let Some(principal) = principal_of(inner, &mut conn, user).await? else {
        return Ok(());
    };
    let may = documents::check(
        &mut conn,
        &principal,
        document,
        Permission::DocEdit,
        Policy::default(),
    )
    .await;
    if !matches!(may, Ok(Decision::Allowed { .. })) {
        return Ok(());
    }
    let block = at.map_or_else(|| NO_BLOCK.to_owned(), |block| block.to_string());
    let text = format!("{TYPING}:{document}:{user}:{block}");
    rows::announce(&mut conn, &text).await?;
    Ok(())
}

/// What the document channel announced: that a document moved up to a
/// sequence, or that a person is writing in it.
pub(crate) fn heard(inner: &Arc<Inner>, payload: &str) {
    if let Some(rest) = payload
        .strip_prefix(TYPING)
        .and_then(|rest| rest.strip_prefix(':'))
    {
        let mut parts = rest.split(':');
        let (Some(document), Some(user), Some(at)) = (
            parts.next().and_then(|part| part.parse::<Id>().ok()),
            parts.next().and_then(|part| part.parse::<Id>().ok()),
            parts.next(),
        ) else {
            return;
        };
        let at = match at {
            NO_BLOCK => None,
            block => match block.parse::<Id>() {
                Ok(block) => Some(block),
                Err(_) => return,
            },
        };
        let task = Arc::clone(inner);
        inner.tasks.spawn(async move {
            if let Err(error) = tell_typing(&task, document, user, at).await {
                tracing::warn!(%error, "who is writing could not be shown");
            }
        });
    } else if let Some((document, seq)) = sequences::parse_payload(payload) {
        inner.announce(Source::Document(document), seq);
    }
}

/// Shows the subscribers of a document, but the person themself, that `user` is writing.
async fn tell_typing(
    inner: &Arc<Inner>,
    document: Id,
    user: Id,
    at: Option<Id>,
) -> Result<(), Fail> {
    let subs: Vec<_> = inner
        .subscribers_of(Source::Document(document))
        .into_iter()
        .filter(|sub| sub.is_live() && shown_to(user, sub.connection().user))
        .collect();
    if subs.is_empty() {
        return Ok(());
    }
    let mut conn = inner.db.acquire().await?;
    let known = crate::middleware::authorization::refusals::user_refs(&mut conn, &[user])
        .await
        .map_err(Fail::other)?;
    let Some(person) = known.get(&user) else {
        return Ok(());
    };
    let notice = EventData::Typing(TypingNotice::Typing {
        user: person.clone(),
        at,
    });
    drop(conn);
    for sub in subs {
        let Some(_identity) = super::watches::reader_guard(inner, &sub).await? else {
            continue;
        };
        let frame = EventFrame::new(sub.tag().id, sub.feed().clone(), &notice);
        if let Ok(frame) = frame
            && let Ok(text) = ServerFrame::Event(frame).encode()
        {
            sub.connection()
                .outbox
                .push_event(sub.tag(), sub.feed(), text);
        }
    }
    Ok(())
}
