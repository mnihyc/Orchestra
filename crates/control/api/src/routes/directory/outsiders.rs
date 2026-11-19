//! People from outside a root tree who reach something of one of its orgs: the list, the ways each reaches it, and taking one out by a plan read whole.
//!
//! The list is for whoever manages the org's members. Each person comes with
//! the first ways they reach something here (an entry or a role naming them, or
//! a group, an org's members or a role of another place they belong to), who
//! gave each and until when, and when they were last seen here; nothing of
//! where else they sign in or work, and no other member of a group.
//!
//! Taking somebody out is a plan first. The preview keeps the complete set of
//! ways on the server (a row of `preflights`, kind `outside_removal`, for the
//! life of every stored change plan) and gives it page by page; the last page
//! carries a fingerprint that binds the caller and that whole set, and the
//! confirmation carries it back. At confirmation every way is read again under
//! lock: a way added, replaced or changed since, an expired plan, another
//! caller, or a right that was lost refuses the whole act before anything is
//! changed. Then the entries and roles that name the person go, each by the
//! ordinary act and its audit line, and where a group, an org's members or a
//! role of another place still lets them in, the existing suspension holds them
//! out of this org instead: no group is emptied and nothing outside this org is
//! touched.

use super::{suspensions_shown, the_org};
use crate::middleware::authorization::{self, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use crate::routes::nodes::{Placed, actor_name, invalid, record, record_about};
use chrono::Duration;
use orch_access_contract::audit::AuditAction;
use orch_access_contract::decisions::{Decision, ObjectRef};
use orch_access_contract::permissions::{Permission, ResourceKind};
use orch_api_contract::access::MissingAccess;
use orch_api_contract::common::{ErrorCode, Page, PageQuery};
use orch_api_contract::directory::{
    OrgOutsiderList, OrgOutsiderListQuery, OrgOutsiderRemove, OrgOutsiderRemoveBody,
    OrgOutsiderRemovePlanGet, OrgOutsiderRemovePlanGetQuery, OrgOutsiderRemovePreview,
    OrgOutsiderSourceList, OrgOutsiderSourceListQuery, OutsidePerson, OutsideRemovalPlan,
    OutsideRemovalResult, OutsideSource, OutsideSourceObject,
};
use orch_authz::{Act, Policy, Principal, documents, epoch, nodes, resources};
use orch_db::queries::jobs::forks::{self as plans, PreflightRow};
use orch_db::queries::outsiders::{self, SourceRow};
use orch_db::queries::{access, second_step};
use orch_db::{Conn, DbError};
use orch_types::{Id, Json, Time};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;

/// The kind of a stored removal plan.
const PLAN_KIND: &str = "outside_removal";
/// For how long a removal plan may be confirmed: the life of every stored change plan.
const PLAN_MINUTES: i64 = 15;
/// How many ways of each person the list shows before "Show more": a person rarely has more,
/// and the rest are a page of their own.
const SOURCES_SHOWN: i64 = 5;
/// The most characters of the reason a removal gives: what a suspension keeps.
const REASON_MAX_CHARS: usize = 500;
/// Every way at once: a plan is never cut short of a way it would leave out.
const ALL: i64 = i64::MAX;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on(OrgOutsiderList, outsider_list)
        .on(OrgOutsiderSourceList, source_list)
        .on(OrgOutsiderRemovePreview, remove_preview)
        .on(OrgOutsiderRemovePlanGet, remove_plan_get)
        .on(OrgOutsiderRemove, remove);
}

/// The kind of resource a page of the kind `word` belongs to, where it is one.
fn resource_kind(word: &str) -> Option<ResourceKind> {
    match word {
        "provider" => Some(ResourceKind::Provider),
        "agent" => Some(ResourceKind::Agent),
        "environment" => Some(ResourceKind::Environment),
        "host" => Some(ResourceKind::Host),
        "machine" => Some(ResourceKind::Machine),
        "template" => Some(ResourceKind::Template),
        "mcp_server" => Some(ResourceKind::McpServer),
        "mount" => Some(ResourceKind::Mount),
        "skill" => Some(ResourceKind::Skill),
        _ => None,
    }
}

/// What changing the source `row` takes of the caller, or `None` when they may: managing the page
/// of the thing an entry is on, or managing who holds what at the place a role is given.
async fn lacks(
    conn: &mut Conn,
    principal: &Principal,
    row: &SourceRow,
) -> ApiResult<Option<Vec<MissingAccess>>> {
    let decided = if row.source == "role" || row.object_kind == "node" {
        nodes::check(
            conn,
            principal,
            row.object_id,
            Permission::AccessManage,
            Act::Use,
            Policy::default(),
        )
        .await
    } else if row.object_kind == "document" {
        documents::check(
            conn,
            principal,
            row.object_id,
            Permission::AccessManage,
            Policy::default(),
        )
        .await
    } else if let Some(kind) = resource_kind(&row.object_kind) {
        let object = ObjectRef {
            kind: kind.into(),
            id: row.object_id,
        };
        resources::check(
            conn,
            principal,
            object,
            Permission::AccessManage,
            Policy::default(),
        )
        .await
    } else {
        return Ok(Some(Vec::new()));
    };
    match decided {
        Ok(Decision::Allowed { .. }) => Ok(None),
        Ok(Decision::Refused(refusal)) => Ok(Some(
            refusals::missing_access(conn, &refusal.missing).await?,
        )),
        Err(error) => Err(authorization::engine_problem(conn, principal, error).await),
    }
}

/// The holder of a source as the row names it: the person themself, or a group, an org's members or
/// a role of another place by its name, never the others it holds.
fn holder_of(row: &SourceRow) -> Json {
    match row.holder_kind.as_str() {
        "user" => json!({ "kind": "user" }),
        "group" => json!({ "kind": "group", "id": row.holder_id, "name": row.holder_name }),
        "members" => json!({
            "kind": "members", "org": { "id": row.holder_id, "name": row.holder_name }
        }),
        "role" => json!({
            "kind": "role", "role": row.holder_role, "at": { "id": row.holder_id, "name": row.holder_name }
        }),
        other => json!({ "kind": other }),
    }
}

/// The sources `rows` as a reader sees them; `can_change` is asked of the direct ones only.
async fn shown(
    conn: &mut Conn,
    principal: &Principal,
    rows: &[SourceRow],
) -> ApiResult<Vec<OutsideSource>> {
    let givers: Vec<Id> = rows.iter().filter_map(|row| row.created_by).collect();
    let people = refusals::user_refs(conn, &givers).await?;
    let mut shown = Vec::with_capacity(rows.len());
    for row in rows {
        let can_change = row.direct && lacks(conn, principal, row).await?.is_none();
        shown.push(OutsideSource {
            object: OutsideSourceObject {
                kind: row.object_shown_kind.clone(),
                id: row.object_id,
                name: row.object_name.clone(),
            },
            row: row.row.clone(),
            holder: holder_of(row),
            by: row.created_by.and_then(|by| people.get(&by).cloned()),
            at: Time::new(row.created_at),
            until: row.expires_at.map(Time::new),
            direct: row.direct,
            can_change,
        });
    }
    Ok(shown)
}

/// The person a path names, who is somebody from outside the tree of `root`: a member of it is
/// taken out by ending their membership, not here.
async fn outsider(conn: &mut Conn, ctx: &Ctx, root: Id) -> ApiResult<Id> {
    let user = ctx.id("user_id")?;
    if !access::members_among(conn, root, &[user]).await?.is_empty() {
        return Err(invalid(
            "user_id",
            "is a member of this tree: their membership is ended in Members",
        ));
    }
    Ok(user)
}

/// A cursor of the list: the username and the identifier of the last person of a page.
fn person_cursor(text: &str) -> ApiResult<(String, Id)> {
    let bad = || invalid("cursor", "is not a cursor of this list");
    let (name, id) = text.rsplit_once('|').ok_or_else(bad)?;
    Ok((name.to_owned(), id.parse().map_err(|_| bad())?))
}

/// A cursor of the ways of one person: the identifier of the last of a page.
fn source_cursor(text: Option<&str>) -> ApiResult<Option<Id>> {
    text.map(|text| {
        text.parse()
            .map_err(|_| invalid("cursor", "is not a cursor of this list"))
    })
    .transpose()
}

/// The people from outside the root tree who reach something of an org now.
async fn outsider_list(
    ctx: Ctx,
    query: OrgOutsiderListQuery,
    (): (),
) -> ApiResult<Page<OutsidePerson>> {
    let id = ctx.id("id")?;
    let page = PageQuery {
        limit: query.limit,
        cursor: query.cursor.clone(),
    };
    let limit = i64::from(page.limit());
    let after = query.cursor.as_deref().map(person_cursor).transpose()?;
    let mut conn = ctx.app.db.acquire().await?;
    let org = the_org(&mut conn, id).await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let candidates = outsiders::candidates(
        &mut conn,
        id,
        org.org,
        ctx.now,
        query.q.as_deref().map(str::trim).filter(|q| !q.is_empty()),
        after.as_ref().map(|(name, user)| (name.as_str(), *user)),
        limit + 1,
    )
    .await?;
    let more = i64::try_from(candidates.len()).unwrap_or(i64::MAX) > limit;
    let scanned: Vec<_> = candidates
        .into_iter()
        .take(usize::try_from(limit).unwrap_or(usize::MAX))
        .collect();
    let next_cursor = more
        .then(|| {
            scanned
                .last()
                .map(|last| format!("{}|{}", last.username, last.id))
        })
        .flatten();
    let users: Vec<Id> = scanned.iter().map(|row| row.id).collect();
    let people = refusals::user_refs(&mut conn, &users).await?;
    let seen: HashMap<Id, Time> = second_step::last_seen_in(&mut conn, id, &users)
        .await?
        .into_iter()
        .map(|(user, at)| (user, Time::new(at)))
        .collect();
    let suspended = outsiders::suspended_among(&mut conn, id, &users).await?;
    let mut items = Vec::new();
    for user in users {
        let mut rows =
            outsiders::sources(&mut conn, id, user, ctx.now, None, SOURCES_SHOWN + 1).await?;
        // Somebody whom nothing here admits after all (a role of too low a rank) is nobody from
        // outside who reaches it.
        if rows.is_empty() {
            continue;
        }
        let Some(person) = people.get(&user).cloned() else {
            continue;
        };
        let continues = i64::try_from(rows.len()).unwrap_or(i64::MAX) > SOURCES_SHOWN;
        rows.truncate(usize::try_from(SOURCES_SHOWN).unwrap_or(usize::MAX));
        let sources_cursor = continues
            .then(|| rows.last().map(|row| row.id.to_string()))
            .flatten();
        items.push(OutsidePerson {
            user: person,
            last_seen_at: seen.get(&user).copied(),
            sources: shown(&mut conn, &principal, &rows).await?,
            sources_cursor,
            suspended: suspended.contains(&user),
        });
    }
    Ok(Page { items, next_cursor })
}

/// The ways one person from outside reaches an org now, page by page.
async fn source_list(
    ctx: Ctx,
    query: OrgOutsiderSourceListQuery,
    (): (),
) -> ApiResult<Page<OutsideSource>> {
    let id = ctx.id("id")?;
    let page = PageQuery {
        limit: query.limit,
        cursor: query.cursor.clone(),
    };
    let limit = i64::from(page.limit());
    let after = source_cursor(query.cursor.as_deref())?;
    let mut conn = ctx.app.db.acquire().await?;
    let org = the_org(&mut conn, id).await?;
    let user = outsider(&mut conn, &ctx, org.org).await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let mut rows = outsiders::sources(&mut conn, id, user, ctx.now, after, limit + 1).await?;
    let more = i64::try_from(rows.len()).unwrap_or(i64::MAX) > limit;
    rows.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
    let next_cursor = more
        .then(|| rows.last().map(|row| row.id.to_string()))
        .flatten();
    Ok(Page {
        items: shown(&mut conn, &principal, &rows).await?,
        next_cursor,
    })
}

/// One way of a stored plan: what is compared at confirmation, and what its pages show.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct KeptSource {
    /// The entry or the role.
    id: Id,
    /// `entry` or `role`.
    source: String,
    /// Its revision when the plan was made.
    revision: i64,
    /// It names the person, and goes.
    direct: bool,
    /// The thing it is on, for checking the caller's right to remove it again.
    object_kind: String,
    /// The thing.
    object_id: Id,
    /// The row of the thing's page, for the audit line of its removal.
    row: String,
    /// How its pages show it.
    shown: OutsideSource,
}

/// What a removal plan keeps for its confirmation.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct KeptPlan {
    /// The person.
    user: Id,
    /// Every way, in the order its pages give them.
    sources: Vec<KeptSource>,
    /// The hash of every way with its revision.
    fingerprint: String,
    /// Something not removed still lets them in.
    suspension_needed: bool,
    /// What the caller lacked when it was made.
    missing: Vec<MissingAccess>,
}

/// The hash of a set of ways: an added, replaced or changed way gives another, whatever the totals.
fn fingerprint_of(rows: &[SourceRow]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    for row in rows {
        hasher.update(
            format!(
                "{}:{}:{}:{}\n",
                row.source, row.id, row.revision, row.direct
            )
            .as_bytes(),
        );
    }
    hex::encode(hasher.finalize())
}

/// What the last page of a plan gives, and the confirmation carries back: bound to the plan, to
/// the caller and to the whole set of ways it holds.
fn reviewed_fingerprint(plan: Id, caller: Id, fingerprint: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(
        format!("outside_removal:{plan}:{caller}:{fingerprint}").as_bytes(),
    ))
}

/// One page of a stored plan, from `cursor` (an index into its ways), for its caller.
async fn plan_page(
    conn: &mut Conn,
    principal: &Principal,
    row: &PreflightRow,
    kept: &KeptPlan,
    org: Id,
    cursor: Option<&str>,
    limit: u32,
) -> ApiResult<OutsideRemovalPlan> {
    let start = match cursor {
        None => 0,
        Some(text) => text
            .parse::<usize>()
            .ok()
            .filter(|start| *start <= kept.sources.len())
            .ok_or_else(|| invalid("cursor", "is not a cursor of this plan"))?,
    };
    let end = start
        .saturating_add(usize::try_from(limit).unwrap_or(usize::MAX))
        .min(kept.sources.len());
    let last = end == kept.sources.len();
    let refs = crate::routes::nodes::view::refs(conn, principal, &[org], Policy::default()).await?;
    let people = refusals::user_refs(conn, &[kept.user]).await?;
    Ok(OutsideRemovalPlan {
        id: row.id,
        expires_at: Time::new(row.expires_at),
        org: refs
            .get(&org)
            .cloned()
            .ok_or_else(|| ApiError::internal("an org of a plan has no reference"))?,
        user: people
            .get(&kept.user)
            .cloned()
            .ok_or_else(ApiError::not_found)?,
        removes: kept.sources[start..end]
            .iter()
            .map(|source| source.shown.clone())
            .collect(),
        next_cursor: (!last).then(|| end.to_string()),
        reviewed_fingerprint: last
            .then(|| reviewed_fingerprint(row.id, row.created_by, &kept.fingerprint)),
        suspension_needed: kept.suspension_needed,
        missing: kept.missing.clone(),
    })
}

/// The stored plan `plan` of taking `user` out of `org`, made by the caller: another's, another
/// person's, another org's or an expired one is no plan of theirs.
async fn kept_plan(
    conn: &mut Conn,
    ctx: &Ctx,
    principal: &Principal,
    plan: Id,
    org: Id,
    user: Id,
) -> ApiResult<(PreflightRow, KeptPlan)> {
    let gone = || {
        ApiError::new(
            ErrorCode::PreflightExpired,
            "there is no such plan: review again",
        )
    };
    let row = plans::preflight(conn, plan)
        .await?
        .filter(|row| {
            row.kind == PLAN_KIND && row.node_id == Some(org) && row.created_by == principal.user()
        })
        .ok_or_else(gone)?;
    if row.expires_at <= ctx.now {
        return Err(ApiError::new(
            ErrorCode::PreflightExpired,
            "the plan is out of date: review again",
        ));
    }
    let kept: KeptPlan = serde_json::from_value(row.plan.clone()).map_err(ApiError::internal)?;
    if kept.user != user {
        return Err(gone());
    }
    Ok((row, kept))
}

/// Plans taking a person from outside out of an org: reads every way they reach it, what the caller
/// lacks to remove those that name them, and whether a suspension is needed; keeps it; gives its
/// first page.
async fn remove_preview(ctx: Ctx, (): (), (): ()) -> ApiResult<OutsideRemovalPlan> {
    let id = ctx.id("id")?;
    let (ctx, mut tx, principal) = crate::routes::nodes::action(&ctx).await?;
    let org = the_org(&mut tx, id).await?;
    let user = outsider(&mut tx, &ctx, org.org).await?;
    let rows = outsiders::sources(&mut tx, id, user, ctx.now, None, ALL).await?;
    if rows.is_empty() {
        return Err(ApiError::not_found());
    }
    let mut missing = Vec::new();
    for row in rows.iter().filter(|row| row.direct) {
        if let Some(lacking) = lacks(&mut tx, &principal, row).await? {
            for each in lacking {
                if !missing.contains(&each) {
                    missing.push(each);
                }
            }
        }
    }
    let already = !outsiders::suspended_among(&mut tx, id, &[user])
        .await?
        .is_empty();
    let shown_rows = shown(&mut tx, &principal, &rows).await?;
    let kept = KeptPlan {
        user,
        sources: rows
            .iter()
            .zip(shown_rows)
            .map(|(row, shown)| KeptSource {
                id: row.id,
                source: row.source.clone(),
                revision: row.revision,
                direct: row.direct,
                object_kind: row.object_kind.clone(),
                object_id: row.object_id,
                row: row.row.clone(),
                shown,
            })
            .collect(),
        fingerprint: fingerprint_of(&rows),
        suspension_needed: !already && rows.iter().any(|row| !row.direct),
        missing,
    };
    let plan = PreflightRow {
        id: orch_types::new_id(),
        kind: PLAN_KIND.to_owned(),
        node_id: Some(id),
        environment_id: None,
        created_by: principal.user(),
        plan: serde_json::to_value(&kept).map_err(ApiError::internal)?,
        expires_at: ctx.now + Duration::minutes(PLAN_MINUTES),
        created_at: ctx.now,
    };
    plans::save_preflight(&mut tx, &plan).await?;
    let page = plan_page(
        &mut tx,
        &principal,
        &plan,
        &kept,
        id,
        None,
        PageQuery::default().limit(),
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(page)
}

/// The next page of a stored removal plan, for the caller who asked for it, while they may still
/// manage the org's members. Nothing is read again: the plan is what it was.
async fn remove_plan_get(
    ctx: Ctx,
    query: OrgOutsiderRemovePlanGetQuery,
    (): (),
) -> ApiResult<OutsideRemovalPlan> {
    let id = ctx.id("id")?;
    let plan = ctx.id("plan_id")?;
    let mut conn = ctx.app.db.acquire().await?;
    let org = the_org(&mut conn, id).await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    authorization::require(
        &mut conn,
        &principal,
        id,
        authorization::exact(Permission::MembersManage),
        Act::Use,
        Policy::default(),
    )
    .await?;
    let user = outsider(&mut conn, &ctx, org.org).await?;
    let (row, kept) = kept_plan(&mut conn, &ctx, &principal, plan, id, user).await?;
    let limit = PageQuery {
        limit: query.limit,
        cursor: None,
    }
    .limit();
    plan_page(
        &mut conn,
        &principal,
        &row,
        &kept,
        id,
        query.cursor.as_deref(),
        limit,
    )
    .await
}

/// Takes a person from outside out of an org as the plan they read whole says, in one transaction:
/// everything is read again under lock and compared first, and any change, any right lost refuses
/// the whole act before a row is touched.
async fn remove(ctx: Ctx, (): (), body: OrgOutsiderRemoveBody) -> ApiResult<OutsideRemovalResult> {
    let id = ctx.id("id")?;
    let reason = body.reason.trim().to_owned();
    if reason.chars().count() > REASON_MAX_CHARS {
        return Err(invalid(
            "reason",
            &format!("must be at most {REASON_MAX_CHARS} characters"),
        ));
    }
    let (ctx, mut tx, principal) = crate::routes::nodes::action(&ctx).await?;
    let org = the_org(&mut tx, id).await?;
    let user = outsider(&mut tx, &ctx, org.org).await?;
    let (row, kept) = kept_plan(&mut tx, &ctx, &principal, body.plan_id, id, user).await?;
    let stale = || {
        ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "what lets them in changed since it was reviewed: review it again",
        )
        .with("plan_id", body.plan_id)
    };
    if body.reviewed_fingerprint != reviewed_fingerprint(row.id, row.created_by, &kept.fingerprint)
    {
        return Err(stale());
    }
    // Every way as it is now, under lock: the set reviewed is the set removed.
    let entries: Vec<Id> = kept
        .sources
        .iter()
        .filter(|source| source.source == "entry")
        .map(|source| source.id)
        .collect();
    let roles: Vec<Id> = kept
        .sources
        .iter()
        .filter(|source| source.source == "role")
        .map(|source| source.id)
        .collect();
    outsiders::lock_sources(&mut tx, &entries, &roles).await?;
    let now_rows = outsiders::sources(&mut tx, id, user, ctx.now, None, ALL).await?;
    if fingerprint_of(&now_rows) != kept.fingerprint {
        return Err(stale());
    }
    for source in now_rows.iter().filter(|source| source.direct) {
        if let Some(lacking) = lacks(&mut tx, &principal, source).await? {
            return Err(ApiError::new(
                ErrorCode::AuthzDenied,
                "you may not change every row that names them",
            )
            .with("missing", lacking));
        }
    }
    let place = Placed { id, org: org.org };
    let who = actor_name(&ctx)?;
    let (mut removed_entries, mut removed_roles) = (0_i64, 0_i64);
    for source in now_rows.iter().filter(|source| source.direct) {
        if source.source == "entry" {
            if access::remove_entry(&mut tx, source.id, principal.user(), ctx.now).await? {
                removed_entries += 1;
                record_about(
                    &ctx,
                    &mut tx,
                    AuditAction::PermissionRemoved,
                    source.object_id,
                    place,
                    &format!(
                        "{who} took somebody from outside out of {}",
                        source.object_name
                    ),
                    json!({ "user": user, "object_kind": source.object_kind,
                            "object_id": source.object_id, "permission": source.row,
                            "entry": source.id, "outside_removal": row.id }),
                )
                .await?;
            }
        } else if access::remove_role(&mut tx, source.id, principal.user(), ctx.now).await? {
            removed_roles += 1;
            record_about(
                &ctx,
                &mut tx,
                AuditAction::RoleRemoved,
                source.object_id,
                place,
                &format!(
                    "{who} took a role in {} from somebody from outside",
                    source.object_name
                ),
                json!({ "user": user, "node": source.object_id, "role": source.row,
                        "role_id": source.id, "outside_removal": row.id }),
            )
            .await?;
        }
    }
    let suspension = if kept.suspension_needed {
        access::suspend_member(&mut tx, id, user, &reason, principal.user(), ctx.now)
            .await
            .map_err(|error| match error {
                DbError::Unique { .. } => stale(),
                other => other.into(),
            })?;
        record(
            &ctx,
            &mut tx,
            AuditAction::MemberSuspended,
            place,
            &format!("{who} held somebody from outside out of this org"),
            json!({ "user": user, "reason": reason, "outside": true, "outside_removal": row.id }),
        )
        .await?;
        let rows = access::suspensions_in(&mut tx, id, Some(user)).await?;
        suspensions_shown(&mut tx, &rows).await?.into_iter().next()
    } else {
        None
    };
    epoch::bump(&mut tx).await.map_err(ApiError::internal)?;
    plans::forget_preflight(&mut tx, row.id).await?;
    let people = refusals::user_refs(&mut tx, &[user]).await?;
    let person = people.get(&user).cloned().ok_or_else(ApiError::not_found)?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(OutsideRemovalResult {
        user: person,
        removed_entries,
        removed_roles,
        suspension,
    })
}
