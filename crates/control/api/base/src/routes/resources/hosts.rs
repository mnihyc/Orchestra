//! Hosts: the machines that make environments from templates, listed by what the caller may use or manage, read, changed by their managers, and the token with which one enrolls.
//!
//! A host is a resource with a machine behind it: the machine is where its node runs, and the host
//! is what is owned, shared and used. What the node reports about itself (its drivers, its
//! capacity, its storages) is shown as it reports it; what an owner decides (the name, the labels,
//! whether it takes new environments, what it does when it is full) is changed here. Its stored
//! revision counter advances on configuration and ownership changes. Taking a host away, its metrics and its recycling
//! wait for the node and the engine's jobs. Its runtimes are listed, stopped and removed by its managers,
//! whoever's they are: that is their machine.

pub use crate::middleware::authorization;

pub use crate::middleware::requests::ApiResult;

pub use crate::routes::access::page::NameBounds;

use crate::routes::access::page::NameSources;

use crate::routes::resources::view::PreparedAccess;

use orch_access_contract::decisions::Held;

pub use orch_access_contract::permissions::Permission;

pub use orch_api_contract::resources as api;

pub use orch_authz::Principal;

pub use orch_authz::resources;

pub use orch_db::queries::resources::routes as queries;

pub use orch_db::queries::resources::routes::HostRow;

pub use orch_db::Conn;

pub use orch_types::Id;

pub use std::collections::BTreeSet;

pub use std::collections::HashMap;
/// What the caller holds on a host, as the engine told it.
#[derive(Debug, Clone)]
pub struct Standing {
    pub admin_access: Option<orch_access_contract::roles::AdminReach>,
    pub admin_place: Option<orch_access_contract::roles::PlaceRef>,
    pub participating: bool,
    pub admin_only: Vec<Permission>,
    pub owner: bool,
    pub administers: bool,
    pub held: Vec<Held>,
}

orch_api_base::api_family_callback! {
    bind __api_bind_abd603b8b5, __api_install_abd603b8b5, __api_callback_abd603b8b5;
    slot __ApiCallbackabd603b8b5, __API_CALLBACK_ABD603B8B5, __api_set_abd603b8b5;
    base orch_api_base::routes::resources::hosts;

    async fn layer_environment_refs<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api [orch_db::queries::resources::routes::EnvironmentRefRow], arg2: orch_types::Time, arg3: Option<&'__api orch_api_base::routes::access::page::NameBounds>, arg4: Option<&'__api std::collections::BTreeSet<orch_types::Id>>) -> orch_api_base::middleware::requests::ApiResult<std::collections::HashMap<orch_types::Id, orch_api_contract::environment::EnvironmentRef>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_7a74f15f7b, __api_install_7a74f15f7b, __api_callback_7a74f15f7b;
    slot __ApiCallback7a74f15f7b, __API_CALLBACK_7A74F15F7B, __api_set_7a74f15f7b;
    base orch_api_base::routes::resources::hosts;

    fn kept_layers_prepared<'__api>(arg0: &'__api std::collections::HashMap<orch_types::Id, Vec<orch_driver_contract::kept::KeptImage>>, arg1: &'__api std::collections::HashMap<orch_types::Id, orch_api_contract::environment::EnvironmentRef>) -> std::collections::HashMap<orch_types::Id, Vec<orch_api_contract::resources::HostKeptLayers>>;
}

pub struct KeptData {
    pub row: orch_db::queries::environments::retained::HostData,
    pub removal: orch_api_contract::environment::EnvironmentRemoval,
}

orch_api_base::api_family_callback! {
    bind __api_bind_e32b4d7a69, __api_install_e32b4d7a69, __api_callback_e32b4d7a69;
    slot __ApiCallbacke32b4d7a69, __API_CALLBACK_E32B4D7A69, __api_set_e32b4d7a69;
    base orch_api_base::routes::resources::hosts;

    fn preserves_kept<'__api>(arg0: &'__api [orch_api_base::routes::resources::hosts::KeptData], arg1: &'__api std::collections::HashMap<orch_types::Id, Vec<orch_driver_contract::kept::KeptImage>>, arg2: Option<&'__api std::collections::BTreeSet<orch_types::Id>>, arg3: &'__api [orch_api_base::routes::resources::hosts::KeptData], arg4: &'__api std::collections::HashMap<orch_types::Id, Vec<orch_driver_contract::kept::KeptImage>>) -> bool;
}

orch_api_base::api_family_callback! {
    bind __api_bind_13e7353061, __api_install_13e7353061, __api_callback_13e7353061;
    slot __ApiCallback13e7353061, __API_CALLBACK_13E7353061, __api_set_13e7353061;
    base orch_api_base::routes::resources::hosts;

    async fn kept_data_views_bounded<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api [orch_api_base::routes::resources::hosts::KeptData], arg3: Option<&'__api orch_api_base::routes::access::page::NameBounds>, arg4: Option<&'__api std::collections::BTreeSet<orch_types::Id>>) -> orch_api_base::middleware::requests::ApiResult<std::collections::HashMap<orch_types::Id, Vec<orch_api_contract::resources::HostKeptData>>>;
}

/// The native fleet and ordinary host readers share retained-file/layer source selection.
pub struct PreparedRetention {
    pub data: Vec<KeptData>,
    pub layers: HashMap<Id, Vec<orch_driver_contract::kept::KeptImage>>,
    pub environments: Vec<queries::EnvironmentRefRow>,
    pub environment_nodes: BTreeSet<Id>,
    pub names: Option<(NameBounds, BTreeSet<Id>)>,
    pub now: orch_types::Time,
}

impl PreparedRetention {
    pub fn sources(&self) -> (NameSources, BTreeSet<Id>) {
        let mut names = NameSources::default();
        names
            .nodes
            .extend(self.data.iter().map(|entry| entry.row.home_id));
        names.nodes.extend(self.environment_nodes.iter().copied());
        let mut environments = self
            .data
            .iter()
            .filter(|entry| entry.removal.can_inspect)
            .map(|entry| entry.row.environment_id)
            .collect::<BTreeSet<_>>();
        environments.extend(self.environments.iter().map(|row| row.id));
        (names, environments)
    }
    pub fn bound_names(&mut self, bounds: NameBounds, environments: BTreeSet<Id>) {
        self.names = Some((bounds, environments));
    }
    pub fn preserves_sources(&self, fresh: &Self) -> bool {
        preserves_kept(
            &self.data,
            &self.layers,
            self.names.as_ref().map(|(_, ids)| ids),
            &fresh.data,
            &fresh.layers,
        )
    }
    pub async fn render(
        &self,
        conn: &mut Conn,
        principal: &Principal,
    ) -> ApiResult<(
        HashMap<Id, Vec<api::HostKeptData>>,
        HashMap<Id, Vec<api::HostKeptLayers>>,
    )> {
        let data = kept_data_views_bounded(
            conn,
            principal,
            &self.data,
            self.names.as_ref().map(|(n, _)| n),
            self.names.as_ref().map(|(_, ids)| ids),
        )
        .await?;
        let environments = layer_environment_refs(
            conn,
            &self.environments,
            self.now,
            self.names.as_ref().map(|(bounds, _)| bounds),
            self.names.as_ref().map(|(_, ids)| ids),
        )
        .await?;
        Ok((data, kept_layers_prepared(&self.layers, &environments)))
    }
}

orch_api_base::api_family_callback! {
    bind __api_bind_795fa7a950, __api_install_795fa7a950, __api_callback_795fa7a950;
    slot __ApiCallback795fa7a950, __API_CALLBACK_795FA7A950, __api_set_795fa7a950;
    base orch_api_base::routes::resources::hosts;

    async fn prepare_retention<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: &'__api [orch_types::Id], arg4: &'__api [orch_types::Id]) -> orch_api_base::middleware::requests::ApiResult<orch_api_base::routes::resources::hosts::PreparedRetention>;
}

/// Opaque native Host facts; optional names narrow the same ordinary construction.
pub struct PreparedInventory {
    pub rows: Vec<HostRow>,
    pub standing: HashMap<Id, Standing>,
    pub access: PreparedAccess,
    pub sources: NameSources,
    pub bounds: NameBounds,
    pub layers: HashMap<Id, Vec<orch_driver_contract::kept::KeptImage>>,
    pub data: Vec<KeptData>,
    pub layer_environments: Vec<queries::EnvironmentRefRow>,
    pub retained_name_sources: Vec<Id>,
    pub retained_bounds: Option<BTreeSet<Id>>,
}

impl PreparedInventory {
    /// The Host's physical publisher and retained context remain this native association.
    pub fn preserves_sources(&self, fresh: &Self) -> bool {
        let rows = fresh
            .rows
            .iter()
            .map(|row| (row.id, row.machine_id))
            .collect::<HashMap<_, _>>();
        let retained = fresh
            .retained_name_sources
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        self.access.preserves_authority(&fresh.access)
            && preserves_kept(
                &self.data,
                &self.layers,
                self.retained_bounds.as_ref(),
                &fresh.data,
                &fresh.layers,
            )
            && self
                .rows
                .iter()
                .all(|row| rows.get(&row.id) == Some(&row.machine_id))
            && self
                .retained_name_sources
                .iter()
                .all(|id| retained.contains(id))
    }

    pub fn name_sources(&self) -> &NameSources {
        &self.sources
    }
    pub fn bound_names(&mut self, bounds: NameBounds) {
        self.bounds = bounds;
    }
    /// Actual protected retained names/refs, excluding ID-only or absent context references.
    /// Bound only the actual protected retention names of this consuming projection.
    pub async fn bound_retained_sources(
        &mut self,
        conn: &mut Conn,
        selected: BTreeSet<Id>,
    ) -> ApiResult<()> {
        self.retained_name_sources
            .retain(|id| selected.contains(id));
        self.sources = self.access.names.clone();
        self.sources
            .nodes
            .extend(self.rows.iter().filter_map(|row| row.home_id));
        self.sources
            .nodes
            .extend(self.data.iter().map(|entry| entry.row.home_id));
        let ids: Vec<_> = self
            .layer_environments
            .iter()
            .filter(|row| selected.contains(&row.id))
            .map(|row| row.id)
            .collect();
        self.sources.nodes.extend(
            resources::address_name_sources(conn, &ids)
                .await
                .map_err(authorization::restoration_error)?,
        );
        self.retained_bounds = Some(selected);
        Ok(())
    }

    pub fn retained_name_sources(&self) -> &[Id] {
        &self.retained_name_sources
    }
}

orch_api_base::api_family_callback! {
    bind __api_bind_60ce0f2a85, __api_install_60ce0f2a85, __api_callback_60ce0f2a85;
    slot __ApiCallback60ce0f2a85, __API_CALLBACK_60CE0F2A85, __api_set_60ce0f2a85;
    base orch_api_base::routes::resources::hosts;

    async fn show_inventory_prepared<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: &'__api orch_api_base::routes::resources::hosts::PreparedInventory) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_api_contract::resources::Host>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_5f012c09b2, __api_install_5f012c09b2, __api_callback_5f012c09b2;
    slot __ApiCallback5f012c09b2, __API_CALLBACK_5F012C09B2, __api_set_5f012c09b2;
    base orch_api_base::routes::resources::hosts;

    async fn inventory<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: &'__api [orch_authz::resources::Seen]) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_api_contract::resources::Host>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_5b3da33bcf, __api_install_5b3da33bcf, __api_callback_5b3da33bcf;
    slot __ApiCallback5b3da33bcf, __API_CALLBACK_5B3DA33BCF, __api_set_5b3da33bcf;
    base orch_api_base::routes::resources::hosts;

    async fn prepare_inventory<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: &'__api [orch_authz::resources::Seen]) -> orch_api_base::middleware::requests::ApiResult<orch_api_base::routes::resources::hosts::PreparedInventory>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_2109b1c7fb, __api_install_2109b1c7fb, __api_callback_2109b1c7fb;
    slot __ApiCallback2109b1c7fb, __API_CALLBACK_2109B1C7FB, __api_set_2109b1c7fb;
    base orch_api_base::routes::resources::hosts;

    async fn inventory_loads<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api [orch_types::Id]) -> orch_api_base::middleware::requests::ApiResult<orch_types::Json>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_8c15bb6b04, __api_install_8c15bb6b04, __api_callback_8c15bb6b04;
    slot __ApiCallback8c15bb6b04, __API_CALLBACK_8C15BB6B04, __api_set_8c15bb6b04;
    base orch_api_base::routes::resources::hosts;

    async fn show<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::resources::Host>;
}
