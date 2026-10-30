//! Ending a membership and what it touches, and changing the domain of a member, each with the plan of what it does before it does it.
//!
//! A person belongs to the tree of a root org by one org, their domain.
//! Ending the membership takes them out of every Owners row, passes what
//! they owned alone to the first owner of the org it lies in, and removes
//! their roles, the entries that name them and their groups. Moving the
//! domain takes the role they held at the old one and gives it on the new
//! one in the same step, so that they are a member throughout, and touches
//! nothing that names them elsewhere. Resources, machines, documents and
//! requests are included in its preview and settled by the ending in the same transaction.

use super::{member_of, the_org};
use crate::middleware::authorization::{self, exact, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use crate::routes::access::page::same;
use crate::routes::access::{require_role_giving, roles_held};
use crate::routes::nodes::{
    Placed, actor_name, invalid, placed, policy_at, record, record_about, view,
};
use orch_access_contract::audit::AuditAction;
use orch_access_contract::decisions::Decision;
use orch_access_contract::permissions::Permission;
use orch_api_contract::access::{MissingAccess, Role};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::directory::Member;
use orch_api_contract::nodes::NodeKind;
use orch_api_contract::ownership::{
    Holdings, HoldingsNodes, HoldingsShared, MemberMoveBody, MemberMovePlan, MemberMoveReach,
    MemberMoveStays, OrgMemberHoldings, OrgMemberMove, OrgMemberMovePreview, OrgMemberRemove,
    OrgMemberRemoveBody,
};
use orch_authz::{Act, Policy, nodes};
use orch_db::queries::{access, hierarchy, identity};
use orch_db::{Conn, DbError};
use orch_types::Id;
use serde_json::json;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on(OrgMemberHoldings, holdings)
        .on(OrgMemberRemove, remove)
        .on(OrgMemberMovePreview, move_preview)
        .on(OrgMemberMove, move_member);
}

/// Fills in what only the managers of the org `org`'s members read of each member: how they stand
/// with a second step (none for a service identity), whether an org of theirs requires one, and when
/// they last used something inside this org. Nothing of where else they signed in or worked.
pub(crate) async fn fill_managed(
    conn: &mut Conn,
    org: Id,
    members: &mut [Member],
) -> ApiResult<()> {
    let people: Vec<Id> = members.iter().map(|member| member.user.id).collect();
    if people.is_empty() {
        return Ok(());
    }
    let local_login = crate::sign_in::system_policy(conn).await?.local_login;
    let steps = crate::sign_in::second_step::of_members(conn, &people, local_login).await?;
    let seen: std::collections::HashMap<Id, orch_types::Time> =
        orch_db::queries::second_step::last_seen_in(conn, org, &people)
            .await?
            .into_iter()
            .map(|(user, at)| (user, orch_types::Time::new(at)))
            .collect();
    for member in members.iter_mut() {
        if let Some((state, required)) = steps.get(&member.user.id) {
            member.second_step = Some(*state);
            member.second_step_required = Some(*required);
        }
        member.last_seen_at = seen.get(&member.user.id).copied();
    }
    Ok(())
}

/// The person a request is about: an identifier, or `me`.
fn subject(ctx: &Ctx) -> ApiResult<Id> {
    let word = ctx.param("user_id")?;
    if word == "me" {
        Ok(ctx.user()?.id())
    } else {
        word.parse().map_err(|_| ApiError::not_found())
    }
}

/// The member a request is about, in the tree of the org it names.
struct Found {
    /// The org the path names, with its root org.
    org: Placed,
    /// The person.
    user: Id,
    /// Their membership of the root org.
    member: access::MemberRow,
}

impl Found {
    /// The root org of the tree, as the place the audit rows stand beside.
    fn root(&self) -> Placed {
        Placed {
            id: self.org.org,
            org: self.org.org,
        }
    }
}

async fn found(ctx: &Ctx, conn: &mut Conn) -> ApiResult<Found> {
    let org = the_org(conn, ctx.id("id")?).await?;
    let user = subject(ctx)?;
    let member = access::member_row(conn, org.org, user, ctx.now)
        .await?
        .ok_or_else(ApiError::not_found)?;
    Ok(Found { org, user, member })
}

/// The orgs from the domain of a member up to the root org of their tree.
async fn domain_path(conn: &mut Conn, domain: Id) -> ApiResult<Vec<Id>> {
    let rows = hierarchy::with_ancestors(conn, &[domain]).await?;
    let node = rows
        .iter()
        .find(|row| row.id == domain)
        .ok_or_else(ApiError::not_found)?;
    Ok(node.path.iter().rev().copied().collect())
}

/// Whether the caller may end this membership: they are the member, or they
/// hold `members.manage` on the org that is the member's domain or on one
/// above it. The refusal is the one about the domain when no org says yes.
async fn may_end(
    conn: &mut Conn,
    principal: &orch_authz::Principal,
    found: &Found,
) -> ApiResult<()> {
    if found.user == principal.user() {
        return Ok(());
    }
    let mut refused = None;
    for org in domain_path(conn, found.member.domain_id).await? {
        match authorization::require(
            conn,
            principal,
            org,
            exact(Permission::MembersManage),
            Act::Use,
            Policy::default(),
        )
        .await
        {
            Ok(()) => return Ok(()),
            Err(error) => {
                refused.get_or_insert(error);
            }
        }
    }
    Err(refused.unwrap_or_else(ApiError::not_found))
}

/// How many nodes of each kind, and documents, as the dialog counts them.
fn nodes_of(counts: &[(String, i64)], documents: i64) -> HoldingsNodes {
    let of = |kind: &str| {
        counts
            .iter()
            .filter(|(each, _)| each == kind)
            .map(|(_, count)| *count)
            .sum()
    };
    HoldingsNodes {
        orgs: of("org"),
        projects: of("project"),
        topics: of("topic"),
        threads: of("thread"),
        documents,
    }
}

/// What is owned with others, by kind: everything owned less what is owned alone.
fn shared_of(every: &HoldingsNodes, alone: &HoldingsNodes) -> HoldingsShared {
    HoldingsShared {
        orgs: every.orgs - alone.orgs,
        projects: every.projects - alone.projects,
        topics: every.topics - alone.topics,
        threads: every.threads - alone.threads,
        documents: every.documents - alone.documents,
        // Resource ownership and personal registrations are filled from their own plan below.
        resources: 0,
    }
}

/// Everything of a person's in a tree that ending their membership touches, documents among it.
async fn holdings_of(ctx: &Ctx, conn: &mut Conn, root: Id, user: Id) -> ApiResult<Holdings> {
    let alone = nodes_of(
        &access::owned_counts(conn, root, user, None, true).await?,
        access::owned_document_count(conn, root, user, None, true).await?,
    );
    let every = nodes_of(
        &access::owned_counts(conn, root, user, None, false).await?,
        access::owned_document_count(conn, root, user, None, false).await?,
    );
    let (roles, entries) = access::named_counts(conn, root, user).await?;
    let affected = orch_engine::membership::holdings(conn, root, user, ctx.now)
        .await
        .map_err(ApiError::internal)?;
    let machine_ids: Vec<_> = affected
        .registrations
        .iter()
        .map(|registration| registration.machine)
        .collect();
    let machines =
        crate::routes::resources::view::machine_refs(&ctx.app, conn, &machine_ids).await?;
    let machine_registrations = affected
        .registrations
        .into_iter()
        .map(|registration| {
            let machine = machines
                .get(&registration.machine)
                .cloned()
                .ok_or_else(|| ApiError::internal("a registered resource's machine is missing"))?;
            Ok(orch_api_contract::ownership::HoldingsMachineRegistration {
                object: registration.object,
                machine,
                threads: registration.threads,
            })
        })
        .collect::<ApiResult<Vec<_>>>()?;
    let mut shared_by_kind = shared_of(&every, &alone);
    shared_by_kind.resources = affected.shared_resources;
    Ok(Holdings {
        shared: shared_by_kind.orgs
            + shared_by_kind.projects
            + shared_by_kind.topics
            + shared_by_kind.threads
            + shared_by_kind.documents
            + shared_by_kind.resources,
        nodes: alone,
        shared_by_kind,
        resources: affected.resources,
        machine_registrations,
        entries: roles + entries,
        pending: affected.pending,
    })
}

/// What ending the membership of a person would touch.
async fn holdings(ctx: Ctx, (): (), (): ()) -> ApiResult<Holdings> {
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let found = found(&ctx, &mut conn).await?;
    may_end(&mut conn, &principal, &found).await?;
    holdings_of(&ctx, &mut conn, found.org.org, found.user).await
}

/// Ends a membership of the root org, with everything that goes with it.
async fn remove(ctx: Ctx, (): (), body: OrgMemberRemoveBody) -> ApiResult<()> {
    let (ctx, mut tx, principal) = crate::routes::nodes::action(&ctx).await?;
    let found = found(&ctx, &mut tx).await?;
    may_end(&mut tx, &principal, &found).await?;
    let root = found.root();
    // The last owner of a root org names another before leaving it.
    let owners = access::owner_users(&mut tx, root.id).await?;
    if owners.contains(&found.user) && owners.iter().all(|owner| *owner == found.user) {
        return Err(ApiError::new(
            ErrorCode::OwnerLast,
            "the last owner of a root org names another owner first",
        ));
    }
    let choices: std::collections::HashMap<Id, orch_authz::ownership::Disposal> = body
        .resources
        .unwrap_or_default()
        .into_iter()
        .map(|(id, fate)| {
            let id = id
                .parse::<Id>()
                .map_err(|_| invalid("resources", "a resource choice must name its identifier"))?;
            let fate = match fate {
                orch_api_contract::ownership::ResourceFate::Keep => {
                    orch_authz::ownership::Disposal::Keep
                }
                orch_api_contract::ownership::ResourceFate::Disable => {
                    orch_authz::ownership::Disposal::Disable
                }
                orch_api_contract::ownership::ResourceFate::Remove => {
                    orch_authz::ownership::Disposal::Remove
                }
            };
            Ok((id, fate))
        })
        .collect::<ApiResult<_>>()?;
    let leaving = match ctx
        .app
        .engine
        .end_member_resources_in(
            &mut tx,
            &principal,
            root.id,
            found.user,
            &choices,
            &crate::sign_in::origin(&ctx),
        )
        .await
    {
        Ok(leaving) => leaving,
        Err(orch_engine::membership::EndError::Denied(refusal)) => {
            return Err(refusals::denied(&mut tx, *refusal).await?);
        }
        Err(orch_engine::membership::EndError::NotFound) => return Err(ApiError::not_found()),
        Err(orch_engine::membership::EndError::Choice(reason)) => {
            return Err(invalid("resources", &reason));
        }
        Err(orch_engine::membership::EndError::InUse(resource)) => {
            return Err(ApiError::new(
                ErrorCode::ResourceInUse,
                "a selected resource must be released before it can be removed",
            )
            .with("resource", resource));
        }
        Err(orch_engine::membership::EndError::Ownership(
            orch_authz::ownership::EndError::Stuck(_),
        )) => {
            return Err(ApiError::new(
                ErrorCode::OwnerLast,
                "name a surviving owner before ending this membership",
            ));
        }
        Err(error) => return Err(ApiError::internal(error)),
    };
    let mut ctx = ctx;
    ctx.now = leaving.at.get();
    let ended =
        access::end_membership(&mut tx, root.id, found.user, principal.user(), ctx.now).await?;
    for node in ended.passed.iter().map(|(node, _)| node).chain(&ended.left) {
        hierarchy::stamp(&mut tx, *node).await?;
    }
    // The ownership ending moved the epoch in this same transaction.
    let who = actor_name(&ctx)?;
    let person = identity::users_brief(&mut tx, &[found.user])
        .await?
        .into_iter()
        .next()
        .map(|brief| brief.display_name)
        .unwrap_or_default();
    let org = refusals::named_node(&mut tx, root.id)
        .await?
        .name
        .unwrap_or_default();
    let summary = if found.user == principal.user() {
        format!("{who} left {org}")
    } else {
        format!("{who} removed {person} from {org}")
    };
    record_about(
        &ctx,
        &mut tx,
        orch_db::audit::action!(AuditAction::MemberRemoved),
        found.user,
        root,
        &summary,
        json!({
            "user": found.user,
            "roles": ended.roles,
            "entries": ended.entries,
            "groups": ended.groups,
            "passed": ended.passed.iter().map(|(node, to)| json!({ "node": node, "to": to })).collect::<Vec<_>>(),
            "left": ended.left,
            "things":leaving.ownership.passed.iter().map(|passing| json!({"kind":passing.object.kind.as_str(),"id":passing.object.id,
                "fate":match passing.fate { orch_authz::ownership::Fate::LeftToOthers { .. }=>"left_to_others",
                    orch_authz::ownership::Fate::PassedOn { .. }=>"passed_on",orch_authz::ownership::Fate::Unregistered=>"unregistered",
                    orch_authz::ownership::Fate::Stuck=>"stuck" }})).collect::<Vec<_>>(),
            "resources":choices.iter().map(|(id,fate)| json!({"id":id,"fate":match fate { orch_authz::ownership::Disposal::Keep=>"keep",
                orch_authz::ownership::Disposal::Disable=>"disable",orch_authz::ownership::Disposal::Remove=>"remove" }})).collect::<Vec<_>>(),
            "cancelled":leaving.cancelled,
        }),
    )
    .await?;
    hierarchy::stamp(&mut tx, root.id).await?;
    tx.commit().await.map_err(DbError::from)?;
    ctx.app.engine.kick();
    Ok(())
}

/// The place a member would move to, and the role they would hold there,
/// as far as the request itself is well formed.
async fn move_inputs(
    ctx: &Ctx,
    conn: &mut Conn,
    body: &MemberMoveBody,
) -> ApiResult<(Found, Placed, Role)> {
    let found = found(ctx, conn).await?;
    let (to, kind) = placed(conn, body.to).await?;
    if kind != NodeKind::Org || to.org != found.org.org {
        return Err(ApiError::new(
            ErrorCode::NodeWrongParent,
            "a member moves to an org of the tree they belong to",
        ));
    }
    if to.id == found.member.domain_id {
        return Err(ApiError::new(
            ErrorCode::MemberSameDomain,
            "that is their domain already",
        ));
    }
    let role = body.role.unwrap_or_else(|| {
        found
            .member
            .role
            .as_deref()
            .and_then(|word| serde_json::from_value(json!(word)).ok())
            .unwrap_or(Role::Viewer)
    });
    Ok((found, to, role))
}

/// What the caller lacks of `members.manage` at an end of a move, and whether
/// they hold it there.
async fn lacking(
    conn: &mut Conn,
    principal: &orch_authz::Principal,
    org: Id,
) -> ApiResult<(bool, Vec<MissingAccess>)> {
    match nodes::check(
        conn,
        principal,
        org,
        Permission::MembersManage,
        Act::Use,
        Policy::default(),
    )
    .await
    {
        Ok(Decision::Allowed { .. }) => Ok((true, Vec::new())),
        Ok(Decision::Refused(refusal)) => Ok((
            false,
            refusals::missing_access(conn, &refusal.missing).await?,
        )),
        Err(error) => Err(refusals::problem(conn, error).await),
    }
}

/// How far a role reaches: the higher, the more.
fn rank(role: Role) -> u8 {
    match role {
        Role::Admin => 4,
        Role::Developer => 3,
        Role::Interactor => 2,
        Role::Viewer => 1,
    }
}

/// What changing the domain of a member would do, before anything does. It
/// answers for a caller who holds `members.manage` at one end only, with what
/// the other end lacks; one who holds neither is refused as for the move.
///
/// What a person loses and gains is counted by where nodes lie: those in the
/// place they leave and not in the one they join, and the reverse, that they
/// reach through no role, group or entry of their own besides the role at
/// their domain. A role given to the members of an org is not counted.
async fn move_preview(ctx: Ctx, (): (), body: MemberMoveBody) -> ApiResult<MemberMovePlan> {
    let (ctx, mut conn, principal) = crate::routes::nodes::action(&ctx).await?;
    let (found, to, wanted) = move_inputs(&ctx, &mut conn, &body).await?;
    let (root, user, domain) = (found.org.org, found.user, found.member.domain_id);
    let mut missing = Vec::new();
    let mut holds_one = false;
    for org in [domain, to.id] {
        let (holds, lacks) = lacking(&mut conn, &principal, org).await?;
        holds_one |= holds;
        missing.extend(lacks);
    }
    if !holds_one {
        authorization::require(
            &mut conn,
            &principal,
            domain,
            exact(Permission::MembersManage),
            Act::Use,
            Policy::default(),
        )
        .await?;
    }
    if missing.is_empty() {
        let policy = policy_at(&mut conn, to.id).await?;
        require_role_giving(&mut conn, &principal, to.id, wanted, policy).await?;
    }
    let existing: Option<Role> = access::role_at(&mut conn, to.id, user)
        .await?
        .and_then(|word| serde_json::from_value(json!(word)).ok());
    let role = existing.map_or(wanted, |held| {
        if rank(held) > rank(wanted) {
            held
        } else {
            wanted
        }
    });
    let loses =
        access::reached_only_through_domain(&mut conn, root, user, domain, domain, to.id).await?;
    let gains =
        access::reached_only_through_domain(&mut conn, root, user, domain, to.id, domain).await?;
    let owned = access::owned_counts(&mut conn, root, user, Some(domain), false).await?;
    let owned_documents =
        access::owned_document_count(&mut conn, root, user, Some(domain), false).await?;
    let (_, named) = access::named_counts(&mut conn, root, user).await?;
    let elsewhere = access::roles_elsewhere(&mut conn, root, user, domain).await?;
    let roles = roles_held(&mut conn, &principal, &elsewhere, root).await?;
    let people = refusals::user_refs(&mut conn, &[user]).await?;
    let refs = view::refs(&mut conn, &principal, &[domain, to.id], Policy::default()).await?;
    let node = |id: Id| {
        refs.get(&id)
            .cloned()
            .ok_or_else(|| ApiError::internal("a place of a move is not there"))
    };
    Ok(MemberMovePlan {
        user: people
            .get(&user)
            .cloned()
            .ok_or_else(|| ApiError::internal("a member is not there"))?,
        from: node(domain)?,
        to: node(to.id)?,
        role,
        loses: MemberMoveReach {
            nodes: loses,
            resources: 0,
            documents: 0,
        },
        gains: MemberMoveReach {
            nodes: gains,
            resources: 0,
            documents: 0,
        },
        stays: MemberMoveStays {
            nodes: nodes_of(&owned, owned_documents),
            resources: 0,
            roles,
            named,
        },
        waiting: Vec::new(),
        missing,
    })
}

/// Changes the domain of a member: the role they held at the old one is
/// taken from it and given on the new one in the same step.
async fn move_member(ctx: Ctx, (): (), body: MemberMoveBody) -> ApiResult<Member> {
    let (ctx, mut tx, principal) = crate::routes::nodes::action(&ctx).await?;
    let (found, to, wanted) = move_inputs(&ctx, &mut tx, &body).await?;
    let (root, user, domain) = (found.root(), found.user, found.member.domain_id);
    for org in [domain, to.id] {
        authorization::require(
            &mut tx,
            &principal,
            org,
            exact(Permission::MembersManage),
            Act::Use,
            Policy::default(),
        )
        .await?;
    }
    let policy = policy_at(&mut tx, to.id).await?;
    require_role_giving(&mut tx, &principal, to.id, wanted, policy).await?;
    let word: String = same(&wanted).ok_or_else(|| invalid("role", "is not a role"))?;
    let held = access::move_member_with_choice(
        &mut tx,
        root.id,
        user,
        domain,
        to.id,
        &word,
        body.role.is_some(),
        principal.user(),
        ctx.now,
    )
    .await?;
    let move_epoch = orch_authz::epoch::bump(&mut tx)
        .await
        .map_err(ApiError::internal)?;
    let who = actor_name(&ctx)?;
    record(
        &ctx,
        &mut tx,
        orch_db::audit::action!(AuditAction::MemberMoved),
        root,
        &format!("{who} moved a member to another org of the tree"),
        json!({ "user": user, "from": domain, "to": to.id, "role": held }),
    )
    .await?;
    hierarchy::stamp(&mut tx, root.id).await?;
    let row = access::member_row(&mut tx, root.id, user, ctx.now)
        .await?
        .ok_or_else(|| ApiError::internal("a member was moved and is not there"))?;
    let people = refusals::user_refs(&mut tx, &[user]).await?;
    let refs = view::refs(&mut tx, &principal, &[row.domain_id], Policy::default()).await?;
    let member = member_of(&row, &people, &refs, true)
        .ok_or_else(|| ApiError::internal("a member was moved and is not shown"))?;
    orch_engine::notifications::tell(&mut tx, &orch_engine::notifications::Event {
        kind:orch_engine::notifications::Kind::MemberMoved, node:Some(to.id), thread:None, approval:None,
        title:"Your place in the team changed".to_owned(), body:String::new(),
        data:json!({"user_id":user,"from":domain,"to":to.id,"role":word,"path":format!("/org/{}", to.id)}),
        source:format!("{}:{user}:{move_epoch}", root.id), actor:Some(principal.user()), at:ctx.now,
    }, &[user], Some(root.id)).await.map_err(ApiError::internal)?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(member)
}
