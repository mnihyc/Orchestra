//! The permission catalogue, roles held in a place, permission pages with their owners and defaults pages with previews, the people behind a holder a page at a time, the one change that names owners and holders at a choice, asks who may name somebody from outside and marks an override, a thing's history, and access requests for everything missing at once.
//!
//! Every decision is the access engine's. A page is read and changed by its
//! functions, in the caller's transaction, so that the rows, the entries,
//! the audit rows, the line of history and the move of the authorization
//! epoch are made together or not at all. What is written here is the
//! translation to and from the wire (names for identifiers, the words of the
//! API for the words of the engine), the roles, which the engine only
//! reads, and the requests that ask about a person's access. The requests
//! that are the same for every kind of thing (`readers`, `previews`) are
//! answered for a node, a resource and a document alike, and a kind that has
//! no page is answered plainly as not there (`subject`); a request for access
//! is made and read in `requests`.

pub mod candidates;
pub mod dependencies;
pub mod page;
pub mod previews;
pub mod readers;
pub mod requests;
pub mod share_preview;
pub mod subject;
pub mod thread_environment;

use crate::middleware::authorization::{self, VIEW, exact, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use crate::routes::nodes::{Placed, actor_name, invalid, placed, policy_at, record, view};
use crate::sign_in::origin;
use chrono::{DateTime, Utc};
use orch_access_contract::audit::AuditAction;
use orch_access_contract::permissions::Permission;
use orch_access_contract::roles::PlaceRef;
use orch_api_contract::access as api;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::nodes::NodeKind;
use orch_authz::giving;
use orch_authz::pages::{self, ChangeError, Target};
use orch_authz::{Act, Policy, Principal, covers, epoch, holders, nodes};
use orch_db::queries::hierarchy;
use orch_db::queries::{access, identity};
use orch_db::{Conn, DbError};
use orch_types::{Id, Time, new_id};
use page::{permission_page, same};
use serde_json::json;
use std::collections::HashMap;

/// The most characters of the note of a role: what the schema allows.
const NOTE_MAX_CHARS: usize = 500;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on(api::PermissionCatalogueGet, catalogue)
        .on(api::HolderSearch, candidates::search)
        .on(api::NodeRoleList, role_list)
        .on(api::NodeRoleCreate, role_create)
        .on(api::RoleUpdate, role_update)
        .on(api::RoleDelete, role_delete)
        .on(api::PermissionPageGet, page_get)
        .on(api::PermissionPageUpdate, page_update)
        .on(api::EffectiveAccessGet, readers::effective)
        .on(api::PermissionPeopleList, readers::people)
        .on(api::HistoryList, readers::history)
        .on(api::NodeDefaultsGet, defaults_get)
        .on(api::NodeDefaultsUpdate, defaults_update)
        .on(api::PermissionPagePreview, previews::page)
        .on(api::SharePreviewCreate, share_preview::create)
        .on(api::NodeDefaultsPreview, previews::node_defaults)
        .on(
            orch_api_contract::admin::AdminDefaultsPreview,
            previews::admin_defaults,
        )
        .on(api::AccessRequestCreate, requests::create)
        .on(api::AccessRequestGet, requests::get);
}

/// The catalogue of permissions, as data.
async fn catalogue(_ctx: Ctx, (): (), (): ()) -> ApiResult<api::PermissionCatalogueGetReply> {
    same(&orch_access_contract::permissions::catalogue())
        .ok_or_else(|| ApiError::internal("the catalogue does not read as the API's"))
}

// ---- roles ----

/// The wire form of roles that were read, with the names of the people,
/// groups, orgs and places they mention.
pub(crate) async fn roles_held(
    conn: &mut Conn,
    principal: &Principal,
    rows: &[access::RoleRow],
    root: Id,
) -> ApiResult<Vec<api::RoleHeld>> {
    let ids: Vec<Id> = rows.iter().map(|row| row.id).collect();
    let flags: HashMap<Id, access::RoleFlags> = access::role_flags(conn, &ids, root)
        .await?
        .into_iter()
        .map(|flags| (flags.id, flags))
        .collect();
    let of_kind = |kind: &str| -> Vec<Id> {
        rows.iter()
            .filter(|row| row.subject_kind == kind)
            .map(|row| row.subject_id)
            .collect()
    };
    let mut people = of_kind("user");
    people.extend(rows.iter().map(|row| row.created_by));
    let people = page::user_refs_at(conn, principal, Some(root), &people).await?;
    let group_rows = access::groups_full(conn, &of_kind("group")).await?;
    let groups: HashMap<Id, orch_api_contract::directory::Group> =
        crate::routes::directory::groups_named(conn, principal, group_rows)
            .await?
            .into_iter()
            .map(|group| (group.id, group))
            .collect();
    let mut places: Vec<Id> = rows.iter().map(|row| row.node_id).collect();
    places.extend(of_kind("members"));
    places.extend(of_kind("role"));
    let refs = view::qualified_refs(conn, principal, &places, Policy::default()).await?;
    rows.iter()
        .map(|row| {
            let subject_kind: api::SubjectKind = match row.subject_kind.as_str() {
                "group" => api::SubjectKind::Group,
                "members" => api::SubjectKind::Members,
                "role" => api::SubjectKind::Role,
                _ => api::SubjectKind::User,
            };
            let flag = flags.get(&row.id);
            Ok(api::RoleHeld {
                id: row.id,
                node: refs
                    .get(&row.node_id)
                    .cloned()
                    .ok_or_else(|| ApiError::internal("the place of a role is not there"))?,
                subject: api::Subject {
                    kind: subject_kind,
                    id: row.subject_id,
                    user: people
                        .get(&row.subject_id)
                        .cloned()
                        .filter(|_| subject_kind == api::SubjectKind::User),
                    group: groups.get(&row.subject_id).cloned(),
                    org: refs
                        .get(&row.subject_id)
                        .cloned()
                        .filter(|_| subject_kind == api::SubjectKind::Members),
                    role: row.subject_role.as_deref().and_then(|word| same(&word)),
                    at: refs
                        .get(&row.subject_id)
                        .cloned()
                        .filter(|_| subject_kind == api::SubjectKind::Role),
                },
                role: same(&row.role)
                    .ok_or_else(|| ApiError::internal("a role is stored that is none"))?,
                domain: flag.is_some_and(|flag| flag.domain),
                outside: flag.is_some_and(|flag| flag.outside),
                expires_at: row.expires_at.map(Time::new),
                note: row.note.clone(),
                created_by: people
                    .get(&row.created_by)
                    .cloned()
                    .ok_or_else(|| ApiError::internal("the giver of a role is not a user"))?,
                created_at: Time::new(row.created_at),
                revision: row.revision,
            })
        })
        .collect()
}

/// A stored role as the engine's contract names it, to be shown: read again
/// from the database.
async fn role_rows(conn: &mut Conn, ids: &[Id]) -> ApiResult<Vec<access::RoleRow>> {
    let mut found = Vec::new();
    for id in ids {
        if let Some(row) = access::role_row(conn, *id).await? {
            found.push(row);
        }
    }
    Ok(found)
}

/// Who holds which role in a place and above it.
async fn role_list(ctx: Ctx, (): (), (): ()) -> ApiResult<api::NodeRoleListReply> {
    let id = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let (place, _) = placed(&mut conn, id).await?;
    let here = holders::roles_here(&mut conn, id, Time::new(ctx.now))
        .await
        .map_err(ApiError::internal)?;
    let ids: Vec<Id> = here
        .own
        .iter()
        .chain(&here.above)
        .map(|held| held.id)
        .collect();
    let rows = role_rows(&mut conn, &ids).await?;
    let held = roles_held(&mut conn, &principal, &rows, place.org).await?;
    let by_id: HashMap<Id, api::RoleHeld> = held.into_iter().map(|held| (held.id, held)).collect();
    let pick = |list: &[orch_access_contract::roles::RoleHeld]| -> Vec<api::RoleHeld> {
        list.iter()
            .filter_map(|each| by_id.get(&each.id).cloned())
            .collect()
    };
    let policy = policy_at(&mut conn, id).await?;
    let standing = giving::role_standing(&mut conn, &principal, id, policy)
        .await
        .map_err(ApiError::internal)?;
    let owner_rows =
        access::owner_entries(&mut conn, &[access::PageKey::new("node", id, "live")]).await?;
    let owner_entries = owner_rows
        .iter()
        .map(orch_authz::engine::entry_of)
        .collect::<Result<Vec<_>, _>>()
        .map_err(ApiError::internal)?;
    let owner_holders: Vec<_> = owner_entries
        .into_iter()
        .map(|entry| entry.holder)
        .collect();
    let owners = page::Names::holders(
        &mut conn,
        &principal,
        Some(place.org),
        Some(id),
        &owner_holders,
    )
    .await?;
    Ok(api::NodeRoleListReply {
        owners,
        own: pick(&here.own),
        above: pick(&here.above),
        can_give: standing.roles().iter().filter_map(same).collect(),
    })
}

fn clean_note(note: Option<&str>) -> ApiResult<String> {
    let note = note.unwrap_or("").trim();
    if note.chars().count() > NOTE_MAX_CHARS {
        return Err(invalid(
            "note",
            &format!("must be at most {NOTE_MAX_CHARS} characters"),
        ));
    }
    Ok(note.to_owned())
}

/// Giving, changing or removing Admin is for the place's owners and for its
/// admins; the engine is asked whether the caller is one of them.
pub(crate) async fn admin_matters(
    conn: &mut Conn,
    principal: &Principal,
    node: Id,
    policy: Policy,
) -> ApiResult<()> {
    let held = nodes::effective(conn, principal, node, policy)
        .await
        .map_err(ApiError::internal)?;
    if held.owner || held.administers {
        return Ok(());
    }
    // Neither: the refusal names what is lacking and who can give it.
    authorization::require(
        conn,
        principal,
        node,
        exact(Permission::AccessOwn),
        Act::Use,
        policy,
    )
    .await?;
    Err(ApiError::new(
        ErrorCode::AuthzDenied,
        "only an owner or an admin of the place gives or removes Admin",
    ))
}

/// Giving, changing or taking away a role is asked of the same rule as naming somebody in a row
/// of a page: the giver holds the role there, or a higher one, and may share the place (they manage
/// its page, or the place lets holders share). Admin is the role only an admin of the place holds,
/// the place's owners among them, and so only an admin gives it. The refusal names what is lacking
/// and who can give it.
pub(crate) async fn role_matters(
    conn: &mut Conn,
    principal: &Principal,
    node: Id,
    role: api::Role,
    standing: &giving::RoleStanding,
    policy: Policy,
) -> ApiResult<()> {
    let stored: orch_access_contract::roles::Role = same(&role)
        .ok_or_else(|| ApiError::internal("a role of the API is none of the engine's"))?;
    if standing.may_give(stored) {
        return Ok(());
    }
    let refusal = giving::role_refusal(conn, principal, node, stored, policy)
        .await
        .map_err(ApiError::internal)?;
    Err(refusals::denied(conn, refusal).await?)
}

/// Checks the current caller's ability to give a role at a place.
pub(crate) async fn require_role_giving(
    conn: &mut Conn,
    principal: &Principal,
    node: Id,
    role: api::Role,
    policy: Policy,
) -> ApiResult<()> {
    let standing = giving::role_standing(conn, principal, node, policy)
        .await
        .map_err(refusals_of)?;
    role_matters(conn, principal, node, role, &standing, policy).await
}

/// Whether the role `row` may be changed or taken back by the caller: whoever manages the place's
/// page may, and whoever gave it may for as long as they could give it again.
async fn role_changeable(
    conn: &mut Conn,
    principal: &Principal,
    row: &access::RoleRow,
    standing: &giving::RoleStanding,
    policy: Policy,
) -> ApiResult<()> {
    let held: Option<orch_access_contract::roles::Role> = same(&row.role);
    let gave =
        row.created_by == principal.user() && held.is_some_and(|role| standing.may_give(role));
    if standing.manages || gave {
        return Ok(());
    }
    // Neither: the refusal is that of a change of the page, which names what is lacking.
    authorization::require(
        conn,
        principal,
        row.node_id,
        exact(Permission::AccessManage),
        Act::Use,
        policy,
    )
    .await?;
    Err(ApiError::new(
        ErrorCode::AuthzDenied,
        "only whoever gave a role, or whoever manages the page of the place, changes or takes it back",
    ))
}

/// A subject that is outside the tree of the place is named only by whoever
/// may name an outsider there.
pub(crate) async fn outsiders_matter(
    conn: &mut Conn,
    principal: &Principal,
    node: Id,
    policy: Policy,
) -> ApiResult<()> {
    match nodes::check(
        conn,
        principal,
        node,
        Permission::AccessExternal,
        Act::Use,
        policy,
    )
    .await
    {
        Ok(orch_access_contract::decisions::Decision::Allowed { .. }) => Ok(()),
        Ok(orch_access_contract::decisions::Decision::Refused(refusal)) => {
            let (ask, more) = refusal.missing.first().map_or_else(
                || (Vec::new(), 0),
                |missing| (missing.ask.clone(), missing.ask_more),
            );
            Err(refusals::external_forbidden(conn, &ask, more).await)
        }
        Err(error) => Err(refusals::problem(conn, error).await),
    }
}

fn expiry(at: Option<Time>, now: DateTime<Utc>) -> ApiResult<Option<DateTime<Utc>>> {
    match at {
        Some(at) if at.get() <= now => Err(invalid("expires_at", "must be in the future")),
        other => Ok(other.map(Time::get)),
    }
}

/// Gives a role in a place.
async fn role_create(ctx: Ctx, (): (), body: api::RoleCreateBody) -> ApiResult<api::RoleHeld> {
    let node = ctx.id("id")?;
    let (ctx, mut tx, principal) = crate::routes::nodes::action(&ctx).await?;
    let held = role_create_in(&ctx, &mut tx, &principal, node, &body).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(held)
}

/// The ordinary role producer, also used inside a preview's rolled-back transaction.
pub(crate) async fn role_create_in(
    ctx: &Ctx,
    tx: &mut Conn,
    principal: &Principal,
    node: Id,
    body: &api::RoleCreateBody,
) -> ApiResult<api::RoleHeld> {
    let note = clean_note(body.note.as_deref())?;
    let ends = expiry(body.expires_at, ctx.now)?;
    let (place, kind) = placed(tx, node).await?;
    if kind == NodeKind::Thread {
        return Err(invalid(
            "id",
            "a role is held on an org, a project or a topic, never on a thread; the people of a thread are named on its page",
        ));
    }
    let policy = policy_at(tx, node).await?;
    // The subject must exist.
    let (subject_kind, subject_root) = match body.subject.kind {
        api::SubjectKind::User => {
            let found = identity::users_brief(tx, &[body.subject.id]).await?;
            if found.is_empty() {
                return Err(invalid("subject.id", "is no user"));
            }
            ("user", None)
        }
        api::SubjectKind::Group => {
            let found = access::groups_full(tx, &[body.subject.id]).await?;
            let Some(group) = found.first() else {
                return Err(invalid("subject.id", "is no group"));
            };
            authorization::require(
                tx,
                principal,
                group.org_id,
                VIEW,
                Act::Use,
                Policy::default(),
            )
            .await?;
            ("group", Some(group.root_id))
        }
        api::SubjectKind::Members => {
            let found = hierarchy::with_ancestors(tx, &[body.subject.id]).await?;
            let org = found
                .iter()
                .find(|row| row.id == body.subject.id && row.kind == "org");
            let Some(org) = org else {
                return Err(invalid("subject.id", "is no org"));
            };
            authorization::require(tx, principal, org.id, VIEW, Act::Use, Policy::default())
                .await?;
            ("members", Some(org.org_id))
        }
        api::SubjectKind::Role => {
            if body.subject.role.is_none() {
                return Err(invalid(
                    "subject.role",
                    "a role at the named place is required",
                ));
            }
            let (source, kind) = placed(tx, body.subject.id).await?;
            if kind == NodeKind::Thread {
                return Err(invalid(
                    "subject.id",
                    "roles are held at an org, project or topic",
                ));
            }
            authorization::require(
                tx,
                principal,
                body.subject.id,
                VIEW,
                Act::Use,
                Policy::default(),
            )
            .await?;
            ("role", Some(source.org))
        }
    };
    if body.subject.kind != api::SubjectKind::Role && body.subject.role.is_some() {
        return Err(invalid(
            "subject.role",
            "only a role subject carries a role",
        ));
    }
    let standing = giving::role_standing(tx, principal, node, policy)
        .await
        .map_err(refusals_of)?;
    role_matters(tx, principal, node, body.role, &standing, policy).await?;
    let outside = match subject_root {
        Some(root) => root != place.org,
        None => {
            let members = access::members_among(tx, place.org, &[body.subject.id]).await?;
            members.is_empty()
        }
    };
    // A person naming themself is not naming an outsider (an admin of the
    // system is a member of no tree).
    if outside && !(body.subject.id == principal.user() && subject_kind == "user") {
        outsiders_matter(tx, principal, node, policy).await?;
    }
    let role_word: String = same(&body.role).unwrap_or_default();
    let subject_role: Option<String> = body.subject.role.as_ref().and_then(same);
    let id = new_id();
    access::put_role(
        tx,
        &access::NewRole {
            id,
            node,
            subject_kind,
            subject: body.subject.id,
            subject_role: subject_role.as_deref(),
            role: &role_word,
            expires_at: ends,
            note: &note,
            by: principal.user(),
            at: ctx.now,
        },
    )
    .await
    .map_err(|error| match error {
        DbError::Unique { .. } => ApiError::new(
            ErrorCode::ConflictExists,
            "that subject holds this role here already, or a lasting role when this one has no end",
        ),
        other => other.into(),
    })?;
    epoch::bump(tx).await.map_err(ApiError::internal)?;
    let who = actor_name(ctx)?;
    let named = place_name(tx, node).await?;
    record_role(
        ctx,
        tx,
        orch_db::audit::action!(AuditAction::RoleGiven),
        place,
        &format!("{who} gave {role_word} in {named}"),
        json!({ "role": role_word, "subject": { "kind": subject_kind, "id": body.subject.id, "role": subject_role }, "role_id": id }),
    )
    .await?;
    let rows = role_rows(tx, &[id]).await?;
    let held = roles_held(tx, principal, &rows, place.org).await?;
    held.into_iter()
        .next()
        .ok_or_else(|| ApiError::internal("a role was given and is not there"))
}

async fn place_name(conn: &mut Conn, node: Id) -> ApiResult<String> {
    Ok(hierarchy::with_ancestors(conn, &[node])
        .await?
        .into_iter()
        .find(|row| row.id == node)
        .map(|row| row.name)
        .unwrap_or_default())
}

/// The audit row of a role: its object is the place it is held in.
async fn record_role(
    ctx: &Ctx,
    conn: &mut Conn,
    action: AuditAction,
    place: Placed,
    summary: &str,
    details: serde_json::Value,
) -> ApiResult<()> {
    record(ctx, conn, action, place, summary, details).await?;
    hierarchy::stamp(conn, place.id).await?;
    Ok(())
}

/// The role `id` that was given and not taken away, with where it is held.
async fn the_role(conn: &mut Conn, id: Id) -> ApiResult<(access::RoleRow, Placed)> {
    let row = access::role_row(conn, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let (place, _) = placed(conn, row.node_id).await?;
    Ok((row, place))
}

/// Changes a role that was given.
async fn role_update(ctx: Ctx, (): (), body: api::RoleUpdateBody) -> ApiResult<api::RoleHeld> {
    let id = ctx.id("id")?;
    let note = body
        .note
        .as_deref()
        .map(|note| clean_note(Some(note)))
        .transpose()?;
    let (ctx, mut tx, principal) = crate::routes::nodes::action(&ctx).await?;
    let ends = match body.expires_at {
        None => None,
        Some(None) => Some(None),
        Some(Some(at)) => Some(expiry(Some(at), ctx.now)?),
    };
    let new_role: Option<String> = body.role.map(|role| same(&role).unwrap_or_default());
    let (row, place) = the_role(&mut tx, id).await?;
    let policy = policy_at(&mut tx, row.node_id).await?;
    let standing = giving::role_standing(&mut tx, &principal, row.node_id, policy)
        .await
        .map_err(refusals_of)?;
    role_changeable(&mut tx, &principal, &row, &standing, policy).await?;
    // Taking away administration needs the current administrator identity;
    // extending a role or raising it also needs every permission it gives.
    if (row.role == "admin" || new_role.as_deref() == Some("admin")) && !standing.is_admin() {
        role_matters(
            &mut tx,
            &principal,
            row.node_id,
            api::Role::Admin,
            &standing,
            policy,
        )
        .await?;
    }
    let raises = body.role.is_some_and(|new| {
        same::<_, orch_access_contract::roles::Role>(&new)
            .zip(same::<_, orch_access_contract::roles::Role>(&row.role))
            .is_some_and(|(new, old)| new > old)
    });
    let extends = ends.is_some_and(|after| match (after, row.expires_at) {
        (None, Some(_)) => true,
        (Some(after), Some(before)) => after > before,
        _ => false,
    });
    if raises || extends {
        let resulting = body
            .role
            .or_else(|| same(&row.role))
            .ok_or_else(|| ApiError::internal("a stored role does not read"))?;
        role_matters(
            &mut tx,
            &principal,
            row.node_id,
            resulting,
            &standing,
            policy,
        )
        .await?;
    }
    let changed = access::change_role(
        &mut tx,
        id,
        &access::RoleChange {
            role: new_role.as_deref(),
            expires_at: ends,
            note: note.as_deref(),
        },
        body.expected_revision,
    )
    .await?;
    let Some(changed) = changed else {
        let current = roles_held(&mut tx, &principal, &[row], place.org).await?;
        return Err(ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "the role was changed since it was read",
        )
        .with("current", current.first()));
    };
    epoch::bump(&mut tx).await.map_err(ApiError::internal)?;
    let who = actor_name(&ctx)?;
    let named = place_name(&mut tx, row.node_id).await?;
    record_role(
        &ctx,
        &mut tx,
        orch_db::audit::action!(AuditAction::RoleChanged),
        place,
        &format!("{who} changed a role in {named}"),
        json!({ "role_id": id, "from": row.role, "to": changed.role }),
    )
    .await?;
    let held = roles_held(&mut tx, &principal, &[changed], place.org).await?;
    tx.commit().await.map_err(DbError::from)?;
    held.into_iter()
        .next()
        .ok_or_else(|| ApiError::internal("a role was changed and is not there"))
}

/// Takes a role away. The role a member holds at their domain stays until
/// their membership ends or they are moved.
async fn role_delete(ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let id = ctx.id("id")?;
    let (ctx, mut tx, principal) = crate::routes::nodes::action(&ctx).await?;
    let (row, place) = the_role(&mut tx, id).await?;
    let policy = policy_at(&mut tx, row.node_id).await?;
    let standing = giving::role_standing(&mut tx, &principal, row.node_id, policy)
        .await
        .map_err(refusals_of)?;
    role_changeable(&mut tx, &principal, &row, &standing, policy).await?;
    if row.role == "admin" && !standing.is_admin() {
        role_matters(
            &mut tx,
            &principal,
            row.node_id,
            api::Role::Admin,
            &standing,
            policy,
        )
        .await?;
    }
    let flags = access::role_flags(&mut tx, &[id], place.org).await?;
    if flags.first().is_some_and(|flags| flags.domain) {
        return Err(ApiError::new(
            ErrorCode::MemberDomainRole,
            "the role a member holds at their domain is changed, and ends with the membership",
        ));
    }
    access::remove_role(&mut tx, id, principal.user(), ctx.now).await?;
    epoch::bump(&mut tx).await.map_err(ApiError::internal)?;
    let who = actor_name(&ctx)?;
    let named = place_name(&mut tx, row.node_id).await?;
    record_role(
        &ctx,
        &mut tx,
        orch_db::audit::action!(AuditAction::RoleRemoved),
        place,
        &format!("{who} took a role away in {named}"),
        json!({ "role_id": id, "role": row.role }),
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

// ---- pages ----

/// Which page of a node a request is about.
#[derive(Clone, Copy)]
enum Which {
    Live,
    Defaults,
}

impl Which {
    fn target(self, node: Id) -> Target {
        match self {
            Self::Live => Target::Live(node),
            Self::Defaults => Target::Defaults(PlaceRef::Node(node)),
        }
    }
}

/// A defaults page belongs to an org, a project or a topic.
fn check_which(which: Which, kind: NodeKind) -> ApiResult<()> {
    if matches!(which, Which::Defaults) && kind == NodeKind::Thread {
        return Err(invalid(
            "id",
            "a thread has no defaults: it gives nothing to what is made inside it",
        ));
    }
    Ok(())
}

async fn read_page(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    node: Id,
    root: Id,
    which: Which,
) -> ApiResult<api::PermissionPage> {
    let policy = policy_at(conn, node).await?;
    let page = pages::read(conn, principal, which.target(node), policy)
        .await
        .map_err(|error| ApiError::internal(error.to_string()))?;
    let mut shown = permission_page(conn, principal, &page, Some(root), Time::new(ctx.now)).await?;
    // The live page of a place says what a role held there, or a share of it with what is in it, reaches.
    if matches!(which, Which::Live)
        && let Some(counted) = covers::of_place(conn, principal, node, policy)
            .await
            .map_err(ApiError::internal)?
    {
        let counts = |each: covers::Counts| api::PageCounts {
            places: each.places,
            threads: each.threads,
            documents: each.documents,
        };
        shown.covers = Some(api::PageCovers {
            places: counted.follows.places,
            threads: counted.follows.threads,
            documents: counted.follows.documents,
            apart: counts(counted.apart),
        });
    }
    // The live page of a thread says what it reaches: its environment, whose files are read by
    // whoever looks into it, and what sharing the thread offers beside it.
    if matches!(which, Which::Live) && shown.object.kind == "thread" {
        thread_environment::show(conn, principal, node, policy, &mut shown).await?;
    }
    Ok(shown)
}

/// What went wrong in asking the engine about a role, as the answer a client gets.
fn refusals_of(error: orch_authz::Error) -> ApiError {
    match error {
        orch_authz::Error::NotFound(_) => ApiError::not_found(),
        orch_authz::Error::Suspended { org: None } => {
            ApiError::new(ErrorCode::AuthSuspended, "this user is suspended")
        }
        other => ApiError::internal(other),
    }
}

async fn get_page(ctx: &Ctx, which: Which) -> ApiResult<api::PermissionPage> {
    let id = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(ctx, &mut conn).await?;
    match which {
        Which::Live => {
            subject::require_page(
                &mut conn,
                &principal,
                orch_authz::things::Thing::Node(id),
                Policy::default(),
            )
            .await?;
        }
        Which::Defaults => {
            authorization::require(&mut conn, &principal, id, VIEW, Act::Use, Policy::default())
                .await?;
        }
    }
    let (place, kind) = placed(&mut conn, id).await?;
    check_which(which, kind)?;
    read_page(ctx, &mut conn, &principal, id, place.org, which).await
}

/// Reads a permission page.
async fn page_get(ctx: Ctx, (): (), (): ()) -> ApiResult<api::PermissionPage> {
    if matches!(ctx.param("object")?, "documents" | "document-containers") {
        return crate::routes::documents::page::get(&ctx).await;
    }
    if ctx.param("object")? != "nodes" {
        return crate::routes::resources::page::get(&ctx).await;
    }
    get_page(&ctx, Which::Live).await
}

/// Reads what a place gives to what is made inside it.
async fn defaults_get(ctx: Ctx, (): (), (): ()) -> ApiResult<api::PermissionPage> {
    get_page(&ctx, Which::Defaults).await
}

async fn change_page(
    ctx: &Ctx,
    which: Which,
    body: &api::PageChange,
) -> ApiResult<api::PermissionPage> {
    let id = ctx.id("id")?;
    let (change, people) = crate::routes::notifications::person::page_change(body)?;
    if matches!(which, Which::Defaults) && !people.is_empty() {
        return Err(invalid(
            "notify",
            "is available on a thing's own permission page",
        ));
    }
    let mut tx = ctx.app.db.begin().await?;
    let principal = authorization::principal(ctx, &mut tx).await?;
    let (place, kind) = placed(&mut tx, id).await?;
    check_which(which, kind)?;
    let policy = policy_at(&mut tx, id).await?;
    if crate::routes::notifications::person::replay_page(ctx, &mut tx, &principal, body).await? {
        subject::require_page(
            &mut tx,
            &principal,
            orch_authz::things::Thing::Node(id),
            policy,
        )
        .await?;
        let page = read_page(ctx, &mut tx, &principal, id, place.org, which).await?;
        tx.commit().await.map_err(DbError::from)?;
        return Ok(page);
    }
    // A thread's own page: its history says that it changed, and who is first now.
    let first_before = if matches!(which, Which::Live) && kind == NodeKind::Thread {
        orch_db::queries::threads::thread(&mut tx, id)
            .await?
            .map(|row| row.owner_id)
    } else {
        None
    };
    let chat_defaults_before = if first_before.is_some() {
        if let Some(chat) = orch_db::queries::chats::chat_of_node(&mut tx, id).await? {
            orch_db::queries::chats::following::derived(
                &mut tx,
                None,
                Some(&[chat.id]),
                principal.now().get(),
            )
            .await?
            .into_iter()
            .map(|reason| reason.user_id)
            .collect::<Vec<_>>()
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };
    match pages::change(
        &mut tx,
        &principal,
        which.target(id),
        &change,
        policy,
        &origin(ctx),
    )
    .await
    {
        Ok(outcome) => {
            if outcome.changed {
                hierarchy::stamp(&mut tx, id).await?;
                if first_before.is_some() {
                    orch_chat::following::refresh_thread(
                        &mut tx,
                        id,
                        &chat_defaults_before,
                        principal.now().get(),
                    )
                    .await
                    .map_err(ApiError::internal)?;
                }
                if let Some(first_before) = first_before {
                    crate::thread_markers::page_changed(
                        &mut tx,
                        ctx.now,
                        id,
                        principal.user(),
                        first_before,
                        &change,
                    )
                    .await?;
                }
            }
        }
        Err(ChangeError::Stale { current }) => {
            let page = read_page(ctx, &mut tx, &principal, id, place.org, which).await?;
            return Err(
                refusals::change_failed(&mut tx, ChangeError::Stale { current })
                    .await
                    .with("current", page),
            );
        }
        Err(error) => return Err(refusals::change_failed(&mut tx, error).await),
    }
    if !people.is_empty() {
        crate::routes::notifications::person::tell(
            ctx,
            &mut tx,
            &principal,
            "nodes",
            id,
            &people,
            body.notify_anchor,
            None,
        )
        .await?;
    }
    let page = read_page(ctx, &mut tx, &principal, id, place.org, which).await?;
    crate::routes::notifications::person::keep_page(ctx, &mut tx, &principal, body).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(page)
}

/// Changes a permission page: the one request that does.
async fn page_update(ctx: Ctx, (): (), body: api::PageChange) -> ApiResult<api::PermissionPage> {
    if matches!(ctx.param("object")?, "documents" | "document-containers") {
        return crate::routes::documents::page::update(&ctx, &body).await;
    }
    if ctx.param("object")? != "nodes" {
        return crate::routes::resources::page::update(&ctx, &body).await;
    }
    change_page(&ctx, Which::Live, &body).await
}

/// Changes what a place gives to what is made inside it.
async fn defaults_update(
    ctx: Ctx,
    (): (),
    body: api::PageChange,
) -> ApiResult<api::PermissionPage> {
    change_page(&ctx, Which::Defaults, &body).await
}
