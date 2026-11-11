//! Guarded stable port targets, reviewed wider admissions and safe outside listings.

pub use crate::middleware::authorization;

pub use crate::middleware::requests::ApiError;

pub use crate::middleware::requests::ApiResult;

pub use crate::registry::Ctx;

pub use orch_access_contract::permissions::Permission;

pub use orch_api_contract::common::Page;

use orch_api_contract::ports::*;

pub use orch_authz::Policy;

pub use orch_authz::Principal;

pub use orch_db::Conn;

pub use orch_db::queries::environments::routes::EnvironmentFull;

pub use orch_db::queries::ports as rows;

pub use orch_engine::ports as core;

pub use orch_types::Id;

pub use orch_types::Json;

pub use std::collections::BTreeSet;
orch_api_base::api_family_callback! {
    bind __api_bind_aa4ff1ee37, __api_install_aa4ff1ee37, __api_callback_aa4ff1ee37;
    slot __ApiCallbackaa4ff1ee37, __API_CALLBACK_AA4FF1EE37, __api_set_aa4ff1ee37;
    base orch_api_base::routes::ports;

    fn mount<'__api>(arg0: &'__api mut orch_api_base::registry::Registry) -> ();
}

pub fn parse<T: serde::de::DeserializeOwned>(value: &str) -> ApiResult<T> {
    serde_json::from_value(Json::String(value.into()))
        .map_err(|_| ApiError::internal("a stored port value cannot be read"))
}

orch_api_base::api_family_callback! {
    bind __api_bind_3a27b86d0a, __api_install_3a27b86d0a, __api_callback_3a27b86d0a;
    slot __ApiCallback3a27b86d0a, __API_CALLBACK_3A27B86D0A, __api_set_3a27b86d0a;
    base orch_api_base::routes::ports;

    fn limit(arg0: Option<u32>) -> usize;
}

pub fn cursor<T: std::str::FromStr>(value: Option<&str>) -> ApiResult<Option<T>> {
    value
        .map(|v| {
            v.parse()
                .map_err(|_| ApiError::invalid("cursor", "the continuation is invalid"))
        })
        .transpose()
}

orch_api_base::api_family_callback! {
    bind __api_bind_649f5bdf2b, __api_install_649f5bdf2b, __api_callback_649f5bdf2b;
    slot __ApiCallback649f5bdf2b, __API_CALLBACK_649F5BDF2B, __api_set_649f5bdf2b;
    base orch_api_base::routes::ports;

    async fn environment<'__api>(arg0: &'__api mut orch_db::Conn, arg1: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<orch_db::queries::environments::routes::EnvironmentFull>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_38c34fdc8f, __api_install_38c34fdc8f, __api_callback_38c34fdc8f;
    slot __ApiCallback38c34fdc8f, __API_CALLBACK_38C34FDC8F, __api_set_38c34fdc8f;
    base orch_api_base::routes::ports;

    async fn exposures<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api [orch_types::Id], arg2: orch_types::Time) -> orch_api_base::middleware::requests::ApiResult<std::collections::BTreeSet<orch_types::Id>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_9bf1cd7659, __api_install_9bf1cd7659, __api_callback_9bf1cd7659;
    slot __ApiCallback9bf1cd7659, __API_CALLBACK_9BF1CD7659, __api_set_9bf1cd7659;
    base orch_api_base::routes::ports;

    async fn require<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: orch_types::Id, arg3: orch_access_contract::permissions::Permission) -> orch_api_base::middleware::requests::ApiResult<()>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_3dde7fe764, __api_install_3dde7fe764, __api_callback_3dde7fe764;
    slot __ApiCallback3dde7fe764, __API_CALLBACK_3DDE7FE764, __api_set_3dde7fe764;
    base orch_api_base::routes::ports;

    fn address<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: orch_types::Id, arg2: u16) -> Option<String>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_9473b2289c, __api_install_9473b2289c, __api_callback_9473b2289c;
    slot __ApiCallback9473b2289c, __API_CALLBACK_9473B2289C, __api_set_9473b2289c;
    base orch_api_base::routes::ports;

    async fn opening_view<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: &'__api orch_db::queries::ports::Opening, arg4: &'__api orch_db::queries::environments::routes::EnvironmentFull, arg5: bool) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::ports::PortOpening>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_31c1d582af, __api_install_31c1d582af, __api_callback_31c1d582af;
    slot __ApiCallback31c1d582af, __API_CALLBACK_31C1D582AF, __api_set_31c1d582af;
    base orch_api_base::routes::ports;

    async fn effective_opening_state<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: &'__api orch_db::queries::ports::Opening, arg4: &'__api orch_db::queries::environments::routes::EnvironmentFull, arg5: orch_api_contract::ports::PortOpeningKind, arg6: u16) -> orch_api_base::middleware::requests::ApiResult<(orch_api_contract::ports::PortOpeningState, Option<String>)>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_3d55c330f8, __api_install_3d55c330f8, __api_callback_3d55c330f8;
    slot __ApiCallback3d55c330f8, __API_CALLBACK_3D55C330F8, __api_set_3d55c330f8;
    base orch_api_base::routes::ports;

    async fn target<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: &'__api orch_db::queries::environments::routes::EnvironmentFull, arg4: u16) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::ports::PortTarget>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_b73cd65628, __api_install_b73cd65628, __api_callback_b73cd65628;
    slot __ApiCallbackb73cd65628, __API_CALLBACK_B73CD65628, __api_set_b73cd65628;
    base orch_api_base::routes::ports;

    async fn list(arg0: orch_api_base::registry::Ctx, arg1: orch_api_contract::ports::EnvironmentPortListQuery, arg2: ()) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::ports::EnvironmentPorts>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_f62f18bc58, __api_install_f62f18bc58, __api_callback_f62f18bc58;
    slot __ApiCallbackf62f18bc58, __API_CALLBACK_F62F18BC58, __api_set_f62f18bc58;
    base orch_api_base::routes::ports;

    async fn list_view<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: &'__api orch_db::queries::environments::routes::EnvironmentFull, arg4: orch_api_contract::ports::EnvironmentPortListQuery) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::ports::EnvironmentPorts>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_d4842200b4, __api_install_d4842200b4, __api_callback_d4842200b4;
    slot __ApiCallbackd4842200b4, __API_CALLBACK_D4842200B4, __api_set_d4842200b4;
    base orch_api_base::routes::ports;

    async fn read_opening_in<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::ports::PortOpening>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_b89f1ee526, __api_install_b89f1ee526, __api_callback_b89f1ee526;
    slot __ApiCallbackb89f1ee526, __API_CALLBACK_B89F1EE526, __api_set_b89f1ee526;
    base orch_api_base::routes::ports;

    async fn may_use<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api orch_db::queries::environments::routes::EnvironmentFull, arg3: u16, arg4: orch_api_contract::ports::PortTransport, arg5: Option<orch_types::Id>) -> orch_api_base::middleware::requests::ApiResult<()>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_a8fb13606b, __api_install_a8fb13606b, __api_callback_a8fb13606b;
    slot __ApiCallbacka8fb13606b, __API_CALLBACK_A8FB13606B, __api_set_a8fb13606b;
    base orch_api_base::routes::ports;

    async fn may_use_link<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_db::queries::environments::routes::EnvironmentFull, arg2: &'__api orch_db::queries::ports::Opening, arg3: orch_api_contract::ports::PortTransport, arg4: orch_types::Time) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::ports::PortCaller>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_1691768fc2, __api_install_1691768fc2, __api_callback_1691768fc2;
    slot __ApiCallback1691768fc2, __API_CALLBACK_1691768FC2, __api_set_1691768fc2;
    base orch_api_base::routes::ports;

    fn proof_matches<'__api>(arg0: &'__api orch_db::queries::ports::Opening, arg1: &'__api str) -> bool;
}

orch_api_base::api_family_callback! {
    bind __api_bind_aef3097442, __api_install_aef3097442, __api_callback_aef3097442;
    slot __ApiCallbackaef3097442, __API_CALLBACK_AEF3097442, __api_set_aef3097442;
    base orch_api_base::routes::ports;

    async fn connections(arg0: orch_api_base::registry::Ctx, arg1: orch_api_contract::ports::EnvironmentPortConnectionListQuery, arg2: ()) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::ports::PortConnectionList>;
}

/// The overview and standalone endpoint use one native administered-place window.
pub async fn outside_in(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    query: NodePortOpeningListQuery,
    reading: &crate::routes::nodes::overview::Reading<'_>,
) -> ApiResult<Page<PortOpening>> {
    outside_operation::read_for_overview(ctx, conn, principal, query, reading).await
}

#[path = "ports/opening_operation.rs"]
pub mod opening_operation;

#[path = "ports/outside_operation.rs"]
pub mod outside_operation;
