//! What the engine tells the realtime side the moment it knows it.
//!
//! The rows of a thread reach every process through the database (the
//! sequence and its notification). Growing text is the one thing that does
//! not wait for that: it is handed to a [`LiveSink`] as it arrives, and the
//! realtime server, which implements the trait, sends it on to whoever has the
//! thread open. What is handed over has been applied, or is being applied in
//! the transaction that will make the same text durable; a client that sees
//! the same stretch twice ignores the second, and one that misses a stretch
//! reads the row.

use orch_link_contract::events::DeltaField;
use orch_types::Id;

/// A stretch of text that was added to an item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveDelta {
    /// The thread.
    pub thread_id: Id,
    /// The item it grew.
    pub item_id: Id,
    /// The turn the item is in.
    pub turn_id: Option<Id>,
    /// The line of the conversation the item is in, `main` or `sub:…`.
    pub stream: String,
    /// The part of the item that grew.
    pub field: DeltaField,
    /// How long that part was before, in its own unit.
    pub offset: i64,
    /// How much it grew by, in the same unit.
    pub len: i64,
    /// What was added.
    pub data: String,
}

/// Where growing text is forwarded.
pub trait LiveSink: Send + Sync {
    /// Whether an authorized live view still follows this job, including readers on other processes.
    fn job_is_watched(&self, _job: Id) -> bool {
        false
    }
    /// A stretch of text was added to an item.
    fn delta(&self, delta: &LiveDelta);

    /// Approvals of a thread were raised or decided: these people may decide
    /// them, and have a list to reload.
    fn approvals_changed(&self, _deciders: &[Id]) {}

    /// How many people have the document open now, here and, as far as the others have said, on the other
    /// processes: what an agent is told before it deletes or moves one. A sink with no realtime side has nobody.
    fn document_readers(&self, _document: Id) -> usize {
        0
    }

    /// Whether a person currently has this thread visible on an authorized live connection.
    /// This reads only the realtime side's existing presence, without I/O. With no such side,
    /// absence is unknown and the normal completion notification is retained.
    fn thread_is_visible(&self, _thread: Id, _person: Id) -> bool {
        false
    }
}

/// A sink that forwards nothing, for a process with no realtime side.
#[derive(Debug, Clone, Copy, Default)]
pub struct Discard;

impl LiveSink for Discard {
    fn delta(&self, _: &LiveDelta) {}
}
