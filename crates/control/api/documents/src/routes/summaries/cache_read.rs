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
        if let Some(checked) = guard_refs(
            conn,
            principal,
            target,
            plan,
            &refs,
            inputs_slot,
            target_inspecting,
        )
        .await?
        {
            inputs = checked.inputs;
            inspecting = checked.inspecting;
        } else {
            readable = false;
        }
        if !same_range(
            conn,
            principal,
            target,
            wanted,
            kept.source_snapshot.as_ref(),
        )
        .await?
        {
            readable = false;
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

pub(super) struct CheckedRefs {
    pub inputs: Vec<KeyResolvedTarget>,
    pub inspecting: bool,
}

/// Both a published paragraph and a later attempt keep their own complete source governors.
pub(super) async fn guard_refs(
    conn: &mut Conn,
    principal: &Principal,
    target: SummaryTarget,
    plan: &Prepared,
    refs: &[SummaryTarget],
    inputs_slot: usize,
    mut inspecting: bool,
) -> ApiResult<Option<CheckedRefs>> {
    let mut inputs = Vec::new();
    for &source in refs {
        let current = match sources::identify(conn, source).await {
            Ok(current) => current,
            Err(_) => return Ok(None),
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
            return Ok(None);
        }
        if source != target {
            inputs.push(current.fact().clone());
        }
        inspecting |= !sources::readable(conn, principal, source)
            .await
            .map_err(source_error)?;
    }
    Ok(Some(CheckedRefs { inputs, inspecting }))
}

/// A private chat receipt describes only its originally selected range.
pub(super) async fn same_range(
    conn: &mut Conn,
    principal: &Principal,
    target: SummaryTarget,
    wanted: Option<SummaryChatRange>,
    snapshot: Option<&Json>,
) -> ApiResult<bool> {
    if !matches!(target, SummaryTarget::Chat { .. }) {
        return Ok(true);
    }
    let selected = match wanted {
        Some(range) => Some(range),
        None => Some(
            sources::disclosure_range(conn, principal, target.id())
                .await
                .map_err(source_error)?,
        ),
    };
    Ok(snapshot.and_then(|source| source.get("range"))
        == Some(&serde_json::to_value(selected).map_err(ApiError::internal)?))
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
        // An admitted model name is a result fact; its account/configuration still needs Read.
        view.offering = cache
            .producer
            .as_ref()
            .and_then(|producer| producer.get("model_name"))
            .and_then(Json::as_str)
            .map(str::to_owned);
        if let Some(name) = cache
            .producer
            .as_ref()
            .and_then(|producer| producer.get("model_display"))
            .and_then(Json::as_str)
            .or(view.offering.as_deref())
        {
            view.title = name.to_owned();
        }
        let id = cache
            .producer
            .as_ref()
            .and_then(|producer| producer.get("provider_id"))
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
            providers.push(fact);
        }
        project_model_names(principal, providers.first(), &mut view);
        Some(view)
    } else {
        None
    };

    Ok((producer, providers))
}
