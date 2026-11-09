//! Copy source choices as definitions; observed descriptors, errors and earlier consent stay behind.
use super::*;
use orch_api_contract::resources::{NewProviderSources, SourceChoiceSet};
use orch_provider_contract::{ProviderSources, SourceChoice, SourceElement};

pub(super) async fn sources(
    conn: &mut Conn,
    provider: Id,
    value: &orch_types::Json,
    owner: bool,
) -> ApiResult<Option<NewProviderSources>> {
    let old: ProviderSources = serde_json::from_value(value.clone()).map_err(ApiError::internal)?;
    let mut fields = [None, None, None];
    for (index, element) in SourceElement::ALL.into_iter().enumerate() {
        let choice = match old.get(element) {
            Some(choice) => Some(copy_source(conn, provider, element, choice, owner).await?),
            None => None,
        };
        fields[index] = choice;
    }
    let [facts, models, presenter] = fields;
    if facts.is_none() && models.is_none() && presenter.is_none() {
        return Ok(None);
    }
    Ok(Some(NewProviderSources {
        facts,
        models,
        presenter,
    }))
}
async fn copy_source(
    conn: &mut Conn,
    provider: Id,
    element: SourceElement,
    choice: &SourceChoice,
    owner: bool,
) -> ApiResult<SourceChoiceSet> {
    let mut selected: BTreeSet<String> = choice
        .secrets
        .iter()
        .map(|name| element.field_secret(name))
        .collect();
    selected.insert(element.authorization_secret());
    let mut values = if owner {
        duplicates::secret_values(conn, "provider", provider, &selected).await?
    } else {
        BTreeMap::new()
    };
    let secrets = choice
        .secrets
        .iter()
        .filter_map(|name| {
            values
                .remove(&element.field_secret(name))
                .map(|value| (name.clone(), Some(value)))
        })
        .collect();
    let authorization = values.remove(&element.authorization_secret()).map(Some);
    let address = choice.address.clone().map(|mut address| {
        address.send_credential = false;
        address
    });
    Ok(SourceChoiceSet {
        by: choice.by,
        name: choice.name.clone(),
        address,
        config: choice.config.clone(),
        secrets,
        authorization,
    })
}

pub(super) async fn write_group_options(
    ctx: &Ctx,
    conn: &mut Conn,
    provider: Id,
    options: &orch_types::Json,
    service_offerings: &orch_types::Json,
) -> ApiResult<()> {
    if options
        .as_object()
        .is_some_and(|options| options.is_empty())
        && service_offerings
            .as_object()
            .is_some_and(|choices| choices.is_empty())
    {
        return Ok(());
    }
    let row = rows::providers(conn, &[provider])
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    if !rows::change_provider(
        conn,
        provider,
        row.revision,
        &rows::ProviderChange {
            model_group_options: Some(options),
            service_offerings: Some(service_offerings),
            ..Default::default()
        },
        ctx.now,
    )
    .await?
    {
        return Err(ApiError::new(
            orch_api_contract::common::ErrorCode::ConflictStaleRevision,
            "the new provider changed before its offering configuration was copied",
        ));
    }
    resources::record(
        ctx,
        conn,
        orch_db::audit::action!(orch_access_contract::audit::AuditAction::ProviderUpdated),
        provider,
        resources::Home {
            node: Some(row.home_id),
            org: Some(row.org_id),
        },
        "The copied provider received its readable offering configuration",
        serde_json::json!({"model_group_options":true,"service_offerings":true}),
    )
    .await
}

/// Runtime settings are readable configuration; a borrowed credential home is never a new home.
pub(super) async fn runtime(
    conn: &mut Conn,
    provider: Id,
    value: &orch_types::Json,
    owner: bool,
) -> ApiResult<(orch_types::Json, BTreeMap<String, String>)> {
    let mut config = value
        .as_object()
        .cloned()
        .ok_or_else(|| ApiError::internal("the native runtime configuration is not an object"))?;
    config.remove(orch_settings_contract::registry::keys::RUNTIME_HOME);
    let names = resources::providers::runtime_secret_names(conn, provider)
        .await?
        .into_iter()
        .map(|name| format!("runtime:{name}"))
        .collect();
    let values = if owner {
        duplicates::secret_values(conn, "provider", provider, &names).await?
    } else {
        BTreeMap::new()
    };
    Ok((orch_types::Json::Object(config), values))
}
