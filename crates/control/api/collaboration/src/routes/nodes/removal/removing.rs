//! One native deletion preparation, the guarded first mutation and its intrinsic id result.

use super::*;

use crate::key_operations::native::effects;

use crate::key_operations::native;

use crate::key_operations::publication;

use crate::key_operations::targets;

use crate::middleware::transaction_guard;

use futures::future::BoxFuture;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::operation_sources::Declaration;

use orch_api_contract::ownership::DeletedResources;

use orch_api_contract::ownership::NodeDeletePreviewReceipt;

use orch_api_contract::ownership_sources as sources;

use orch_api_contract::ownership_sources::Source;

use orch_api_contract::routes::Endpoint;

use orch_authz::Caller;

use orch_authz::CapturedOperation;

use orch_authz::Principal;

use orch_engine::archive::deletion;

use std::collections::BTreeMap;

use std::collections::BTreeSet;

use std::sync::Arc;
pub(super) struct Delete;

struct Confirmation {
    id: Id,
    origins: Vec<CapturedOperation>,
    same: bool,
    review: Arc<DeleteReview>,
}

pub(super) struct Prepared {
    native: deletion::Prepared,
    keep_at: Option<Id>,
    role: String,
    confirmation: Option<Confirmation>,
    original_callers: Vec<Caller>,
}

pub(super) enum Outcome {
    Deleted {
        receipt: NodeDeleted,
        wake: Option<Id>,
        originals: Vec<Caller>,
    },
    Stale {
        saved: Box<SavedReview>,
        full: Option<Box<NodeDeletePlan>>,
        originals: Vec<Caller>,
    },
}

async fn side_allowed(
    conn: &mut Conn,
    principal: &Principal,
    side: Id,
    parent: Id,
) -> ApiResult<()> {
    let row = orch_db::queries::threads::thread(conn, side)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let first = authorization::require_projected(
        conn,
        principal,
        parent,
        MANAGE,
        Act::Use,
        Policy::default(),
        native::modern(principal.caller()),
    )
    .await
    .err();
    let path = orch_db::queries::threads::node_path(conn, parent).await?;
    let maker = row.owner_id == principal.user()
        && principal.caller().permits(Permission::ThreadManage)
        && principal.caller().reaches(Some(&path))
        && !principal.suspension().applies(&path);
    if first.is_some() && !maker {
        let managed = match row.environment_id {
            Some(environment) => authorization::require_resource_projected(
                conn,
                principal,
                orch_access_contract::permissions::ResourceKind::Environment,
                environment,
                Permission::EnvironmentManage,
                Policy::default(),
                native::modern(principal.caller()),
            )
            .await
            .is_ok(),
            None => false,
        };
        if !managed {
            return Err(first.unwrap_or_else(ApiError::not_found));
        }
    }
    Ok(())
}

async fn subject_allowed(
    conn: &mut Conn,
    principal: &Principal,
    node: Id,
    side: Option<Id>,
) -> ApiResult<()> {
    if let Some(parent) = side {
        side_allowed(conn, principal, node, parent).await
    } else {
        authorization::require_projected(
            conn,
            principal,
            node,
            MANAGE,
            Act::Delete,
            Policy::default(),
            native::modern(principal.caller()),
        )
        .await
    }
}

async fn require(conn: &mut Conn, principal: &Principal, value: &Prepared) -> ApiResult<()> {
    subject_allowed(
        conn,
        principal,
        value.native.node().id,
        value.native.side_parent(),
    )
    .await?;
    if let Some(parent) = value.native.member_destination() {
        for org in [value.native.node().id, parent] {
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
    }
    if let Some(parent) = value.keep_at {
        authorization::require_projected(
            conn,
            principal,
            parent,
            MANAGE,
            Act::Use,
            Policy::default(),
            native::modern(principal.caller()),
        )
        .await?;
    }
    Ok(())
}

async fn facts(
    conn: &mut Conn,
    prepared: &deletion::Prepared,
    keep_at: Option<Id>,
) -> ApiResult<Vec<native::Facts<Source>>> {
    let root = targets::node_identity(conn, prepared.node().id).await?;
    let mut groups = BTreeMap::<KeyTargetKind, Vec<_>>::new();
    for fact in prepared.resources() {
        let kind: ObjectKind = fact.kind.parse().map_err(ApiError::internal)?;
        let target = targets::resource_identity(
            conn,
            orch_access_contract::decisions::ObjectRef { kind, id: fact.id },
        )
        .await?;
        groups.entry(target.object.kind).or_default().push(target);
    }
    let mut destinations = Vec::new();
    for id in prepared
        .member_destination()
        .into_iter()
        .chain(keep_at)
        .collect::<BTreeSet<_>>()
    {
        destinations.push(targets::node_identity(conn, id).await?);
    }
    Ok(vec![
        native::Facts::one(Source::DeleteSubject, root.object.kind, root),
        native::Facts::collection(Source::DeletionResources, groups.into_iter().collect()),
        native::Facts::collection(
            Source::DeletionDestinations,
            if destinations.is_empty() {
                Vec::new()
            } else {
                vec![(KeyTargetKind::Object(ObjectKind::Org), destinations)]
            },
        ),
    ])
}

async fn confirmation(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    prepared: &deletion::Prepared,
    body: &NodeDeleteBody,
) -> ApiResult<Option<Confirmation>> {
    let Some(id) = body.preflight_id else {
        return Ok(None);
    };
    sqlx::query("SELECT id FROM preflights WHERE id=$1 FOR UPDATE")
        .bind(id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(DbError::from)?;
    let row = orch_db::queries::jobs::forks::preflight(conn, id)
        .await?
        .filter(|row| {
            row.kind == DELETE_PLAN_KIND
                && row.node_id == Some(prepared.node().id)
                && row.created_by == principal.user()
        })
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::PreflightExpired,
                "there is no such plan of this node",
            )
        })?;
    if row.expires_at <= ctx.app.clock.now() {
        return Err(ApiError::new(
            ErrorCode::PreflightExpired,
            "the plan is out of date: ask for it again",
        ));
    }
    let kept: KeptPlan = serde_json::from_value(row.plan).map_err(ApiError::internal)?;
    let purge = purge_at(ctx.now, ctx.app.config.retention.soft_delete_days);
    let revision = prepared.node().revision;
    let same = revision == kept.revision
        && body.expected_revision.is_none_or(|seen| seen == revision)
        && fingerprint_of(prepared.set()) == kept.fingerprint
        && purge.date_naive() <= kept.purge_at.date_naive();
    let review =
        Arc::new(prepare_delete_review(ctx, conn, prepared.node().id, prepared.set()).await?);
    Ok(Some(Confirmation {
        id,
        origins: kept.origins,
        same,
        review,
    }))
}

async fn original_callers(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    value: &Prepared,
    required: &[native::Facts<Source>],
) -> ApiResult<Vec<Caller>> {
    let mut callers = Vec::new();
    for origin in value
        .confirmation
        .iter()
        .flat_map(|confirmation| &confirmation.origins)
    {
        let refused = || {
            ApiError::new(
                ErrorCode::AuthzDenied,
                "the original deletion review is no longer allowed",
            )
        };
        if origin.user() != principal.user() {
            return Err(refused());
        }
        if origin.operation() == sources::DELETE_PREVIEW.operation {
            callers.extend(
                review::require_origins(
                    ctx,
                    conn,
                    principal,
                    value.native.node().id,
                    std::slice::from_ref(origin),
                )
                .await?,
            );
            continue;
        }
        if origin.operation() != sources::DELETE.operation {
            return Err(refused());
        }
        let original = origin
            .restore_current(conn, ctx.app.clock.as_ref())
            .await
            .map_err(authorization::restoration_error)?
            .ok_or_else(refused)?;
        let plan = native::bind_required_route(NodeDelete::ROUTE, &sources::DELETE, required)?;
        let caller = plan
            .shared()
            .admit(&original, false)
            .map_err(crate::key_operations::admission::binding_error)?;
        let original = authorization::load_principal(conn, caller, ctx.app.clock.time()).await?;
        let user = orch_db::queries::identity::user_by_id(conn, origin.user())
            .await?
            .ok_or_else(refused)?;
        let original = crate::sign_in::second_step::hold_key_origin(conn, &user, original).await?;
        require(conn, &original, value).await?;
        if !original
            .caller()
            .key_deadline_allows(ctx.app.clock.as_ref())
        {
            return Err(refused());
        }
        callers.push(original.caller().clone());
    }
    Ok(callers)
}

fn check_origins(ctx: &Ctx, callers: &[Caller]) -> ApiResult<()> {
    if callers
        .iter()
        .all(|caller| caller.key_deadline_allows(ctx.app.clock.as_ref()))
    {
        Ok(())
    } else {
        Err(ApiError::new(
            ErrorCode::AuthzDenied,
            "the original deletion review is no longer allowed",
        ))
    }
}

impl effects::Effect for Delete {
    type Source = Source;
    type Query = ();
    type Body = NodeDeleteBody;
    type Value = Prepared;
    type Output = Outcome;
    fn declaration(_: &Prepared) -> &'static Declaration<Source> {
        &sources::DELETE
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: NodeDeleteBody,
    ) -> BoxFuture<'a, ApiResult<(Prepared, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let id = ctx.id("id")?;
            let root = targets::node_identity(conn, id).await?;
            let known = native::bind_known(
                ctx,
                &sources::DELETE,
                &[native::Facts::one(
                    Source::DeleteSubject,
                    root.object.kind,
                    root,
                )],
            )?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            let (_, kind) = placed(conn, id).await?;
            let side = if kind == NodeKind::Thread {
                orch_db::queries::threads::forking::side_parent(conn, id).await?
            } else {
                None
            };
            subject_allowed(conn, &principal, id, side).await?;
            let keep_at = if kind == NodeKind::Org {
                let org = hierarchy::org(conn, id)
                    .await?
                    .ok_or_else(ApiError::not_found)?;
                if org.org_kind == "personal" {
                    return Err(invalid("id", "a personal org is never deleted"));
                }
                if body.resources == Some(DeletedResources::ToParent) {
                    Some(
                        hierarchy::with_ancestors(conn, &[id])
                            .await?
                            .into_iter()
                            .find(|row| row.id == id)
                            .and_then(|row| row.parent_id)
                            .ok_or_else(|| {
                                invalid(
                                    "resources",
                                    "a root org has no parent to receive its resources",
                                )
                            })?,
                    )
                } else {
                    None
                }
            } else {
                None
            };
            let native = ctx
                .app
                .engine
                .prepare_home_deletion_in(conn, id, keep_at)
                .await
                .map_err(crate::routes::environments::removal_schedule_error)?;
            if native.node().kind == "org"
                && hierarchy::org(conn, id)
                    .await?
                    .is_some_and(|org| org.org_kind == "personal")
            {
                return Err(invalid("id", "a personal org is never deleted"));
            }
            if keep_at.is_some() && native.node().parent_id != keep_at {
                return Err(ApiError::new(
                    ErrorCode::ConflictStaleRevision,
                    "the deletion destination changed meanwhile",
                ));
            }
            let confirmation = confirmation(ctx, conn, &principal, &native, &body).await?;
            let required = facts(conn, &native, keep_at).await?;
            let role = body
                .members
                .as_ref()
                .and_then(|members| members.role)
                .map_or_else(
                    || "viewer".to_owned(),
                    |role| same(&role).unwrap_or_default(),
                );
            let mut value = Prepared {
                native,
                keep_at,
                role,
                confirmation,
                original_callers: Vec::new(),
            };
            require(conn, &principal, &value).await?;
            value.original_callers =
                original_callers(ctx, conn, &principal, &value, &required).await?;
            Ok((value, required))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Prepared,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            require(conn, principal, value).await?;
            check_origins(ctx, &value.original_callers)
        })
    }
}

impl effects::Apply for Delete {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Prepared,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Outcome>>> {
        Box::pin(async move {
            let mut before = Some(before);
            let mut cut = || -> ApiResult<()> {
                check_origins(ctx, &value.original_callers)?;
                if let Some(check) = before.take() {
                    check.check()?;
                }
                Ok(())
            };
            if let Some(confirmation) = &value.confirmation {
                cut()?;
                orch_db::queries::jobs::forks::forget_preflight(conn, confirmation.id).await?;
                if !confirmation.same {
                    let saved = save_delete_review(
                        ctx,
                        conn,
                        principal,
                        Arc::clone(&confirmation.review),
                        || Ok(()),
                        &confirmation.origins,
                    )
                    .await?;
                    let full = if native::modern(principal.caller()) {
                        None
                    } else {
                        Some(Box::new(
                            render_delete_review(conn, principal, &saved).await?,
                        ))
                    };
                    return Ok(effects::Outcome::Applied(Outcome::Stale {
                        saved: Box::new(saved),
                        full,
                        originals: value.original_callers.clone(),
                    }));
                }
            }
            let id = value.native.node().id;
            if value.native.side_parent().is_some() {
                let wake = ctx
                    .app
                    .engine
                    .close_side_thread_before(
                        conn,
                        id,
                        "closed",
                        Some(principal.user()),
                        None,
                        principal.now(),
                        move || {
                            cut().map_err(|error| {
                                orch_engine::EngineError::BeforeEffect(Box::new(error))
                            })
                        },
                    )
                    .await
                    .map_err(ApiError::engine)?;
                return Ok(effects::Outcome::Applied(Outcome::Deleted {
                    receipt: NodeDeleted { deletion_id: None },
                    wake,
                    originals: value.original_callers.clone(),
                }));
            }
            let (place, kind) = placed(conn, id).await?;
            let node = value.native.node();
            if kind == NodeKind::Thread {
                let removed = ctx
                    .app
                    .engine
                    .delete_prepared_thread_before(
                        conn,
                        &value.native,
                        Some(principal.user()),
                        principal.now(),
                        move || {
                            cut().map_err(|error| {
                                orch_engine::EngineError::BeforeEffect(Box::new(error))
                            })
                        },
                    )
                    .await
                    .map_err(crate::routes::environments::removal_schedule_error)?;
                let settled = &removed.settled;
                let who = actor_name(ctx)?;
                record(ctx,conn,orch_db::audit::action!(AuditAction::NodeDeleted),place,&format!("{who} deleted the thread {}",node.name),
                    json!({"nodes":removed.gone,"archived_first":removed.archived_first,"threads":settled.threads,"turns":settled.turns,"inputs":settled.inputs,"approvals":settled.approvals,"environments":removed.environments.len(),"resources_before":[],"deleted_nodes":removed.deleted_nodes,"purge_at":purge_at(ctx.now,ctx.app.config.retention.soft_delete_days)})).await?;
            } else {
                if let Some(parent) = value.native.member_destination() {
                    cut()?;
                    hierarchy::move_members_up(
                        conn,
                        id,
                        parent,
                        &value.role,
                        principal.user(),
                        ctx.now,
                    )
                    .await?;
                }
                ctx.app
                    .engine
                    .begin_home_deletion_before(conn, &value.native, cut)
                    .await?;
                let deleted_nodes = value.native.node_ids();
                let archived_first =
                    hierarchy::archive(conn, id, principal.user(), ctx.now).await?;
                epoch::bump(conn).await.map_err(ApiError::internal)?;
                let settled = if archived_first
                    || orch_db::queries::lifecycle::archive_work_pending(conn, id).await?
                {
                    ctx.app
                        .engine
                        .archive_in(conn, id, principal.user(), principal.now())
                        .await
                        .map_err(ApiError::engine)?
                } else {
                    orch_engine::archive::Settled::default()
                };
                let (environments, resources_before) = if kind == NodeKind::Org {
                    ctx.app
                        .engine
                        .end_prepared_home_resources_in(
                            conn,
                            &value.native,
                            value.keep_at,
                            principal.user(),
                            principal.now(),
                            &origin(ctx),
                        )
                        .await
                        .map_err(ApiError::internal)?
                } else {
                    (
                        ctx.app
                            .engine
                            .remove_prepared_home_in(
                                conn,
                                &value.native,
                                principal.user(),
                                principal.now(),
                                value.keep_at,
                            )
                            .await
                            .map_err(crate::routes::environments::removal_schedule_error)?,
                        json!([]),
                    )
                };
                let gone = hierarchy::delete_subtree(conn, id, principal.user(), ctx.now)
                    .await?
                    .saturating_add(
                        hierarchy::include_settled_in_deletion(conn, id, &deleted_nodes).await?,
                    );
                let who = actor_name(ctx)?;
                record(ctx,conn,orch_db::audit::action!(AuditAction::NodeDeleted),place,&format!("{who} deleted the {} {}",kind_word(kind),node.name),
                    json!({"nodes":gone,"archived_first":archived_first,"threads":settled.threads,"turns":settled.turns,"inputs":settled.inputs,"approvals":settled.approvals,"environments":environments.len(),"resources_before":resources_before,"deleted_nodes":deleted_nodes,"purge_at":purge_at(ctx.now,ctx.app.config.retention.soft_delete_days)})).await?;
                hierarchy::stamp_subtree(conn, id).await?;
            }
            let deletion_id = orch_db::queries::trace::node_deletion(conn, id).await?;
            Ok(effects::Outcome::Applied(Outcome::Deleted {
                receipt: NodeDeleted { deletion_id },
                wake: None,
                originals: value.original_callers.clone(),
            }))
        })
    }
}

pub(super) async fn run(ctx: Ctx, _body: NodeDeleteBody) -> ApiResult<NodeDeleted> {
    let (mut tx, _) = transaction_guard::begin(&ctx).await?;
    let bounded = native::modern(&authorization::caller_of(ctx.user()?)?);
    let completed = effects::apply::<Delete>(&ctx, &mut tx)
        .await
        .map_err(|error| {
            if bounded {
                authorization::problem_without_content(error)
            } else {
                error
            }
        })?;
    let identity = transaction_guard::current_http_identity(
        &completed.ctx,
        &mut tx,
        transaction_guard::HttpIdentityMode::Read,
    )
    .await?;
    identity.check_deadline(&completed.ctx.app)?;
    // Every separate original review is checked before commit while all effects are rollbackable.
    // Its exact key rows and ordinary authority stay pinned by this same guarded transaction.
    let originals = match &completed.output {
        Outcome::Deleted { originals, .. } | Outcome::Stale { originals, .. } => originals,
    };
    check_origins(&completed.ctx, originals)?;
    tx.commit().await.map_err(DbError::from)?;
    match completed.output {
        Outcome::Deleted { receipt, wake, .. } => {
            publication::receipt(&completed.ctx, &receipt)?;
            if let Some(parent) = wake {
                completed.ctx.app.engine.wake(parent);
            }
            completed.ctx.app.engine.kick();
            Ok(receipt)
        }
        Outcome::Stale { saved, full, .. } => {
            if native::modern(&completed.accepted) {
                let receipt = NodeDeletePreviewReceipt {
                    projection: orch_api_contract::common::ReceiptProjection::Receipt,
                    preflight_id: saved.preflight.id,
                    expires_at: Time::new(saved.preflight.expires_at),
                    node_id: saved.review.node.id,
                    revision: saved.review.node.revision,
                    counts: counts_of(&saved.review.set),
                    purge_at: Time::new(saved.review.purge_at),
                };
                Err(ApiError::new(ErrorCode::ConflictStaleRevision,"what this deletion would remove changed since the plan: confirm the plan as it now is").with("current",receipt))
            } else {
                Err(stale_delete_plan(*full.ok_or_else(|| {
                    ApiError::internal("the ordinary stale deletion lost its native full review")
                })?))
            }
        }
    }
}
