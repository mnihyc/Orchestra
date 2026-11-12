//! Favorites, recents and read/subscription state; what the interface keeps for a person.
//!
//! Everything here acts on the caller and on nobody else. A favorite or a
//! recent is a kind/id the caller may view; one they lost the right to is no
//! longer listed. The interface state is kept under nine keys and no
//! others, whole, and no value is larger than the limit; the last view of a
//! project is also kept one project at a time, beside the others. Drafts and the
//! chat states belong to the threads and the chats, and the read position
//! of a thread to the threads routes: none of the three is bound here.

orch_api_base::api_family_callback! {
    bind __api_bind_3346c39c88, __api_install_3346c39c88, __api_callback_3346c39c88;
    slot __ApiCallback3346c39c88, __API_CALLBACK_3346C39C88, __api_set_3346c39c88;
    base orch_api_base::routes::personal;

    fn mount<'__api>(arg0: &'__api mut orch_api_base::registry::Registry) -> ();
}

#[path = "personal/viewed.rs"]
pub mod viewed;

#[path = "personal/native.rs"]
pub mod native;
