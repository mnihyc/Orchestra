//! Management: users and their suspension, a reset link that may switch a second step off, what a new person's own org starts as, root orgs and their two limits, both pools of runtimes and their updates, stored metrics, audit and system state.
//!
//! Everything here is the system's own. Each handler enters the same ordinary administrative
//! operation gate, which checks current system administration and the credential's admission, before it
//! reads or changes anything, so that an admin of an org, who is a system admin of nothing, is
//! refused like anybody else. The acts that change a person's standing are the access engine's
//! (`orch_authz::standing`), the same functions the operator's command line calls, here with the
//! signed-in admin as the actor of the audit line; the others write their line in the transaction
//! that makes the change, so that neither exists without the other.
//!
//! The parts are:
//!
//! - `users` lists and makes people, suspends and lets them in again, makes and unmakes system
//!   admins, and makes the link with which somebody sets a password anew.
//! - `orgs` lists every org and sets the two limits of a root org.
//! - `semantic` says where search by meaning stands for each root org, rebuilds an org's index in place
//!   and makes the provider Local embeddings.
//! - `settings` and `defaults` are the system's settings and its access defaults, which are
//!   the settings engine's and the access engine's with the system as the place.
//! - [`audit`] reads the audit trail of the whole system and writes a span of it as a file.
//! - `system` says how the system stands: its parts, its queues, its blob store, its
//!   certificates, its OpenAPI document and the certificate of its authority.
//! - `fleet` lists the hosts, the runtimes, the stored metrics and the plugins.
//! - [`catalogue`] is the data admins keep: capability definitions, harness builds and the
//!   agent profiles of the system.
//! - `identity_providers` keeps the single sign-in methods.

orch_api_base::api_family_callback! {
    bind __api_bind_6e6c1a7061, __api_install_6e6c1a7061, __api_callback_6e6c1a7061;
    slot __ApiCallback6e6c1a7061, __API_CALLBACK_6E6C1A7061, __api_set_6e6c1a7061;
    base orch_api_base::routes::admin;

    fn mount<'__api>(arg0: &'__api mut orch_api_base::registry::Registry) -> ();
}

orch_api_base::api_family_callback! {
    bind __api_bind_fbe3eb6f13, __api_install_fbe3eb6f13, __api_callback_fbe3eb6f13;
    slot __ApiCallbackfbe3eb6f13, __API_CALLBACK_FBE3EB6F13, __api_set_fbe3eb6f13;
    base orch_api_base::routes::admin;

    fn admin_of<'__api>(arg0: &'__api orch_api_base::registry::Ctx) -> orch_api_base::middleware::requests::ApiResult<orch_types::Id>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_49c603b860, __api_install_49c603b860, __api_callback_49c603b860;
    slot __ApiCallback49c603b860, __API_CALLBACK_49C603B860, __api_set_49c603b860;
    base orch_api_base::routes::admin;

    async fn action<'__api>(arg0: &'__api orch_api_base::registry::Ctx) -> orch_api_base::middleware::requests::ApiResult<(orch_db::Tx, orch_api_base::middleware::transaction_guard::Guard)>;
}

#[path = "admin/audit.rs"]
pub mod audit;

#[path = "admin/catalogue.rs"]
pub mod catalogue;

#[path = "admin/defaults.rs"]
pub mod defaults;
