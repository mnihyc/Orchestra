//! Settings of a node, an environment, a provider and the system; secrets metadata and parameters.
//!
//! The registry says which keys there are and where each is written; the
//! settings resolver says what holds at a node and why; the access engine
//! says who may write. Writing a key asks the engine the question the key's
//! class and the scope it is written at give (`validation::needs`), and the
//! resolver then writes the value, or takes it back, with its audit row in
//! the same transaction. A secret is written once and never read back: only
//! its name is listed. What belongs to an environment, a provider or a
//! thread's parameters is read by the routes of those, and is not bound here.

pub use crate::middleware::requests::ApiError;

pub use crate::middleware::requests::ApiResult;

pub use crate::routes::nodes::placed;

pub use crate::routes::nodes::policy_at;

pub use orch_access_contract::permissions::Permission;

pub use orch_api_contract::nodes::NodeKind;

pub use orch_api_contract::settings as api;

pub use orch_authz::Policy;

pub use orch_authz::Principal;

pub use orch_authz::nodes;

pub use orch_db::Conn;

pub use orch_settings::validation::Need;

pub use orch_types::Id;

use std::collections::HashSet;
orch_api_base::api_family_callback! {
    bind __api_bind_03d340b82b, __api_install_03d340b82b, __api_callback_03d340b82b;
    slot __ApiCallback03d340b82b, __API_CALLBACK_03D340B82B, __api_set_03d340b82b;
    base orch_api_base::routes::settings;

    fn install_master_key(arg0: orch_db::secrets::MasterKey) -> bool;
}

orch_api_base::api_family_callback! {
    bind __api_bind_d9260d2301, __api_install_d9260d2301, __api_callback_d9260d2301;
    slot __ApiCallbackd9260d2301, __API_CALLBACK_D9260D2301, __api_set_d9260d2301;
    base orch_api_base::routes::settings;

    fn mount<'__api>(arg0: &'__api mut orch_api_base::registry::Registry) -> ();
}

orch_api_base::api_family_callback! {
    bind __api_bind_ddd9a09986, __api_install_ddd9a09986, __api_callback_ddd9a09986;
    slot __ApiCallbackddd9a09986, __API_CALLBACK_DDD9A09986, __api_set_ddd9a09986;
    base orch_api_base::routes::settings;

    fn registry_definitions() -> orch_api_base::middleware::requests::ApiResult<Vec<orch_api_contract::settings::SettingDef>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_84368f51ca, __api_install_84368f51ca, __api_callback_84368f51ca;
    slot __ApiCallback84368f51ca, __API_CALLBACK_84368F51CA, __api_set_84368f51ca;
    base orch_api_base::routes::settings;

    fn scope_of(arg0: orch_api_contract::nodes::NodeKind) -> orch_settings_contract::registry::Scope;
}

/// What the caller holds on the node, once: whether a key can be written by
/// them is read from it.
pub struct Holds {
    pub permissions: HashSet<Permission>,
    pub system_admin: bool,
    /// For a thread that is in an environment: that environment, and whether
    /// the caller manages it. Where a thread starts and how far its commands
    /// are confined are its environment's to decide, not the thread's.
    pub environment: Option<(Id, bool)>,
}

impl Holds {
    pub async fn of(
        conn: &mut Conn,
        principal: &Principal,
        node: Id,
        kind: NodeKind,
        policy: Policy,
    ) -> ApiResult<Self> {
        let held = nodes::effective(conn, principal, node, policy)
            .await
            .map_err(ApiError::internal)?;
        let environment = match kind {
            NodeKind::Thread => {
                match orch_db::queries::settings::thread_environment(conn, node).await? {
                    Some(found) => Some((
                        found.environment_id,
                        manages_environment(conn, principal, found.environment_id).await,
                    )),
                    None => None,
                }
            }
            NodeKind::Org | NodeKind::Project | NodeKind::Topic => None,
        };
        Ok(Self {
            permissions: held.held.iter().map(|held| held.permission).collect(),
            system_admin: principal.is_system_admin(),
            environment,
        })
    }

    /// The environment the thread asked about is in, where it is in one.
    pub fn thread_environment(&self) -> Option<Id> {
        self.environment.map(|(environment, _)| environment)
    }

    pub fn meets(&self, need: Need) -> bool {
        match need {
            Need::SystemAdmin => self.system_admin,
            Need::OnNode { permission, .. } => self.permissions.contains(&permission),
            Need::OnEnvironment {
                permission: Permission::EnvironmentManage,
                environment,
            } => self.environment == Some((environment, true)),
            Need::OnEnvironment { .. } | Need::OnProvider { .. } => false,
        }
    }
}

orch_api_base::api_family_callback! {
    bind __api_bind_d30c90eee1, __api_install_d30c90eee1, __api_callback_d30c90eee1;
    slot __ApiCallbackd30c90eee1, __API_CALLBACK_D30C90EEE1, __api_set_d30c90eee1;
    base orch_api_base::routes::settings;

    async fn manages_environment<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: orch_types::Id) -> bool;
}

orch_api_base::api_family_callback! {
    bind __api_bind_ee30f0eed4, __api_install_ee30f0eed4, __api_callback_ee30f0eed4;
    slot __ApiCallbackee30f0eed4, __API_CALLBACK_EE30F0EED4, __api_set_ee30f0eed4;
    base orch_api_base::routes::settings;

    async fn shown<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: Vec<(orch_settings::SettingView, bool)>) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_api_contract::settings::ResolvedSetting>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_e6f0b9fe82, __api_install_e6f0b9fe82, __api_callback_e6f0b9fe82;
    slot __ApiCallbacke6f0b9fe82, __API_CALLBACK_E6F0B9FE82, __api_set_e6f0b9fe82;
    base orch_api_base::routes::settings;

    async fn write_failed<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: orch_types::Id, arg3: orch_settings::write::WriteError) -> orch_api_base::middleware::requests::ApiError;
}

/// The settings `keys` as they hold at the node `id` for the caller: what each
/// resolves to, where that comes from, and whether the caller may write it.
/// The requests that show a part of a node's settings next to something else,
/// as a thread's parameters do, read them here.
pub async fn resolved_keys(
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
    keys: &[&str],
) -> ApiResult<Vec<api::ResolvedSetting>> {
    let paired = readers::resolved_views(conn, principal, id, Some(keys)).await?;
    shown(conn, principal, paired).await
}

#[path = "settings/readers.rs"]
pub mod readers;
