//! Whether an environment can be asked now, asking it, and asking its machine for its measurements once for each session.
//!
//! An environment is asked when it stands attached in the session its machine has: the engine attaches
//! an environment in each session of its machine (a directory on a person's machine by the schedule of
//! environments, one made from a template by the job that makes it) and says so in the environment's
//! row, and nothing here sends `env.attach`. Where it does not stand attached, or its machine has no
//! session, what is asked of it is answered from what the system kept, or refused as offline. A machine
//! that holds an environment reports measurements only while a visible live subscription requests
//! them; the API keeps those samples for six hours. A directory observation may temporarily share
//! native registration with an initial live view; it starts no periodic read or retained HTTP watch.

pub use super::problems::Failure;

pub use crate::registry::Ctx;

use orch_link_contract::commands::CommandBody;

use orch_link_contract::commands::SerialKey;

pub use orch_types::Id;
orch_api_base::api_family_callback! {
    bind __api_bind_0a904728c5, __api_install_0a904728c5, __api_callback_0a904728c5;
    slot __ApiCallback0a904728c5, __API_CALLBACK_0A904728C5, __api_set_0a904728c5;
    base orch_api_base::routes::workspace::live;

    async fn machine_of<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_db::queries::environments::workspace::EnvironmentRow) -> orch_api_base::middleware::requests::ApiResult<Option<orch_types::Id>>;
}

/// Runs a command of the catalogue at a machine and waits for its result.
pub async fn run<B: CommandBody>(
    ctx: &Ctx,
    machine: Id,
    key: SerialKey,
    body: &B,
) -> Result<B::Result, Failure> {
    ctx.app
        .link
        .dispatcher()
        .run(machine, &key, body)
        .await
        .map_err(command_failure)
}

orch_api_base::api_family_callback! {
    bind __api_bind_30aad6c07a, __api_install_30aad6c07a, __api_callback_30aad6c07a;
    slot __ApiCallback30aad6c07a, __API_CALLBACK_30AAD6C07A, __api_set_30aad6c07a;
    base orch_api_base::routes::workspace::live;

    fn command_failure(arg0: orch_api_base::dispatch::RunError) -> orch_api_base::routes::workspace::problems::Failure;
}

orch_api_base::api_family_callback! {
    bind __api_bind_190ad4e921, __api_install_190ad4e921, __api_callback_190ad4e921;
    slot __ApiCallback190ad4e921, __API_CALLBACK_190AD4E921, __api_set_190ad4e921;
    base orch_api_base::routes::workspace::live;

    async fn require_live<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_db::queries::environments::workspace::EnvironmentRow) -> orch_api_base::middleware::requests::ApiResult<orch_types::Id>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_c89aa3c2c0, __api_install_c89aa3c2c0, __api_callback_c89aa3c2c0;
    slot __ApiCallbackc89aa3c2c0, __API_CALLBACK_C89AA3C2C0, __api_set_c89aa3c2c0;
    base orch_api_base::routes::workspace::live;

    fn refused(arg0: orch_api_base::routes::workspace::problems::Failure, arg1: orch_api_base::routes::workspace::problems::Doing) -> orch_api_base::middleware::requests::ApiError;
}
