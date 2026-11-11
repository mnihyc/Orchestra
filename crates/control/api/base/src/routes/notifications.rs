//! The Inbox and its preferences; channels of the system, a root org and a person with their check, test and texts; a person's routes and the link of one to a bot; push subscriptions; and the destinations of a node.
//!
//! **The Inbox** is the caller's own: their notifications newest first, the
//! counters of the badge, what they read, and what they chose about being
//! told. Everything under `/me` and `/notifications` acts on the caller and
//! on nobody else.
//!
//! **Ways out.** A channel is set up once, by the system for everybody, by a
//! root org for its members or by a person for themself; nothing goes through
//! it to anybody until that person switches their own route on. The fields of
//! a channel are checked against its notifier, its secrets are sealed as the
//! settings' are and never returned, its texts are checked as they are saved,
//! and its notifier looks at the far side before it is kept. A destination is
//! where what happens at a node is sent for nobody in particular, made the
//! same way. The notifiers there are, built in or installed, are given to the
//! requests once by the program that composes the application
//! (`install_notifiers`).

orch_api_base::api_family_callback! {
    bind __api_bind_63f7322eaf, __api_install_63f7322eaf, __api_callback_63f7322eaf;
    slot __ApiCallback63f7322eaf, __API_CALLBACK_63F7322EAF, __api_set_63f7322eaf;
    base orch_api_base::routes::notifications;

    fn push_public_key() -> Option<String>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_4e529c083e, __api_install_4e529c083e, __api_callback_4e529c083e;
    slot __ApiCallback4e529c083e, __API_CALLBACK_4E529C083E, __api_set_4e529c083e;
    base orch_api_base::routes::notifications;

    fn mount<'__api>(arg0: &'__api mut orch_api_base::registry::Registry) -> ();
}

#[path = "notifications/grouping.rs"]
pub mod grouping;

#[path = "notifications/inbox.rs"]
pub mod inbox;

#[path = "notifications/person.rs"]
pub mod person;
