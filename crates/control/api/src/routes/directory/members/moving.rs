//! The native membership move uses fresh immutable operands and typed completed readback.

use super::*;
use crate::key_operations::native::{effects, reply};
use crate::key_operations::publication;
use crate::key_operations::{native, targets};
use crate::middleware::transaction_guard;
use futures::future::BoxFuture;
use orch_access_contract::key_scope::{KeyIdentityKind, KeyTargetKind};
use orch_api_contract::operation_sources::Declaration;
use orch_api_contract::ownership::{MemberMoveReceipt, MemberMoveResponse};
use orch_api_contract::ownership_sources::{self as sources, Source};
use orch_authz::{Caller, Principal};
use std::collections::BTreeSet;
use std::sync::Arc;

const ORG: KeyTargetKind =
    KeyTargetKind::Object(orch_access_contract::permissions::ObjectKind::Org);
const ACCOUNT: KeyTargetKind = KeyTargetKind::Identity(KeyIdentityKind::Account);

pub(super) struct Move;
pub(super) struct Core {
    found: Found,
    destination: Placed,
    wanted: Role,
}
pub(super) struct Outcome {
    core: Arc<Core>,
}

async fn inputs(ctx: &Ctx, conn: &mut Conn, body: &MemberMoveBody) -> ApiResult<Core> {
    // The member's native row is the first serialization point, as in membership ending.
    let initial = found(ctx, conn).await?;
    let domain: Option<Id> = sqlx::query_scalar(
        "SELECT domain_id FROM org_members WHERE org_id=$1 AND user_id=$2 FOR UPDATE",
    )
    .bind(initial.org.org)
    .bind(initial.user)
    .fetch_optional(&mut *conn)
    .await
    .map_err(DbError::from)?;
    domain.ok_or_else(ApiError::not_found)?;
    let (selected, destination, _) = move_inputs(ctx, conn, body).await?;
    let nodes = [selected.org.org, selected.member.domain_id, destination.id]
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    sqlx::query("SELECT id FROM nodes WHERE id=ANY($1) ORDER BY id FOR UPDATE")
        .bind(&nodes)
        .fetch_all(&mut *conn)
        .await
        .map_err(DbError::from)?;
    sqlx::query("SELECT id FROM node_roles WHERE node_id=ANY($1) AND subject_kind='user' AND subject_id=$2 AND removed_at IS NULL ORDER BY node_id,id FOR UPDATE")
        .bind(vec![selected.member.domain_id, destination.id]).bind(selected.user).fetch_all(&mut *conn).await.map_err(DbError::from)?;
    let (found, destination, wanted) = move_inputs(ctx, conn, body).await?;
    if found.org.org != initial.org.org
        || found.user != initial.user
        || found.member.domain_id != selected.member.domain_id
    {
        return Err(ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "the membership changed while preparing its move",
        ));
    }
    Ok(Core {
        found,
        destination,
        wanted,
    })
}

async fn facts(conn: &mut Conn, core: &Core) -> ApiResult<Vec<native::Facts<Source>>> {
    let from = targets::node(conn, core.found.member.domain_id).await?;
    let to = targets::node(conn, core.destination.id).await?;
    let account = targets::account(conn, core.found.user).await?;
    Ok(vec![
        native::Facts::one(Source::MemberDomain, ORG, from),
        native::Facts::one(Source::MemberDestination, ORG, to),
        native::Facts::one(Source::MemberAccount, ACCOUNT, account),
    ])
}

impl effects::Effect for Move {
    type Source = Source;
    type Query = ();
    type Body = MemberMoveBody;
    type Value = Arc<Core>;
    type Output = Outcome;

    fn declaration(_value: &Arc<Core>) -> &'static Declaration<Source> {
        &sources::MEMBER_MOVE
    }

    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: MemberMoveBody,
    ) -> BoxFuture<'a, ApiResult<(Arc<Core>, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let (found, destination, wanted) = move_inputs(ctx, conn, &body).await?;
            let preliminary = Arc::new(Core {
                found,
                destination,
                wanted,
            });
            let known = native::bind_known(
                ctx,
                &sources::MEMBER_MOVE,
                &facts(conn, &preliminary).await?,
            )?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            Self::require(ctx, conn, &principal, &preliminary).await?;
            let core = Arc::new(inputs(ctx, conn, &body).await?);
            let facts = facts(conn, &core).await?;
            Ok((core, facts))
        })
    }

    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        core: &'a Arc<Core>,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            for org in [core.found.member.domain_id, core.destination.id] {
                authorization::require_projected(
                    conn,
                    principal,
                    org,
                    exact(Permission::MembersManage),
                    Act::Use,
                    Policy::default(),
                    native::modern(principal.caller()),
                )
                .await?;
            }
            if core.wanted == Role::Admin {
                let policy = policy_at(conn, core.destination.id).await?;
                admin_matters(conn, principal, core.destination.id, policy).await?;
            }
            Ok(())
        })
    }
}

impl effects::Apply for Move {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        core: &'a Arc<Core>,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Outcome>>> {
        Box::pin(async move {
            apply_native(ctx, conn, principal, core, move || before.check()).await?;
            Ok(effects::Outcome::Applied(Outcome {
                core: Arc::clone(core),
            }))
        })
    }
}

/// The same native writer/audit/notice, with its first effect after the complete move footprint.
async fn apply_native(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    core: &Core,
    before: impl FnOnce() -> ApiResult<()> + Send,
) -> ApiResult<()> {
    let root = core.found.root();
    let user = core.found.user;
    let domain = core.found.member.domain_id;
    let to = core.destination.id;
    let word: String = same(&core.wanted).ok_or_else(|| invalid("role", "is not a role"))?;
    before()?;
    let held = access::move_member(
        conn,
        root.id,
        user,
        domain,
        to,
        &word,
        principal.user(),
        ctx.now,
    )
    .await?;
    let move_epoch = orch_authz::epoch::bump(conn)
        .await
        .map_err(ApiError::internal)?;
    let who = actor_name(ctx)?;
    record(
        ctx,
        conn,
        orch_db::audit::action!(AuditAction::MemberMoved),
        root,
        &format!("{who} moved a member to another org of the tree"),
        json!({"user":user,"from":domain,"to":to,"role":held}),
    )
    .await?;
    hierarchy::stamp(conn, root.id).await?;
    orch_engine::notifications::tell(conn, &orch_engine::notifications::Event {
        kind:orch_engine::notifications::Kind::MemberMoved, node:Some(to), thread:None, approval:None,
        title:"Your place in the team changed".to_owned(), body:String::new(),
        data:json!({"user_id":user,"from":domain,"to":to,"role":word,"path":format!("/org/{to}")}),
        source:format!("{}:{user}:{move_epoch}", root.id), actor:Some(principal.user()), at:ctx.now,
    }, &[user], Some(root.id)).await.map_err(ApiError::internal)?;
    Ok(())
}

pub(super) async fn run(ctx: Ctx, _body: MemberMoveBody) -> ApiResult<MemberMoveResponse> {
    let (mut tx, _) = transaction_guard::begin(&ctx).await?;
    let completed = effects::apply::<Move>(&ctx, &mut tx).await?;
    // The native writes are still rollbackable. Refresh only the actual credential/lifetime:
    // replaying preparation here would mistake our completed destination for a new source.
    let identity = transaction_guard::current_http_identity(
        &completed.ctx,
        &mut tx,
        transaction_guard::HttpIdentityMode::Read,
    )
    .await?;
    identity.check_deadline(&completed.ctx.app)?;
    tx.commit().await.map_err(DbError::from)?;
    let core = &completed.output.core;
    let receipt = MemberMoveReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        org_id: core.found.org.org,
        user_id: core.found.user,
        from: core.found.member.domain_id,
        to: core.destination.id,
    };
    publication::receipt(&completed.ctx, &receipt)?;
    let full =
        reply::render::<Moved>(&completed.ctx, &completed.accepted, completed.output).await?;
    Ok(full.map_or_else(
        || MemberMoveResponse::Receipt(receipt),
        MemberMoveResponse::Full,
    ))
}

struct Moved;
struct Subject {
    found: Found,
}
struct Names {
    path: Vec<Id>,
}

impl reply::Projection for Moved {
    type Source = Source;
    type Input = Outcome;
    type Subject = Subject;
    type Value = Names;
    type Reply = Member;
    const READ_SOURCE: Source = Source::MemberAccount;

    fn declaration(_input: &Outcome) -> &'static Declaration<Source> {
        &sources::MEMBER_MOVE
    }

    fn subject<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Outcome,
    ) -> BoxFuture<'a, ApiResult<(Subject, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let selected = found(ctx, conn).await?;
            let facts = facts(conn, &input.core).await?;
            Ok((Subject { found: selected }, facts))
        })
    }

    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Outcome,
        subject: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if subject.found.org.org != input.core.found.org.org
                || subject.found.user != input.core.found.user
                || subject.found.member.domain_id != input.core.destination.id
            {
                return Err(ApiError::new(
                    ErrorCode::AuthzDenied,
                    "the moved member's response source changed",
                ));
            }
            may_end(conn, principal, &subject.found).await
        })
    }

    fn prepare<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Outcome,
        subject: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<(Names, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let path = domain_path(conn, subject.found.member.domain_id).await?;
            let graph = orch_authz::engine::Graph::load(conn, &path)
                .await
                .map_err(ApiError::internal)?;
            let mut targets = Vec::new();
            for id in &path {
                targets.push(
                    orch_engine::operation_scope::targets::from_node_identity(&graph, *id)
                        .ok_or_else(ApiError::not_found)?,
                );
            }
            let groups = if targets.is_empty() {
                Vec::new()
            } else {
                vec![(ORG, targets)]
            };
            Ok((
                Names { path },
                vec![native::Facts::collection(Source::MemberNames, groups)],
            ))
        })
    }

    fn render<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a Outcome,
        prepared: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<Member>> {
        Box::pin(async move {
            let selected = &prepared.subject.found;
            let people = refusals::user_refs(conn, &[selected.user]).await?;
            let refs = view::refs(
                conn,
                principal,
                &[selected.member.domain_id],
                Policy::default(),
            )
            .await?;
            member_of(&selected.member, &people, &refs, true)
                .ok_or_else(|| ApiError::internal("a moved member is not shown"))
        })
    }

    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Outcome,
        opened: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(opened.subject.found.user == fresh.subject.found.user
                && opened.subject.found.org.org == fresh.subject.found.org.org
                && opened.subject.found.member.domain_id == fresh.subject.found.member.domain_id
                && opened.value.path == fresh.value.path)
        })
    }
}
