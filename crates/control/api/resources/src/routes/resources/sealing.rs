//! The key that seals what a request brings, and sealing a provider's key and secret headers with it before they are stored.
//!
//! A secret is sealed as soon as the request that carries it is read and is stored only sealed, bound to its
//! own row, so that a sealed value copied to another provider or another name does not open there. No request
//! returns one: the rows that list what a provider has give names, never values.

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use orch_db::Conn;

use orch_db::queries::settings as stored;

use orch_db::secrets::MasterKey;

use orch_types::Id;

pub use orch_api_base::routes::resources::sealing::MASTER_KEY_VARIABLE;

pub use orch_api_base::routes::resources::sealing::MASTER_KEY_PATH;

pub use orch_api_base::routes::resources::sealing::MASTER_KEY;

pub use orch_api_base::routes::resources::sealing::installed_master_key;

pub use orch_api_base::routes::resources::sealing::master_key;
/// The most bytes of a key or a secret header: the settings crate's limit for any secret.
pub const SECRET_MAX_BYTES: usize = orch_settings::secrets::SECRET_MAX_BYTES;

/// The owner kind under which the secrets of a provider are stored.
pub const PROVIDER: &str = "provider";

/// The owner kind under which a skill keeps the private part of a source URL.
pub const SKILL: &str = "skill";

/// The name of a provider's key.
pub const API_KEY: &str = "api_key";

/// What the name of a secret header begins with.
pub const HEADER_PREFIX: &str = "header:";

/// Gives the key that secrets are sealed under, for composition and for tests. The first key given
/// stays.
pub fn install_master_key(key: MasterKey) -> bool {
    MASTER_KEY.set(key).is_ok()
}
orch_api_base::__api_bind_2238e563fe!();

/// What a secret is bound to when it is sealed: its owner and its name, as every secret of the system.
#[must_use]
pub fn binding(kind: &str, owner: Id, name: &str) -> Vec<u8> {
    format!("secrets/{kind}/{owner}/{name}").into_bytes()
}
orch_api_base::__api_bind_f53aec78ac!();

/// The name under which a provider's secret header is stored.
#[must_use]
pub fn header_secret(name: &str) -> String {
    format!("{HEADER_PREFIX}{name}")
}

/// Seals `value` for the secret `name` of the provider `provider` and stores it, replacing the value
/// there is. Returns whether it replaced one.
pub async fn put_provider_secret(
    conn: &mut Conn,
    provider: Id,
    name: &str,
    value: &str,
    by: Id,
    at: chrono::DateTime<chrono::Utc>,
) -> ApiResult<bool> {
    let sealed = master_key()?
        .seal(value.as_bytes(), &binding(PROVIDER, provider, name))
        .map_err(|_| ApiError::internal("a secret could not be sealed"))?;
    let stored = stored::put_secret(conn, PROVIDER, provider, name, &sealed, Some(by), at).await?;
    Ok(stored.rotated_at.is_some())
}

/// Stores a skill source URL only as a sealed value bound to the skill and this source's name.
/// Its identifier, never the URL, is what a durable fetch command carries.
pub async fn put_skill_source_secret(
    conn: &mut Conn,
    skill: Id,
    name: &str,
    url: &str,
    by: Option<Id>,
    at: chrono::DateTime<chrono::Utc>,
) -> ApiResult<stored::SecretName> {
    let sealed = master_key()?
        .seal(url.as_bytes(), &binding(SKILL, skill, name))
        .map_err(|_| ApiError::internal("a skill source could not be sealed"))?;
    Ok(stored::put_secret(conn, SKILL, skill, name, &sealed, by, at).await?)
}
