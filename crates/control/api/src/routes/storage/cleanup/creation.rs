//! Native confirmed cleanup creation, its original sources and independently admitted job metadata.
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
    publication, targets,
};
use crate::middleware::authentication::Via;
use futures::future::BoxFuture;
use orch_access_contract::key_scope::{KeyIdentityKind, KeyTargetKind};
use orch_api_contract::{
    operation_sources::Declaration,
    storage_sources::{self as sources, Source},
    workspace::Job,
};
use orch_authz::{Caller, Principal};
use orch_db::{Conn, DbError, Tx, queries::jobs::forks::PreflightRow};
use orch_engine::jobs::{
    cleanup::{
        Recipe,
        operation::{self, Origin},
        sources::{Reference, Sources},
    },
    runner::initial_progress,
};
use orch_types::{Id, new_id};
use serde_json::json;

pub(super) struct Create;
pub(super) struct Intent {
    scope: Scope,
    replay: bool,
}
pub(super) struct Prepared {
    stored: Option<PreflightRow>,
    replay: Option<jobs::JobRow>,
    request: api::CleanupRequest,
    plan: api::CleanupPlan,
    targets: HashMap<String, Target>,
    records: Option<PreparedRecords>,
    body: api::CleanupStart,
    sources: Sources,
    preview: Option<Origin>,
    preview_deadline: Option<operation::Deadline>,
}
pub(super) struct Saved {
    row: jobs::JobRow,
    sources: Sources,
}

struct Accepted {
    id: Id,
    sources: Sources,
    recipe: Option<serde_json::Value>,
}
impl From<Saved> for Accepted {
    fn from(value: Saved) -> Self {
        Self {
            id: value.row.id,
            sources: value.sources,
            recipe: Some(value.row.plan["recipe"].clone()),
        }
    }
}
pub(super) fn replay_binding()
-> crate::middleware::idempotency::Creation<(), api::CleanupStart, api::CleanupJobResponse> {
    crate::middleware::idempotency::Creation {
        kind: "cleanup_job",
        replay,
        identity: |value| {
            Ok(orch_db::idempotency::creation::Created {
                kind: "cleanup_job".into(),
                id: match value {
                    api::CleanupJobResponse::Full(value) => value.id,
                    api::CleanupJobResponse::Receipt(value) => value.id,
                },
                data: serde_json::Value::Null,
            })
        },
    }
}
fn replay(
    ctx: Ctx,
    (): (),
    _: api::CleanupStart,
    created: orch_db::idempotency::creation::Created,
) -> BoxFuture<'static, ApiResult<api::CleanupJobResponse>> {
    Box::pin(async move {
        let receipt = api::CleanupJobResponse::Receipt(api::CleanupJobReceipt {
            projection: orch_api_contract::common::ReceiptProjection::Receipt,
            id: created.id,
        });
        publication::receipt(&ctx, &receipt)?;
        let mut conn = ctx.app.db.acquire().await?;
        let sources: Option<serde_json::Value> = sqlx::query_scalar(
            "SELECT plan->'sources' FROM jobs WHERE id=$1 AND created_by=$2 AND kind=$3",
        )
        .bind(created.id)
        .bind(ctx.user()?.id())
        .bind(cleanup::KIND)
        .fetch_optional(&mut *conn)
        .await
        .map_err(DbError::from)?;
        let Some(sources) = sources else {
            return Ok(receipt);
        };
        let sources = serde_json::from_value(sources).map_err(ApiError::internal)?;
        drop(conn);
        match reply::render_completed_replay::<Shown>(
            &ctx,
            ctx.user()?.id(),
            Accepted {
                id: created.id,
                sources,
                recipe: None,
            },
        )
        .await
        {
            Ok(Some(value)) => Ok(api::CleanupJobResponse::full(value)),
            Ok(None) => Ok(receipt),
            Err(error) if matches!(error.code(), ErrorCode::NotFound | ErrorCode::AuthzDenied) => {
                Ok(receipt)
            }
            Err(error) => Err(error),
        }
    })
}
async fn locate(
    conn: &mut Conn,
    user: Id,
    id: Id,
    now: chrono::DateTime<chrono::Utc>,
) -> ApiResult<(
    Option<PreflightRow>,
    Option<jobs::JobRow>,
    api::CleanupRequest,
)> {
    if let Some(id)=sqlx::query_scalar::<_,Id>("SELECT id FROM jobs WHERE kind='storage_cleanup' AND plan->'recipe'->'plan'->>'id'=$1 AND created_by=$2").bind(id.to_string()).bind(user).fetch_optional(&mut *conn).await.map_err(DbError::from)? {
        let row=jobs::job(conn,id).await?.ok_or_else(ApiError::not_found)?;
        let recipe:Recipe=serde_json::from_value(row.plan["recipe"].clone()).map_err(ApiError::internal)?;
        let request=recipe.request.ok_or_else(||ApiError::internal("the native cleanup request is missing"))?;
        return Ok((None,Some(row),request));
    }
    let row = jobs::forks::preflight(conn, id)
        .await?
        .filter(|row| row.kind == cleanup::KIND && row.created_by == user && row.expires_at > now)
        .ok_or_else(|| ApiError::new(ErrorCode::PreflightExpired, "read the cleanup plan again"))?;
    let request =
        serde_json::from_value(row.plan["request"].clone()).map_err(ApiError::internal)?;
    Ok((Some(row), None, request))
}
fn session(ctx: &Ctx) -> ApiResult<Option<Id>> {
    Ok(match &ctx.user()?.via {
        Via::Session { session_id } => Some(*session_id),
        Via::Token { .. } | Via::OwnerRead { .. } => None,
    })
}
fn preview_origin(row: &PreflightRow) -> ApiResult<Origin> {
    let operation = row
        .plan
        .get("operation")
        .ok_or_else(|| ApiError::internal("the saved cleanup preview has no original operation"))?;
    let checkpoint = row.plan.get("checkpoint").ok_or_else(|| {
        ApiError::internal("the saved cleanup preview has no original HTTP checkpoint")
    })?;
    Ok(Origin {
        operation: serde_json::from_value(operation.clone()).map_err(ApiError::internal)?,
        session: row
            .plan
            .get("session_id")
            .filter(|value| !value.is_null())
            .map(|value| serde_json::from_value(value.clone()).map_err(ApiError::internal))
            .transpose()?,
        checkpoint: Some(serde_json::from_value(checkpoint.clone()).map_err(ApiError::internal)?),
    })
}
impl Effect for Create {
    type Source = Source;
    type Query = ();
    type Body = api::CleanupStart;
    type Value = Prepared;
    type Output = Saved;
    fn declaration(_: &Prepared) -> &'static Declaration<Source> {
        &sources::CREATE
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: api::CleanupStart,
    ) -> BoxFuture<'a, ApiResult<(Prepared, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
                .bind(format!("storage.cleanup:{}", body.plan_id))
                .execute(&mut *conn)
                .await
                .map_err(DbError::from)?;
            let (stored, replay, request) =
                locate(conn, caller.user(), body.plan_id, ctx.now).await?;
            let scope = Scope::parse(&query(&request))?;
            let initial = effects::initial_value::<Create>(ctx)?;
            let (plan, targets, saved_sources) = if let Some(row) = &replay {
                let recipe: Recipe = serde_json::from_value(row.plan["recipe"].clone())
                    .map_err(ApiError::internal)?;
                (
                    recipe.plan,
                    recipe
                        .work
                        .into_iter()
                        .filter_map(|work| work.target.map(|target| (work.id, target)))
                        .collect(),
                    row.plan.get("sources").cloned(),
                )
            } else {
                let row = stored.as_ref().ok_or_else(ApiError::not_found)?;
                (
                    serde_json::from_value(row.plan["preview"].clone())
                        .map_err(ApiError::internal)?,
                    serde_json::from_value(row.plan["targets"].clone())
                        .map_err(ApiError::internal)?,
                    row.plan.get("sources").cloned(),
                )
            };
            let (mut sources, principal, records) = if let Some(saved) = saved_sources {
                let sources: Sources = serde_json::from_value(saved).map_err(ApiError::internal)?;
                let facts = sources
                    .required(conn)
                    .await
                    .map_err(ApiError::engine)?
                    .ok_or_else(ApiError::not_found)?;
                let plan = native::bind_required(ctx, &sources::CREATE, &facts)?;
                let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
                if replay.is_none() {
                    report_sources::require_namespace(conn, &principal, scope).await?;
                }
                let records = if replay.is_none() {
                    Some(
                        super::super::prepare_records(
                            ctx,
                            conn,
                            &principal,
                            &query(&request),
                            Some(initial.as_ref()),
                            None,
                        )
                        .await?,
                    )
                } else {
                    None
                };
                (sources, principal, records)
            } else {
                let selected = Some(
                    plan.items
                        .iter()
                        .map(|item| item.record.id.clone())
                        .collect(),
                );
                let mut book =
                    Book::for_operation(ctx, conn, caller, scope, selected, &sources::CREATE, true)
                        .await?;
                let principal =
                    native::preparation_principal(ctx, conn, caller, &book.plan).await?;
                if replay.is_none() {
                    report_sources::require_namespace(conn, &principal, scope).await?;
                }
                let records = super::super::prepare_records(
                    ctx,
                    conn,
                    &principal,
                    &query(&request),
                    Some(initial.as_ref()),
                    Some(&mut book),
                )
                .await?;
                (book.snapshot()?, principal, Some(records))
            };
            let account = targets::account(conn, caller.user()).await?;
            sources.names = vec![Reference {
                family: KeyTargetKind::Identity(KeyIdentityKind::Account),
                object: account.object,
            }];
            let preview = stored.as_ref().map(preview_origin).transpose()?;
            let preview_deadline = if let Some(origin) = &preview {
                Some(
                    operation::check_preview(conn, ctx.app.clock.as_ref(), origin, &sources)
                        .await
                        .map_err(ApiError::engine)?,
                )
            } else {
                None
            };
            if principal.user() != caller.user() {
                return Err(ApiError::not_found());
            }
            let facts = sources
                .required(conn)
                .await
                .map_err(ApiError::engine)?
                .ok_or_else(ApiError::not_found)?;
            Ok((
                Prepared {
                    stored,
                    replay,
                    request,
                    plan,
                    targets,
                    records,
                    body,
                    sources,
                    preview,
                    preview_deadline,
                },
                facts,
            ))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        _: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Prepared,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if principal.user() != ctx.user()?.id()
                || value
                    .stored
                    .as_ref()
                    .is_some_and(|row| row.created_by != principal.user())
                || value
                    .replay
                    .as_ref()
                    .is_some_and(|row| row.created_by != Some(principal.user()))
            {
                return Err(ApiError::not_found());
            }
            Ok(())
        })
    }
}
impl Initial for Create {
    type Input = Intent;
    type Fetched = OperatorSnapshot;
    fn declaration(_: &Intent) -> &'static Declaration<Source> {
        &sources::CREATE
    }
    fn authorize<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: api::CleanupStart,
    ) -> BoxFuture<'a, ApiResult<(Intent, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let (_, replay, request) = locate(conn, caller.user(), body.plan_id, ctx.now).await?;
            let scope = Scope::parse(&query(&request))?;
            let target = report_sources::namespace(conn, caller, scope).await?;
            Ok((
                Intent {
                    scope,
                    replay: replay.is_some(),
                },
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
        value: &'a Intent,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if value.replay {
                Ok(())
            } else {
                report_sources::require_namespace(conn, principal, value.scope).await
            }
        })
    }
    fn fetch<'a>(
        ctx: &'a Ctx,
        caller: &'a Caller,
        value: Intent,
    ) -> BoxFuture<'a, ApiResult<OperatorSnapshot>> {
        Box::pin(report_sources::fetch_operator(ctx, caller, value.scope))
    }
}
impl ApplyOwned for Create {
    fn apply_owned<'a>(
        ctx: &'a Ctx,
        mut tx: Tx,
        principal: &'a Principal,
        value: &'a Prepared,
        before: FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<Outcome<Saved>>> {
        Box::pin(async move {
            if let Some(row) = &value.replay {
                let current = jobs::job(&mut tx, row.id)
                    .await?
                    .filter(|current| {
                        current.created_by == Some(principal.user())
                            && current.kind == cleanup::KIND
                            && current.plan["recipe"] == row.plan["recipe"]
                    })
                    .ok_or_else(ApiError::not_found)?;
                orch_db::idempotency::creation::observe(
                    &mut tx,
                    Some(principal.user()),
                    "cleanup_job",
                    current.id,
                )
                .await?;
                tx.commit().await.map_err(DbError::from)?;
                return Ok(Outcome::Unchanged(Saved {
                    row: current,
                    sources: value.sources.clone(),
                }));
            }
            let deadline = before.deadline();
            let (_, rows, _) = value
                .records
                .as_ref()
                .ok_or_else(|| ApiError::internal("the native cleanup operands are missing"))?
                .render()?;
            let current = rows
                .into_iter()
                .map(|mut row| {
                    deliberate_stop(&mut row);
                    (row.id.clone(), row)
                })
                .collect();
            let (plan, work) =
                select_work(value.plan.clone(), current, &value.targets, &value.body)?;
            let recipe = Recipe {
                request: Some(value.request.clone()),
                plan,
                work,
            };
            let id = new_id();
            let retained = operation::Request::capture(
                id,
                principal.caller(),
                session(ctx)?,
                value.preview.clone(),
                value.sources.clone(),
                &recipe,
            )
            .map_err(ApiError::engine)?;
            let identity = match &ctx.user()?.via {
                Via::OwnerRead { source } => json!({"owner_read":source.source()}),
                Via::Session { session_id } => json!({"session_id":session_id}),
                Via::Token { token_id, .. } => json!({"token_id":token_id}),
            };
            if let Some(preview) = &value.preview_deadline {
                preview
                    .check(ctx.app.clock.as_ref())
                    .map_err(ApiError::engine)?;
            }
            before.check()?;
            let row=jobs::insert(&mut tx,&jobs::NewJob{id,kind:cleanup::KIND,node_id:None,environment_id:None,provider_id:None,created_by:Some(principal.user()),plan:&json!({"recipe":recipe,"identity":identity,"origin":orch_db::audit::current_origin(),"cleanup_operation":retained,"sources":value.sources,"cleanup_results":[]}),progress:&serde_json::to_value(initial_progress(&cleanup::PHASES)).map_err(ApiError::internal)?},ctx.now).await?;
            sqlx::query("DELETE FROM preflights WHERE id=$1")
                .bind(value.stored.as_ref().ok_or_else(ApiError::not_found)?.id)
                .execute(&mut *tx)
                .await
                .map_err(DbError::from)?;
            orch_db::idempotency::creation::observe(
                &mut tx,
                Some(principal.user()),
                "cleanup_job",
                row.id,
            )
            .await?;
            deadline.check()?;
            if let Some(preview) = &value.preview_deadline {
                preview
                    .check(ctx.app.clock.as_ref())
                    .map_err(ApiError::engine)?;
            }
            tx.commit().await.map_err(DbError::from)?;
            Ok(Outcome::Applied(Saved {
                row,
                sources: value.sources.clone(),
            }))
        })
    }
}
pub(super) async fn run(
    ctx: Ctx,
    (): (),
    _: api::CleanupStart,
) -> ApiResult<api::CleanupJobResponse> {
    let (tx, _) = transaction_guard::begin(&ctx).await?;
    let done = effects::apply_owned::<Create>(&ctx, tx).await?;
    let receipt = api::CleanupJobReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        id: done.output.row.id,
    };
    publication::receipt(&done.ctx, &receipt)?;
    let full = match reply::render::<Shown>(&done.ctx, &done.accepted, done.output.into()).await {
        Ok(full) => full,
        Err(error) if matches!(error.code(), ErrorCode::AuthzDenied | ErrorCode::NotFound) => None,
        Err(error) => return Err(error),
    };
    Ok(full.map_or_else(
        || api::CleanupJobResponse::Receipt(receipt),
        api::CleanupJobResponse::full,
    ))
}
struct Shown;
impl reply::Projection for Shown {
    type Source = Source;
    type Input = Accepted;
    type Subject = jobs::JobRow;
    type Value = ();
    type Reply = Job;
    const READ_SOURCE: Source = Source::Content;
    fn declaration(_: &Accepted) -> &'static Declaration<Source> {
        &sources::CREATE
    }
    fn subject<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Accepted,
    ) -> BoxFuture<'a, ApiResult<(jobs::JobRow, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = jobs::job(conn, input.id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            if row.plan["sources"]
                != serde_json::to_value(&input.sources).map_err(ApiError::internal)?
            {
                return Err(ApiError::not_found());
            }
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
        _: &'a Ctx,
        _conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Accepted,
        row: &'a jobs::JobRow,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if row.kind != cleanup::KIND
                || row.created_by != Some(principal.user())
                || input
                    .recipe
                    .as_ref()
                    .is_some_and(|recipe| row.plan.get("recipe") != Some(recipe))
            {
                return Err(ApiError::new(
                    ErrorCode::AuthzDenied,
                    "the cleanup job is no longer readable",
                ));
            }
            Ok(())
        })
    }
    fn prepare<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Accepted,
        _: &'a jobs::JobRow,
    ) -> BoxFuture<'a, ApiResult<((), Vec<native::Facts<Source>>)>> {
        Box::pin(async { Ok(((), Vec::new())) })
    }
    fn render<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        _: &'a Principal,
        _: &'a Accepted,
        value: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<Job>> {
        Box::pin(crate::routes::environments::jobs::view(
            conn,
            &value.subject,
        ))
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Accepted,
        old: &'a reply::Prepared<Self>,
        now: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(old.subject.created_by == now.subject.created_by
                && old.subject.plan["recipe"] == now.subject.plan["recipe"])
        })
    }
}
