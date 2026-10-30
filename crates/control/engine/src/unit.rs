//! The transaction a change of a thread is made in.
//!
//! A [`Unit`] wraps the connection of a transaction that its owner holds. It
//! gives out the thread sequence (every row a reader of the thread sees is
//! stamped with a number of its own, taken while the thread's row is
//! locked), remembers the highest number given for each thread, and, when
//! the work is done, queues one announcement for each thread touched, which
//! PostgreSQL delivers when the transaction commits. It also collects the
//! threads whose follow-up work (a start to carry out, a steer to hand
//! over) is to be taken up once the transaction has committed.

use crate::error::EngineError;
use chrono::{DateTime, Utc};
use orch_db::{Conn, sequences};
use orch_types::{Id, Seq};
use std::collections::{BTreeMap, BTreeSet};

/// The work of one transaction.
#[derive(Debug)]
pub struct Unit<'c> {
    /// The connection of the transaction.
    pub conn: &'c mut Conn,
    now: DateTime<Utc>,
    touched: BTreeMap<Id, Seq>,
    wakes: BTreeSet<Id>,
    approval_deciders: BTreeSet<Id>,
    source_checked: bool,
}

/// What a unit leaves for after the commit.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Finished {
    /// The threads whose follow-up work is to be taken up.
    pub wakes: BTreeSet<Id>,
    /// The highest number given for each thread touched.
    pub touched: BTreeMap<Id, Seq>,
}

impl<'c> Unit<'c> {
    /// A unit over the connection of a transaction, at the moment `now`.
    pub fn new(conn: &'c mut Conn, now: DateTime<Utc>) -> Self {
        Self {
            conn,
            now,
            touched: BTreeMap::new(),
            wakes: BTreeSet::new(),
            approval_deciders: BTreeSet::new(),
            source_checked: false,
        }
    }

    /// The moment the work is done at.
    #[must_use]
    pub const fn now(&self) -> DateTime<Utc> {
        self.now
    }

    /// The next number of the sequence of `thread`. The thread's row is
    /// locked from now until the transaction ends.
    pub async fn stamp(&mut self, thread: Id) -> Result<Seq, EngineError> {
        self.check_tool_source().await?;
        let seq = sequences::next(&mut *self.conn, thread)
            .await?
            .ok_or_else(|| EngineError::unreadable(format!("there is no thread {thread}")))?;
        self.touched.insert(thread, seq);
        Ok(seq)
    }

    /// Holds a fresh own-tool source through this transaction's effects and commit.
    /// A tool may legitimately interrupt itself later in this same transaction.
    pub(crate) async fn check_tool_source(&mut self) -> Result<(), EngineError> {
        if !self.source_checked {
            crate::tools::source::check(self.conn).await?;
            self.source_checked = true;
        }
        Ok(())
    }

    /// Asks that the follow-up work of `thread` be taken up after the
    /// commit.
    pub fn wake(&mut self, thread: Id) {
        self.wakes.insert(thread);
    }

    /// Keeps approval news behind the same commit as the decision and thread state.
    pub fn approvals_changed(&mut self, people: &[Id]) {
        self.approval_deciders.extend(people.iter().copied());
    }

    /// Queues the announcement of every thread touched and returns what is
    /// left for after the commit. The transaction is the caller's to commit.
    pub async fn finish(mut self) -> Result<Finished, EngineError> {
        self.check_tool_source().await?;
        for (thread, seq) in &self.touched {
            sequences::notify(&mut *self.conn, *thread, *seq).await?;
        }
        if !self.approval_deciders.is_empty() {
            let people: Vec<_> = self.approval_deciders.into_iter().collect();
            // Native report application commits outside Engine::commit too. Queue on this
            // connection so every API process hears the news only after either caller commits.
            sequences::notify_user_event(
                &mut *self.conn,
                &people,
                &serde_json::json!({ "what": "approvals" }),
            )
            .await?;
        }
        Ok(Finished {
            wakes: self.wakes,
            touched: self.touched,
        })
    }
}
