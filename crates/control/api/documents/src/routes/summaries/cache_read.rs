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
                    1,
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
            .and_then(|producer| producer.get("provider_id"))
            .cloned()
            .map(serde_json::from_value::<Id>)
            .transpose()
            .map_err(ApiError::internal)?;
        if let Some(id) = id
            && let Some(fact) = readable_provider_fact(conn, principal, plan, id).await?
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
                .and_then(|producer| producer.get("model_name"))
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
