//! Users, and invitations into an org at any level, to one place or the system's: a link to share or a personal link, within what the root org allows.
//!
//! People are found by name among those who share an org with the caller,
//! and by an exact username or address anywhere, never all of them. An
//! invitation is a link: a shared one that up to a number of people may use,
//! or a personal one for one address. What a root org allows (which forms,
//! how long, how many) is its policy, resolved where the invitation points;
//! accepting an org invitation makes the org the person's domain, a node
//! invitation names them on one place's page without making them a member of
//! anything. Whoever made one must still be able to name an outsider where it
//! points at the moment it is accepted. A group invitation adds a guest to that chat alone.

orch_api_base::api_family_callback! {
    bind __api_bind_43faf5f5b7, __api_install_43faf5f5b7, __api_callback_43faf5f5b7;
    slot __ApiCallback43faf5f5b7, __API_CALLBACK_43FAF5F5B7, __api_set_43faf5f5b7;
    base orch_api_base::routes::users;

    fn mount<'__api>(arg0: &'__api mut orch_api_base::registry::Registry) -> ();
}
