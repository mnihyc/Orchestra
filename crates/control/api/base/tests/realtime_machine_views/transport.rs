//! This target's API journal <-> already connected scripted machine transport.
//! The shared Rig's normal native/accounting setup is deliberately unchanged.

use super::realtime_support::{PATIENCE, Rig};
use orch_api_core::dispatch::cmd_of;
use orch_db::queries::link as rows;
use orch_link::commands::Progress;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::ops::Deref;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

// Reuse the existing feeds_machine journal bridge's20ms fixture cadence. Product views never poll.
const JOURNAL_STEP: Duration = Duration::from_millis(20);

pub struct MachineViews {
    // Normal close joins the journal reader before releasing this rig; Drop is only a failure fallback.
    bridge: Bridge,
    rig: Rig,
}

struct Bridge {
    stop: CancellationToken,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl Bridge {
    async fn join(&mut self) -> Result<(), tokio::task::JoinError> {
        self.stop.cancel();
        // Borrow the handle until joined. Cancelling this future must leave Drop able to abort it.
        let outcome = self
            .task
            .as_mut()
            .expect("the transport is joined exactly once")
            .await;
        self.task = None;
        outcome
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        self.stop.cancel();
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

async fn until_stopped<T>(stop: &CancellationToken, work: impl Future<Output = T>) -> Option<T> {
    tokio::select! {biased;()=stop.cancelled()=>None,value=work=>Some(value)}
}

async fn return_connection(
    mut connection: sqlx::pool::PoolConnection<sqlx::Postgres>,
    stop: &CancellationToken,
) -> bool {
    // SQLx0.9 otherwise spawns its return future from Drop. Await that exact eager return here;
    // cancellation drops its owned floating connection instead of leaving a detached return task.
    until_stopped(stop, connection.return_to_pool())
        .await
        .is_some()
}

fn require_joined(outcome: Result<(), tokio::task::JoinError>) {
    if let Err(error) = outcome {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("the native command bridge was unexpectedly cancelled: {error}");
    }
}

impl Deref for MachineViews {
    type Target = Rig;
    fn deref(&self) -> &Rig {
        &self.rig
    }
}

impl MachineViews {
    pub async fn new() -> Self {
        let rig = Rig::with_unstarted_engine().await;
        // The activation test deliberately elects the coordinator before its first socket.
        rig.server
            .app
            .realtime
            .attach_link(rig.server.app.link.clone());
        // The worker needs no App/Rig owner, only these metadata handles.
        let db = rig.pool.clone();
        let clock = rig.clock.clone();
        let controller = rig.machine_controller();
        let machine = rig.machine_id;
        let stop = CancellationToken::new();
        let stopped = stop.clone();
        let task = tokio::spawn(async move {
            let mut taken = HashSet::new();
            let mut handles = HashMap::new();
            loop {
                if until_stopped(&stopped, tokio::time::sleep(JOURNAL_STEP))
                    .await
                    .is_none()
                {
                    return;
                }
                controller.commands().tick(tokio::time::Instant::now());
                let Some(connection) = until_stopped(&stopped, db.acquire()).await else {
                    return;
                };
                let mut conn = connection.expect("read the fixture command journal");
                let open = until_stopped(&stopped, rows::open_commands(&mut conn, machine)).await;
                let Some(open) = open else {
                    // A cancelled query must not be left to an unjoined pool-return/ping future.
                    conn.close()
                        .await
                        .expect("close the transport's interrupted metadata read");
                    return;
                };
                if !return_connection(conn, &stopped).await {
                    return;
                }
                let open = open.expect("read only this machine's open commands");
                for row in open {
                    if taken.insert(row.id) {
                        handles.insert(row.id, controller.commands().issue(cmd_of(&row)));
                    }
                }
                let done: Vec<_> = handles
                    .iter()
                    .filter_map(|(id, handle)| {
                        let progress = handle.progress();
                        progress.is_final().then_some((*id, progress))
                    })
                    .collect();
                for (id, progress) in done {
                    handles.remove(&id);
                    let failed = match &progress {
                        Progress::Failed(error) => Some(
                            serde_json::to_value(error)
                                .expect("encode the typed synthetic refusal"),
                        ),
                        _ => None,
                    };
                    let ending = match &progress {
                        Progress::Succeeded(value) => rows::Ending::Succeeded(value),
                        Progress::Failed(_) => rows::Ending::Failed(
                            failed.as_ref().expect("the actual native refusal"),
                        ),
                        Progress::Cancelled => rows::Ending::Cancelled,
                        Progress::Expired => rows::Ending::Expired,
                        _ => unreachable!("a nonterminal command is not collected"),
                    };
                    let Some(connection) = until_stopped(&stopped, db.acquire()).await else {
                        return;
                    };
                    let mut conn = connection.expect("persist the actual machine result");
                    let written = until_stopped(
                        &stopped,
                        rows::finish_command(&mut conn, id, &ending, clock.now()),
                    )
                    .await;
                    let Some(written) = written else {
                        conn.close()
                            .await
                            .expect("close the transport's interrupted metadata write");
                        return;
                    };
                    if !return_connection(conn, &stopped).await {
                        return;
                    }
                    written.expect("mirror one completed native command");
                }
            }
        });
        Self {
            bridge: Bridge {
                stop,
                task: Some(task),
            },
            rig,
        }
    }

    /// Explicitly joins our transport and stops the existing server inside one original fixture budget.
    pub async fn close(mut self) {
        let deadline = tokio::time::Instant::now() + PATIENCE;
        let joined = tokio::time::timeout_at(deadline, self.bridge.join()).await;
        let server = tokio::time::timeout_at(deadline, self.rig.server.stop()).await;
        // Preserve an actual child panic even if another teardown step also exhausts the budget.
        require_joined(
            joined.expect("join the native command bridge within the original fixture budget"),
        );
        server.expect("stop the machine-view server within the same original fixture budget");
    }
}

impl Drop for MachineViews {
    fn drop(&mut self) {
        self.bridge.stop.cancel();
        assert!(
            self.bridge.task.is_none() || std::thread::panicking(),
            "MachineViews::close must be awaited; abort-on-drop is only an unwinding fallback"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn cancellation_interrupts_a_real_pool_wait_and_join_releases_the_worker_owner() {
        let database = orch_testkit::database::TestDb::fresh().await;
        let mut held = Vec::new();
        for _ in 0..database.pool.options().get_max_connections() {
            held.push(
                database
                    .pool
                    .acquire()
                    .await
                    .expect("hold the existing pool's connections without a transaction"),
            );
        }
        let db = orch_db::Db::from_pool(database.pool.clone());
        let owner = Arc::new(());
        let worker_owner = Arc::clone(&owner);
        let stop = CancellationToken::new();
        let stopped = stop.clone();
        let (entered, waiting) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let _owner = worker_owner;
            entered
                .send(())
                .expect("announce the actual blocked acquisition");
            assert!(until_stopped(&stopped, db.acquire()).await.is_none());
        });
        let mut bridge = Bridge {
            stop,
            task: Some(task),
        };
        waiting.await.expect("the worker owns its metadata wait");
        require_joined(
            tokio::time::timeout(PATIENCE, bridge.join())
                .await
                .expect("join a cancelled acquisition within the original budget"),
        );
        assert!(bridge.task.is_none());
        assert_eq!(Arc::strong_count(&owner), 1);
        for mut connection in held {
            connection.return_to_pool().await;
        }
    }

    #[tokio::test]
    async fn joining_propagates_the_original_transport_panic_instead_of_a_successful_summary() {
        let owner = Arc::new(());
        let worker_owner = Arc::clone(&owner);
        let task = tokio::spawn(async move {
            let _owner = worker_owner;
            std::panic::resume_unwind(Box::new("synthetic transport panic"));
        });
        let mut bridge = Bridge {
            stop: CancellationToken::new(),
            task: Some(task),
        };
        let joined = tokio::time::timeout(PATIENCE, bridge.join())
            .await
            .expect("observe the child outcome within the original budget");
        let panic =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| require_joined(joined)))
                .expect_err("a child panic cannot become successful fixture teardown");
        assert_eq!(
            panic.downcast_ref::<&str>(),
            Some(&"synthetic transport panic")
        );
        assert!(bridge.task.is_none());
        assert_eq!(Arc::strong_count(&owner), 1);
    }
}
