//! Kept environment metadata and guarded restore or permanent removal requests.

pub use crate::middleware::requests::ApiError;

pub use crate::middleware::requests::ApiResult;

pub use crate::registry::Ctx;

pub use orch_api_contract::environment as api;

use orch_api_contract::nodes::DeletedInspection;

pub use orch_authz::Principal;

pub use orch_db::Conn;

pub use orch_types::Id;

pub use orch_types::Time;
orch_api_base::api_family_callback! {
    bind __api_bind_069be0bc4b, __api_install_069be0bc4b, __api_callback_069be0bc4b;
    slot __ApiCallback069be0bc4b, __API_CALLBACK_069BE0BC4B, __api_set_069be0bc4b;
    base orch_api_base::routes::environments::retained;

    async fn may_read<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<(bool, bool)>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_c4a5a90a7f, __api_install_c4a5a90a7f, __api_callback_c4a5a90a7f;
    slot __ApiCallbackc4a5a90a7f, __API_CALLBACK_C4A5A90A7F, __api_set_c4a5a90a7f;
    base orch_api_base::routes::environments::retained;

    async fn present<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<Option<orch_api_contract::environment::EnvironmentRemoval>>;
}

/// The native retained view's stored operands, before a transport body is rendered.
pub struct PreparedRead {
    pub id: Id,
    pub home: Id,
    pub projection: super::PreparedInventory,
    pub deleted_at: Option<chrono::DateTime<chrono::Utc>>,
    pub authority: (bool, bool),
}

impl PreparedRead {
    pub async fn render(
        &self,
        ctx: &Ctx,
        conn: &mut Conn,
        principal: &Principal,
    ) -> ApiResult<api::Environment> {
        let mut shown = super::show_inventory_prepared(ctx, conn, principal, &self.projection)
            .await?
            .pop()
            .ok_or_else(ApiError::not_found)?;
        if let Some(deleted) = self.deleted_at {
            let removal = shown
                .removal
                .as_ref()
                .ok_or_else(|| ApiError::internal("the kept data has no removal metadata"))?;
            shown.inspection = Some(DeletedInspection {
                deleted_at: Time::new(deleted),
                deleted_by: removal.deleted_by.clone(),
                purge_at: removal
                    .purge_at
                    .ok_or_else(|| ApiError::internal("the kept data has no purge day"))?,
                readonly: true,
            });
        }
        Ok(shown)
    }
}

orch_api_base::api_family_callback! {
    bind __api_bind_e0b0aea787, __api_install_e0b0aea787, __api_callback_e0b0aea787;
    slot __ApiCallbacke0b0aea787, __API_CALLBACK_E0B0AEA787, __api_set_e0b0aea787;
    base orch_api_base::routes::environments::retained;

    fn identity<'__api>(arg0: &'__api orch_api_base::registry::Ctx) -> orch_api_base::middleware::requests::ApiResult<orch_types::Json>;
}
