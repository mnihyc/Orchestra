//! Explicit retained-content reads reuse restoration guards and never mark anything read or notify anyone.

use super::authorization;

use super::requests::ApiError;

use super::requests::ApiResult;

use super::transaction_guard;

use crate::registry::Ctx;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::nodes::DeletedInspection;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_db::Conn;

use orch_db::DbError;

use orch_db::queries::documents;

use orch_db::queries::hierarchy;

use orch_db::queries::trace;

use orch_types::Id;

use orch_types::Time;

use serde_json::Value;

use serde_json::json;
/// The one retained thing admitted by the current stored-content reader.
#[derive(Debug, Clone)]
pub struct Info {
    /// Actual object kind, shared by all its readers for daily deduplication.
    pub kind: String,
    /// That thing's identity.
    pub id: Id,
    /// Current deletion identity, where known.
    pub deletion: Option<Id>,
    /// The read-only banner.
    pub banner: DeletedInspection,
    /// Exact stored objects needed by the existing renderer.
    pub reads: Vec<Id>,
}

/// The successful native snapshot's exact retained association, shared with its reply.
/// Failed snapshot attempts never publish a retained scope or an audit obligation.
#[derive(Debug, Default)]
pub struct Snapshot(std::sync::Mutex<Option<Option<Info>>>);

fn same(old: Option<&Info>, fresh: Option<&Info>) -> bool {
    match (old, fresh) {
        (None, None) => true,
        (Some(old), Some(fresh)) => {
            old.kind == fresh.kind
                && old.id == fresh.id
                && old.deletion == fresh.deletion
                && old.reads.iter().collect::<std::collections::BTreeSet<_>>()
                    == fresh.reads.iter().collect()
        }
        _ => false,
    }
}

impl Snapshot {
    pub fn get(&self) -> ApiResult<Option<Option<Info>>> {
        self.0
            .lock()
            .map(|held| held.clone())
            .map_err(|_| ApiError::internal("inspection snapshot state is poisoned"))
    }
    pub fn check(&self, supplied: Option<&Info>, fresh: Option<&Info>) -> ApiResult<()> {
        if supplied.is_some() && !same(supplied, fresh)
            || self.get()?.is_some_and(|held| !same(held.as_ref(), fresh))
        {
            return Err(ApiError::new(
                ErrorCode::ConflictStaleRevision,
                "the retained inspection changed",
            ));
        }
        Ok(())
    }
    pub fn publish(&self, info: Option<Info>) -> ApiResult<()> {
        let mut held = self
            .0
            .lock()
            .map_err(|_| ApiError::internal("inspection snapshot state is poisoned"))?;
        if held
            .as_ref()
            .is_some_and(|old| !same(old.as_ref(), info.as_ref()))
        {
            return Err(ApiError::new(
                ErrorCode::ConflictStaleRevision,
                "the retained inspection changed",
            ));
        }
        if held.is_none() {
            *held = Some(info);
        }
        Ok(())
    }
}

/// Rejects the inspection flag everywhere except the listed stored-content GET readers.
pub(crate) fn requested(
    route: &orch_api_contract::routes::Route,
    query: Option<&str>,
) -> ApiResult<bool> {
    let values: Vec<_> = form_urlencoded::parse(query.unwrap_or_default().as_bytes())
        .filter(|(key, _)| key == "inspect_deleted")
        .map(|(_, v)| v.into_owned())
        .collect();
    if values.is_empty() {
        return Ok(false);
    }
    let allowed = matches!(
        route.name,
        "NodeGet"
            | "NodeChildList"
            | "ThreadSyncGet"
            | "ThreadItemList"
            | "ThreadItemGet"
            | "ThreadItemOutputGet"
            | "ThreadOutlineGet"
            | "ThreadSegmentList"
            | "ThreadTurnList"
            | "ThreadCatchupGet"
            | "ThreadBlobGet"
            | "DocumentGet"
            | "DocumentContainerGet"
            | "DocumentTreeEntryGet"
            | "DocumentSyncGet"
            | "DocumentVersionList"
            | "DocumentVersionGet"
            | "DocumentChangesGet"
            | "DocumentDiscussionList"
            | "DocumentCommentList"
            | "DocumentFileGet"
            | "DocumentFileArchiveList"
            | "DocumentFileArchiveEntryGet"
            | "EnvironmentGet"
    );
    if route.method.as_str() != "GET" || !allowed {
        return Err(ApiError::invalid(
            "inspect_deleted",
            "this reader does not inspect deleted content",
        ));
    }
    match values.as_slice() {
        [value] if value == "true" => Ok(true),
        [value] if value == "false" => Ok(false),
        _ => Err(ApiError::invalid(
            "inspect_deleted",
            "must be given once as true or false",
        )),
    }
}

/// Typed entry readers choose their native kind before ordinary inspection preparation.
pub(crate) async fn prepare_query(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    query: Option<&str>,
) -> ApiResult<Option<Info>> {
    if ctx.inspect_deleted && ctx.route.name == "DocumentTreeEntryGet" {
        let kinds: Vec<_> = form_urlencoded::parse(query.unwrap_or_default().as_bytes())
            .filter(|(key, _)| key == "kind")
            .map(|(_, value)| value.into_owned())
            .collect();
        let item = match kinds.as_slice() {
            [kind] if kind == "document" => {
                orch_engine::document_tree::Item::Document(ctx.id("id")?)
            }
            [kind] if kind == "container" => {
                orch_engine::document_tree::Item::Container(ctx.id("id")?)
            }
            _ => {
                return Err(ApiError::invalid(
                    "kind",
                    "must be given once as document or container",
                ));
            }
        };
        return document_entry(ctx, conn, principal, item).await;
    }
    prepare(ctx, conn, principal).await
}

/// Metadata inspection uses the current native restoration authority and recorded promise.
pub(crate) async fn document_entry(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    item: orch_engine::document_tree::Item,
) -> ApiResult<Option<Info>> {
    use orch_engine::document_tree as domain;
    let kind = match item {
        domain::Item::Document(_) => "document",
        domain::Item::Container(_) => "container",
    };
    let row = documents::containers::entry(conn, kind, item.id())
        .await?
        .ok_or_else(ApiError::not_found)?;
    let Some(deleted) = row.deleted_at else {
        return Ok(None);
    };
    let mut reads = vec![row.id, row.node_id];
    reads.extend(
        documents::containers::ancestors_many(conn, &[row.id])
            .await?
            .into_iter()
            .filter(|parent| parent.deleted_at.is_some())
            .map(|parent| parent.id),
    );
    let admitted = trace::read_scope(
        &reads,
        domain::metadata_in(
            conn,
            domain::Actor::Person(principal),
            item,
            ctx.app.clock.time(),
            true,
        ),
    )
    .await
    .map_err(|error| match error {
        domain::Error::NotFound | domain::Error::Denied => ApiError::not_found(),
        other => ApiError::internal(other),
    })?;
    let entry = admitted
        .entries
        .iter()
        .find(|entry| entry.entry.item == item && entry.standing.manage)
        .ok_or_else(ApiError::not_found)?;
    let canonical = match item {
        domain::Item::Document(_) => "document",
        domain::Item::Container(_) => "document_container",
    };
    let life = trace::life(conn, canonical, row.id).await?;
    if life.as_ref().is_some_and(|life| life.purged_at.is_some()) {
        return Err(ApiError::not_found());
    }
    let until = domain::restore_deadline_in(
        conn,
        &entry.entry,
        crate::routes::document_tree::options(ctx),
    )
    .await
    .map_err(ApiError::internal)?
    .ok_or_else(ApiError::not_found)?;
    let by = match item {
        domain::Item::Document(id) => documents::get(conn, id)
            .await?
            .and_then(|row| row.deleted_by),
        domain::Item::Container(id) => documents::containers::get(conn, id)
            .await?
            .and_then(|row| row.deleted_by),
    };
    let person = match by {
        Some(id) => authorization::refusals::user_refs(conn, &[id])
            .await?
            .remove(&id),
        None => None,
    };
    Ok(Some(Info {
        kind: canonical.to_owned(),
        id: row.id,
        deletion: row.deleted_in,
        reads,
        banner: DeletedInspection {
            deleted_at: Time::new(deleted),
            deleted_by: person,
            purge_at: Time::new(until),
            readonly: true,
        },
    }))
}

/// Checks the requested thing's own restoration authority, separately from audit access.
pub async fn prepare(ctx: &Ctx, conn: &mut Conn, principal: &Principal) -> ApiResult<Option<Info>> {
    if !ctx.inspect_deleted {
        return Ok(None);
    }
    if ctx.route.name == "DocumentContainerGet" {
        return document_entry(
            ctx,
            conn,
            principal,
            orch_engine::document_tree::Item::Container(ctx.id("id")?),
        )
        .await;
    }
    if ctx.route.name == "DocumentTreeEntryGet" {
        let item = match ctx.inspection.as_ref().map(|info| info.kind.as_str()) {
            Some("document") => orch_engine::document_tree::Item::Document(ctx.id("id")?),
            Some("document_container") => {
                orch_engine::document_tree::Item::Container(ctx.id("id")?)
            }
            _ => return Ok(None),
        };
        return document_entry(ctx, conn, principal, item).await;
    }
    let id = ctx.id("id")?;
    let plural = ctx
        .route
        .path
        .trim_start_matches('/')
        .split('/')
        .next()
        .unwrap_or_default();
    if plural == "environments" {
        return Ok(None);
    }
    let (kind, deleted, by, reads, home) = if plural == "documents" {
        let Some(row) = documents::get(conn, id).await? else {
            if principal.is_system_admin()
                && trace::life(conn, "document", id)
                    .await?
                    .is_some_and(|l| l.purged_at.is_some())
            {
                return Err(ApiError::new(ErrorCode::Gone, "this document was purged"));
            }
            return Err(ApiError::not_found());
        };
        let nodes = hierarchy::details(conn, &[row.node_id], ctx.now).await?;
        let parent = nodes.first();
        (
            "document".to_owned(),
            row.deleted_at.or_else(|| parent.and_then(|n| n.deleted_at)),
            row.deleted_by.or_else(|| parent.and_then(|n| n.deleted_by)),
            vec![id, row.node_id],
            Some(row.node_id),
        )
    } else {
        let mut rows = hierarchy::details(conn, &[id], ctx.now).await?;
        let Some(row) = rows.pop() else {
            let life = trace::life(conn, "node", id).await?;
            if life.as_ref().is_some_and(|l| l.purged_at.is_some()) && principal.is_system_admin() {
                return Err(ApiError::new(ErrorCode::Gone, "this thing was purged"));
            }
            return Err(ApiError::not_found());
        };
        (
            row.kind,
            row.deleted_at,
            row.deleted_by,
            vec![id],
            row.parent_id,
        )
    };
    let Some(deleted) = deleted else {
        return Ok(None);
    };
    let authority_reads: Vec<Id> = home.into_iter().chain(reads.iter().copied()).collect();
    if !trace::read_scope(
        &authority_reads,
        crate::routes::trace::restore_allowed(conn, principal, &kind, id),
    )
    .await
    {
        return Err(ApiError::new(
            ErrorCode::AuthzDenied,
            "inspecting this deleted thing requires its restoration authority",
        )
        .with(
            "required",
            if kind == "document" {
                "doc.manage"
            } else {
                "node.manage"
            },
        ));
    }
    let canonical = if kind == "document" {
        "document"
    } else {
        "node"
    };
    let life = trace::life(conn, canonical, id).await?;
    if life.as_ref().is_some_and(|l| l.purged_at.is_some()) {
        return Err(ApiError::new(ErrorCode::Gone, "this thing was purged"));
    }
    let days =
        i64::try_from(ctx.app.config.retention.soft_delete_days).map_err(ApiError::internal)?;
    let until = life
        .as_ref()
        .and_then(|l| l.purge_at)
        .unwrap_or(deleted + chrono::Duration::days(days));
    let person = match by {
        Some(id) => authorization::refusals::user_refs(conn, &[id])
            .await?
            .remove(&id),
        None => None,
    };
    Ok(Some(Info {
        kind,
        id,
        deletion: life.as_ref().and_then(|l| l.deletion_id),
        banner: DeletedInspection {
            deleted_at: Time::new(deleted),
            deleted_by: person,
            purge_at: Time::new(until),
            readonly: true,
        },
        reads,
    }))
}

/// Rechecks current authority and records a successful extraordinary read once per UTC day.
pub(crate) async fn finish(ctx: &Ctx) -> ApiResult<()> {
    let Some(info) = &ctx.inspection else {
        return Ok(());
    };
    let (mut tx, guard) = transaction_guard::begin(ctx).await?;
    let current = prepare(ctx, &mut tx, guard.principal()).await?;
    if current
        .as_ref()
        .is_some_and(|current| current.deletion != info.deletion)
    {
        return Err(ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "the deletion changed during inspection",
        ));
    }
    if current.is_some() {
        record(ctx, &mut tx, guard.principal(), info, guard.now()).await?;
    }
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

/// The same extraordinary-read audit for explicitly admitted metadata pages and live views.
pub async fn record(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    info: &Info,
    now: Time,
) -> ApiResult<()> {
    let mut entry = orch_db::audit::Entry::new(
        orch_db::audit::Actor::User(principal.user()),
        "deleted.inspected",
        &info.kind,
        Some(info.id),
        "A retained deleted thing was inspected",
    );
    entry.node_id = if matches!(info.kind.as_str(), "document" | "document_container") {
        info.reads.get(1).copied()
    } else {
        Some(info.id)
    };
    orch_db::audit::append(conn, &entry, &crate::sign_in::origin(ctx), now.get()).await?;
    Ok(())
}

/// Adds the banner to existing object replies without changing array or byte payloads.
pub(crate) fn banner(value: &mut Value, info: Option<&Info>) {
    let Some(info) = info else {
        return;
    };
    let target = if value.get("thread").is_some_and(Value::is_object) {
        value.get_mut("thread")
    } else if value.get("document").is_some_and(Value::is_object) {
        value.get_mut("document")
    } else if value.get("owner").is_some() {
        Some(value)
    } else {
        None
    };
    if let Some(target) = target.filter(|target| target.is_object()) {
        target["inspection"] = json!(info.banner);
    }
}

/// Carries deletion facts for array, page and attachment readers without changing their payload shape.
pub(crate) fn headers(response: &mut axum::response::Response, ctx: &Ctx) -> ApiResult<()> {
    if ctx.inspect_deleted {
        response.headers_mut().insert(
            http::header::CACHE_CONTROL,
            http::HeaderValue::from_static("no-store"),
        );
    }
    if let Some(info) = &ctx.inspection {
        let bytes = serde_json::to_vec(&info.banner).map_err(ApiError::internal)?;
        let value = http::HeaderValue::from_bytes(&bytes).map_err(ApiError::internal)?;
        response.headers_mut().insert("orch-inspection", value);
    }
    Ok(())
}

/// Applies each child's own restore guard before paging an inspected place.
pub async fn children(
    ctx: Ctx,
    query: orch_api_contract::nodes::NodeChildListQuery,
) -> ApiResult<orch_api_contract::common::Page<orch_api_contract::nodes::Node>> {
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let (page, inspections) = children_in(&ctx, &query, &mut conn, &principal, None).await?;
    drop(conn);
    for info in inspections {
        let mut child = ctx.for_id(info.id);
        child.inspection = Some(info);
        finish(&child).await?;
    }
    Ok(orch_api_contract::common::Page {
        next_cursor: page.page.next_cursor,
        items: page
            .page
            .items
            .into_iter()
            .filter_map(orch_api_contract::common::Listed::into_visible)
            .collect(),
    })
}

/// The same native retained-child producer without audit effects during preparation.
pub async fn children_in(
    ctx: &Ctx,
    query: &orch_api_contract::nodes::NodeChildListQuery,
    conn: &mut Conn,
    principal: &Principal,
    scope: Option<&crate::routes::nodes::projection::Projection<'_>>,
) -> ApiResult<(
    crate::routes::nodes::projection::Window<orch_api_contract::nodes::Node>,
    Vec<Info>,
)> {
    use orch_api_contract::{
        common::{DEFAULT_LIMIT, MAX_LIMIT, Page},
        nodes::Node,
    };
    let parent = ctx.id("id")?;
    let limit = usize::try_from(query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT))
        .map_err(ApiError::internal)?;
    let mut after = query
        .cursor
        .as_deref()
        .map(|v| {
            v.parse::<Id>()
                .map_err(|_| ApiError::invalid("cursor", "is not a cursor of this list"))
        })
        .transpose()?;
    let mut entries = Vec::new();
    loop {
        let ids:Vec<Id>=sqlx::query_scalar("SELECT id FROM nodes WHERE parent_id=$1 AND ($2::uuid IS NULL OR id>$2) ORDER BY id LIMIT $3").bind(parent).bind(after).bind(i64::from(MAX_LIMIT)).fetch_all(&mut *conn).await.map_err(DbError::from)?;
        if ids.is_empty() {
            break;
        }
        let selected = match scope {
            Some(scope) => scope.selected(conn, &ids).await?,
            None => Vec::new(),
        };
        let selected_ids: std::collections::HashSet<_> =
            selected.iter().filter_map(|row| row.object.id).collect();
        let scoped_principal = match scope {
            Some(scope) if !selected.is_empty() => Some(scope.principal(conn, &selected).await?),
            _ => None,
        };
        let principal = scoped_principal.as_ref().unwrap_or(principal);
        for id in &ids {
            after = Some(*id);
            let mut child = ctx.for_id(*id);
            if scope.is_some() && !selected_ids.contains(id) {
                continue;
            }
            let info = match prepare(&child, conn, principal).await {
                Ok(info) => info,
                Err(error)
                    if matches!(
                        error.code(),
                        ErrorCode::AuthzDenied | ErrorCode::NotFound | ErrorCode::Gone
                    ) =>
                {
                    continue;
                }
                Err(error) => return Err(error),
            };
            if info.is_none()
                && authorization::require(
                    conn,
                    principal,
                    *id,
                    authorization::VIEW,
                    orch_authz::Act::Use,
                    Policy::default(),
                )
                .await
                .is_err()
            {
                continue;
            }
            let read_ids = info.as_ref().map(|i| i.reads.clone()).unwrap_or_default();
            let node = trace::read_scope(
                &read_ids,
                crate::routes::nodes::view::one(conn, principal, *id, ctx.now, Policy::default()),
            )
            .await?;
            let node = match (scope, node) {
                (Some(scope), Some(node)) => {
                    trace::read_scope(&read_ids, scope.project(conn, principal, vec![node]))
                        .await?
                        .into_iter()
                        .next()
                }
                (_, node) => node,
            };
            let Some(mut node) = node else {
                continue;
            };
            let (kind, archived) = match &node {
                Node::Org(n) => (orch_api_contract::nodes::NodeKind::Org, n.base.archived),
                Node::Project(n) => (orch_api_contract::nodes::NodeKind::Project, n.base.archived),
                Node::Topic(n) => (orch_api_contract::nodes::NodeKind::Topic, n.base.archived),
                Node::Thread(n) => (orch_api_contract::nodes::NodeKind::Thread, n.base.archived),
            };
            if query.kind.is_some_and(|wanted| wanted != kind) {
                continue;
            }
            if info.is_none() {
                let filter = serde_json::to_value(query.archived.unwrap_or_default())
                    .map_err(ApiError::internal)?;
                if (filter == "exclude" && archived) || (filter == "only" && !archived) {
                    continue;
                }
            }
            if let Some(info) = &info {
                match &mut node {
                    Node::Org(n) => n.base.inspection = Some(info.banner.clone()),
                    Node::Project(n) => n.base.inspection = Some(info.banner.clone()),
                    Node::Topic(n) => n.base.inspection = Some(info.banner.clone()),
                    Node::Thread(n) => n.base.inspection = Some(info.banner.clone()),
                };
            }
            child.inspection = info;
            entries.push((node, child));
            if entries.len() > limit {
                break;
            }
        }
        if entries.len() > limit
            || ids.len() < usize::try_from(MAX_LIMIT).map_err(ApiError::internal)?
        {
            break;
        }
    }
    let more = entries.len() > limit;
    let witness = entries
        .get(limit)
        .map(|(node, _)| crate::routes::nodes::node_id(node));
    entries.truncate(limit);
    let next_cursor = more
        .then(|| {
            entries
                .last()
                .map(|(_, ctx)| ctx.id("id").map(|id| id.to_string()))
        })
        .flatten()
        .transpose()?;
    let mut items = Vec::with_capacity(entries.len());
    let mut inspections = Vec::new();
    for (node, child) in entries {
        if let Some(info) = child.inspection {
            inspections.push(info);
        }
        items.push(orch_api_contract::common::Listed::visible(node));
    }
    Ok((
        crate::routes::nodes::projection::Window {
            page: Page { items, next_cursor },
            more: witness,
        },
        inspections,
    ))
}

/// A list retains its own operation identity while reusing each actual child's native
/// restoration/deletion checks and inspection audit. No child-reader admission is borrowed.
pub async fn finish_child(
    ctx: &Ctx,
    info: &Info,
    node: &orch_api_contract::nodes::Node,
) -> ApiResult<()> {
    let (mut tx, guard) = transaction_guard::begin(ctx).await?;
    let scope = crate::routes::nodes::projection::Projection::new(
        ctx,
        guard.principal().caller(),
        "NodeChildList.inspect_deleted",
    )?;
    // This audit is an actual current checkpoint. Rebind the original selected
    // payload's typed sources too: a repaged current list cannot authorize old
    // environment/binding/README content after the same key lost that Read.
    let response = scope
        .finish(&mut tx, std::slice::from_ref(node), &[], None)
        .await?;
    response
        .read_response(ctx, &mut tx, guard.principal().caller(), |_, _| {
            Box::pin(async { Ok(()) })
        })
        .await?
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::AuthzDenied,
                "the node inspection is no longer allowed",
            )
        })?;
    let child = ctx.for_id(info.id);
    let current = prepare(&child, &mut tx, guard.principal()).await?;
    if current
        .as_ref()
        .is_some_and(|current| current.deletion != info.deletion)
    {
        return Err(ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "the deletion changed during inspection",
        ));
    }
    if current.is_some() {
        record(&child, &mut tx, guard.principal(), info, guard.now()).await?;
    }
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

#[cfg(any(test, feature = "test-support"))]
pub mod snapshot_tests {
    #![allow(clippy::unwrap_used, reason = "test assertions fail by panicking")]

    use super::*;
    fn info() -> Info {
        Info {
            kind: "document".into(),
            id: Id::from_u128(1),
            deletion: Some(Id::from_u128(2)),
            reads: vec![Id::from_u128(1), Id::from_u128(3)],
            banner: DeletedInspection {
                deleted_at: Time::new(chrono::DateTime::UNIX_EPOCH),
                deleted_by: None,
                purge_at: Time::new(chrono::DateTime::UNIX_EPOCH),
                readonly: true,
            },
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    fn successful_snapshot_keeps_exact_deletion_and_read_scope_without_auto_upgrade() {
        let state = Snapshot::default();
        let original = info();
        assert!(state.get().unwrap().is_none());
        state.publish(Some(original.clone())).unwrap();
        state.check(None, Some(&original)).unwrap();
        assert!(state.check(None, None).is_err());
        for field in 0..4 {
            let mut different = original.clone();
            match field {
                0 => different.id = Id::from_u128(9),
                1 => different.deletion = Some(Id::from_u128(9)),
                2 => different.reads.push(Id::from_u128(9)),
                _ => different.kind = "document_container".into(),
            }
            assert!(state.check(None, Some(&different)).is_err());
            assert!(state.publish(Some(different)).is_err());
        }
        let live = Snapshot::default();
        live.publish(None).unwrap();
        assert!(live.check(None, Some(&original)).is_err());
        assert!(live.publish(Some(original)).is_err());
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_successful_snapshot_keeps_exact_deletion_and_read_scope_without_auto_upgrade()
    {
        self::successful_snapshot_keeps_exact_deletion_and_read_scope_without_auto_upgrade()
    }
}
