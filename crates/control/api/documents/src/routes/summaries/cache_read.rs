//! The same private-viewer, pinned-range and complete-governor stored-content read for headers and search batches.

use super::*;
pub(super) struct Checked {
    pub cache: Option<rows::Cache>,
    pub inputs: Vec<KeyResolvedTarget>,
    pub inspecting: bool,
}

pub(super) async fn guard(
    conn: &mut Conn,
    principal: &Principal,
    target: SummaryTarget,
    plan: &Prepared,
    wanted: Option<SummaryChatRange>,
    mut cache: Option<rows::Cache>,
    inputs_slot: usize,
) -> ApiResult<Checked> {
    let target_inspecting = !sources::readable(conn, principal, target)
        .await
        .map_err(source_error)?;
    let mut inspecting = target_inspecting;
    let mut inputs = Vec::new();
    if let Some(kept) = &cache {
        let refs: Vec<SummaryTarget> =
            serde_json::from_value(kept.source_refs.clone()).map_err(ApiError::internal)?;
        let mut readable = !refs.is_empty() || kept.text.is_none();
        for source in refs {
            let current = match sources::identify(conn, source).await {
                Ok(current) => current,
                Err(_) => {
                    readable = false;
                    break;
                }
            };
            if !plan
                .permits_binding(
                    principal.caller(),
                    inputs_slot,
                    current.fact().object.kind,
                    std::slice::from_ref(current.fact()),
                )
                .map_err(source_error)?
                || !sources::viewable(conn, principal, source)
                    .await
                    .map_err(source_error)?
            {
                readable = false;
                break;
            }
            if source != target {
                inputs.push(current.fact().clone());
            }
            inspecting |= !sources::readable(conn, principal, source)
                .await
                .map_err(source_error)?;
        }
        if matches!(target, SummaryTarget::Chat { .. }) {
            let selected = match wanted {
                Some(range) => Some(range),
                None => Some(
                    sources::disclosure_range(conn, principal, target.id())
                        .await
                        .map_err(source_error)?,
                ),
            };
            if kept
                .source_snapshot
                .as_ref()
                .and_then(|source| source.get("range"))
                != Some(&serde_json::to_value(selected).map_err(ApiError::internal)?)
            {
                readable = false;
            }
        }
        if !readable {
            cache = None;
            inputs.clear();
            inspecting = target_inspecting;
        }
    }

    Ok(Checked {
        cache,
        inputs,
        inspecting,
    })
}

/// The same optional producer metadata projection for singleton and batch paragraph reads.
pub(super) async fn producer(
    conn: &mut Conn,
    principal: &Principal,
    plan: &Prepared,
    cache: Option<&rows::Cache>,
    providers_slot: usize,
) -> ApiResult<(Option<SummaryProducerView>, Vec<KeyResolvedTarget>)> {
    let mut providers = Vec::new();
    let producer = if let Some(cache) = cache.filter(|cache| cache.text.is_some()) {
        let mut view = SummaryProducerView {
            kind: "model".into(),
            title: "Model summarizer".into(),
            provider: None,
            offering: None,
        };
        let id = cache
            .producer
            .as_ref()
            .and_then(|producer| producer.pointer("/backend/offering/provider"))
            .cloned()
            .map(serde_json::from_value::<Id>)
            .transpose()
            .map_err(ApiError::internal)?;
        if let Some(id) = id
            && let Some(fact) =
                readable_provider_fact(conn, principal, plan, id, providers_slot).await?
            && let Some(row) =
                orch_db::queries::resources::thread_engine::provider(conn, id).await?
        {
            view.provider = Some(orch_provider_contract::capabilities::ProviderView {
                id,
                title: row.name,
            });
            view.offering = cache
                .producer
                .as_ref()
                .and_then(|producer| producer.pointer("/backend/offering/offering"))
                .and_then(Json::as_str)
                .map(str::to_owned);
            providers.push(fact);
        }
        Some(view)
    } else {
        None
    };

    Ok((producer, providers))
}

/// Execution identity is optional metadata; source Read never becomes helper or provider Read.
pub(super) async fn program(
    conn: &mut Conn,
    principal: &Principal,
    plan: &Prepared,
    request: Option<&rows::Request>,
    slots: [usize; 3],
    inputs: &mut Vec<KeyResolvedTarget>,
    providers: &mut Vec<KeyResolvedTarget>,
) -> ApiResult<Option<SummaryProgramView>> {
    let Some(request) = request else {
        return Ok(None);
    };
    let work: Option<(Id,Id,Id)> = sqlx::query_as(
        "SELECT helper_id,actor_id,(backend#>>'{offering,provider}')::uuid FROM delegated_program_work WHERE request_id=$1 AND purpose='summary' ORDER BY created_at DESC,id DESC LIMIT 1",
    )
    .bind(request.id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(DbError::from)?;
    let Some((helper, actor, provider)) = work else {
        return Ok(None);
    };
    let mut view = SummaryProgramView {
        helper_id: None,
        actor: None,
        provider: None,
    };
    let helper_target = SummaryTarget::Thread { id: helper };
    if let Ok(identity) = sources::identify(conn, helper_target).await
        && plan
            .permits_binding(
                principal.caller(),
                slots[1],
                identity.fact().object.kind,
                std::slice::from_ref(identity.fact()),
            )
            .map_err(source_error)?
        && sources::viewable(conn, principal, helper_target)
            .await
            .map_err(source_error)?
    {
        view.helper_id = Some(helper);
        view.actor = Some(
            serde_json::from_value(
                orch_engine::views::user_ref_json(conn, actor)
                    .await
                    .map_err(ApiError::engine)?,
            )
            .map_err(ApiError::internal)?,
        );
        if !inputs.contains(identity.fact()) {
            inputs.push(identity.fact().clone());
        }
    }
    if let Some(fact) = readable_provider_fact(conn, principal, plan, provider, slots[2]).await?
        && let Some(row) =
            orch_db::queries::resources::thread_engine::provider(conn, provider).await?
    {
        view.provider = Some(orch_provider_contract::capabilities::ProviderView {
            id: provider,
            title: row.name,
        });
        if !providers.contains(&fact) {
            providers.push(fact);
        }
    }
    Ok(Some(view))
}
