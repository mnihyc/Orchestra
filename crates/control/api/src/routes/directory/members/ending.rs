//! Membership ending consumes the native serialized ownership/registration plan once.

use super::*;
use crate::key_operations::native::effects;
use crate::key_operations::{native, targets};
use crate::middleware::transaction_guard;
use crate::routes::nodes::read_sources::Sources;
use futures::future::BoxFuture;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::KeyTargetKind;
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::operation_sources::Declaration;
use orch_api_contract::ownership_sources::{self as sources, Source};
use orch_authz::ownership::Disposal;
use orch_authz::{Caller, Principal};
use orch_engine::membership::{EndError, EndFailure, PreparedMemberEnding};
use std::collections::{BTreeMap, BTreeSet, HashMap};

pub(super) struct End;
pub(super) struct Prepared {
    found: Found,
    ending: PreparedMemberEnding,
    choices: HashMap<Id, Disposal>,
    leave: bool,
}

fn declaration(leave: bool) -> &'static Declaration<Source> {
    if leave {
        &sources::MEMBER_LEAVE
    } else {
        &sources::MEMBER_REMOVE
    }
}

async fn require(conn: &mut Conn, principal: &Principal, found: &Found) -> ApiResult<()> {
    may_end(conn, principal, found).await.map_err(|error| {
        if native::modern(principal.caller()) {
            authorization::problem_without_content(error)
        } else {
            error
        }
    })?;
    let owners = access::owner_users(conn, found.root().id).await?;
    if owners.contains(&found.user) && owners.iter().all(|owner| *owner == found.user) {
        return Err(ApiError::new(
            ErrorCode::OwnerLast,
            "the last owner of a root org names another owner first",
        ));
    }
    Ok(())
}

fn choices(body: OrgMemberRemoveBody) -> ApiResult<HashMap<Id, Disposal>> {
    body.resources
        .unwrap_or_default()
        .into_iter()
        .map(|(id, fate)| {
            let id = id
                .parse()
                .map_err(|_| invalid("resources", "a resource choice must name its identifier"))?;
            let fate = match fate {
                orch_api_contract::ownership::ResourceFate::Keep => Disposal::Keep,
                orch_api_contract::ownership::ResourceFate::Disable => Disposal::Disable,
                orch_api_contract::ownership::ResourceFate::Remove => Disposal::Remove,
            };
            Ok((id, fate))
        })
        .collect()
}

async fn error(conn: &mut Conn, principal: &Principal, error: EndError) -> ApiError {
    let error = match error {
        EndError::Denied(refusal) => match refusals::denied(conn, *refusal).await {
            Ok(error) | Err(error) => error,
        },
        EndError::NotFound => ApiError::not_found(),
        EndError::Choice(reason) => invalid("resources", &reason),
        EndError::InUse(resource) => ApiError::new(
            ErrorCode::ResourceInUse,
            "a selected resource must be released before it can be removed",
        )
        .with("resource", resource),
        EndError::Ownership(orch_authz::ownership::EndError::Stuck(_)) => ApiError::new(
            ErrorCode::OwnerLast,
            "name a surviving owner before ending this membership",
        ),
        error => ApiError::internal(error),
    };
    if native::modern(principal.caller()) {
        authorization::problem_without_content(error)
    } else {
        error
    }
}

async fn domain(conn: &mut Conn, member: &Found) -> ApiResult<native::Facts<Source>> {
    let target = targets::node(conn, member.member.domain_id).await?;
    Ok(native::Facts::one(
        Source::MemberDomain,
        target.object.kind,
        target,
    ))
}

async fn facts(conn: &mut Conn, prepared: &Prepared) -> ApiResult<Vec<native::Facts<Source>>> {
    let mut objects = prepared
        .ending
        .plan()
        .things
        .iter()
        .map(|passing| (passing.object.kind, passing.object.id))
        .collect::<BTreeSet<_>>();
    for row in prepared.ending.registrations() {
        objects.insert((
            row.kind.parse::<ObjectKind>().map_err(ApiError::internal)?,
            row.id,
        ));
    }
    let nodes = orch_authz::engine::Graph::load(conn, prepared.ending.member_nodes())
        .await
        .map_err(ApiError::internal)?;
    for id in prepared.ending.member_nodes() {
        let target = orch_engine::operation_scope::targets::from_node_identity(&nodes, *id)
            .ok_or_else(ApiError::not_found)?;
        let KeyTargetKind::Object(kind) = target.object.kind else {
            return Err(ApiError::internal("a native owned node has no node kind"));
        };
        objects.insert((kind, *id));
    }
    let identities =
        Sources::from_objects(objects.iter().map(|&(kind, id)| ObjectRef { kind, id }))
            .identity_targets(conn)
            .await?;
    let mut effects = BTreeMap::<KeyTargetKind, Vec<_>>::new();
    for (kind, id) in objects {
        let target = identities
            .get(&ObjectRef { kind, id })
            .cloned()
            .ok_or_else(ApiError::not_found)?;
        effects
            .entry(KeyTargetKind::Object(kind))
            .or_default()
            .push(target);
    }
    let mut destinations = Vec::new();
    for id in prepared.ending.destinations() {
        destinations.push(targets::node(conn, *id).await?);
    }
    Ok(vec![
        domain(conn, &prepared.found).await?,
        native::Facts::collection(Source::MemberEffects, effects.into_iter().collect()),
        native::Facts::collection(
            Source::MemberDestinations,
            if destinations.is_empty() {
                Vec::new()
            } else {
                vec![(KeyTargetKind::Object(ObjectKind::Org), destinations)]
            },
        ),
    ])
}

impl effects::Effect for End {
    type Source = Source;
    type Query = ();
    type Body = OrgMemberRemoveBody;
    type Value = Prepared;
    type Output = ();
    fn declaration(value: &Prepared) -> &'static Declaration<Source> {
        declaration(value.leave)
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: OrgMemberRemoveBody,
    ) -> BoxFuture<'a, ApiResult<(Prepared, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let member = found(ctx, conn).await?;
            let leave = member.user == caller.user();
            let known =
                native::bind_known(ctx, declaration(leave), &[domain(conn, &member).await?])?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            require(conn, &principal, &member).await?;
            let choices = choices(body)?;
            let ending = ctx
                .app
                .engine
                .prepare_member_ending_in(conn, &principal, member.org.org, member.user, &choices)
                .await;
            let ending = match ending {
                Ok(ending) => ending,
                Err(failure) => return Err(error(conn, &principal, failure).await),
            };
            let member = found(ctx, conn).await?;
            if member.member.domain_id != ending.domain() {
                return Err(ApiError::new(
                    ErrorCode::ConflictStaleRevision,
                    "the membership changed while preparing its ending",
                ));
            }
            let prepared = Prepared {
                found: member,
                ending,
                choices,
                leave,
            };
            let facts = facts(conn, &prepared).await?;
            Ok((prepared, facts))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        prepared: &'a Prepared,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require(conn, principal, &prepared.found))
    }
}

impl effects::Apply for End {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        prepared: &'a Prepared,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<()>>> {
        Box::pin(async move {
            let leaving = ctx
                .app
                .engine
                .end_prepared_member_in(
                    conn,
                    principal,
                    &prepared.ending,
                    &prepared.choices,
                    &crate::sign_in::origin(ctx),
                    move || before.check(),
                )
                .await;
            let leaving = match leaving {
                Ok(leaving) => leaving,
                Err(EndFailure::BeforeEffect(error)) => return Err(error),
                Err(EndFailure::Native(failure)) => {
                    return Err(error(conn, principal, failure).await);
                }
            };
            finish(ctx, conn, principal, prepared, leaving).await?;
            Ok(effects::Outcome::Applied(()))
        })
    }
}

async fn finish(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    prepared: &Prepared,
    leaving: orch_engine::membership::Leaving,
) -> ApiResult<()> {
    let found = &prepared.found;
    let root = found.root();
    let choices = &prepared.choices;
    let mut ctx = ctx.clone();
    ctx.now = leaving.at.get();
    let ended = access::end_membership_in_set(
        conn,
        root.id,
        found.user,
        principal.user(),
        ctx.now,
        prepared.ending.member_nodes(),
    )
    .await?;
    for node in ended.passed.iter().map(|(node, _)| node).chain(&ended.left) {
        hierarchy::stamp(conn, *node).await?;
    }
    // The ownership ending moved the epoch in this same transaction.
    let who = actor_name(&ctx)?;
    let person = identity::users_brief(conn, &[found.user])
        .await?
        .into_iter()
        .next()
        .map(|brief| brief.display_name)
        .unwrap_or_default();
    let org = refusals::named_node(conn, root.id)
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
        conn,
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
    hierarchy::stamp(conn, root.id).await?;
    Ok(())
}

pub(super) async fn run(ctx: Ctx, _body: OrgMemberRemoveBody) -> ApiResult<()> {
    let (mut tx, _) = transaction_guard::begin(&ctx).await?;
    let completed = effects::apply::<End>(&ctx, &mut tx).await?;
    let identity = transaction_guard::current_http_identity(
        &completed.ctx,
        &mut tx,
        transaction_guard::HttpIdentityMode::Read,
    )
    .await?;
    identity.check_deadline(&completed.ctx.app)?;
    tx.commit().await.map_err(DbError::from)?;
    ctx.app.engine.kick();
    Ok(())
}
