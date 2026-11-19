//! Current document readers for mentions, expanded by the common holder graph and checked by the ordinary document resolver.
//!
//! Holder definitions are expanded by one SQL keyset page. Only a bounded candidate batch is
//! checked by the authoritative document resolver; no request materializes the reader roster.

use super::*;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::{Choice, ObjectKind};
use orch_api_contract::{
    chats::ChatMentionListQuery,
    documents::{DocumentMentionCandidate, DocumentMentionList},
};
use orch_authz::{
    Caller,
    engine::{Frame, Graph},
    holders, pages,
};
use orch_db::queries::{access, identity};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// A request checks at most one maximum API page of identities, even with many denied candidates.
const SCAN_MOST: usize = orch_api_contract::common::MAX_LIMIT as usize;
/// Exact address lookup remains available as well as handles and display names.
const QUERY_CHARS: usize = 320;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Readers,
    Directory,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    phase: Phase,
    after: Option<Id>,
    snapshot: chrono::DateTime<chrono::Utc>,
    query: String,
}

pub(super) fn mount(registry: &mut Registry) {
    registry.on(DocumentMentionList, list);
}

async fn list(
    ctx: Ctx,
    query: ChatMentionListQuery,
    (): (),
) -> ApiResult<Page<DocumentMentionCandidate>> {
    let id = ctx.id("id")?;
    let (_, mut tx, principal) = crate::routes::nodes::action(&ctx).await?;
    let row = super::live(&mut tx, id).await?;
    authorization::require_document(
        &mut tx,
        &principal,
        id,
        Permission::DocView,
        Policy::default(),
    )
    .await?;
    let q = query
        .q
        .unwrap_or_default()
        .trim_start_matches('@')
        .to_lowercase();
    if q.chars().count() > QUERY_CHARS {
        return Err(invalid("q", "is longer than a person or address search"));
    }
    let mut cursor = match query.cursor.as_deref() {
        Some(written) => serde_json::from_str::<Cursor>(written)
            .map_err(|_| invalid("cursor", "is not a mention page cursor"))?,
        None => Cursor {
            phase: Phase::Readers,
            after: None,
            snapshot: ctx.now,
            query: q.clone(),
        },
    };
    if cursor.query != q || cursor.snapshot > ctx.now {
        return Err(invalid("cursor", "belongs to another mention search"));
    }
    let graph = Graph::load(&mut tx, &[row.node_id])
        .await
        .map_err(ApiError::internal)?;
    let site =
        orch_authz::documents::site_of(&graph, &orch_authz::documents::Document::of_row(&row))
            .ok_or_else(ApiError::not_found)?;
    let permissions = [
        Permission::DocView,
        Permission::DocEdit,
        Permission::DocManage,
    ];
    let frame = Frame::load(
        &mut tx,
        &principal,
        Policy::default(),
        graph,
        std::slice::from_ref(&site),
        &permissions,
    )
    .await
    .map_err(ApiError::internal)?;
    let sources = holders::notification_sources(&frame, &site, &permissions);
    let policy = crate::routes::nodes::policy_at(&mut tx, row.node_id).await?;
    let page = pages::read(
        &mut tx,
        &principal,
        pages::Target::Resource(ObjectRef {
            kind: ObjectKind::Document,
            id,
        }),
        policy,
    )
    .await
    .map_err(ApiError::internal)?;
    let can_share = page.gives.choices.contains(&Choice::FullView);
    let limit = usize::try_from(
        PageQuery {
            limit: query.limit,
            cursor: None,
        }
        .limit(),
    )
    .map_err(ApiError::internal)?;
    let mut selected = Vec::new();
    let mut scanned = 0;
    let mut more = true;
    while scanned < SCAN_MOST && selected.len() < limit {
        if cursor.phase == Phase::Directory && !can_share {
            more = false;
            break;
        }
        let take = (limit - selected.len() + 1).min(SCAN_MOST - scanned);
        let found = if cursor.phase == Phase::Readers {
            identity::mention_readers_page(
                &mut tx,
                &sources,
                cursor.after,
                &q,
                i64::try_from(take).map_err(ApiError::internal)?,
                principal.now().get(),
                cursor.snapshot,
            )
            .await?
        } else {
            identity::mention_directory_page(
                &mut tx,
                principal.user(),
                cursor.after,
                &q,
                i64::try_from(take).map_err(ApiError::internal)?,
                cursor.snapshot,
            )
            .await?
        };
        let ended = found.len() < take;
        let ids: Vec<_> = found.iter().map(|person| person.id).collect();
        let local: HashSet<_> = if cursor.phase == Phase::Directory && !page.external {
            access::local_holders_among(&mut tx, row.org_id, &ids)
                .await?
                .into_iter()
                .collect()
        } else {
            HashSet::new()
        };
        let found_len = found.len();
        let mut consumed = 0;
        for person in found {
            scanned += 1;
            consumed += 1;
            cursor.after = Some(person.id);
            if cursor.phase == Phase::Directory && !page.external && !local.contains(&person.id) {
                continue;
            }
            let can_view =
                match Principal::load(&mut tx, Caller::session(person.id), principal.now()).await {
                    Ok(reader) => matches!(
                        orch_authz::documents::check(
                            &mut tx,
                            &reader,
                            id,
                            Permission::DocView,
                            Policy::default()
                        )
                        .await,
                        Ok(orch_access_contract::decisions::Decision::Allowed { .. })
                    ),
                    Err(orch_authz::Error::NoSuchUser(_) | orch_authz::Error::Suspended { .. }) => {
                        false
                    }
                    Err(error) => return Err(ApiError::internal(error)),
                };
            if can_view == (cursor.phase == Phase::Readers) {
                selected.push((person.id, can_view));
            }
            if selected.len() == limit {
                break;
            }
        }
        if ended && consumed == found_len {
            if cursor.phase == Phase::Readers && can_share {
                cursor.phase = Phase::Directory;
                cursor.after = None;
            } else {
                more = false;
                break;
            }
        }
    }
    let ids: Vec<_> = selected.iter().map(|(id, _)| *id).collect();
    let names = refusals::user_refs(&mut tx, &ids).await?;
    let items = selected
        .into_iter()
        .filter_map(|(id, can_view)| {
            names
                .get(&id)
                .cloned()
                .map(|user| DocumentMentionCandidate { user, can_view })
        })
        .collect();
    let next_cursor = if more {
        Some(serde_json::to_string(&cursor).map_err(ApiError::internal)?)
    } else {
        None
    };
    tx.commit().await.map_err(DbError::from)?;
    Ok(Page { items, next_cursor })
}
