//! Event-driven, session-personalized current document metadata. No idle data sweep or shared rich cache.

use super::{
    Fail, Inner,
    subscriptions::{Access, SubState, Subscription, principal_for_connection, principal_of},
};
use crate::middleware::{
    authentication::{Authed, Caller, Via},
    requests::ApiError,
};
use crate::registry::Ctx;
use crate::routes::document_tree::metadata::{self, Projection};
use orch_api_contract::{
    common::ErrorCode,
    document_tree::{DocumentDirectory, DocumentTreeItem, DocumentTreeListQuery},
    routes::Endpoint,
};
use orch_authz::Principal;
use orch_db::Conn;
use orch_realtime_contract::{
    document_tree::{DocumentTreeData, DocumentTreeView},
    feeds::Feed,
    frames::FeedClosedReason,
};
use orch_types::{Hash, Id, Json};
use std::sync::Arc;

#[derive(Debug)]
pub(crate) struct State {
    root: Id,
    fingerprint: Hash,
}

fn identity(view: &DocumentTreeView) -> Id {
    match view {
        DocumentTreeView::Directory {
            at: DocumentDirectory::NodeDocs(id) | DocumentDirectory::Container(id),
            ..
        }
        | DocumentTreeView::Entry {
            item: DocumentTreeItem::Document(id) | DocumentTreeItem::Container(id),
            ..
        } => *id,
    }
}

async fn context(
    inner: &Inner,
    conn: &mut Conn,
    principal: &Principal,
    session: Id,
    view: &DocumentTreeView,
) -> Result<Option<Ctx>, Fail> {
    let Some(app) = inner.app() else {
        return Ok(None);
    };
    let Some(user) = orch_db::queries::identity::user_by_id(conn, principal.user()).await? else {
        return Ok(None);
    };
    let route = match view {
        DocumentTreeView::Directory { .. } => {
            orch_api_contract::document_tree::DocumentTreeList::ROUTE
        }
        DocumentTreeView::Entry { .. } => {
            orch_api_contract::document_tree::DocumentTreeEntryGet::ROUTE
        }
    };
    Ok(Some(Ctx::for_tool(
        app,
        Caller::User(Arc::new(Authed {
            user,
            as_admin: principal.caller().as_admin(),
            via: Via::Session {
                session_id: session,
            },
        })),
        route,
        identity(view),
    )))
}

fn denied(error: ApiError) -> Result<Access, Fail> {
    match error.problem.status {
        403 => Ok(Access::Forbidden),
        404 | 410 => Ok(Access::NotFound),
        _ => Err(Fail::other(error)),
    }
}

/// Subscription admission uses the real session principal; a cursor is not an authority token.
pub(crate) async fn decide(
    inner: &Inner,
    user: Id,
    view: &DocumentTreeView,
) -> Result<Access, Fail> {
    let mut conn = inner.db.acquire().await?;
    let Some(principal) = principal_of(inner, &mut conn, user).await? else {
        return Ok(Access::Forbidden);
    };
    let Some(ctx) = context(inner, &mut conn, &principal, Id::nil(), view).await? else {
        return Ok(Access::NotFound);
    };
    let result = match view {
        DocumentTreeView::Directory {
            at: DocumentDirectory::NodeDocs(node),
            ..
        } => {
            crate::middleware::authorization::require(
                &mut conn,
                &principal,
                *node,
                orch_access_contract::permissions::PermissionName::OfNode(
                    orch_access_contract::permissions::NodePermission::View,
                ),
                orch_authz::Act::Use,
                orch_authz::Policy::default(),
            )
            .await
        }
        DocumentTreeView::Directory {
            at: DocumentDirectory::Container(id),
            deleted,
            ..
        } => metadata::entry(
            &ctx,
            &mut conn,
            &principal,
            DocumentTreeItem::Container(*id),
            deleted.is_some(),
        )
        .await
        .map(|_| ()),
        DocumentTreeView::Entry {
            item,
            inspect_deleted,
        } => metadata::entry(&ctx, &mut conn, &principal, *item, *inspect_deleted)
            .await
            .map(|_| ()),
    };
    match result {
        Ok(()) => Ok(Access::Allowed),
        Err(error) if error.problem.status == 404 => {
            let typed = match view {
                DocumentTreeView::Directory {
                    at: DocumentDirectory::Container(id),
                    deleted,
                    ..
                } => Some(("container", *id, deleted.is_some())),
                DocumentTreeView::Entry {
                    item,
                    inspect_deleted,
                } => Some((
                    match item {
                        DocumentTreeItem::Document(_) => "document",
                        DocumentTreeItem::Container(_) => "container",
                    },
                    identity(view),
                    *inspect_deleted,
                )),
                _ => None,
            };
            if let Some((kind, id, retained)) = typed
                && let Some(row) =
                    orch_db::queries::documents::containers::entry(&mut conn, kind, id).await?
            {
                return Ok(if row.deleted_at.is_some() && !retained {
                    Access::Gone
                } else {
                    Access::Forbidden
                });
            }
            Ok(Access::NotFound)
        }
        Err(error) => denied(error),
    }
}

struct Current {
    data: DocumentTreeData,
    cut: metadata::Cut,
    inspections: Vec<crate::middleware::inspection::Info>,
}

async fn unchanged(
    inner: &Arc<Inner>,
    sub: &Subscription,
    cut: &metadata::Cut,
) -> Result<bool, Fail> {
    let mut conn = inner.db.acquire().await?;
    let Some(principal) = principal_for_connection(inner, &mut conn, sub.connection()).await?
    else {
        return Ok(false);
    };
    match metadata::unchanged(&mut conn, &principal, cut, inner.clock.time()).await {
        Ok(()) => Ok(true),
        Err(error) if error.code() == ErrorCode::ConflictStaleRevision => Ok(false),
        Err(error) => Err(Fail::other(error)),
    }
}

async fn read(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    view: &DocumentTreeView,
) -> Result<Current, ApiError> {
    match view {
        DocumentTreeView::Directory {
            at,
            deleted,
            cursor,
            limit,
        } => {
            let mut query = DocumentTreeListQuery {
                parent_container_id: match at {
                    DocumentDirectory::NodeDocs(_) => None,
                    DocumentDirectory::Container(id) => Some(*id),
                },
                deleted: *deleted,
                cursor: cursor.clone(),
                limit: *limit,
            };
            let first = metadata::page(ctx, conn, principal, None, *at, &query).await;
            let (
                Projection {
                    value,
                    cut,
                    inspections,
                },
                restart,
            ) = match first {
                Err(error)
                    if error.code() == ErrorCode::ConflictStaleRevision && cursor.is_some() =>
                {
                    query.cursor = None;
                    (
                        metadata::page(ctx, conn, principal, None, *at, &query).await?,
                        true,
                    )
                }
                other => (other?, false),
            };
            Ok(Current {
                data: DocumentTreeData::Directory {
                    directory: Box::new(value),
                    restart,
                },
                cut,
                inspections,
            })
        }
        DocumentTreeView::Entry {
            item,
            inspect_deleted,
        } => {
            let Projection {
                value,
                cut,
                inspections,
            } = metadata::entry(ctx, conn, principal, *item, *inspect_deleted).await?;
            Ok(Current {
                data: DocumentTreeData::Entry {
                    entry: Box::new(value),
                },
                cut,
                inspections,
            })
        }
    }
}

fn fingerprint(data: &Json) -> Result<Hash, Fail> {
    let mut semantic = data.clone();
    if let Some(directory) = semantic.get_mut("directory") {
        // AEAD nonces rotate even when candidate progress is identical. Their bytes are not a change.
        if let Some(cursor) = directory.get_mut("next_cursor")
            && !cursor.is_null()
        {
            *cursor = Json::Bool(true);
        }
        if let Some(feed) = directory.get_mut("feed") {
            *feed = Json::Null;
        }
    }
    if let Some(entry) = semantic.get_mut("entry")
        && let Some(feed) = entry.get_mut("feed")
    {
        *feed = Json::Null;
    }
    Ok(Hash::of(
        &serde_json::to_vec(&semantic).map_err(Fail::other)?,
    ))
}

/// Called with this subscription's state lock, so initial and later cuts cannot overtake one another.
pub(crate) async fn current(
    inner: &Arc<Inner>,
    sub: &Arc<Subscription>,
    st: &mut SubState,
) -> Result<(), Fail> {
    if !st.open {
        return Ok(());
    }
    let Feed::DocumentTree(view) = sub.feed() else {
        return Ok(());
    };
    let Some(_identity) = super::watches::delivery_guard(inner, sub.connection()).await? else {
        return Ok(());
    };
    if matches!(view, DocumentTreeView::Directory { .. }) {
        crate::routes::resources::sealing::master_key().map_err(Fail::other)?;
    }
    let mut tx = orch_db::queries::feeds::begin_reading(&inner.db).await?;
    let Some(principal) = principal_for_connection(inner, &mut tx, sub.connection()).await? else {
        return Ok(());
    };
    let Some(ctx) = context(inner, &mut tx, &principal, sub.connection().session(), view).await?
    else {
        return Ok(());
    };
    let loaded = read(&ctx, &mut tx, &principal, view).await;
    tx.commit().await.map_err(Fail::other)?;
    let loaded = match loaded {
        Ok(loaded) => loaded,
        Err(error) if matches!(error.problem.status, 403 | 404 | 410) => {
            let access =
                super::subscriptions::decide_for(inner, sub.connection(), sub.feed()).await?;
            if matches!(access, Access::Allowed | Access::Inspecting) {
                inner.ephemeral.document_tree_changed(None);
                return Ok(());
            }
            inner
                .close_sub(
                    sub,
                    st,
                    if access == Access::Forbidden {
                        FeedClosedReason::Revoked
                    } else {
                        FeedClosedReason::Deleted
                    },
                )
                .await;
            return Ok(());
        }
        Err(error) => return Err(Fail::other(error)),
    };
    let epoch = super::sync::read_epoch(inner).await?;
    if !inner.recheck(sub, st, epoch).await? {
        return Ok(());
    }
    let mut conn = inner.db.acquire().await?;
    let Some(current_principal) =
        principal_for_connection(inner, &mut conn, sub.connection()).await?
    else {
        return Ok(());
    };
    drop(conn);
    if !unchanged(inner, sub, &loaded.cut).await? {
        inner.ephemeral.document_tree_changed(Some(loaded.cut.root));
        return Ok(());
    }
    let data = serde_json::to_value(&loaded.data).map_err(Fail::other)?;
    let hash = fingerprint(&data)?;
    if st
        .document_tree
        .as_ref()
        .is_some_and(|state| state.root == loaded.cut.root && state.fingerprint == hash)
    {
        return Ok(());
    }
    if !loaded.inspections.is_empty() {
        let mut tx = inner.db.begin().await?;
        for info in &loaded.inspections {
            crate::middleware::inspection::record(
                &ctx,
                &mut tx,
                &current_principal,
                info,
                inner.clock.time(),
            )
            .await
            .map_err(Fail::other)?;
        }
        tx.commit().await.map_err(Fail::other)?;
    }
    // A cut never advances while a credential, authority or namespace generation changed behind it.
    let Some(_identity) = super::watches::delivery_guard(inner, sub.connection()).await? else {
        return Ok(());
    };
    let epoch = super::sync::read_epoch(inner).await?;
    if epoch != loaded.cut.epoch
        || !inner.recheck(sub, st, epoch).await?
        || !unchanged(inner, sub, &loaded.cut).await?
    {
        inner.ephemeral.document_tree_changed(Some(loaded.cut.root));
        return Ok(());
    }
    super::ephemeral::send(sub, &data)?;
    st.document_tree = Some(State {
        root: loaded.cut.root,
        fingerprint: hash,
    });
    Ok(())
}

/// Existing transactional tree/authority/reconnect notices are the only refresh triggers.
pub(crate) async fn refresh(inner: &Arc<Inner>, root: Option<Id>) -> Result<(), Fail> {
    for sub in inner.ephemeral.document_tree_subscribers() {
        let mut st = sub.state.lock().await;
        if root.is_some_and(|root| {
            st.document_tree
                .as_ref()
                .is_some_and(|state| state.root != root)
        }) {
            continue;
        }
        current(inner, &sub, &mut st).await?;
    }
    Ok(())
}
