//! One normal setting resolution for a provider's translated model requests.

use crate::{Error, Place, Snapshot, Wanted};
use orch_db::Conn;
use orch_provider_contract::models::TranslationOptions;
use orch_settings_contract::registry::{get, keys};

/// Resolve once when provider.start is captured; readers do not start a runtime.
pub async fn load(conn: &mut Conn, place: Place) -> Result<TranslationOptions, Error> {
    let allowed = get(keys::PROVIDERS_TRANSLATION)
        .ok_or_else(|| Error::UnknownKey(keys::PROVIDERS_TRANSLATION.into()))?;
    let output = get(keys::PROVIDERS_TRANSLATION_OUTPUT_TOKENS)
        .ok_or_else(|| Error::UnknownKey(keys::PROVIDERS_TRANSLATION_OUTPUT_TOKENS.into()))?;
    let snapshot = Snapshot::load(conn, place, Wanted::Keys(&[allowed, output])).await?;
    Ok(TranslationOptions {
        allowed: snapshot
            .resolve(allowed)
            .value
            .and_then(|value| value.as_bool())
            .ok_or(Error::Unreadable {
                what: "translation policy",
            })?,
        output_tokens: snapshot
            .resolve(output)
            .value
            .and_then(|value| value.as_i64())
            .filter(|value| *value > 0)
            .ok_or(Error::Unreadable {
                what: "translation output default",
            })?,
    })
}
