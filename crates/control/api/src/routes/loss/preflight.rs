//! Native review persistence and current immutable-preview disclosure through shared adapters.

use super::*;
use crate::key_operations::{
    native::{self, effects, reply},
    publication, targets,
};
use futures::future::BoxFuture;
use orch_access_contract::{
    decisions::ObjectRef,
    key_scope::{KeyResolvedTarget, KeyTargetKind},
    permissions::ObjectKind,
};
use orch_api_contract::{
    loss_sources::{DECLARE, PREFLIGHT, Source},
    operation_sources::Declaration,
    operations::FacetUsage,
};
use orch_authz::{Caller, Principal, engine::Graph};
use orch_db::{
    Conn, DbError,
    queries::jobs::forks::{self, PreflightRow},
};
use orch_engine::loss::{self, PreparedLossDeclaration, PreparedLossPreview, ProjectedLossPreview};
use std::{
    collections::{BTreeMap, HashSet},
    sync::Arc,
};

pub(super) struct Review;
pub(super) struct Ready {
    native: Arc<PreparedLossPreview>,
}
pub(super) struct Saved {
    row: Option<PreflightRow>,
    native: Arc<PreparedLossPreview>,
    declared: Option<Arc<PreparedLossDeclaration>>,
}
impl Saved {
    pub(super) fn declared(declared: Arc<PreparedLossDeclaration>) -> Self {
        Self {
            native: declared.preview(),
            row: None,
            declared: Some(declared),
        }
    }
}
fn declaration(input: &Saved) -> &'static Declaration<Source> {
    if input.declared.is_some() {
        &DECLARE
    } else {
        &PREFLIGHT
    }
}

pub(super) async fn affected_facts(
    conn: &mut Conn,
    objects: &[ObjectRef],
) -> ApiResult<native::Facts<Source>> {
    let mut groups = BTreeMap::<_, Vec<_>>::new();
    for object in objects {
        let target = match object.kind {
            ObjectKind::Machine => targets::machine_subject(conn, object.id).await?,
            ObjectKind::Thread => targets::node_identity(conn, object.id).await?,
            _ => targets::resource_identity(conn, *object).await?,
        };
        groups
            .entry(KeyTargetKind::Object(object.kind))
            .or_default()
            .push(target);
    }
    Ok(native::Facts::collection(
        Source::Affected,
        groups.into_iter().collect(),
    ))
}
async fn known_facts(conn: &mut Conn, input: &Saved) -> ApiResult<Vec<native::Facts<Source>>> {
    let mut facts = vec![target_fact(conn, input.native.target()).await?];
    if let Some(declared) = &input.declared {
        facts.push(affected_facts(conn, declared.affected()).await?);
    }
    Ok(facts)
}

pub(super) async fn target_fact(
    conn: &mut Conn,
    target: api::LossTarget,
) -> ApiResult<native::Facts<Source>> {
    let object = target.object();
    let identity = if object.kind == ObjectKind::Machine {
        targets::machine_subject(conn, object.id).await?
    } else {
        targets::resource_identity(conn, object).await?
    };
    Ok(native::Facts::one(
        Source::Target,
        KeyTargetKind::Object(object.kind),
        identity,
    ))
}
pub(super) async fn require_target(
    conn: &mut Conn,
    principal: &Principal,
    target: api::LossTarget,
) -> ApiResult<()> {
    if loss::allowed(conn, principal, target)
        .await
        .map_err(refused)?
    {
        Ok(())
    } else {
        Err(refused(LossError::Denied))
    }
}

impl effects::Effect for Review {
    type Source = Source;
    type Query = ();
    type Body = api::LossPreflightBody;
    type Value = Ready;
    type Output = Saved;
    fn declaration(_: &Ready) -> &'static Declaration<Source> {
        &PREFLIGHT
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: Self::Body,
    ) -> BoxFuture<'a, ApiResult<(Ready, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let known = vec![target_fact(conn, body.target).await?];
            let plan = native::bind_required(ctx, &PREFLIGHT, &known)?;
            let principal = plan.preparation_principal(ctx, conn, caller).await?;
            require_target(conn, &principal, body.target).await?;
            if !orch_db::queries::resources::lock_resource(
                conn,
                body.target.kind(),
                body.target.id(),
            )
            .await?
            {
                return Err(ApiError::not_found());
            }
            let facts = vec![target_fact(conn, body.target).await?];
            let plan = native::bind_required(ctx, &PREFLIGHT, &facts)?;
            let principal = plan.preparation_principal(ctx, conn, caller).await?;
            let prepared = ctx
                .app
                .engine
                .prepare_loss_preview(conn, &principal, body.target, orch_types::new_id())
                .await
                .map_err(refused)?;
            Ok((
                Ready {
                    native: Arc::new(prepared),
                },
                facts,
            ))
        })
    }
    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Ready,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if principal.user() != value.native.creator() {
                return Err(ApiError::not_found());
            }
            require_target(conn, principal, value.native.target()).await
        })
    }
}
impl effects::Apply for Review {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Ready,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Saved>>> {
        Box::pin(async move {
            let deadline = before.deadline();
            let origin = orch_authz::CapturedOperation::capture(principal.caller())
                .map_err(ApiError::internal)?;
            before.check()?;
            ctx.app
                .engine
                .save_loss_preview_with_origin(conn, &value.native, &origin)
                .await
                .map_err(refused)?;
            let row = forks::preflight(conn, value.native.plan().preflight_id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            deadline.check()?;
            Ok(effects::Outcome::Applied(Saved {
                row: Some(row),
                native: Arc::clone(&value.native),
                declared: None,
            }))
        })
    }
}

pub(super) async fn run(
    ctx: Ctx,
    (): (),
    _: api::LossPreflightBody,
) -> ApiResult<api::LossPreflightResponse> {
    let (mut tx, _) = crate::middleware::transaction_guard::begin(&ctx).await?;
    let done = effects::apply::<Review>(&ctx, &mut tx).await?;
    tx.commit().await.map_err(DbError::from)?;
    let receipt = api::LossPreflightReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        preflight_id: done.output.native.plan().preflight_id,
        expires_at: done.output.native.plan().expires_at,
    };
    publication::receipt(&done.ctx, &receipt)?;
    let full = match reply::render::<Content>(&done.ctx, &done.accepted, done.output).await {
        Ok(full) => full,
        Err(error) if matches!(error.code(), ErrorCode::AuthzDenied | ErrorCode::NotFound) => None,
        Err(error) => return Err(error),
    };
    Ok(match full {
        Some(plan) => api::LossPreflightResponse::Full(Box::new(plan)),
        None => api::LossPreflightResponse::Receipt(receipt),
    })
}

pub(super) struct Content;
pub(super) struct Subject {
    row: Option<PreflightRow>,
    related: Vec<(ObjectRef, KeyResolvedTarget)>,
}
fn related_fact(related: &[(ObjectRef, KeyResolvedTarget)]) -> native::Facts<Source> {
    let mut groups = BTreeMap::<_, Vec<_>>::new();
    for (object, target) in related {
        groups
            .entry(KeyTargetKind::Object(object.kind))
            .or_default()
            .push(target.clone());
    }
    native::Facts::collection(Source::Related, groups.into_iter().collect())
}
impl reply::Projection for Content {
    type Source = Source;
    type Input = Saved;
    type Subject = Subject;
    type Value = ProjectedLossPreview;
    type Reply = api::LossPlan;
    const READ_SOURCE: Source = Source::Target;
    fn declaration(input: &Saved) -> &'static Declaration<Source> {
        declaration(input)
    }
    fn subject<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Saved,
    ) -> BoxFuture<'a, ApiResult<(Subject, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = match &input.row {
                Some(row) => Some(
                    forks::preflight(conn, row.id)
                        .await?
                        .ok_or_else(ApiError::not_found)?,
                ),
                None => None,
            };
            let inventory = input.native.inventory();
            let objects = inventory
                .environments
                .iter()
                .map(|row| ObjectRef {
                    kind: ObjectKind::Environment,
                    id: row.id,
                })
                .chain(inventory.providers.iter().map(|id| ObjectRef {
                    kind: ObjectKind::Provider,
                    id: *id,
                }))
                .chain(inventory.mounts.iter().map(|id| ObjectRef {
                    kind: ObjectKind::Mount,
                    id: *id,
                }));
            let mut related = Vec::new();
            for object in objects {
                related.push((object, targets::resource_identity(conn, object).await?));
            }
            let mut facts = known_facts(conn, input).await?;
            facts.push(related_fact(&related));
            Ok((Subject { row, related }, facts))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Saved,
        subject: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if input.native.creator() != principal.user() {
                return Err(ApiError::not_found());
            }
            match (&input.row, &subject.row, &input.declared) {
                (Some(original), Some(row), None)
                    if row == original
                        && row.kind == "loss"
                        && row.created_by == principal.user()
                        && row.expires_at > ctx.app.clock.now() =>
                {
                    ()
                }
                (None, None, Some(declared))
                    if declared.current(conn).await.map_err(refused)? =>
                {
                    ()
                }
                _ => return Err(ApiError::not_found()),
            }
            require_target(conn, principal, input.native.target()).await
        })
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Saved,
        subject: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<(ProjectedLossPreview, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let mut facts = known_facts(conn, input).await?;
            facts.push(related_fact(&subject.related));
            let declaration = declaration(input);
            let plan = native::bind_known(ctx, declaration, &facts)?;
            let mut allowed = HashSet::new();
            for (object, target) in &subject.related {
                if native::permits_usage(
                    &plan,
                    declaration,
                    principal.caller(),
                    &Source::Related,
                    FacetUsage::ResponseContent,
                    KeyTargetKind::Object(object.kind),
                    target,
                )? {
                    allowed.insert(*object);
                }
            }
            let ids = input
                .native
                .named_sources()
                .iter()
                .filter(|object| object.kind == ObjectKind::Thread)
                .map(|object| object.id)
                .collect::<Vec<_>>();
            let graph = Graph::load(conn, &ids).await.map_err(ApiError::internal)?;
            let mut threads = BTreeMap::new();
            for id in ids {
                let Some(target) =
                    orch_engine::operation_scope::targets::from_node_identity(&graph, id)
                else {
                    continue;
                };
                if native::permits_usage(
                    &plan,
                    declaration,
                    principal.caller(),
                    &Source::ThreadNames,
                    FacetUsage::ResponseContent,
                    KeyTargetKind::Object(ObjectKind::Thread),
                    &target,
                )? {
                    allowed.insert(ObjectRef {
                        kind: ObjectKind::Thread,
                        id,
                    });
                    threads.insert(id, target);
                }
            }
            let projected = loss::project_loss_preview(
                conn,
                principal,
                &input.native,
                Some(&loss::NameBounds::new(allowed)),
            )
            .await
            .map_err(refused)?;
            let actual = projected
                .named_sources()
                .iter()
                .filter(|object| object.kind == ObjectKind::Thread)
                .filter_map(|object| threads.remove(&object.id))
                .collect::<Vec<_>>();
            let facts = vec![native::Facts::collection(
                Source::ThreadNames,
                if actual.is_empty() {
                    Vec::new()
                } else {
                    vec![(KeyTargetKind::Object(ObjectKind::Thread), actual)]
                },
            )];
            Ok((projected, facts))
        })
    }
    fn render<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Saved,
        prepared: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<api::LossPlan>> {
        Box::pin(async move { Ok(prepared.value.plan().clone()) })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Saved,
        opened: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(opened.subject.row == fresh.subject.row
                && opened
                    .value
                    .named_sources()
                    .is_subset(fresh.value.named_sources()))
        })
    }
}
