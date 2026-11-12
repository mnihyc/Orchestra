//! Overview, the environments a topic's threads use, places and announcements.
//!
//! Bound here: the environments of a topic (those its threads run in and those whose home it is, the
//! default first, each as far as the caller may see it), the one that new threads join, the
//! places where one more could be made, and the state behind the topic's feed, which is in `sync`.
//!
//! What people tell the threads of a topic or of a project is in the module `announcements` beside this one.

pub use announcements::mount_announcements;
orch_api_base::api_family_callback! {
    bind __api_bind_0d167a273a, __api_install_0d167a273a, __api_callback_0d167a273a;
    slot __ApiCallback0d167a273a, __API_CALLBACK_0D167A273A, __api_set_0d167a273a;
    base orch_api_base::routes::topics;

    fn mount<'__api>(arg0: &'__api mut orch_api_base::registry::Registry) -> ();
}

#[path = "topics/announcements.rs"]
pub mod announcements;

#[path = "topics/sync.rs"]
pub mod sync;
