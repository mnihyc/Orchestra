//! Registration, signing in by a password, by a code sent by mail and through an identity provider, the second step, sessions, recovery and the change of an address.
//!
//! The requests that get a person in or give them back their way in are here:
//! registering, signing in, asking for a code, refreshing and ending a
//! session, recovering a password, and confirming an address. What a signed-in
//! person does to their own identity (their profile, password, second steps,
//! sessions and API tokens) is in `me`. Signing in through an identity
//! provider, and linking an identity to a person and unlinking it, are in
//! `oidc`.
//!
//! The requests that answer `202` whatever the name or the address do the
//! same work for every one, so that they tell nothing.

orch_api_base::api_family_callback! {
    bind __api_bind_074cc200b5, __api_install_074cc200b5, __api_callback_074cc200b5;
    slot __ApiCallback074cc200b5, __API_CALLBACK_074CC200B5, __api_set_074cc200b5;
    base orch_api_base::routes::auth;

    fn mount<'__api>(arg0: &'__api mut orch_api_base::registry::Registry) -> ();
}

#[path = "auth/creation.rs"]
pub mod creation;

#[path = "auth/tokens.rs"]
pub mod tokens;
