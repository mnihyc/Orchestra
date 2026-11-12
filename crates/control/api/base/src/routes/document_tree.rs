//! Typed document-folder metadata, child lists and atomic replay-safe organization.

use crate::middleware::authorization::refusals;

pub use crate::middleware::requests::ApiError;

pub use crate::middleware::requests::ApiResult;

pub use orch_api_contract::common::ErrorCode;

pub use orch_db::Conn;

pub use orch_engine::document_tree as domain;

use serde::Serialize;

use serde::de::DeserializeOwned;
orch_api_base::api_family_callback! {
    bind __api_bind_f1635129b9, __api_install_f1635129b9, __api_callback_f1635129b9;
    slot __ApiCallbackf1635129b9, __API_CALLBACK_F1635129B9, __api_set_f1635129b9;
    base orch_api_base::routes::document_tree;

    fn mount<'__api>(arg0: &'__api mut orch_api_base::registry::Registry) -> ();
}

pub fn convert<T: Serialize, U: DeserializeOwned>(value: T) -> ApiResult<U> {
    serde_json::from_value(serde_json::to_value(value).map_err(ApiError::internal)?)
        .map_err(ApiError::internal)
}

orch_api_base::api_family_callback! {
    bind __api_bind_b705bab7c6, __api_install_b705bab7c6, __api_callback_b705bab7c6;
    slot __ApiCallbackb705bab7c6, __API_CALLBACK_B705BAB7C6, __api_set_b705bab7c6;
    base orch_api_base::routes::document_tree;

    async fn container_refs<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api [orch_types::Id]) -> orch_api_base::middleware::requests::ApiResult<std::collections::HashMap<orch_types::Id, orch_api_contract::document_tree::DocumentContainerRef>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_d61ba637ac, __api_install_d61ba637ac, __api_callback_d61ba637ac;
    slot __ApiCallbackd61ba637ac, __API_CALLBACK_D61BA637AC, __api_set_d61ba637ac;
    base orch_api_base::routes::document_tree;

    fn options<'__api>(arg0: &'__api orch_api_base::registry::Ctx) -> orch_engine::document_tree::Options;
}

pub async fn result<T>(conn: &mut Conn, value: Result<T, domain::Error>) -> ApiResult<T> {
    match value {
        Ok(value) => Ok(value),
        Err(domain::Error::Invalid(message)) => {
            Err(ApiError::new(ErrorCode::RequestInvalid, message))
        }
        Err(domain::Error::NotFound) => Err(ApiError::not_found()),
        Err(domain::Error::Denied) => Err(ApiError::new(
            ErrorCode::AuthzDenied,
            "the complete document operation is not permitted",
        )),
        Err(domain::Error::Conflict) => Err(ApiError::new(
            ErrorCode::ConflictExists,
            "a document entry conflicts with this operation",
        )),
        Err(domain::Error::Stale) => Err(ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "the document tree changed; obtain a new deletion preview",
        )),
        Err(domain::Error::Limit) => Err(ApiError::new(
            ErrorCode::DocsTooLarge,
            "the complete document operation exceeds its limit",
        )),
        Err(domain::Error::Authorization(error)) => Err(refusals::problem(conn, error).await),
        Err(error) => Err(ApiError::internal(error)),
    }
}

#[path = "document_tree/metadata.rs"]
pub mod metadata;
