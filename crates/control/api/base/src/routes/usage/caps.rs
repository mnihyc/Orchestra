//! Caps made, changed and removed by whoever the place of the cap says; how each of them stands; everything that bounds work at a node; a person's own share of the caps counted per person; and who waits for room in a cap.
//!
//! **Where a cap is defined decides who may touch it** ([`orch_authz::caps`]):
//! `usage.manage` on its node, `provider.manage` on its provider (and sight of
//! the node it also names), or a system admin for a cap of the system. Reading
//! the caps defined in a place needs `usage.view`, `provider.view`, or a system
//! admin. A person a cap covers reads their own share of it without any of
//! these, in their own limits, in the limits of a place they can see, and in the
//! queue of a cap on turns running at once.
//!
//! **A cap is checked whole before anything is written**: its subject and split
//! fit each other, a cap on turns at once has no period, warning, `at_limit` or
//! `uncounted`, the amount is a whole number where the meter counts whole
//! things and has at most six decimal places in credits, and a group belongs to
//! the tree of the place, and a model it names as `@<group>` names a group that can exist. A cap of the
//! same node, provider, subject, split, models, tiers, harness kinds, meter and period is
//! `409 conflict.exists`; a change
//! against a revision that is out of date is `409 conflict.stale_revision`
//! with the cap as it stands.
//!
//! **A cap made, changed or removed is looked at by the engine at once**: the
//! turns it covers that are under way are asked of it as when their usage is
//! reported, so that a cap that is reached and says `stop` ends them, and the
//! threads and inputs that waited are let through in order when room was made.
//! A person who is covered by a cap and is not in the tree of its place is still
//! covered.

pub use crate::routes::access::page::NameSources;

pub use chrono::DateTime;

pub use chrono::Utc;

pub use orch_authz::caps::Home;

pub use orch_db::usage::caps::CapRow;

pub use orch_engine::caps as engine;

pub use orch_types::Id;
pub type ComputedStatus = (CapRow, engine::Status, bool, bool);

/// Existing provider-choice standings and every source their ordinary projection names.
/// The caller admits full group payloads before bounded projection in the same read snapshot.
#[derive(Clone)]
pub struct PreparedProviderChoices {
    pub computed: Vec<ComputedStatus>,
    pub providers: Vec<Id>,
    pub at: DateTime<Utc>,
    pub names: NameSources,
}

orch_api_base::api_family_callback! {
    bind __api_bind_3887dbdd7a, __api_install_3887dbdd7a, __api_callback_3887dbdd7a;
    slot __ApiCallback3887dbdd7a, __API_CALLBACK_3887DBDD7A, __api_set_3887dbdd7a;
    base orch_api_base::routes::usage::caps;

    async fn for_provider_choices<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: &'__api [orch_types::Id]) -> orch_api_base::middleware::requests::ApiResult<std::collections::HashMap<orch_types::Id, Vec<orch_api_contract::usage::CapStatus>>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_e13c9f148f, __api_install_e13c9f148f, __api_callback_e13c9f148f;
    slot __ApiCallbacke13c9f148f, __API_CALLBACK_E13C9F148F, __api_set_e13c9f148f;
    base orch_api_base::routes::usage::caps;

    async fn prepare_provider_choices<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: &'__api [orch_types::Id]) -> orch_api_base::middleware::requests::ApiResult<orch_api_base::routes::usage::caps::PreparedProviderChoices>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_ef3be4443e, __api_install_ef3be4443e, __api_callback_ef3be4443e;
    slot __ApiCallbackef3be4443e, __API_CALLBACK_EF3BE4443E, __api_set_ef3be4443e;
    base orch_api_base::routes::usage::caps;

    async fn project_provider_choices<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: orch_api_base::routes::usage::caps::PreparedProviderChoices, arg4: Option<&'__api orch_api_base::routes::access::page::NameBounds>) -> orch_api_base::middleware::requests::ApiResult<std::collections::HashMap<orch_types::Id, Vec<orch_api_contract::usage::CapStatus>>>;
}

/// Native reached-cap rows and their actual provenance, before any full cap/group/name projection.
#[derive(Clone)]
pub struct PreparedReached {
    pub computed: Vec<ComputedStatus>,
    pub names: NameSources,
    pub providers: Vec<Id>,
    pub at: DateTime<Utc>,
}

impl PreparedReached {
    /// Scope homes come from the real selected cap rows, including system-wide caps.
    pub fn homes(&self) -> Vec<Home> {
        self.computed
            .iter()
            .map(|each| engine::home_of(&each.0))
            .collect()
    }

    /// The exact retained native cap set, independent of mutable use counters.
    pub fn ids(&self) -> Vec<Id> {
        self.computed.iter().map(|each| each.0.id).collect()
    }
}

orch_api_base::api_family_callback! {
    bind __api_bind_dddf635d8b, __api_install_dddf635d8b, __api_callback_dddf635d8b;
    slot __ApiCallbackdddf635d8b, __API_CALLBACK_DDDF635D8B, __api_set_dddf635d8b;
    base orch_api_base::routes::usage::caps;

    async fn prepare_reached_for_thread<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: orch_types::Id, arg4: Option<&'__api orch_db::queries::threads::InputRow>) -> orch_api_base::middleware::requests::ApiResult<orch_api_base::routes::usage::caps::PreparedReached>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_917524335b, __api_install_917524335b, __api_callback_917524335b;
    slot __ApiCallback917524335b, __API_CALLBACK_917524335B, __api_set_917524335b;
    base orch_api_base::routes::usage::caps;

    async fn reached_for_thread<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: orch_types::Id, arg4: Option<&'__api orch_db::queries::threads::InputRow>) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_api_contract::threads::LimitReached>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_285fabea4f, __api_install_285fabea4f, __api_callback_285fabea4f;
    slot __ApiCallback285fabea4f, __API_CALLBACK_285FABEA4F, __api_set_285fabea4f;
    base orch_api_base::routes::usage::caps;

    async fn show_reached_for_thread<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: &'__api orch_api_base::routes::usage::caps::PreparedReached, arg4: Option<&'__api orch_db::queries::threads::InputRow>, arg5: Option<&'__api orch_api_base::routes::access::page::NameBounds>) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_api_contract::threads::LimitReached>>;
}
