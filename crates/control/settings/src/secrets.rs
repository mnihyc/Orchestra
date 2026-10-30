//! Secrets of a node or an environment: sealed, listed by name, opened for the engine.
//!
//! A secret belongs to a node, where it is given to the environments below
//! it, or to one environment alone. It is sealed with the system's master key
//! and bound to its own row, so a sealed value copied to another owner or
//! another name does not open there. The API never reads one back: a list
//! shows names, resolved down the hierarchy with the nearest owner's winning,
//! and the engine alone opens them.
//!
//! Setting a secret at a node also puts its name in the node's own list of
//! secret names, which is what says that environments are given it; taking
//! the secret away takes the name out again. A lock above that holds the list
//! refuses the secret, since its name could not be listed. A secret is not
//! kept on a thread: the list is not set there.
//!
//! The audit row names the secret and never its value. Who may write a secret
//! is not decided here: the caller asks the access engine first.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use orch_access_contract::audit::AuditAction;
use orch_db::audit::{self, Actor, Origin};
use orch_db::queries::settings::{self as queries, SecretName, owner};
use orch_db::secrets::{MasterKey, SealError};
use orch_db::{Conn, DbError};
use orch_settings_contract::provenance::LockedBy;
use orch_settings_contract::registry::{Invalid, is_secret_name};
use orch_types::Id;
use serde_json::json;

use crate::Error;
use crate::resolver::chain_to;
use crate::write::{Listing, WriteError, list_secret_name};

/// The most bytes of a secret: a certificate chain or a key file fits, and a
/// request carries no more than its own limit in any case.
pub const SECRET_MAX_BYTES: usize = 65_536;

/// What owns a secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Owner {
    /// A node (an org, a project or a topic): the secret is given to the
    /// environments below it.
    Node(Id),
    /// One environment: its own secret.
    Environment(Id),
}

/// A secret without its value: what a list shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretInfo {
    /// Its name.
    pub name: String,
    /// The node it is kept at; none for an environment's own.
    pub node: Option<Id>,
    /// When it was first written.
    pub created_at: DateTime<Utc>,
    /// When its value was last replaced; none until then.
    pub rotated_at: Option<DateTime<Utc>>,
}

impl From<SecretName> for SecretInfo {
    fn from(stored: SecretName) -> Self {
        let node = (stored.owner_kind == owner::SETTING).then_some(stored.owner_id);
        Self {
            name: stored.name,
            node,
            created_at: stored.created_at,
            rotated_at: stored.rotated_at,
        }
    }
}

/// A secret opened, for the engine alone.
#[derive(Clone, PartialEq, Eq)]
pub struct Opened {
    /// Its name.
    pub name: String,
    /// The node it is kept at; none for an environment's own.
    pub node: Option<Id>,
    /// Its value.
    pub value: String,
}

impl std::fmt::Debug for Opened {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Opened")
            .field("name", &self.name)
            .field("node", &self.node)
            .field("value", &"…")
            .finish()
    }
}

/// Why a secret could not be written, deleted or opened.
#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    /// The database refused.
    #[error(transparent)]
    Db(#[from] DbError),
    /// There is no such node or environment, or it was deleted.
    #[error("there is no such place")]
    NotFound,
    /// A secret is named by capitals, digits and underscores, and does not
    /// begin with a digit.
    #[error("the name of a secret is capitals, digits and underscores that begin with no digit")]
    BadName,
    /// A secret has a value.
    #[error("a secret needs a value")]
    Empty,
    /// A secret has at most [`SECRET_MAX_BYTES`] bytes.
    #[error("a secret has at most {max} bytes")]
    TooLarge {
        /// The most.
        max: usize,
    },
    /// Secrets are not kept at this kind of place: a thread has none.
    #[error("secrets are not kept at this kind of place")]
    NotHere,
    /// A side thread has no settings of its own.
    #[error("a side thread has no settings of its own")]
    SideThread,
    /// A lock above holds the list of secret names, so that the name of this
    /// one could not be listed.
    #[error("a lock above holds the list of secret names")]
    Locked {
        /// Where the lock was set.
        by: LockedBy<Id>,
    },
    /// There is no secret of that name there.
    #[error("there is no such secret")]
    NoSuchSecret,
    /// A sealed value does not open.
    #[error(transparent)]
    Seal(#[from] SealError),
    /// Something stored could not be read.
    #[error("what is stored for {what} cannot be read")]
    Unreadable {
        /// What it was.
        what: &'static str,
    },
}

impl From<Error> for SecretError {
    fn from(error: Error) -> Self {
        match error {
            Error::Db(error) => Self::Db(error),
            Error::NotFound => Self::NotFound,
            Error::UnknownKey(_) | Error::NotOfThatType { .. } => {
                Self::Unreadable { what: "a setting" }
            }
            Error::Unreadable { what } => Self::Unreadable { what },
        }
    }
}

impl From<WriteError> for SecretError {
    fn from(error: WriteError) -> Self {
        match error {
            WriteError::Db(error) => Self::Db(error),
            WriteError::NotFound => Self::NotFound,
            WriteError::SideThread => Self::SideThread,
            WriteError::Locked { by } => Self::Locked { by },
            WriteError::Invalid(Invalid::WrongScope { .. }) => Self::NotHere,
            WriteError::Invalid(_) | WriteError::Lock(_) | WriteError::Stale { .. } => {
                Self::Unreadable {
                    what: "a list of names",
                }
            }
            WriteError::Unreadable { what } => Self::Unreadable { what },
        }
    }
}

/// What a secret is bound to when it is sealed: its owner and its name. A sealed
/// value does not open for another owner or another name.
fn binding(kind: &str, owner_id: Id, name: &str) -> Vec<u8> {
    format!("secrets/{kind}/{owner_id}/{name}").into_bytes()
}

/// Where a secret's owner stands.
struct Located {
    /// The kind and identifier of the owners from the top down: the nodes
    /// from the root org, then the environment itself.
    owners: Vec<(&'static str, Id)>,
    /// The nodes from the root org down.
    chain: Vec<Id>,
    /// The node the owner stands at, or an environment's home.
    node: Id,
}

async fn locate(conn: &mut Conn, owner_of: Owner) -> Result<Located, SecretError> {
    let (node, environment) = match owner_of {
        Owner::Node(id) => (id, None),
        Owner::Environment(id) => {
            let found = queries::environment(conn, id)
                .await?
                .ok_or(SecretError::NotFound)?;
            (found.home_node_id, Some(id))
        }
    };
    let chain: Vec<Id> = chain_to(conn, node)
        .await?
        .into_iter()
        .map(|link| link.id)
        .collect();
    let mut owners: Vec<(&'static str, Id)> =
        chain.iter().map(|id| (owner::SETTING, *id)).collect();
    if let Some(environment) = environment {
        owners.push((owner::ENVIRONMENT, environment));
    }
    Ok(Located {
        owners,
        chain,
        node,
    })
}

fn user_of(actor: Actor) -> Option<Id> {
    match actor {
        Actor::User(id) => Some(id),
        Actor::System | Actor::Agent(_) => None,
    }
}

/// A secret to write.
#[derive(Debug, Clone)]
pub struct Put<'a> {
    /// Who writes it.
    pub actor: Actor,
    /// What owns it.
    pub owner: Owner,
    /// Its name.
    pub name: &'a str,
    /// Its value, which is never read back.
    pub value: &'a str,
    /// Where the request came from.
    pub origin: &'a Origin,
}

/// A secret to delete.
#[derive(Debug, Clone)]
pub struct Delete<'a> {
    /// Who deletes it.
    pub actor: Actor,
    /// What owns it.
    pub owner: Owner,
    /// Its name.
    pub name: &'a str,
    /// Where the request came from.
    pub origin: &'a Origin,
}

fn kind_and_id(owner_of: Owner) -> (&'static str, Id) {
    match owner_of {
        Owner::Node(id) => (owner::SETTING, id),
        Owner::Environment(id) => (owner::ENVIRONMENT, id),
    }
}

/// One audit row of a secret, but for where it stands.
struct Line<'a> {
    owner: Owner,
    actor: Actor,
    origin: &'a Origin,
    action: AuditAction,
    summary: String,
    details: serde_json::Value,
}

async fn audit_secret(
    conn: &mut Conn,
    located: &Located,
    line: Line<'_>,
    now: DateTime<Utc>,
) -> Result<(), SecretError> {
    let action = line.action;
    let mut entry = audit::Entry::new(
        line.actor,
        action.as_str(),
        action.object_kind().unwrap_or("settings"),
        Some(located.node),
        &line.summary,
    );
    entry.history = action.is_history_line();
    entry.node_id = Some(located.node);
    entry.org_id = located.chain.first().copied();
    entry.environment_id = match line.owner {
        Owner::Environment(id) => Some(id),
        Owner::Node(_) => None,
    };
    entry.details = line.details;
    audit::append(conn, &entry, line.origin, now).await?;
    Ok(())
}

/// Seals and stores a secret, replacing the value if there is one of that
/// name, for the caller to commit. At a node its name is also put in the
/// node's list of secret names.
pub async fn put(
    conn: &mut Conn,
    master: &MasterKey,
    request: &Put<'_>,
    now: DateTime<Utc>,
) -> Result<SecretInfo, SecretError> {
    if !is_secret_name(request.name) {
        return Err(SecretError::BadName);
    }
    if request.value.is_empty() {
        return Err(SecretError::Empty);
    }
    if request.value.len() > SECRET_MAX_BYTES {
        return Err(SecretError::TooLarge {
            max: SECRET_MAX_BYTES,
        });
    }
    let located = locate(conn, request.owner).await?;
    let by = user_of(request.actor);
    let listed = match request.owner {
        Owner::Node(node) => {
            list_secret_name(conn, node, request.name, true, by, now).await? == Listing::Changed
        }
        Owner::Environment(_) => false,
    };
    let (kind, id) = kind_and_id(request.owner);
    let sealed = master.seal(request.value.as_bytes(), &binding(kind, id, request.name))?;
    let stored = queries::put_secret(conn, kind, id, request.name, &sealed, by, now).await?;
    let replaced = stored.rotated_at.is_some();
    let summary = format!(
        "Secret {} {}",
        request.name,
        if replaced { "replaced" } else { "set" }
    );
    let details = json!({ "name": request.name, "replaced": replaced, "listed": listed });
    let line = Line {
        owner: request.owner,
        actor: request.actor,
        origin: request.origin,
        action: orch_db::audit::action!(AuditAction::SettingsSecretSet),
        summary,
        details,
    };
    audit_secret(conn, &located, line, now).await?;
    Ok(stored.into())
}

/// Deletes a secret, for the caller to commit. At a node its name is also
/// taken out of the node's list of secret names.
pub async fn delete(
    conn: &mut Conn,
    request: &Delete<'_>,
    now: DateTime<Utc>,
) -> Result<SecretInfo, SecretError> {
    let located = locate(conn, request.owner).await?;
    let (kind, id) = kind_and_id(request.owner);
    let removed = queries::delete_secret(conn, kind, id, request.name)
        .await?
        .ok_or(SecretError::NoSuchSecret)?;
    let unlisted = match request.owner {
        Owner::Node(node) => {
            list_secret_name(conn, node, request.name, false, user_of(request.actor), now).await?
                == Listing::Changed
        }
        Owner::Environment(_) => false,
    };
    let summary = format!("Secret {} deleted", request.name);
    let details = json!({ "name": request.name, "unlisted": unlisted });
    let line = Line {
        owner: request.owner,
        actor: request.actor,
        origin: request.origin,
        action: orch_db::audit::action!(AuditAction::SettingsSecretDeleted),
        summary,
        details,
    };
    audit_secret(conn, &located, line, now).await?;
    Ok(removed.into())
}

/// The secrets that apply to the owner, by name and without their values:
/// those of the nodes from the root org down, and for an environment its own,
/// the nearest owner's winning when two have one name. In the order of the
/// names.
pub async fn list(conn: &mut Conn, owner_of: Owner) -> Result<Vec<SecretInfo>, SecretError> {
    let located = locate(conn, owner_of).await?;
    let stored = queries::secret_names_of(conn, &located.owners).await?;
    let mut nearest: BTreeMap<String, SecretInfo> = BTreeMap::new();
    for (kind, id) in &located.owners {
        for each in stored
            .iter()
            .filter(|each| each.owner_kind == *kind && each.owner_id == *id)
        {
            nearest.insert(each.name.clone(), each.clone().into());
        }
    }
    Ok(nearest.into_values().collect())
}

/// Opens the secret `name` of exactly this owner: for the engine alone. None
/// where there is no such secret.
pub async fn open(
    conn: &mut Conn,
    master: &MasterKey,
    owner_of: Owner,
    name: &str,
) -> Result<Option<String>, SecretError> {
    let (kind, id) = kind_and_id(owner_of);
    let Some(row) = queries::secret(conn, kind, id, name).await? else {
        return Ok(None);
    };
    open_row(master, &row).map(|opened| Some(opened.value))
}

fn open_row(master: &MasterKey, row: &queries::SecretRow) -> Result<Opened, SecretError> {
    let plain = master.open(
        &row.sealed,
        &binding(&row.owner_kind, row.owner_id, &row.name),
    )?;
    let value =
        String::from_utf8(plain).map_err(|_| SecretError::Unreadable { what: "a secret" })?;
    let node = (row.owner_kind == owner::SETTING).then_some(row.owner_id);
    Ok(Opened {
        name: row.name.clone(),
        node,
        value,
    })
}

/// Opens every secret that applies to the owner, as [`list`] names them: for
/// the engine alone.
pub async fn open_resolved(
    conn: &mut Conn,
    master: &MasterKey,
    owner_of: Owner,
) -> Result<Vec<Opened>, SecretError> {
    let located = locate(conn, owner_of).await?;
    let stored = queries::secrets_of(conn, &located.owners).await?;
    let mut nearest: BTreeMap<String, &queries::SecretRow> = BTreeMap::new();
    for (kind, id) in &located.owners {
        for each in stored
            .iter()
            .filter(|each| each.owner_kind == *kind && each.owner_id == *id)
        {
            nearest.insert(each.name.clone(), each);
        }
    }
    nearest
        .into_values()
        .map(|row| open_row(master, row))
        .collect()
}
