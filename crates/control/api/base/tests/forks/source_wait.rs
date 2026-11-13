//! A request held at its source thread's row is let go only once it has taken its snapshot, and a failure says which wait it was.
//!
//! A case locks the thread's row in a transaction of its own and sends the request. Other things of the same database
//! can wait for that transaction too (the engine's work on the thread, a write that refers to it), so a helper that
//! asks for any waiting backend can let the case change the source, or end the key, before the request has taken the
//! snapshot it compares with: the request then starts after the change and is rightly served. The request is
//! therefore followed by a recorder of its own, and the database is asked only once the request has finished its
//! first authorization, which is the snapshot every later check compares with, and has begun to fetch what it needs:
//! from there on, whatever it waits for, a change made under the lock lands after its first check and before its
//! first write. What waited on the case's transaction at that moment is kept for the failure message.
//!
//! The request is followed by a record of the boundaries it enters that is written where it enters them
//! (`orch_api_core::testing::follow`), and by the recorder of tracing events only for the failure's account: the
//! events have been found to miss a request that was at its row with its first two boundaries behind it, and a gate
//! that opened on an event then never opened.
//!
//! The case's transaction (`hold`) and the database are asked over connections of their own, never through the pool
//! the request works in: a pool of four that held the case's transaction, the two connections the request holds at
//! its wait (one for what it fetches, one for its checkpoint) and the helper's question would have no connection
//! left for anything else of the world, and a question that waits for a connection is a question nobody sees. When the request is not found at the row, the helper says what the request
//! had done (the boundaries it entered, the events it was heard to produce), what every backend of the database was
//! doing, the locks nobody was granted, and how the pool and the runtime stood, so that the failure tells whether the
//! request waited on the case, on something else of the database, or on something the database cannot see.
use super::*;
use crate::support::failure_trace::{FailureTrace, Recorder};
use orch_api_core::testing::{Boundaries, RawReply};
use sqlx::Connection as _;
use std::sync::Arc;
use thread_history_scope::Proof;
use tokio::task::JoinHandle;
use tracing::instrument::WithSubscriber as _;

/// How often the database is asked while the request is on its way.
const POLL: Duration = Duration::from_millis(2);

/// How long the report of a failure may take to ask the database: it is given only once the wait has failed, and it
/// is not a wait of the case.
const REPORT: std::time::Duration = std::time::Duration::from_secs(10);

/// A transaction of the case's own, on a connection of its own, outside the pool the world works in. The case holds
/// the source row for as long as the request waits, and a pool of four that also lent the case a connection for
/// that would leave the request, the engine's tasks and the rest of the world three.
pub(super) async fn hold(
    w: &Forks,
) -> Result<sqlx::Transaction<'static, sqlx::Postgres>, sqlx::Error> {
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_lazy(&w.s.rig.world.db.url)?
        .begin()
        .await
}

/// What follows the request: the record of its boundaries and the recorder of its events.
pub(super) struct Follow {
    recorder: Recorder,
    log: Arc<Boundaries>,
}

impl Follow {
    /// Runs the request: `follow.run(call.send_bytes()).await`, in the task that sends it.
    pub(super) async fn run<F: std::future::Future>(self, work: F) -> F::Output {
        orch_api_core::testing::follow(self.log, work.with_subscriber(self.recorder)).await
    }
}

/// The wait of one request on the source row, which the case's own transaction holds.
pub(super) struct SourceWait {
    mode: &'static str,
    operation: &'static str,
    trace: FailureTrace,
    log: Arc<Boundaries>,
    started: std::time::Instant,
    waiting: Vec<String>,
    active: Vec<String>,
}

impl SourceWait {
    /// `operation` is the route of the request and `mode` the variant of the case that runs.
    pub(super) fn new(operation: &'static str, mode: &'static str) -> Self {
        let trace = FailureTrace::new("fork request at its source thread's row", rig::PATIENCE);
        trace.mark(format!("{operation}, mode {mode}: the case holds the row"));
        Self {
            mode,
            operation,
            trace,
            log: Boundaries::new(),
            started: std::time::Instant::now(),
            waiting: Vec::new(),
            active: Vec::new(),
        }
    }

    /// What the request is sent under: `wait.follow().run(call.send_bytes())`, in the task that sends it.
    pub(super) fn follow(&self) -> Follow {
        Follow {
            recorder: self.trace.recorder(),
            log: self.log.clone(),
        }
    }

    /// Waits until the request has finished its first authorization and a statement waits for the transaction of the
    /// backend `blocker`.
    pub(super) async fn reached(
        &mut self,
        w: &Forks,
        blocker: i32,
        pending: &JoinHandle<RawReply>,
    ) -> Proof {
        let mut probe = sqlx::PgConnection::connect(&w.s.rig.world.db.url).await?;
        let log = self.log.clone();
        let operation = self.operation;
        let mut last = String::from("the request had not finished its first authorization");
        let found = tokio::time::timeout(rig::PATIENCE, async {
            loop {
                if pending.is_finished() {
                    return Err(format!("the request answered before it waited; {last}"));
                }
                // The fetch of the initial phase follows the snapshot of the first authorization.
                if log.entered(operation, "initial.fetch") {
                    let waiting = waiting_on(&mut probe, blocker)
                        .await
                        .map_err(|error| error.to_string())?;
                    if !waiting.is_empty() {
                        return Ok(waiting);
                    }
                    last = String::from(
                        "the request is past its first authorization and nothing waits on the case yet",
                    );
                }
                tokio::time::sleep(POLL).await;
            }
        })
        .await;
        let why = match found {
            Ok(Ok(waiting)) => {
                self.waiting = waiting
                    .iter()
                    .map(|(pid, query)| format!("backend {pid}: {query}"))
                    .collect();
                self.active = active_beside(&mut probe, blocker)
                    .await
                    .unwrap_or_else(|error| vec![error.to_string()]);
                self.trace.mark(format!(
                    "let go; waiting on the case: {:?}; other active backends: {:?}",
                    self.waiting, self.active
                ));
                return Ok(());
            }
            Ok(Err(why)) => why,
            Err(_) => format!(
                "{} in mode {}: nothing waited on the case's transaction within {:?}; {last}",
                self.operation,
                self.mode,
                rig::PATIENCE
            ),
        };
        let verdict = self.give_up(w, &mut probe, blocker, pending).await;
        Err(format!("{why}; {verdict}").into())
    }

    /// The answer of a request that should have been refused, with the mode and what waited when the case went on.
    pub(super) fn explain(&self, reply: &RawReply) -> String {
        format!(
            "{} in mode {} was answered {} {}; the case went on while {:?} waited on its transaction, and these other backends were active: {:?}",
            self.operation,
            self.mode,
            reply.status,
            String::from_utf8_lossy(&reply.raw),
            self.waiting,
            self.active
        )
    }

    /// The case went through: nothing is printed.
    pub(super) fn finish(&mut self) {
        self.trace.finish();
    }

    /// Prints what the request and the database were doing when the wait failed, and returns the short account of it
    /// that goes into the error: how many backends wait for the case, how many for something else, how many sit idle
    /// in a transaction, how the pool and the request stood.
    async fn give_up(
        &self,
        w: &Forks,
        probe: &mut sqlx::PgConnection,
        blocker: i32,
        pending: &JoinHandle<RawReply>,
    ) -> String {
        let recorder = self.trace.recorder();
        let pool = w.s.rig.db.pool();
        let runtime = tokio::runtime::Handle::current().metrics();
        let fetched = self.log.at(self.operation, "initial.fetch");
        let mut report = vec![format!(
            "{} in mode {}: the request {}; it entered [{}] (written where it enters them); the tracing log heard [{}] and {}",
            self.operation,
            self.mode,
            if pending.is_finished() {
                "had answered"
            } else {
                "had not answered"
            },
            self.log.all(self.started).join(", "),
            recorder
                .boundaries()
                .iter()
                .map(|(operation, phase)| format!("{operation}:{phase}"))
                .collect::<Vec<_>>()
                .join(", "),
            recorder.heard(),
        )];
        report.push(format!(
            "  pool: {} connections, {} idle; runtime: {} workers, {} tasks alive, {} in the global queue",
            pool.size(),
            pool.num_idle(),
            runtime.num_workers(),
            runtime.num_alive_tasks(),
            runtime.global_queue_depth(),
        ));
        let backends = match tokio::time::timeout(REPORT, backends(probe)).await {
            Ok(Ok(backends)) => backends,
            Ok(Err(error)) => {
                report.push(format!("  the database could not be asked: {error}"));
                Vec::new()
            }
            Err(_) => {
                report.push(String::from(
                    "  the database did not answer the question about its backends",
                ));
                Vec::new()
            }
        };
        let others = backends.iter().filter(|backend| backend.pid != blocker);
        let for_the_case = others
            .clone()
            .filter(|backend| backend.blocked_by.contains(&blocker))
            .count();
        let for_others = others
            .clone()
            .filter(|backend| backend.wait_type == "Lock" && !backend.blocked_by.contains(&blocker))
            .count();
        let idle_in_transaction = others
            .clone()
            .filter(|backend| backend.state == "idle in transaction")
            .count();
        let executing = others
            .clone()
            .filter(|backend| backend.state == "active" && backend.wait_type != "Lock")
            .count();
        for backend in &backends {
            // When a transaction began, against when the request entered its fetch: one that began after is the
            // request's own checkpoint, one that began before is not.
            let began = match fetched {
                Some(entered) if backend.transaction_age >= 0.0 => {
                    let after = entered.elapsed().as_secs_f64() - backend.transaction_age;
                    if after >= 0.0 {
                        format!(
                            ", began {:.0} ms after the request entered initial.fetch",
                            after * 1000.0
                        )
                    } else {
                        format!(
                            ", began {:.0} ms before the request entered initial.fetch",
                            -after * 1000.0
                        )
                    }
                }
                _ => String::new(),
            };
            report.push(format!(
                "  backend {}{}: {} {}:{} in transaction {:.1}s{began}, in state {:.1}s, blocked by {:?}: {}",
                backend.pid,
                if backend.pid == blocker {
                    " (the case's transaction)"
                } else {
                    ""
                },
                backend.state,
                backend.wait_type,
                backend.wait_event,
                backend.transaction_age,
                backend.state_age,
                backend.blocked_by,
                backend.query,
            ));
        }
        match tokio::time::timeout(REPORT, ungranted(probe)).await {
            Ok(Ok(locks)) if locks.is_empty() => {
                report.push(String::from("  no lock is waited for"))
            }
            Ok(Ok(locks)) => report.extend(locks),
            Ok(Err(error)) => report.push(format!("  the locks could not be asked: {error}")),
            Err(_) => report.push(String::from(
                "  the database did not answer the question about its locks",
            )),
        }
        if self.log.all(self.started).is_empty() {
            report.push(String::from("  the request entered no native boundary"));
        }
        eprintln!("{}", report.join("\n"));
        format!(
            "of the other backends of the database {for_the_case} wait for the case's transaction, {for_others} wait for something else, {idle_in_transaction} sit idle in a transaction and {executing} execute; the pool has {} idle of {}; the request had {}answered and entered {} native boundaries (the tracing log heard {})",
            pool.num_idle(),
            pool.size(),
            if pending.is_finished() { "" } else { "not " },
            self.log.all(self.started).len(),
            recorder.boundaries().len(),
        )
    }
}

/// What a backend of the database was doing.
struct Backend {
    pid: i32,
    state: String,
    wait_type: String,
    wait_event: String,
    transaction_age: f64,
    state_age: f64,
    blocked_by: Vec<i32>,
    query: String,
}

/// Every other backend of this database, with what it runs and whom it waits for.
async fn backends(probe: &mut sqlx::PgConnection) -> Result<Vec<Backend>, sqlx::Error> {
    let rows = sqlx::query_as::<_, (i32, String, String, String, f64, f64, Vec<i32>, String)>(
        r"SELECT pid, COALESCE(state, ''), COALESCE(wait_event_type, ''), COALESCE(wait_event, ''),
                 COALESCE(EXTRACT(EPOCH FROM now() - xact_start), -1)::float8,
                 COALESCE(EXTRACT(EPOCH FROM now() - state_change), -1)::float8,
                 pg_blocking_pids(pid),
                 left(regexp_replace(COALESCE(query, ''), '\s+', ' ', 'g'), 160)
            FROM pg_stat_activity
           WHERE datname = current_database() AND pid <> pg_backend_pid()
           ORDER BY pid",
    )
    .fetch_all(&mut *probe)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(pid, state, wait_type, wait_event, transaction_age, state_age, blocked_by, query)| {
                Backend {
                    pid,
                    state,
                    wait_type,
                    wait_event,
                    transaction_age,
                    state_age,
                    blocked_by,
                    query,
                }
            },
        )
        .collect())
}

/// The locks that a backend of this database asks for and has not been granted.
async fn ungranted(probe: &mut sqlx::PgConnection) -> Result<Vec<String>, sqlx::Error> {
    let rows = sqlx::query_as::<_, (i32, String, String, String, String)>(
        "SELECT COALESCE(l.pid, 0), COALESCE(l.locktype, ''), COALESCE(l.mode, ''),
                COALESCE(l.relation::regclass::text, ''), COALESCE(l.transactionid::text, '')
           FROM pg_locks l
          WHERE NOT l.granted
            AND l.pid IN (SELECT pid FROM pg_stat_activity WHERE datname = current_database())
          ORDER BY l.pid",
    )
    .fetch_all(&mut *probe)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(pid, kind, mode, relation, transaction)| {
            format!("  backend {pid} waits for {mode} on {kind} {relation} {transaction}")
        })
        .collect())
}

/// The backends of this database that wait for a lock the transaction of `blocker` holds, with their statements.
async fn waiting_on(
    probe: &mut sqlx::PgConnection,
    blocker: i32,
) -> Result<Vec<(i32, String)>, sqlx::Error> {
    sqlx::query_as::<_, (i32, String)>(
        "SELECT pid, COALESCE(query, '') FROM pg_stat_activity
          WHERE datname = current_database() AND wait_event_type = 'Lock' AND $1 = ANY(pg_blocking_pids(pid))
          ORDER BY pid",
    )
    .bind(blocker)
    .fetch_all(&mut *probe)
    .await
}

/// The other backends of this database that are doing something, with what each waits for and runs.
async fn active_beside(
    probe: &mut sqlx::PgConnection,
    blocker: i32,
) -> Result<Vec<String>, sqlx::Error> {
    let rows = sqlx::query_as::<_, (i32, String, String, String)>(
        "SELECT pid, COALESCE(state, ''), COALESCE(wait_event_type || ':' || wait_event, ''), left(COALESCE(query, ''), 200)
           FROM pg_stat_activity
          WHERE datname = current_database() AND pid <> pg_backend_pid() AND pid <> $1 AND state IS DISTINCT FROM 'idle'
          ORDER BY pid",
    )
    .bind(blocker)
    .fetch_all(&mut *probe)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(pid, state, wait, query)| format!("backend {pid} {state} {wait}: {query}"))
        .collect())
}
