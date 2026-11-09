//! One native composing-context check for all existing grant owners.

use crate::middleware::requests::{ApiError, ApiResult};
use orch_access_contract::audit::LinkOrigin;
use orch_api_contract::common::ErrorCode;
use orch_authz::Principal;
use orch_db::Conn;

pub(crate) async fn check(
    conn: &mut Conn,
    principal: &Principal,
    source: Option<LinkOrigin>,
) -> ApiResult<()> {
    orch_engine::link_origin::check(conn, principal, source)
        .await
        .map_err(|error| match error {
            orch_engine::EngineError::BeforeEffect(_) => ApiError::new(
                ErrorCode::AuthzDenied,
                "the sender cannot compose in this link's source context",
            ),
            error => ApiError::engine(error),
        })
}
