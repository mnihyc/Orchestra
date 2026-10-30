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
//! ([`install_notifiers`]).

pub mod channels;
mod chat_grants;
pub mod destinations;
pub mod exceptions;
pub mod grouping;
pub mod inbox;
pub(crate) mod person;
pub mod routing;
pub mod views;

use crate::middleware::requests::ApiResult;
use crate::registry::{Ctx, Registry};
use crate::routes::resources::sealing;
use orch_api_contract::notifications::{
    AdminNotificationChannelCreate, DestinationDelete, DestinationTest, DestinationUpdate,
    MeNotificationChannelCreate, MeNotificationPrefsGet, MeNotificationPrefsSet,
    MeNotificationRouteDelete, MeNotificationRouteLinkCreate, MeNotificationRouteList,
    MeNotificationRouteRemove, MeNotificationRouteSet, MePushSubscriptionCreate,
    MePushSubscriptionDelete, MePushSubscriptionList, MePushSubscriptionRemove,
    NodeDestinationCreate, NodeDestinationList, NotificationChannelDelete, NotificationChannelList,
    NotificationChannelTest, NotificationChannelUpdate, NotificationList, NotificationRead,
    NotificationSummaryGet, NotifierList, OrgNotificationChannelCreate,
};
use orch_db::secrets::MasterKey;
use orch_worker_core::channels::{Notifiers, Surroundings, Toolbox};
use std::sync::OnceLock;

static NOTIFIERS: OnceLock<Notifiers> = OnceLock::new();
static PUSH_PUBLIC_KEY: OnceLock<String> = OnceLock::new();

/// Gives the requests the public half of the deployment's Web Push key, the one the worker
/// signs push with: what a browser subscribes with. Its private half never reaches a request.
/// Called once by the program that composes the application where Web Push is set up.
pub fn install_push_public_key(key: String) -> bool {
    PUSH_PUBLIC_KEY.set(key).is_ok()
}

/// The public half of the Web Push key, where the deployment has one.
pub(crate) fn push_public_key() -> Option<String> {
    PUSH_PUBLIC_KEY.get().cloned()
}

/// Gives the requests the notifiers there are: the built-in ones and any that
/// were installed. Called once by the program that composes the application,
/// and by tests; the first set given stays.
pub fn install_notifiers(notifiers: Notifiers) -> bool {
    NOTIFIERS.set(notifiers).is_ok()
}

/// The notifiers the requests know; none until some were given.
pub(crate) fn notifiers() -> &'static Notifiers {
    static NONE: OnceLock<Notifiers> = OnceLock::new();
    NOTIFIERS
        .get()
        .unwrap_or_else(|| NONE.get_or_init(Notifiers::new))
}

/// What the application knows of itself that a sample message is written with.
pub(crate) fn surroundings(ctx: &Ctx) -> Surroundings {
    Surroundings {
        system_name: ctx.app.config.system.name.clone(),
        public_url: ctx.app.config.system.public_url.clone(),
    }
}

/// The key that seals the secrets of channels and destinations: the one every
/// place that seals a secret is given.
pub(crate) fn master_key() -> ApiResult<&'static MasterKey> {
    sealing::master_key()
}

/// What trying a channel or a destination takes besides the channel itself.
pub(crate) fn toolbox<'a>(master: &'a MasterKey, surroundings: &'a Surroundings) -> Toolbox<'a> {
    Toolbox {
        master,
        notifiers: notifiers(),
        surroundings,
    }
}

/// The kind of notification as the word the schema and the wire write.
pub(crate) fn kind_word(kind: orch_api_contract::notifications::NotificationKind) -> String {
    serde_json::to_value(kind)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    person::mount(registry);
    chat_grants::mount(registry);
    registry
        .on(NotificationList, inbox::list)
        .on(NotificationSummaryGet, inbox::summary_get)
        .on(NotificationRead, inbox::read)
        .on(MeNotificationPrefsGet, inbox::prefs_get)
        .on(MeNotificationPrefsSet, inbox::prefs_set)
        .on(
            orch_api_contract::notifications::MeNotificationExceptionList,
            exceptions::list,
        )
        .on(
            orch_api_contract::notifications::MeNotificationExceptionSet,
            exceptions::change,
        )
        .on(MePushSubscriptionCreate, inbox::push_create)
        .on(MePushSubscriptionDelete, inbox::push_delete)
        .on(MePushSubscriptionList, inbox::push_list)
        .on(MePushSubscriptionRemove, inbox::push_remove)
        .on(MeNotificationRouteList, routing::list)
        .on(MeNotificationRouteRemove, routing::remove)
        .on(NotifierList, inbox::notifier_list)
        .on(NotificationChannelList, channels::list)
        .on(AdminNotificationChannelCreate, channels::create_system)
        .on(OrgNotificationChannelCreate, channels::create_org)
        .on(MeNotificationChannelCreate, channels::create_own)
        .on(NotificationChannelUpdate, channels::update)
        .on(NotificationChannelDelete, channels::delete)
        .on(NotificationChannelTest, channels::test)
        .on(MeNotificationRouteSet, routing::set)
        .on(MeNotificationRouteLinkCreate, routing::link_create)
        .on(MeNotificationRouteDelete, routing::delete)
        .on(NodeDestinationList, destinations::list)
        .on(NodeDestinationCreate, destinations::create)
        .on(DestinationUpdate, destinations::update)
        .on(DestinationDelete, destinations::delete)
        .on(DestinationTest, destinations::test);
}
