//! Locked ancestor handling.
//!
//! A row may be locked: the lock takes the choice away from everything under
//! it, and whoever holds the place below can neither write the key there nor
//! take a row back. Which rows a lock silences when a key is read is the
//! contract's rule; this is what a lock means for a **write**, and for who may
//! set or lift one.
//!
//! A lock at the place where a value is written is not above it: whoever
//! makes the rules there changes that row, lock included. Where lower scopes
//! can only narrow a key, a lock changes nothing and is never reported, so
//! nothing here is refused for it.

use orch_settings_contract::provenance::LockedBy;
use orch_settings_contract::registry::{Scope, SettingDef};
use orch_types::Id;

use crate::Place;

/// Where a value is read or written, as far as a lock above it is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Here {
    /// The system: nothing lies above it.
    System,
    /// A node: a lock at any other place is above it.
    Node(Id),
    /// The own value of an environment or of a provider's runtime, the
    /// innermost scope: every lock on the way is above it.
    Own,
}

impl From<Place> for Here {
    fn from(place: Place) -> Self {
        match place {
            Place::System => Self::System,
            Place::Node(id) => Self::Node(id),
            Place::Environment(_) | Place::Provider(_) | Place::Host(_) => Self::Own,
        }
    }
}

/// The lock above `here` that decides the key, where one does: `locked_by` is
/// the topmost locked row on the way, and it is above `here` unless it stands
/// at `here` itself.
#[must_use]
pub fn lock_above(here: Here, locked_by: Option<&LockedBy<Id>>) -> Option<&LockedBy<Id>> {
    let lock = locked_by?;
    match here {
        Here::System => None,
        Here::Node(id) => (lock.node != Some(id)).then_some(lock),
        Here::Own => Some(lock),
    }
}

/// What a write does to the lock flag of the row it writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockChange {
    /// The row is locked as it was, or stays unlocked.
    Kept,
    /// The row becomes locked.
    Set,
    /// The row stops being locked.
    Lifted,
}

impl LockChange {
    /// What going from `was` to `wants` is.
    #[must_use]
    pub fn of(was: bool, wants: bool) -> Self {
        match (was, wants) {
            (false, true) => Self::Set,
            (true, false) => Self::Lifted,
            _ => Self::Kept,
        }
    }
}

/// Why the flag of a row cannot be changed as asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    /// A lock on this key at this scope would change nothing: it is set at
    /// the system, or nothing lies below it, or lower scopes can only narrow
    /// the key.
    #[error("a lock on this key at this scope would change nothing")]
    NotLockable,
    /// Setting or lifting a lock is for whoever makes the rules there, and the
    /// caller did not show that they are.
    #[error("setting or lifting a lock is for whoever makes the rules there")]
    NotPermitted,
}

/// Whether the flag of a row of `def` at `scope` may go from `was` to
/// `wants`. `may_lock` says that the caller holds what changing a lock takes
/// at this place. A row that is locked as it was is not a change, and is not
/// refused even if the registry has changed since.
pub fn check_change(
    def: &SettingDef,
    scope: Scope,
    was: bool,
    wants: bool,
    may_lock: bool,
) -> Result<LockChange, Refusal> {
    let change = LockChange::of(was, wants);
    if change == LockChange::Set && !def.lockable_at(scope) {
        return Err(Refusal::NotLockable);
    }
    if change != LockChange::Kept && !may_lock {
        return Err(Refusal::NotPermitted);
    }
    Ok(change)
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_settings_contract::provenance::LockScope;
    use orch_settings_contract::registry::{get, keys};
    use orch_types::new_id;

    fn lock_at(node: Option<Id>) -> LockedBy<Id> {
        LockedBy {
            scope: LockScope::Org,
            node,
        }
    }

    #[test]
    fn a_lock_at_another_place_is_above_and_one_at_the_place_itself_is_not() {
        let (org, project) = (new_id(), new_id());
        let at_org = lock_at(Some(org));
        assert!(lock_above(Here::Node(project), Some(&at_org)).is_some());
        assert!(lock_above(Here::Node(org), Some(&at_org)).is_none());
        assert!(lock_above(Here::Node(org), None).is_none());
        assert!(
            lock_above(Here::Own, Some(&at_org)).is_some(),
            "every lock on the way is above an environment's own value"
        );
        assert!(
            lock_above(Here::System, Some(&lock_at(None))).is_none(),
            "nothing lies above the system"
        );
    }

    #[test]
    fn only_a_key_a_lock_changes_something_for_can_be_locked_and_only_by_whoever_may() {
        let org = Scope::Org;
        let model = get(keys::AGENT_MODEL).unwrap();
        assert_eq!(
            check_change(model, org, false, true, true),
            Ok(LockChange::Set)
        );
        assert_eq!(
            check_change(model, org, false, true, false),
            Err(Refusal::NotPermitted)
        );
        assert_eq!(
            check_change(model, org, true, false, true),
            Ok(LockChange::Lifted)
        );
        assert_eq!(
            check_change(model, org, false, false, false),
            Ok(LockChange::Kept)
        );
        let narrowing = get(keys::MODELS_ALLOWED).unwrap();
        assert_eq!(
            check_change(narrowing, org, false, true, true),
            Err(Refusal::NotLockable)
        );
        assert_eq!(
            check_change(model, Scope::Thread, false, true, true),
            Err(Refusal::NotLockable),
            "nothing lies below a thread"
        );
    }
}
