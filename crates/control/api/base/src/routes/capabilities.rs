//! Taking a capability back from an environment: the key removed, the removal written down, and the unit of the environment asked to be made anew without it.
//!
//! Whoever manages an environment, or the host it runs on, can give up a capability it was granted. The request
//! answers at once with the job that makes the unit anew; what that job does is the environments' part, and until
//! it runs the job waits queued. A capability is granted by a request of an agent that its policy allows or a
//! person of its scope approves (see the engine's `capabilities`), and decided through the approvals.

orch_api_base::api_family_callback! {
    bind __api_bind_51c3f443d6, __api_install_51c3f443d6, __api_callback_51c3f443d6;
    slot __ApiCallback51c3f443d6, __API_CALLBACK_51C3F443D6, __api_set_51c3f443d6;
    base orch_api_base::routes::capabilities;

    fn mount<'__api>(arg0: &'__api mut orch_api_base::registry::Registry) -> ();
}
