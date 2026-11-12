//! Update and recreate plans over HTTP, confirmed once and checked again before a runtime is held.

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::middleware::authorization;

use crate::middleware::transaction_guard;

use crate::registry::Ctx;

use crate::registry::Registry;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::environment as api;

use orch_api_contract::forks::EnvironmentUpdatePlan;

use orch_api_contract::forks::ForkCreateBody;

use orch_api_contract::forks::ProviderUpdatePlan;

use orch_api_contract::forks::Rollback;

use orch_api_contract::forks::UpdateReason;

use orch_api_contract::forks::UpdateThread;

use orch_api_contract::workspace::Job;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_db::queries::jobs;

use orch_db::queries::jobs::forks::PreflightRow;

use orch_db::Conn;

use orch_db::DbError;

use orch_engine::updates;

use orch_engine::updates::Draft;

use orch_engine::updates::Recipe;

use orch_engine::updates::Target;

use orch_engine::updates::UpdateError;

use orch_types::Id;

use orch_types::Time;

use orch_types::new_id;

use serde_json::json;
/// Binds the environment replacement requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on(api::EnvironmentUpdatePreflight, preflight)
        .on(api::EnvironmentUpdateCreate, create)
        .creation(
            api::EnvironmentUpdateCreate,
            super::jobs::creation::binding::<super::jobs::creation::EnvironmentUpdate>(),
        );
}

/// Converts an engine refusal into the API's stable error codes.
pub(crate) fn refused(error: UpdateError) -> ApiError {
    match error {
        UpdateError::NotFound => ApiError::not_found(),
        UpdateError::Denied => ApiError::new(
            ErrorCode::AuthzDenied,
            "manage permission on this object is required",
        ),
        UpdateError::Expired => {
            ApiError::new(ErrorCode::Gone, "the plan has expired; make a new one")
        }
        UpdateError::Held(job) => ApiError::new(
            ErrorCode::ConflictExists,
            "another job holds this unit or one of its threads",
        )
        .with("job", job),
        UpdateError::Blocked(blockers) => {
            ApiError::new(ErrorCode::PlanBlocked, "the plan cannot be carried out")
                .with("blockers", blockers)
        }
        UpdateError::Invalid(message) => ApiError::new(ErrorCode::RequestInvalid, message),
        UpdateError::Internal(error) => ApiError::engine(error),
    }
}
orch_api_base::__api_bind_c3cec1834a!();

/// Refuses configuration changes while the environment or one of its threads is held.
pub(crate) async fn ensure_unheld(conn: &mut Conn, environment: Id) -> ApiResult<()> {
    orch_db::queries::environments::hosting::lock_environment(conn, environment)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if let Some(job) = jobs::changes::environment_hold(conn, environment).await? {
        return Err(
            ApiError::new(ErrorCode::ConflictExists, "a job holds this environment")
                .with("job", orch_engine::locks::job_ref(&job)),
        );
    }
    Ok(())
}
orch_api_base::__api_bind_e77c594372!();

/// Stores the complete recipe; plans disclose only thread names the caller may see.
pub(crate) async fn keep(
    ctx: &Ctx,
    conn: &mut Conn,
    draft: &Draft,
    id: Id,
    kind: &str,
    node: Option<Id>,
) -> ApiResult<()> {
    jobs::forks::save_preflight(
        conn,
        &PreflightRow {
            id,
            kind: kind.to_owned(),
            node_id: node,
            environment_id: match draft.recipe.target {
                Target::Environment(id) => Some(id),
                _ => None,
            },
            created_by: draft.recipe.by,
            plan: serde_json::to_value(&draft.recipe).map_err(ApiError::internal)?,
            expires_at: updates::expires(ctx.now),
            created_at: ctx.now,
        },
    )
    .await?;
    Ok(())
}
orch_api_base::__api_bind_bf2b3071af!();

/// Threads shown in a plan, with hidden ones counted as unnamed entries.
pub(crate) async fn threads(
    conn: &mut Conn,
    principal: &Principal,
    draft: &Draft,
) -> ApiResult<Vec<UpdateThread>> {
    threads_bounded(conn, principal, draft, None).await
}

async fn threads_bounded(
    conn: &mut Conn,
    principal: &Principal,
    draft: &Draft,
    bounds: Option<&crate::routes::access::page::NameBounds>,
) -> ApiResult<Vec<UpdateThread>> {
    let refs = match bounds {
        Some(bounds) => bounds.refs(conn, principal, &draft.recipe.threads).await?,
        None => {
            crate::routes::nodes::view::refs(
                conn,
                principal,
                &draft.recipe.threads,
                Policy::default(),
            )
            .await?
        }
    };
    let mut shown = Vec::new();
    for id in &draft.recipe.threads {
        shown.push(UpdateThread {
            thread: refs.get(id).filter(|r| !r.redacted).cloned(),
            turn_running: orch_db::queries::threads::active_turn(conn, *id)
                .await?
                .is_some(),
        });
    }
    Ok(shown)
}

/// The environment plan in the browser's contract.
pub(crate) async fn environment_plan(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
    draft: &Draft,
) -> ApiResult<EnvironmentUpdatePlan> {
    environment_plan_bounded(ctx, conn, principal, id, draft, None).await
}

/// The native plan formatter's optional protected name inputs, selected before rendering.
pub(crate) struct PlanBounds<'a> {
    pub environment: &'a api::EnvironmentRef,
    pub names: &'a crate::routes::access::page::NameBounds,
    pub template_name: bool,
}

/// The same plan, using the existing response source and name bounds on scoped replies.
pub(crate) async fn environment_plan_bounded(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
    draft: &Draft,
    bounds: Option<PlanBounds<'_>>,
) -> ApiResult<EnvironmentUpdatePlan> {
    let Target::Environment(environment) = draft.recipe.target else {
        return Err(ApiError::internal("an environment plan names a provider"));
    };
    Ok(EnvironmentUpdatePlan {
        target_programs: Some(
            crate::routes::admin::catalogue::target_programs(ctx, conn, &draft.recipe.tools)
                .await?,
        ),
        notify: Some(
            audience_bounded(
                ctx,
                conn,
                principal,
                &draft.recipe.threads,
                bounds.as_ref().map(|b| b.names),
            )
            .await?,
        ),
        preflight_id: id,
        expires_at: Time::new(updates::expires(ctx.now)),
        environment: match &bounds {
            Some(bounds) => bounds.environment.clone(),
            None => super::show(ctx, conn, principal, environment).await?.base,
        },
        reason: if draft.recipe.reason == api::UpdatePreflightReason::Update {
            UpdateReason::Update
        } else {
            UpdateReason::Recreate
        },
        updates: draft
            .updates
            .iter()
            .cloned()
            .map(|mut update| {
                if update.what == api::UpdateWhat::Template
                    && bounds.as_ref().is_some_and(|bounds| !bounds.template_name)
                {
                    update.name = "Template".to_owned();
                }
                update
            })
            .collect(),
        kept: draft.kept.clone(),
        lost: draft.lost.clone(),
        rollback: Rollback::Unit,
        network: super::network::planned_bounded(
            conn,
            principal,
            &draft.recipe,
            bounds.as_ref().map(|b| b.names),
        )
        .await?,
        threads: threads_bounded(conn, principal, draft, bounds.as_ref().map(|b| b.names)).await?,
        options: draft.recipe.options.clone(),
        estimate: updates::estimate(),
        blockers: draft.blockers.clone(),
    })
}

/// The provider plan in the browser's contract.
pub(crate) async fn provider_plan(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
    draft: &Draft,
) -> ApiResult<ProviderUpdatePlan> {
    let Target::Provider(provider) = draft.recipe.target else {
        return Err(ApiError::internal("a provider plan names an environment"));
    };
    let provider = crate::routes::resources::view::provider_refs(conn, &[provider], &[])
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?
        .1;
    Ok(ProviderUpdatePlan {
        target_programs: Some(
            crate::routes::admin::catalogue::target_programs(ctx, conn, &draft.recipe.tools)
                .await?,
        ),
        notify: Some(audience(ctx, conn, principal, &draft.recipe.threads).await?),
        recovery: false,
        notification: None,
        admin_grants: Vec::new(),
        preflight_id: id,
        expires_at: Time::new(updates::expires(ctx.now)),
        provider,
        reason: if draft.recipe.reason == api::UpdatePreflightReason::Update {
            UpdateReason::Update
        } else {
            UpdateReason::Recreate
        },
        updates: draft.updates.clone(),
        kept: draft.kept.clone(),
        lost: draft.lost.clone(),
        rollback: Rollback::Unit,
        network: super::network::planned(conn, principal, &draft.recipe).await?,
        threads: threads(conn, principal, draft).await?,
        options: draft.recipe.options.clone(),
        estimate: updates::estimate(),
        blockers: draft.blockers.clone(),
    })
}
orch_api_base::__api_bind_3149c67600!();

async fn preflight(
    mut ctx: Ctx,
    (): (),
    body: api::UpdatePreflightBody,
) -> ApiResult<EnvironmentUpdatePlan> {
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let principal = guard.into_principal();
    let draft = ctx
        .app
        .engine
        .draft_update(
            &mut tx,
            &principal,
            Target::Environment(ctx.id("id")?),
            body.reason.unwrap_or(api::UpdatePreflightReason::Update),
            None,
        )
        .await
        .map_err(refused)?;
    let id = new_id();
    let plan = environment_plan(&ctx, &mut tx, &principal, id, &draft).await?;
    keep(&ctx, &mut tx, &draft, id, "environment_update", None).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(plan)
}

/// Confirms one plan in a transaction that also takes every affected hold.
/// Confirms the unit change and records the caller's optional notice in that transaction.
pub(crate) async fn confirm_notifying(
    mut ctx: Ctx,
    target: Target,
    body: ForkCreateBody,
    notify: bool,
) -> ApiResult<Job> {
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let principal = guard.into_principal();
    let row = jobs::changes::lock_plan(&mut tx, body.preflight_id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if row.created_by != principal.user() {
        return Err(ApiError::not_found());
    }
    if row.expires_at <= ctx.now {
        return Err(refused(UpdateError::Expired));
    }
    let recipe: Recipe = serde_json::from_value(row.plan).map_err(ApiError::internal)?;
    let kind = match target {
        Target::Environment(_) => "environment_update",
        Target::Provider(_) => "provider_update",
    };
    if recipe.target != target || row.kind != kind {
        return Err(ApiError::not_found());
    }
    let draft = ctx
        .app
        .engine
        .draft_update(
            &mut tx,
            &principal,
            target,
            recipe.reason,
            body.options.as_ref(),
        )
        .await
        .map_err(refused)?;
    if draft.recipe.old != recipe.old || draft.recipe.hash != recipe.hash {
        return Err(ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "the unit or its selected target changed after this plan was made; review a new plan",
        ));
    }
    let affected = draft.recipe.threads.clone();
    let job = orch_engine::jobs::recreate::queue(&mut tx, draft, None, ctx.now)
        .await
        .map_err(refused)?;
    if notify {
        tell_update(&ctx, &mut tx, &affected, job.id).await?;
    }
    match target {
        Target::Environment(_) => {
            super::jobs::creation::observed::<super::jobs::creation::EnvironmentUpdate>(
                &mut tx,
                principal.user(),
                job.id,
            )
            .await?
        }
        Target::Provider(_) => {
            super::jobs::creation::observed::<super::jobs::creation::ProviderUpdate>(
                &mut tx,
                principal.user(),
                job.id,
            )
            .await?
        }
    }
    jobs::forks::forget_preflight(&mut tx, body.preflight_id).await?;
    let shown = super::jobs::view(&mut tx, &job).await?;
    tx.commit().await.map_err(DbError::from)?;
    ctx.app.engine.kick();
    Ok(shown)
}
orch_api_base::__api_bind_c44e070c1c!();

async fn create(
    ctx: Ctx,
    query: orch_api_contract::forks::UpdateConfirmQuery,
    body: ForkCreateBody,
) -> ApiResult<orch_api_contract::workspace::JobCreationResponse> {
    let target = Target::Environment(ctx.id("id")?);
    let notify = query.notify.or(body.notify).unwrap_or(true);
    let job = confirm_notifying(ctx.clone(), target, body, notify).await?;
    super::jobs::creation::respond::<super::jobs::creation::EnvironmentUpdate>(
        &ctx,
        job.id,
        Some(job),
    )
    .await
}

/// Owners and active input authors are the people whose work a unit replacement holds.
async fn affected_people(conn: &mut Conn, threads: &[Id], by: Id) -> ApiResult<Vec<(Id, Id)>> {
    let affected: Vec<(Id,Id)> = sqlx::query_as("SELECT DISTINCT thread_id,person FROM (
        SELECT t.node_id AS thread_id,n.owner_id AS person FROM threads t JOIN nodes n ON n.id=t.node_id WHERE t.node_id=ANY($1)
        UNION SELECT i.thread_id,people.person FROM thread_inputs i
            CROSS JOIN LATERAL unnest(ARRAY[i.author_id,i.run_as]) AS people(person)
            LEFT JOIN turns t ON t.id=i.turn_id AND t.thread_id=i.thread_id
            WHERE i.thread_id=ANY($1) AND (i.state='delivering' OR t.state IN ('starting','running'))) affected
        WHERE person IS NOT NULL AND person<>$2")
        .bind(threads).bind(by).fetch_all(&mut *conn).await.map_err(DbError::from)?;
    let eligible: std::collections::HashSet<_> =
        orch_db::queries::notifications::subscribers::people_among(
            conn,
            &affected
                .iter()
                .map(|(_, person)| *person)
                .collect::<Vec<_>>(),
        )
        .await?
        .into_iter()
        .collect();
    Ok(affected
        .into_iter()
        .filter(|(_, person)| eligible.contains(person))
        .collect())
}

/// Shows one distinct recipient list, hiding people reached only through hidden threads.
pub(crate) async fn audience(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    threads: &[Id],
) -> ApiResult<orch_api_contract::forks::UpdateAudience> {
    audience_bounded(ctx, conn, principal, threads, None).await
}

/// The same notification audience, within this operation's current optional name sources.
pub(crate) async fn audience_bounded(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    threads: &[Id],
    bounds: Option<&crate::routes::access::page::NameBounds>,
) -> ApiResult<orch_api_contract::forks::UpdateAudience> {
    let affected = affected_people(conn, threads, ctx.user()?.id()).await?;
    let refs = match bounds {
        Some(bounds) => bounds.refs(conn, principal, threads).await?,
        None => {
            crate::routes::nodes::view::refs(conn, principal, threads, Policy::default()).await?
        }
    };
    let all: std::collections::BTreeSet<_> = affected.iter().map(|(_, person)| *person).collect();
    let visible: std::collections::BTreeSet<_> = affected
        .iter()
        .filter(|(thread, _)| {
            refs.get(thread)
                .is_some_and(|reference| !reference.redacted)
        })
        .map(|(_, person)| *person)
        .collect();
    let people =
        authorization::refusals::user_refs(conn, &visible.iter().copied().collect::<Vec<_>>())
            .await?;
    Ok(orch_api_contract::forks::UpdateAudience {
        people: visible
            .iter()
            .filter_map(|id| people.get(id).cloned())
            .collect(),
        hidden: all.len().saturating_sub(people.len()) as i64,
    })
}
orch_api_base::__api_bind_8e42e7dfad!();

/// One durable Inbox item per affected person and operation, regardless of how many units they use.
pub(crate) async fn tell_update(
    ctx: &Ctx,
    conn: &mut Conn,
    threads: &[Id],
    operation: Id,
) -> ApiResult<()> {
    let by = ctx.user()?.id();
    let mut people: std::collections::BTreeMap<Id, std::collections::BTreeSet<Id>> =
        std::collections::BTreeMap::new();
    for (thread, person) in affected_people(conn, threads, by).await? {
        people.entry(person).or_default().insert(thread);
    }
    for (person, affected) in people {
        let Some(thread) = affected.first().copied() else {
            continue;
        };
        let event = orch_engine::notifications::Event {
            kind: orch_engine::notifications::Kind::EnvironmentUpdate,
            node: Some(thread),
            thread: Some(thread),
            approval: None,
            title: "Update requested".to_owned(),
            body: format!("{} affected threads; running work may stop", affected.len()),
            data: json!({"actor":by,"operation":operation,"reason":"requested","affected_threads":affected.len() as i64,"threads":affected,"path":format!("/thread/{thread}")}),
            source: format!("{by}:{operation}"),
            actor: Some(by),
            at: ctx.now,
        };
        orch_engine::notifications::tell(conn, &event, &[person], None)
            .await
            .map_err(ApiError::internal)?;
    }
    Ok(())
}
orch_api_base::__api_bind_6ac5ae2c78!();

/// Provider runtime batches retain their exact accepted preview in the existing private row.
pub(crate) async fn keep_provider_batch(
    ctx: &Ctx,
    conn: &mut Conn,
    id: Id,
    recipes: &[Recipe],
    origin: &orch_authz::CapturedOperation,
) -> ApiResult<()> {
    if origin.operation() != "ProviderRuntimeUpdateBatchPreflight"
        || origin.user() != ctx.user()?.id()
        || recipes.iter().any(|recipe| {
            !matches!(recipe.target, Target::Provider(_)) || recipe.by != origin.user()
        })
    {
        return Err(ApiError::internal(
            "the provider batch preview origin does not match its recipes",
        ));
    }
    keep_batch_plan(
        ctx,
        conn,
        id,
        None,
        json!({"recipes":recipes,"providers":true,"provider_update_origins":[origin]}),
    )
    .await
}
orch_api_base::__api_bind_0daadb08b6!();

/// Retains each actual executable review source for its independently checked confirmation.
pub(crate) async fn keep_batch_with_sources(
    ctx: &Ctx,
    conn: &mut Conn,
    id: Id,
    node: Option<Id>,
    recipes: &[Recipe],
    sources: &std::collections::BTreeMap<Id, orch_engine::updates::operation::Input>,
) -> ApiResult<()> {
    let mut selected = std::collections::BTreeMap::new();
    for recipe in recipes {
        let source = sources
            .get(&recipe.target.id())
            .ok_or_else(|| ApiError::internal("the native review source is missing"))?;
        if source.environment() != recipe.target.id()
            || source.origin().user() != recipe.by
            || source.origin().operation() != ctx.route.name
        {
            return Err(ApiError::internal(
                "the native review source differs from its recipe",
            ));
        }
        selected.insert(recipe.target.id(), source.clone());
    }
    keep_batch_plan(
        ctx,
        conn,
        id,
        node,
        json!({"recipes":recipes,"providers":false,"update_review_sources":selected}),
    )
    .await
}

async fn keep_batch_plan(
    ctx: &Ctx,
    conn: &mut Conn,
    id: Id,
    node: Option<Id>,
    plan: orch_types::Json,
) -> ApiResult<()> {
    jobs::forks::save_preflight(
        conn,
        &PreflightRow {
            id,
            kind: "update_batch".to_owned(),
            node_id: node,
            environment_id: None,
            created_by: ctx.user()?.id(),
            plan,
            expires_at: updates::expires(ctx.now),
            created_at: ctx.now,
        },
    )
    .await?;
    Ok(())
}

/// The existing stored-review selection, with a nonlocking form for once-only initial IO.
pub(crate) async fn reviewed_batch(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    node: Option<Id>,
    providers: bool,
    body: &ForkCreateBody,
    lock: bool,
) -> ApiResult<Vec<Recipe>> {
    Ok(
        reviewed_batch_row(ctx, conn, principal, node, providers, body, lock)
            .await?
            .1,
    )
}

/// Stored metadata and native recipes selected together, before formatting or external IO.
pub(crate) async fn reviewed_batch_row(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    node: Option<Id>,
    providers: bool,
    body: &ForkCreateBody,
    lock: bool,
) -> ApiResult<(PreflightRow, Vec<Recipe>)> {
    let row = if lock {
        jobs::changes::lock_plan(conn, body.preflight_id).await?
    } else {
        jobs::forks::preflight(conn, body.preflight_id).await?
    }
    .ok_or_else(ApiError::not_found)?;
    if row.kind != "update_batch"
        || row.node_id != node
        || row.created_by != principal.user()
        || row.plan["providers"] != providers
    {
        return Err(ApiError::not_found());
    }
    if row.expires_at <= ctx.app.clock.now() {
        return Err(refused(UpdateError::Expired));
    }
    let recipes: Vec<Recipe> =
        serde_json::from_value(row.plan["recipes"].clone()).map_err(ApiError::internal)?;
    let mut ids = std::collections::BTreeSet::new();
    if recipes.iter().any(|recipe| {
        recipe.by != principal.user()
            || matches!(recipe.target, Target::Provider(_)) != providers
            || !ids.insert(recipe.target.id())
    }) {
        return Err(ApiError::not_found());
    }
    Ok((row, recipes))
}

/// The same native confirmation comparison for fresh ordinary and observation-backed drafts.
pub(crate) fn check_reviewed_draft(
    recipe: &Recipe,
    draft: &Draft,
    providers: bool,
) -> ApiResult<()> {
    if draft.recipe.old != recipe.old
        || draft.recipe.hash != recipe.hash
        || providers
            && (draft.recipe.host != recipe.host || draft.recipe.template != recipe.template)
    {
        return Err(ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "a unit or its selected target changed after this batch was planned; review a new plan",
        ));
    }
    if !draft.blockers.is_empty() {
        return Err(refused(UpdateError::Blocked(draft.blockers.clone())));
    }
    Ok(())
}

/// Checks a batch before any member is queued, so a refusal leaves no partial set of jobs.
pub(crate) async fn batch_drafts(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    node: Option<Id>,
    providers: bool,
    body: &ForkCreateBody,
) -> ApiResult<Vec<Draft>> {
    let recipes = reviewed_batch(ctx, conn, principal, node, providers, body, true).await?;
    let mut drafts = Vec::new();
    for recipe in recipes {
        let draft = ctx
            .app
            .engine
            .draft_update(
                conn,
                principal,
                recipe.target,
                api::UpdatePreflightReason::Update,
                body.options.as_ref(),
            )
            .await
            .map_err(refused)?;
        check_reviewed_draft(&recipe, &draft, providers)?;
        drafts.push(draft);
    }
    Ok(drafts)
}
orch_api_base::__api_bind_d91e721e63!();
