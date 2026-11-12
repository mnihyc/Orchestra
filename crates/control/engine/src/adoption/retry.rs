//! Prepared retry application over the shared original retained-history supplier.
pub use super::RetainedHistory as Source;
use super::*;
use orch_api_contract::threads::AdoptHistoryState;
use orch_db::queries::threads;
use std::sync::Arc;

pub async fn select(conn: &mut Conn, thread: Id) -> Result<Source, AdoptError> {
    retained_history_source(conn, thread)
        .await?
        .ok_or(AdoptError::Gone)
}
pub async fn require(
    conn: &mut Conn,
    principal: &Principal,
    source: &Source,
) -> Result<(), AdoptError> {
    require_retry_history(conn, principal, source.thread()).await
}
pub async fn require_thread(
    conn: &mut Conn,
    principal: &Principal,
    thread: Id,
) -> Result<(), AdoptError> {
    require_retry_history(conn, principal, thread).await
}
pub(super) async fn principal(
    conn: &mut Conn,
    caller: &Caller,
    source: &Source,
    now: Time,
) -> Result<Principal, AdoptError> {
    use orch_api_contract::{routes::Endpoint, threads::ThreadAdoptRetryHistory};
    let facts = operation::retry_history_facts(conn, source).await?;
    let plan = crate::operation_scope::sources::bind_required(
        ThreadAdoptRetryHistory::ROUTE,
        &orch_api_contract::adoption_sources::RETRY_PREPARATION,
        &facts,
    )
    .map_err(EngineError::unreadable)?;
    let caller = plan.admit(caller, false).map_err(EngineError::unreadable)?;
    let principal = Principal::load(conn, caller, now)
        .await
        .map_err(EngineError::from)?;
    require(conn, &principal, source).await?;
    Ok(principal)
}

/// One actual observation. Factual failure stays native; an ended source is always fatal.
pub struct Fetched {
    source: Source,
    outcome: Outcome,
    record_source: Option<Arc<FetchedHistory>>,
    ceiling: Option<(orch_authz::CapturedOperation, Option<Time>)>,
}
enum Outcome {
    Unchanged,
    Failed {
        missing: AdoptHistoryMissing,
        reason: String,
    },
    Read {
        read: Box<AgentReadSessionResult>,
        entries: Vec<RecordedEntry>,
        turns: usize,
    },
}
impl Fetched {
    pub fn origin(&self) -> Option<&orch_authz::CapturedOperation> {
        self.ceiling.as_ref().map(|(origin, _)| origin)
    }
    pub fn until(&self) -> Option<Time> {
        self.ceiling.as_ref().and_then(|(_, until)| *until)
    }
    pub fn source(&self) -> &Source {
        &self.source
    }
}
/// The native writer's actual locked/fenced operands on the guarded transaction.
#[derive(Clone)]
pub struct Prepared {
    fetched: Arc<Fetched>,
    transaction: i64,
    actor: Id,
    unchanged: bool,
}
impl Prepared {
    pub fn thread(&self) -> Id {
        self.fetched.source.thread()
    }
    pub fn unchanged(&self) -> bool {
        self.unchanged
    }
    pub fn source(&self) -> &Source {
        &self.fetched.source
    }
    pub fn until(&self) -> Option<Time> {
        self.fetched.until()
    }
}
/// Accepted identity and whether this attempt actually updated its original history marker.
#[derive(Clone)]
pub struct Written {
    pub thread: Id,
    pub changed: bool,
}

impl Engine {
    /// One source-aware native history observation, released from every database snapshot during IO.
    pub async fn fetch_retry_adopted_history(
        &self,
        caller: &Caller,
        source: Source,
        credential: operation::HttpCredential,
        transport: Option<Arc<dyn operation::Transport>>,
        until: Option<Time>,
    ) -> Result<Fetched, AdoptError> {
        if !source.history_missing() {
            return Ok(Fetched {
                source,
                outcome: Outcome::Unchanged,
                record_source: None,
                ceiling: None,
            });
        }
        {
            let mut conn = self.inner.db.acquire().await?;
            if let Some(status) = history_status(&mut conn, source.thread()).await? {
                match status.state {
                    AdoptHistoryState::Read => {
                        return Ok(Fetched {
                            source,
                            outcome: Outcome::Unchanged,
                            record_source: None,
                            ceiling: None,
                        });
                    }
                    AdoptHistoryState::Unsupported => return Err(AdoptError::Unsupported),
                    AdoptHistoryState::Offline => return Err(AdoptError::Offline),
                    AdoptHistoryState::Unavailable => return Err(AdoptError::NoMachine),
                    AdoptHistoryState::Unreadable => {}
                }
            }
        }
        let has_turns: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM turns WHERE segment_id=$1)")
                .bind(source.segment())
                .fetch_one(self.inner.db.pool())
                .await
                .map_err(orch_db::DbError::from)?;
        let observed = self
            .fetch_retained_adopted_history(caller, credential, source.clone(), transport, until)
            .await?;
        let outcome = match observed.record()? {
            HistoryRecord::Unsupported(reason) => Outcome::Failed {
                missing: AdoptHistoryMissing::Unsupported,
                reason: reason.to_owned(),
            },
            HistoryRecord::Failed(reason) => Outcome::Failed {
                missing: AdoptHistoryMissing::Unreadable,
                reason: reason.to_owned(),
            },
            HistoryRecord::Read {
                entries: Err(reason),
                ..
            } => Outcome::Failed {
                missing: AdoptHistoryMissing::Unreadable,
                reason: reason.to_owned(),
            },
            HistoryRecord::Read {
                record,
                entries: Ok(entries),
            } => {
                let mut entries = entries.to_vec();
                if (has_turns
                    || observed
                        .session_updated_at()
                        .is_some_and(|updated| updated > source.before()))
                    && entries.iter().any(|entry| at(entry).is_none())
                {
                    return Err(AdoptError::Failed("the harness supplies no time for some entries, so earlier history cannot be separated from work done after adoption; nothing was imported".into()));
                }
                entries.retain(|entry| at(entry).is_none_or(|time| time <= source.before()));
                let turns = entries
                    .iter()
                    .map(|entry| match entry {
                        RecordedEntry::Said(words) => words.turn,
                        RecordedEntry::Item(item) => item.turn,
                    })
                    .collect::<std::collections::HashSet<_>>()
                    .len();
                Outcome::Read {
                    read: Box::new(record.clone()),
                    entries,
                    turns,
                }
            }
        };
        let mut tx = self.inner.db.begin().await?;
        let check = observed
            .check_in(&mut tx, caller, self.inner.clock.as_ref())
            .await?;
        let origin = orch_authz::CapturedOperation::capture(check.caller())
            .map_err(EngineError::unreadable)?;
        let until = check.until();
        check.check(self.inner.clock.as_ref())?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(Fetched {
            source,
            outcome,
            record_source: Some(Arc::new(observed)),
            ceiling: Some((origin, until)),
        })
    }
    /// Hold the same original source, native session and actual history row before application.
    pub async fn prepare_retry_adopted_history(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        fetched: Arc<Fetched>,
    ) -> Result<Prepared, AdoptError> {
        require(conn, principal, &fetched.source).await?;
        let source = &fetched.source;
        if let Some(record) = &fetched.record_source {
            let check = record
                .check_in(conn, principal.caller(), self.inner.clock.as_ref())
                .await?;
            check.check(self.inner.clock.as_ref())?;
        }
        adoption::lock_session(conn, source.native_session_ref()).await?;
        threads::lock(conn, source.thread())
            .await?
            .ok_or(AdoptError::Gone)?;
        let current = select(conn, source.thread()).await?;
        if !source.same_source(&current) {
            return Err(AdoptError::Failed("the original adopted history source changed while it was being read; nothing was imported".into()));
        }
        if let Outcome::Read { entries, .. } = &fetched.outcome {
            let has_turns: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM turns WHERE segment_id=$1)")
                    .bind(source.segment())
                    .fetch_one(&mut *conn)
                    .await
                    .map_err(orch_db::DbError::from)?;
            if has_turns && entries.iter().any(|entry| at(entry).is_none()) {
                return Err(AdoptError::Failed("the session ran while its history was read, and the harness supplies no time boundary; nothing was imported".into()));
            }
        }
        let transaction = sqlx::query_scalar("SELECT txid_current()::bigint")
            .fetch_one(&mut *conn)
            .await
            .map_err(orch_db::DbError::from)?;
        let unchanged = !current.history_missing() || matches!(fetched.outcome, Outcome::Unchanged);
        Ok(Prepared {
            fetched,
            transaction,
            actor: principal.user(),
            unchanged,
        })
    }
    pub async fn apply_retry_adopted_history<'a>(
        &self,
        mut tx: orch_db::Tx,
        principal: &Principal,
        prepared: Prepared,
        before: impl FnOnce() -> Result<(), EngineError> + Send + 'a,
        before_commit: impl FnOnce() -> Result<(), EngineError> + Send + 'a,
    ) -> Result<Written, AdoptError> {
        let transaction: i64 = sqlx::query_scalar("SELECT txid_current()::bigint")
            .fetch_one(&mut *tx)
            .await
            .map_err(orch_db::DbError::from)?;
        if transaction != prepared.transaction || principal.user() != prepared.actor {
            return Err(EngineError::unreadable(
                "the native retry preparation changed its transaction or actor",
            )
            .into());
        }
        let source = &prepared.fetched.source;
        if prepared.unchanged {
            return Ok(Written {
                thread: source.thread(),
                changed: false,
            });
        }
        let finished = {
            let mut unit = Unit::with_first_effect(&mut tx, self.inner.clock.now(), before);
            let mut current: orch_types::Json =
                sqlx::query_scalar("SELECT payload FROM items WHERE id=$1 AND thread_id=$2")
                    .bind(source.marker())
                    .bind(source.thread())
                    .fetch_one(&mut *unit.conn)
                    .await
                    .map_err(orch_db::DbError::from)?;
            unit.before_effect()?;
            let mut report = None;
            match &prepared.fetched.outcome {
                Outcome::Unchanged => {
                    return Ok(Written {
                        thread: source.thread(),
                        changed: false,
                    });
                }
                Outcome::Failed { missing, reason } => {
                    current["data"]["history_missing"] =
                        serde_json::to_value(missing).map_err(EngineError::from)?;
                    current["data"]["history_error"] = json!(reason);
                }
                Outcome::Read {
                    read,
                    entries,
                    turns,
                } => {
                    self.write_session_record(
                        &mut unit,
                        source.thread(),
                        source.segment(),
                        source.author(),
                        entries,
                    )
                    .await?;
                    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM items WHERE thread_id=$1 AND segment_id=$2 AND before_orch").bind(source.thread()).bind(source.segment()).fetch_one(&mut *unit.conn).await.map_err(orch_db::DbError::from)?;
                    current["data"]["history_missing"] = serde_json::Value::Null;
                    current["data"]["history_error"] = serde_json::Value::Null;
                    current["data"]["turns"] = json!(turns);
                    current["data"]["items"] = json!(total);
                    let mut value =
                        serde_json::to_value(&read.report).map_err(EngineError::from)?;
                    value["items_total"] = json!(total);
                    report = Some(value);
                }
            }
            let seq = unit.stamp(source.thread()).await?;
            sqlx::query("UPDATE items SET payload=$2,seq_updated=$3,updated_at=$4 WHERE id=$1")
                .bind(source.marker())
                .bind(current)
                .bind(seq)
                .bind(unit.now())
                .execute(&mut *unit.conn)
                .await
                .map_err(orch_db::DbError::from)?;
            if let Some(report) = report {
                adoption::set_adopted_report(&mut *unit.conn, source.segment(), &report).await?;
            }
            unit.finish().await?
        };
        before_commit()?;
        self.commit(tx, finished).await?;
        Ok(Written {
            thread: source.thread(),
            changed: true,
        })
    }
}
fn at(entry: &RecordedEntry) -> Option<Time> {
    match entry {
        RecordedEntry::Said(words) => words.at,
        RecordedEntry::Item(item) => item.at,
    }
}
