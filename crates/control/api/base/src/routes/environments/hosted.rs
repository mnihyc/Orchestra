//! Environments made from a template on a host over HTTP: making one, starting and stopping it, keeping it awake, taking back a removal that has not begun, and the fleet of a place.
//!
//! Making an environment is one request that does two things in one transaction: the environment's
//! row, owned by the caller, and the job that makes its unit on a host, which the reply names. A host
//! that is named and cannot take it is not replaced by another; the request says why and which places
//! there are. Starting is the same job for an environment that is stopped (a turn does it by itself);
//! stopping asks the host to stop the unit and keeps it and its volumes; a unit's credentials that were
//! handed over are returned first, which is not built yet, so an environment that holds one stays up.

pub use crate::middleware::requests::ApiResult;

pub use orch_api_contract::environment::FleetEntry;

pub use orch_authz::Policy;

pub use orch_authz::Principal;

pub use orch_db::queries::environments::routes as queries;

pub use orch_db::Conn;

pub use orch_types::Id;

pub use std::collections::HashMap;
orch_api_base::api_family_callback! {
    bind __api_bind_3664da9cf1, __api_install_3664da9cf1, __api_callback_3664da9cf1;
    slot __ApiCallback3664da9cf1, __API_CALLBACK_3664DA9CF1, __api_set_3664da9cf1;
    base orch_api_base::routes::environments::hosted;

    async fn create_in<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: orch_api_contract::binding::NewEnvironment) -> orch_api_base::middleware::requests::ApiResult<orch_engine::environments::Made>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_25a1104266, __api_install_25a1104266, __api_callback_25a1104266;
    slot __ApiCallback25a1104266, __API_CALLBACK_25A1104266, __api_set_25a1104266;
    base orch_api_base::routes::environments::hosted;

    async fn create_with_prepared_initial<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: orch_api_contract::binding::NewEnvironment, arg4: Option<orch_engine::initial_files::Plan>) -> orch_api_base::middleware::requests::ApiResult<orch_engine::environments::Made>;
}

// ---------------------------------------------------------------- the fleet of a place

/// The environments that threads under a node run in and those whose home is there, as far as the
/// caller may see them, with the hosts the caller manages.
pub struct FleetSource {
    pub environment: Id,
    pub home: Id,
    pub template: Option<Id>,
    pub host: Option<Id>,
    pub providers: Vec<Id>,
    pub manageable: bool,
}

#[derive(Default)]
pub struct FleetBounds {
    pub limited: bool,
    pub homes: std::collections::BTreeSet<Id>,
    pub hosts: std::collections::BTreeSet<Id>,
    pub templates: std::collections::BTreeSet<Id>,
    pub providers: std::collections::BTreeSet<Id>,
}

pub struct PreparedFleet {
    pub sources: Vec<FleetSource>,
    pub entries: Vec<FleetEntry>,
    pub providers: HashMap<Id, Vec<orch_db::queries::resources::routes::ProviderRefRow>>,
    pub bounds: FleetBounds,
    pub disclosure: crate::routes::nodes::view::ReferenceDisclosure,
}

orch_api_base::api_family_callback! {
    bind __api_bind_84fb7b9d00, __api_install_84fb7b9d00, __api_callback_84fb7b9d00;
    slot __ApiCallback84fb7b9d00, __API_CALLBACK_84FB7B9D00, __api_set_84fb7b9d00;
    base orch_api_base::routes::environments::hosted;

    async fn prepare_fleet<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: &'__api [orch_types::Id], arg4: &'__api orch_api_contract::environment::NodeEnvironmentListQuery) -> orch_api_base::middleware::requests::ApiResult<orch_api_base::routes::environments::hosted::PreparedFleet>;
}

impl PreparedFleet {
    pub async fn render(
        &self,
        conn: &mut Conn,
        principal: &Principal,
    ) -> ApiResult<Vec<FleetEntry>> {
        let ids = self
            .sources
            .iter()
            .map(|source| source.home)
            .collect::<Vec<_>>();
        let refs = if self.bounds.limited {
            crate::routes::nodes::view::refs_with_name_sources(
                conn,
                principal,
                &ids,
                Policy::default(),
                &self.bounds.homes,
            )
            .await?
        } else {
            crate::routes::nodes::view::refs(conn, principal, &ids, Policy::default()).await?
        };
        let mut result = Vec::new();
        for (old, source) in self.entries.iter().zip(&self.sources) {
            let mut entry = old.clone();
            entry.home = refs.get(&source.home).cloned().or_else(|| old.home.clone());
            if self.bounds.limited {
                if source
                    .host
                    .is_some_and(|host| !self.bounds.hosts.contains(&host))
                {
                    entry.host_name = None;
                }
                if source
                    .template
                    .is_some_and(|template| !self.bounds.templates.contains(&template))
                {
                    entry.template = None;
                }
            }
            let rows = self
                .providers
                .get(&source.environment)
                .into_iter()
                .flatten()
                .filter(|row| !self.bounds.limited || self.bounds.providers.contains(&row.id))
                .cloned()
                .collect();
            entry.holds = crate::routes::resources::view::provider_refs_from_rows(conn, rows)
                .await?
                .into_iter()
                .map(|(_, provider)| provider)
                .collect();
            result.push(entry);
        }
        Ok(result)
    }

    pub fn preserves(&self, fresh: &Self) -> bool {
        let current: HashMap<_, _> = fresh
            .sources
            .iter()
            .map(|source| (source.environment, source))
            .collect();
        self.sources.iter().all(|old| {
            current.get(&old.environment).is_some_and(|now| {
                old.home == now.home
                    && (!old.manageable || now.manageable)
                    && old
                        .host
                        .filter(|host| self.bounds.hosts.contains(host))
                        .is_none_or(|host| now.host == Some(host))
                    && old
                        .template
                        .filter(|template| self.bounds.templates.contains(template))
                        .is_none_or(|template| now.template == Some(template))
                    && old
                        .providers
                        .iter()
                        .filter(|id| self.bounds.providers.contains(id))
                        .all(|id| now.providers.contains(id))
            })
        }) && self.disclosure.names.is_subset(&fresh.disclosure.names)
            && self
                .disclosure
                .displays
                .is_subset(&fresh.disclosure.displays)
    }
}

orch_api_base::api_family_callback! {
    bind __api_bind_a292f8a126, __api_install_a292f8a126, __api_callback_a292f8a126;
    slot __ApiCallbacka292f8a126, __API_CALLBACK_A292F8A126, __api_set_a292f8a126;
    base orch_api_base::routes::environments::hosted;

    async fn host_loads<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api [orch_types::Id]) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_api_contract::environment::HostLoad>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_0df7f1c396, __api_install_0df7f1c396, __api_callback_0df7f1c396;
    slot __ApiCallback0df7f1c396, __API_CALLBACK_0DF7F1C396, __api_set_0df7f1c396;
    base orch_api_base::routes::environments::hosted;

    fn initial_failed(arg0: orch_engine::initial_files::Error) -> orch_api_base::middleware::requests::ApiError;
}
