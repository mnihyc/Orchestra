//! Results by meaning joined with results by words in people's search ordered by best matches: one order by reciprocal-rank fusion, each result checked against what its reader may read now.
//!
//! The provider is called with no database transaction open. Each batch is hydrated only after its parts pass
//! the same current permission projection and filters as literal search.

use crate::middleware::{
    authorization,
    requests::{ApiError, ApiResult},
};
use crate::registry::Ctx;
use orch_api_contract::search::{SearchHit, SearchListQuery};
use orch_db::queries::search as rows;
use orch_engine::semantic::query::Meaning;
use orch_settings::{Place, get};
use orch_settings_contract::registry::keys;
use std::collections::{BTreeMap, HashSet};

/// An org's source advances in its own candidate windows and embedding generation.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub(super) struct Position {
    /// Raw candidates consumed in this org.
    pub offset: i64,
    /// The model generation used by earlier windows.
    pub generation: Option<orch_types::Id>,
    /// No further request is made for a completed or unavailable source.
    pub done: bool,
    /// An unavailable source keeps its reason through continued pages.
    pub reason: Option<String>,
    /// An intentional index pause does not make a cached-vector query unavailable.
    #[serde(default)]
    pub paused: Option<String>,
}

/// One independent semantic batch, ready for the stream's server-side fusion.
pub(super) struct Batch {
    pub hits: Vec<SearchHit>,
    pub ranks: Vec<Vec<String>>,
    pub more: bool,
    pub positions: BTreeMap<orch_types::Id, Position>,
    pub fusion_k: u32,
    pub unavailable: Option<String>,
    pub paused: Option<String>,
}

pub(super) async fn retrieve(
    mut ctx: Ctx,
    query: SearchListQuery,
    mut positions: BTreeMap<orch_types::Id, Position>,
    legacy_offset: i64,
    origin: Option<orch_authz::CapturedOperation>,
    shown: Vec<String>,
) -> ApiResult<Batch> {
    let mut orgs = ctx
        .app
        .engine
        .semantic_orgs()
        .await
        .map_err(ApiError::internal)?;
    let prepared = if origin.is_some() {
        let (_, _, output) = super::stream_native::read(
            ctx.clone(),
            super::stream_native::Query::MeaningScope {
                query: query.clone(),
                orgs,
                origin: origin.clone(),
            },
        )
        .await?;
        match output {
            super::stream_native::Output::MeaningScope(value) => value,
            _ => {
                return Err(ApiError::internal(
                    "the meaning source returned another native frame",
                ));
            }
        }
    } else {
        let mut conn = ctx.app.db.begin().await?;
        rows::snapshot(&mut conn).await?;
        let mut cancellation = rows::cancel_on_drop(&ctx.app.db, &mut conn).await?;
        let principal = authorization::principal(&ctx, &mut conn).await?;
        let prepared = prepare_scope(&ctx, &mut conn, &principal, &query, orgs, None).await?;
        cancellation.disarm();
        conn.commit().await.map_err(orch_db::DbError::from)?;
        prepared
    };
    orgs = prepared.orgs;
    let filter = prepared.filter;
    let mut origins = prepared.origins;
    let eligible: HashSet<_> = orgs.iter().copied().collect();
    for (org, position) in &mut positions {
        position.done |= !eligible.contains(org);
    }
    for org in &orgs {
        positions.entry(*org).or_insert_with(|| Position {
            offset: legacy_offset,
            ..Position::default()
        });
    }
    let phase = std::time::Instant::now();
    let requests = orgs.into_iter().filter_map(|org| {
        positions
            .get(&org)
            .filter(|position| !position.done)
            .map(|position| (org, position.clone()))
    });
    let scoped = origin.is_some();
    let answers = futures::future::join_all(requests.map(|(org, position)| {
        let engine = &ctx.app.engine;
        let query = &query.q;
        let filter = filter.clone();
        let origin = origins.remove(&org);
        async move {
            if scoped && origin.is_none() {
                return (
                    org,
                    Err(orch_engine::EngineError::BeforeEffect(Box::new(
                        orch_engine::operation_scope::continuing::Error::Refused,
                    ))),
                );
            }
            (
                org,
                engine
                    .meaning_filtered_for_origin(
                        org,
                        query,
                        position.offset,
                        position.generation,
                        Some(filter),
                        origin,
                    )
                    .await,
            )
        }
    }))
    .await;
    tracing::debug!(target: "orch::search::timing", phase="meaning_lookup", elapsed_ms=phase.elapsed().as_secs_f64()*1000.);
    let mut lists = Vec::new();
    let mut near = Vec::new();
    let mut fusion_k = None;
    for (org, answer) in answers {
        let position = positions
            .get_mut(&org)
            .ok_or_else(|| ApiError::internal("semantic source position missing"))?;
        match answer.map_err(ApiError::internal)? {
            Meaning::Found {
                near: found,
                fusion_k: k,
                candidates,
                generation_id,
                paused,
            } => {
                let consumed = i64::try_from(found.len()).unwrap_or(i64::MAX);
                position.offset = position.offset.saturating_add(consumed);
                position.generation = Some(generation_id);
                position.paused = paused;
                position.done = consumed < candidates;
                lists.push(
                    found
                        .iter()
                        .filter_map(|part| {
                            part.first_part.map(|id| match part.kind.as_str() {
                                "document" => format!("doc:{}:{id}", part.source_id),
                                "chat" => format!("chat:{id}"),
                                _ => format!("item:{id}"),
                            })
                        })
                        .collect::<Vec<_>>(),
                );
                near.extend(found);
                fusion_k.get_or_insert(k);
            }
            Meaning::Unavailable(reason) => {
                position.done = true;
                position.reason = Some(reason);
            }
            Meaning::Off => position.done = true,
        }
    }
    let more = positions.values().any(|position| !position.done);
    let unavailable = positions
        .values()
        .find_map(|position| position.reason.clone());
    let paused = positions
        .values()
        .find_map(|position| position.paused.clone());
    if near.is_empty() {
        return Ok(Batch {
            hits: Vec::new(),
            ranks: lists,
            more,
            positions,
            fusion_k: fusion_k.unwrap_or(super::stream::DEFAULT_FUSION_K),
            unavailable,
            paused,
        });
    }
    ctx.now = ctx.app.clock.now();
    let hits = if origin.is_some() {
        let (_, _, output) = super::stream_native::read(
            ctx.clone(),
            super::stream_native::Query::MeaningHits {
                query: query.clone(),
                near,
                shown: shown.into_iter().collect(),
                origin,
            },
        )
        .await?;
        match output {
            super::stream_native::Output::MeaningHits(hits) => hits,
            _ => {
                return Err(ApiError::internal(
                    "the meaning parts returned another native frame",
                ));
            }
        }
    } else {
        let mut tx = ctx.app.db.begin().await?;
        rows::snapshot(&mut tx).await?;
        let mut cancellation = rows::cancel_on_drop(&ctx.app.db, &mut tx).await?;
        let principal = authorization::principal(&ctx, &mut tx).await?;
        let found = prepare_hits(&ctx, &mut tx, &principal, &query, &near).await?;
        let phase = std::time::Instant::now();
        let hits = super::content::hydrate(&mut tx, &principal, &found, &query, ctx.now).await?;
        tracing::debug!(target: "orch::search::timing", phase="meaning_payloads", elapsed_ms=phase.elapsed().as_secs_f64()*1000.);
        cancellation.disarm();
        tx.commit().await.map_err(orch_db::DbError::from)?;
        hits
    };
    let allowed: HashSet<String> = hits.iter().map(super::stream::identity).collect();
    for list in &mut lists {
        list.retain(|id| allowed.contains(id));
    }
    Ok(Batch {
        hits,
        ranks: lists,
        more,
        positions,
        fusion_k: fusion_k.unwrap_or(super::stream::DEFAULT_FUSION_K),
        unavailable,
        paused,
    })
}

/// Native semantic-source selection and filters, with no model call in the frame.
pub(super) struct PreparedScope {
    pub orgs: Vec<orch_types::Id>,
    pub filter: orch_types::Json,
    pub origins: BTreeMap<orch_types::Id, orch_engine::semantic::query_origin::QueryOrigin>,
}
pub(super) async fn prepare_scope(
    ctx: &Ctx,
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    query: &SearchListQuery,
    mut orgs: Vec<orch_types::Id>,
    readable: Option<&orch_types::Json>,
) -> ApiResult<PreparedScope> {
    let requested = super::kinds(query.kinds.as_deref(), super::content::CONTENT_KINDS)?;
    let item_kinds = requested
        .iter()
        .filter(|k| !matches!(k.as_str(), "document" | "chat_message"))
        .cloned()
        .collect::<Vec<_>>();
    let mut filter = serde_json::to_value(query).map_err(ApiError::internal)?;
    let source_kinds: Vec<_> = [
        ("document", requested.iter().any(|kind| kind == "document")),
        ("chat", requested.iter().any(|kind| kind == "chat_message")),
        ("conversation", !item_kinds.is_empty()),
    ]
    .into_iter()
    .filter_map(|(kind, enabled)| enabled.then_some(kind))
    .collect();
    filter["source_kinds"] = serde_json::json!(source_kinds);
    filter["item_kinds"] = serde_json::json!(item_kinds);
    {
        let phase = std::time::Instant::now();
        let advanced = super::advanced_selection(conn, principal, query, ctx.now).await?;
        for key in ["search_user", "search_at", "search_holders"] {
            if let Some(value) = advanced.get(key) {
                filter[key] = value.clone();
            }
        }
        filter["credential_scope"] = serde_json::to_value(
            principal
                .caller()
                .grant()
                .and_then(orch_authz::TokenGrant::subtree),
        )
        .map_err(ApiError::internal)?;
        if query.mentions_me == Some(true) {
            filter["mentions_id"] =
                serde_json::to_value(principal.user()).map_err(ApiError::internal)?;
        }
        let selected_documents: Vec<_> = query.document_id.into_iter().collect();
        let selected_chats: Vec<_> = query.chat_id.into_iter().collect();
        let projection = if selected_documents.is_empty() && selected_chats.is_empty() {
            let tuning = get::<rows::candidates::Tuning>(
                conn,
                query.scope.map_or(Place::System, Place::Node),
                keys::SEARCH_WORD_TUNING,
            )
            .await
            .map_err(ApiError::internal)?
            .ok_or_else(|| ApiError::internal("word retrieval setting missing"))?;
            orch_authz::everything::project_search_scope(
                conn,
                principal,
                query.scope,
                tuning.scope_candidates,
            )
            .await
            .map_err(ApiError::internal)?
        } else {
            Some(
                orch_authz::everything::project_search(
                    conn,
                    principal,
                    query.scope,
                    &selected_documents,
                    &selected_chats,
                    &[],
                )
                .await
                .map_err(ApiError::internal)?,
            )
        };
        filter["readable"] = projection
            .as_ref()
            .map(|projection| projection.search_candidates(principal))
            .unwrap_or_else(|| serde_json::json!({"unrestricted":true}));
        if let Some(readable) = readable {
            filter["readable"] = readable.clone();
        }
        let reachable: std::collections::HashSet<orch_types::Id> = projection
            .as_ref()
            .and_then(|projection| projection.grants.get("reachable"))
            .and_then(orch_types::Json::as_array)
            .into_iter()
            .flatten()
            .filter_map(orch_types::Json::as_str)
            .filter_map(|id| id.parse().ok())
            .collect();
        let potential = if orgs.iter().any(|org| !reachable.contains(org)) {
            let held =
                orch_authz::holders::held(conn, principal.user(), orch_types::Time::new(ctx.now))
                    .await
                    .map_err(ApiError::internal)?;
            rows::name_roots(
                conn,
                principal.user(),
                principal.is_system_admin() && principal.caller().as_admin(),
                ctx.now,
                Some(&held.held),
            )
            .await?
        } else {
            Vec::new()
        };
        orgs.retain(|org| {
            (reachable.contains(org) || potential.contains(org))
                && !principal.suspension().applies(&[*org])
        });
        if let Some(scope) = query.scope {
            let root = orch_db::queries::semantic::org_of(conn, scope).await?;
            orgs.retain(|org| Some(*org) == root);
        }
        tracing::debug!(target: "orch::search::timing", phase="meaning_scope", elapsed_ms=phase.elapsed().as_secs_f64()*1000.);
    }
    Ok(PreparedScope {
        orgs,
        filter,
        origins: BTreeMap::new(),
    })
}

/// Reuses the native current permission and part filters for exactly these immutable identities.
pub(super) async fn prepare_selected(
    ctx: &Ctx,
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    query: &SearchListQuery,
    selected: &rows::CandidateSet,
) -> ApiResult<Vec<rows::HitRow>> {
    let requested = super::kinds(query.kinds.as_deref(), super::content::CONTENT_KINDS)?;
    let item_kinds = requested
        .iter()
        .filter(|k| !matches!(k.as_str(), "document" | "chat_message"))
        .cloned()
        .collect::<Vec<_>>();
    let phase = std::time::Instant::now();
    let projection = orch_authz::everything::project_search(
        conn,
        principal,
        query.scope,
        &selected.documents,
        &selected.chats,
        &selected.nodes,
    )
    .await
    .map_err(ApiError::internal)?;
    let mut grants = projection.grants.clone();
    grants["search_advanced"] = super::advanced_selection(conn, principal, query, ctx.now).await?;
    tracing::debug!(target: "orch::search::timing", phase="meaning_access", elapsed_ms=phase.elapsed().as_secs_f64()*1000., parts=selected.items.len() + selected.messages.len() + selected.blocks.len());
    let found = rows::meaning_hits(
        conn,
        &rows::MeaningAsk {
            user: principal.user(),
            groups: principal.groups(),
            member_roots: &projection.member_roots,
            grants: &grants,
            at: ctx.app.clock.now(),
            scope: query.scope,
            participating: query.participating.unwrap_or(false),
            items: &selected.items,
            messages: &selected.messages,
            blocks: &selected.blocks,
            text: &query.q,
            item_kinds: &item_kinds,
            chat_messages: requested.iter().any(|k| k == "chat_message"),
            document_content: requested.iter().any(|k| k == "document"),
            from: query.from.map(orch_types::Time::get),
            to: query.to.map(orch_types::Time::get),
            chat: query.chat_id,
            document: query.document_id,
            author: query.author_id,
            with_file: query.with_file.unwrap_or(false),
            mentions_me: query
                .mentions_me
                .unwrap_or(false)
                .then_some(principal.user()),
        },
    )
    .await?;
    Ok(found)
}

pub(super) async fn prepare_hits(
    ctx: &Ctx,
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    query: &SearchListQuery,
    near: &[orch_engine::semantic::query::Near],
) -> ApiResult<Vec<rows::HitRow>> {
    let mut selected = rows::CandidateSet::default();
    for part in near {
        match part.kind.as_str() {
            "document" => {
                selected.documents.push(part.source_id);
                selected.blocks.extend(part.first_part);
            }
            "chat" => {
                selected.chats.push(part.source_id);
                selected.messages.extend(part.first_part);
            }
            _ => {
                selected.nodes.push(part.source_id);
                selected.items.extend(part.first_part);
            }
        }
    }
    prepare_selected(ctx, conn, principal, query, &selected).await
}
