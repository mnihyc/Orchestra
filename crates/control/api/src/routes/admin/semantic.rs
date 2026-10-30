//! Search by meaning as a system admin sees it: whether the database can keep vectors, each root org's index with its progress, estimate, disk and reason, the choice to rebuild one in place, and the provider Local embeddings.
//!
//! The overview lists the root orgs where search by meaning is on, and those that still have an index. A
//! rebuild in place removes an org's vectors so that the indexer makes them anew with the model named: the
//! choice an admin makes when the disk share cannot hold a new model's vectors beside the present ones.

use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use crate::routes::admin::admin_of;
use orch_api_contract::admin::{
    AdminSemanticSearchGet, AdminSemanticSearchKeep, AdminSemanticSearchRebuild,
    SemanticGeneration, SemanticSearchKeepBody, SemanticSearchOrg, SemanticSearchOverview,
    SemanticState,
};
use orch_api_contract::nodes::{NodeKind, NodeRef};
use orch_db::DbError;
use orch_engine::semantic::state::{Generation, OrgState};
use serde_json::json;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on(AdminSemanticSearchGet, overview)
        .on(AdminSemanticSearchRebuild, rebuild)
        .on(AdminSemanticSearchKeep, keep);
}

fn state_of(word: &str) -> SemanticState {
    match word {
        "off" => SemanticState::Off,
        "not_named" => SemanticState::NotNamed,
        "unavailable" => SemanticState::Unavailable,
        "indexing" => SemanticState::Indexing,
        "paused" => SemanticState::Paused,
        "waiting" => SemanticState::Waiting,
        _ => SemanticState::Idle,
    }
}

fn generation_of(generation: Generation) -> SemanticGeneration {
    SemanticGeneration {
        id: generation.id,
        provider_id: generation.provider_id,
        model_name: generation.model_name,
        asked_dimensions: generation.asked_dimensions,
        dimensions: generation.dimensions,
        query_prefix: generation.query_prefix,
        document_prefix: generation.document_prefix,
    }
}

fn view(org: NodeRef, state: OrgState) -> SemanticSearchOrg {
    SemanticSearchOrg {
        org,
        enabled: state.enabled,
        provider_id: state.provider_id,
        model_name: state.model_name,
        dimensions: state.dimensions,
        state: state_of(&state.state),
        reason: state.reason,
        detail: state.detail,
        active: state.active.map(generation_of),
        building: state.building.map(generation_of),
        tokens: state.tokens,
        pending_tokens: state.pending_tokens,
        tokens_per_second: state.rate,
        seconds_left: state.seconds_left,
        content_bytes: state.content_bytes,
        used_bytes: state.used_bytes,
        allowed_bytes: state.allowed_bytes,
        rebuild_bytes: state.rebuild_bytes,
        rebuild_seconds: state.rebuild_seconds,
    }
}

fn org_ref(id: orch_types::Id, name: String) -> NodeRef {
    NodeRef {
        id,
        kind: NodeKind::Org,
        name: Some(name),
        redacted: false,
        thread_display: None,
    }
}

async fn overview(ctx: Ctx, (): (), (): ()) -> ApiResult<SemanticSearchOverview> {
    admin_of(&ctx)?;
    let engine = &ctx.app.engine;
    let ready = engine
        .semantic_available()
        .await
        .map_err(ApiError::internal)?;
    let interval_secs = engine
        .semantic_interval()
        .await
        .map_err(ApiError::internal)?;
    let mut orgs = Vec::new();
    for (id, name) in engine
        .semantic_orgs_shown()
        .await
        .map_err(ApiError::internal)?
    {
        let state = engine
            .semantic_state(id)
            .await
            .map_err(ApiError::internal)?;
        orgs.push(view(org_ref(id, name), state));
    }
    Ok(SemanticSearchOverview {
        available: ready.is_ok(),
        unavailable_reason: ready.err(),
        interval_secs,
        orgs,
    })
}

async fn rebuild(ctx: Ctx, (): (), (): ()) -> ApiResult<SemanticSearchOrg> {
    admin_of(&ctx)?;
    let id = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    let (_, name) = orch_db::queries::semantic::root_orgs(&mut conn)
        .await?
        .into_iter()
        .find(|(org, _)| *org == id)
        .ok_or_else(ApiError::not_found)?;
    drop(conn);
    let engine = &ctx.app.engine;
    engine
        .semantic_rebuild_in_place(id)
        .await
        .map_err(ApiError::internal)?;
    let state = engine
        .semantic_state(id)
        .await
        .map_err(ApiError::internal)?;
    engine.kick();
    Ok(view(org_ref(id, name), state))
}

/// Keeps the whole vector configuration together, so retaining a model also retains its prompts and cuts.
async fn keep(ctx: Ctx, (): (), body: SemanticSearchKeepBody) -> ApiResult<SemanticSearchOrg> {
    use orch_api_contract::common::ErrorCode;
    use orch_db::queries::semantic as rows;
    use orch_settings::write::{Write, write};
    use orch_settings_contract::registry::keys;

    admin_of(&ctx)?;
    let org = ctx.id("id")?;
    let (ctx, mut tx, principal) = crate::routes::nodes::action(&ctx).await?;
    admin_of(&ctx)?;
    let name = rows::root_orgs(&mut tx)
        .await?
        .into_iter()
        .find(|(id, _)| *id == org)
        .map(|(_, name)| name)
        .ok_or_else(ApiError::not_found)?;
    let claimed: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT claimed_until FROM search_indexes WHERE org_id = $1 FOR UPDATE")
            .bind(org)
            .fetch_optional(&mut *tx)
            .await
            .map_err(DbError::from)?
            .flatten();
    if claimed.is_some_and(|until| until > ctx.now) {
        return Err(ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "The indexer is finishing a batch; try again when it has finished.",
        ));
    }
    let generations = rows::generations(&mut tx, org).await?;
    let active = generations
        .iter()
        .find(|generation| generation.state == "active" && generation.id == body.generation_id)
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::ConflictStaleRevision,
                "The active index changed; read the current model before choosing.",
            )
        })?;
    let chunk_tokens = active
        .chunking
        .rsplit_once(':')
        .and_then(|(_, size)| size.parse::<i64>().ok())
        .ok_or_else(|| ApiError::internal("the active index's chunk size is unreadable"))?;
    let mut named = json!({"provider_id":active.provider_id,"model_name":active.model_name,
        "query_prefix":active.query_prefix,"document_prefix":active.document_prefix});
    if let Some(dimensions) = active.asked_dimensions {
        named["dimensions"] = json!(dimensions);
    }
    let mut tuning = orch_settings::get::<serde_json::Value>(
        &mut tx,
        orch_settings::Place::Node(org),
        keys::SEARCH_SEMANTIC_TUNING,
    )
    .await
    .map_err(ApiError::internal)?
    .ok_or_else(|| ApiError::internal("search tuning has no value"))?;
    tuning["chunk_tokens"] = json!(chunk_tokens);
    let policy = crate::routes::nodes::policy_at(&mut tx, org).await?;
    let origin = crate::sign_in::origin(&ctx);
    for (key, value) in [
        (keys::SEARCH_SEMANTIC_EMBEDDINGS, named),
        (keys::SEARCH_SEMANTIC_TUNING, tuning),
    ] {
        let may_lock =
            crate::routes::settings::permit(&mut tx, &principal, org, key, policy).await?;
        let written = write(
            &mut tx,
            &Write {
                actor: orch_db::audit::Actor::User(principal.user()),
                target: orch_settings::Target::Node(org),
                key,
                value,
                locked: None,
                expected_revision: None,
                may_lock,
                origin: &origin,
            },
            ctx.now,
        )
        .await;
        if let Err(error) = written {
            return Err(
                crate::routes::settings::write_failed(&mut tx, &principal, org, error).await,
            );
        }
    }
    for generation in &generations {
        if generation.state == "building" {
            rows::drop_generation(&mut tx, generation.id).await?;
        }
    }
    rows::generation_writer(&mut tx, active.id, Some(principal.user())).await?;
    rows::reset_index(&mut tx, org, ctx.now).await?;
    orch_db::queries::hierarchy::stamp(&mut tx, org).await?;
    tx.commit().await.map_err(DbError::from)?;
    ctx.app.engine.kick();
    let state = ctx
        .app
        .engine
        .semantic_state(org)
        .await
        .map_err(ApiError::internal)?;
    Ok(view(org_ref(org, name), state))
}
