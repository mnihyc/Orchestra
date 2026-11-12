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

use super::member_of;

use super::the_org;

use crate::middleware::authorization;

use crate::middleware::authorization::exact;

use crate::middleware::authorization::refusals;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::Registry;

use crate::routes::access::page::same;

use crate::routes::access::admin_matters;

use crate::routes::access::roles_held;

use crate::routes::nodes::Placed;

use crate::routes::nodes::actor_name;

use crate::routes::nodes::invalid;

use crate::routes::nodes::placed;

use crate::routes::nodes::policy_at;

use crate::routes::nodes::record;

use crate::routes::nodes::record_about;

use crate::routes::nodes::view;

use orch_access_contract::audit::AuditAction;

use orch_access_contract::decisions::Decision;

use orch_access_contract::permissions::Permission;

use orch_api_contract::access::MissingAccess;

use orch_api_contract::access::Role;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::directory::Member;

use orch_api_contract::nodes::NodeKind;

use orch_api_contract::ownership::Holdings;

use orch_api_contract::ownership::HoldingsNodes;

use orch_api_contract::ownership::HoldingsShared;

use orch_api_contract::ownership::MemberMoveBody;

use orch_api_contract::ownership::MemberMovePlan;

use orch_api_contract::ownership::MemberMoveReach;

use orch_api_contract::ownership::MemberMoveStays;

use orch_api_contract::ownership::OrgMemberHoldings;

use orch_api_contract::ownership::OrgMemberMove;

use orch_api_contract::ownership::OrgMemberMovePreview;

use orch_api_contract::ownership::OrgMemberRemove;

use orch_api_contract::ownership::OrgMemberRemoveBody;

use orch_authz::Act;

use orch_authz::Policy;

use orch_authz::nodes;

use orch_db::queries::access;

use orch_db::queries::hierarchy;

use orch_db::queries::identity;

use orch_db::Conn;

use orch_db::DbError;

use orch_types::Id;

use serde_json::json;
/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on_snapshot_read_with_optional_operation(
            OrgMemberHoldings,
            crate::key_operations::native::selected::<holdings_read::Members>,
            holdings,
        )
        .on_with_operation(
            OrgMemberRemove,
            crate::key_operations::native::effects::prepare::<ending::End>,
            remove,
        )
        .on_snapshot_body_read_with_optional_operation(
            OrgMemberMovePreview,
            crate::key_operations::native::selected_body::<move_review::Preview, (), MemberMoveBody>,
            move_preview,
        )
        .on_with_operation(
            OrgMemberMove,
            crate::key_operations::native::effects::prepare::<moving::Move>,
            move_member,
        );
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

struct PreparedHoldings {
    nodes: HoldingsNodes,
    shared_by_kind: HoldingsShared,
    entries: i64,
    affected: orch_engine::membership::PreparedHoldings,
}

/// Select the original membership preview rows before constructing resource references.
async fn prepare_holdings(
    ctx: &Ctx,
    conn: &mut Conn,
    root: Id,
    user: Id,
) -> ApiResult<PreparedHoldings> {
    let alone = nodes_of(
        &access::owned_counts(conn, root, user, None, true).await?,
        access::owned_document_count(conn, root, user, None, true).await?,
    );
    let every = nodes_of(
        &access::owned_counts(conn, root, user, None, false).await?,
        access::owned_document_count(conn, root, user, None, false).await?,
    );
    let (roles, entries) = access::named_counts(conn, root, user).await?;
    let affected = orch_engine::membership::prepare_holdings(conn, root, user, ctx.now)
        .await
        .map_err(ApiError::internal)?;
    let mut shared_by_kind = shared_of(&every, &alone);
    shared_by_kind.resources = affected.shared_resources;
    Ok(PreparedHoldings {
        nodes: alone,
        shared_by_kind,
        entries: roles + entries,
        affected,
    })
}

/// Existing full formatter; every caller consumes the same prepared native selection.
async fn render_holdings(
    ctx: &Ctx,
    conn: &mut Conn,
    prepared: &PreparedHoldings,
) -> ApiResult<Holdings> {
    let full = prepared.affected.render();
    let machine_ids: Vec<_> = full
        .registrations
        .iter()
        .map(|registration| registration.machine)
        .collect();
    let machines =
        crate::routes::resources::view::machine_refs(&ctx.app, conn, &machine_ids).await?;
    let machine_registrations = full
        .registrations
        .iter()
        .map(|registration| {
            let machine = machines
                .get(&registration.machine)
                .cloned()
                .ok_or_else(|| ApiError::internal("a registered resource's machine is missing"))?;
            Ok(orch_api_contract::ownership::HoldingsMachineRegistration {
                object: registration.object.clone(),
                machine,
                threads: registration.threads,
            })
        })
        .collect::<ApiResult<Vec<_>>>()?;
    let shared_by_kind = prepared.shared_by_kind.clone();
    Ok(Holdings {
        shared: shared_by_kind.orgs
            + shared_by_kind.projects
            + shared_by_kind.topics
            + shared_by_kind.threads
            + shared_by_kind.documents
            + shared_by_kind.resources,
        nodes: prepared.nodes.clone(),
        shared_by_kind,
        resources: full.resources,
        machine_registrations,
        entries: prepared.entries,
        pending: prepared.affected.pending,
    })
}

/// What ending the membership of a person would touch.
async fn holdings(
    ctx: Ctx,
    (): (),
    (): (),
) -> ApiResult<orch_api_contract::ownership::HoldingsResponse> {
    crate::key_operations::native::selected_read::<holdings_read::Members>(ctx, (), ()).await
}

/// Ends a membership of the root org, with everything that goes with it.
async fn remove(ctx: Ctx, (): (), body: OrgMemberRemoveBody) -> ApiResult<()> {
    ending::run(ctx, body).await
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
async fn move_preview(
    ctx: Ctx,
    (): (),
    body: MemberMoveBody,
) -> ApiResult<orch_api_contract::ownership::MemberMovePreviewResponse> {
    move_review::run(ctx, (), body).await
}

/// Changes the domain of a member: the role they held at the old one is
/// taken from it and given on the new one in the same step.
async fn move_member(
    ctx: Ctx,
    (): (),
    body: MemberMoveBody,
) -> ApiResult<orch_api_contract::ownership::MemberMoveResponse> {
    moving::run(ctx, body).await
}

#[path = "members/ending.rs"]
pub mod ending;

#[path = "members/holdings_read.rs"]
pub mod holdings_read;

#[path = "members/move_review.rs"]
pub mod move_review;

#[path = "members/moving.rs"]
pub mod moving;
