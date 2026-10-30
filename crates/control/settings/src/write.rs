//! Writing and clearing a setting in the caller's transaction, with its audit row.
//!
//! A write puts a value at the system or at a node. Before it touches
//! anything it checks what only this package can: the key exists and is set
//! at that kind of place, the value fits its schema, the place is not a side
//! thread (whose settings are its parent's), no lock above decides the key,
//! and the lock of the row itself is changed only by a caller who showed that
//! they may. Whether the caller may write the key at all is not decided here:
//! the access engine does, and [`crate::validation::needs`] says what to ask
//! it. Then the row is written by compare-and-set on its revision, so that a
//! writer who saw an older row is refused with the value as it stands now.
//!
//! Everything happens on the connection of the caller's transaction, and the
//! audit row is written in it: neither exists without the other. A value
//! that is written as it already stands changes nothing and writes no row.
//! A change of a setting does not move the authorization epoch, which only
//! the changes of who may do what do; and the notices that other parts add (a
//! marker in a thread's conversation, the node's feed sequence) are theirs.

use chrono::{DateTime, Utc};
use orch_access_contract::audit::AuditAction;
use orch_db::audit::{self, Actor, Origin};
use orch_db::queries::hierarchy;
use orch_db::queries::settings::{self as queries, Put, Removed, Stored};
use orch_db::{Conn, DbError};
use orch_settings_contract::provenance::LockedBy;
use orch_settings_contract::registry::{self, Class, Invalid, Schema, SettingDef, Unit, keys};
use orch_types::{Id, Json, Revision};
use serde_json::json;

use crate::locks::{self, LockChange, Refusal};
use crate::provenance::SettingView;
use crate::resolver::{Snapshot, Wanted};
use crate::{Error, Place, Target};

/// The most characters of a value that a line of history carries. A line is a
/// sentence and a few facts; a text or a structure longer than this is named
/// by the line and not carried by it.
const LINE_VALUE_MAX_CHARS: usize = 80;

/// How many times a change of a list of names goes round when another writer
/// changed the same row meanwhile. Two writers at one node are rare; three
/// rounds is far more than they need and still ends.
const LIST_ROUNDS: usize = 3;

/// Own commands run beside a credential. Setting, replacing or removing that member needs ownership,
/// independently of the provider.manage permission needed for the surrounding runtime configuration.
/// Choosing another template also chooses its programs and commands, so carries the same requirement.
/// The resource writer checks the caller's effective Owners row in its transaction before writing.
#[must_use]
pub fn provider_commands_need_owner(before: &Json, after: &Json) -> bool {
    before.get(keys::RUNTIME_AFTER_UP) != after.get(keys::RUNTIME_AFTER_UP)
        || before.get(keys::RUNTIME_TEMPLATE_ID) != after.get(keys::RUNTIME_TEMPLATE_ID)
}

/// A value to write.
#[derive(Debug, Clone)]
pub struct Write<'a> {
    /// Who writes it.
    pub actor: Actor,
    /// Where.
    pub target: Target,
    /// The key.
    pub key: &'a str,
    /// The value. `null` is a value, for a key that allows it.
    pub value: Json,
    /// Whether the row decides the key for everything below. Left out, a row
    /// that is locked stays locked and a new one is not locked.
    pub locked: Option<bool>,
    /// The revision of the row that the writer saw. Left out, the write is
    /// unconditional.
    pub expected_revision: Option<Revision>,
    /// That the caller holds what setting or lifting a lock here takes. It
    /// matters only when the write changes the lock.
    pub may_lock: bool,
    /// Where the request came from.
    pub origin: &'a Origin,
}

/// A value to take back, so that the key is inherited again.
#[derive(Debug, Clone)]
pub struct Clear<'a> {
    /// Who takes it back.
    pub actor: Actor,
    /// Where.
    pub target: Target,
    /// The key.
    pub key: &'a str,
    /// The revision of the row that the caller saw. Left out, the row goes
    /// whatever it is at.
    pub expected_revision: Option<Revision>,
    /// That the caller holds what lifting a lock here takes. It matters only
    /// when the row that goes is locked.
    pub may_lock: bool,
    /// Where the request came from.
    pub origin: &'a Origin,
}

/// What a write or a clear came to.
#[derive(Debug, Clone, PartialEq)]
pub struct Written {
    /// The key as it now holds at the place.
    pub view: SettingView,
    /// The key as it held before: what a marker in a thread's conversation says
    /// the value was. The same as `view` when nothing changed.
    pub before: SettingView,
    /// Whether anything changed: a value written as it stands changes
    /// nothing, and neither does taking back what is not there.
    pub changed: bool,
}

/// Why a value could not be written.
#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    /// The database refused.
    #[error(transparent)]
    Db(#[from] DbError),
    /// There is no such node, or it was deleted.
    #[error("there is no such place")]
    NotFound,
    /// The key does not exist, is not set at that kind of place, or the value
    /// does not fit it.
    #[error(transparent)]
    Invalid(#[from] Invalid),
    /// The place is a side thread, which has no settings of its own.
    #[error("a side thread has no settings of its own")]
    SideThread,
    /// A lock above decides the key.
    #[error("a lock above decides this setting")]
    Locked {
        /// Where the lock was set.
        by: LockedBy<Id>,
    },
    /// The lock of the row cannot be changed as asked.
    #[error(transparent)]
    Lock(#[from] Refusal),
    /// The writer saw another revision of the row than there is now.
    #[error("the setting was changed meanwhile")]
    Stale {
        /// The key as it holds now.
        current: Box<SettingView>,
    },
    /// Something stored could not be read.
    #[error("what is stored for {what} cannot be read")]
    Unreadable {
        /// What it was.
        what: &'static str,
    },
}

impl From<Error> for WriteError {
    fn from(error: Error) -> Self {
        match error {
            Error::Db(error) => Self::Db(error),
            Error::NotFound => Self::NotFound,
            Error::UnknownKey(key) => Self::Invalid(Invalid::UnknownKey(key)),
            Error::NotOfThatType { .. } => Self::Unreadable { what: "a value" },
            Error::Unreadable { what } => Self::Unreadable { what },
        }
    }
}

fn node_of(target: Target) -> Option<Id> {
    match target {
        Target::System => None,
        Target::Node(id) => Some(id),
    }
}

/// The user who wrote, for the column that names one: what a person does, and
/// not what the system or an agent does.
fn user_of(actor: Actor) -> Option<Id> {
    match actor {
        Actor::User(id) => Some(id),
        Actor::System | Actor::Agent(_) => None,
    }
}

/// What a line of history says a value is, where a line says: the value of a
/// parameter that is a short text, a number or a switch. A policy is said to
/// have changed and not what it changed to; a long text or a structure is
/// named and not carried.
fn shown(def: &SettingDef, value: &Json) -> Option<String> {
    if def.class != Class::Parameter {
        return None;
    }
    let unit = match def.schema {
        Schema::Int { unit, .. } if unit != Unit::Count => format!(" {unit}"),
        _ => String::new(),
    };
    let text = match value {
        Json::String(text) if !text.is_empty() => text.clone(),
        Json::Number(number) => format!("{number}{unit}"),
        Json::Bool(switch) => switch.to_string(),
        _ => return None,
    };
    (text.chars().count() <= LINE_VALUE_MAX_CHARS).then_some(text)
}

fn summary_of_write(
    def: &SettingDef,
    value: &Json,
    changed_value: bool,
    lock: LockChange,
) -> String {
    let title = def.title;
    let lock_word = match lock {
        LockChange::Set => Some("locked"),
        LockChange::Lifted => Some("unlocked"),
        LockChange::Kept => None,
    };
    match (changed_value, lock_word) {
        (false, Some(word)) => format!("{title} {word}"),
        (true, word) => {
            let base = match shown(def, value) {
                Some(text) => format!("{title} set to {text}"),
                None => format!("{title} changed"),
            };
            match word {
                Some(word) => format!("{base} and {word}"),
                None => base,
            }
        }
        (false, None) => format!("{title} written"),
    }
}

/// One audit row of a change, but for where it was made.
struct Line<'a> {
    actor: Actor,
    origin: &'a Origin,
    action: AuditAction,
    summary: String,
    details: Json,
}

/// Writes the audit row of a change made at the place of `snapshot`.
async fn audit_change(
    conn: &mut Conn,
    snapshot: &Snapshot,
    line: Line<'_>,
    now: DateTime<Utc>,
) -> Result<(), WriteError> {
    let node = match snapshot.place() {
        Place::Node(id) => Some(id),
        _ => None,
    };
    let action = line.action;
    let mut entry = audit::Entry::new(
        line.actor,
        action.as_str(),
        action.object_kind().unwrap_or("settings"),
        node,
        &line.summary,
    );
    entry.history = action.is_history_line();
    entry.node_id = node;
    entry.org_id = snapshot.chain().first().map(|link| link.id);
    entry.details = line.details;
    audit::append(conn, &entry, line.origin, now).await?;
    // Only system-scope inputs used by Meta wake its connected readers. The hint carries
    // no setting value and no caller-specific may_create_org flag, and rolls back with the write.
    if matches!(snapshot.place(), Place::System)
        && entry
            .details
            .get("key")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|key| {
                matches!(
                    key,
                    keys::SYSTEM_BANNER
                        | keys::STORAGE_MAX_BLOB_BYTES
                        | keys::REGISTRATION_MODE
                        | keys::ORGS_CREATED_BY
                        | keys::AUTH_LOCAL_LOGIN
                        | keys::AUTH_EMAIL_CODE
                )
            })
    {
        orch_db::sequences::notify_meta_changed(conn, line.origin.request_id.as_deref()).await?;
    }
    Ok(())
}

async fn refreshed(
    conn: &mut Conn,
    snapshot: &mut Snapshot,
    node: Option<Id>,
    def: &'static SettingDef,
) -> Result<SettingView, WriteError> {
    match queries::row(conn, node, def.key).await? {
        Some(row) => snapshot.rows_mut().put(row),
        None => {
            snapshot.rows_mut().take(node, def.key);
        }
    }
    Ok(snapshot.view(def))
}

/// Writes `request.value` at the place, for the caller to commit.
pub async fn write(
    conn: &mut Conn,
    request: &Write<'_>,
    now: DateTime<Utc>,
) -> Result<Written, WriteError> {
    let def =
        registry::get(request.key).ok_or_else(|| Invalid::UnknownKey(request.key.to_owned()))?;
    let mut snapshot = Snapshot::load(conn, request.target.into(), Wanted::Keys(&[def])).await?;
    if snapshot.is_side_thread() {
        return Err(WriteError::SideThread);
    }
    let scope = snapshot.scope();
    let value = registry::canonical_value(def.key, &request.value);
    registry::validate_at(def.key, scope, &value)?;
    let before = snapshot.view(def);
    if let Some(lock) = before.lock_above() {
        return Err(WriteError::Locked { by: lock.clone() });
    }
    let node = node_of(request.target);
    let (was_locked, was_value) = before
        .own
        .as_ref()
        .map_or((false, None), |own| (own.locked, Some(&own.value)));
    let wants_locked = request.locked.unwrap_or(was_locked);
    let lock = locks::check_change(def, scope, was_locked, wants_locked, request.may_lock)?;
    let same_value = was_value == Some(&value);
    let at_revision = before.own.as_ref().is_some_and(|own| {
        request
            .expected_revision
            .is_none_or(|seen| seen == own.revision)
    });
    if same_value && lock == LockChange::Kept && at_revision {
        return Ok(Written {
            view: before.clone(),
            before,
            changed: false,
        });
    }
    let stored = Stored {
        value: &value,
        locked: wants_locked,
        by: user_of(request.actor),
        at: now,
    };
    let row = match queries::put(conn, node, def.key, &stored, request.expected_revision).await? {
        Put::Written(row) => row,
        Put::Stale => {
            let current = refreshed(conn, &mut snapshot, node, def).await?;
            return Err(WriteError::Stale {
                current: Box::new(current),
            });
        }
    };
    let action = match request.target {
        Target::System => orch_db::audit::action!(AuditAction::SystemSettingsUpdated),
        Target::Node(_) => orch_db::audit::action!(AuditAction::SettingsUpdated),
    };
    let summary = summary_of_write(def, &value, !same_value, lock);
    let mut details = json!({
        "key": def.key,
        "scope": scope.as_str(),
        "locked": row.locked,
        "revision": row.revision,
    });
    if let Some(text) = shown(def, &value) {
        details["value"] = json!(text);
    }
    if let Some(text) = was_value.and_then(|was| shown(def, was)) {
        details["was"] = json!(text);
    }
    snapshot.rows_mut().put(row);
    let line = Line {
        actor: request.actor,
        origin: request.origin,
        action,
        summary,
        details,
    };
    audit_change(conn, &snapshot, line, now).await?;
    Ok(Written {
        view: snapshot.view(def),
        before,
        changed: true,
    })
}

/// Takes back the value written at the place, so that the key is inherited
/// again, for the caller to commit.
pub async fn clear(
    conn: &mut Conn,
    request: &Clear<'_>,
    now: DateTime<Utc>,
) -> Result<Written, WriteError> {
    let def =
        registry::get(request.key).ok_or_else(|| Invalid::UnknownKey(request.key.to_owned()))?;
    let mut snapshot = Snapshot::load(conn, request.target.into(), Wanted::Keys(&[def])).await?;
    if snapshot.is_side_thread() {
        return Err(WriteError::SideThread);
    }
    let scope = snapshot.scope();
    if !def.set_at(scope) {
        return Err(Invalid::WrongScope {
            key: def.key,
            scope,
        }
        .into());
    }
    let before = snapshot.view(def);
    if let Some(lock) = before.lock_above() {
        return Err(WriteError::Locked { by: lock.clone() });
    }
    let node = node_of(request.target);
    let Some(own) = before.own.clone() else {
        return match request.expected_revision {
            Some(_) => Err(WriteError::Stale {
                current: Box::new(before),
            }),
            None => Ok(Written {
                view: before.clone(),
                before,
                changed: false,
            }),
        };
    };
    if own.locked && !request.may_lock {
        return Err(Refusal::NotPermitted.into());
    }
    match queries::remove(conn, node, def.key, request.expected_revision).await? {
        Removed::Gone(row) => {
            snapshot.rows_mut().take(node, def.key);
            let action = match request.target {
                Target::System => AuditAction::SystemSettingsUpdated,
                Target::Node(_) => orch_db::audit::action!(AuditAction::SettingsReset),
            };
            let summary = format!("{} reset to what is inherited", def.title);
            let mut details = json!({
                "key": def.key,
                "scope": scope.as_str(),
                "was_locked": row.locked,
                "reset": true,
            });
            if let Some(text) = shown(def, &row.value) {
                details["was"] = json!(text);
            }
            let line = Line {
                actor: request.actor,
                origin: request.origin,
                action,
                summary,
                details,
            };
            audit_change(conn, &snapshot, line, now).await?;
            Ok(Written {
                view: snapshot.view(def),
                before,
                changed: true,
            })
        }
        Removed::Absent => Ok(Written {
            view: refreshed(conn, &mut snapshot, node, def).await?,
            before,
            changed: false,
        }),
        Removed::Stale => {
            let current = refreshed(conn, &mut snapshot, node, def).await?;
            Err(WriteError::Stale {
                current: Box::new(current),
            })
        }
    }
}

/// What listing or unlisting a secret's name at a node came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Listing {
    /// The name was put in the node's own list, or taken out of it.
    Changed,
    /// It was in it, or out of it, already, or a lock above holds the list.
    Unchanged,
}

/// Puts `name` in the node's own list of the names of the secrets that its
/// environments are given (`listed`), or takes it out. This is a change of a
/// row of the key as any other, in the caller's transaction, but it is the
/// secret's audit row that records it. Putting a name in is refused when a lock
/// above holds the list; taking it out then is not needed, since the row counts
/// for nothing under that lock.
pub(crate) async fn list_secret_name(
    conn: &mut Conn,
    node: Id,
    name: &str,
    listed: bool,
    by: Option<Id>,
    now: DateTime<Utc>,
) -> Result<Listing, WriteError> {
    let def = registry::get(keys::ENV_SECRET_NAMES)
        .ok_or_else(|| Invalid::UnknownKey(keys::ENV_SECRET_NAMES.to_owned()))?;
    // Two secrets put at once at one node both read the list as it is and
    // would each write it without the other's name; the node is held until the
    // transaction ends, so that the second reads what the first wrote.
    if !hierarchy::lock(conn, node).await? {
        return Err(WriteError::NotFound);
    }
    for _ in 0..LIST_ROUNDS {
        let snapshot = Snapshot::load(conn, Place::Node(node), Wanted::Keys(&[def])).await?;
        if snapshot.is_side_thread() {
            return Err(WriteError::SideThread);
        }
        let scope = snapshot.scope();
        if !def.set_at(scope) {
            return Err(Invalid::WrongScope {
                key: def.key,
                scope,
            }
            .into());
        }
        let view = snapshot.view(def);
        if let Some(lock) = view.lock_above() {
            if listed {
                return Err(WriteError::Locked { by: lock.clone() });
            }
            return Ok(Listing::Unchanged);
        }
        let mut names: Vec<Json> = view
            .own
            .as_ref()
            .and_then(|own| own.value.as_array().cloned())
            .unwrap_or_default();
        let name_value = json!(name);
        let present = names.contains(&name_value);
        if present == listed {
            return Ok(Listing::Unchanged);
        }
        if listed {
            names.push(name_value);
        } else {
            names.retain(|each| each != &name_value);
        }
        let expected = view.own.as_ref().map(|own| own.revision);
        let locked = view.own.as_ref().is_some_and(|own| own.locked);
        let done = if names.is_empty() {
            matches!(
                queries::remove(conn, Some(node), def.key, expected).await?,
                Removed::Gone(_)
            )
        } else {
            let value = Json::Array(names);
            let stored = Stored {
                value: &value,
                locked,
                by,
                at: now,
            };
            matches!(
                queries::put(conn, Some(node), def.key, &stored, expected).await?,
                Put::Written(_)
            )
        };
        if done {
            return Ok(Listing::Changed);
        }
    }
    Err(WriteError::Db(DbError::Retry))
}
