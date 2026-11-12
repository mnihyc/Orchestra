//! Where an environment could be made or put, over HTTP: the hosts the caller may use for one more, the most free memory first, then the environments they manage.
//!
//! A host is listed with what it has, what the units on it declare they need, what is left, whether
//! it can be chosen and why not, what it warns about, and whether it is what the scheduler would
//! choose; the figures come from the engine ([`orch_engine::places`]), which also decides what a
//! request that makes an environment does with them. The environments are listed as places a thread
//! could be put into: those the caller manages, with what they run on and who works there.

pub use crate::middleware::requests::ApiError;

pub use crate::middleware::requests::ApiResult;

pub use crate::registry::Ctx;

pub use orch_api_contract::environment::PlaceEnvironment;

pub use orch_authz::Policy;

pub use orch_db::queries::resources::hosting as host_rows;

pub use orch_engine::places;

pub use orch_types::Id;

pub use orch_types::Json;
orch_api_base::api_family_callback! {
    bind __api_bind_0165bcdf3c, __api_install_0165bcdf3c, __api_callback_0165bcdf3c;
    slot __ApiCallback0165bcdf3c, __API_CALLBACK_0165BCDF3C, __api_set_0165bcdf3c;
    base orch_api_base::routes::environments::places;

    async fn host_choices<'__api>(arg0: &'__api orch_api_base::state::AppState, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: orch_types::Id, arg4: &'__api orch_api_contract::environment::PlaceListQuery, arg5: orch_types::Time) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_api_contract::environment::PlaceHost>>;
}

/// The existing typed configuration/template input, before scope or host selection.
pub struct Request {
    pub own: Json,
    pub template: host_rows::TemplateState,
}

orch_api_base::api_family_callback! {
    bind __api_bind_84d36c5654, __api_install_84d36c5654, __api_callback_84d36c5654;
    slot __ApiCallback84d36c5654, __API_CALLBACK_84D36C5654, __api_set_84d36c5654;
    base orch_api_base::routes::environments::places;

    async fn prepare_request<'__api>(arg0: &'__api mut orch_db::Conn, arg1: orch_types::Id, arg2: &'__api orch_api_contract::environment::PlaceListQuery) -> orch_api_base::middleware::requests::ApiResult<orch_api_base::routes::environments::places::Request>;
}

/// Native resolved planner operands, shared by all host-choice readers.
pub struct HostChoiceInput {
    pub wanted: places::Wanted,
    pub minutes_idle: i64,
}

orch_api_base::api_family_callback! {
    bind __api_bind_a7a7119068, __api_install_a7a7119068, __api_callback_a7a7119068;
    slot __ApiCallbacka7a7119068, __API_CALLBACK_A7A7119068, __api_set_a7a7119068;
    base orch_api_base::routes::environments::places;

    async fn prepare_host_choices<'__api>(arg0: &'__api mut orch_db::Conn, arg1: orch_types::Id, arg2: &'__api orch_api_contract::environment::PlaceListQuery) -> orch_api_base::middleware::requests::ApiResult<orch_api_base::routes::environments::places::HostChoiceInput>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_08e2411db6, __api_install_08e2411db6, __api_callback_08e2411db6;
    slot __ApiCallback08e2411db6, __API_CALLBACK_08E2411DB6, __api_set_08e2411db6;
    base orch_api_base::routes::environments::places;

    async fn prepare_hosts<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: orch_types::Id, arg3: &'__api orch_api_base::routes::environments::places::Request, arg4: orch_types::Time) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_engine::places::HostPlace>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_804a7d87b5, __api_install_804a7d87b5, __api_callback_804a7d87b5;
    slot __ApiCallback804a7d87b5, __API_CALLBACK_804A7D87B5, __api_set_804a7d87b5;
    base orch_api_base::routes::environments::places;

    async fn prepare_hosts_among<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: orch_types::Id, arg3: &'__api orch_api_base::routes::environments::places::Request, arg4: orch_types::Time, arg5: &'__api [orch_types::Id]) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_engine::places::HostPlace>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_f09a39d7c1, __api_install_f09a39d7c1, __api_callback_f09a39d7c1;
    slot __ApiCallbackf09a39d7c1, __API_CALLBACK_F09A39D7C1, __api_set_f09a39d7c1;
    base orch_api_base::routes::environments::places;

    async fn select_host_choices<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: orch_types::Id, arg3: &'__api orch_api_base::routes::environments::places::HostChoiceInput, arg4: orch_types::Time, arg5: Option<&'__api [orch_types::Id]>) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_engine::places::HostPlace>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_525d07a808, __api_install_525d07a808, __api_callback_525d07a808;
    slot __ApiCallback525d07a808, __API_CALLBACK_525D07A808, __api_set_525d07a808;
    base orch_api_base::routes::environments::places;

    async fn name_host_choices<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api mut [orch_engine::places::HostPlace], arg2: Option<&'__api orch_api_base::routes::access::page::NameBounds>) -> orch_api_base::middleware::requests::ApiResult<()>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_1c0f1e698f, __api_install_1c0f1e698f, __api_callback_1c0f1e698f;
    slot __ApiCallback1c0f1e698f, __API_CALLBACK_1C0F1E698F, __api_set_1c0f1e698f;
    base orch_api_base::routes::environments::places;

    async fn group_host_choices<'__api>(arg0: &'__api orch_api_base::state::AppState, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: &'__api mut [orch_engine::places::HostPlace], arg4: Option<&'__api orch_api_base::routes::access::page::NameBounds>, arg5: Option<&'__api std::collections::BTreeSet<orch_types::Id>>) -> orch_api_base::middleware::requests::ApiResult<()>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_e34cda7dae, __api_install_e34cda7dae, __api_callback_e34cda7dae;
    slot __ApiCallbacke34cda7dae, __API_CALLBACK_E34CDA7DAE, __api_set_e34cda7dae;
    base orch_api_base::routes::environments::places;

    async fn manageable_environment_ids<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: Option<&'__api [orch_types::Id]>) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_types::Id>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_3cd543019e, __api_install_3cd543019e, __api_callback_3cd543019e;
    slot __ApiCallback3cd543019e, __API_CALLBACK_3CD543019E, __api_set_3cd543019e;
    base orch_api_base::routes::environments::places;

    fn place_of(arg0: orch_api_contract::environment::Environment) -> orch_api_contract::environment::PlaceEnvironment;
}

pub struct EnvironmentSource {
    pub environment: Id,
    pub host: Option<Id>,
    pub machine: Option<Id>,
    pub topics: std::collections::BTreeSet<Id>,
}

pub struct PreparedEnvironments {
    pub inventory: super::PreparedInventory,
    pub sources: Vec<EnvironmentSource>,
    pub disclosure: crate::routes::nodes::view::ReferenceDisclosure,
}

orch_api_base::api_family_callback! {
    bind __api_bind_75ad864d9c, __api_install_75ad864d9c, __api_callback_75ad864d9c;
    slot __ApiCallback75ad864d9c, __API_CALLBACK_75AD864D9C, __api_set_75ad864d9c;
    base orch_api_base::routes::environments::places;

    async fn prepare_environments<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: &'__api [orch_authz::resources::Seen]) -> orch_api_base::middleware::requests::ApiResult<orch_api_base::routes::environments::places::PreparedEnvironments>;
}

/// The consuming projection's already resolved bounds on protected context names.
pub struct EnvironmentNames<'a> {
    pub limited: bool,
    pub hosts: &'a std::collections::BTreeSet<Id>,
    pub machines: &'a std::collections::BTreeSet<Id>,
    pub topics: &'a std::collections::BTreeSet<Id>,
}

impl PreparedEnvironments {
    pub async fn render(
        &self,
        ctx: &Ctx,
        conn: &mut orch_db::Conn,
        principal: &orch_authz::Principal,
        names: EnvironmentNames<'_>,
    ) -> ApiResult<Vec<PlaceEnvironment>> {
        let EnvironmentNames {
            limited,
            hosts,
            machines,
            topics,
        } = names;
        let ids = self
            .sources
            .iter()
            .flat_map(|source| source.topics.iter().copied())
            .collect::<Vec<_>>();
        let refs = if limited {
            crate::routes::nodes::view::refs_with_name_sources(
                conn,
                principal,
                &ids,
                Policy::default(),
                topics,
            )
            .await?
        } else {
            crate::routes::nodes::view::refs(conn, principal, &ids, Policy::default()).await?
        };
        let mut result = Vec::new();
        for view in super::show_inventory_prepared(ctx, conn, principal, &self.inventory).await? {
            let id = view.base.id;
            let mut place = place_of(view);
            let source = self
                .sources
                .iter()
                .find(|source| source.environment == id)
                .ok_or_else(ApiError::not_found)?;
            if limited {
                if source.host.is_some_and(|host| !hosts.contains(&host)) {
                    place.host = None;
                }
                if source
                    .machine
                    .is_some_and(|machine| !machines.contains(&machine))
                {
                    place.machine = None;
                }
                for topic in &mut place.topics {
                    *topic = refs
                        .get(&topic.id)
                        .cloned()
                        .ok_or_else(ApiError::not_found)?;
                }
            }
            result.push(place);
        }
        Ok(result)
    }

    pub fn preserves(
        &self,
        fresh: &Self,
        hosts: &std::collections::BTreeSet<Id>,
        machines: &std::collections::BTreeSet<Id>,
    ) -> bool {
        self.sources.iter().all(|old| {
            fresh
                .sources
                .iter()
                .find(|now| now.environment == old.environment)
                .is_some_and(|now| {
                    old.host
                        .filter(|host| hosts.contains(host))
                        .is_none_or(|host| now.host == Some(host))
                        && old
                            .machine
                            .filter(|machine| machines.contains(machine))
                            .is_none_or(|machine| now.machine == Some(machine))
                        && old.topics.is_subset(&now.topics)
                })
        })
    }
}
