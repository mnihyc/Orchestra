//! Sync, timeline with the lines of a thread's history and that history as a view, inputs on the sender's own access, side threads, the limits in a turn's way and letting an input past them, draft, pins, binding, fork from any message and exports at three depths.
//!
//! A thread is a node of the tree, made in a topic and read and changed through its own requests. What
//! the engine keeps of it is shown here as the wire has it; who may do what is the access engine's to say,
//! asked of it before a handler runs or by the engine's own functions inside the transaction of the act.

orch_api_base::api_family_callback! {
    bind __api_bind_6de5279cdb, __api_install_6de5279cdb, __api_callback_6de5279cdb;
    slot __ApiCallback6de5279cdb, __API_CALLBACK_6DE5279CDB, __api_set_6de5279cdb;
    base orch_api_base::routes::threads;

    fn mount<'__api>(arg0: &'__api mut orch_api_base::registry::Registry) -> ();
}

orch_api_base::api_family_callback! {
    bind __api_bind_0eb3ad3c68, __api_install_0eb3ad3c68, __api_callback_0eb3ad3c68;
    slot __ApiCallback0eb3ad3c68, __API_CALLBACK_0EB3AD3C68, __api_set_0eb3ad3c68;
    base orch_api_base::routes::threads;

    async fn create_failed<'__api>(arg0: &'__api mut orch_db::Conn, arg1: orch_engine::binding::CreateThreadError) -> orch_api_base::middleware::requests::ApiError;
}

orch_api_base::api_family_callback! {
    bind __api_bind_d06fba3ddf, __api_install_d06fba3ddf, __api_callback_d06fba3ddf;
    slot __ApiCallbackd06fba3ddf, __API_CALLBACK_D06FBA3DDF, __api_set_d06fba3ddf;
    base orch_api_base::routes::threads;

    async fn input_failed<'__api>(arg0: &'__api mut orch_db::Conn, arg1: orch_types::Id, arg2: orch_engine::inputs::InputError) -> orch_api_base::middleware::requests::ApiError;
}

orch_api_base::api_family_callback! {
    bind __api_bind_a84e544ee2, __api_install_a84e544ee2, __api_callback_a84e544ee2;
    slot __ApiCallbacka84e544ee2, __API_CALLBACK_A84E544EE2, __api_set_a84e544ee2;
    base orch_api_base::routes::threads;

    async fn input_failed_projected<'__api>(arg0: &'__api mut orch_db::Conn, arg1: orch_types::Id, arg2: orch_engine::inputs::InputError, arg3: bool) -> orch_api_base::middleware::requests::ApiError;
}

orch_api_base::api_family_callback! {
    bind __api_bind_f0eaaf0710, __api_install_f0eaaf0710, __api_callback_f0eaaf0710;
    slot __ApiCallbackf0eaaf0710, __API_CALLBACK_F0EAAF0710, __api_set_f0eaaf0710;
    base orch_api_base::routes::threads;

    async fn native_failed<'__api>(arg0: &'__api mut orch_db::Conn, arg1: orch_engine::native_actions::NativeError) -> orch_api_base::middleware::requests::ApiError;
}

#[path = "threads/adopt.rs"]
pub mod adopt;

#[path = "threads/approval_items.rs"]
pub mod approval_items;

#[path = "threads/binding.rs"]
pub mod binding;

#[path = "threads/creation_operation.rs"]
pub mod creation_operation;

#[path = "threads/make.rs"]
pub mod make;

#[path = "threads/native.rs"]
pub mod native;

#[path = "threads/uploads.rs"]
pub mod uploads;

#[path = "threads/view.rs"]
pub mod view;
