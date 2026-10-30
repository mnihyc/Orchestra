//! Break-glass administration on the host: a reset link, which may also switch a second step off, giving and removing admin, suspending a user and lifting that.
//!
//! For when nobody can sign in, or mail is off, or an admin is needed
//! before there is a browser to make one in. Each command is what
//! Management → Users does, done by calling the same functions of the same
//! libraries, with no person signed in: the line it leaves in the audit
//! trail names the actor `system`.
//!
//! - `reset-link` prints a one-use address at which the person sets a new
//!   password; with `--clear-second-step`, using it also switches their second
//!   step off, for somebody who lost their phone or their mailbox.
//! - `grant` and `revoke` make and unmake a system admin. The last system admin
//!   who can sign in is never unmade.
//! - `suspend` and `resume`: the person can no longer sign in anywhere, every
//!   session of theirs ends now, and what they own stays theirs; `resume` lifts
//!   that. The last system admin who can sign in is never suspended.

use super::console::{finish, note, say};
use super::host::{self, Host, HostError};
use chrono::{DateTime, Utc};
use clap::{Args, Subcommand};
use orch_api_core::sign_in::reset_links;
use orch_authz::standing::{self, Acting, Done, Refused};
use orch_config::ServerConfig;
use orch_db::audit::Actor;
use orch_db::queries::identity::{self, UserRow};
use orch_db::{Conn, DbError};
use orch_types::{Clock, SystemClock};
use std::process::ExitCode;

/// What is done to the accounts of the system from the host.
#[derive(Debug, Args)]
pub struct AdminArgs {
    #[command(flatten)]
    host: Host,
    #[command(subcommand)]
    command: AdminCommand,
}

#[derive(Debug, Subcommand)]
enum AdminCommand {
    /// Print an address, good for one use, at which a person sets a new
    /// password: for when nobody can sign in and mail is off.
    ResetLink {
        /// The username the person signs in by.
        username: String,
        /// Using the address also switches the person's second step off, an
        /// authenticator and a mailed code alike: for somebody who lost
        /// theirs.
        #[arg(long)]
        clear_second_step: bool,
    },
    /// Make a person a system admin.
    Grant {
        /// The username the person signs in by.
        username: String,
    },
    /// Make a system admin an ordinary person again. The last system admin
    /// who can sign in stays one.
    Revoke {
        /// The username the person signs in by.
        username: String,
    },
    /// Suspend a person: they can no longer sign in anywhere, every session of
    /// theirs ends now, and what they own stays theirs.
    Suspend {
        /// The username the person signs in by.
        username: String,
    },
    /// Lift the suspension of a person.
    Resume {
        /// The username the person signs in by.
        username: String,
    },
}

/// Why a command did not do what it was asked.
#[derive(Debug, thiserror::Error)]
enum AdminError {
    #[error(transparent)]
    Host(#[from] HostError),
    #[error(
        "there is no user named {0}: it is the name the person signs in by, as the list of users shows it"
    )]
    NoSuchUser(String),
    #[error(
        "{username} is the last system admin who can sign in, so {username} cannot be {unmade}: make another system admin first (orchctl admin grant <username>)"
    )]
    LastAdmin {
        username: String,
        unmade: &'static str,
    },
    #[error("the database failed: {0}")]
    Database(String),
    #[error("cannot write the answer: {0}")]
    Output(#[from] std::io::Error),
}

impl From<DbError> for AdminError {
    fn from(error: DbError) -> Self {
        Self::Database(error.to_string())
    }
}

/// What a refusal of the library comes to in the words of a command.
fn refusal(error: Refused, unmade: &'static str) -> AdminError {
    match error {
        Refused::LastAdmin { username } => AdminError::LastAdmin { username, unmade },
        Refused::NoSuchUser(id) => AdminError::NoSuchUser(id.to_string()),
        Refused::Db(error) => error.into(),
        Refused::Authz(error) => AdminError::Database(error.to_string()),
    }
}

/// The person a name is, or the sentence that says there is none.
async fn who(conn: &mut Conn, username: &str) -> Result<UserRow, AdminError> {
    identity::user_by_username(conn, username.trim())
        .await?
        .ok_or_else(|| AdminError::NoSuchUser(username.to_owned()))
}

/// A moment as a person reads it.
fn stamp(at: DateTime<Utc>) -> String {
    at.format("%Y-%m-%d %H:%M UTC").to_string()
}

fn sessions(count: u64) -> String {
    if count == 1 {
        "1 session".to_owned()
    } else {
        format!("{count} sessions")
    }
}

/// The address of a reset link, and what to tell about it.
async fn reset_link(
    host: &Host,
    username: &str,
    clear_second_step: bool,
) -> Result<(), AdminError> {
    let deployment = host.connect().await?;
    let now = SystemClock.now();
    let mut tx = deployment.db.begin().await?;
    let user = who(&mut tx, username).await?;
    let made = reset_links::make(
        &mut tx,
        &deployment.config,
        &user,
        clear_second_step,
        Actor::System,
        &host::origin(),
        now,
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    say(&made.url)?;
    note(format!(
        "good for one use, until {}{}",
        stamp(made.expires_at),
        if clear_second_step {
            format!(
                "; it also switches off the second step of {}",
                user.username
            )
        } else {
            String::new()
        }
    ));
    if user.suspended() {
        note(format!(
            "{} is suspended, and cannot sign in until the suspension is lifted (orchctl admin resume {})",
            user.username, user.username
        ));
    }
    if deployment.config.system.public_url == ServerConfig::default().system.public_url {
        note(
            "system.public_url is still the example address of the configuration: set it to the address people open, or this link leads nowhere",
        );
    }
    Ok(())
}

/// Which of the four acts on a standing is done.
#[derive(Debug, Clone, Copy)]
enum Act {
    Grant,
    Revoke,
    Suspend,
    Resume,
}

impl Act {
    /// What the person cannot be made, as a sentence says it, where the act
    /// is refused because they are the last system admin.
    fn unmade(self) -> &'static str {
        match self {
            Self::Revoke => "made an ordinary person",
            _ => "suspended",
        }
    }

    /// What was done, or what was so already.
    fn told(self, username: &str, done: Done) -> String {
        match (self, done.changed) {
            (Self::Grant, true) => format!("{username} is a system admin now"),
            (Self::Grant, false) => {
                format!("{username} is a system admin already; nothing was changed")
            }
            (Self::Revoke, true) => format!("{username} is no system admin any more"),
            (Self::Revoke, false) => format!("{username} was no system admin; nothing was changed"),
            (Self::Suspend, true) => format!(
                "{username} is suspended: {} ended; what they own stays theirs",
                sessions(done.sessions_ended)
            ),
            (Self::Suspend, false) => {
                format!("{username} was suspended already; nothing was changed")
            }
            (Self::Resume, true) => format!(
                "{username} can sign in again; the sessions the suspension ended are not brought back"
            ),
            (Self::Resume, false) => format!("{username} was not suspended; nothing was changed"),
        }
    }
}

/// Changes the standing of a person in one transaction.
async fn change(host: &Host, username: &str, act: Act) -> Result<(), AdminError> {
    let deployment = host.connect().await?;
    let origin = host::origin();
    let acting = Acting {
        by: Actor::System,
        origin: &origin,
        now: SystemClock.now(),
    };
    let mut tx = deployment.db.begin().await?;
    let user = who(&mut tx, username).await?;
    let done = match act {
        Act::Grant => standing::grant_admin(&mut tx, user.id, &acting).await,
        Act::Revoke => standing::revoke_admin(&mut tx, user.id, &acting).await,
        Act::Suspend => standing::suspend(&mut tx, user.id, &acting).await,
        Act::Resume => standing::resume(&mut tx, user.id, &acting).await,
    }
    .map_err(|error| refusal(error, act.unmade()))?;
    tx.commit().await.map_err(DbError::from)?;
    say(act.told(&user.username, done))?;
    Ok(())
}

async fn work(args: &AdminArgs) -> Result<(), AdminError> {
    match &args.command {
        AdminCommand::ResetLink {
            username,
            clear_second_step,
        } => reset_link(&args.host, username, *clear_second_step).await,
        AdminCommand::Grant { username } => change(&args.host, username, Act::Grant).await,
        AdminCommand::Revoke { username } => change(&args.host, username, Act::Revoke).await,
        AdminCommand::Suspend { username } => change(&args.host, username, Act::Suspend).await,
        AdminCommand::Resume { username } => change(&args.host, username, Act::Resume).await,
    }
}

/// Does what the command asks and says what came of it.
#[must_use]
pub fn run(args: &AdminArgs) -> ExitCode {
    finish(
        host::block_on(work(args))
            .map_err(AdminError::from)
            .and_then(|done| done),
    )
}
