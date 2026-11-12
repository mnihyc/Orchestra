//! One ordinary MCP projection for collections, individual reads and existing writers.

use crate::routes::access::page::NameSources;

use crate::routes::resources::view::Holding;

use crate::routes::resources::view::PreparedAccess;

use orch_access_contract::decisions::Held;

pub use orch_access_contract::permissions::Permission;

use orch_access_contract::roles::AdminReach;

use orch_access_contract::roles::PlaceRef;

pub use orch_db::queries::resources::mcp as rows;

pub use orch_db::queries::resources::mcp::McpRow;

pub use orch_types::Id;

pub use std::collections::HashMap;
#[derive(Clone, Copy)]
pub enum Style {
    /// The original collection/preset Get projection masks machine-backed preset definitions.
    Presets,
    /// The original `mcp::show` projection also serves an environment's own definition.
    Individual,
}

pub struct Access {
    pub id: Id,
    pub admin_only: Vec<Permission>,
    pub admin_access: Option<AdminReach>,
    pub admin_place: Option<PlaceRef>,
    pub participating: bool,
    pub owner: bool,
    pub administers: bool,
    pub held: Vec<Held>,
}

impl Access {
    pub fn holding(&self) -> Holding<'_> {
        Holding {
            id: self.id,
            admin_only: &self.admin_only,
            admin_access: self.admin_access,
            admin_place: self.admin_place,
            participating: self.participating,
            owner: self.owner,
            administers: self.administers,
            held: &self.held,
        }
    }
}

pub struct Prepared {
    pub context: Option<Id>,
    pub rows: Vec<McpRow>,
    pub access: Vec<Access>,
    pub access_rows: PreparedAccess,
    pub subjects: HashMap<Id, Id>,
    pub offerings: Vec<rows::Offering>,
    pub style: Style,
    // Only the current ordinary preset selection supplies a proven-false Manage candidate.
    // Individual show and direct Get preserve the original unconditional native check.
    pub from_selection: bool,
    pub machines: Vec<Id>,
    pub environments: Vec<Id>,
    pub names: NameSources,
}

orch_api_base::api_family_callback! {
    bind __api_bind_86dad5b667, __api_install_86dad5b667, __api_callback_86dad5b667;
    slot __ApiCallback86dad5b667, __API_CALLBACK_86DAD5B667, __api_set_86dad5b667;
    base orch_api_base::routes::resources::mcp::projection;

    fn subject<'__api>(arg0: &'__api orch_db::queries::resources::mcp::McpRow) -> orch_access_contract::decisions::ObjectRef;
}

orch_api_base::api_family_callback! {
    bind __api_bind_c00043a98d, __api_install_c00043a98d, __api_callback_c00043a98d;
    slot __ApiCallbackc00043a98d, __API_CALLBACK_C00043A98D, __api_set_c00043a98d;
    base orch_api_base::routes::resources::mcp::projection;

    async fn prepare<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api [orch_types::Id], arg3: Option<&'__api [orch_authz::resources::Seen]>, arg4: orch_api_base::routes::resources::mcp::projection::Style) -> orch_api_base::middleware::requests::ApiResult<orch_api_base::routes::resources::mcp::projection::Prepared>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_b71d1e0316, __api_install_b71d1e0316, __api_callback_b71d1e0316;
    slot __ApiCallbackb71d1e0316, __API_CALLBACK_B71D1E0316, __api_set_b71d1e0316;
    base orch_api_base::routes::resources::mcp::projection;

    async fn render<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: &'__api orch_api_base::routes::resources::mcp::projection::Prepared, arg4: Option<&'__api orch_api_base::routes::access::page::NameBounds>) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_api_contract::resources::McpServer>>;
}
