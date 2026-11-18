//! Event-driven, session-personalized current document metadata. No idle data sweep or shared rich cache.

use super::Fail;

use super::Inner;

use super::subscriptions::Access;

use super::subscriptions::SubState;

use super::subscriptions::Subscription;

use super::subscriptions::principal_for_connection;

use crate::middleware::authentication::Caller;

use crate::middleware::requests::ApiError;

use crate::registry::Ctx;

use crate::routes::document_tree::metadata;

use crate::routes::document_tree::metadata::Projection;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::document_tree::DocumentDirectory;

use orch_api_contract::document_tree::DocumentTreeItem;

use orch_api_contract::document_tree::DocumentTreeListQuery;

use orch_api_contract::routes::Endpoint;

use orch_authz::Principal;

use orch_db::Conn;

use orch_realtime_contract::document_tree::DocumentTreeData;

use base64::Engine as _;
use orch_api_contract::nodes::OrgTreeGetQuery;
use orch_realtime_contract::document_tree::DocumentTreeView;
use orch_realtime_contract::document_tree::HierarchyWindow;
use std::collections::BTreeMap;

use orch_realtime_contract::feeds::Feed;

use orch_realtime_contract::frames::FeedClosedReason;

use orch_types::Hash;

use orch_types::Id;

use orch_types::Json;

use std::sync::Arc;
#[derive(Debug)]
pub struct State {
    root: Id,
    fingerprints: std::collections::HashMap<String, Hash>,
}

/// Connection-owned selectors are separate from rich projected state, which is discarded on reauthentication.
#[derive(Debug, Default)]
pub(crate) struct HierarchyWindows {
    queries: BTreeMap<u64, (Id, OrgTreeGetQuery)>,
    pending: Option<(u64, Id, Vec<u8>)>,
}

impl HierarchyWindows {
    fn receive(
        &mut self,
        change: HierarchyWindow,
    ) -> Result<Option<(u64, Id, OrgTreeGetQuery)>, ()> {
        let window = change.window();
        match change {
            HierarchyWindow::Remove { .. } => {
                self.queries.remove(&window);
                if self
                    .pending
                    .as_ref()
                    .is_some_and(|(id, _, _)| *id == window)
                {
                    self.pending = None;
                }
                Ok(None)
            }
            HierarchyWindow::Query {
                root,
                data,
                complete,
                ..
            } => {
                let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .decode(data)
                    .map_err(|_| ())?;
                self.pending
                    .get_or_insert_with(|| (window, root, Vec::new()));
                if self
                    .pending
                    .as_ref()
                    .is_some_and(|(id, at, _)| *id != window || *at != root)
                {
                    self.pending = Some((window, root, Vec::new()));
                }
                let (_, _, bytes) = self.pending.as_mut().ok_or(())?;
                bytes.extend(decoded);
                if !complete {
                    return Ok(None);
                }
                let (_, _, bytes) = self.pending.take().ok_or(())?;
                let query: OrgTreeGetQuery = serde_json::from_slice(&bytes).map_err(|_| ())?;
                if !orch_realtime_contract::document_tree::bounded_hierarchy_query(&query) {
                    return Err(());
                }
                self.queries.insert(window, (root, query.clone()));
                Ok(Some((window, root, query)))
            }
        }
    }
}

/// Page registration neither replaces the root subscription nor consumes another live slot.
pub(crate) async fn update_window(
    inner: &Arc<Inner>,
    sub: &Arc<Subscription>,
    st: &mut SubState,
    change: HierarchyWindow,
) -> Result<(), Fail> {
    let window = change.window();
    if let Some(state) = st.document_tree.as_mut() {
        state.fingerprints.remove(&window.to_string());
    }
    match st.hierarchy.receive(change) {
        Ok(Some(page)) => {
            let Feed::DocumentTree(view) = sub.feed() else {
                return Ok(());
            };
            current_view(inner, sub, st, view, Some(page)).await
        }
        Ok(None) => Ok(()),
        Err(()) => {
            st.hierarchy.pending = None;
            st.hierarchy.queries.remove(&window);
            send_problem(inner, sub, st, window, ErrorCode::RequestInvalid).await
        }
    }
}

async fn send_problem(
    inner: &Arc<Inner>,
    sub: &Arc<Subscription>,
    st: &mut SubState,
    window: u64,
    code: ErrorCode,
) -> Result<(), Fail> {
    let Some(_identity) = super::watches::delivery_guard(inner, sub.connection()).await? else {
        return Ok(());
    };
    let epoch = super::sync::read_epoch(inner).await?;
    if !inner.recheck(sub, st, epoch).await? {
        return Ok(());
    }
    let data = serde_json::to_value(DocumentTreeData::Hierarchy {
        window,
        problem: Some(code),
        hierarchy: Box::new(orch_api_contract::nodes::OrgTreeGetReply {
            ordering: None,
            page_size: None,
            feed: None,
            ancestors: Vec::new(),
            settings: None,
            seq: 0,
            since: None,
            complete: true,
            next_cursor: None,
            nodes: Vec::new(),
        }),
    })
    .map_err(Fail::other)?;
    super::ephemeral::send(sub, &data)
}

fn identity(view: &DocumentTreeView) -> Option<Id> {
    match view {
        DocumentTreeView::Hierarchy { .. } => None,
        DocumentTreeView::Directory {
            at: DocumentDirectory::NodeDocs(id) | DocumentDirectory::Container(id),
            ..
        }
        | DocumentTreeView::Entry {
            item: DocumentTreeItem::Document(id) | DocumentTreeItem::Container(id),
            ..
        } => Some(*id),
    }
}

async fn context(
    inner: &Inner,
    principal: &Principal,
    authed: crate::middleware::authentication::Authed,
    view: &DocumentTreeView,
    root: Option<Id>,
) -> Result<Option<Ctx>, Fail> {
    let Some(app) = inner.app() else {
        return Ok(None);
    };
    let route = match view {
        DocumentTreeView::Hierarchy { .. } => orch_api_contract::nodes::OrgTreeGet::ROUTE,
        DocumentTreeView::Directory { .. } => {
            orch_api_contract::document_tree::DocumentTreeList::ROUTE
        }
        DocumentTreeView::Entry { .. } => {
            orch_api_contract::document_tree::DocumentTreeEntryGet::ROUTE
        }
    };
    let Some(at) = root.or_else(|| identity(view)) else {
        return Ok(None);
    };
    Ok(Some(
        Ctx::for_tool(app, Caller::User(Arc::new(authed)), route, at)
            .with_admitted_caller(principal.caller().clone())
            .map_err(Fail::other)?,
    ))
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
    let Some((principal, authed)) = super::subscriptions::reader_of(inner, &mut conn, user).await?
    else {
        return Ok(Access::Forbidden);
    };
    if matches!(view, DocumentTreeView::Hierarchy { .. }) {
        return Ok(Access::Allowed);
    }
    let Some(ctx) = context(inner, &principal, authed, view, None).await? else {
        return Ok(Access::NotFound);
    };
    let result = match view {
        DocumentTreeView::Hierarchy { .. } => return Ok(Access::Allowed),
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
                    match item {
                        DocumentTreeItem::Document(id) | DocumentTreeItem::Container(id) => *id,
                    },
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
    page: Option<&(u64, Id, OrgTreeGetQuery)>,
) -> Result<Current, ApiError> {
    match view {
        DocumentTreeView::Hierarchy { .. } => {
            let (window, root, query) =
                page.ok_or_else(|| ApiError::internal("a hierarchy view needs its page"))?;
            let cut = metadata::cut(conn, principal, *root, ctx.now.into()).await?;
            let (hierarchy, problem) =
                match crate::routes::nodes::tree::hierarchy_metadata(ctx, conn, principal, query)
                    .await
                {
                    Ok(value) => (value, None),
                    Err(error) => {
                        let seq = 0;
                        (
                            orch_api_contract::nodes::OrgTreeGetReply {
                                ordering: None,
                                page_size: None,
                                feed: None,
                                ancestors: Vec::new(),
                                settings: None,
                                seq,
                                since: None,
                                complete: true,
                                next_cursor: None,
                                nodes: Vec::new(),
                            },
                            Some(error.code()),
                        )
                    }
                };
            Ok(Current {
                data: DocumentTreeData::Hierarchy {
                    window: *window,
                    problem,
                    hierarchy: Box::new(hierarchy),
                },
                cut,
                inspections: Vec::new(),
            })
        }

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
    if let Some(hierarchy) = semantic.get_mut("hierarchy").and_then(Json::as_object_mut) {
        hierarchy.remove("feed");
        hierarchy.remove("seq");
        if let Some(cursor) = hierarchy.get_mut("next_cursor")
            && let Some(text) = cursor.as_str()
        {
            let mut parts = text.split('|').map(str::to_owned).collect::<Vec<_>>();
            if parts.len() == 5 {
                parts[2] = "current".to_owned();
                *cursor = Json::String(parts.join("|"));
            }
        }
    }
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
    current_for_root(inner, sub, st, None).await
}

async fn current_for_root(
    inner: &Arc<Inner>,
    sub: &Arc<Subscription>,
    st: &mut SubState,
    changed_root: Option<Id>,
) -> Result<(), Fail> {
    if !st.open {
        return Ok(());
    }
    let Feed::DocumentTree(view) = sub.feed() else {
        return Ok(());
    };
    if matches!(view, DocumentTreeView::Hierarchy { .. }) {
        for (window, (root, query)) in st.hierarchy.queries.clone() {
            if !st.open {
                break;
            }
            if changed_root.is_none_or(|at| at == root) {
                current_view(inner, sub, st, view, Some((window, root, query))).await?;
            }
        }
    } else {
        current_view(inner, sub, st, view, None).await?;
    }
    Ok(())
}

async fn current_view(
    inner: &Arc<Inner>,
    sub: &Arc<Subscription>,
    st: &mut SubState,
    view: &DocumentTreeView,
    page: Option<(u64, Id, OrgTreeGetQuery)>,
) -> Result<(), Fail> {
    let key = page
        .as_ref()
        .map_or_else(String::new, |(window, _, _)| window.to_string());
    let Some(_identity) = super::watches::delivery_guard(inner, sub.connection()).await? else {
        return Ok(());
    };
    if matches!(view, DocumentTreeView::Directory { .. }) {
        crate::routes::resources::sealing::master_key().map_err(Fail::other)?;
    }
    let mut tx = orch_db::queries::feeds::begin_reading(&inner.db).await?;
    let Some((principal, authed)) =
        super::subscriptions::reader_for_connection(inner, &mut tx, sub.connection()).await?
    else {
        return Ok(());
    };
    let Some(ctx) = context(
        inner,
        &principal,
        authed,
        view,
        page.as_ref().map(|(_, root, _)| *root),
    )
    .await?
    else {
        return Ok(());
    };
    let loaded = read(&ctx, &mut tx, &principal, view, page.as_ref()).await;
    tx.commit().await.map_err(Fail::other)?;
    let loaded = match loaded {
        Ok(loaded) => loaded,
        Err(error) if page.is_some() => {
            if let Some((window, _, _)) = page {
                return send_problem(inner, sub, st, window, error.code()).await;
            }
            return Ok(());
        }
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
        .is_some_and(|state| state.fingerprints.get(&key) == Some(&hash))
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
    let mut state = st.document_tree.take().unwrap_or_else(|| State {
        root: loaded.cut.root,
        fingerprints: std::collections::HashMap::new(),
    });
    state.fingerprints.insert(key, hash);
    st.document_tree = Some(state);
    Ok(())
}

/// Existing transactional tree/authority/reconnect notices are the only refresh triggers.
pub(crate) async fn refresh(inner: &Arc<Inner>, root: Option<Id>) -> Result<(), Fail> {
    for sub in inner.ephemeral.document_tree_subscribers() {
        let mut st = sub.state.lock().await;
        let hierarchy = matches!(
            sub.feed(),
            Feed::DocumentTree(DocumentTreeView::Hierarchy { .. })
        );
        if !hierarchy
            && root.is_some_and(|root| {
                st.document_tree
                    .as_ref()
                    .is_some_and(|state| state.root != root)
            })
        {
            continue;
        }
        current_for_root(inner, &sub, &mut st, root).await?;
    }
    Ok(())
}

/// Personal state wakes are coalesced and never poll idle document directories or other people's trees.
pub(crate) async fn refresh_reader(inner: &Arc<Inner>, user: Id) -> Result<(), Fail> {
    for sub in inner.ephemeral.document_tree_subscribers() {
        if sub.connection().user != user
            || !matches!(
                sub.feed(),
                Feed::DocumentTree(DocumentTreeView::Hierarchy { .. })
            )
        {
            continue;
        }
        let mut state = sub.state.lock().await;
        current(inner, &sub, &mut state).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_realtime_contract::frames::ClientFrame;

    #[test]
    fn a_long_native_selector_keeps_its_other_windows_and_every_fragment_inside_the_socket_boundary()
     {
        let root = Id::from_u128(1);
        let feed = Feed::DocumentTree(DocumentTreeView::Hierarchy {}).to_string();
        let mut windows = HierarchyWindows::default();
        let first = serde_json::json!({"parent_id":root});
        assert!(
            windows
                .receive(HierarchyWindow::Query {
                    root,
                    window: 1,
                    data: base64::engine::general_purpose::URL_SAFE_NO_PAD
                        .encode(serde_json::to_vec(&first).unwrap()),
                    complete: true,
                })
                .unwrap()
                .is_some()
        );
        let query = serde_json::json!({"parent_id":root,"q":"界".repeat(orch_api_contract::common::MAX_JSON_BYTES/3+1)});
        let bytes = serde_json::to_vec(&query).unwrap();
        assert!(bytes.len() > orch_api_contract::common::MAX_JSON_BYTES);
        let size = orch_realtime_contract::version::MAX_CLIENT_FRAME_BYTES / 2;
        for (index, part) in bytes.chunks(size).enumerate() {
            let change = HierarchyWindow::Query {
                root,
                window: 2,
                data: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(part),
                complete: (index + 1) * size >= bytes.len(),
            };
            let frame = serde_json::json!({"op":"sub","id":1,"feed":feed,"window":change});
            assert!(ClientFrame::decode(&frame.to_string()).is_ok());
            assert!(windows.receive(change).is_ok());
        }
        assert_eq!(windows.queries.len(), 2);
        assert_eq!(
            windows
                .queries
                .get(&2)
                .and_then(|(_, query)| query.q.as_deref()),
            query["q"].as_str()
        );
        assert!(windows.pending.is_none());
    }
}
