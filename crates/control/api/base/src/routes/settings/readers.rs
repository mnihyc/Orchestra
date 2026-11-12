//! Native settings and secret metadata prepared on a caller-supplied read snapshot.
//!
//! Preparation keeps raw resolver facts. The enclosing operation admits the actual
//! provenance sources before these helpers put bounded current names in the response.

use super::Holds;

use super::placed;

use super::policy_at;

use super::scope_of;

pub use crate::middleware::requests::ApiError;

pub use crate::middleware::requests::ApiResult;

pub use orch_authz::Principal;

pub use orch_db::Conn;

pub use orch_settings::secrets;

use orch_settings::validation;

pub use orch_settings::Place;

pub use orch_settings::SettingView;

use orch_settings::resolve_all;

pub use orch_types::Id;
/// Raw resolved values and write hints, without a projected Node reference.
#[derive(Clone)]
pub struct PreparedSettings {
    pub node: Id,
    pub views: Vec<(SettingView, bool)>,
}

impl PreparedSettings {
    /// Only actual provenance references, never identifiers guessed from stored values.
    pub fn source_nodes(&self) -> Vec<Id> {
        setting_sources(&self.views)
    }

    /// A configured resource identity already present in the settings' stored value.
    pub fn selected_id(&self, key: &str, member: Option<&str>) -> Option<Id> {
        let value = self
            .views
            .iter()
            .find(|(view, _)| view.key() == key)?
            .0
            .value()?;
        let value = match member {
            Some(member) => value.get(member)?,
            None => value,
        };
        value.as_str()?.parse().ok()
    }
}

/// Secret names and timestamps only. This type never contains a secret value.
#[derive(Clone)]
pub struct PreparedSecrets {
    pub list: Vec<secrets::SecretInfo>,
}

impl PreparedSecrets {
    pub fn source_nodes(&self) -> Vec<Id> {
        let mut ids: Vec<_> = self.list.iter().filter_map(|info| info.node).collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    }
}

orch_api_base::api_family_callback! {
    bind __api_bind_e928a69f70, __api_install_e928a69f70, __api_callback_e928a69f70;
    slot __ApiCallbacke928a69f70, __API_CALLBACK_E928A69F70, __api_set_e928a69f70;
    base orch_api_base::routes::settings::readers;

    async fn prepare_settings<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<orch_api_base::routes::settings::readers::PreparedSettings>;
}

/// Kept separate from the list's NodeView gate: existing internal selected-key readers
/// already have their own ordinary entry gate and retain that exact behavior.
pub async fn resolved_views(
    conn: &mut Conn,
    principal: &Principal,
    node: Id,
    keys: Option<&[&str]>,
) -> ApiResult<Vec<(SettingView, bool)>> {
    let (_, kind) = placed(conn, node).await?;
    let policy = policy_at(conn, node).await?;
    let holds = Holds::of(conn, principal, node, kind, policy).await?;
    let views = resolve_all(conn, Place::Node(node))
        .await
        .map_err(ApiError::internal)?;
    let scope = scope_of(kind);
    Ok(views
        .into_iter()
        .filter(|view| keys.is_none_or(|keys| keys.contains(&view.def().key)))
        .map(|view| {
            let need =
                validation::need_for_node(view.def(), scope, node, holds.thread_environment());
            let permitted = need.is_some_and(|need| holds.meets(need));
            (view, permitted)
        })
        .collect())
}

orch_api_base::api_family_callback! {
    bind __api_bind_63a3158c5e, __api_install_63a3158c5e, __api_callback_63a3158c5e;
    slot __ApiCallback63a3158c5e, __API_CALLBACK_63A3158C5E, __api_set_63a3158c5e;
    base orch_api_base::routes::settings::readers;

    async fn show_settings<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: &'__api orch_api_base::routes::settings::readers::PreparedSettings, arg4: &'__api orch_api_base::routes::access::page::NameBounds) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_api_contract::settings::ResolvedSetting>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_645b0cf6ac, __api_install_645b0cf6ac, __api_callback_645b0cf6ac;
    slot __ApiCallback645b0cf6ac, __API_CALLBACK_645B0CF6AC, __api_set_645b0cf6ac;
    base orch_api_base::routes::settings::readers;

    async fn show_settings_with_controls<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: &'__api orch_api_base::routes::settings::readers::PreparedSettings, arg4: &'__api orch_api_base::routes::access::page::NameBounds, arg5: bool) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_api_contract::settings::ResolvedSetting>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_29632770a2, __api_install_29632770a2, __api_callback_29632770a2;
    slot __ApiCallback29632770a2, __API_CALLBACK_29632770A2, __api_set_29632770a2;
    base orch_api_base::routes::settings::readers;

    fn setting_sources<'__api>(arg0: &'__api [(orch_settings::SettingView, bool)]) -> Vec<orch_types::Id>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_e4bc8ef338, __api_install_e4bc8ef338, __api_callback_e4bc8ef338;
    slot __ApiCallbacke4bc8ef338, __API_CALLBACK_E4BC8EF338, __api_set_e4bc8ef338;
    base orch_api_base::routes::settings::readers;

    async fn settings_shown<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: Vec<(orch_settings::SettingView, bool)>, arg3: &'__api orch_api_base::routes::access::page::NameBounds) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_api_contract::settings::ResolvedSetting>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_b00f576e8e, __api_install_b00f576e8e, __api_callback_b00f576e8e;
    slot __ApiCallbackb00f576e8e, __API_CALLBACK_B00F576E8E, __api_set_b00f576e8e;
    base orch_api_base::routes::settings::readers;

    async fn prepare_secrets<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<orch_api_base::routes::settings::readers::PreparedSecrets>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_d846652dcd, __api_install_d846652dcd, __api_callback_d846652dcd;
    slot __ApiCallbackd846652dcd, __API_CALLBACK_D846652DCD, __api_set_d846652dcd;
    base orch_api_base::routes::settings::readers;

    async fn show_secrets<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api orch_api_base::routes::settings::readers::PreparedSecrets, arg3: &'__api orch_api_base::routes::access::page::NameBounds) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_api_contract::settings::SecretInfo>>;
}
