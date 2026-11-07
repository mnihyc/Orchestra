//! The native complete header stage precedes private audience, material and effects.

use super::{
    Actor, Command, Conn, Error, Item, LockedTree, LockedTreeState, Request, access, lock_tree,
    prepare, require_replayed_entries, transaction_id,
};
use crate::document_tree::{Directory, TextCopy};
use orch_db::queries::documents::containers::EntryRow;
use orch_types::{Id, Time};
use std::collections::BTreeSet;

/// Factual source membership from the existing planner, not an authorization or effect lease.
/// Out-of-transaction discovery is provisional and cannot be moved into an action transaction.
pub struct OperationSources {
    pub(super) locked: LockedTree,
    transaction: Option<String>,
}

impl OperationSources {
    /// The command of the actual validated native request.
    #[must_use]
    pub fn command(&self) -> Command {
        self.locked.request.command
    }

    /// Only the native actor/request record can establish that this is an existing outcome.
    #[must_use]
    pub fn is_replay(&self) -> bool {
        matches!(&self.locked.state, LockedTreeState::Replay { .. })
    }

    /// Actual selected source rows for a new act; replay has no newly selected input rows.
    #[must_use]
    pub fn roots(&self) -> Option<&[EntryRow]> {
        match &self.locked.state {
            LockedTreeState::New(prepared) => Some(&prepared.roots),
            LockedTreeState::Replay { .. } => None,
        }
    }

    /// The complete native source closure, including hidden descendants and native live filtering.
    /// None denotes actual durable replay, never an unperformed or failed selector.
    #[must_use]
    pub fn entries(&self) -> Option<&[EntryRow]> {
        match &self.locked.state {
            LockedTreeState::New(prepared) => Some(&prepared.entries),
            LockedTreeState::Replay { .. } => None,
        }
    }

    /// The actual locked destination and its hierarchy node/root, for a new act with a destination.
    #[must_use]
    pub fn destination(&self) -> Option<(Directory, Id, Id)> {
        match &self.locked.state {
            LockedTreeState::New(prepared) => prepared.destination.map(|(node, root, parent)| {
                (
                    parent.map_or(Directory::NodeDocs(node), Directory::Container),
                    node,
                    root,
                )
            }),
            LockedTreeState::Replay { .. } => None,
        }
    }

    /// Actual recorded restoration parents, or the actual selected explicit destination.
    /// None means a different native command or durable replay, rather than checked-empty parents.
    #[must_use]
    pub fn restoration_destinations(&self) -> Option<Vec<Directory>> {
        let LockedTreeState::New(prepared) = &self.locked.state else {
            return None;
        };
        if self.command() != Command::Restore {
            return None;
        }
        Some(
            prepared
                .roots
                .iter()
                .map(|row| {
                    self.locked.request.to.unwrap_or_else(|| {
                        row.parent_container_id
                            .map_or(Directory::NodeDocs(row.node_id), Directory::Container)
                    })
                })
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
        )
    }

    /// Typed identities from the actual native durable result, without exposing its old body.
    #[must_use]
    pub fn replay_items(&self) -> Option<&[Item]> {
        match &self.locked.state {
            LockedTreeState::Replay { items, .. } => Some(items),
            LockedTreeState::New(_) => None,
        }
    }
}

pub(super) async fn prepare_with_copy(
    conn: &mut Conn,
    actor: Actor<'_>,
    request: &Request,
    text_copy: Option<&TextCopy>,
) -> Result<OperationSources, Error> {
    let locked = lock_tree(conn, actor, request, text_copy).await?;
    let transaction = if orch_db::queries::transaction_guard::in_transaction(conn) {
        Some(transaction_id(conn).await?)
    } else {
        None
    };
    Ok(OperationSources {
        locked,
        transaction,
    })
}

/// Use the same complete native header selector for every document-tree command.
/// No audience, file/block material, new permission, history or operation result is produced here.
/// Existing serialization scope rows retain their native behavior.
pub async fn prepare_operation_sources_in(
    conn: &mut Conn,
    actor: Actor<'_>,
    request: &Request,
) -> Result<OperationSources, Error> {
    prepare_with_copy(conn, actor, request, None).await
}

pub(super) async fn check_association(
    conn: &mut Conn,
    actor: Actor<'_>,
    sources: &OperationSources,
) -> Result<(), Error> {
    match (
        &sources.transaction,
        orch_db::queries::transaction_guard::in_transaction(conn),
    ) {
        (Some(original), true) if transaction_id(conn).await? == *original => {}
        (None, false) => {}
        _ => return Err(Error::Stale),
    }
    let current = access::identity(conn, actor).await?;
    let original = &sources.locked.identity;
    if current.kind != original.kind
        || current.id != original.id
        || current.owner != original.owner
        || current.thread != original.thread
    {
        return Err(Error::Denied);
    }
    Ok(())
}

/// The original ordinary gate over that exact source stage, without computing an audience.
/// Provisional discovery remains observational and is never a completed action preparation.
pub async fn require_operation_sources_in(
    conn: &mut Conn,
    actor: Actor<'_>,
    sources: &OperationSources,
    now: Time,
) -> Result<(), Error> {
    check_association(conn, actor, sources).await?;
    match &sources.locked.state {
        LockedTreeState::New(prepared) => {
            prepare::require(conn, actor, &sources.locked.request, prepared, now).await
        }
        LockedTreeState::Replay { receipt, .. } => {
            require_replayed_entries(conn, actor, receipt, now).await
        }
    }
}
