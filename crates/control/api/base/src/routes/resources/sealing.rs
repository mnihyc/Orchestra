//! The key that seals what a request brings, and sealing a provider's key and secret headers with it before they are stored.
//!
//! A secret is sealed as soon as the request that carries it is read and is stored only sealed, bound to its
//! own row, so that a sealed value copied to another provider or another name does not open there. No request
//! returns one: the rows that list what a provider has give names, never values.

pub use crate::middleware::requests::ApiError;

pub use crate::middleware::requests::ApiResult;

pub use orch_db::secrets::MasterKey;

use std::sync::OnceLock;
/// Where a deployment keeps the master key that seals secrets, when no key was installed: the same
/// file that signing in reads.
pub const MASTER_KEY_VARIABLE: &str = "ORCH_MASTER_KEY_FILE";

/// The path the deployment's container runtime puts that key at.
pub const MASTER_KEY_PATH: &str = "/run/secrets/orch_master_key";

pub static MASTER_KEY: OnceLock<MasterKey> = OnceLock::new();

orch_api_base::api_family_callback! {
    bind __api_bind_2238e563fe, __api_install_2238e563fe, __api_callback_2238e563fe;
    slot __ApiCallback2238e563fe, __API_CALLBACK_2238E563FE, __api_set_2238e563fe;
    base orch_api_base::routes::resources::sealing;

    fn install_master_key(arg0: orch_db::secrets::MasterKey) -> bool;
}

/// Composition installs the deployment key before serving reads. Metadata projections may run
/// inside a snapshot transaction, so they must never fall back to reading a key file here.
pub fn installed_master_key() -> ApiResult<&'static MasterKey> {
    MASTER_KEY.get().ok_or_else(|| {
        ApiError::internal("the deployment key was not installed before serving metadata")
    })
}

/// The master key: the one that was given, or else the one in the file the deployment provides.
pub fn master_key() -> ApiResult<&'static MasterKey> {
    if let Some(key) = MASTER_KEY.get() {
        return Ok(key);
    }
    let path = std::env::var(MASTER_KEY_VARIABLE).unwrap_or_else(|_| MASTER_KEY_PATH.to_owned());
    let bytes = orch_config::read_secret_hex::<32>(std::path::Path::new(&path))
        .map_err(|error| ApiError::internal(format!("the master key cannot be read: {error}")))?;
    Ok(MASTER_KEY.get_or_init(|| MasterKey::new(bytes)))
}

orch_api_base::api_family_callback! {
    bind __api_bind_f53aec78ac, __api_install_f53aec78ac, __api_callback_f53aec78ac;
    slot __ApiCallbackf53aec78ac, __API_CALLBACK_F53AEC78AC, __api_set_f53aec78ac;
    base orch_api_base::routes::resources::sealing;

    fn binding<'__api>(arg0: &'__api str, arg1: orch_types::Id, arg2: &'__api str) -> Vec<u8>;
}
