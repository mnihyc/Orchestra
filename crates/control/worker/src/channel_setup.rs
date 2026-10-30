//! A channel of the system set up from its fields on the host, and a sample sent through it with no person behind it.
//!
//! This is what an operator does with `orchctl channel put` and `channel
//! test` when nobody can use Management, or a script sets a system up. A
//! channel is **put**: made if the system has none of that kind (or none of
//! that name, for the kinds that can be several, such as a webhook), else
//! replaced by the fields given. The fields are looked at as the notifier asks
//! ([`check_before_save`]) before anything is written: what the far side
//! refuses for good refuses the save, and a far side that cannot be reached
//! now does not, and is kept as the error of the channel. Its secret fields
//! are sealed as they are stored, bound to the channel and their names as
//! every sealed secret is; the ones the new fields lack are removed. The
//! audit trail names whoever put it.

use crate::channels::{
    ChannelError, Given, Notifiers, SaveFault, TestOutcome, Toolbox, check_before_save,
    config_of_channel, guarded, sample, seal_secret,
};
use crate::render::{self, DESTINATION_LANGUAGE, Rendering};
use chrono::{DateTime, Utc};
use orch_access_contract::audit::AuditAction;
use orch_db::audit::{self, Actor, Entry, Origin};
use orch_db::queries::notifications::channels::{self as rows, ChannelRow, NewChannel, owner};
use orch_db::queries::settings;
use orch_db::secrets::MasterKey;
use orch_db::{Conn, Db, DbError};
use orch_notifier_contract::delivery::{DeliverError, Delivery, Saved};
use orch_notifier_contract::descriptor::{NotifierDescriptor, Owner, Reaches, Target};
use orch_types::{Id, Json};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

/// The most characters of the name of a channel: what the schema allows.
pub const NAME_MAX_CHARS: usize = 80;

/// What an operator gives to set a channel of the system up.
#[derive(Debug, Clone, Copy)]
pub struct SystemChannel<'a> {
    /// The notifier: `email`, `telegram`, `webhook`, or one that was installed.
    pub kind: &'a str,
    /// What it is called. Left out, a channel that is there keeps its name and
    /// a new one is named for its notifier. For the kinds that can be several,
    /// the name also says which is meant.
    pub name: Option<&'a str>,
    /// The notifier's fields as an object, the secret ones among them by their
    /// names.
    pub fields: &'a Json,
    /// The person a new channel is set up by, as the schema keeps it: on the
    /// host, the system admin made first.
    pub created_by: Id,
}

/// A channel that was put.
#[derive(Debug, Clone, PartialEq)]
pub struct Put {
    /// The channel as it is stored now.
    pub channel: ChannelRow,
    /// Whether it was made now, and not put over one that was there.
    pub created: bool,
    /// Where the far side could not be reached when it was looked at: the
    /// channel is saved with this as its error, since a server that is down
    /// today is not a wrong entry.
    pub unreachable: Option<DeliverError>,
    /// The names of the secret fields it holds now.
    pub secrets: Vec<String>,
}

/// Why a channel was not put.
#[derive(Debug, thiserror::Error)]
pub enum PutError {
    /// The database refused.
    #[error(transparent)]
    Db(#[from] DbError),
    /// The fields or the notifier do not fit.
    #[error(transparent)]
    Fault(#[from] SaveFault),
    /// The notifier is not one that the system sets up.
    #[error("{kind} is not a way out that the system sets up")]
    NotForTheSystem {
        /// The notifier.
        kind: String,
    },
    /// The name cannot be a channel's.
    #[error("a name is 1 to {max} characters")]
    Name {
        /// The most characters a name has.
        max: usize,
    },
    /// The channel was removed by somebody else while it was being put.
    #[error("the channel was removed while it was being put: try again")]
    Vanished,
    /// Somebody else made the channel at the same moment.
    #[error("somebody else set that channel up at the same moment: run it again to replace theirs")]
    Raced,
    /// The system has several channels of that kind and the name does not say
    /// which is meant.
    #[error("the system has several {kind} channels ({names}): say which with a name")]
    Several {
        /// The notifier.
        kind: String,
        /// The names they have.
        names: String,
    },
    /// The far side refuses what was given for good, such as a password.
    #[error("the far side refuses it: {0}")]
    Refused(DeliverError),
    /// A secret could not be sealed.
    #[error("a secret could not be sealed")]
    Seal,
}

/// Which channel of the system an operator means.
#[derive(Debug, Clone, PartialEq)]
pub enum Pick {
    /// There is none of that kind, or none of that name.
    Nothing,
    /// This one.
    One(Box<ChannelRow>),
    /// Several fit and the name does not tell them apart.
    Several(Vec<String>),
}

/// The channel of the system that `kind` and `name` mean among those the
/// system has. A kind the system has one of (a mail server, a bot) is that
/// one; for the others a name says which, and without a name the only one is
/// meant.
#[must_use]
pub fn pick(declared: &NotifierDescriptor, existing: Vec<ChannelRow>, name: Option<&str>) -> Pick {
    if declared.is_single() {
        return one_of(existing);
    }
    match name {
        Some(name) => existing
            .into_iter()
            .find(|channel| channel.name == name)
            .map_or(Pick::Nothing, |channel| Pick::One(Box::new(channel))),
        None => {
            if existing.len() > 1 {
                return Pick::Several(existing.into_iter().map(|channel| channel.name).collect());
            }
            one_of(existing)
        }
    }
}

/// The first of the channels there are, if there is one.
fn one_of(existing: Vec<ChannelRow>) -> Pick {
    existing
        .into_iter()
        .next()
        .map_or(Pick::Nothing, |channel| Pick::One(Box::new(channel)))
}

/// The channel of the system of a kind that a name means, for whoever tries
/// or changes it.
pub async fn find(
    conn: &mut Conn,
    notifiers: &Notifiers,
    kind: &str,
    name: Option<&str>,
) -> Result<Pick, ChannelError> {
    let notifier = notifiers
        .get(kind)
        .ok_or_else(|| ChannelError::UnknownKind(kind.to_owned()))?;
    let existing = rows::system_channels_of(conn, kind).await?;
    Ok(pick(&notifier.describe(), existing, name))
}

/// Puts a channel of the system: looks at the fields, then makes the channel or
/// replaces the one there is, with its secrets, and writes its line in the
/// audit trail, in one transaction. The look at the far side is made before
/// the transaction begins, since a server may take its time to answer.
pub async fn put(
    db: &Db,
    master: &MasterKey,
    notifiers: &Notifiers,
    setup: &SystemChannel<'_>,
    by: Actor,
    origin: &Origin,
    now: DateTime<Utc>,
) -> Result<Put, PutError> {
    let notifier = notifiers
        .get(setup.kind)
        .ok_or_else(|| SaveFault::UnknownKind(setup.kind.to_owned()))?;
    let declared = notifier.describe();
    if !declared.offered_to(Owner::System) {
        return Err(PutError::NotForTheSystem {
            kind: setup.kind.to_owned(),
        });
    }
    let name = setup.name.map(str::trim);
    if let Some(name) = name
        && !(1..=NAME_MAX_CHARS).contains(&name.chars().count())
    {
        return Err(PutError::Name {
            max: NAME_MAX_CHARS,
        });
    }
    let checked = check_before_save(
        notifiers,
        &Given {
            kind: setup.kind,
            target: Target::Person,
            fields: setup.fields,
            templates: &BTreeMap::new(),
        },
        None,
    )
    .await?;
    let unreachable = match &checked.saved {
        Saved::Refused(refusal) => return Err(PutError::Refused(refusal.clone())),
        Saved::KeptWithError(error) => Some(error.clone()),
        Saved::Kept(_) => None,
    };
    let values = checked.values_to_store();

    let mut tx = db.begin().await?;
    orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
    rows::lock_system_channels(&mut tx, setup.kind).await?;
    let existing = rows::system_channels_of(&mut tx, setup.kind).await?;
    let target = match pick(&declared, existing, name) {
        Pick::Nothing => None,
        Pick::One(channel) => Some(*channel),
        Pick::Several(names) => {
            return Err(PutError::Several {
                kind: setup.kind.to_owned(),
                names: names.join(", "),
            });
        }
    };
    let created = target.is_none();
    let id = target
        .as_ref()
        .map_or_else(orch_types::new_id, |channel| channel.id);
    let called = name
        .map(str::to_owned)
        .or_else(|| target.as_ref().map(|channel| channel.name.clone()))
        .unwrap_or_else(|| declared.title.clone());
    let error_text = unreachable.as_ref().map(|error| error.message.as_str());
    if created {
        let made = rows::insert_channel(
            &mut tx,
            &NewChannel {
                id,
                org_id: None,
                user_id: None,
                kind: setup.kind,
                name: &called,
                config: &values,
                templates: &json!({}),
                enabled: true,
                created_by: setup.created_by,
                at: now,
            },
        )
        .await;
        match made {
            Ok(_) => {}
            // Another writer that does not take the lock made one at the same
            // moment: the schema allows one mail server and one bot.
            Err(DbError::Unique { .. }) => return Err(PutError::Raced),
            Err(other) => return Err(other.into()),
        }
        if let Some(error) = error_text {
            rows::record_channel_error(&mut tx, id, error).await?;
        }
    } else {
        rows::replace_channel(&mut tx, id, &called, &values, error_text, now)
            .await?
            .ok_or(PutError::Vanished)?;
    }

    let mut kept = BTreeSet::new();
    for (secret_name, secret) in &checked.config.secrets {
        let sealed = seal_secret(master, owner::CHANNEL, id, secret_name, secret.expose())
            .map_err(|_| PutError::Seal)?;
        settings::put_secret(
            &mut tx,
            owner::CHANNEL,
            id,
            secret_name,
            &sealed,
            Some(setup.created_by),
            now,
        )
        .await?;
        kept.insert(secret_name.clone());
    }
    for stored in settings::secret_names_of(&mut tx, &[(owner::CHANNEL, id)]).await? {
        if !kept.contains(&stored.name) {
            settings::delete_secret(&mut tx, owner::CHANNEL, id, &stored.name).await?;
        }
    }

    let action = if created {
        AuditAction::ChannelCreated
    } else {
        AuditAction::ChannelUpdated
    };
    let summary = format!(
        "the {} channel \"{called}\" of the system was {}",
        setup.kind,
        if created { "set up" } else { "replaced" }
    );
    let mut entry = Entry::new(
        by,
        action.as_str(),
        action.object_kind().unwrap_or("channel"),
        Some(id),
        &summary,
    );
    entry.history = action.is_history_line();
    // Only the names of the fields: a value of a secret is in no row but its own.
    entry.details = json!({
        "kind": setup.kind,
        "fields": values.as_object().map(|fields| fields.keys().cloned().collect::<Vec<_>>()),
        "secrets": kept,
        "unreachable": error_text,
    });
    audit::append(&mut tx, &entry, origin, now).await?;
    let channel = rows::channel(&mut tx, id)
        .await?
        .ok_or(PutError::Vanished)?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(Put {
        channel,
        created,
        unreachable,
        secrets: kept.into_iter().collect(),
    })
}

/// Sends one sample message through a channel of the system, with no person
/// behind it: to `to` for a mail server, to its own address for a channel
/// that has one (a webhook). A channel that reaches a person through what
/// they linked or the browsers they gave (a bot, push) has nobody to write
/// to here, and says so. What the far side answers when it does not take the
/// sample is returned.
pub async fn send_sample(
    conn: &mut Conn,
    tools: &Toolbox<'_>,
    channel: &ChannelRow,
    to: Option<&str>,
    now: DateTime<Utc>,
) -> Result<TestOutcome, ChannelError> {
    let notifier = tools
        .notifiers
        .get(&channel.kind)
        .ok_or_else(|| ChannelError::UnknownKind(channel.kind.clone()))?;
    let declared = notifier.describe();
    let address = match (declared.reaches, to) {
        (Reaches::Email, Some(address)) => json!({ "email": address }),
        (Reaches::Email, None) => {
            return Ok(TestOutcome::failed(
                "a mail server needs an address to send the sample to",
            ));
        }
        (Reaches::Channel, _) => json!({}),
        (Reaches::Devices | Reaches::Link, _) => {
            return Ok(TestOutcome::failed(
                "this channel writes to a person's browsers or to a chat they linked, so its sample is sent from that person's own settings",
            ));
        }
    };
    let config = config_of_channel(conn, tools.master, channel).await?;
    let notification = [sample(tools.surroundings, &channel.name, now)];
    let message = match render::render(
        orch_types::new_id(),
        &Rendering {
            descriptor: &declared,
            own_texts: &channel.templates,
            language: DESTINATION_LANGUAGE,
            recipient: None,
            notifications: &notification,
            places: &[],
            org_name: &tools.surroundings.system_name,
            public_url: &tools.surroundings.public_url,
        },
    ) {
        Ok(message) => message,
        Err(fault) => return Ok(TestOutcome::failed(&fault.to_string())),
    };
    let delivery = Delivery {
        id: orch_types::new_id(),
        attempt: 1,
        config,
        to: address,
        recipient: None,
        language: DESTINATION_LANGUAGE.to_owned(),
        subject: message.subject,
        body: message.body,
        notifications: message.notifications,
    };
    Ok(TestOutcome::of(guarded(notifier.deliver(delivery)).await))
}
