//! Org groups, roles, tags and service identities, each kept by one org at any level.
//!
//! An org keeps its own people: the members whose domain is it or an org
//! inside it, the groups it makes, and the service identities it creates for
//! automation. A member can be suspended, which closes the org and
//! everything inside it to them without removing anything of theirs. Tags
//! are the labels that topics carry, defined by an org or a project. Every
//! change here is decided by the access engine on the org that keeps the
//! thing, and writes its audit row in the same transaction.

use crate::middleware::authorization::{self, exact, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Answer, Ctx, Registry};
use crate::routes::nodes::{Placed, actor_name, invalid, placed, record, record_about};
use crate::sign_in::NOT_KEPT;
use http::header::CACHE_CONTROL;
use orch_access_contract::audit::AuditAction;
use orch_access_contract::permissions::Permission;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::common::{Page, PageQuery};
use orch_api_contract::directory::{
    Group, GroupDelete, GroupHolderAdd, GroupHolderList, GroupHolderRemove, GroupMemberAdd,
    GroupMemberList, GroupMemberRemove, GroupUpdate, GroupUpdateBody, Member, NodeGroupList,
    NodeTagCreate, NodeTagCreateBody, NodeTagList, OrgGroupCreate, OrgGroupCreateBody,
    OrgGroupList, OrgGroupListQuery, OrgMemberList, OrgMemberListQuery, OrgMemberResume,
    OrgMemberSuspend, OrgMemberSuspendBody, OrgSuspensionList, Suspension, TagDelete, TagUpdate,
    TagUpdateBody,
};
use orch_api_contract::nodes::{NodeKind, NodeRef, Tag};
use orch_api_contract::users::COLOR_COUNT;
use orch_api_contract::users::UserRef;
use orch_authz::{Act, Policy};
use orch_db::queries::hierarchy::{self, TagRow};
use orch_db::queries::{access, identity};
use orch_db::{Conn, DbError};
use orch_types::{Id, new_id};
use serde_json::json;
use std::collections::HashMap;

mod members;
mod outsiders;

/// The most characters of a tag's name: what the schema allows.
const TAG_NAME_MAX_CHARS: usize = 40;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    members::mount(registry);
    outsiders::mount(registry);
    registry
        .on(OrgGroupList, group_list)
        .on(NodeGroupList, place_groups)
        .on(OrgGroupCreate, group_create)
        .on(GroupUpdate, group_update)
        .on(GroupDelete, group_delete)
        .on(GroupMemberList, group_member_list)
        .on(GroupHolderList, group_holder_list)
        .on(GroupHolderAdd, group_holder_add)
        .on(GroupHolderRemove, group_holder_remove)
        .on(GroupMemberAdd, group_member_add)
        .on(GroupMemberRemove, group_member_remove)
        .on(OrgMemberList, member_list)
        .on(OrgMemberSuspend, member_suspend)
        .on(OrgMemberResume, member_resume)
        .on(OrgSuspensionList, suspension_list)
        .on(
            orch_api_contract::directory::OrgServiceAccountList,
            service_list,
        )
        .on(
            orch_api_contract::directory::OrgServiceAccountCreate,
            service_create,
        )
        .on(
            orch_api_contract::directory::ServiceAccountDelete,
            service_delete,
        )
        .on(
            orch_api_contract::directory::ServiceAccountTokenList,
            service_token_list,
        )
        .on(
            orch_api_contract::directory::ServiceAccountTokenCreate,
            service_token_create,
        )
        .on(
            orch_api_contract::directory::ServiceAccountTokenDelete,
            service_token_delete,
        )
        .on(NodeTagList, tag_list)
        .on(NodeTagCreate, tag_create)
        .on(TagUpdate, tag_update)
        .on(TagDelete, tag_delete);
}

fn clean_tag_name(name: &str) -> ApiResult<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > TAG_NAME_MAX_CHARS {
        return Err(invalid(
            "name",
            &format!("must be 1 to {TAG_NAME_MAX_CHARS} characters"),
        ));
    }
    Ok(name.to_owned())
}

fn clean_color(color: u32) -> ApiResult<i16> {
    if color >= COLOR_COUNT {
        return Err(invalid("color", &format!("must be below {COLOR_COUNT}")));
    }
    i16::try_from(color).map_err(|_| invalid("color", "is not a colour"))
}

/// The colour a tag made at `node` without one takes: the first of the palette that no tag of the
/// place has, so that the tags of a place are told apart; once every one is taken, the next in turn
/// after the most recently created tag, retaining that order after manual colour changes.
async fn next_color(conn: &mut Conn, node: Id) -> ApiResult<i16> {
    let taken = hierarchy::tags_of(conn, &[node]).await?;
    let count = i16::try_from(COLOR_COUNT).map_err(|_| ApiError::internal("too many colours"))?;
    let free = (0..count).find(|color| !taken.iter().any(|tag| tag.color == *color));
    let turn = taken
        .last()
        .map_or(0, |tag| (tag.color + 1).rem_euclid(count));
    Ok(free.unwrap_or(turn))
}

/// The tags as the caller reads them, each with the org or project that
/// defines it, named where they may know it.
async fn shown(
    conn: &mut Conn,
    principal: &orch_authz::Principal,
    rows: &[TagRow],
) -> ApiResult<Vec<Tag>> {
    let nodes: Vec<Id> = rows.iter().map(|row| row.node_id).collect();
    let refs: HashMap<Id, NodeRef> =
        crate::routes::nodes::view::refs(conn, principal, &nodes, Policy::default()).await?;
    Ok(rows
        .iter()
        .map(|row| Tag {
            id: row.id,
            node: refs.get(&row.node_id).cloned().unwrap_or(NodeRef {
                thread_display: None,
                id: row.node_id,
                kind: NodeKind::Org,
                name: None,
                redacted: true,
            }),
            name: row.name.clone(),
            color: u32::try_from(row.color).unwrap_or_default(),
        })
        .collect())
}

/// A node's own tags and those of the orgs above it.
async fn tag_list(ctx: Ctx, (): (), (): ()) -> ApiResult<Vec<Tag>> {
    let id = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let rows = hierarchy::with_ancestors(&mut conn, &[id]).await?;
    let node = rows
        .iter()
        .find(|row| row.id == id)
        .ok_or_else(ApiError::not_found)?;
    let defining = node.path.clone();
    let mut tags = hierarchy::tags_of(&mut conn, &defining).await?;
    // From the root org down, then in the order they were defined.
    tags.sort_by_key(|tag| defining.iter().position(|place| *place == tag.node_id));
    shown(&mut conn, &principal, &tags).await
}

/// Requires what defining a tag on the node `id` takes: Manage of the org or
/// the project.
async fn require_tag_manager(
    conn: &mut Conn,
    principal: &orch_authz::Principal,
    id: Id,
    kind: NodeKind,
) -> ApiResult<()> {
    let permission = match kind {
        NodeKind::Org => Permission::OrgManage,
        NodeKind::Project => Permission::ProjectManage,
        NodeKind::Topic => Permission::TopicManage,
        NodeKind::Thread => Permission::ThreadManage,
    };
    authorization::require(
        conn,
        principal,
        id,
        exact(permission),
        Act::Use,
        Policy::default(),
    )
    .await
}

fn taken(error: DbError) -> ApiError {
    match error {
        DbError::Unique { .. } => ApiError::new(
            ErrorCode::ConflictExists,
            "a tag of that name exists already here",
        ),
        other => other.into(),
    }
}

/// Defines a tag at any place.
async fn tag_create(ctx: Ctx, (): (), body: NodeTagCreateBody) -> ApiResult<Tag> {
    let id = ctx.id("id")?;
    let name = clean_tag_name(&body.name)?;
    let asked = body.color.map(clean_color).transpose()?;
    let (ctx, mut tx, principal) = crate::routes::nodes::action(&ctx).await?;
    let (place, kind) = placed(&mut tx, id).await?;
    require_tag_manager(&mut tx, &principal, id, kind).await?;
    let live: Option<Id> =
        sqlx::query_scalar("SELECT id FROM nodes WHERE id=$1 AND deleted_at IS NULL FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(DbError::from)?;
    if live.is_none() {
        return Err(ApiError::not_found());
    }
    let color = match asked {
        Some(color) => color,
        None => next_color(&mut tx, id).await?,
    };
    let tag = new_id();
    hierarchy::create_tag(&mut tx, tag, id, &name, color, ctx.now)
        .await
        .map_err(taken)?;
    let who = actor_name(&ctx)?;
    record_about(
        &ctx,
        &mut tx,
        orch_db::audit::action!(AuditAction::TagCreated),
        tag,
        place,
        &format!("{who} defined the tag {name}"),
        json!({ "tag": tag, "name": name, "color": color }),
    )
    .await?;
    let rows = hierarchy::tags_by_id(&mut tx, &[tag]).await?;
    let shown = shown(&mut tx, &principal, &rows).await?;
    tx.commit().await.map_err(DbError::from)?;
    shown
        .into_iter()
        .next()
        .ok_or_else(|| ApiError::internal("a tag was made and is not there"))
}

/// The tag `id` and the node that defines it.
async fn tag_and_place(conn: &mut Conn, tag: Id) -> ApiResult<(TagRow, Placed, NodeKind)> {
    let row = hierarchy::tags_by_id(conn, &[tag])
        .await?
        .into_iter()
        .next()
        .ok_or_else(ApiError::not_found)?;
    let (place, kind) = placed(conn, row.node_id).await?;
    Ok((row, place, kind))
}

/// Renames a tag or gives it another colour.
async fn tag_update(ctx: Ctx, (): (), body: TagUpdateBody) -> ApiResult<Tag> {
    let id = ctx.id("id")?;
    let name = body.name.as_deref().map(clean_tag_name).transpose()?;
    let color = body.color.map(clean_color).transpose()?;
    let (_ctx, mut tx, principal) = crate::routes::nodes::action(&ctx).await?;
    let (row, _, kind) = tag_and_place(&mut tx, id).await?;
    require_tag_manager(&mut tx, &principal, row.node_id, kind).await?;
    hierarchy::change_tag(&mut tx, id, name.as_deref(), color)
        .await
        .map_err(taken)?;
    let rows = hierarchy::tags_by_id(&mut tx, &[id]).await?;
    let shown = shown(&mut tx, &principal, &rows).await?;
    tx.commit().await.map_err(DbError::from)?;
    shown
        .into_iter()
        .next()
        .ok_or_else(|| ApiError::internal("a tag was changed and is not there"))
}

/// Deletes a tag; it is taken off every place that carries it.
async fn tag_delete(ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let id = ctx.id("id")?;
    let (ctx, mut tx, principal) = crate::routes::nodes::action(&ctx).await?;
    let (row, place, kind) = tag_and_place(&mut tx, id).await?;
    require_tag_manager(&mut tx, &principal, row.node_id, kind).await?;
    let carried = hierarchy::delete_tag(&mut tx, id).await?;
    for node in &carried {
        hierarchy::stamp(&mut tx, *node).await?;
    }
    let who = actor_name(&ctx)?;
    record_about(
        &ctx,
        &mut tx,
        orch_db::audit::action!(AuditAction::TagDeleted),
        id,
        place,
        &format!("{who} deleted the tag {}", row.name),
        json!({ "tag": id, "name": row.name, "places": carried.len() }),
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

// ---- groups ----

/// The most characters of a group's name: what the schema allows.
const GROUP_NAME_MAX_CHARS: usize = 60;

fn clean_group_name(name: &str) -> ApiResult<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > GROUP_NAME_MAX_CHARS {
        return Err(invalid(
            "name",
            &format!("must be 1 to {GROUP_NAME_MAX_CHARS} characters"),
        ));
    }
    Ok(name.to_owned())
}

fn group_of(row: access::GroupFull) -> Group {
    Group {
        id: row.id,
        org_id: row.org_id,
        place: None,
        name: row.name,
        description: row.description,
        member_count: row.member_count,
    }
}

/// Group labels and counts are built from the current holder graph and visible places.
pub(crate) async fn groups_named(
    conn: &mut Conn,
    principal: &orch_authz::Principal,
    rows: Vec<access::GroupFull>,
) -> ApiResult<Vec<Group>> {
    let orgs: Vec<Id> = rows.iter().map(|row| row.org_id).collect();
    let places =
        crate::routes::nodes::view::qualified_refs(conn, principal, &orgs, Policy::default())
            .await?;
    let groups: Vec<Id> = rows.iter().map(|row| row.id).collect();
    let mut counts = HashMap::<Id, i64>::new();
    for (group, _) in access::group_people(conn, &groups, i64::MAX, principal.now().get()).await? {
        *counts.entry(group).or_default() += 1;
    }
    Ok(rows
        .into_iter()
        .map(|row| {
            let mut group = group_of(row);
            group.place = places
                .get(&group.org_id)
                .and_then(|place| place.name.clone());
            group.member_count = counts.get(&group.id).copied().unwrap_or(0);
            group
        })
        .collect())
}

/// A refusal that nobody can lift by asking: what the engine would answer
/// for an act that is shut to everybody.
fn shut(detail: &str, required: &str) -> ApiError {
    ApiError::new(ErrorCode::AuthzDenied, detail)
        .with("required", required)
        .with("object", json!({ "kind": "org", "id": Id::nil() }))
        .with("requestable", false)
        .with("override", false)
        .with("missing", Vec::<serde_json::Value>::new())
}

/// The org `id`, or not found: the address named something else.
async fn the_org(conn: &mut Conn, id: Id) -> ApiResult<Placed> {
    let (place, kind) = placed(conn, id).await?;
    if kind == NodeKind::Org {
        Ok(place)
    } else {
        Err(ApiError::not_found())
    }
}

/// The groups an org keeps.
async fn group_list(ctx: Ctx, query: OrgGroupListQuery, (): ()) -> ApiResult<Vec<Group>> {
    let id = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    the_org(&mut conn, id).await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let rows = access::groups_of_orgs(&mut conn, id, query.under.unwrap_or(false)).await?;
    groups_named(&mut conn, &principal, rows).await
}

/// A project or topic uses the groups its nearest org keeps; a private org discloses none.
async fn place_groups(ctx: Ctx, (): (), (): ()) -> ApiResult<Vec<Group>> {
    let id = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let rows = hierarchy::with_ancestors(&mut conn, &[id]).await?;
    let Some(node) = rows.iter().find(|row| row.id == id && row.kind != "thread") else {
        return Err(ApiError::not_found());
    };
    let Some(org) = rows
        .iter()
        .filter(|row| row.kind == "org" && node.path.contains(&row.id))
        .max_by_key(|row| row.path.len())
    else {
        return Ok(Vec::new());
    };
    if authorization::require(
        &mut conn,
        &principal,
        org.id,
        authorization::VIEW,
        Act::Use,
        Policy::default(),
    )
    .await
    .is_err()
    {
        return Ok(Vec::new());
    }
    let groups = access::groups_of_orgs(&mut conn, org.id, false).await?;
    groups_named(&mut conn, &principal, groups).await
}

fn group_taken(error: DbError) -> ApiError {
    match error {
        DbError::Unique { .. } => ApiError::new(
            ErrorCode::ConflictExists,
            "a group of that name exists already here",
        ),
        other => other.into(),
    }
}

/// Makes a group in an org.
async fn group_create(ctx: Ctx, (): (), body: OrgGroupCreateBody) -> ApiResult<Group> {
    let id = ctx.id("id")?;
    let name = clean_group_name(&body.name)?;
    let description = body.description.unwrap_or_default();
    let (ctx, mut tx, principal) = crate::routes::nodes::action(&ctx).await?;
    let place = the_org(&mut tx, id).await?;
    let group = new_id();
    access::create_group(
        &mut tx,
        group,
        id,
        &name,
        description.trim(),
        ctx.user()?.id(),
        ctx.now,
    )
    .await
    .map_err(group_taken)?;
    let who = actor_name(&ctx)?;
    record_about(
        &ctx,
        &mut tx,
        orch_db::audit::action!(AuditAction::GroupCreated),
        group,
        place,
        &format!("{who} made the group {name}"),
        json!({ "group": group, "name": name }),
    )
    .await?;
    let rows = access::groups_full(&mut tx, &[group]).await?;
    let made = groups_named(&mut tx, &principal, rows).await?;
    tx.commit().await.map_err(DbError::from)?;
    made.into_iter()
        .next()
        .ok_or_else(|| ApiError::internal("a group was made and is not there"))
}

/// The group `id` and where its org lies.
async fn the_group(conn: &mut Conn, id: Id) -> ApiResult<(access::GroupFull, Placed)> {
    let group = access::groups_full(conn, &[id])
        .await?
        .into_iter()
        .next()
        .ok_or_else(ApiError::not_found)?;
    let place = the_org(conn, group.org_id).await?;
    Ok((group, place))
}

/// Requires `permission` on the org that keeps a group.
async fn require_on_keeper(
    conn: &mut Conn,
    principal: &orch_authz::Principal,
    org: Id,
    permission: orch_access_contract::permissions::PermissionName,
) -> ApiResult<()> {
    authorization::require(
        conn,
        principal,
        org,
        permission,
        Act::Use,
        Policy::default(),
    )
    .await
}

/// Renames a group or describes it anew.
async fn group_update(ctx: Ctx, (): (), body: GroupUpdateBody) -> ApiResult<Group> {
    let id = ctx.id("id")?;
    let name = body.name.as_deref().map(clean_group_name).transpose()?;
    let (ctx, mut tx, principal) = crate::routes::nodes::action(&ctx).await?;
    let (group, place) = the_group(&mut tx, id).await?;
    require_on_keeper(
        &mut tx,
        &principal,
        group.org_id,
        exact(Permission::MembersManage),
    )
    .await?;
    access::change_group(
        &mut tx,
        id,
        name.as_deref(),
        body.description.as_deref().map(str::trim),
        ctx.now,
    )
    .await
    .map_err(group_taken)?;
    let who = actor_name(&ctx)?;
    record_about(
        &ctx,
        &mut tx,
        orch_db::audit::action!(AuditAction::GroupUpdated),
        id,
        place,
        &format!(
            "{who} changed the group {}",
            name.as_deref().unwrap_or(&group.name)
        ),
        json!({ "group": id, "name": name }),
    )
    .await?;
    let rows = access::groups_full(&mut tx, &[id]).await?;
    let changed = groups_named(&mut tx, &principal, rows).await?;
    tx.commit().await.map_err(DbError::from)?;
    changed
        .into_iter()
        .next()
        .ok_or_else(|| ApiError::internal("a group was changed and is not there"))
}

/// Deletes a group, with the roles it holds and the entries that name it.
async fn group_delete(ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let id = ctx.id("id")?;
    let (ctx, mut tx, principal) = crate::routes::nodes::action(&ctx).await?;
    let (group, place) = the_group(&mut tx, id).await?;
    require_on_keeper(
        &mut tx,
        &principal,
        group.org_id,
        exact(Permission::MembersManage),
    )
    .await?;
    let (roles, entries) = access::delete_group(&mut tx, id, principal.user(), ctx.now).await?;
    orch_authz::epoch::bump(&mut tx)
        .await
        .map_err(ApiError::internal)?;
    let who = actor_name(&ctx)?;
    record_about(
        &ctx,
        &mut tx,
        orch_db::audit::action!(AuditAction::GroupDeleted),
        id,
        place,
        &format!("{who} deleted the group {}", group.name),
        json!({ "group": id, "name": group.name, "roles": roles, "entries": entries }),
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

/// The people in a group.
async fn group_member_list(ctx: Ctx, (): (), (): ()) -> ApiResult<Vec<UserRef>> {
    let id = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let (group, place) = the_group(&mut conn, id).await?;
    require_on_keeper(&mut conn, &principal, group.org_id, authorization::VIEW).await?;
    let ids: Vec<Id> = access::group_people(&mut conn, &[id], i64::MAX, principal.now().get())
        .await?
        .into_iter()
        .map(|(_, user)| user)
        .collect();
    let mut people =
        crate::routes::access::page::user_refs_at(&mut conn, &principal, Some(place.org), &ids)
            .await?;
    Ok(ids.iter().filter_map(|user| people.remove(user)).collect())
}

fn group_holder(row: &access::GroupHolderRow) -> ApiResult<orch_access_contract::pages::Holder> {
    use orch_access_contract::pages::Holder;
    Ok(if let Some(id) = row.user_id {
        Holder::User { id }
    } else if let Some(id) = row.member_group_id {
        Holder::Group { id }
    } else if let Some(id) = row.member_org_id {
        Holder::Members { id }
    } else if let (Some(id), Some(role)) = (row.member_node_id, row.member_role.as_deref()) {
        Holder::Role {
            at: Some(id),
            role: role.parse().map_err(ApiError::internal)?,
        }
    } else {
        return Err(ApiError::internal("a group member has no holder"));
    })
}

async fn group_holder_list(
    ctx: Ctx,
    (): (),
    (): (),
) -> ApiResult<Vec<orch_api_contract::access::Holder>> {
    let id = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let (group, place) = require_group(&mut conn, &principal, id, authorization::VIEW).await?;
    let members = access::group_holders(&mut conn, id)
        .await?
        .iter()
        .map(group_holder)
        .collect::<ApiResult<Vec<_>>>()?;
    crate::routes::access::page::Names::holders(
        &mut conn,
        &principal,
        Some(place.org),
        Some(group.org_id),
        &members,
    )
    .await
}

async fn require_group(
    conn: &mut Conn,
    principal: &orch_authz::Principal,
    id: Id,
    permission: orch_access_contract::permissions::PermissionName,
) -> ApiResult<(access::GroupFull, Placed)> {
    let (group, place) = the_group(conn, id).await?;
    require_on_keeper(conn, principal, group.org_id, permission).await?;
    Ok((group, place))
}

fn group_holder_input(
    input: &orch_api_contract::access::HolderInput,
    place: Id,
) -> ApiResult<access::GroupHolderRow> {
    use orch_api_contract::access::HolderInput;
    let mut row = access::GroupHolderRow {
        user_id: None,
        member_group_id: None,
        member_node_id: None,
        member_org_id: None,
        member_role: None,
    };
    match input {
        HolderInput::User { id } => row.user_id = Some(*id),
        HolderInput::Group { id } => row.member_group_id = Some(*id),
        HolderInput::Members { id } => row.member_org_id = Some(*id),
        HolderInput::Role { role, at } => {
            row.member_node_id = Some(at.unwrap_or(place));
            row.member_role = Some(
                serde_json::to_value(role)
                    .map_err(ApiError::internal)?
                    .as_str()
                    .ok_or_else(|| invalid("role", "is not a role"))?
                    .to_owned(),
            );
        }
        _ => {
            return Err(invalid(
                "holder",
                "a group contains people, groups, roles or members of an org",
            ));
        }
    }
    Ok(row)
}

async fn group_holder_add(
    ctx: Ctx,
    (): (),
    input: orch_api_contract::access::HolderInput,
) -> ApiResult<()> {
    let id = ctx.id("id")?;
    let (ctx, mut tx, principal) = crate::routes::nodes::action(&ctx).await?;
    let (group, place) =
        require_group(&mut tx, &principal, id, exact(Permission::MembersManage)).await?;
    let member = group_holder_input(&input, group.org_id)?;
    if let Some(user) = member.user_id
        && identity::users_brief(&mut tx, &[user]).await?.is_empty()
    {
        return Err(invalid("id", "is no person"));
    }
    let source = if let Some(nested) = member.member_group_id {
        let rows = access::groups_full(&mut tx, &[nested]).await?;
        let nested_group = rows.first().ok_or_else(|| invalid("id", "is no group"))?;
        if !access::group_edge_allowed(&mut tx, id, nested).await? {
            return Err(invalid(
                "holder",
                "group nesting cannot contain a cycle or be deeper than five groups",
            ));
        }
        Some(nested_group.org_id)
    } else {
        member.member_node_id.or(member.member_org_id)
    };
    if let Some(source) = source {
        let (_, kind) = placed(&mut tx, source).await?;
        if kind == NodeKind::Thread || (member.member_org_id.is_some() && kind != NodeKind::Org) {
            return Err(invalid("id", "is no place of the requested kind"));
        }
        authorization::require(
            &mut tx,
            &principal,
            source,
            authorization::VIEW,
            Act::Use,
            Policy::default(),
        )
        .await?;
    }
    let stored = group_holder(&member)?;
    if orch_authz::external::any_outside(&mut tx, place.org, &[stored])
        .await
        .map_err(ApiError::internal)?
    {
        let policy = crate::routes::nodes::policy_at(&mut tx, group.org_id).await?;
        crate::routes::access::outsiders_matter(&mut tx, &principal, group.org_id, policy).await?;
    }
    access::add_group_holder(&mut tx, id, &member, principal.user(), ctx.now).await?;
    orch_authz::epoch::bump(&mut tx)
        .await
        .map_err(ApiError::internal)?;
    record_about(
        &ctx,
        &mut tx,
        orch_db::audit::action!(AuditAction::GroupMemberAdded),
        id,
        place,
        &format!("{} added a member to {}", actor_name(&ctx)?, group.name),
        json!({ "group": id, "member": input }),
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

async fn group_holder_remove(
    ctx: Ctx,
    (): (),
    input: orch_api_contract::access::HolderInput,
) -> ApiResult<()> {
    let id = ctx.id("id")?;
    let (ctx, mut tx, principal) = crate::routes::nodes::action(&ctx).await?;
    let (group, place) =
        require_group(&mut tx, &principal, id, exact(Permission::MembersManage)).await?;
    let member = group_holder_input(&input, group.org_id)?;
    if access::remove_group_holder(&mut tx, id, &member).await? {
        orch_authz::epoch::bump(&mut tx)
            .await
            .map_err(ApiError::internal)?;
        record_about(
            &ctx,
            &mut tx,
            orch_db::audit::action!(AuditAction::GroupMemberRemoved),
            id,
            place,
            &format!("{} removed a member from {}", actor_name(&ctx)?, group.name),
            json!({ "group": id, "member": input }),
        )
        .await?;
    }
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

/// Puts a member of the group's root org into the group.
async fn group_member_add(ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let id = ctx.id("id")?;
    let user = ctx.id("user_id")?;
    let (ctx, mut tx, principal) = crate::routes::nodes::action(&ctx).await?;
    let (group, place) = the_group(&mut tx, id).await?;
    require_on_keeper(
        &mut tx,
        &principal,
        group.org_id,
        exact(Permission::MembersManage),
    )
    .await?;
    if access::members_among(&mut tx, group.root_id, &[user])
        .await?
        .is_empty()
    {
        return Err(invalid(
            "user_id",
            "is no member of the root org of this group",
        ));
    }
    if access::add_group_member(&mut tx, id, user, principal.user(), ctx.now).await? {
        orch_authz::epoch::bump(&mut tx)
            .await
            .map_err(ApiError::internal)?;
        let who = actor_name(&ctx)?;
        record_about(
            &ctx,
            &mut tx,
            AuditAction::GroupMemberAdded,
            id,
            place,
            &format!("{who} put somebody into the group {}", group.name),
            json!({ "group": id, "user": user }),
        )
        .await?;
    }
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

/// Takes somebody out of a group.
async fn group_member_remove(ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let id = ctx.id("id")?;
    let user = ctx.id("user_id")?;
    let (ctx, mut tx, principal) = crate::routes::nodes::action(&ctx).await?;
    let (group, place) = the_group(&mut tx, id).await?;
    require_on_keeper(
        &mut tx,
        &principal,
        group.org_id,
        exact(Permission::MembersManage),
    )
    .await?;
    if access::remove_group_member(&mut tx, id, user).await? {
        orch_authz::epoch::bump(&mut tx)
            .await
            .map_err(ApiError::internal)?;
        let who = actor_name(&ctx)?;
        record_about(
            &ctx,
            &mut tx,
            AuditAction::GroupMemberRemoved,
            id,
            place,
            &format!("{who} took somebody out of the group {}", group.name),
            json!({ "group": id, "user": user }),
        )
        .await?;
    }
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

// ---- members and suspension ----

/// A member as the API shows them. `manages` says whether the reader may
/// be told who is suspended. Somebody whose person or domain is not among
/// those read is left out.
fn member_of(
    row: &access::MemberRow,
    people: &HashMap<Id, UserRef>,
    refs: &HashMap<Id, NodeRef>,
    manages: bool,
) -> Option<Member> {
    Some(Member {
        user: people.get(&row.user_id)?.clone(),
        domain: refs.get(&row.domain_id)?.clone(),
        address: row.address.clone(),
        // An owner holds no row for a role at their domain, and counts as an
        // Admin there; anybody else without one is shown the lowest role.
        role: row
            .role
            .as_deref()
            .and_then(|role| serde_json::from_value(json!(role)).ok())
            .unwrap_or(if row.is_owner {
                orch_api_contract::access::Role::Admin
            } else {
                orch_api_contract::access::Role::Viewer
            }),
        is_owner: row.is_owner,
        since: Some(orch_types::Time::new(row.since)),
        also: row.also,
        suspended: manages && row.suspended,
        // What only the managers of the members read is filled in for them by the list.
        second_step: None,
        second_step_required: None,
        last_seen_at: None,
    })
}

/// The members of an org, at whatever level it lies.
async fn member_list(ctx: Ctx, query: OrgMemberListQuery, (): ()) -> ApiResult<Page<Member>> {
    let id = ctx.id("id")?;
    let page = PageQuery {
        limit: query.limit,
        cursor: query.cursor.clone(),
    };
    let limit = usize::try_from(page.limit()).unwrap_or(usize::MAX);
    let after = match query.cursor.as_deref() {
        None => None,
        Some(text) => {
            let bad = || invalid("cursor", "is not a cursor of this list");
            let (name, user) = text.rsplit_once('|').ok_or_else(bad)?;
            Some((name.to_owned(), user.parse::<Id>().map_err(|_| bad())?))
        }
    };
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    the_org(&mut conn, id).await?;
    let manages = orch_authz::nodes::check(
        &mut conn,
        &principal,
        id,
        Permission::MembersManage,
        Act::Use,
        Policy::default(),
    )
    .await
    .is_ok_and(|decision| decision.is_allowed());
    let batch = i64::try_from(limit + 1).unwrap_or(i64::MAX);
    let mut rows = access::members_of(
        &mut conn,
        id,
        query.direct.unwrap_or(false),
        query.q.as_deref().filter(|text| !text.is_empty()),
        after.as_ref().map(|(name, user)| (name.as_str(), *user)),
        batch,
        ctx.now,
    )
    .await?;
    let more = rows.len() > limit;
    rows.truncate(limit);
    let mut users: Vec<Id> = rows.iter().map(|row| row.user_id).collect();
    users.sort_unstable();
    let people = refusals::user_refs(&mut conn, &users).await?;
    let domains: Vec<Id> = rows.iter().map(|row| row.domain_id).collect();
    let refs = crate::routes::nodes::view::refs(&mut conn, &principal, &domains, Policy::default())
        .await?;
    let next = rows.last().and_then(|row| {
        people
            .get(&row.user_id)
            .map(|user| format!("{}|{}", user.username, user.id))
    });
    let mut items: Vec<Member> = rows
        .iter()
        .filter_map(|row| member_of(row, &people, &refs, manages))
        .collect();
    if manages {
        members::fill_managed(&mut conn, id, &mut items).await?;
    }
    Ok(Page {
        next_cursor: more.then_some(next).flatten(),
        items,
    })
}

/// A suspension as the API shows it.
async fn suspensions_shown(
    conn: &mut Conn,
    rows: &[access::SuspensionRow],
) -> ApiResult<Vec<Suspension>> {
    let mut ids: Vec<Id> = rows.iter().map(|row| row.user_id).collect();
    ids.extend(rows.iter().map(|row| row.suspended_by));
    let people = refusals::user_refs(conn, &ids).await?;
    Ok(rows
        .iter()
        .filter_map(|row| {
            Some(Suspension {
                user: people.get(&row.user_id)?.clone(),
                outside: row.outside,
                reason: row.reason.clone(),
                by: people.get(&row.suspended_by)?.clone(),
                at: orch_types::Time::new(row.suspended_at),
                owns: row.owns,
            })
        })
        .collect())
}

/// The most characters of the reason of a suspension: what the schema allows.
const REASON_MAX_CHARS: usize = 500;

/// Holds a member out of an org, and everything inside it.
async fn member_suspend(ctx: Ctx, (): (), body: OrgMemberSuspendBody) -> ApiResult<Suspension> {
    let id = ctx.id("id")?;
    let user = ctx.id("user_id")?;
    let reason = body.reason.unwrap_or_default();
    if reason.chars().count() > REASON_MAX_CHARS {
        return Err(invalid(
            "reason",
            &format!("must be at most {REASON_MAX_CHARS} characters"),
        ));
    }
    let (ctx, mut tx, principal) = crate::routes::nodes::action(&ctx).await?;
    let place = the_org(&mut tx, id).await?;
    require_on_keeper(&mut tx, &principal, id, exact(Permission::MembersManage)).await?;
    if user == principal.user() {
        return Err(shut("nobody suspends themself", "members.manage"));
    }
    let target = identity::users_brief(&mut tx, &[user]).await?;
    let Some(target) = target.first() else {
        return Err(ApiError::not_found());
    };
    let admins = access::system_admin_people(&mut tx, orch_authz::holders::SCAN_MOST).await?;
    if admins.contains(&user) {
        return Err(shut(
            "a system admin is not suspended in an org",
            "members.manage",
        ));
    }
    if access::owns_page(&mut tx, id, user).await? {
        return Err(shut(
            "an owner of the org is not suspended in it",
            "members.manage",
        ));
    }
    let outside = access::belongs_in(&mut tx, id, user)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let _ = outside;
    access::suspend_member(&mut tx, id, user, reason.trim(), principal.user(), ctx.now)
        .await
        .map_err(|error| match error {
            DbError::Unique { .. } => ApiError::new(
                ErrorCode::ConflictExists,
                "they are held out of this org already",
            ),
            other => other.into(),
        })?;
    orch_authz::epoch::bump(&mut tx)
        .await
        .map_err(ApiError::internal)?;
    let who = actor_name(&ctx)?;
    record(
        &ctx,
        &mut tx,
        orch_db::audit::action!(AuditAction::MemberSuspended),
        place,
        &format!("{who} suspended {} in this org", target.display_name),
        json!({ "user": user, "reason": reason.trim() }),
    )
    .await?;
    let rows = access::suspensions_in(&mut tx, id, Some(user)).await?;
    let shown = suspensions_shown(&mut tx, &rows).await?;
    tx.commit().await.map_err(DbError::from)?;
    shown
        .into_iter()
        .next()
        .ok_or_else(|| ApiError::internal("a suspension was made and is not there"))
}

/// Lifts a suspension: everything is as it was.
async fn member_resume(ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let id = ctx.id("id")?;
    let user = ctx.id("user_id")?;
    let (ctx, mut tx, _principal) = crate::routes::nodes::action(&ctx).await?;
    let place = the_org(&mut tx, id).await?;
    if !access::end_suspension(&mut tx, id, user).await? {
        return Err(ApiError::not_found());
    }
    orch_authz::epoch::bump(&mut tx)
        .await
        .map_err(ApiError::internal)?;
    let who = actor_name(&ctx)?;
    record(
        &ctx,
        &mut tx,
        orch_db::audit::action!(AuditAction::MemberResumed),
        place,
        &format!("{who} lifted a suspension in this org"),
        json!({ "user": user }),
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

/// Everybody who is held out of an org now.
async fn suspension_list(ctx: Ctx, (): (), (): ()) -> ApiResult<Vec<Suspension>> {
    let id = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    the_org(&mut conn, id).await?;
    let rows = access::suspensions_in(&mut conn, id, None).await?;
    suspensions_shown(&mut conn, &rows).await
}

// ---- service identities ----

/// The most characters of a service identity's display name: the schema's.
const SERVICE_NAME_MAX_CHARS: usize = 64;

/// A username as users keep it: lower-case letters, digits, hyphens and
/// underscores, three to thirty-two characters, beginning with a letter or a
/// digit.
fn clean_username(name: &str) -> ApiResult<String> {
    let name = name.trim();
    let length = name.chars().count();
    let fit = (3..=32).contains(&length)
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    if fit {
        Ok(name.to_owned())
    } else {
        Err(invalid(
            "username",
            "must be 3 to 32 lower-case letters, digits, hyphens or underscores, beginning with a letter or a digit",
        ))
    }
}

async fn services_shown(
    conn: &mut Conn,
    rows: &[identity::ServiceRow],
) -> ApiResult<Vec<orch_api_contract::directory::ServiceAccount>> {
    let mut ids: Vec<Id> = rows.iter().map(|row| row.id).collect();
    ids.extend(rows.iter().filter_map(|row| row.created_by));
    let people = refusals::user_refs(conn, &ids).await?;
    Ok(rows
        .iter()
        .filter_map(|row| {
            Some(orch_api_contract::directory::ServiceAccount {
                user: people.get(&row.id)?.clone(),
                created_by: row.created_by.and_then(|by| people.get(&by).cloned()),
                created_at: orch_types::Time::new(row.created_at),
                token_count: row.token_count,
            })
        })
        .collect())
}

/// The service identities an org keeps.
async fn service_list(
    ctx: Ctx,
    (): (),
    (): (),
) -> ApiResult<Vec<orch_api_contract::directory::ServiceAccount>> {
    let id = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    the_org(&mut conn, id).await?;
    let rows = identity::service_accounts_of(&mut conn, id).await?;
    services_shown(&mut conn, &rows).await
}

/// Makes a service identity of an org.
async fn service_create(
    ctx: Ctx,
    (): (),
    body: orch_api_contract::directory::OrgServiceAccountCreateBody,
) -> ApiResult<orch_api_contract::directory::ServiceAccount> {
    let org = ctx.id("id")?;
    let username = clean_username(&body.username)?;
    let display = body.display_name.trim();
    if display.is_empty() || display.chars().count() > SERVICE_NAME_MAX_CHARS {
        return Err(invalid(
            "display_name",
            &format!("must be 1 to {SERVICE_NAME_MAX_CHARS} characters"),
        ));
    }
    let (ctx, mut tx, _principal) = crate::routes::nodes::action(&ctx).await?;
    let place = the_org(&mut tx, org).await?;
    let id = new_id();
    identity::create_service_account(
        &mut tx,
        id,
        org,
        &username,
        display,
        ctx.user()?.id(),
        ctx.now,
    )
    .await
    .map_err(|error| match error {
        DbError::Unique { .. } => {
            ApiError::new(ErrorCode::ConflictExists, "that username is taken")
        }
        other => other.into(),
    })?;
    let who = actor_name(&ctx)?;
    record_about(
        &ctx,
        &mut tx,
        orch_db::audit::action!(AuditAction::ServiceAccountCreated),
        id,
        place,
        &format!("{who} made the service identity {username}"),
        json!({ "user": id, "username": username }),
    )
    .await?;
    let rows = identity::service_accounts_of(&mut tx, org).await?;
    let mine: Vec<identity::ServiceRow> = rows.into_iter().filter(|row| row.id == id).collect();
    let shown = services_shown(&mut tx, &mine).await?;
    tx.commit().await.map_err(DbError::from)?;
    shown
        .into_iter()
        .next()
        .ok_or_else(|| ApiError::internal("a service identity was made and is not there"))
}

/// A service identity of the caller's reach, and the org that keeps it.
async fn the_service(
    conn: &mut Conn,
    principal: &orch_authz::Principal,
    id: Id,
) -> ApiResult<(identity::ServiceRow, Placed)> {
    let row = identity::service_account(conn, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let place = the_org(conn, row.org_id).await?;
    require_on_keeper(
        conn,
        principal,
        row.org_id,
        exact(Permission::MembersManage),
    )
    .await?;
    Ok((row, place))
}

/// Disables a service identity, revokes its tokens, and takes the roles it
/// was given and the entries that name it.
async fn service_delete(ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let id = ctx.id("id")?;
    let (ctx, mut tx, principal) = crate::routes::nodes::action(&ctx).await?;
    let (_, place) = the_service(&mut tx, &principal, id).await?;
    let (roles, entries) =
        identity::disable_service_account(&mut tx, id, principal.user(), ctx.now).await?;
    orch_authz::epoch::bump(&mut tx)
        .await
        .map_err(ApiError::internal)?;
    let who = actor_name(&ctx)?;
    record_about(
        &ctx,
        &mut tx,
        orch_db::audit::action!(AuditAction::ServiceAccountDeleted),
        id,
        place,
        &format!("{who} deleted a service identity"),
        json!({ "user": id, "roles": roles, "entries": entries }),
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

fn token_info(row: identity::ApiTokenListed) -> orch_api_contract::auth::ApiTokenInfo {
    orch_api_contract::auth::ApiTokenInfo {
        id: row.id,
        name: row.name,
        scopes: row.scopes,
        node: row.node_id.map(|id| NodeRef {
            thread_display: None,
            id,
            kind: row
                .node_kind
                .as_deref()
                .and_then(refusals::node_kind)
                .unwrap_or(NodeKind::Org),
            name: row.node_name.clone(),
            redacted: false,
        }),
        created_at: orch_types::Time::new(row.created_at),
        last_used_at: row.last_used_at.map(orch_types::Time::new),
        expires_at: row.expires_at.map(orch_types::Time::new),
    }
}

/// The tokens of a service identity: never the secrets.
async fn service_token_list(
    ctx: Ctx,
    (): (),
    (): (),
) -> ApiResult<Vec<orch_api_contract::auth::ApiTokenInfo>> {
    let id = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    the_service(&mut conn, &principal, id).await?;
    Ok(identity::api_tokens_of(&mut conn, id)
        .await?
        .into_iter()
        .map(token_info)
        .collect())
}

/// Makes a token for a service identity, narrow (no scopes, nothing) and with the human maker's fresh
/// proof in this request: its secret is shown once, and the same creation asked again by its key
/// answers what was made, without it.
async fn service_token_create(
    ctx: Ctx,
    (): (),
    body: orch_api_contract::auth::ApiTokenCreateBody,
) -> ApiResult<Answer<orch_api_contract::auth::TokenCreated>> {
    use crate::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
    use crate::routes::auth::tokens;
    use crate::sign_in::fresh::{self, Purpose};
    let authed = std::sync::Arc::clone(ctx.user()?);
    if !authed.is_session() {
        return Err(ApiError::new(
            ErrorCode::AuthSessionRequired,
            "this is done from a browser sign-in",
        ));
    }
    let id = ctx.id("id")?;
    {
        let mut conn = ctx.app.db.acquire().await?;
        if let Some(again) = tokens::replayed(&ctx, &mut conn, authed.id()).await? {
            return Ok(Answer::from(again).header(CACHE_CONTROL, NOT_KEPT));
        }
    }
    let asked = tokens::asked(&body, ctx.now)?;
    // An identity provider's proof for this act names the service identity it is for.
    let proven = fresh::prove(&ctx, &authed, &body.proof, Purpose::TokenCreate)
        .await?
        .for_target(Some(id));
    let (ctx, mut tx, principal) = crate::routes::nodes::action(&ctx).await?;
    let (row, place) = the_service(&mut tx, &principal, id).await?;
    if let Some(node) = body.node_id {
        // The token is kept to a place of the tree that keeps the identity,
        // which the caller may view.
        let (found, _) = placed(&mut tx, node).await?;
        if found.org != place.org {
            return Err(invalid(
                "node_id",
                "is not in the tree of the org that keeps this identity",
            ));
        }
        authorization::require(
            &mut tx,
            &principal,
            node,
            authorization::VIEW,
            Act::Use,
            Policy::default(),
        )
        .await?;
    }
    let bounded_at = orch_settings::Place::Node(body.node_id.unwrap_or(row.org_id));
    tokens::within_lifetime(&mut tx, bounded_at, ctx.now, asked.expires_at).await?;
    proven
        .spend(&ctx, &mut tx, authed.id(), Purpose::TokenCreate)
        .await?;
    let token_id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    let name = asked.name.clone();
    identity::create_api_token(
        &mut tx,
        &identity::NewApiToken {
            id: token_id,
            user_id: id,
            name: &name,
            token_hash: &token_hash(&secret),
            scopes: &asked.scopes,
            node_id: body.node_id,
            at: ctx.now,
            expires_at: asked.expires_at,
        },
    )
    .await?;
    let who = actor_name(&ctx)?;
    record_about(
        &ctx,
        &mut tx,
        AuditAction::AuthTokenCreated,
        row.id,
        place,
        &format!("{who} made the API token {name} for a service identity"),
        json!({ "token": token_id, "user": row.id, "name": name, "scopes": asked.scopes, "node_id": body.node_id }),
    )
    .await?;
    let info = identity::api_token_listed(&mut tx, token_id, id)
        .await?
        .map(token_info)
        .ok_or_else(|| ApiError::internal("a token that was just made is not there"))?;
    tokens::keep(&ctx, &mut tx, authed.id(), &info).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(Answer::from(orch_api_contract::auth::TokenCreated {
        token: Some(secret),
        info,
        already_created: false,
    })
    .header(CACHE_CONTROL, NOT_KEPT))
}

/// Revokes a token of a service identity.
async fn service_token_delete(ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let id = ctx.id("id")?;
    let token = ctx.id("token_id")?;
    let (ctx, mut tx, principal) = crate::routes::nodes::action(&ctx).await?;
    let (_, place) = the_service(&mut tx, &principal, id).await?;
    if !identity::revoke_api_token(&mut tx, token, id, ctx.now).await? {
        return Err(ApiError::not_found());
    }
    orch_authz::epoch::bump(&mut tx)
        .await
        .map_err(ApiError::internal)?;
    let who = actor_name(&ctx)?;
    record_about(
        &ctx,
        &mut tx,
        AuditAction::AuthTokenRevoked,
        id,
        place,
        &format!("{who} revoked an API token of a service identity"),
        json!({ "token": token, "user": id }),
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}
