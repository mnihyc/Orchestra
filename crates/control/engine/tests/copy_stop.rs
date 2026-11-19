//! Cancelled or expired file commands retain the job until their machine confirms that cleanup stopped.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use async_trait::async_trait;
use common::World;
use orch_db::queries::jobs::{self, NewJob};
use orch_engine::commands::{Commands, CommandsError, Info, Outcome, Outgoing, Stage};
use orch_link_contract::commands::CommandKind;
use orch_types::{Id, Time, new_id};
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

struct Cleanup {
    command: Mutex<Option<Id>>,
    entered: Notify,
    ended: AtomicBool,
    changed: Notify,
    probed: Notify,
    stopped: AtomicBool,
    expired: bool,
    at: Time,
}

struct HeldCommand {
    inner: Arc<dyn Commands>,
    cleanup: Arc<Cleanup>,
}

impl HeldCommand {
    fn owns(&self, id: Id) -> bool {
        *self.cleanup.command.lock().unwrap() == Some(id)
    }
    fn outcome(&self) -> Outcome {
        if self.cleanup.expired {
            Outcome::Expired
        } else {
            Outcome::Cancelled
        }
    }
}

#[async_trait]
impl Commands for HeldCommand {
    async fn issue(&self, command: Outgoing) -> Result<(), CommandsError> {
        if command.kind != CommandKind::NodeMountMeasure {
            return self.inner.issue(command).await;
        }
        *self.cleanup.command.lock().unwrap() = Some(command.id);
        self.cleanup.entered.notify_one();
        Ok(())
    }
    async fn status(&self, id: Id) -> Result<Option<Info>, CommandsError> {
        if !self.owns(id) {
            return self.inner.status(id).await;
        }
        let ended = self.cleanup.ended.load(Ordering::SeqCst);
        Ok(Some(Info {
            stage: if ended { Stage::Over } else { Stage::Accepted },
            issued_at: self.cleanup.at.get(),
            outcome: ended.then(|| self.outcome()),
        }))
    }
    async fn wait(&self, id: Id) -> Result<Outcome, CommandsError> {
        if !self.owns(id) {
            return self.inner.wait(id).await;
        }
        loop {
            let changed = self.cleanup.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.cleanup.ended.load(Ordering::SeqCst) {
                return Ok(self.outcome());
            }
            changed.await;
        }
    }
    async fn cancel(&self, id: Id) -> Result<bool, CommandsError> {
        if !self.owns(id) {
            return self.inner.cancel(id).await;
        }
        self.cleanup.ended.store(true, Ordering::SeqCst);
        self.cleanup.changed.notify_waiters();
        Ok(true)
    }
    async fn stopped(&self, id: Id) -> Result<bool, CommandsError> {
        if !self.owns(id) {
            return self.inner.stopped(id).await;
        }
        self.cleanup.probed.notify_one();
        Ok(self.cleanup.stopped.load(Ordering::SeqCst))
    }
}

async fn retains_until_stopped(expired: bool) {
    let world = World::new().await;
    let cleanup = Arc::new(Cleanup {
        command: Mutex::new(None),
        entered: Notify::new(),
        ended: AtomicBool::new(false),
        changed: Notify::new(),
        probed: Notify::new(),
        stopped: AtomicBool::new(false),
        expired,
        at: Time::new(world.clock.now()),
    });
    let state = Arc::clone(&cleanup);
    world
        .restart_api_using(move |inner| {
            Arc::new(HeldCommand {
                inner,
                cleanup: state,
            })
        })
        .await;
    let id = new_id();
    let mount = new_id();
    let engine = world.engine();
    jobs::insert(&mut world.pool.acquire().await.unwrap(), &NewJob {
        id, kind: "measurement", node_id: None, environment_id: None, provider_id: None,
        created_by: Some(world.ada),
        plan: &json!({"machine":world.machine_id,"command_kind":"node.mount.measure",
            "body":{"driver":"test","mount_id":mount,"data":{"kind":"volume","name":format!("orch-mount-{mount}"),"storage":null}},
            "subject":{},"timeout_secs":86400,"keep_until":world.clock.now()+chrono::Duration::hours(1)}),
        progress: &json!({"phase":"measure","phases":[{"name":"measure","state":"pending","weight":100}],
            "percent":0,"files_done":0,"files_total":null,"bytes_done":0,"bytes_total":null,"rate_bps":null,"eta_seconds":null,"message":null}),
    }, world.clock.now()).await.unwrap();
    engine.kick();
    tokio::time::timeout(common::PATIENCE, cleanup.entered.notified())
        .await
        .unwrap();
    if expired {
        cleanup.ended.store(true, Ordering::SeqCst);
        cleanup.changed.notify_waiters();
    } else {
        assert!(engine.cancel_job(id, world.ada).await.unwrap());
    }
    tokio::time::timeout(common::PATIENCE, cleanup.probed.notified())
        .await
        .unwrap();
    let row = jobs::job(&mut world.pool.acquire().await.unwrap(), id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.state, "running",
        "a terminal command outcome is not proof of stopped I/O"
    );
    assert_eq!(
        row.plan["active_command"]["id"],
        json!(cleanup.command.lock().unwrap().unwrap())
    );
    assert!(row.finished_at.is_none());
    cleanup.stopped.store(true, Ordering::SeqCst);
    common::eventually(
        "the job to end only after cleanup acknowledgement",
        || async {
            !jobs::job(&mut world.pool.acquire().await.unwrap(), id)
                .await
                .unwrap()
                .unwrap()
                .is_open()
        },
    )
    .await;
    let row = jobs::job(&mut world.pool.acquire().await.unwrap(), id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.state, if expired { "failed" } else { "cancelled" });
    assert!(row.plan["active_command"].is_null());
    drop(engine);
    world.shutdown().await;
}

#[tokio::test]
async fn a_cancelled_count_does_not_finish_until_the_machine_joins_its_cleanup() {
    retains_until_stopped(false).await;
}

#[tokio::test]
async fn an_expired_file_command_does_not_release_its_job_before_cleanup_acknowledgement() {
    retains_until_stopped(true).await;
}
