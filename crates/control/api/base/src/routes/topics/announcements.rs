//! What a person tells the threads of a topic, or of every open topic of a project, and what has been told: the four requests that list and make announcements for people.
//!
//! Every place uses one delivery and reader policy. Each row is one topic's delivery;
//! its group identifier joins rows made by one announcement without hiding their scope.

orch_api_base::api_family_callback! {
    bind __api_bind_e136e90bd5, __api_install_e136e90bd5, __api_callback_e136e90bd5;
    slot __ApiCallbacke136e90bd5, __API_CALLBACK_E136E90BD5, __api_set_e136e90bd5;
    base orch_api_base::routes::topics::announcements;

    fn mount_announcements<'__api>(arg0: &'__api mut orch_api_base::registry::Registry) -> ();
}
