//! Update and recreate plans over HTTP, confirmed once and checked again before a runtime is held.

use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::{authorization, transaction_guard};
use crate::registry::{Ctx, Registry};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::{
    environment as api,
    forks::{
        EnvironmentUpdatePlan, ForkCreateBody, ProviderUpdatePlan, Rollback, UpdateReason,
        UpdateThread,
    },
    workspace::Job,
};
use orch_authz::{Policy, Principal};
use orch_db::queries::jobs::{self, forks::PreflightRow};
use orch_db::{Conn, DbError};
use orch_engine::updates::{self, Draft, Recipe, Target, UpdateError};
use orch_types::{Id, Time, new_id};
use serde_json::json;

/// Binds the environment replacement requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on(api::EnvironmentUpdatePreflight, preflight)
        .on(api::EnvironmentUpdateCreate, create);
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

/// Threads shown in a plan, with hidden ones counted as unnamed entries.
pub(crate) async fn threads(
    conn: &mut Conn,
    principal: &Principal,
    draft: &Draft,
) -> ApiResult<Vec<UpdateThread>> {
    let refs =
        crate::routes::nodes::view::refs(conn, principal, &draft.recipe.threads, Policy::default())
            .await?;
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
    let Target::Environment(environment) = draft.recipe.target else {
        return Err(ApiError::internal("an environment plan names a provider"));
    };
    Ok(EnvironmentUpdatePlan {
        target_programs: Some(
            crate::routes::admin::catalogue::target_programs(ctx, conn, &draft.recipe.tools)
                .await?,
        ),
        notify: Some(audience(ctx, conn, principal, &draft.recipe.threads).await?),
        preflight_id: id,
        expires_at: Time::new(updates::expires(ctx.now)),
        environment: super::show(ctx, conn, principal, environment).await?.base,
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
    jobs::forks::forget_preflight(&mut tx, body.preflight_id).await?;
    let shown = super::jobs::view(&mut tx, &job).await?;
    tx.commit().await.map_err(DbError::from)?;
    ctx.app.engine.kick();
    Ok(shown)
}

async fn create(
    ctx: Ctx,
    query: orch_api_contract::forks::UpdateConfirmQuery,
    body: ForkCreateBody,
) -> ApiResult<Job> {
    let target = Target::Environment(ctx.id("id")?);
    let notify = query.notify.or(body.notify).unwrap_or(true);
    confirm_notifying(ctx, target, body, notify).await
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
    let affected = affected_people(conn, threads, ctx.user()?.id()).await?;
    let refs =
        crate::routes::nodes::view::refs(conn, principal, threads, Policy::default()).await?;
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

/// Stores a batch once; confirmation will validate its creator, scope and all of its members again.
pub(crate) async fn keep_batch(
    ctx: &Ctx,
    conn: &mut Conn,
    id: Id,
    node: Option<Id>,
    recipes: &[Recipe],
    providers: bool,
) -> ApiResult<()> {
    jobs::forks::save_preflight(
        conn,
        &PreflightRow {
            id,
            kind: "update_batch".to_owned(),
            node_id: node,
            environment_id: None,
            created_by: ctx.user()?.id(),
            plan: json!({"recipes":recipes,"providers":providers}),
            expires_at: updates::expires(ctx.now),
            created_at: ctx.now,
        },
    )
    .await?;
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
    let row = jobs::changes::lock_plan(conn, body.preflight_id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if row.kind != "update_batch"
        || row.node_id != node
        || row.created_by != principal.user()
        || row.plan["providers"] != providers
    {
        return Err(ApiError::not_found());
    }
    if row.expires_at <= ctx.now {
        return Err(refused(UpdateError::Expired));
    }
    let recipes: Vec<Recipe> =
        serde_json::from_value(row.plan["recipes"].clone()).map_err(ApiError::internal)?;
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
        if draft.recipe.old != recipe.old || draft.recipe.hash != recipe.hash {
            return Err(ApiError::new(
                ErrorCode::ConflictStaleRevision,
                "a unit or its selected target changed after this batch was planned; review a new plan",
            ));
        }
        if !draft.blockers.is_empty() {
            return Err(refused(UpdateError::Blocked(draft.blockers)));
        }
        drafts.push(draft);
    }
    Ok(drafts)
}
