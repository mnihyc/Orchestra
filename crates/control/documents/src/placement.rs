//! The node a document is beside, an org at any level among them; moving it to another, deleting and restoring it.
//!
//! A document is written beside a node, and its page follows that node's
//! document rows until somebody says otherwise. Moving it changes what it
//! follows, and nothing else: its owners, its own rows, its blocks and its
//! history go with it. It stays within the tree of orgs it was made in,
//! because the people its page names are members of that tree, and a move
//! to another would hand it to strangers. Deleting it is soft: it can be
//! restored until it is purged, as the other things that are deleted.

use orch_types::{Id, Time};

/// What is known of the place a document is to be moved to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Destination {
    /// The root org of its tree.
    pub root: Id,
    /// Whether it, or a node above it, is deleted.
    pub deleted: bool,
    /// Whether `docs.enabled` resolves true there.
    pub docs_enabled: bool,
}

/// Why a document is not moved where it was asked to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MoveRefusal {
    /// It is beside that node already.
    #[error("the document is beside that node already")]
    Already,
    /// The node is in another tree of orgs.
    #[error("the document stays within the tree of orgs it was made in")]
    OtherTree,
    /// The node is deleted.
    #[error("that node is deleted")]
    Gone,
    /// Documents are switched off there.
    #[error("documents are switched off there")]
    Disabled,
}

/// Whether a document that is beside `from` in the tree of `root` may be
/// moved beside `to`.
pub fn check_move(
    from: Id,
    root: Id,
    to: Id,
    destination: &Destination,
) -> Result<(), MoveRefusal> {
    if destination.deleted {
        return Err(MoveRefusal::Gone);
    }
    if destination.root != root {
        return Err(MoveRefusal::OtherTree);
    }
    if from == to {
        return Err(MoveRefusal::Already);
    }
    if !destination.docs_enabled {
        return Err(MoveRefusal::Disabled);
    }
    Ok(())
}

/// Whether a document deleted at `deleted_at` can still be restored at
/// `now`: until the soft-deleted rows are purged, `retention_days` after.
#[must_use]
pub fn restorable(deleted_at: Time, now: Time, retention_days: i64) -> bool {
    let kept_for = retention_days.saturating_mul(86_400_000);
    now.unix_millis().saturating_sub(deleted_at.unix_millis()) < kept_for
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_types::new_id;

    #[test]
    fn a_document_moves_within_its_tree_to_another_node_where_documents_are_on() {
        let (root, from, to) = (new_id(), new_id(), new_id());
        let there = Destination {
            root,
            deleted: false,
            docs_enabled: true,
        };
        assert_eq!(check_move(from, root, to, &there), Ok(()));
        assert_eq!(
            check_move(from, root, from, &there),
            Err(MoveRefusal::Already)
        );
        assert_eq!(
            check_move(
                from,
                root,
                to,
                &Destination {
                    root: new_id(),
                    ..there
                }
            ),
            Err(MoveRefusal::OtherTree)
        );
        assert_eq!(
            check_move(
                from,
                root,
                to,
                &Destination {
                    deleted: true,
                    ..there
                }
            ),
            Err(MoveRefusal::Gone)
        );
        assert_eq!(
            check_move(
                from,
                root,
                to,
                &Destination {
                    docs_enabled: false,
                    ..there
                }
            ),
            Err(MoveRefusal::Disabled)
        );
    }

    #[test]
    fn a_deleted_document_can_be_restored_until_it_is_purged() {
        let deleted: Time = "2026-10-01T00:00:00.000Z".parse().unwrap();
        let day = |n: i64| Time::new(deleted.get() + chrono::Duration::days(n));
        assert!(restorable(deleted, day(0), 30));
        assert!(restorable(deleted, day(29), 30));
        assert!(!restorable(deleted, day(30), 30));
        assert!(!restorable(deleted, day(0), 0));
    }
}
