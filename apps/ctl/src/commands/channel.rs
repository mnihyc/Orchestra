//! A channel of the system made or replaced from a file of its fields, and one sample sent through it.
//!
//! `channel put <kind> --file <file>` sets up a way out of the application
//! for the whole system, such as its mail server or its bot, from a file of
//! the fields its notifier asks for: a JSON object, the secret fields (a
//! password, a token) among the rest. It is what Management → Channels does,
//! for a system that is set up by a script, by calling the same functions:
//! the fields are looked at as the notifier asks before anything is written
//! (a mail server is connected to and signed in at, a bot is asked who it
//! is), what the far side refuses for good refuses the save, and one that
//! cannot be reached now does not and is kept with that as its error. The
//! secret fields are sealed under the master key as they are stored and are
//! in no other row. Put again, a channel is replaced and switched on.
//!
//! `channel test <kind> [--to <address>]` sends one sample through the
//! channel and says what the far side answered: a mail server needs the
//! address to send it to; a webhook posts to its own address. A bot and push
//! reach a person through what that person linked or gave, so their sample is
//! sent from that person's own settings.

use super::console::{finish, note, say};
use super::host::{self, Host, HostError, Sealing};
use clap::{Args, Subcommand};
use orch_db::audit::Actor;
use orch_db::{DbError, queries::identity};
use orch_notifier_contract::descriptor::{Owner, Reaches};
use orch_notifier_email::EmailNotifier;
use orch_notifier_telegram::TelegramNotifier;
use orch_notifier_webhook::WebhookNotifier;
use orch_types::{Clock, Json, SystemClock};
use orch_worker_core::channel_setup::{self, Pick, PutError, SystemChannel};
use orch_worker_core::channels::{
    ChannelError, Notifiers, RegisterError, SaveFault, Surroundings, Toolbox,
};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

/// The most bytes of a file of fields: as much as a JSON body the API takes.
/// A mail server's extra authority is the longest thing in one.
const FIELDS_MAX_BYTES: u64 = 1024 * 1024;

/// The ways out of the application for the whole system.
#[derive(Debug, Args)]
pub struct ChannelArgs {
    #[command(flatten)]
    host: Host,
    #[command(flatten)]
    sealing: Sealing,
    #[command(subcommand)]
    command: ChannelCommand,
}

#[derive(Debug, Subcommand)]
enum ChannelCommand {
    /// Make a channel of the system, or replace the one there is, from a file
    /// of its fields: its mail server, its bot, a webhook.
    Put {
        /// The kind: `email`, `telegram` or `webhook`.
        kind: String,
        /// A JSON object of the fields the kind asks for, the secret ones
        /// among them. The file holds secrets: keep it from other people, and
        /// remove it when the channel is put.
        #[arg(long, value_name = "FILE")]
        file: PathBuf,
        /// What the channel is called, and which one is meant where the
        /// system has several of the kind, such as webhooks.
        #[arg(long)]
        name: Option<String>,
    },
    /// Send one sample through a channel of the system and print what the
    /// far side answered.
    Test {
        /// The kind: `email`, `telegram` or `webhook`.
        kind: String,
        /// For a mail server, the address to send the sample to.
        #[arg(long, value_name = "ADDRESS")]
        to: Option<String>,
        /// Which channel is meant where the system has several of the kind.
        #[arg(long)]
        name: Option<String>,
    },
}

/// Why a channel was not put or tried.
#[derive(Debug, thiserror::Error)]
enum ChannelCommandError {
    #[error(transparent)]
    Host(#[from] HostError),
    #[error("cannot read {path}: {reason}")]
    Read { path: String, reason: String },
    #[error("{0} is larger than the 1 MiB a file of fields may be")]
    TooLarge(String),
    #[error("{path} is not JSON: {reason}")]
    NotJson { path: String, reason: String },
    #[error(
        "{0} holds JSON but not an object of fields, such as {{\"host\": \"smtp.example.org\"}}"
    )]
    NotAnObject(String),
    #[error("{kind} is no kind of channel the system sets up here: use {kinds}")]
    UnknownKind { kind: String, kinds: String },
    #[error(
        "the system has no administrator yet, whom a channel can be set up by: make the first one in the browser, then put the channel"
    )]
    NobodyToSetItUpBy,
    #[error("{0}")]
    Put(String),
    #[error(
        "the system has no {kind} channel: put one with orchctl channel put {kind} --file <file>"
    )]
    NoChannel { kind: String },
    #[error("the system has several {kind} channels ({names}): say which with --name")]
    Several { kind: String, names: String },
    #[error("a sample of {kind} cannot be sent from here: {reason}")]
    CannotSend { kind: String, reason: &'static str },
    #[error("the far side did not take the sample: {0}")]
    NotTaken(String),
    #[error("the database failed: {0}")]
    Database(String),
    #[error("cannot write the answer: {0}")]
    Output(#[from] std::io::Error),
}

impl From<DbError> for ChannelCommandError {
    fn from(error: DbError) -> Self {
        Self::Database(error.to_string())
    }
}

impl From<ChannelError> for ChannelCommandError {
    fn from(error: ChannelError) -> Self {
        Self::Database(error.to_string())
    }
}

/// The notifiers the program can set up and try: mail, bots and webhooks.
/// Push needs the keys the worker holds and the browsers of people, so it is
/// set up in Management.
fn notifiers() -> Result<Notifiers, RegisterError> {
    let mut notifiers = Notifiers::new();
    notifiers.register(Arc::new(EmailNotifier::new()))?;
    notifiers.register(Arc::new(WebhookNotifier::new()))?;
    notifiers.register(Arc::new(TelegramNotifier::new()))?;
    Ok(notifiers)
}

/// The kinds the system can set up, as a sentence lists them.
fn kinds(notifiers: &Notifiers) -> String {
    notifiers
        .descriptors()
        .into_iter()
        .filter(|declared| declared.offered_to(Owner::System))
        .map(|declared| declared.name)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Refuses a kind the program does not know, naming the ones it does.
fn known(notifiers: &Notifiers, kind: &str) -> Result<(), ChannelCommandError> {
    if notifiers.get(kind).is_some() {
        return Ok(());
    }
    Err(ChannelCommandError::UnknownKind {
        kind: kind.to_owned(),
        kinds: kinds(notifiers),
    })
}

/// The fields in a file: a JSON object.
fn fields_of(path: &Path) -> Result<Json, ChannelCommandError> {
    let shown = path.display().to_string();
    let unreadable = |error: std::io::Error| ChannelCommandError::Read {
        path: shown.clone(),
        reason: error.to_string(),
    };
    if std::fs::metadata(path).map_err(unreadable)?.len() > FIELDS_MAX_BYTES {
        return Err(ChannelCommandError::TooLarge(shown));
    }
    let text = std::fs::read_to_string(path).map_err(unreadable)?;
    let fields: Json =
        serde_json::from_str(&text).map_err(|error| ChannelCommandError::NotJson {
            path: shown.clone(),
            reason: error.to_string(),
        })?;
    if !fields.is_object() {
        return Err(ChannelCommandError::NotAnObject(shown));
    }
    Ok(fields)
}

/// What a refusal of the library comes to in the words of a command.
fn refusal(error: PutError, kind: &str, notifiers: &Notifiers) -> ChannelCommandError {
    match error {
        PutError::Db(error) => error.into(),
        PutError::Fault(SaveFault::UnknownKind(kind)) => ChannelCommandError::UnknownKind {
            kinds: kinds(notifiers),
            kind,
        },
        PutError::Fault(SaveFault::WrongTarget { kind }) => ChannelCommandError::Put(format!(
            "{kind} cannot be a channel: it is only a place a node is told at"
        )),
        PutError::Fault(SaveFault::Fields(faults)) => ChannelCommandError::Put(format!(
            "the fields do not fit {kind}: {}",
            faults
                .iter()
                .map(|fault| format!("{}: {}", fault.path, fault.message))
                .collect::<Vec<_>>()
                .join("; ")
        )),
        PutError::Fault(SaveFault::Texts(_)) => {
            ChannelCommandError::Put("the texts of the channel do not fit".to_owned())
        }
        PutError::Several { kind, names } => ChannelCommandError::Several { kind, names },
        other @ (PutError::NotForTheSystem { .. }
        | PutError::Name { .. }
        | PutError::Vanished
        | PutError::Raced
        | PutError::Seal) => ChannelCommandError::Put(other.to_string()),
        PutError::Refused(error) => ChannelCommandError::Put(format!(
            "the far side refuses it, and nothing was saved: {}",
            error.message
        )),
    }
}

async fn put(
    host: &Host,
    sealing: &Sealing,
    kind: &str,
    file: &Path,
    name: Option<&str>,
) -> Result<(), ChannelCommandError> {
    let notifiers = notifiers().map_err(|error| ChannelCommandError::Put(error.to_string()))?;
    known(&notifiers, kind)?;
    let fields = fields_of(file)?;
    let deployment = host.connect().await?;
    let master = sealing.master_key()?;
    let created_by = {
        let mut conn = deployment.db.acquire().await?;
        identity::first_system_admin(&mut conn)
            .await?
            .ok_or(ChannelCommandError::NobodyToSetItUpBy)?
    };
    let put = channel_setup::put(
        &deployment.db,
        &master,
        &notifiers,
        &SystemChannel {
            kind,
            name,
            fields: &fields,
            created_by,
        },
        Actor::System,
        &host::origin(),
        SystemClock.now(),
    )
    .await
    .map_err(|error| refusal(error, kind, &notifiers))?;
    let secrets = if put.secrets.is_empty() {
        String::new()
    } else {
        format!("; sealed as stored: {}", put.secrets.join(", "))
    };
    say(format!(
        "{} the {kind} channel \"{}\" of the system{secrets}",
        if put.created { "made" } else { "replaced" },
        put.channel.name
    ))?;
    if let Some(error) = &put.unreachable {
        note(format!(
            "the far side could not be reached just now ({}): the channel is saved with that as its error, and works when the server answers",
            error.message
        ));
    }
    Ok(())
}

async fn test(
    host: &Host,
    sealing: &Sealing,
    kind: &str,
    to: Option<&str>,
    name: Option<&str>,
) -> Result<(), ChannelCommandError> {
    let notifiers = notifiers().map_err(|error| ChannelCommandError::Put(error.to_string()))?;
    known(&notifiers, kind)?;
    let declared = notifiers
        .get(kind)
        .map(|notifier| notifier.describe())
        .ok_or_else(|| ChannelCommandError::NoChannel {
            kind: kind.to_owned(),
        })?;
    match (declared.reaches, to) {
        (Reaches::Devices | Reaches::Link, _) => {
            return Err(ChannelCommandError::CannotSend {
                kind: kind.to_owned(),
                reason: "it writes to a person through what they linked or the browsers they gave, so its sample is sent from that person's own settings",
            });
        }
        (Reaches::Channel, Some(_)) => {
            return Err(ChannelCommandError::CannotSend {
                kind: kind.to_owned(),
                reason: "it posts to its own address, so --to has no meaning for it",
            });
        }
        (Reaches::Email, None) => {
            return Err(ChannelCommandError::CannotSend {
                kind: kind.to_owned(),
                reason: "a mail server needs the address to send the sample to: give it with --to",
            });
        }
        (Reaches::Email, Some(_)) | (Reaches::Channel, None) => {}
    }
    let deployment = host.connect().await?;
    let master = sealing.master_key()?;
    let mut conn = deployment.db.acquire().await?;
    let channel = match channel_setup::find(&mut conn, &notifiers, kind, name).await? {
        Pick::One(channel) => channel,
        Pick::Nothing => {
            return Err(ChannelCommandError::NoChannel {
                kind: kind.to_owned(),
            });
        }
        Pick::Several(names) => {
            return Err(ChannelCommandError::Several {
                kind: kind.to_owned(),
                names: names.join(", "),
            });
        }
    };
    let surroundings = Surroundings {
        system_name: deployment.config.system.name.clone(),
        public_url: deployment.config.system.public_url.clone(),
    };
    let tools = Toolbox {
        master: &master,
        notifiers: &notifiers,
        surroundings: &surroundings,
    };
    let outcome =
        channel_setup::send_sample(&mut conn, &tools, &channel, to, SystemClock.now()).await?;
    if !outcome.ok {
        return Err(ChannelCommandError::NotTaken(
            outcome
                .error
                .unwrap_or_else(|| "it gave no reason".to_owned()),
        ));
    }
    say(format!(
        "sent a sample through the {kind} channel \"{}\" of the system{}: the far side took it",
        channel.name,
        to.map_or_else(String::new, |address| format!(" to {address}"))
    ))?;
    Ok(())
}

async fn work(args: &ChannelArgs) -> Result<(), ChannelCommandError> {
    match &args.command {
        ChannelCommand::Put { kind, file, name } => {
            put(&args.host, &args.sealing, kind, file, name.as_deref()).await
        }
        ChannelCommand::Test { kind, to, name } => {
            test(
                &args.host,
                &args.sealing,
                kind,
                to.as_deref(),
                name.as_deref(),
            )
            .await
        }
    }
}

/// Does what the command asks and says what came of it.
#[must_use]
pub fn run(args: &ChannelArgs) -> ExitCode {
    finish(
        host::block_on(work(args))
            .map_err(ChannelCommandError::from)
            .and_then(|done| done),
    )
}
