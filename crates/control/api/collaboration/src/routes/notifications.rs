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

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::Registry;

use crate::routes::resources::sealing;

use orch_api_contract::notifications::AdminNotificationChannelCreate;

use orch_api_contract::notifications::DestinationDelete;

use orch_api_contract::notifications::DestinationTest;

use orch_api_contract::notifications::DestinationUpdate;

use orch_api_contract::notifications::MeNotificationChannelCreate;

use orch_api_contract::notifications::MeNotificationPrefsGet;

use orch_api_contract::notifications::MeNotificationPrefsSet;

use orch_api_contract::notifications::MeNotificationRouteDelete;

use orch_api_contract::notifications::MeNotificationRouteLinkCreate;

use orch_api_contract::notifications::MeNotificationRouteList;

use orch_api_contract::notifications::MeNotificationRouteRemove;

use orch_api_contract::notifications::MeNotificationRouteSet;

use orch_api_contract::notifications::MePushSubscriptionCreate;

use orch_api_contract::notifications::MePushSubscriptionDelete;

use orch_api_contract::notifications::MePushSubscriptionList;

use orch_api_contract::notifications::MePushSubscriptionRemove;

use orch_api_contract::notifications::NodeDestinationCreate;

use orch_api_contract::notifications::NodeDestinationList;

use orch_api_contract::notifications::NotificationChannelDelete;

use orch_api_contract::notifications::NotificationChannelList;

use orch_api_contract::notifications::NotificationChannelTest;

use orch_api_contract::notifications::NotificationChannelUpdate;

use orch_api_contract::notifications::NotificationList;

use orch_api_contract::notifications::NotificationRead;

use orch_api_contract::notifications::NotificationSummaryGet;

use orch_api_contract::notifications::NotifierList;

use orch_api_contract::notifications::OrgNotificationChannelCreate;

use orch_db::secrets::MasterKey;

use orch_worker_core::channels::Notifiers;

use orch_worker_core::channels::Surroundings;

use orch_worker_core::channels::Toolbox;

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
orch_api_base::__api_bind_63f7322eaf!();

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
        .creation(MePushSubscriptionCreate, inbox::push_creation())
        .creation(
            AdminNotificationChannelCreate,
            channels::update_operation::creation::<
                orch_api_contract::notifications::NotificationChannelCreateBody,
            >(),
        )
        .creation(
            OrgNotificationChannelCreate,
            channels::update_operation::creation::<
                orch_api_contract::notifications::NotificationChannelCreateBody,
            >(),
        )
        .creation(
            MeNotificationChannelCreate,
            channels::update_operation::creation::<
                orch_api_contract::notifications::MeNotificationChannelCreateBody,
            >(),
        )
        .on(NotificationList, inbox::list)
        .on(NotificationSummaryGet, inbox::summary_get)
        .on(NotificationRead, inbox::read)
        .on(MeNotificationPrefsGet, inbox::prefs_get)
        .on(MeNotificationPrefsSet, inbox::prefs_set)
        .on_snapshot_read_with_optional_operation(
            orch_api_contract::notifications::MeNotificationExceptionList,
            crate::key_operations::native::selected::<exception_read::Exceptions>,
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
        .on_snapshot_read_with_optional_operation(
            NotificationChannelList,
            crate::key_operations::native::selected::<channels::list_read::Channels>,
            crate::key_operations::native::selected_read::<channels::list_read::Channels>,
        )
        .on(AdminNotificationChannelCreate, channels::create_system)
        .on(OrgNotificationChannelCreate, channels::create_org)
        .on(MeNotificationChannelCreate, channels::create_own)
        .on_with_initial_operation(
            NotificationChannelUpdate,
            crate::key_operations::native::effects::initial::<channels::update_operation::Update>,
            crate::key_operations::native::effects::prepare::<channels::update_operation::Update>,
            channels::update_operation::run,
        )
        .on_with_optional_operation(
            NotificationChannelDelete,
            crate::key_operations::native::stored::<channels::operation::StoredChannel, _, _>,
            channels::delete,
        )
        .on_with_optional_operation(
            NotificationChannelTest,
            crate::key_operations::native::stored::<channels::operation::StoredChannelTest, _, _>,
            channels::test,
        )
        .on(MeNotificationRouteSet, routing::set)
        .creation(MeNotificationRouteLinkCreate, routing::creation::binding())
        .on(MeNotificationRouteLinkCreate, routing::link_create)
        .on(MeNotificationRouteDelete, routing::delete)
        .on(NodeDestinationList, destinations::list)
        .creation(NodeDestinationCreate, destinations::operation::creation())
        .on(NodeDestinationCreate, destinations::create)
        .on_with_operation(
            DestinationUpdate,
            destinations::operation::prepare,
            destinations::update,
        )
        .on_with_operation(
            DestinationDelete,
            destinations::operation::prepare,
            destinations::delete,
        )
        .on_with_operation(
            DestinationTest,
            destinations::operation::prepare,
            destinations::test,
        );
}
orch_api_base::__api_bind_4e529c083e!();

#[path = "notifications/channels.rs"]
pub mod channels;

#[path = "notifications/chat_grants.rs"]
pub mod chat_grants;

#[path = "notifications/destinations.rs"]
pub mod destinations;

#[path = "notifications/document_jobs.rs"]
pub mod document_jobs;

#[path = "notifications/exception_read.rs"]
pub mod exception_read;

#[path = "notifications/exceptions.rs"]
pub mod exceptions;

#[path = "notifications/grouping.rs"]
pub mod grouping;

#[path = "notifications/inbox.rs"]
pub mod inbox;

#[path = "notifications/person.rs"]
pub mod person;

#[path = "notifications/routing.rs"]
pub mod routing;

#[path = "notifications/subjects.rs"]
pub mod subjects;

#[path = "notifications/views.rs"]
pub mod views;
