//! Hierarchy with orgs inside orgs, a project and a department moved inside their tree, archive, what was deleted and its restore, tags, subscriptions, a place's Activity, members with their domain, moving one and suspending one, and the end of a membership.
//!
//! The tree a person sees is made here: a node by itself, what lies directly
//! inside it, the orgs they belong to, the way down from a root org with
//! the names they may know, and the nodes of a whole tree for a view of it.
//! Making a node (an org inside an org, a project, a topic), renaming it,
//! archiving it and bringing it back, and the person's own subscription to
//! it are changes; every change writes the audit row its action has in the
//! same transaction, stamps the node into the feed of its tree, and is
//! decided by the access engine and nobody else. A thread is a node too,
//! but its view and its making belong to the threads routes: a list leaves
//! threads out, and the requests about threads are not bound here.

pub mod activity;
mod archive_operation;
pub mod archive_plan;
pub(crate) mod content_sources;
mod creation_operation;
mod creation_replay;
mod deleted_orgs;
mod hierarchy_operation;
mod list_operation;
pub mod make;
mod metadata_mutation;
pub mod moves;
mod org_reading;
pub(crate) mod overview;
pub(crate) mod projection;
pub(crate) mod read_sources;
pub mod removal;
mod shared_operation;
pub mod tree;
pub(crate) mod tree_work;
pub mod view;

use crate::middleware::authentication::Caller;
use crate::middleware::authorization::{self, MANAGE, exact, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
use crate::registry::{Ctx, Registry};
use crate::sign_in::origin;
use orch_access_contract::audit::AuditAction;
use orch_access_contract::permissions::{NodePermission, Permission, PermissionName};
use orch_api_contract::common::{ErrorCode, FieldFault, Listed, Page, PageQuery, Problem};
use orch_api_contract::nodes::{
    ArchivedFilter, AutoAction, AutoField, Node, NodeArchive, NodeAuto, NodeAutoBody,
    NodeChildList, NodeChildListQuery, NodeGet, NodeKind, NodeSubscriptionDelete,
    NodeSubscriptionSet, NodeSubscriptionSetBody, NodeUnarchive, NodeUpdate, NodeUpdateBody,
    OrgBySlugGet, OrgList,
};
use orch_authz::{Act, Policy, Principal, epoch};
use orch_db::audit::{self, Actor, Entry};
use orch_db::queries::hierarchy::{self, Field, TextChange};
use orch_db::{Conn, Tx};
use orch_settings::{Place, get as setting};
use orch_settings_contract::registry::keys;
use orch_types::{Id, Json};
use serde_json::json;
use std::sync::Arc;

/// Begins the action with current authority, identity and time for its writes and audit.
pub(crate) async fn action(ctx: &Ctx) -> ApiResult<(Ctx, Tx, Principal)> {
    let (tx, guard) = transaction_guard::begin(ctx).await?;
    let mut current = ctx.clone();
    current.now = guard.now().get();
    current.caller = Caller::User(Arc::new(guard.user().clone()));
    Ok((current, tx, guard.principal().clone()))
}

/// The native Node-row serialization checkpoint, shared by actual metadata
/// mutations. Call after any earlier Thread lock; callers keep their no-op order.
pub(crate) async fn lock_node_change(
    ctx: &Ctx,
    conn: &mut Conn,
    id: Id,
) -> ApiResult<(Ctx, transaction_guard::Guard)> {
    if !hierarchy::lock(conn, id).await? {
        return Err(ApiError::not_found());
    }
    transaction_guard::after_wait(ctx, conn).await
}

/// The most characters of a node's name: what the schema allows.
pub const NAME_MAX_CHARS: usize = 120;
/// The most characters of a node's description: what the schema allows.
pub const DESCRIPTION_MAX_CHARS: usize = 4_000;
/// How many nodes of a list are read in one statement besides the page asked
/// for: a page of mostly visible nodes is found in one round.
const READ_AHEAD: usize = 1;
/// The name of the project that a team's root org starts with.
pub const GENERAL_PROJECT: &str = "General";
/// The name of a topic made without one, until automation or a person names it.
pub const NEW_TOPIC: &str = "New topic";

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    archive_plan::mount(registry);
    registry
        .on_with_operation(OrgList, org_reading::list, org_list)
        .on_with_operation(OrgBySlugGet, org_reading::address, org_by_slug)
        .on_with_operation(NodeGet, node_get_operation, node_get)
        .on_with_operation(
            orch_api_contract::nodes::OrgOverviewGet,
            overview::prepare,
            overview::get,
        )
        .on_with_operation(
            orch_api_contract::nodes::NodeOverviewGet,
            overview::node_prepare,
            overview::node_get,
        )
        .on_with_optional_operation(NodeChildList, list_operation::children, child_list)
        .on_with_operation(NodeUpdate, metadata_mutation::update, node_update)
        .on(NodeAuto, node_auto)
        .on_with_operation(NodeArchive, archive_operation::change, archive)
        .on_with_operation(NodeUnarchive, archive_operation::change, unarchive)
        .on(NodeSubscriptionSet, subscription_set)
        .on(NodeSubscriptionDelete, subscription_delete);
    make::mount(registry);
    moves::mount(registry);
    removal::mount(registry);
    tree::mount(registry);
    activity::mount(registry);
}

/// A member of a body or a query that is wrong.
pub fn invalid(path: &str, message: &str) -> ApiError {
    Problem::invalid(vec![FieldFault {
        path: path.to_owned(),
        message: message.to_owned(),
    }])
    .into()
}

/// A name as it is kept: without the space around it, and neither empty nor
/// longer than a node's name may be.
pub fn clean_name(path: &str, name: &str) -> ApiResult<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(invalid(path, "must not be empty"));
    }
    if name.chars().count() > NAME_MAX_CHARS {
        return Err(invalid(
            path,
            &format!("must be at most {NAME_MAX_CHARS} characters"),
        ));
    }
    Ok(name.to_owned())
}

/// A description as it is kept: no longer than a node's description may be.
pub fn clean_description(path: &str, text: &str) -> ApiResult<String> {
    if text.chars().count() > DESCRIPTION_MAX_CHARS {
        return Err(invalid(
            path,
            &format!("must be at most {DESCRIPTION_MAX_CHARS} characters"),
        ));
    }
    Ok(text.to_owned())
}

/// The answer to a request that names a thread where the thread engine has
/// the part to play that is not built yet: what deleting, restoring or listing
/// the Activity of a thread takes of it.
pub fn threads_are_not_served_here() -> ApiError {
    ApiError::internal(
        "a thread is read and changed by the threads routes, which are not bound here",
    )
}

/// The two decisions of the access engine that settings make about sharing at `node`: whether
/// `sharing.allow_external` lets the owners of a thing there name somebody from outside its root
/// org, and whether `sharing.allow_holders` lets somebody who holds a thing there without owning
/// it or managing its page share it. Both are true as shipped.
pub async fn policy_at(conn: &mut Conn, node: Id) -> ApiResult<Policy> {
    let allowed = setting::<bool>(conn, Place::Node(node), keys::SHARING_ALLOW_EXTERNAL)
        .await
        .map_err(ApiError::internal)?;
    let holders = setting::<bool>(conn, Place::Node(node), keys::SHARING_ALLOW_HOLDERS)
        .await
        .map_err(ApiError::internal)?;
    Ok(Policy {
        owners_name_outsiders: allowed.unwrap_or(true),
        holders_share: holders.unwrap_or(true),
    })
}

/// Where a node stands, as the audit row of an act on it needs it.
#[derive(Debug, Clone, Copy)]
pub struct Placed {
    /// The node.
    pub id: Id,
    /// Its root org.
    pub org: Id,
}

/// Writes the audit row of an act of the caller on a node, in the caller's
/// transaction, and stamps the node into the feed of its tree when the act
/// leaves a line in its history.
pub async fn record(
    ctx: &Ctx,
    conn: &mut Conn,
    action: AuditAction,
    node: Placed,
    summary: &str,
    details: Json,
) -> ApiResult<()> {
    record_about(ctx, conn, action, node.id, node, summary, details).await
}

/// Writes the audit row of an act of the caller on `object`, a thing that
/// lies at `node` without being the node: a group, a tag, a service
/// identity. The row stands beside the node, and is a line of its history
/// where the action is one.
pub async fn record_about(
    ctx: &Ctx,
    conn: &mut Conn,
    action: AuditAction,
    object: Id,
    node: Placed,
    summary: &str,
    details: Json,
) -> ApiResult<()> {
    let mut entry = Entry::new(
        Actor::User(ctx.user()?.id()),
        action.as_str(),
        action.object_kind().unwrap_or("node"),
        Some(object),
        summary,
    );
    entry.history = action.is_history_line();
    entry.node_id = Some(node.id);
    entry.org_id = Some(node.org);
    entry.details = details;
    audit::append(conn, &entry, &origin(ctx), ctx.now).await?;
    Ok(())
}

/// The root org of a node, and its kind, where it is there.
pub async fn placed(conn: &mut Conn, id: Id) -> ApiResult<(Placed, NodeKind)> {
    let found = hierarchy::with_ancestors(conn, &[id]).await?;
    let node = found
        .iter()
        .find(|node| node.id == id && node.deleted_at.is_none())
        .ok_or_else(ApiError::not_found)?;
    let kind = refusals::node_kind(&node.kind).ok_or_else(ApiError::not_found)?;
    Ok((
        Placed {
            id,
            org: node.org_id,
        },
        kind,
    ))
}

/// Who is acting, as a sentence names them.
pub fn actor_name(ctx: &Ctx) -> ApiResult<String> {
    Ok(ctx.user()?.user.display_name.clone())
}

/// The orgs the caller belongs to or owns, their own first.
async fn org_list(ctx: Ctx, (): (), (): ()) -> ApiResult<Vec<orch_api_contract::nodes::Org>> {
    org_reading::get_list(ctx, (), ()).await
}

/// An org by its short name or its address.
async fn org_by_slug(ctx: Ctx, (): (), (): ()) -> ApiResult<orch_api_contract::nodes::Org> {
    org_reading::get_address(ctx, (), ()).await
}

/// A node of any kind the caller may view.
async fn node_get(
    ctx: Ctx,
    _query: orch_api_contract::nodes::DeletedInspectionQuery,
    (): (),
) -> ApiResult<Node> {
    let id = ctx.id("id")?;
    let mut conn = ctx.app.db.begin_snapshot().await?;
    let principal = ctx.operation_principal(&mut conn).await?;
    let answer = view::one(&mut conn, &principal, id, ctx.now, Policy::default())
        .await?
        .ok_or_else(threads_are_not_served_here)?;
    conn.commit().await.map_err(orch_db::DbError::from)?;
    Ok(answer)
}

fn node_get_operation<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    _caller: &'a orch_authz::Caller,
    query: orch_api_contract::nodes::DeletedInspectionQuery,
    (): (),
) -> crate::registry::OperationFuture<'a> {
    Box::pin(async move {
        let selection = if query.inspect_deleted == Some(true) {
            "NodeGet.inspect_deleted"
        } else {
            "NodeGet.read"
        };
        let target = crate::key_operations::targets::node_identity(conn, ctx.id("id")?).await?;
        let mut operation = crate::registry::OperationPlan::new(ctx.route, selection)?;
        operation.bind(0, target.object.kind, &[target])?;
        Ok(operation)
    })
}

/// A cursor is the identifier of the last node of the page before.
fn cursor_of(path: &str, cursor: Option<&str>) -> ApiResult<Option<Id>> {
    cursor
        .map(|cursor| {
            cursor
                .parse()
                .map_err(|_| invalid(path, "is not a cursor of this list"))
        })
        .transpose()
}

/// Whether a node is listed under the archive filter asked for.
fn archived_ok(filter: ArchivedFilter, archived: bool) -> bool {
    match filter {
        ArchivedFilter::Exclude => !archived,
        ArchivedFilter::Include => true,
        ArchivedFilter::Only => archived,
    }
}

/// What lies directly inside a node, by when it was made, as far as the
/// caller may view it.
async fn child_list(ctx: Ctx, query: NodeChildListQuery, (): ()) -> ApiResult<Page<Listed<Node>>> {
    list_operation::read_children(ctx, query).await
}

/// The ordinary selector/window producer, reused on the current read connection.
pub(super) async fn child_list_in(
    ctx: &Ctx,
    query: &NodeChildListQuery,
    conn: &mut Conn,
    principal: &Principal,
    scope: Option<&projection::Projection<'_>>,
) -> ApiResult<projection::Window<Node>> {
    let parent = ctx.id("id")?;
    let page = PageQuery {
        limit: query.limit,
        cursor: query.cursor.clone(),
    };
    let limit = usize::try_from(page.limit()).unwrap_or(usize::MAX);
    let filter = query.archived.unwrap_or_default();
    let mut after = cursor_of("cursor", query.cursor.as_deref())?;
    let child_kind = if let Some(kind) = query.kind {
        let (_, parent_kind) = placed(conn, parent).await?;
        let allowed = match parent_kind {
            NodeKind::Org => matches!(kind, NodeKind::Org | NodeKind::Project),
            NodeKind::Project => matches!(kind, NodeKind::Project | NodeKind::Topic),
            NodeKind::Topic | NodeKind::Thread => kind == NodeKind::Thread,
        };
        if !allowed {
            return Err(invalid("kind", "is not a kind this node can hold"));
        }
        Some(match kind {
            NodeKind::Org => "org",
            NodeKind::Project => "project",
            NodeKind::Topic => "topic",
            NodeKind::Thread => "thread",
        })
    } else {
        None
    };
    let batch = i64::try_from(limit + READ_AHEAD).unwrap_or(i64::MAX);
    let mut items: Vec<Node> = Vec::new();
    loop {
        let rows = match child_kind {
            Some(kind) => {
                hierarchy::presentation::children_of_kind(conn, parent, kind, after, batch).await?
            }
            None => hierarchy::children_after(conn, parent, after, batch).await?,
        };
        let exhausted = i64::try_from(rows.len()).is_ok_and(|count| count < batch);
        let Some(last) = rows.last().map(|row| row.id) else {
            break;
        };
        after = Some(last);
        let ids: Vec<Id> = rows.iter().map(|row| row.id).collect();
        let nodes = match scope {
            Some(scope) => scope.visible(conn, &ids).await?,
            None => view::visible(conn, principal, &ids, ctx.now, Policy::default()).await?,
        };
        items.extend(
            nodes
                .into_iter()
                .filter(|node| archived_ok(filter, node_archived(node)))
                .filter(|node| !query.participating.unwrap_or(false) || node_participating(node)),
        );
        if items.len() > limit || exhausted {
            break;
        }
    }
    let more = items.len() > limit;
    let witness = items.get(limit).map(node_id);
    items.truncate(limit);
    // The private children the caller administers stand once, on the first page, after what they may
    // read, sorted by name: an existence list that counts nothing and tells no work.
    let hidden = if query.cursor.is_none() && !query.participating.unwrap_or(false) {
        let candidates = match child_kind {
            Some(kind) => {
                hierarchy::presentation::children_of_kind(
                    conn,
                    parent,
                    kind,
                    None,
                    PRIVATE_CHILDREN_MOST,
                )
                .await?
            }
            None => hierarchy::children_after(conn, parent, None, PRIVATE_CHILDREN_MOST).await?,
        };
        let ids: Vec<Id> = candidates
            .iter()
            .filter(|row| archived_ok(filter, row.archived_at.is_some()))
            .map(|row| row.id)
            .collect();
        match scope {
            Some(scope) => scope.private_rows(conn, &ids).await?,
            None => tree::private_rows(conn, principal, &ids).await?,
        }
    } else {
        Vec::new()
    };
    let next_cursor = more
        .then(|| items.last().map(|node| node_id(node).to_string()))
        .flatten();
    let answer = Page {
        next_cursor,
        items: items
            .into_iter()
            .map(Listed::visible)
            .chain(hidden.into_iter().map(Listed::hidden))
            .collect(),
    };
    Ok(projection::Window {
        page: answer,
        more: witness,
    })
}

/// The most private children a first page names for an admin: an existence list, bounded so that the
/// first page costs what the others do.
const PRIVATE_CHILDREN_MOST: i64 = 200;

/// A node's identifier.
#[must_use]
pub fn node_id(node: &Node) -> Id {
    match node {
        Node::Org(node) => node.base.id,
        Node::Project(node) => node.base.id,
        Node::Topic(node) => node.base.id,
        Node::Thread(node) => node.base.id,
    }
}

/// The frame's ordinary View result, never a client-derived permission/role test.
pub fn node_participating(node: &Node) -> bool {
    match node {
        Node::Org(each) => each.base.access.participating,
        Node::Project(each) => each.base.access.participating,
        Node::Topic(each) => each.base.access.participating,
        Node::Thread(each) => each.base.access.participating,
    }
}

/// Whether a node is archived, itself or through a node above it.
#[must_use]
pub fn node_archived(node: &Node) -> bool {
    match node {
        Node::Org(node) => node.base.archived,
        Node::Project(node) => node.base.archived,
        Node::Topic(node) => node.base.archived,
        Node::Thread(node) => node.base.archived,
    }
}

/// A node's revision.
#[must_use]
pub fn node_revision(node: &Node) -> i64 {
    match node {
        Node::Org(node) => node.base.revision,
        Node::Project(node) => node.base.revision,
        Node::Topic(node) => node.base.revision,
        Node::Thread(node) => node.base.revision,
    }
}

/// The node as the caller sees it after a change, or the answer to a
/// request about a thread.
pub async fn shown(ctx: &Ctx, conn: &mut Conn, principal: &Principal, id: Id) -> ApiResult<Node> {
    view::one(conn, principal, id, ctx.now, Policy::default())
        .await?
        .ok_or_else(threads_are_not_served_here)
}

/// The original native README association check, shared by early validation and
/// the final locked metadata checkpoint; it adds no document View requirement.
async fn readme_choice(
    conn: &mut Conn,
    node: Id,
    choice: Option<Option<Id>>,
) -> ApiResult<Option<Option<Id>>> {
    Ok(match choice {
        None => None,
        Some(None) => Some(None),
        Some(Some(document)) => {
            let row = orch_db::queries::documents::get(conn, document)
                .await?
                .filter(|row| row.deleted_at.is_none())
                .ok_or_else(ApiError::not_found)?;
            if row.node_id != node {
                return Err(ApiError::new(
                    ErrorCode::NodeWrongParent,
                    "a README is one of the place's own documents",
                ));
            }
            Some(Some(document))
        }
    })
}

/// Renames a node or changes its description; either hands that field to
/// the person until it is given back to automation.
async fn node_update(ctx: Ctx, (): (), body: NodeUpdateBody) -> ApiResult<Node> {
    let id = ctx.id("id")?;
    // The same pure input normalization keeps its original early error order.
    let _ = metadata_mutation::fields(&body)?;
    let (ctx, mut tx, _) = action(&ctx).await?;
    let initial = ctx.operation_data::<metadata_mutation::Update>()?;
    if initial.noop() {
        // The original no-op does not acquire a new Thread/Node lock or a revision.
        return Ok(initial.before.clone());
    }
    let is_thread = initial.kind == NodeKind::Thread;
    if is_thread {
        orch_db::queries::threads::lock(&mut tx, id)
            .await?
            .ok_or_else(ApiError::not_found)?;
    }
    let (ctx, guard) = lock_node_change(&ctx, &mut tx, id).await?;
    let principal = guard.principal().clone();
    let prepared = guard.operation_data::<metadata_mutation::Update>()?;
    let metadata_mutation::Update {
        fields,
        place,
        readme,
        before,
        ..
    } = prepared.as_ref().clone();
    let metadata_mutation::Fields {
        color,
        own,
        name,
        description,
    } = fields;
    guard.check_deadline(&ctx)?;
    let revision = hierarchy::presentation::change_with_own(
        &mut tx,
        id,
        TextChange {
            name: name.as_deref(),
            description: description.as_deref(),
        },
        color,
        own.as_ref().map(|own| own.as_deref()),
        body.expected_revision,
        ctx.now,
    )
    .await?;
    if revision.is_none() {
        let current = shown(&ctx, &mut tx, &principal, id).await?;
        return Err(stale(&current));
    }
    if let Some(document) = readme {
        orch_db::queries::documents::set_readme(&mut tx, id, document).await?;
    }
    let who = actor_name(&ctx)?;
    let summary = match (&name, &description, &color, &readme) {
        (Some(name), _, _, _) => format!("{who} renamed {} to {name}", before_name(&before)),
        (None, Some(_), _, _) => {
            format!("{who} changed the description of {}", before_name(&before))
        }
        (None, None, Some(_), _) => {
            format!("{who} changed the icon colour of {}", before_name(&before))
        }
        (None, None, None, _) if own.is_some() => {
            format!("{who} changed the icon colour of {}", before_name(&before))
        }
        (None, None, None, Some(Some(_))) => {
            format!("{who} chose the README of {}", before_name(&before))
        }
        _ => format!("{who} took away the README of {}", before_name(&before)),
    };
    record(
        &ctx,
        &mut tx,
        orch_db::audit::action!(AuditAction::NodeUpdated),
        place,
        &summary,
        json!({
            "name": name.as_ref().map(|to| json!({ "from": before_name(&before), "to": to })),
            "description": description.is_some(),
            "color": body.color.map(|to| json!({"from": color::current(&before), "to": to})),
            "custom_color": own.as_ref().map(|to| json!({"from": color::current_own(&before), "to": to})),
            "readme": readme.map(|to| json!({ "to": to })),
        }),
    )
    .await?;
    hierarchy::stamp(&mut tx, id).await?;
    if is_thread {
        let seq = orch_db::sequences::next(&mut tx, id)
            .await?
            .ok_or_else(ApiError::not_found)?;
        orch_db::queries::threads::set_seq_state(&mut tx, id, seq).await?;
        orch_db::sequences::notify(&mut tx, id, seq).await?;
    }
    let node = shown(&ctx, &mut tx, &principal, id).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(node)
}

/// A node's name.
pub fn before_name(node: &Node) -> String {
    match node {
        Node::Org(node) => node.base.name.clone(),
        Node::Project(node) => node.base.name.clone(),
        Node::Topic(node) => node.base.name.clone(),
        Node::Thread(node) => node.base.name.clone(),
    }
}

/// `409 conflict.stale_revision`, with the node as it is now under `current`.
pub fn stale(current: &Node) -> ApiError {
    ApiError::new(
        ErrorCode::ConflictStaleRevision,
        "it was changed since it was read",
    )
    .with("current", current)
}

/// Hands the title or tags back to automation, or asks
/// it for a new value.
async fn node_auto(ctx: Ctx, (): (), body: NodeAutoBody) -> ApiResult<Node> {
    let id = ctx.id("id")?;
    let (ctx, mut tx, principal) = action(&ctx).await?;
    let (place, _) = placed(&mut tx, id).await?;
    let (field, key) = match body.field {
        AutoField::Title => (Field::Title, keys::AUTO_TITLE),
        AutoField::Tags => (Field::Tags, keys::AUTO_TAGS),
    };
    let mode = setting::<String>(&mut tx, Place::Node(id), key)
        .await
        .map_err(ApiError::internal)?;
    if mode.as_deref().is_none_or(|mode| mode == "off") {
        return Err(
            ApiError::new(ErrorCode::AutoDisabled, format!("{key} is off here"))
                .with("setting", key),
        );
    }
    hierarchy::hand_back(&mut tx, id, field, ctx.now).await?;
    let who = actor_name(&ctx)?;
    let word = match body.field {
        AutoField::Title => "title",
        AutoField::Tags => "tags",
    };
    let asked = match body.action {
        AutoAction::Resume => "gave the",
        AutoAction::Refresh => "gave the",
    };
    let refresh = body.action == AutoAction::Refresh;
    record(
        &ctx,
        &mut tx,
        AuditAction::NodeUpdated,
        place,
        &format!("{who} {asked} {word} back to automation"),
        json!({ "field": word, "refresh": refresh }),
    )
    .await?;
    hierarchy::stamp(&mut tx, id).await?;
    let node = shown(&ctx, &mut tx, &principal, id).await?;
    if refresh {
        orch_engine::summaries::due(&mut tx, id, "refresh", ctx.now)
            .await
            .map_err(ApiError::internal)?;
    }
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(node)
}

/// Whether the caller may archive or bring back the node `id`: the Archive
/// row of any node, which whoever manages it holds
/// with it.
async fn may_archive(conn: &mut Conn, principal: &Principal, id: Id, act: Act) -> ApiResult<()> {
    authorization::require(
        conn,
        principal,
        id,
        PermissionName::OfNode(NodePermission::Archive),
        act,
        Policy::default(),
    )
    .await
}

/// Archives an org, a project, a topic or a thread, with everything inside it.
async fn archive(
    ctx: Ctx,
    (): (),
    _body: orch_api_contract::nodes::NodeArchiveBody,
) -> ApiResult<orch_api_contract::nodes::NodeArchiveResponse> {
    change_archive(&ctx, true)
        .await
        .map_err(|error| archive_operation::problem(&ctx, error))
}

/// Takes a node out of the archive.
async fn unarchive(
    ctx: Ctx,
    (): (),
    _body: orch_api_contract::nodes::NodeArchiveBody,
) -> ApiResult<orch_api_contract::nodes::NodeArchiveResponse> {
    change_archive(&ctx, false)
        .await
        .map_err(|error| archive_operation::problem(&ctx, error))
}

async fn change_archive(
    ctx: &Ctx,
    archiving: bool,
) -> ApiResult<orch_api_contract::nodes::NodeArchiveResponse> {
    let id = ctx.id("id")?;
    let (current, mut tx, _) = archive_operation::action(ctx).await?;
    let (current, guard, bounds) = archive_operation::effect(&current, &mut tx, id).await?;
    let ctx = &current;
    let principal = guard.principal().clone();
    let Some(archive_operation::NativePrefix::Change {
        place,
        before,
        review,
    }) = bounds.native.clone()
    else {
        return Err(ApiError::internal(
            "an archive action lost its native operands",
        ));
    };
    // The original private confirmation DELETE is the first unaccepted SQL.
    // All its caller/Kept/canonical-stamp reads were completed by this exact guard.
    guard.check_deadline(ctx)?;
    if let Some(current) = review.consume(&mut tx).await? {
        tx.commit().await.map_err(orch_db::DbError::from)?;
        return Err(ApiError::new(orch_api_contract::common::ErrorCode::ConflictStaleRevision,
            "The subtree or its work changed after this review. Nothing was archived; review the current plan.").with("current", current));
    }
    let changed = if archiving {
        hierarchy::archive(&mut tx, id, principal.user(), ctx.now).await?
    } else {
        hierarchy::unarchive(&mut tx, id, ctx.now).await?
    };
    let settle_older_archive = archiving
        && !changed
        && orch_db::queries::lifecycle::archive_work_pending(&mut tx, id).await?;
    if changed || settle_older_archive {
        epoch::bump(&mut tx).await.map_err(ApiError::internal)?;
        let settled = if archiving {
            Some(
                ctx.app
                    .engine
                    .archive_in(&mut tx, id, principal.user(), principal.now())
                    .await
                    .map_err(ApiError::engine)?,
            )
        } else {
            None
        };
        let who = actor_name(ctx)?;
        let (action, word) = if archiving {
            (
                orch_db::audit::action!(AuditAction::NodeArchived),
                "archived",
            )
        } else {
            (
                orch_db::audit::action!(AuditAction::NodeUnarchived),
                "brought back",
            )
        };
        let inside = hierarchy::descendants(&mut tx, id).await?.len();
        record(
            ctx,
            &mut tx,
            action,
            place,
            &format!("{who} {word} {}", before_name(&before)),
            json!({ "inside": inside, "threads": settled.as_ref().map(|count| count.threads),
                "turns": settled.as_ref().map(|count| count.turns),
                "inputs": settled.as_ref().map(|count| count.inputs),
                "approvals": settled.as_ref().map(|count| count.approvals) }),
        )
        .await?;
        hierarchy::stamp_subtree(&mut tx, id).await?;
    }
    let (current, principal, bounds) =
        archive_operation::response_bounds(ctx, &mut tx, &principal).await?;
    let ctx = &current;
    let node = archive_operation::read_node(ctx, &mut tx, &principal, id, &bounds).await?;
    let undo = if changed {
        Some(archive_plan::make(ctx, &mut tx, &principal, id, !archiving, Some(&bounds)).await?)
    } else {
        None
    };
    let revision = hierarchy::with_ancestors(&mut tx, &[id])
        .await?
        .into_iter()
        .find(|row| row.id == id)
        .ok_or_else(ApiError::not_found)?
        .revision;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    ctx.app.engine.kick();
    match node {
        Some(node) if !ctx.key_receipt_required() => {
            Ok(orch_api_contract::nodes::NodeArchiveResponse::Full(
                Box::new(orch_api_contract::nodes::NodeArchiveResult { node, undo }),
            ))
        }
        _ => Ok(orch_api_contract::nodes::NodeArchiveResponse::Receipt(
            orch_api_contract::nodes::NodeArchiveReceipt {
                projection: orch_api_contract::common::ReceiptProjection::Receipt,
                node_id: id,
                archiving,
                revision,
                undo: undo.as_ref().map(archive_operation::safe_body),
            },
        )),
    }
}

/// The caller's own `Access` on a node, as the node shows it.
async fn access_of(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
) -> ApiResult<orch_api_contract::nodes::Access> {
    let node = shown(ctx, conn, principal, id).await?;
    Ok(match node {
        Node::Org(node) => node.base.access,
        Node::Project(node) => node.base.access,
        Node::Topic(node) => node.base.access,
        Node::Thread(node) => node.base.access,
    })
}

/// Sets how much of a node the caller wants to be told.
async fn subscription_set(
    ctx: Ctx,
    (): (),
    body: NodeSubscriptionSetBody,
) -> ApiResult<orch_api_contract::nodes::Access> {
    let id = ctx.id("id")?;
    let (ctx, mut tx, principal) = action(&ctx).await?;
    let access = change_subscription(&ctx, &mut tx, &principal, id, Some(body.level)).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(access)
}

/// Removes the caller's own subscription row on a node.
async fn subscription_delete(
    ctx: Ctx,
    (): (),
    (): (),
) -> ApiResult<orch_api_contract::nodes::Access> {
    let id = ctx.id("id")?;
    let (ctx, mut tx, principal) = action(&ctx).await?;
    let access = change_subscription(&ctx, &mut tx, &principal, id, None).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(access)
}

/// The one node subscription mutation, shared by the bell and the personal exception list.
/// Its caller owns the action transaction and the route's current node View check.
pub(crate) async fn change_subscription(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
    level: Option<orch_api_contract::common::SubscriptionLevel>,
) -> ApiResult<orch_api_contract::nodes::Access> {
    placed(conn, id).await?;
    if let Some(level) = level {
        let word = serde_json::to_value(level).map_err(ApiError::internal)?;
        let word = word
            .as_str()
            .ok_or_else(|| ApiError::internal("the subscription level is not a word"))?;
        hierarchy::put_subscription(conn, principal.user(), id, word, "manual", ctx.now).await?;
    } else {
        hierarchy::delete_subscription(conn, principal.user(), id).await?;
    }
    orch_chat::following::refresh_thread(conn, id, &[principal.user()], ctx.now)
        .await
        .map_err(ApiError::internal)?;
    access_of(ctx, conn, principal, id).await
}

/// Used by the creating requests: the permission the engine is asked for
/// before a node is made inside another is the one the request is declared
/// with; this names the one for making a thing of `kind` where it goes.
#[must_use]
pub fn permission_to_make(kind: NodeKind) -> PermissionName {
    match kind {
        NodeKind::Org => exact(Permission::OrgManage),
        NodeKind::Project => exact(Permission::OrgInteract),
        NodeKind::Topic => exact(Permission::ProjectCreateTopics),
        NodeKind::Thread => exact(Permission::TopicStartThreads),
    }
}

/// Requires Manage of the node `id`, whatever its kind.
pub async fn require_manage(
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
    act: Act,
) -> ApiResult<()> {
    authorization::require(conn, principal, id, MANAGE, act, Policy::default()).await
}

// Node icon colours and document visibility in tree summaries.
mod color;
pub(crate) mod tags;
pub(crate) mod tree_docs;
