//! Process control and resource samples of an environment.
//!
//! Every process of an environment is listed to whoever may see its processes, whichever thread
//! started it, each with the thread it belongs to when that is known and the caller's own thread's
//! first: a list kept to one thread's processes would suggest a separation that is not there. A
//! signal names the process by its number and by the moment it started, so that a number that was
//! reused is never signalled. How much the environment as a whole used lately is a series of
//! measurements that the API keeps in memory for six hours; they are not stored.

orch_api_base::api_family_callback! {
    bind __api_bind_a36fdefcd0, __api_install_a36fdefcd0, __api_callback_a36fdefcd0;
    slot __ApiCallbacka36fdefcd0, __API_CALLBACK_A36FDEFCD0, __api_set_a36fdefcd0;
    base orch_api_base::routes::processes;

    fn mount<'__api>(arg0: &'__api mut orch_api_base::registry::Registry) -> ();
}
