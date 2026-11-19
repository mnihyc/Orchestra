//! Actual same-operation input producer -> queued command -> dispatcher -> native journal.
//! The native peer is the existing component peer; no vendor process or HTTP endpoint claim.

use super::selection::{Split, common};
use super::*;
use orch_access_contract::key_scope::KeyScope;
use orch_authz::{Caller, TokenGrant};
use orch_db::queries::{identity, threads};
use orch_engine::commands::{Commands, CommandsError, Outgoing};
use orch_engine::operation_scope::{Prepared, blob_sources, targets};

const INPUT_KINDS: &[CommandKind] = &[
    CommandKind::TurnStart,
    CommandKind::TurnStartFenced,
    CommandKind::TurnSteer,
];

struct Issuing {
    reached: oneshot::Sender<Id>,
    release: oneshot::Receiver<()>,
}
static ISSUING: OnceLock<Mutex<HashMap<Id, Issuing>>> = OnceLock::new();

/// The existing post-store/pre-adoption cut discovers the actual command, rather than
/// manufacturing its id/body or teaching this test the product's future authority format.
pub(super) async fn adoption_read(link: &Link, id: Id) {
    if ISSUING
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .is_empty()
    {
        return;
    }
    let row = rows::command(
        &mut *link.inner.db.acquire().await.expect("owned input cut DB"),
        id,
    )
    .await
    .expect("owned canonical input command");
    let Some(row) = row.filter(|row| INPUT_KINDS.iter().any(|kind| kind.as_str() == row.kind))
    else {
        return;
    };
    let Some(thread) = row.body.get("thread_id").cloned() else {
        return;
    };
    let Ok(thread) = serde_json::from_value::<Id>(thread) else {
        return;
    };
    let gate = ISSUING
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(&thread);
    if let Some(gate) = gate {
        let _ = gate.reached.send(id);
        let _ = gate.release.await;
    }
}

#[derive(Clone, Copy, Debug)]
enum Kind {
    Start,
    Fenced,
    Steer,
}
impl Kind {
    fn native(self) -> CommandKind {
        match self {
            Self::Start => CommandKind::TurnStart,
            Self::Fenced => CommandKind::TurnStartFenced,
            Self::Steer => CommandKind::TurnSteer,
        }
    }
}

struct Rig {
    world: common::World,
    clock: ManualClock,
    link: Link,
    dispatcher: Dispatcher,
    server: CommandServer,
    native: Arc<Native>,
    peer: Option<Peer>,
    thread: Id,
    kind: Kind,
    _directory: Arc<tempfile::TempDir>,
}
impl Drop for Rig {
    fn drop(&mut self) {
        if let Some(collisions) = COLLISIONS.get() {
            collisions
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&self.thread);
        }
        self.link.shutdown();
        if let Some(peer) = &self.peer {
            peer.stop.cancel();
        }
    }
}
impl Rig {
    async fn new(kind: Kind) -> Proof<Self> {
        let clock = clock::manual();
        let world = if matches!(kind, Kind::Fenced) {
            common::World::observed_with_clock(Arc::new(clock.clone())).await
        } else {
            common::World::with_clock(Arc::new(clock.clone())).await
        };
        if matches!(kind, Kind::Fenced) {
            let mut descriptor = common::descriptor();
            descriptor["capabilities"]["native_input_prefix"] = json!("turn_start");
            // The existing facade updates BOTH the profile and actual native
            // AgentStart declaration before this fixture creates its first Thread.
            world.declares(descriptor).await;
        }
        let thread = world.thread("Actual input first-wire origin").await;
        if matches!(kind, Kind::Steer) {
            // The current turn was really started before routing only its new input
            // delivery through the existing production dispatcher/native journal.
            world
                .running_turn(thread, "Existing ordinary running turn")
                .await;
        }
        let directory = Arc::new(tempfile::tempdir()?);
        let native = Arc::new(Native {
            _directory: directory.clone(),
            environment: world.environment,
            payload: directory.path().join("unused-input"),
            destination: directory.path().join("unused-output"),
            clock: Arc::new(clock.clone()),
            expected_path: "unused".into(),
            blobs: None,
            hold: false,
            entered: AtomicBool::new(false),
            release: Semaphore::new(0),
            reads: AtomicUsize::new(0),
            writes: AtomicUsize::new(0),
            probes: AtomicUsize::new(0),
            starts: AtomicUsize::new(0),
            wire: Mutex::new(Vec::new()),
        });
        let server = CommandServer::new(
            Journal::open(
                &directory.path().join("journal"),
                Duration::from_secs(24 * 3600),
                clock.time(),
            )?,
            Arc::new(clock.clone()),
            ServerOptions {
                roles: vec![Role::Environment, Role::Agent, Role::Provider],
                connected: true,
            },
        );
        server.serve(native.clone());
        let link = Link::detached(
            world.pool.clone(),
            Arc::new(clock.clone()),
            Arc::new(ServerConfig::default()),
        )?;
        let dispatcher = link.dispatcher();
        let dispatch = dispatcher.clone();
        let machine = world.machine_id;
        world
            .restart_api_using(move |original| {
                Arc::new(Split::selected(original, machine, dispatch, INPUT_KINDS))
            })
            .await;
        let (peer, ready) = peer(link.clone(), machine, server.clone(), native.clone(), false);
        within(ready).await??;
        Ok(Self {
            world,
            clock,
            link,
            dispatcher,
            server,
            native,
            peer: Some(peer),
            thread,
            kind,
            _directory: directory,
        })
    }

    async fn caller(&self) -> Proof<(Id, Caller)> {
        let mut conn = self.world.pool.acquire().await?;
        let key = new_id();
        let scope: KeyScope = serde_json::from_value(json!({"rules":[{
            "target":{"kind":"thread","id":self.thread,"reach":"self"},
            "allow":[{"operation":"ThreadInputCreate","actions":["write"]}]
        }]}))?;
        assert!(scope.validate().is_empty());
        // A one-second natural key deadline, with a stopped domain clock, stays
        // valid through all positive setup; the negative moves two seconds only.
        let expiry = self.clock.now() + chrono::Duration::seconds(1);
        identity::create_api_token_scoped(
            &mut conn,
            &identity::NewApiToken {
                id: key,
                user_id: self.world.ada,
                name: "Actual owned input origin",
                token_hash: Hash::of(key.as_bytes()).as_bytes(),
                scopes: &[],
                node_id: None,
                at: self.clock.now(),
                expires_at: Some(expiry),
            },
            Some(&json!(scope)),
        )
        .await?;
        let caller = Caller::token(
            self.world.ada,
            TokenGrant::from_stored_scope(&[], None, Some(scope), Some(Time::new(expiry)))?
                .for_token(key),
        );
        // The normal shared operation binder admits this real thread and exact
        // literal-input footprint. No synthetic admitted Caller or other endpoint.
        let mut plan = Prepared::for_selection("ThreadInputCreate")?;
        let target = targets::node(&mut conn, self.thread).await?;
        orch_engine::inputs::operation::bind_thread(&mut plan, &target)?;
        orch_engine::inputs::operation::bind_native(&mut plan, &target, None)?;
        orch_engine::inputs::operation::bind_bypass(&mut plan, &mut conn, &[]).await?;
        blob_sources::bind(&mut plan, &[])?;
        let admitted = plan.admit(&caller, false)?;
        Ok((key, admitted))
    }

    async fn queued(&self, caller: &Caller) -> Proof<(rows::CommandRow, oneshot::Sender<()>)> {
        let (reached, waiting) = oneshot::channel();
        let (release, resume) = oneshot::channel();
        assert!(
            ISSUING
                .get_or_init(Mutex::default)
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(
                    self.thread,
                    Issuing {
                        reached,
                        release: resume
                    }
                )
                .is_none()
        );
        let request = if matches!(self.kind, Kind::Steer) {
            common::steering("Original same-operation scoped input")
        } else {
            common::queued("Original same-operation scoped input")
        };
        let submitted = self
            .world
            .engine()
            .submit_input(caller, self.thread, request)
            .await?;
        let id = within(waiting).await??;
        let row = rows::command(&mut *self.world.pool.acquire().await?, id)
            .await?
            .ok_or("the actual input command disappeared")?;
        assert_eq!(row.kind, self.kind.native().as_str());
        assert_eq!(row.state, "queued");
        assert_eq!(row.machine_id, self.world.machine_id);
        assert_eq!(
            row.serial_key(),
            SerialKey::Thread(self.thread).wire().as_deref()
        );
        assert_eq!(row.body["thread_id"], json!(self.thread));
        let input = threads::input(&mut *self.world.pool.acquire().await?, submitted.input.id)
            .await?
            .ok_or("the actual accepted input disappeared")?;
        assert_eq!(input.thread_id, self.thread);
        assert_eq!(input.state, "delivering");
        assert!(input.submission_authority.is_some());
        assert_eq!(input.author_id, Some(self.world.ada));
        let turn: Id = serde_json::from_value(if matches!(self.kind, Kind::Fenced) {
            row.body["turn"]["turn_id"].clone()
        } else {
            row.body["turn_id"].clone()
        })?;
        let actual_turn = threads::turn(&mut *self.world.pool.acquire().await?, turn)
            .await?
            .ok_or("the canonical command turn is absent")?;
        assert_eq!(actual_turn.thread_id, self.thread);
        if matches!(self.kind, Kind::Steer) {
            assert_eq!(id, orch_engine::ids::turn_steer(input.id));
            assert_eq!(input.state_detail.as_deref(), Some("steering"));
        } else {
            assert_eq!(actual_turn.input_id, input.id);
            assert_eq!(id, orch_engine::ids::turn_start(turn, 0));
        }
        assert!(self.server.entry(id).is_none());
        Ok((row, release))
    }

    async fn revoke(&self, key: Id) -> Proof {
        assert_eq!(
            sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
                .bind(key)
                .bind(self.clock.now())
                .execute(self.world.pool.pool())
                .await?
                .rows_affected(),
            1
        );
        Ok(())
    }

    async fn fence(&self) -> Proof {
        let id = self
            .dispatcher
            .issue(
                self.world.machine_id,
                &SerialKey::Environment(self.world.environment),
                &MachineProbe {},
            )
            .await?
            .id;
        assert!(matches!(
            within(self.dispatcher.wait(id)).await??,
            CommandOutcome::Succeeded(_)
        ));
        assert!(
            matches!(self.server.entry(id).ok_or("real probe journal absent")?.step,
            Step::Done {ref result} if result.outcome.is_ok())
        );
        Ok(())
    }

    async fn close(mut self) -> Proof {
        self.world.shutdown().await;
        self.link.shutdown();
        if let Some(peer) = self.peer.take() {
            peer.stop.cancel();
            peer.join().await?;
        }
        Ok(())
    }
}

async fn positive(kind: Kind) -> Proof {
    let rig = Rig::new(kind).await?;
    let (_, caller) = rig.caller().await?;
    let (original, release) = rig.queued(&caller).await?;
    release
        .send(())
        .map_err(|_| "the actual input adoption cut ended")?;
    let outcome = within(rig.dispatcher.wait(original.id)).await??;
    assert!(
        matches!(outcome, CommandOutcome::Succeeded(_)),
        "same-path positive failed: {outcome:?}"
    );
    rig.fence().await?;
    let journal = rig
        .server
        .entry(original.id)
        .ok_or("actual positive input journal absent")?;
    assert_eq!(journal.cmd.body, original.body);
    assert!(matches!(journal.step, Step::Done {ref result} if result.outcome.is_ok()));
    rig.close().await
}

#[derive(Clone, Copy)]
enum Change {
    Revoke,
    Expire,
}
async fn before_first_wire(kind: Kind, change: Change) -> Proof {
    positive(kind).await?;
    let rig = Rig::new(kind).await?;
    let (key, caller) = rig.caller().await?;
    let (original, release) = rig.queued(&caller).await?;
    match change {
        Change::Revoke => {
            rig.revoke(key).await?;
            release.send(()).map_err(|_| "actual adoption cut ended")?;
        }
        Change::Expire => {
            let (checked, send) = gate(original.id, Phase::Wire);
            release.send(()).map_err(|_| "actual adoption cut ended")?;
            within(checked).await??;
            rig.clock.advance(chrono::Duration::seconds(2));
            assert!(
                rig.clock.now() < original.deadline_at,
                "the command's own native deadline must remain valid"
            );
            send.send(()).map_err(|_| "actual final wire cut ended")?;
        }
    }
    let outcome = within(rig.dispatcher.wait(original.id)).await??;
    rig.fence().await?;
    assert!(
        rig.server.entry(original.id).is_none(),
        "a withdrawn original input acquired a fresh actual native journal entry: {outcome:?}"
    );
    assert!(
        !rig.native
            .wire
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .any(|command| command.id == original.id)
    );
    assert!(
        matches!(outcome, CommandOutcome::Failed(ref error) if error.code == ErrorCode::Conflict.as_str()),
        "same current-origin withdrawal must truthfully refuse the new send: {outcome:?}"
    );
    rig.close().await
}

async fn accepted_result(kind: Kind) -> Proof {
    positive(kind).await?;
    let rig = Rig::new(kind).await?;
    let (key, caller) = rig.caller().await?;
    let (original, release) = rig.queued(&caller).await?;
    let (entered, finish) = gate(original.id, Phase::NativeInput);
    release.send(()).map_err(|_| "actual adoption cut ended")?;
    within(entered).await??;
    let accepted = rig
        .server
        .entry(original.id)
        .ok_or("native Accepted journal absent")?;
    assert!(matches!(accepted.step, Step::Accepted | Step::Started));
    assert_eq!(accepted.cmd.body, original.body);
    rig.revoke(key).await?;
    rig.clock.advance(chrono::Duration::seconds(2));
    assert!(rig.clock.now() < original.deadline_at);
    finish
        .send(())
        .map_err(|_| "actual native handler cut ended")?;
    assert!(
        matches!(
            within(rig.dispatcher.wait(original.id)).await??,
            CommandOutcome::Succeeded(_)
        ),
        "an exact alreadyAccepted input result must reconcile after key withdrawal"
    );
    rig.fence().await?;
    assert!(
        matches!(rig.server.entry(original.id).ok_or("accepted result journal absent")?.step,
        Step::Done {ref result} if result.outcome.is_ok())
    );
    rig.close().await
}

#[tokio::test]
async fn queued_start_rechecks_its_actual_input_origin_before_first_wire() -> Proof {
    before_first_wire(Kind::Start, Change::Revoke).await
}
#[tokio::test]
async fn queued_fenced_start_rechecks_its_actual_input_origin_before_first_wire() -> Proof {
    before_first_wire(Kind::Fenced, Change::Revoke).await
}
#[tokio::test]
async fn queued_steer_rechecks_its_actual_input_origin_before_first_wire() -> Proof {
    before_first_wire(Kind::Steer, Change::Revoke).await
}
#[tokio::test]
async fn start_cannot_publish_after_key_expiry_during_the_final_wire_cut() -> Proof {
    before_first_wire(Kind::Start, Change::Expire).await
}
#[tokio::test]
async fn fenced_start_cannot_publish_after_key_expiry_during_the_final_wire_cut() -> Proof {
    before_first_wire(Kind::Fenced, Change::Expire).await
}
#[tokio::test]
async fn steer_cannot_publish_after_key_expiry_during_the_final_wire_cut() -> Proof {
    before_first_wire(Kind::Steer, Change::Expire).await
}
#[tokio::test]
async fn an_actual_accepted_start_result_reconciles_after_its_origin_expires() -> Proof {
    accepted_result(Kind::Start).await
}
#[tokio::test]
async fn an_actual_accepted_fenced_start_result_reconciles_after_its_origin_expires() -> Proof {
    accepted_result(Kind::Fenced).await
}
#[tokio::test]
async fn an_actual_accepted_steer_result_reconciles_after_its_origin_expires() -> Proof {
    accepted_result(Kind::Steer).await
}

struct Collision {
    link: Link,
    accepted: bool,
    report: oneshot::Sender<CollisionReport>,
}
struct CollisionReport {
    old: StoredCommand,
    mismatch: bool,
    finish: Option<oneshot::Sender<()>>,
}
static COLLISIONS: OnceLock<Mutex<HashMap<Id, Collision>>> = OnceLock::new();

/// Test-only cut around the REAL Commands port. The old None row uses the exact
/// Outgoing emitted by the actual input writer, before its new typed issue starts.
/// Both issues still run the real dispatcher; no collision algorithm is copied.
pub(super) async fn collide(
    dispatcher: &Dispatcher,
    native: &crate::thread_engine::DispatchCommands,
    command: &Outgoing,
) -> Option<Result<(), CommandsError>> {
    if command.kind != CommandKind::TurnSteer {
        return None;
    }
    let thread = command.body.get("thread_id")?.clone();
    let thread: Id = serde_json::from_value(thread).ok()?;
    let collision = COLLISIONS
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(&thread)?;
    let prepared = async {
        let deadline = command.deadline.unwrap_or_else(|| {
            Time::new(
                collision.link.inner.clock.now()
                    + chrono::Duration::seconds(i64::from(command.kind.deadline_secs())),
            )
        });
        let old = StoredCommand {
            id: Some(command.id),
            machine: command.machine,
            kind: command.kind.as_str().into(),
            key: command.key.wire(),
            deadline: Some(deadline),
            body: command.body.clone(),
            authority: None,
        };
        assert!(
            rows::insert_command(
                &mut *collision.link.inner.db.acquire().await?,
                &rows::NewCommand {
                    id: command.id,
                    machine_id: command.machine,
                    kind: command.kind.as_str(),
                    body: &command.body,
                    serial_key: old.key.as_deref(),
                    deadline_at: deadline.get(),
                },
                collision.link.inner.clock.now(),
            )
            .await?
        );
        let finish = if collision.accepted {
            let (entered, finish) = gate(command.id, Phase::NativeInput);
            dispatcher.issue_stored(old.clone()).await?;
            within(entered).await??;
            Some(finish)
        } else {
            None
        };
        // Only this call is the NEW attempt. A correct source refuses it while
        // leaving every fact/lifecycle of the older canonical None command intact.
        let attempted = native.issue(command.clone()).await;
        let mismatch = matches!(&attempted, Err(CommandsError::AuthorityMismatch));
        let _ = collision.report.send(CollisionReport {
            old,
            mismatch,
            finish,
        });
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(attempted)
    }
    .await;
    Some(match prepared {
        Ok(attempted) => attempted,
        Err(error) => Err(CommandsError::Other(format!(
            "owned None collision fixture: {error}"
        ))),
    })
}

async fn canonical_none(accepted: bool) -> Proof {
    positive(Kind::Steer).await?;
    let rig = Rig::new(Kind::Steer).await?;
    let (key, caller) = rig.caller().await?;
    let (send, receive) = oneshot::channel();
    assert!(
        COLLISIONS
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                rig.thread,
                Collision {
                    link: rig.link.clone(),
                    accepted,
                    report: send
                }
            )
            .is_none()
    );
    let input = rig
        .world
        .engine()
        .submit_input(
            &caller,
            rig.thread,
            common::steering("Actual original canonical None work"),
        )
        .await?;
    let report = within(receive).await??;
    let id = report.old.id.ok_or("the older canonical id is absent")?;
    assert_eq!(id, orch_engine::ids::turn_steer(input.input.id));
    assert!(
        report.mismatch,
        "the new scoped attempt must truthfully refuse canonical None without adoption"
    );
    let before = rows::command(&mut *rig.world.pool.acquire().await?, id)
        .await?
        .ok_or("older canonical work disappeared")?;
    assert_eq!(before.body, report.old.body);
    assert_eq!(before.machine_id, report.old.machine);
    assert_eq!(before.kind, report.old.kind);
    assert_eq!(before.serial_key(), report.old.key.as_deref());
    assert_eq!(
        Time::new(before.deadline_at),
        report.old.deadline.ok_or("old budget missing")?
    );
    assert!(
        rows::command_authority(&mut *rig.world.pool.acquire().await?, id)
            .await?
            .is_none()
    );
    if accepted {
        assert!(matches!(
            rig.server
                .entry(id)
                .ok_or("old actual native Accepted absent")?
                .step,
            Step::Accepted | Step::Started
        ));
    } else {
        assert_eq!(before.state, "queued");
        assert_eq!(before.attempts, 0);
        assert!(rig.server.entry(id).is_none());
    }
    rig.revoke(key).await?;
    if let Some(finish) = report.finish {
        finish
            .send(())
            .map_err(|_| "old accepted handler cut ended")?;
    } else {
        // This is the old ordinary issuer, with the exact old None tuple, rather
        // than a new current-input pointer upgrading somebody else's work.
        rig.dispatcher.issue_stored(report.old.clone()).await?;
    }
    assert!(matches!(
        within(rig.dispatcher.wait(id)).await??,
        CommandOutcome::Succeeded(_)
    ));
    rig.fence().await?;
    let after = rows::command(&mut *rig.world.pool.acquire().await?, id)
        .await?
        .ok_or("old canonical result disappeared")?;
    assert_eq!(after.body, before.body);
    assert_eq!(after.machine_id, before.machine_id);
    assert_eq!(after.serial_key(), before.serial_key());
    assert!(
        rows::command_authority(&mut *rig.world.pool.acquire().await?, id)
            .await?
            .is_none()
    );
    assert!(
        matches!(rig.server.entry(id).ok_or("old owned result journal absent")?.step,
        Step::Done {ref result} if result.outcome.is_ok())
    );
    rig.close().await
}

#[tokio::test]
async fn a_new_scoped_steer_cannot_adopt_or_cancel_an_old_queued_none_command() -> Proof {
    canonical_none(false).await
}
#[tokio::test]
async fn a_new_scoped_steer_cannot_relabel_an_actual_accepted_none_result() -> Proof {
    canonical_none(true).await
}
