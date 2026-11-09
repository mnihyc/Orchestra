//! The native archive is opened once before its actual resolved import intent enters the queue.

use super::*;

use crate::key_operations::blob_sources;

use crate::key_operations::native;

use crate::key_operations::native::effects;

use crate::key_operations::native::http_origin;

use crate::key_operations::native::reply;

use crate::key_operations::publication;

use crate::key_operations::targets;

use futures::future::BoxFuture;

use orch_api_contract::import_creation_sources as sources;

use orch_api_contract::import_creation_sources::Source;

use orch_api_contract::imports::ImportCreationReceipt;

use orch_api_contract::imports::ImportCreationResponse;

use orch_api_contract::imports::ImportLimits;

use orch_api_contract::operation_sources::Declaration;

use orch_authz::Caller;

use orch_authz::CapturedOperation;

use orch_authz::Principal;

use orch_chat::following::BlobOrigin;

use orch_db::queries::identity;

use orch_db::queries::jobs;

use orch_engine::imports::Made;

use orch_engine::imports::Opened;

use orch_engine::imports::Plan;

use orch_engine::imports::PreparedImportQueue;

use orch_types::Hash;

use orch_types::Id;

use std::sync::Arc;
pub(super) struct Create;

pub(super) struct Known {
    node: Id,
    request: ImportRequest,
    selected: Vec<(Hash, BlobOrigin)>,
    limits: ImportLimits,
}

pub(super) struct Fetched {
    known: Known,
    opened: Arc<Opened>,
}

pub(super) struct Ready {
    fetched: Arc<Fetched>,
    plan: Arc<Plan>,
    queue: PreparedImportQueue,
    facts: Vec<native::Facts<Source>>,
}

pub(super) struct Queued {
    made: Made,
    required: Vec<native::Facts<Source>>,
    plan: Arc<Plan>,
    selected: Vec<(Hash, BlobOrigin)>,
    fresh: bool,
}

struct Accepted {
    id: Id,
    import: Id,
    node: Option<Id>,
    user: Option<Id>,
    variant: usize,
    required: Vec<native::Facts<Source>>,
    initial: Option<jobs::JobRow>,
}

impl From<Queued> for Accepted {
    fn from(value: Queued) -> Self {
        Self {
            id: value.made.job.id,
            import: value.made.import.id,
            node: value.made.import.node_id,
            user: value.made.import.user_id,
            variant: usize::from(
                value.plan.access.restored
                    || value.plan.authors == orch_api_contract::imports::ImportAuthors::Accounts,
            ),
            required: value.required,
            initial: Some(value.made.job),
        }
    }
}

pub(super) fn replay_binding()
-> crate::middleware::idempotency::Creation<(), ImportRequest, ImportCreationResponse> {
    crate::middleware::idempotency::Creation {
        kind: "import_job",
        replay,
        identity: |value| {
            let id = match value {
                ImportCreationResponse::Full(value) => value.id,
                ImportCreationResponse::Receipt(value) => value.id,
            };
            Ok(orch_db::idempotency::creation::Created {
                kind: "import_job".into(),
                id,
                data: serde_json::Value::Null,
            })
        },
    }
}

fn replay(
    ctx: Ctx,
    (): (),
    _: ImportRequest,
    created: orch_db::idempotency::creation::Created,
) -> BoxFuture<'static, ApiResult<ImportCreationResponse>> {
    Box::pin(async move {
        let import: Id =
            serde_json::from_value(created.data["import"].clone()).map_err(ApiError::internal)?;
        let node: Option<Id> =
            serde_json::from_value(created.data["node"].clone()).map_err(ApiError::internal)?;
        let variant = created.data["variant"]
            .as_u64()
            .and_then(|index| usize::try_from(index).ok())
            .filter(|index| *index < sources::CANDIDATES.len())
            .ok_or_else(|| ApiError::internal("the import creation has no native variant"))?;
        let receipt = ImportCreationResponse::Receipt(ImportCreationReceipt {
            projection: orch_api_contract::common::ReceiptProjection::Receipt,
            id: created.id,
            import_id: import,
        });
        publication::receipt(&ctx, &receipt)?;
        match reply::render_completed_replay::<JobMetadata>(
            &ctx,
            ctx.user()?.id(),
            Accepted {
                id: created.id,
                import,
                node,
                user: Some(ctx.user()?.id()),
                variant,
                required: Vec::new(),
                initial: None,
            },
        )
        .await
        {
            Ok(Some(value)) => Ok(ImportCreationResponse::full(value)),
            Ok(None) => Ok(receipt),
            Err(error) if matches!(error.code(), ErrorCode::NotFound | ErrorCode::AuthzDenied) => {
                Ok(receipt)
            }
            Err(error) => Err(error),
        }
    })
}

fn declaration(plan: &Plan) -> &'static Declaration<Source> {
    &sources::CANDIDATES[usize::from(
        plan.access.restored || plan.authors == orch_api_contract::imports::ImportAuthors::Accounts,
    )]
}

fn validate(body: &ImportRequest) -> ApiResult<()> {
    if body.client_id.trim().is_empty()
        || body.client_id.trim().chars().count() > CLIENT_ID_MAX_CHARS
    {
        return Err(ApiError::invalid(
            "client_id",
            "must be an identity of one to two hundred characters",
        ));
    }
    if body.source.files.is_empty() {
        return Err(ApiError::invalid(
            "source.files",
            "name the uploads to import",
        ));
    }
    Ok(())
}

async fn creator(conn: &mut Conn, user: Id) -> ApiResult<native::Facts<Source>> {
    let row = identity::user_by_id(conn, user)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let target = targets::from_account(conn, &row).await?;
    Ok(native::Facts::one(
        Source::Creator,
        target.object.kind,
        target,
    ))
}

async fn require(ctx: &Ctx, conn: &mut Conn, principal: &Principal, node: Id) -> ApiResult<()> {
    match ctx
        .app
        .engine
        .import_place_seen(conn, principal, node)
        .await
    {
        Ok(()) => Ok(()),
        Err(error) => Err(failed(conn, error).await),
    }
}

async fn input_sources(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    body: &ImportRequest,
    row: &Declaration<Source>,
    selected: Option<&[(Hash, BlobOrigin)]>,
) -> ApiResult<blob_sources::Prepared> {
    let base = crate::registry::OperationPlan::new(ctx.route, row.operation)?;
    let hashes = body
        .source
        .files
        .iter()
        .map(|file| file.blob)
        .collect::<Vec<_>>();
    match selected {
        Some(selected) => {
            blob_sources::prepare_selected(ctx, conn, caller, &base, &hashes, &[], selected).await
        }
        None => blob_sources::prepare(ctx, conn, caller, &base, &hashes, &[]).await,
    }
}

impl effects::CommonInitial for Create {
    type Input = Known;
    type Fetched = Fetched;
    const CANDIDATES: &'static [Declaration<Source>] = &sources::CANDIDATES;
    fn authorize<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: ImportRequest,
    ) -> BoxFuture<'a, ApiResult<(Known, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            validate(&body)?;
            let node = ctx.id("id")?;
            let mut last = None;
            let mut selected = None;
            // Scope-only candidate source selection is not a resolved variant or reusable admission.
            for row in &sources::CANDIDATES {
                match input_sources(ctx, conn, caller, &body, row, None).await {
                    Ok(value) => {
                        selected = Some(value);
                        break;
                    }
                    Err(error)
                        if matches!(error.code(), ErrorCode::NotFound | ErrorCode::AuthzDenied) =>
                    {
                        last = Some(error)
                    }
                    Err(error) => return Err(error),
                }
            }
            let selected = selected.ok_or_else(|| last.unwrap_or_else(ApiError::not_found))?;
            let limits = match ctx.app.engine.import_limits(conn, node).await {
                Ok(value) => value,
                Err(error) => return Err(failed(conn, error).await),
            };
            let facts = orch_engine::imports::scope::byte_facts(&selected.bindings);
            Ok((
                Known {
                    node,
                    request: body,
                    selected: selected.selected,
                    limits,
                },
                facts,
            ))
        })
    }
    fn require_initial<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Known,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            require(ctx, conn, principal, input.node).await?;
            for file in &input.request.source.files {
                crate::attachments::require(conn, principal, &file.blob).await?;
            }
            Ok(())
        })
    }
    fn fetch<'a>(
        ctx: &'a Ctx,
        caller: &'a Caller,
        known: Known,
    ) -> BoxFuture<'a, ApiResult<Fetched>> {
        Box::pin(async move {
            match orch_engine::imports::open(&ctx.app.engine, &known.request.source, &known.limits)
                .await
            {
                Ok(opened) => Ok(Fetched {
                    known,
                    opened: Arc::new(opened),
                }),
                Err(error) => {
                    let mut conn = ctx.app.db.acquire().await?;
                    let error = failed(&mut conn, error.into()).await;
                    Err(if native::modern(caller) {
                        authorization::problem_without_content(error)
                    } else {
                        error
                    })
                }
            }
        })
    }
}

impl effects::Effect for Create {
    type Source = Source;
    type Query = ();
    type Body = ImportRequest;
    type Value = Ready;
    type Output = Queued;
    fn declaration(value: &Ready) -> &'static Declaration<Source> {
        declaration(&value.plan)
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: ImportRequest,
    ) -> BoxFuture<'a, ApiResult<(Ready, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let fetched = effects::common_initial_value::<Self>(ctx)?;
            if fetched.known.node != ctx.id("id")? || fetched.known.request != body {
                return Err(ApiError::internal("the native import input changed"));
            }
            let read = crate::middleware::authorization::load_principal(
                conn,
                caller.clone(),
                ctx.app.clock.time(),
            )
            .await?;
            require(ctx, conn, &read, fetched.known.node).await?;
            let plan = match orch_engine::imports::build(
                &ctx.app.engine,
                conn,
                &read,
                fetched.known.node,
                &body,
                Arc::clone(&fetched.opened),
            )
            .await
            {
                Ok(plan) => Arc::new(plan),
                Err(error) => return Err(failed(conn, error).await),
            };
            let row = declaration(&plan);
            let bytes =
                input_sources(ctx, conn, caller, &body, row, Some(&fetched.known.selected)).await?;
            if bytes.selected != fetched.known.selected {
                return Err(ApiError::not_found());
            }
            let mut facts = orch_engine::imports::scope::byte_facts(&bytes.bindings);
            facts.extend(
                match orch_engine::imports::scope::facts(conn, &plan, None).await {
                    Ok(value) => value,
                    Err(error) => return Err(failed(conn, error).await),
                },
            );
            facts.push(creator(conn, caller.user()).await?);
            let native = match ctx.app.engine.prepare_import_from_plan(
                caller.user(),
                fetched.known.node,
                body,
                &plan,
            ) {
                Ok(value) => value,
                Err(error) => return Err(failed(conn, error).await),
            };
            let queue = match ctx
                .app
                .engine
                .prepare_import_queue(conn, caller, native)
                .await
            {
                Ok(value) => value,
                Err(error) => return Err(failed(conn, error).await),
            };
            Ok((
                Ready {
                    fetched,
                    plan,
                    queue,
                    facts: facts.clone(),
                },
                facts,
            ))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Ready,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require(ctx, conn, principal, value.fetched.known.node))
    }
}

impl effects::Apply for Create {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Ready,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Queued>>> {
        Box::pin(async move {
            let made = match ctx
                .app
                .engine
                .apply_import_queue_before(
                    conn,
                    principal.caller(),
                    value.queue.clone(),
                    move || {
                        before.check().map_err(|error| {
                            orch_engine::EngineError::BeforeEffect(Box::new(error))
                        })
                    },
                )
                .await
            {
                Ok(value) => value,
                Err(error) => return Err(failed(conn, error).await),
            };
            let output = Queued {
                made,
                plan: Arc::clone(&value.plan),
                selected: value.fetched.known.selected.clone(),
                fresh: value.queue.replay().is_none(),
                required: value
                    .facts
                    .iter()
                    .filter(|fact| fact.source != Source::Creator)
                    .cloned()
                    .collect(),
            };
            Ok(if value.queue.replay().is_some() {
                effects::Outcome::Unchanged(output)
            } else {
                effects::Outcome::Applied(output)
            })
        })
    }
}

pub(super) async fn run(ctx: Ctx, (): (), _: ImportRequest) -> ApiResult<ImportCreationResponse> {
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    let done = effects::apply::<Create>(&ctx, &mut tx).await?;
    if done.output.fresh {
        let selected = CapturedOperation::capture(&done.accepted).map_err(ApiError::internal)?;
        let http = http_origin::capture_operation(&done.ctx, &mut tx, &selected).await?;
        match orch_engine::imports::operation::Request::retain(
            &mut tx,
            &done.output.made,
            http,
            done.output.selected.clone(),
            &done.output.plan,
            ctx.app.clock.time(),
        )
        .await
        {
            Ok(_) => {}
            Err(error) => return Err(failed(&mut tx, error).await),
        }
    }
    orch_db::idempotency::creation::observe_with(&mut tx,Some(ctx.user()?.id()),"import_job",done.output.made.job.id,serde_json::json!({"import":done.output.made.import.id,"node":done.output.made.import.node_id,"variant":usize::from(done.output.plan.access.restored||done.output.plan.authors==orch_api_contract::imports::ImportAuthors::Accounts)})).await?;
    guard.check_deadline(&ctx)?;
    if !done.accepted.key_deadline_allows(ctx.app.clock.as_ref()) {
        return Err(ApiError::new(
            ErrorCode::AuthInvalidToken,
            "the original import key has ended",
        ));
    }
    tx.commit().await.map_err(DbError::from)?;
    let receipt = ImportCreationReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        id: done.output.made.job.id,
        import_id: done.output.made.import.id,
    };
    publication::receipt(&done.ctx, &receipt)?;
    let projected =
        reply::render::<JobMetadata>(&done.ctx, &done.accepted, done.output.into()).await;
    ctx.app.engine.kick();
    match projected {
        Ok(Some(job)) => Ok(ImportCreationResponse::full(job)),
        Ok(None) => Ok(ImportCreationResponse::Receipt(receipt)),
        Err(error) if matches!(error.code(), ErrorCode::AuthzDenied | ErrorCode::NotFound) => {
            Ok(ImportCreationResponse::Receipt(receipt))
        }
        Err(error) => Err(error),
    }
}

struct JobMetadata;

impl reply::Projection for JobMetadata {
    type Source = Source;
    type Input = Accepted;
    type Subject = jobs::JobRow;
    type Value = ();
    type Reply = Job;
    const READ_SOURCE: Source = Source::Creator;
    fn declaration(value: &Accepted) -> &'static Declaration<Source> {
        &sources::CANDIDATES[value.variant]
    }
    fn subject<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Accepted,
    ) -> BoxFuture<'a, ApiResult<(jobs::JobRow, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let job = jobs::job(conn, input.id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            if job.kind != jobs::kind::IMPORT
                || job.created_by != input.user
                || job.plan["import"] != serde_json::json!(input.import)
                || job.plan["node"] != serde_json::json!(input.node)
                || job.plan["user"] != serde_json::json!(input.user)
            {
                return Err(ApiError::not_found());
            }
            let mut facts = input.required.clone();
            facts.push(creator(conn, job.created_by.ok_or_else(ApiError::not_found)?).await?);
            Ok((job, facts))
        })
    }
    fn require<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        principal: &'a Principal,
        _: &'a Accepted,
        job: &'a jobs::JobRow,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if job.created_by == Some(principal.user()) {
                Ok(())
            } else {
                Err(ApiError::not_found())
            }
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
        input: &'a Accepted,
        value: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<Job>> {
        Box::pin(async move {
            job_view::view(conn, input.initial.as_ref().unwrap_or(&value.subject)).await
        })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Accepted,
        old: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(old.subject.id == fresh.subject.id
                && old.subject.kind == fresh.subject.kind
                && old.subject.created_by == fresh.subject.created_by
                && old.subject.created_at == fresh.subject.created_at
                && [
                    "import",
                    "node",
                    "user",
                    "import_operation",
                    "import_operation_required",
                ]
                .iter()
                .all(|key| old.subject.plan.get(*key) == fresh.subject.plan.get(*key)))
        })
    }
}
