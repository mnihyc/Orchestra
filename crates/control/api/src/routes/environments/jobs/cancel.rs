//! Native cancellation: fresh operands after the Job/command wait, then optional disclosure.

use super::*;
use crate::key_operations::native::{self, effects, reply};
use crate::middleware::transaction_guard::{self, HttpIdentityMode};
use crate::registry::OperationPlan;
use crate::routes::nodes::projection::groups;
use futures::future::BoxFuture;
use orch_access_contract::key_scope::{KeyIdentityKind, KeyTargetKind};
use orch_api_contract::common::ReceiptProjection;
use orch_api_contract::operation_sources::Declaration;
use orch_api_contract::workspace::{JobCancelReceipt, JobCancelResponse};
use orch_api_contract::workspace_sources::{self as sources, Source};
use orch_authz::Caller;
use orch_db::DbError;

const JOB: KeyTargetKind = KeyTargetKind::Identity(KeyIdentityKind::Job);

pub(super) struct Cancel;
pub(super) struct Value {
    row: JobRow,
    choice: subject::Choice,
    facts: Vec<native::Facts<Source>>,
}

pub(super) struct Output {
    row: JobRow,
    requested: bool,
    choice: subject::Choice,
    // Completed source provenance, never reused as current Write/Manage admission.
    facts: Vec<native::Facts<Source>>,
}

fn declaration(row: &JobRow) -> &'static Declaration<Source> {
    if row.kind == orch_db::queries::thread_continuations::JOB_KIND {
        &sources::JOB_CANCEL_CONTINUATION
    } else {
        &sources::JOB_CANCEL_LIFECYCLE
    }
}

async fn facts(
    conn: &mut Conn,
    row: &JobRow,
    choice: subject::Choice,
) -> ApiResult<Vec<native::Facts<Source>>> {
    let (namespace, target) = read::resolved(conn, choice).await?;
    Ok(vec![
        native::Facts::one(Source::Job, JOB, subject::identity(row, namespace.as_ref())),
        native::Facts::collection(
            Source::JobSubjects,
            groups(
                choice
                    .subject
                    .zip(target)
                    .map(|(object, target)| (KeyTargetKind::Object(object.kind), target)),
            ),
        ),
    ])
}

struct Bounds<'a> {
    ctx: &'a Ctx,
    caller: &'a Caller,
}

struct Exact(subject::Choice);
impl subject::Selection for Exact {
    fn accepts<'a>(
        &'a self,
        _: &'a mut Conn,
        _: &'a JobRow,
        choice: subject::Choice,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move { Ok(choice == self.0) })
    }
}
impl subject::Selection for Bounds<'_> {
    fn accepts<'a>(
        &'a self,
        conn: &'a mut Conn,
        row: &'a JobRow,
        choice: subject::Choice,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            let declaration = declaration(row);
            let plan = OperationPlan::new(self.ctx.route, declaration.operation)?;
            let facts = facts(conn, row, choice).await?;
            for fact in facts {
                for (family, targets) in fact.groups {
                    for target in targets {
                        if !native::permits(
                            &plan,
                            declaration,
                            self.caller,
                            &fact.source,
                            family,
                            &target,
                        )? {
                            return Ok(false);
                        }
                    }
                }
            }
            Ok(true)
        })
    }
}

async fn principal(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    row: &JobRow,
) -> ApiResult<Principal> {
    let declaration = declaration(row);
    if !native::modern(caller) {
        let known = native::bind_known(ctx, declaration, &[])?;
        return native::preparation_principal(ctx, conn, caller, &known).await;
    }
    let plan = OperationPlan::new(ctx.route, declaration.operation)?;
    for candidate in subject::cancellation_candidates(conn, row).await? {
        let (namespace, _) = read::resolved(conn, candidate.choice).await?;
        let target = subject::identity(row, namespace.as_ref());
        if native::permits(&plan, declaration, caller, &Source::Job, JOB, &target)? {
            let root = native::Facts::one(Source::Job, JOB, target);
            let known = native::bind_known(ctx, declaration, &[root])?;
            return native::preparation_principal(ctx, conn, caller, &known).await;
        }
    }
    Err(subject::scope_refused())
}

impl effects::Effect for Cancel {
    type Source = Source;
    type Query = ();
    type Body = ();
    type Value = Value;
    type Output = Output;

    fn declaration(value: &Value) -> &'static Declaration<Source> {
        declaration(&value.row)
    }

    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        _: (),
        _: (),
    ) -> BoxFuture<'a, ApiResult<(Value, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let original = row_of(conn, ctx.id("id")?).await?;
            let actor = principal(ctx, conn, caller, &original).await?;
            // Preserve ordinary denial before any native serialization wait.
            could_have_started(conn, &actor, &original).await?;
            let thread = subject::cancellation_thread(&original);
            if let Some(thread) = thread {
                orch_db::queries::threads::lock(conn, thread)
                    .await?
                    .ok_or_else(ApiError::not_found)?;
            }
            let row = rows::lock_cancellation(conn, original.id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            if thread != subject::cancellation_thread(&row) || original.kind != row.kind {
                return Err(ApiError::new(
                    ErrorCode::ConflictStaleRevision,
                    "the job's cancellation source changed while waiting",
                ));
            }
            let identity =
                transaction_guard::current_http_identity(ctx, conn, HttpIdentityMode::Read).await?;
            transaction_guard::guard_user(&identity.user)?;
            let caller = identity
                .caller
                .intersect_restrictions(caller)
                .map_err(ApiError::internal)?;
            let actor = principal(ctx, conn, &caller, &row).await?;
            let bounds = Bounds {
                ctx,
                caller: &caller,
            };
            let choice = subject::cancel(
                conn,
                &actor,
                &row,
                native::modern(&caller).then_some(&bounds as &dyn subject::Selection),
            )
            .await?;
            let choice = subject::actual_choice(conn, choice).await?;
            let facts = facts(conn, &row, choice).await?;
            Ok((
                Value {
                    row,
                    choice,
                    facts: facts.clone(),
                },
                facts,
            ))
        })
    }

    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Value,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            subject::cancel(conn, principal, &value.row, Some(&Exact(value.choice)))
                .await
                .map(|_| ())
        })
    }
}

impl effects::Apply for Cancel {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Value,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Output>>> {
        Box::pin(async move {
            let requested = value.row.is_open();
            if requested {
                if value.row.kind == kind::ENVIRONMENT_REMOVE
                    || value.row.kind == rows::retained::PURGE
                {
                    return Err(ApiError::new(
                        ErrorCode::PlanBlocked,
                        "a removal is not stopped once it is under way; keep the environment before it begins",
                    ));
                }
                if !rows::request_cancel_by_before(
                    conn,
                    value.row.id,
                    Some(principal.user()),
                    ctx.now,
                    move || before.check(),
                )
                .await?
                {
                    return Err(ApiError::new(
                        ErrorCode::PlanBlocked,
                        "the job is past the point after which nothing is undone",
                    ));
                }
            }
            let output = Output {
                row: row_of(conn, value.row.id).await?,
                requested,
                choice: value.choice,
                facts: value.facts.clone(),
            };
            Ok(if requested {
                effects::Outcome::Applied(output)
            } else {
                effects::Outcome::Unchanged(output)
            })
        })
    }
}

fn receipt(output: &Output) -> JobCancelResponse {
    JobCancelResponse::Receipt(JobCancelReceipt {
        projection: ReceiptProjection::Receipt,
        job_id: output.row.id,
        cancel_requested: output.requested,
    })
}

/// The ordinary response keeps its original cancellation gate. An independently
/// readable Job can still be shown after completed cancellation authority ends.
async fn response_cut(conn: &mut Conn, principal: &Principal, row: &JobRow) -> ApiResult<bool> {
    if row.kind != orch_db::queries::thread_continuations::JOB_KIND {
        match could_have_started(conn, principal, row).await {
            Ok(()) => return Ok(false),
            Err(error) if matches!(error.problem.status, 401 | 403 | 404 | 410) => {}
            Err(error) => return Err(error),
        }
    }
    may_see(conn, principal, row).await
}

struct Projection;
struct ReplyValue {
    prepared: PreparedRead,
    names: Option<HashMap<Id, String>>,
}

impl reply::Projection for Projection {
    type Source = Source;
    type Input = Output;
    type Subject = JobRow;
    type Value = ReplyValue;
    type Reply = Job;
    const READ_SOURCE: Source = Source::JobReply;

    fn declaration(input: &Output) -> &'static Declaration<Source> {
        declaration(&input.row)
    }

    fn subject<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Output,
    ) -> BoxFuture<'a, ApiResult<(JobRow, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = row_of(conn, input.row.id).await?;
            if !(PreparedRead {
                row: input.row.clone(),
                cut: false,
            })
            .preserves(&PreparedRead {
                row: row.clone(),
                cut: false,
            }) {
                return Err(ApiError::not_found());
            }
            // Reload the actual chosen binding without re-admitting its completed Manage.
            if !subject::cancellation_candidates(conn, &row)
                .await?
                .iter()
                .any(|candidate| candidate.choice == input.choice)
            {
                return Err(ApiError::not_found());
            }
            let (namespace, _) = read::resolved(conn, input.choice).await?;
            let mut facts = input.facts.clone();
            facts.push(native::Facts::one(
                Source::JobReply,
                JOB,
                subject::identity(&row, namespace.as_ref()),
            ));
            let _ = ctx;
            Ok((row, facts))
        })
    }

    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _: &'a Output,
        row: &'a JobRow,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move { response_cut(conn, principal, row).await.map(|_| ()) })
    }

    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Output,
        row: &'a JobRow,
    ) -> BoxFuture<'a, ApiResult<(ReplyValue, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let cut = response_cut(conn, principal, row).await?;
            let (names, facts) = read::named_machines(
                ctx,
                conn,
                principal.caller(),
                row,
                declaration(&input.row),
                cut,
            )
            .await?;
            Ok((
                ReplyValue {
                    prepared: PreparedRead {
                        row: row.clone(),
                        cut,
                    },
                    names,
                },
                vec![facts],
            ))
        })
    }

    fn render<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _: &'a Output,
        prepared: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<Job>> {
        Box::pin(async move {
            if !prepared.value.prepared.cut
                && response_cut(conn, principal, &prepared.subject).await?
            {
                return Err(ApiError::not_found());
            }
            prepared
                .value
                .prepared
                .render(conn, prepared.value.names.as_ref())
                .await
        })
    }

    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Output,
        old: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            let names = match (&old.value.names, &fresh.value.names) {
                (Some(old_names), Some(current)) => {
                    old_names.keys().all(|id| current.contains_key(id))
                        && (old_names.is_empty()
                            || old.value.prepared.machine_source()
                                == fresh.value.prepared.machine_source())
                }
                (None, _) => true,
                _ => false,
            };
            Ok(old.value.prepared.preserves(&fresh.value.prepared) && names)
        })
    }
}

pub(super) async fn run(ctx: Ctx, (): (), (): ()) -> ApiResult<JobCancelResponse> {
    let mut tx = ctx.app.db.begin().await?;
    let completed = effects::apply::<Cancel>(&ctx, &mut tx).await?;
    let identity =
        transaction_guard::current_http_identity(&completed.ctx, &mut tx, HttpIdentityMode::Read)
            .await?;
    transaction_guard::guard_user(&identity.user)?;
    identity.check_deadline(&ctx.app)?;
    if !completed
        .accepted
        .key_deadline_allows(ctx.app.clock.as_ref())
    {
        return Err(ApiError::new(
            ErrorCode::AuthInvalidToken,
            "the cancellation credential expired",
        ));
    }
    tx.commit().await.map_err(DbError::from)?;
    let fallback = receipt(&completed.output);
    crate::key_operations::publication::receipt(&completed.ctx, &fallback)?;
    if completed.output.requested {
        // Durable accepted cancellation is cleanup. The runner reconciles any failed
        // stop transport; later key expiry must not strand a recorded cancellation.
        if let Err(error) = ctx
            .app
            .engine
            .stop_job_command(completed.output.row.id)
            .await
        {
            tracing::warn!(job = %completed.output.row.id, %error, "recorded job cancellation needs runner reconciliation");
        }
        ctx.app.engine.kick();
    }
    Ok(
        match reply::render::<Projection>(&completed.ctx, &completed.accepted, completed.output)
            .await?
        {
            Some(job) => JobCancelResponse::Full(Box::new(job)),
            None => fallback,
        },
    )
}
