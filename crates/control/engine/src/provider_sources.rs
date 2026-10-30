//! Provider source specs keep secrets beside the credential and observations attached to the unchanged choice.

use crate::EngineError;
use orch_db::{
    Conn,
    queries::{provider_facts, resources::keeper},
};
use orch_provider_contract::{
    ProviderSources, SourceChoiceLastError, SourceDescriptor, SourceElement, SourceError,
    SourceSpec,
};
use orch_types::{Id, Json, Time};

/// The selected source, with only this element's declared secret names resolved to provider-owned references.
pub async fn spec(
    conn: &mut Conn,
    provider: Id,
    choices: &Json,
    element: SourceElement,
) -> Result<Option<SourceSpec>, EngineError> {
    let choices: ProviderSources = if choices.is_null() {
        ProviderSources::default()
    } else {
        serde_json::from_value(choices.clone())?
    };
    let Some(choice) = choices.get(element) else {
        return Ok(None);
    };
    let secrets = keeper::secret_refs(conn, provider).await?;
    let mut spec = choice.spec(element, &|name| {
        secrets
            .iter()
            .find(|secret| secret.name == name)
            .map(|secret| secret.id)
    });
    let row: Option<(String, Id)> =
        sqlx::query_as("SELECT name,home_id FROM providers WHERE id=$1 AND deleted_at IS NULL")
            .bind(provider)
            .fetch_optional(&mut *conn)
            .await
            .map_err(orch_db::DbError::from)?;
    if let Some((name, home)) = row {
        spec.provider =
            Some(orch_provider_contract::source::SourceAskProvider { id: provider, name });
        if spec.by == orch_provider_contract::SourceBy::Address {
            spec.allow_http = orch_settings::get::<bool>(
                conn,
                orch_settings::Place::Node(home),
                orch_settings_contract::registry::keys::PROVIDERS_ALLOW_HTTP_UPSTREAMS,
            )
            .await?
            .unwrap_or(false);
        }
    }
    Ok(Some(spec))
}

/// Store per-element metadata in the same transaction that accepts its correlated facts/models.
/// Source failure does not clear facts and is never a provider-state transition.
pub async fn observed(
    conn: &mut Conn,
    provider: Id,
    choices: &Json,
    element: SourceElement,
    descriptor: Option<&SourceDescriptor>,
    error: Option<&SourceError>,
    now: Time,
) -> Result<bool, EngineError> {
    let expected = choices
        .get(element.as_str())
        .filter(|value| !value.is_null())
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let descriptor = descriptor.map(serde_json::to_value).transpose()?;
    let error = error.map(|error| SourceChoiceLastError {
        code: error.code,
        message: error
            .message
            .chars()
            .take(orch_provider_contract::limits::ERROR_MESSAGE_MAX_CHARS)
            .collect(),
        at: now,
    });
    let error = error.as_ref().map(serde_json::to_value).transpose()?;
    Ok(provider_facts::source_observed(
        conn,
        provider,
        element.as_str(),
        &expected,
        descriptor.as_ref(),
        error.as_ref(),
        now.get(),
    )
    .await?)
}

/// A timeout or failed link call has no reply metadata; keep a bounded local explanation without its raw body.
pub(crate) async fn failed(
    engine: &crate::Engine,
    provider: Id,
    epoch: &Json,
    choices: &Json,
    element: SourceElement,
    code: orch_provider_contract::SourceErrorCode,
    message: &str,
) -> Result<(), EngineError> {
    let mut tx = orch_db::queries::transaction_guard::scope(true, engine.db().begin()).await?;
    if !provider_facts::lock_provider(&mut tx, provider).await?
        || provider_facts::credential_epoch(&mut tx, provider, None)
            .await?
            .as_ref()
            != Some(epoch)
    {
        return Ok(());
    }
    let error = SourceError::new(code, message);
    observed(
        &mut tx,
        provider,
        choices,
        element,
        None,
        Some(&error),
        Time::new(engine.inner.clock.now()),
    )
    .await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}
