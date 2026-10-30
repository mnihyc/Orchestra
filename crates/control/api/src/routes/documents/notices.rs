//! Who is told that a document names them, and who is told of an answer in a discussion beside it.
//!
//! Naming a person sends no notice. Small reply audiences are delivered immediately; larger
//! audiences use the durable job runner with bounded pages, current View and restartable progress.

use crate::middleware::requests::{ApiError, ApiResult};
use chrono::{DateTime, Utc};
use orch_access_contract::permissions::Permission;
use orch_authz::{Caller, Policy, Principal, documents};
use orch_db::Conn;
use orch_db::queries::documents::{self as rows, DocumentRow};
use orch_engine::jobs::document_notices::{self, Plan};
use orch_types::Id;

const QUOTE_MAX_CHARS: usize = 140;
/// A larger audience is background work; ordinary small discussions retain immediate delivery.
const INLINE_MOST: usize = 20;

/// Where in the document something was said.
#[derive(Debug, Clone, Copy, Default)]
pub struct Where {
    /// The block.
    pub block: Option<Id>,
    /// The discussion.
    pub discussion: Option<Id>,
    /// The comment.
    pub comment: Option<Id>,
}
/// Who said it, and the words.
#[derive(Debug, Clone, Copy)]
pub struct Said<'a> {
    /// Who.
    pub by: Id,
    /// Their shown name.
    pub by_name: &'a str,
    /// The accepted words, read only far enough to quote them.
    pub text: &'a str,
}

fn quote(text: &str) -> String {
    let mut result = String::new();
    let mut count = 0;
    for word in text.split_whitespace() {
        if !result.is_empty() {
            if count == QUOTE_MAX_CHARS {
                result.push('…');
                return result;
            }
            result.push(' ');
            count += 1;
        }
        for ch in word.chars() {
            if count == QUOTE_MAX_CHARS {
                result.push('…');
                return result;
            }
            result.push(ch);
            count += 1;
        }
    }
    result
}

/// Tells the existing participants and explicit subscribers of an answer.
pub async fn answered(
    conn: &mut Conn,
    document: &DocumentRow,
    at: Where,
    said: Said<'_>,
    now: DateTime<Utc>,
) -> ApiResult<()> {
    publish(conn, document, at, said, false, Default::default(), now).await
}

/// A new discussion also names its current owners and the last writer of its block.
pub async fn started(
    conn: &mut Conn,
    document: &DocumentRow,
    at: Where,
    said: Said<'_>,
    now: DateTime<Utc>,
) -> ApiResult<()> {
    let principal = Principal::load(conn, Caller::session(said.by), orch_types::Time::new(now))
        .await
        .map_err(ApiError::internal)?;
    let (graph, _, site) = documents::site(conn, document.id)
        .await
        .map_err(ApiError::internal)?;
    let frame = orch_authz::engine::Frame::load(
        conn,
        &principal,
        Policy::default(),
        graph,
        std::slice::from_ref(&site),
        &[Permission::AccessOwn],
    )
    .await
    .map_err(ApiError::internal)?;
    let mut owners = orch_authz::holders::notification_sources(&frame, &site, &[]);
    if let Some(block) = at.block {
        if let Some(writer) = rows::people::block_writer(conn, document.id, block).await? {
            owners.kinds.push("user".to_owned());
            owners.ids.push(Some(writer));
            owners.minimum_roles.push(String::new());
            owners.owner_roots.push(None);
        }
    }
    publish(conn, document, at, said, true, owners, now).await
}

async fn publish(
    conn: &mut Conn,
    document: &DocumentRow,
    at: Where,
    said: Said<'_>,
    started: bool,
    owners: orch_db::queries::identity::MentionSources,
    now: DateTime<Utc>,
) -> ApiResult<()> {
    let Some(discussion) = at.discussion else {
        return Ok(());
    };
    let plan = Plan {
        document: document.id,
        discussion,
        block: at.block,
        comment: at.comment,
        by: said.by,
        by_name: said.by_name.to_owned(),
        quote: quote(said.text),
        started,
        sequence: rows::get(conn, document.id)
            .await?
            .ok_or_else(ApiError::not_found)?
            .doc_seq,
        subscriptions: rows::people::subscription_cutoff(conn).await?,
        at: now,
        owners,
    };
    let page = document_notices::audience(conn, &plan, None, now)
        .await
        .map_err(ApiError::internal)?;
    if page.more || page.people.len() > INLINE_MOST {
        document_notices::enqueue(conn, &plan)
            .await
            .map_err(ApiError::internal)?;
    } else {
        document_notices::deliver(conn, &plan, &page.people, now)
            .await
            .map_err(ApiError::internal)?;
    }
    Ok(())
}
