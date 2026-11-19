//! The kinds of channel there are, built in and installed, and the channel a delivery goes through: the system's, a root org's or a person's own.
//!
//! A **kind** is a notifier, found by the name a channel gives as its kind.
//! The program that composes the worker registers the built-in ones and any
//! that were installed; the library below it knows only the contract. A
//! **channel** is the values of one such kind, set up by the system, by a root
//! org or by a person; its secret values are sealed in the database and are
//! unsealed here for the length of one call.
//!
//! Which channel a message goes through is the channel it names, for a
//! notification (the route names it) and for mail that is no notification
//! (the row names it). Where a writer has to choose, [`mail_channel_for`] is
//! the rule: the mail server of the root org, else the system's if the system
//! lets it be shared, else none; and what concerns a person's own user never
//! goes by an org's.
//!
//! A channel is looked at when it is saved ([`check_before_save`]) and can be
//! tried ([`send_test`]); the routes of the HTTP API call both.

use crate::render::{self, Rendering};
use chrono::{DateTime, Utc};
use orch_db::queries::notifications::channels::{self as rows, ChannelRow, DestinationRow, owner};
use orch_db::queries::notifications::outbox::Snapshot;
use orch_db::queries::notifications::people::{self, PushSubscriptionRow, Recipient};
use orch_db::queries::settings;
use orch_db::secrets::{MasterKey, SealError};
use orch_db::{Conn, DbError};
use orch_notifier_contract::delivery::{Config, DeliverError, Delivery, Saved, on_check};
use orch_notifier_contract::descriptor::{DescriptorFault, NotifierDescriptor, Reaches, Target};
use orch_notifier_contract::notifier::Notifier;
use orch_notifier_contract::templates::{TemplateOverride, TextFault, check_texts};
use orch_plugin_contract::Secret;
use orch_settings::Place;
use orch_types::fields::FieldError;
use orch_types::{Id, Json};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

/// The longest a notifier is given for one delivery or one look at a channel,
/// whatever it says it will take: the notifiers set their own, shorter, limits,
/// and this is what stops one that does not from holding a claim.
pub const DELIVERY_DEADLINE: Duration = Duration::from_secs(300);

/// Why a channel could not be loaded or tried.
#[derive(Debug, thiserror::Error)]
pub enum ChannelError {
    /// The database refused.
    #[error(transparent)]
    Db(#[from] DbError),
    /// A setting could not be read.
    #[error(transparent)]
    Setting(#[from] orch_settings::Error),
    /// A secret does not open: it was sealed under another key, for another
    /// row, or it is damaged.
    #[error("the secret {name} of a channel does not open")]
    Sealed {
        /// The secret's name.
        name: String,
    },
    /// No notifier of that name is registered.
    #[error("there is no notifier named {0}")]
    UnknownKind(String),
}

/// A notifier that cannot be registered.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegisterError {
    /// A notifier of that name is registered already.
    #[error("a notifier named {0} is registered already")]
    Taken(String),
    /// What it declares does not hold together.
    #[error("what {name} declares is not consistent: {faults:?}")]
    Inconsistent {
        /// Its name.
        name: String,
        /// Every fault.
        faults: Vec<DescriptorFault>,
    },
}

/// The notifiers there are, by name.
#[derive(Clone, Default)]
pub struct Notifiers {
    by_name: BTreeMap<String, Arc<dyn Notifier>>,
}

impl std::fmt::Debug for Notifiers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.by_name.keys()).finish()
    }
}

impl Notifiers {
    /// No notifiers.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a notifier under the name it declares. Its declaration is
    /// checked first, the texts it ships included.
    pub fn register(&mut self, notifier: Arc<dyn Notifier>) -> Result<(), RegisterError> {
        let declared = notifier.describe();
        if let Err(faults) = declared.check() {
            return Err(RegisterError::Inconsistent {
                name: declared.name,
                faults,
            });
        }
        if self.by_name.contains_key(&declared.name) {
            return Err(RegisterError::Taken(declared.name));
        }
        self.by_name.insert(declared.name, notifier);
        Ok(())
    }

    /// The notifier named `name`.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Arc<dyn Notifier>> {
        self.by_name.get(name)
    }

    /// What every notifier declares, in the order of their names.
    #[must_use]
    pub fn descriptors(&self) -> Vec<NotifierDescriptor> {
        self.by_name.values().map(|each| each.describe()).collect()
    }
}

/// What a secret is bound to, so that a sealed value copied to another row
/// does not open there. The same text the settings of nodes and environments
/// bind theirs with.
#[must_use]
pub fn secret_binding(owner_kind: &str, owner_id: Id, name: &str) -> Vec<u8> {
    format!("secrets/{owner_kind}/{owner_id}/{name}").into_bytes()
}

/// Seals the value of a secret field of a channel (`owner::CHANNEL`) or a
/// destination (`owner::DESTINATION`), to be stored under `name`.
pub fn seal_secret(
    master: &MasterKey,
    owner_kind: &str,
    owner_id: Id,
    name: &str,
    value: &str,
) -> Result<Vec<u8>, SealError> {
    master.seal(
        value.as_bytes(),
        &secret_binding(owner_kind, owner_id, name),
    )
}

/// Opens the secrets of a channel or a destination, by their names.
pub async fn open_secrets(
    conn: &mut Conn,
    master: &MasterKey,
    owner_kind: &str,
    owner_id: Id,
) -> Result<BTreeMap<String, Secret>, ChannelError> {
    let mut opened = BTreeMap::new();
    for row in settings::secrets_of(conn, &[(owner_kind, owner_id)]).await? {
        // The codes of links that wait to be pressed are kept with the channel, and are no
        // field of its notifier's.
        if row.name.starts_with(crate::bot_links::LINK_SECRET_PREFIX) {
            continue;
        }
        let plain = master
            .open(
                &row.sealed,
                &secret_binding(owner_kind, owner_id, &row.name),
            )
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .ok_or_else(|| ChannelError::Sealed {
                name: row.name.clone(),
            })?;
        opened.insert(row.name, Secret::new(plain));
    }
    Ok(opened)
}

/// The hosts on loopback, link-local and private addresses that what an org or
/// a person set up may reach (`outbound.allowed_private_hosts`).
pub async fn outbound_hosts(conn: &mut Conn) -> Result<Vec<String>, ChannelError> {
    Ok(
        orch_settings::get::<Vec<String>>(conn, Place::System, "outbound.allowed_private_hosts")
            .await?
            .unwrap_or_default(),
    )
}

/// Whether a root org that has no mail server of its own may send its mail
/// through the system's (`mail.shared`).
pub async fn mail_may_be_shared(conn: &mut Conn) -> Result<bool, ChannelError> {
    Ok(
        orch_settings::get::<bool>(conn, Place::System, "mail.shared")
            .await?
            .unwrap_or(true),
    )
}

/// The values of a channel for one call: its fields, its secrets unsealed,
/// and the list of private hosts it is held to, which is none for the
/// system's own.
pub async fn config_of_channel(
    conn: &mut Conn,
    master: &MasterKey,
    channel: &ChannelRow,
) -> Result<Config, ChannelError> {
    let allowed = if channel.is_the_systems() {
        None
    } else {
        Some(outbound_hosts(conn).await?)
    };
    Ok(Config {
        values: channel.config.clone(),
        secrets: open_secrets(conn, master, owner::CHANNEL, channel.id).await?,
        allowed_private_hosts: allowed,
    })
}

/// The values of a destination for one call. A destination is set up by whoever
/// manages a node, so it is always held to the list of private hosts.
pub async fn config_of_destination(
    conn: &mut Conn,
    master: &MasterKey,
    destination: &DestinationRow,
) -> Result<Config, ChannelError> {
    Ok(Config {
        values: destination.config.clone(),
        secrets: open_secrets(conn, master, owner::DESTINATION, destination.id).await?,
        allowed_private_hosts: Some(outbound_hosts(conn).await?),
    })
}

/// The mail server that mail goes by.
///
/// For mail about a root org's own matters (an invitation into it) `org` is
/// that root org: its mail server where it has one in use, else the system's
/// where the system lets mail be shared, else none. For mail that concerns a
/// person's own user (a confirmation, a reset, a code, a notice of a changed
/// address) `org` is none and the system's mail server is the only one.
pub async fn mail_channel_for(
    conn: &mut Conn,
    org: Option<Id>,
) -> Result<Option<ChannelRow>, ChannelError> {
    if let Some(org) = org {
        if let Some(channel) = rows::org_channel(conn, org, "email").await?
            && channel.enabled
        {
            return Ok(Some(channel));
        }
        if !mail_may_be_shared(conn).await? {
            return Ok(None);
        }
    }
    Ok(rows::system_channel(conn, "email")
        .await?
        .filter(|channel| channel.enabled))
}

/// What is wrong with what a person gave for a channel or a destination.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum SaveFault {
    /// There is no notifier of that name.
    #[error("there is no notifier named {0}")]
    UnknownKind(String),
    /// The notifier cannot be what was asked for.
    #[error("{kind} cannot be set up for this")]
    WrongTarget {
        /// The notifier.
        kind: String,
    },
    /// A field is missing or wrong.
    #[error("the fields do not fit: {0:?}")]
    Fields(Vec<FieldError>),
    /// A text does not pass the check a saved text passes.
    #[error("a text is wrong: {0:?}")]
    Texts(Vec<TextFault>),
}

/// What a person gave for a channel or a destination.
#[derive(Debug, Clone, Copy)]
pub struct Given<'a> {
    /// The notifier.
    pub kind: &'a str,
    /// A channel, which reaches a person, or a destination, which a node is
    /// told at.
    pub target: Target,
    /// The fields as an object, the secret ones by their names.
    pub fields: &'a Json,
    /// The texts of its own, by language.
    pub templates: &'a BTreeMap<String, TemplateOverride>,
}

/// What looking at a channel found.
#[derive(Debug, Clone, PartialEq)]
pub struct Checked {
    /// The fields split into the values and the secrets, with the list of
    /// private hosts the channel is held to.
    pub config: Config,
    /// What the notifier decided: the channel is kept (with what the far side
    /// said of itself), kept with an error shown, or refused.
    pub saved: Saved,
}

impl Checked {
    /// The values to store: those the person gave, and, if the far side said
    /// something of itself that is an object, its members beside them (a bot's
    /// name).
    #[must_use]
    pub fn values_to_store(&self) -> Json {
        let mut values = self.config.values.clone();
        if let (Saved::Kept(Json::Object(remote)), Json::Object(own)) = (&self.saved, &mut values) {
            for (name, member) in remote {
                own.insert(name.clone(), member.clone());
            }
        }
        values
    }
}

/// Looks at a channel or a destination before it is saved: the fields are
/// checked against what the notifier asks for, its texts against the rules a
/// saved text meets, and then the notifier looks at the far side. What the far
/// side refuses for good refuses the save; one that cannot be reached now does
/// not, and the error is shown on the channel.
pub async fn check_before_save(
    notifiers: &Notifiers,
    given: &Given<'_>,
    allowed: Option<Vec<String>>,
) -> Result<Checked, SaveFault> {
    let notifier = notifiers
        .get(given.kind)
        .ok_or_else(|| SaveFault::UnknownKind(given.kind.to_owned()))?;
    let declared = notifier.describe();
    if !declared.targets.contains(&given.target) {
        return Err(SaveFault::WrongTarget {
            kind: given.kind.to_owned(),
        });
    }
    let config =
        Config::from_answer(&declared.fields, given.fields, allowed).map_err(SaveFault::Fields)?;
    check_texts(given.templates, declared.body).map_err(SaveFault::Texts)?;
    let saved = on_check(guarded(notifier.check(&config)).await);
    Ok(Checked { config, saved })
}

/// Waits for a notifier no longer than [`DELIVERY_DEADLINE`].
pub(crate) async fn guarded<T>(
    call: impl std::future::Future<Output = Result<T, DeliverError>>,
) -> Result<T, DeliverError> {
    match tokio::time::timeout(DELIVERY_DEADLINE, call).await {
        Ok(result) => result,
        Err(_) => Err(DeliverError::temporary(
            "timeout",
            "the notifier did not answer in time",
        )),
    }
}

/// What a sample message came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestOutcome {
    /// Whether the far side took it.
    pub ok: bool,
    /// What the far side answered, when it did not.
    pub error: Option<String>,
}

impl TestOutcome {
    pub(crate) fn of(result: Result<(), DeliverError>) -> Self {
        match result {
            Ok(()) => Self {
                ok: true,
                error: None,
            },
            Err(error) => Self {
                ok: false,
                error: Some(error.message),
            },
        }
    }

    pub(crate) fn failed(reason: &str) -> Self {
        Self {
            ok: false,
            error: Some(reason.to_owned()),
        }
    }
}

/// What the worker knows of itself that a message is written with.
#[derive(Debug, Clone)]
pub struct Surroundings {
    /// What the system calls itself.
    pub system_name: String,
    /// The origin a browser opens.
    pub public_url: String,
}

/// The notification a sample message tells.
pub(crate) fn sample(env: &Surroundings, channel_name: &str, now: DateTime<Utc>) -> Snapshot {
    Snapshot {
        id: Some(orch_types::new_id()),
        kind: "milestone".to_owned(),
        at: now.into(),
        title: format!("Test message from {}", env.system_name),
        body: format!("This is a test of {channel_name}."),
        node_id: None,
        thread_id: None,
        approval_id: None,
        chat_id: None,
        data: serde_json::json!({ "actor": env.system_name }),
    }
}

/// The deliveries a sample message comes to, one for each place the person is
/// reached at on this channel: an address for mail, each browser for push, the
/// chat of a bot, the channel's own address for a webhook.
async fn reach(
    conn: &mut Conn,
    reaches: Reaches,
    caller: &Recipient,
    route_address: Option<&Json>,
    mail_to: Option<&str>,
) -> Result<Vec<Json>, TestOutcome> {
    match reaches {
        Reaches::Email => {
            let address = mail_to
                .map(str::to_owned)
                .or_else(|| caller.confirmed_email().map(str::to_owned));
            match address {
                Some(address) => Ok(vec![serde_json::json!({ "email": address })]),
                None => Err(TestOutcome::failed(
                    "there is no address to send to: give one, or confirm yours",
                )),
            }
        }
        Reaches::Devices => {
            let browsers = people::push_subscriptions_of(conn, caller.id)
                .await
                .map_err(|error| TestOutcome::failed(&error.to_string()))?;
            if browsers.is_empty() {
                return Err(TestOutcome::failed("no browser of yours takes push"));
            }
            Ok(browsers.iter().map(device_address).collect())
        }
        Reaches::Link => match route_address {
            Some(address)
                if address
                    .as_object()
                    .is_some_and(|members| !members.is_empty()) =>
            {
                Ok(vec![address.clone()])
            }
            _ => Err(TestOutcome::failed("your route through it is not linked")),
        },
        Reaches::Channel => Ok(vec![serde_json::json!({})]),
    }
}

/// The address a browser is reached at.
#[must_use]
pub fn device_address(browser: &PushSubscriptionRow) -> Json {
    serde_json::json!({
        "id": browser.id, "endpoint": browser.endpoint,
        "keys": { "p256dh": browser.p256dh, "auth": browser.auth }
    })
}

/// What sending a sample takes besides the channel: the key that opens its
/// secrets, the notifiers there are, and what the worker knows of itself.
#[derive(Clone, Copy)]
pub struct Toolbox<'a> {
    /// The key that opens sealed secrets.
    pub master: &'a MasterKey,
    /// The notifiers there are.
    pub notifiers: &'a Notifiers,
    /// What messages are written with.
    pub surroundings: &'a Surroundings,
}

/// Whom a sample message is for.
#[derive(Clone, Copy)]
pub struct TestFor<'a> {
    /// The channel.
    pub channel: &'a ChannelRow,
    /// The person who asked.
    pub caller: &'a Recipient,
    /// Where their route through the channel has them, once it is linked.
    pub route_address: Option<&'a Json>,
    /// For a mail server, the address to send to, where it is not the caller's.
    pub to: Option<&'a str>,
    /// For push, one browser of the caller's own; every browser of theirs otherwise.
    pub device: Option<Id>,
}

/// A sample message made ready, to be sent with no connection or lock held.
pub struct PreparedTest {
    notifier: Arc<dyn Notifier>,
    deliveries: Vec<Delivery>,
}

/// What getting a sample message ready came to.
pub enum TestPrepared {
    /// It is ready to go.
    Ready(PreparedTest),
    /// It cannot go, and says why.
    Done(TestOutcome),
}

/// Sends one sample message through a channel, as it would reach the caller
/// through their route, or, for a mail server, to the address given. What the
/// far side answers is returned when it does not take the message. The caller
/// that holds a connection uses [`prepare_test`] and [`run_test`] instead, and
/// lets the connection go between the two.
pub async fn send_test(
    conn: &mut Conn,
    tools: &Toolbox<'_>,
    test: &TestFor<'_>,
    now: DateTime<Utc>,
) -> Result<TestOutcome, ChannelError> {
    match prepare_test(conn, tools, test, now).await? {
        TestPrepared::Done(outcome) => Ok(outcome),
        TestPrepared::Ready(ready) => Ok(run_test(ready).await),
    }
}

/// Sends a sample message that was made ready: the far side is called with nothing of the
/// database held.
pub async fn run_test(ready: PreparedTest) -> TestOutcome {
    let mut first_error = None;
    for delivery in ready.deliveries {
        if let Err(error) = guarded(ready.notifier.deliver(delivery)).await {
            first_error.get_or_insert(error.message);
        }
    }
    match first_error {
        None => TestOutcome::of(Ok(())),
        Some(error) => TestOutcome::failed(&error),
    }
}

/// Makes a sample message through a channel ready: what it says, and where it goes.
pub async fn prepare_test(
    conn: &mut Conn,
    tools: &Toolbox<'_>,
    test: &TestFor<'_>,
    now: DateTime<Utc>,
) -> Result<TestPrepared, ChannelError> {
    let channel = test.channel;
    let notifier = tools
        .notifiers
        .get(&channel.kind)
        .cloned()
        .ok_or_else(|| ChannelError::UnknownKind(channel.kind.clone()))?;
    let declared = notifier.describe();
    let config = config_of_channel(conn, tools.master, channel).await?;
    let mut addresses = match reach(
        conn,
        declared.reaches,
        test.caller,
        test.route_address,
        test.to,
    )
    .await
    {
        Ok(addresses) => addresses,
        Err(outcome) => return Ok(TestPrepared::Done(outcome)),
    };
    if let Some(device) = test.device {
        let wanted = device.to_string();
        addresses
            .retain(|address| address.get("id").and_then(Json::as_str) == Some(wanted.as_str()));
        if addresses.is_empty() {
            return Ok(TestPrepared::Done(TestOutcome::failed(
                "that browser is not one of yours that takes push",
            )));
        }
    }
    let notification = [sample(tools.surroundings, &channel.name, now)];
    let message = match render::render(
        orch_types::new_id(),
        &Rendering {
            descriptor: &declared,
            own_texts: &channel.templates,
            language: &test.caller.locale,
            recipient: Some(test.caller),
            notifications: &notification,
            places: &[],
            org_name: &tools.surroundings.system_name,
            public_url: &tools.surroundings.public_url,
        },
    ) {
        Ok(message) => message,
        Err(fault) => return Ok(TestPrepared::Done(TestOutcome::failed(&fault.to_string()))),
    };
    let deliveries = addresses
        .into_iter()
        .map(|address| Delivery {
            id: orch_types::new_id(),
            attempt: 1,
            config: config.clone(),
            to: address,
            recipient: Some(render::user_ref(test.caller)),
            language: test.caller.locale.clone(),
            subject: message.subject.clone(),
            body: message.body.clone(),
            notifications: message.notifications.clone(),
        })
        .collect();
    Ok(TestPrepared::Ready(PreparedTest {
        notifier,
        deliveries,
    }))
}

/// Sends one sample notification to a destination.
pub async fn send_test_to_destination(
    conn: &mut Conn,
    tools: &Toolbox<'_>,
    destination: &DestinationRow,
    now: DateTime<Utc>,
) -> Result<TestOutcome, ChannelError> {
    match prepare_destination_test(conn, tools, destination, now).await? {
        TestPrepared::Done(outcome) => Ok(outcome),
        TestPrepared::Ready(ready) => Ok(run_test(ready).await),
    }
}

/// Makes a sample notification to a destination ready.
pub async fn prepare_destination_test(
    conn: &mut Conn,
    tools: &Toolbox<'_>,
    destination: &DestinationRow,
    now: DateTime<Utc>,
) -> Result<TestPrepared, ChannelError> {
    let notifier = tools
        .notifiers
        .get(&destination.channel)
        .cloned()
        .ok_or_else(|| ChannelError::UnknownKind(destination.channel.clone()))?;
    let declared = notifier.describe();
    let config = config_of_destination(conn, tools.master, destination).await?;
    let env = tools.surroundings;
    let notification = [sample(env, &destination.name, now)];
    let message = match render::render(
        orch_types::new_id(),
        &Rendering {
            descriptor: &declared,
            own_texts: &destination.templates,
            language: render::DESTINATION_LANGUAGE,
            recipient: None,
            notifications: &notification,
            places: &[],
            org_name: &env.system_name,
            public_url: &env.public_url,
        },
    ) {
        Ok(message) => message,
        Err(fault) => return Ok(TestPrepared::Done(TestOutcome::failed(&fault.to_string()))),
    };
    let delivery = Delivery {
        id: orch_types::new_id(),
        attempt: 1,
        config,
        to: serde_json::json!({}),
        recipient: None,
        language: render::DESTINATION_LANGUAGE.to_owned(),
        subject: message.subject,
        body: message.body,
        notifications: message.notifications,
    };
    Ok(TestPrepared::Ready(PreparedTest {
        notifier,
        deliveries: vec![delivery],
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use orch_notifier_contract::delivery::LinkPoll;
    use orch_notifier_contract::descriptor::Owner;
    use orch_notifier_contract::templates::{BodyKind, TemplatePair, TemplateText};
    use orch_types::{Field, FieldType};
    use serde_json::json;

    struct Fake {
        name: &'static str,
        check: Result<Json, DeliverError>,
    }

    #[async_trait]
    impl Notifier for Fake {
        fn describe(&self) -> NotifierDescriptor {
            let text = |body: &str| TemplateText {
                subject: None,
                body: body.to_owned(),
            };
            let mut token = Field::new("token", "Token", FieldType::Secret);
            token.required = true;
            NotifierDescriptor {
                name: self.name.to_owned(),
                title: "Fake".into(),
                summary: "x".into(),
                targets: vec![Target::Person],
                fields: vec![Field::new("url", "URL", FieldType::Text), token],
                owners: vec![Owner::System],
                single: false,
                reaches: Reaches::Channel,
                body: BodyKind::Text,
                max_chars: None,
                templates: BTreeMap::from([(
                    "*".to_owned(),
                    TemplatePair {
                        single: text("{{title}}"),
                        digest: text("{{count}}"),
                    },
                )]),
                extensions: orch_plugin_contract::Extensions::new(),
            }
        }
        async fn check(&self, _: &Config) -> Result<Json, DeliverError> {
            self.check.clone()
        }
        async fn deliver(&self, _: Delivery) -> Result<(), DeliverError> {
            Ok(())
        }
        async fn poll_links(
            &self,
            _: &Config,
            _: Option<String>,
            _: Duration,
        ) -> Result<LinkPoll, DeliverError> {
            Ok(LinkPoll::default())
        }
    }

    fn registry(check: Result<Json, DeliverError>) -> Notifiers {
        let mut notifiers = Notifiers::new();
        notifiers
            .register(Arc::new(Fake {
                name: "fake",
                check,
            }))
            .unwrap();
        notifiers
    }

    fn given<'a>(fields: &'a Json, templates: &'a BTreeMap<String, TemplateOverride>) -> Given<'a> {
        Given {
            kind: "fake",
            target: Target::Person,
            fields,
            templates,
        }
    }

    #[test]
    fn a_notifier_is_registered_once_under_the_name_it_declares_and_found_by_it() {
        let mut notifiers = registry(Ok(Json::Null));
        assert!(notifiers.get("fake").is_some() && notifiers.get("other").is_none());
        assert_eq!(
            notifiers.register(Arc::new(Fake {
                name: "fake",
                check: Ok(Json::Null)
            })),
            Err(RegisterError::Taken("fake".into()))
        );
        assert_eq!(notifiers.descriptors().len(), 1);
    }

    #[test]
    fn a_secret_is_bound_to_its_owner_and_its_name_as_the_settings_of_nodes_bind_theirs() {
        let owner_id: Id = "0199abcd-1234-7000-8000-0000000000a1".parse().unwrap();
        assert_eq!(
            String::from_utf8(secret_binding("notification_channel", owner_id, "password"))
                .unwrap(),
            "secrets/notification_channel/0199abcd-1234-7000-8000-0000000000a1/password"
        );
        let key = MasterKey::new([9; 32]);
        let sealed = seal_secret(&key, owner::CHANNEL, owner_id, "password", "s3cr3t").unwrap();
        let opened = key
            .open(
                &sealed,
                &secret_binding(owner::CHANNEL, owner_id, "password"),
            )
            .unwrap();
        assert_eq!(opened, b"s3cr3t");
        assert!(
            key.open(&sealed, &secret_binding(owner::CHANNEL, owner_id, "other"))
                .is_err()
        );
        assert!(
            key.open(
                &sealed,
                &secret_binding(owner::DESTINATION, owner_id, "password")
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn what_the_far_side_says_of_itself_is_kept_and_a_refusal_for_good_refuses_the_save() {
        let templates = BTreeMap::new();
        let fields = json!({ "url": "https://x.example", "token": "good" });
        let kept = check_before_save(
            &registry(Ok(json!({ "bot_username": "orch_bot" }))),
            &given(&fields, &templates),
            None,
        )
        .await
        .unwrap();
        assert_eq!(
            kept.saved,
            Saved::Kept(json!({ "bot_username": "orch_bot" }))
        );
        assert_eq!(
            kept.values_to_store(),
            json!({ "url": "https://x.example", "bot_username": "orch_bot" }),
            "a secret is not among the values"
        );
        assert_eq!(
            kept.config.secret("token").map(Secret::expose),
            Some("good")
        );

        let refused = DeliverError::refused("unauthorized", "the token is not valid");
        let result = check_before_save(
            &registry(Err(refused.clone())),
            &given(&fields, &templates),
            None,
        )
        .await
        .unwrap();
        assert_eq!(result.saved, Saved::Refused(refused));

        let down = DeliverError::temporary("unreachable", "the server is down today");
        let result = check_before_save(
            &registry(Err(down.clone())),
            &given(&fields, &templates),
            Some(vec![]),
        )
        .await
        .unwrap();
        assert_eq!(result.saved, Saved::KeptWithError(down));
        assert_eq!(result.config.allowed_private_hosts, Some(vec![]));
    }

    #[tokio::test]
    async fn fields_and_texts_that_do_not_fit_are_refused_by_name_before_the_far_side_is_asked() {
        let notifiers = registry(Ok(Json::Null));
        let templates = BTreeMap::new();
        let missing = json!({ "token": "t" });
        let Err(SaveFault::Fields(faults)) =
            check_before_save(&notifiers, &given(&missing, &templates), None).await
        else {
            panic!("a missing field must be refused");
        };
        assert_eq!(faults[0].path, "url");
        let fields = json!({ "url": "u", "token": "t" });
        let texts = BTreeMap::from([(
            "*".to_owned(),
            TemplateOverride {
                single: Some(TemplateText {
                    subject: None,
                    body: "{{password}}".into(),
                }),
                digest: None,
            },
        )]);
        let Err(SaveFault::Texts(faults)) =
            check_before_save(&notifiers, &given(&fields, &texts), None).await
        else {
            panic!("a placeholder outside the list must be refused");
        };
        assert!(faults[0].to_string().contains("password"));
        let mut unknown = given(&fields, &templates);
        unknown.kind = "nope";
        assert_eq!(
            check_before_save(&notifiers, &unknown, None).await,
            Err(SaveFault::UnknownKind("nope".into()))
        );
        let mut destination = given(&fields, &templates);
        destination.target = Target::Destination;
        assert!(matches!(
            check_before_save(&notifiers, &destination, None).await,
            Err(SaveFault::WrongTarget { .. })
        ));
    }
}
