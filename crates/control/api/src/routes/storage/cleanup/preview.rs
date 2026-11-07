//! One native prepared preview, its real first save and independently admitted stored metadata.
use super::super::{
    PreparedRecords, Scope,
    report_sources::{self, Book, OperatorSnapshot},
};
use super::*;
use crate::key_operations::{
    native::{
        effects::{self, ApplyOwned, Effect, FirstEffect, Initial, Outcome},
        reply,
    },
    publication,
};
use crate::registry::Ctx;
use futures::future::BoxFuture;
use orch_api_contract::{
    operation_sources::Declaration,
    storage_sources::{self as sources, Source},
};
use orch_authz::{Caller, Principal};
use orch_db::{Conn, Tx, queries::jobs::forks::PreflightRow};
use orch_engine::jobs::cleanup::sources::Sources;
use orch_types::Time;

pub(super) struct Preview;
pub(super) struct Intent {
    scope: Scope,
}
pub(super) struct Prepared {
    request: api::CleanupRequest,
    records: PreparedRecords,
    sources: Sources,
    resource_name: Option<String>,
}
pub(super) struct Saved {
    row: PreflightRow,
    plan: api::CleanupPlan,
    sources: Sources,
}

impl Effect for Preview {
    type Source = Source;
    type Query = ();
    type Body = api::CleanupRequest;
    type Value = Prepared;
    type Output = Saved;
    fn declaration(_: &Prepared) -> &'static Declaration<Source> {
        &sources::PREFLIGHT
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        request: api::CleanupRequest,
    ) -> BoxFuture<'a, ApiResult<(Prepared, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let query = query(&request);
            let scope = Scope::parse(&query)?;
            let selected =
                (!request.items.is_empty()).then(|| request.items.iter().cloned().collect());
            let mut book = Book::for_operation(
                ctx,
                conn,
                caller,
                scope,
                selected,
                &sources::PREFLIGHT,
                true,
            )
            .await?;
            let principal = native::preparation_principal(ctx, conn, caller, &book.plan).await?;
            report_sources::require_namespace(conn, &principal, scope).await?;
            let operator = effects::initial_value::<Preview>(ctx)?;
            let records = super::super::prepare_records(
                ctx,
                conn,
                &principal,
                &query,
                Some(operator.as_ref()),
                Some(&mut book),
            )
            .await?;
            let resource_name = if let Some(object) = request.resource {
                Some(
                    orch_engine::resource_facts::authorities(conn, &principal, &[object.id])
                        .await
                        .map_err(ApiError::internal)?
                        .into_iter()
                        .find(|row| row.resource.object() == object && row.manageable)
                        .map(|row| row.resource.name)
                        .ok_or_else(ApiError::not_found)?,
                )
            } else {
                None
            };
            let sources = book.snapshot()?;
            let facts = book.facts();
            Ok((
                Prepared {
                    request,
                    records,
                    sources,
                    resource_name,
                },
                facts,
            ))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        _: &'a mut Conn,
        principal: &'a Principal,
        _: &'a Prepared,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if ctx.user()?.id() != principal.user() {
                return Err(ApiError::not_found());
            }
            Ok(())
        })
    }
}
impl Initial for Preview {
    type Input = Intent;
    type Fetched = OperatorSnapshot;
    fn declaration(_: &Intent) -> &'static Declaration<Source> {
        &sources::PREFLIGHT
    }
    fn authorize<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        request: api::CleanupRequest,
    ) -> BoxFuture<'a, ApiResult<(Intent, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let scope = Scope::parse(&query(&request))?;
            let target = report_sources::namespace(conn, caller, scope).await?;
            Ok((
                Intent { scope },
                vec![native::Facts::one(
                    Source::Namespace,
                    target.object.kind,
                    target,
                )],
            ))
        })
    }
    fn require_initial<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        intent: &'a Intent,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(report_sources::require_namespace(
            conn,
            principal,
            intent.scope,
        ))
    }
    fn fetch<'a>(
        ctx: &'a Ctx,
        caller: &'a Caller,
        intent: Intent,
    ) -> BoxFuture<'a, ApiResult<OperatorSnapshot>> {
        Box::pin(report_sources::fetch_operator(ctx, caller, intent.scope))
    }
}
impl ApplyOwned for Preview {
    fn apply_owned<'a>(
        ctx: &'a Ctx,
        mut tx: Tx,
        principal: &'a Principal,
        value: &'a Prepared,
        before: FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<Outcome<Saved>>> {
        Box::pin(async move {
            let deadline = before.deadline();
            let (_, rows, _) = value.records.render()?;
            let (items, targets) = plan_items(ctx, rows, value.records.targets.clone());
            let name = value
                .resource_name
                .clone()
                .or_else(|| {
                    items
                        .iter()
                        .find_map(|item| item.record.host.as_ref())
                        .filter(|_| value.request.scope.starts_with("host:"))
                        .map(|host| host.name.clone())
                })
                .unwrap_or_else(|| "Storage".into());
            let plan = api::CleanupPlan {
                id: new_id(),
                expires_at: Time::new(orch_engine::updates::expires(ctx.now)),
                confirmation_name: name,
                items,
                people: Vec::new(),
                other_people: 0,
                notify: false,
            };
            let operation = orch_authz::CapturedOperation::capture(principal.caller())
                .map_err(ApiError::internal)?;
            let row = PreflightRow {
                id: plan.id,
                kind: cleanup::KIND.into(),
                node_id: None,
                environment_id: None,
                created_by: principal.user(),
                plan: json!({"request": value.request, "preview": plan, "targets": targets, "sources": value.sources, "operation": operation}),
                expires_at: plan.expires_at.get(),
                created_at: ctx.now,
            };
            jobs::forks::save_preflight_before(&mut tx, &row, || before.check()).await?;
            deadline.check()?;
            tx.commit().await.map_err(DbError::from)?;
            Ok(Outcome::Applied(Saved {
                row,
                plan,
                sources: value.sources.clone(),
            }))
        })
    }
}

pub(super) async fn run(
    ctx: Ctx,
    (): (),
    _: api::CleanupRequest,
) -> ApiResult<api::CleanupPlanResponse> {
    let (tx, _) = transaction_guard::begin(&ctx).await?;
    let done = effects::apply_owned::<Preview>(&ctx, tx).await?;
    let receipt = api::CleanupPlanReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        id: done.output.plan.id,
        expires_at: done.output.plan.expires_at,
        items: done
            .output
            .plan
            .items
            .iter()
            .map(|item| api::CleanupItemReceipt {
                id: item.record.id.clone(),
                choices: item.choices.clone(),
                selected: item.selected,
                blocker: item.record.blocker.clone(),
            })
            .collect(),
    };
    publication::receipt(&done.ctx, &receipt)?;
    let full = match reply::render::<Stored>(&done.ctx, &done.accepted, done.output).await {
        Ok(full) => full,
        Err(error) if matches!(error.code(), ErrorCode::AuthzDenied | ErrorCode::NotFound) => None,
        Err(error) => return Err(error),
    };
    Ok(full.map_or_else(
        || api::CleanupPlanResponse::Receipt(receipt),
        api::CleanupPlanResponse::full,
    ))
}

struct Stored;
impl reply::Projection for Stored {
    type Source = Source;
    type Input = Saved;
    type Subject = PreflightRow;
    type Value = ();
    type Reply = api::CleanupPlan;
    const READ_SOURCE: Source = Source::Content;
    fn declaration(_: &Saved) -> &'static Declaration<Source> {
        &sources::PREFLIGHT
    }
    fn subject<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Saved,
    ) -> BoxFuture<'a, ApiResult<(PreflightRow, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = jobs::forks::preflight(conn, input.row.id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let facts = input
                .sources
                .facts(conn)
                .await
                .map_err(ApiError::engine)?
                .ok_or_else(ApiError::not_found)?;
            Ok((row, facts))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Saved,
        row: &'a PreflightRow,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if row.kind != cleanup::KIND
                || row.created_by != principal.user()
                || row.created_by != input.row.created_by
                || row.plan != input.row.plan
                || row.expires_at <= ctx.now
                || !input
                    .sources
                    .may_read(conn, principal)
                    .await
                    .map_err(ApiError::engine)?
            {
                return Err(ApiError::new(
                    ErrorCode::AuthzDenied,
                    "the stored cleanup preview is no longer readable",
                ));
            }
            Ok(())
        })
    }
    fn prepare<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Saved,
        _: &'a PreflightRow,
    ) -> BoxFuture<'a, ApiResult<((), Vec<native::Facts<Source>>)>> {
        Box::pin(async { Ok(((), Vec::new())) })
    }
    fn render<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        input: &'a Saved,
        _: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<api::CleanupPlan>> {
        Box::pin(async move { Ok(input.plan.clone()) })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Saved,
        old: &'a reply::Prepared<Self>,
        now: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(old.subject.plan == now.subject.plan
                && old.subject.created_by == now.subject.created_by
                && old.subject.expires_at == now.subject.expires_at)
        })
    }
}
