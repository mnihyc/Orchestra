//! Current readable setup observations and atomic portable first-steps choices.

pub use crate::middleware::requests::ApiError;

pub use crate::middleware::requests::ApiResult;

pub use crate::registry::Ctx;

pub use crate::routes::nodes::view;

pub use orch_access_contract::permissions::Permission;

use orch_api_contract::first_steps::*;

pub use orch_api_contract::nodes::Node;

pub use orch_authz::Policy;

pub use orch_authz::Principal;

pub use orch_db::Conn;

pub use orch_db::queries::first_steps as rows;

pub use orch_db::queries::identity;

pub use orch_types::Id;
orch_api_base::api_family_callback! {
    bind __api_bind_f635db2fdc, __api_install_f635db2fdc, __api_callback_f635db2fdc;
    slot __ApiCallbackf635db2fdc, __API_CALLBACK_F635DB2FDC, __api_set_f635db2fdc;
    base orch_api_base::routes::first_steps;

    fn mount<'__api>(arg0: &'__api mut orch_api_base::registry::Registry) -> ();
}

orch_api_base::api_family_callback! {
    bind __api_bind_712bb19250, __api_install_712bb19250, __api_callback_712bb19250;
    slot __ApiCallback712bb19250, __API_CALLBACK_712BB19250, __api_set_712bb19250;
    base orch_api_base::routes::first_steps;

    async fn state<'__api>(arg0: &'__api mut orch_db::Conn, arg1: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::personal::PersonalState>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_3b673ad843, __api_install_3b673ad843, __api_callback_3b673ad843;
    slot __ApiCallback3b673ad843, __API_CALLBACK_3B673AD843, __api_set_3b673ad843;
    base orch_api_base::routes::first_steps;

    async fn readable_org<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::nodes::Org>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_ed8b0e6ce1, __api_install_ed8b0e6ce1, __api_callback_ed8b0e6ce1;
    slot __ApiCallbacked8b0e6ce1, __API_CALLBACK_ED8B0E6CE1, __api_set_ed8b0e6ce1;
    base orch_api_base::routes::first_steps;

    fn action(arg0: String, arg1: bool) -> Option<orch_api_contract::first_steps::FirstStepsAction>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_8bf6e24f02, __api_install_8bf6e24f02, __api_callback_8bf6e24f02;
    slot __ApiCallback8bf6e24f02, __API_CALLBACK_8BF6E24F02, __api_set_8bf6e24f02;
    base orch_api_base::routes::first_steps;

    fn step(arg0: orch_api_contract::first_steps::FirstStepsId, arg1: orch_api_contract::first_steps::FirstStepsFact, arg2: Option<orch_api_contract::nodes::NodeRef>, arg3: Option<orch_api_contract::first_steps::FirstStepsAction>, arg4: bool) -> orch_api_contract::first_steps::FirstStepsStep;
}

orch_api_base::api_family_callback! {
    bind __api_bind_c22f2bb4e0, __api_install_c22f2bb4e0, __api_callback_c22f2bb4e0;
    slot __ApiCallbackc22f2bb4e0, __API_CALLBACK_C22F2BB4E0, __api_set_c22f2bb4e0;
    base orch_api_base::routes::first_steps;

    async fn role_summaries<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_api_contract::first_steps::InvitationRoleSummary>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_293ce96b17, __api_install_293ce96b17, __api_callback_293ce96b17;
    slot __ApiCallback293ce96b17, __API_CALLBACK_293CE96B17, __api_set_293ce96b17;
    base orch_api_base::routes::first_steps;

    async fn start_here_row<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: orch_types::Id, arg3: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<
    Option<(
        orch_db::queries::documents::DocumentRow,
        orch_authz::documents::Listed,
    )>,
>;
}

/// Internal observation handoff; the standalone reader retains its existing DTO.
pub struct ObservedFirstSteps {
    pub facts: FirstSteps,
    pub outside: Option<orch_api_contract::notifications::OutsideStatus>,
}

/// The overview consumes the same corrected observation cohort under its own scope.
pub async fn read_in(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    query: FirstStepsGetQuery,
    initial: Option<orch_api_contract::nodes::Org>,
    reading: Option<&crate::routes::nodes::overview::Reading<'_>>,
) -> ApiResult<ObservedFirstSteps> {
    let reading = reading.ok_or_else(|| {
        ApiError::internal("a composite setup observation requires its declared reader")
    })?;
    read::for_overview(ctx, conn, principal, query, initial, reading).await
}

#[path = "first_steps/read.rs"]
pub mod read;
