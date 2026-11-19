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
    SemanticSearchResponse, SemanticState,
};
use orch_api_contract::nodes::{NodeKind, NodeRef};
use orch_engine::semantic::state::{Generation, OrgState};

mod action;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on(AdminSemanticSearchGet, overview)
        .on_with_operation(
            AdminSemanticSearchRebuild,
            crate::key_operations::native::effects::prepare::<action::Action<action::Rebuild>>,
            rebuild,
        )
        .on_with_operation(
            AdminSemanticSearchKeep,
            crate::key_operations::native::effects::prepare::<action::Action<action::Keep>>,
            keep,
        );
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

async fn rebuild(ctx: Ctx, (): (), (): ()) -> ApiResult<SemanticSearchResponse> {
    action::run::<action::Rebuild>(ctx).await
}

/// Keeps the model, prompts and cuts through the same native prepared writer.
async fn keep(
    ctx: Ctx,
    (): (),
    _body: SemanticSearchKeepBody,
) -> ApiResult<SemanticSearchResponse> {
    action::run::<action::Keep>(ctx).await
}
