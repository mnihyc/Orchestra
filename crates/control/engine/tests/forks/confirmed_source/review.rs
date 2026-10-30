//! Composition controls for a confirmed fork source. Reconstructed receipt/adoption checkpoints
//! below are labeled; the owned engine/link/journal is real, the peer is synthetic.

use super::*;
use orch_access_contract::permissions::Permission;
use orch_authz::TokenGrant;
use orch_db::queries::{identity, link};
use orch_link_contract::events::{Event, SessionOpened};

async fn remove_side_cache_observation(rig: &Rig, side: Id) -> Id {
    let segment = rig.world.segment_of(side).await;
    let mut conn = rig.world.pool.acquire().await.unwrap();
    let history = orch_db::queries::agent_history::get(&mut conn, segment)
        .await
        .unwrap()
        .unwrap();
    orch_db::queries::agent_history::measured(
        &mut conn,
        segment,
        &history.native_session_ref,
        Some(4096),
        true,
        None,
        rig.world.clock.now(),
    )
    .await
    .unwrap();
    segment
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_side_recovers_its_own_conversation_without_substituting_its_parent_or_creation_receipt()
{
    let rig = Rig::new(
        Can {
            fork_native: true,
            ..Default::default()
        },
        "fake",
        Can::default(),
    )
    .await;
    let parent = rig.thread_with_history("Recovered side source").await;
    let side = rig
        .world
        .engine()
        .start_side_thread(
            &rig.caller(),
            parent,
            vec![orch_api_contract::threads::ContentPart::Text {
                text: "Remember this side's copper compass".into(),
            }],
            "recovered-side".into(),
            None,
        )
        .await
        .unwrap()
        .thread;
    rig.world
        .run_turn(side, 0, "The copper compass belongs to this side")
        .await;
    rig.world.wait_state(side, "idle").await;
    rig.world.wait_followed_up(side).await;
    let original_cut = cuts::cut(&mut rig.world.pool.acquire().await.unwrap(), side)
        .await
        .unwrap()
        .unwrap();
    let original = original_cut.artifacts["side_native_result"].clone();
    let other_segment = rig.world.segment_of(parent).await;
    let refused = sqlx::query(
        "INSERT INTO thread_history_cuts
           (id,thread_id,continuation_id,job_id,owner_thread_id,source_segment_id,source_native_ref,
            execution_revision,first_turn,last_turn,captured_seq,captured_at,head,artifacts)
         SELECT $2,thread_id,continuation_id,job_id,owner_thread_id,source_segment_id,source_native_ref,
            execution_revision,first_turn,last_turn,captured_seq,captured_at,head,artifacts
         FROM thread_history_cuts WHERE id=$1",
    )
    .bind(side)
    .bind(other_segment)
    .execute(rig.world.pool.pool())
    .await
    .expect_err("another thread's segment cannot own this side's recovery cut");
    assert_eq!(
        refused
            .as_database_error()
            .and_then(|error| error.constraint()),
        Some("thread_history_cut_owner_segment")
    );
    let old_segment = remove_side_cache_observation(&rig, side).await;
    rig.world.restart_api().await;
    rig.world.engine().attach_blobs(rig.store.clone());
    rig.say(side, "Continue with this side's remembered words")
        .await;
    let next = rig.world.turn_start(side, 1).await;
    assert!(
        next.text.contains("Remember this side's copper compass"),
        "{}",
        next.text
    );
    assert!(
        next.text
            .contains("The copper compass belongs to this side"),
        "{}",
        next.text
    );
    assert!(next.text.contains(".orch/handoff/"), "{}", next.text);
    let recovered = rig.world.segment_of(side).await;
    assert_ne!(recovered, old_segment);
    let cut = cuts::cut(&mut rig.world.pool.acquire().await.unwrap(), side)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        cut.artifacts["side_native_result"], original,
        "the original parent preparation is immutable"
    );
    assert_eq!(rig.world.count("SELECT count(*) FROM items WHERE thread_id=$1 AND payload->>'marker'='side_started'",parent).await, 1);
    rig.world
        .say(
            side,
            crate::common::turn_ended(next.turn_id, TurnOutcome::Completed),
        )
        .await;
    rig.world.wait_state(side, "idle").await;
    rig.world.wait_followed_up(side).await;
    let native: String = sqlx::query_scalar("SELECT native_session_ref FROM segments WHERE id=$1")
        .bind(recovered)
        .fetch_one(rig.world.pool.pool())
        .await
        .unwrap();
    rig.world.restart_api().await;
    rig.world.engine().attach_blobs(rig.store.clone());
    rig.say(side, "Resume the recovered side").await;
    let third = rig.world.turn_start(side, 2).await;
    assert!(
        !third.text.contains(".orch/handoff/"),
        "a received handoff is not injected again: {}",
        third.text
    );
    assert_eq!(rig.world.segment_of(side).await, recovered);
    let opens = rig.world.commands.issued_bodies("session.open");
    let fenced = rig.world.commands.issued_bodies("session.open.fenced");
    assert!(opens.iter().chain(fenced.iter()).any(|(_, body)| {
        let body = body.get("session").unwrap_or(body);
        body["segment_id"] == recovered.to_string()
            && body["mode"] == "resume"
            && body["native_session_ref"] == native
    }));
    rig.world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn side_recovery_does_not_use_a_frozen_token_to_override_its_current_narrowing() {
    let rig = Rig::new(
        Can {
            fork_native: true,
            ..Default::default()
        },
        "fake",
        Can::default(),
    )
    .await;
    let parent = rig.thread_with_history("Side recovery grant").await;
    let token = new_id();
    let scopes = Permission::ALL
        .iter()
        .map(|permission| permission.as_str().to_owned())
        .collect::<Vec<_>>();
    identity::create_api_token(
        &mut rig.world.pool.acquire().await.unwrap(),
        &identity::NewApiToken {
            id: token,
            user_id: rig.world.ada,
            name: "synthetic-side-recovery",
            token_hash: &[0x54; 32],
            scopes: &scopes,
            node_id: Some(rig.world.root),
            at: rig.world.clock.now(),
            expires_at: None,
        },
    )
    .await
    .unwrap();
    let caller = Caller::token(
        rig.world.ada,
        TokenGrant::from_stored(&scopes, Some(rig.world.root))
            .unwrap()
            .for_token(token),
    );
    let side = rig
        .world
        .engine()
        .start_side_thread(
            &caller,
            parent,
            vec![orch_api_contract::threads::ContentPart::Text {
                text: "An authorized side".into(),
            }],
            "side-recovery-token".into(),
            None,
        )
        .await
        .unwrap()
        .thread;
    rig.world
        .run_turn(side, 0, "The side has its context")
        .await;
    rig.world.wait_state(side, "idle").await;
    rig.world.wait_followed_up(side).await;
    remove_side_cache_observation(&rig, side).await;
    let narrowed: Vec<_> = scopes
        .iter()
        .filter(|scope| scope.as_str() != "provider.use")
        .cloned()
        .collect();
    let mut tx = rig.world.pool.begin().await.unwrap();
    orch_db::queries::transaction_guard::lock_authority(&mut tx)
        .await
        .unwrap();
    sqlx::query("UPDATE api_tokens SET scopes=$2 WHERE id=$1")
        .bind(token)
        .bind(narrowed)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    rig.world.restart_api().await;
    rig.world.engine().attach_blobs(rig.store.clone());
    rig.say(
        side,
        "A new prompt cannot widen the original recovery grant",
    )
    .await;
    rig.world.wait_state(side, "blocked").await;
    assert_eq!(rig.world.turn_starts(side).len(), 1);
    let original = cuts::cut(&mut rig.world.pool.acquire().await.unwrap(), side)
        .await
        .unwrap()
        .unwrap();
    assert!(
        original.artifacts["side_authorization"]["scope"]["scopes"]
            .as_array()
            .unwrap()
            .contains(&json!("provider.use"))
    );
    rig.world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_observed_side_without_its_confirmed_creation_receipt_cannot_create_a_recovery_child() {
    let rig = Rig::new(
        Can {
            fork_native: true,
            ..Default::default()
        },
        "fake",
        Can::default(),
    )
    .await;
    let parent = rig.thread_with_history("Unconfirmed recovery").await;
    let side = rig
        .world
        .engine()
        .start_side_thread(
            &rig.caller(),
            parent,
            vec![orch_api_contract::threads::ContentPart::Text {
                text: "Keep this side's original source".into(),
            }],
            "side-recovery-unconfirmed".into(),
            None,
        )
        .await
        .unwrap()
        .thread;
    rig.world.run_turn(side, 0, "The side was observed").await;
    rig.world.wait_state(side, "idle").await;
    rig.world.wait_followed_up(side).await;
    remove_side_cache_observation(&rig, side).await;
    // Inject a damaged retained record. Normal writes cannot remove a published artifact; the table lock
    // keeps the trigger disabled only within this transaction, and recovery runs with it enabled again.
    let mut tx = rig.world.pool.pool().begin().await.unwrap();
    sqlx::query("ALTER TABLE thread_history_cuts DISABLE TRIGGER history_cut_identity")
        .execute(&mut *tx)
        .await
        .unwrap();
    let changed = sqlx::query(
        "UPDATE thread_history_cuts SET artifacts=artifacts-'side_native_result'
         WHERE id=$1 AND artifacts ? 'side_native_result'",
    )
    .bind(side)
    .execute(&mut *tx)
    .await
    .unwrap();
    assert_eq!(changed.rows_affected(), 1);
    sqlx::query("ALTER TABLE thread_history_cuts ENABLE TRIGGER history_cut_identity")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    rig.world.restart_api().await;
    rig.world.engine().attach_blobs(rig.store.clone());
    rig.say(
        side,
        "An observed reference is not a new preparation authority",
    )
    .await;
    rig.world.wait_state(side, "blocked").await;
    assert_eq!(rig.world.turn_starts(side).len(), 1);
    assert_eq!(rig.world.count("SELECT count(*) FROM items WHERE thread_id=$1 AND payload->>'marker'='side_started'",parent).await, 1);
    rig.world.shutdown().await;
}

struct SideGap {
    inner: Mutex<Arc<dyn Commands>>,
    parent: Id,
    saved: Mutex<Option<Outgoing>>,
    reached: Notify,
    waiting: std::sync::atomic::AtomicBool,
    visible: std::sync::atomic::AtomicBool,
    release: Semaphore,
}

impl SideGap {
    fn inner(&self) -> Arc<dyn Commands> {
        self.inner.lock().unwrap().clone()
    }
    fn owns(&self, id: Id) -> bool {
        self.saved
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|command| command.id == id)
    }
}

#[async_trait]
impl Commands for SideGap {
    async fn observe(&self, command: Outgoing) -> Result<Outcome, CommandsError> {
        self.inner().observe(command).await
    }
    async fn issue(&self, command: Outgoing) -> Result<(), CommandsError> {
        if command.kind == CommandKind::SessionOpenFenced
            && command.body["thread_id"] != json!(self.parent)
            && command.body["session"]["mode"] != "resume"
        {
            let mut original = self.saved.lock().unwrap();
            if original.is_none() {
                *original = Some(command.clone());
            }
        }
        self.inner().issue(command).await
    }
    async fn wait(&self, id: Id) -> Result<Outcome, CommandsError> {
        let outcome = self.inner().wait(id).await?;
        if self.owns(id)
            && matches!(&outcome, Outcome::Succeeded(_))
            && !self.waiting.swap(true, Ordering::AcqRel)
        {
            self.reached.notify_waiters();
            self.release
                .acquire()
                .await
                .map_err(|_| CommandsError::Stopping)?
                .forget();
        }
        Ok(outcome)
    }
    async fn retained_body(&self, id: Id) -> Result<Option<Json>, CommandsError> {
        self.inner().retained_body(id).await
    }
    async fn retained_command(
        &self,
        id: Id,
    ) -> Result<Option<orch_engine::commands::RetainedCommand>, CommandsError> {
        self.inner().retained_command(id).await
    }
    async fn status(&self, id: Id) -> Result<Option<Info>, CommandsError> {
        if self.owns(id) && !self.visible.load(Ordering::Acquire) {
            return Ok(None);
        }
        self.inner().status(id).await
    }
    async fn cancel(&self, id: Id) -> Result<bool, CommandsError> {
        self.inner().cancel(id).await
    }
    fn hears_behind(&self, machine: Id, stream: &Stream) -> bool {
        self.inner().hears_behind(machine, stream)
    }
}

async fn install_gap(rig: &Rig, parent: Id) -> Arc<SideGap> {
    let saved = Arc::new(Mutex::new(None));
    let capture = saved.clone();
    rig.world
        .restart_api_using(move |inner| {
            let gate = Arc::new(SideGap {
                inner: Mutex::new(inner),
                parent,
                saved: Mutex::new(None),
                reached: Notify::new(),
                waiting: std::sync::atomic::AtomicBool::new(false),
                visible: std::sync::atomic::AtomicBool::new(true),
                release: Semaphore::new(0),
            });
            *capture.lock().unwrap() = Some(gate.clone());
            gate
        })
        .await;
    rig.world.engine().attach_blobs(rig.store.clone());
    saved.lock().unwrap().clone().unwrap()
}

async fn reenter_gap(rig: &Rig, gate: &Arc<SideGap>) {
    let gate = gate.clone();
    // Deterministic controller/link re-entry, not an OS process-kill claim.
    // The old wait remains gated until its replacement has reconciled/refused.
    rig.world
        .restart_api_using(move |inner| {
            *gate.inner.lock().unwrap() = inner;
            gate
        })
        .await;
    rig.world.engine().attach_blobs(rig.store.clone());
}

async fn observed_success(
    rig: &Rig,
    gate: &SideGap,
    side: Id,
) -> (Outgoing, SessionOpenResult, Vec<u8>) {
    tokio::time::timeout(common::PATIENCE, async {
        loop {
            let woke = gate.reached.notified();
            if gate.waiting.load(Ordering::Acquire) {
                break;
            }
            woke.await;
        }
    })
    .await
    .unwrap();
    let original = gate.saved.lock().unwrap().clone().unwrap();
    let entry_bytes = tokio::fs::read(
        rig.world
            .machine
            .data_dir()
            .join("journal")
            .join(format!("{}.json", original.id)),
    )
    .await
    .unwrap();
    let entry: orch_link::journal::Entry = serde_json::from_slice(&entry_bytes).unwrap();
    assert_eq!(entry.cmd.id, original.id);
    assert!(entry.cmd.body == original.body);
    assert_eq!(Some(entry.cmd.deadline), original.deadline);
    assert_eq!(entry.cmd.kind, original.kind.as_str());
    assert_eq!(entry.cmd.key, original.key.wire());
    let opened: SessionOpenResult =
        serde_json::from_value(entry.result().unwrap().outcome.clone().unwrap()).unwrap();
    let body: orch_link_contract::continuations::SessionOpenFenced =
        serde_json::from_value(original.body.clone()).unwrap();
    assert_eq!(body.thread_id, side);
    // Exercise actual machine ownership/SessionOpened ingestion, not a direct
    // UPDATE of the native reference or a fabricated creation success.
    rig.world
        .machine
        .publish(
            &Stream::Session {
                thread_id: side,
                segment_id: body.session.segment_id,
            },
            &Event::SessionOpened(SessionOpened {
                native_session_ref: opened.native_session_ref.clone(),
                resumed: false,
            }),
        )
        .await;
    eventually(
        "the original SessionOpened event commits before retained-success publication",
        || async {
            threads::segment(
                &mut rig.world.pool.acquire().await.unwrap(),
                body.session.segment_id,
            )
            .await
            .unwrap()
            .is_some_and(|segment| {
                segment.native_session_ref.as_deref() == Some(opened.native_session_ref.as_str())
            })
        },
    )
    .await;
    assert!(
        cuts::cut(&mut rig.world.pool.acquire().await.unwrap(), side)
            .await
            .unwrap()
            .unwrap()
            .artifacts
            .get("side_native_result")
            .is_none()
    );
    (original, opened, entry_bytes)
}

async fn retained_side_result(
    rig: &Rig,
    side: Id,
    original: &Outgoing,
    native: &SessionOpenResult,
) {
    eventually(
        "the side retains its original supplier success despite an earlier observed reference",
        || async {
            cuts::cut(&mut rig.world.pool.acquire().await.unwrap(), side)
                .await
                .unwrap()
                .is_some_and(|cut| {
                    cut.artifacts["side_native_result"]["command_id"] == original.id.to_string()
                        && cut.artifacts["side_native_result"]["native_session_ref"]
                            == native.native_session_ref
                })
        },
    )
    .await;
    let row = link::command(&mut rig.world.pool.acquire().await.unwrap(), original.id)
        .await
        .unwrap()
        .unwrap();
    assert!(row.body == original.body);
    assert_eq!(row.machine_id, original.machine);
    assert_eq!(row.serial_key(), original.key.wire().as_deref());
    assert_eq!(
        Some(orch_types::Time::new(row.deadline_at)),
        original.deadline
    );
    let actual = rig
        .world
        .commands
        .as_engine()
        .status(original.id)
        .await
        .unwrap()
        .unwrap()
        .outcome
        .unwrap();
    assert!(
        matches!(actual,Outcome::Succeeded(value) if value==serde_json::to_value(native).unwrap()),
        "the actual original supplier outcome must be the exact journal-native success"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn observed_neutral_and_empty_side_successes_rejoin_the_original_receipt_before_one_delivery()
{
    for history in [false, true] {
        let rig = Rig::new(
            Can {
                import_neutral: true,
                ..Default::default()
            },
            "fake",
            Can::default(),
        )
        .await;
        let parent = if history {
            let parent = rig.thread_with_history("Neutral gap").await;
            accounted(&rig, parent).await;
            parent
        } else {
            rig.world.thread("Empty gap").await
        };
        let gate = install_gap(&rig, parent).await;
        let side = rig
            .world
            .engine()
            .start_side_thread(
                &rig.caller(),
                parent,
                vec![orch_api_contract::threads::ContentPart::Text {
                    text: "The same original side input".into(),
                }],
                format!("observed-success-{history}"),
                None,
            )
            .await
            .unwrap();
        let (original, native, journal) = observed_success(&rig, &gate, side.thread).await;
        assert_eq!(
            original.body["session"]["mode"],
            if history { "import_neutral" } else { "new" }
        );
        let input = side.submitted.input.id;
        reenter_gap(&rig, &gate).await;
        retained_side_result(&rig, side.thread, &original, &native).await;
        let start = rig.world.turn_start(side.thread, 0).await;
        let actual = threads::turn(&mut rig.world.pool.acquire().await.unwrap(), start.turn_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(actual.input_id, input);
        gate.release.add_permits(1);
        rig.world
            .run_turn(side.thread, 0, "Exactly one original delivery")
            .await;
        accounted(&rig, side.thread).await;
        assert_eq!(rig.world.turn_starts(side.thread).len(), 1);
        assert_eq!(
            rig.opens()
                .iter()
                .filter(|open| open.thread_id == side.thread && open.mode != SessionMode::Resume)
                .count(),
            1
        );
        assert_eq!(rig.world.machine.handler.runs_of(original.id), 1);
        assert!(
            tokio::fs::read(
                rig.world
                    .machine
                    .data_dir()
                    .join("journal")
                    .join(format!("{}.json", original.id))
            )
            .await
            .unwrap()
                == journal
        );
        let kept = threads::input(&mut rig.world.pool.acquire().await.unwrap(), input)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(kept.state, "delivered");
        assert_eq!(kept.turn_id, Some(start.turn_id));
        assert_eq!(kept.thread_id, side.thread);
        assert!(kept.content == serde_json::to_value(&side.submitted.input.content).unwrap());
        drop(gate);
        rig.world.shutdown().await;
        assert_eq!(
            rig.world.turn_starts(side.thread).len(),
            1,
            "the old gated controller cannot deliver again while it drains"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_observed_side_with_an_unavailable_original_outcome_keeps_its_input_and_creates_no_other_child()
 {
    for history in [false, true] {
        let rig = Rig::new(
            Can {
                import_neutral: true,
                ..Default::default()
            },
            "fake",
            Can::default(),
        )
        .await;
        let parent = if history {
            let parent = rig.thread_with_history("Unconfirmed neutral gap").await;
            accounted(&rig, parent).await;
            parent
        } else {
            rig.world.thread("Unconfirmed empty gap").await
        };
        let gate = install_gap(&rig, parent).await;
        let side = rig
            .world
            .engine()
            .start_side_thread(
                &rig.caller(),
                parent,
                vec![orch_api_contract::threads::ContentPart::Text {
                    text: "Keep unconfirmed side words".into(),
                }],
                format!("observed-unknown-{history}"),
                None,
            )
            .await
            .unwrap();
        let (original, native, journal) = observed_success(&rig, &gate, side.thread).await;
        let before = threads::thread(&mut rig.world.pool.acquire().await.unwrap(), side.thread)
            .await
            .unwrap()
            .unwrap();
        let accepted = threads::input(
            &mut rig.world.pool.acquire().await.unwrap(),
            side.submitted.input.id,
        )
        .await
        .unwrap()
        .unwrap();
        let original_segment = before.current_segment_id.unwrap();
        let original_turn = before.current_turn_id.unwrap();
        // Reconstruct an unavailable controller outcome supplier. The machine's
        // genuine Done is still on disk; its observed reference is not a result.
        gate.visible.store(false, Ordering::Release);
        reenter_gap(&rig, &gate).await;
        // Pending alone can be a restart transition. Join the same input and
        // terminal turn to the actual source-scoped history-unavailable block.
        eventually("the original side input reaches its exact terminal history-unavailable block",||async {
            sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM threads h JOIN thread_inputs i ON i.thread_id=h.node_id AND i.id=$2
                JOIN turns t ON t.thread_id=h.node_id AND t.id=$6 AND t.input_id=i.id AND t.segment_id=$3 AND t.continuation_id=$4
                WHERE h.node_id=$1 AND h.current_turn_id IS NULL AND h.current_segment_id=$3
                  AND h.active_continuation_id=$4 AND h.execution_revision=$5 AND i.continuation_id=$4
                  AND h.state='blocked' AND h.blocked_reason='error' AND i.state='pending' AND i.state_detail='history_unavailable'
                  AND t.state='failed' AND t.error->>'code'='history_unavailable')")
                .bind(side.thread).bind(side.submitted.input.id).bind(original_segment).bind(before.active_continuation_id)
                .bind(before.execution_revision).bind(original_turn).fetch_one(rig.world.pool.pool()).await.unwrap_or(false)
        }).await;
        let input = threads::input(
            &mut rig.world.pool.acquire().await.unwrap(),
            side.submitted.input.id,
        )
        .await
        .unwrap()
        .unwrap();
        // queue_held records a person's Stop, not this thread error block.
        // Preserve its original flag instead of manufacturing that action.
        assert_eq!(input.queue_held, accepted.queue_held);
        assert_eq!(input.state_detail.as_deref(), Some("history_unavailable"));
        assert_eq!(input.continuation_id, accepted.continuation_id);
        assert!(input.content == serde_json::to_value(&side.submitted.input.content).unwrap());
        assert_eq!(input.thread_id, side.thread);
        assert!(rig.world.turn_starts(side.thread).is_empty());
        assert_eq!(
            rig.opens()
                .iter()
                .filter(|open| open.thread_id == side.thread && open.mode != SessionMode::Resume)
                .count(),
            1
        );
        assert!(
            rig.opens()
                .iter()
                .all(|open| open.thread_id != side.thread || open.mode != SessionMode::Resume)
        );
        assert!(gate.status(original.id).await.unwrap().is_none());
        let cut = cuts::cut(&mut rig.world.pool.acquire().await.unwrap(), side.thread)
            .await
            .unwrap()
            .unwrap();
        assert!(cut.artifacts.get("side_native_result").is_none());
        assert!(
            cut.artifacts[if history {
                "side_open_neutral"
            } else {
                "side_open_empty"
            }] == serde_json::to_value(&original).unwrap()
        );
        let segment_id = rig.world.segment_of(side.thread).await;
        let segment = threads::segment(&mut rig.world.pool.acquire().await.unwrap(), segment_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            segment.native_session_ref.as_deref(),
            Some(native.native_session_ref.as_str())
        );
        assert!(
            tokio::fs::read(
                rig.world
                    .machine
                    .data_dir()
                    .join("journal")
                    .join(format!("{}.json", original.id))
            )
            .await
            .unwrap()
                == journal
        );
        gate.release.add_permits(1);
        drop(gate);
        rig.world.shutdown().await;
        assert!(
            rig.world.turn_starts(side.thread).is_empty(),
            "a late original receipt is housekeeping, not a second admission"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retaining_an_observed_side_success_does_not_use_the_frozen_token_to_override_current_provider_narrowing()
 {
    let rig = Rig::new(
        Can {
            import_neutral: true,
            ..Default::default()
        },
        "fake",
        Can::default(),
    )
    .await;
    let parent = rig.thread_with_history("Narrowed side gap").await;
    accounted(&rig, parent).await;
    let gate = install_gap(&rig, parent).await;
    let token = new_id();
    let scopes = Permission::ALL
        .iter()
        .map(|permission| permission.as_str().to_owned())
        .collect::<Vec<_>>();
    identity::create_api_token(
        &mut rig.world.pool.acquire().await.unwrap(),
        &identity::NewApiToken {
            id: token,
            user_id: rig.world.ada,
            name: "synthetic-side-gap",
            token_hash: &[0x53; 32],
            scopes: &scopes,
            node_id: Some(rig.world.root),
            at: rig.world.clock.now(),
            expires_at: None,
        },
    )
    .await
    .unwrap();
    let caller = Caller::token(
        rig.world.ada,
        TokenGrant::from_stored(&scopes, Some(rig.world.root))
            .unwrap()
            .for_token(token),
    );
    let side = rig
        .world
        .engine()
        .start_side_thread(
            &caller,
            parent,
            vec![orch_api_contract::threads::ContentPart::Text {
                text: "Retain, do not deliver under a narrowed credential".into(),
            }],
            "side-current-provider-scope".into(),
            None,
        )
        .await
        .unwrap();
    let (original, native, _) = observed_success(&rig, &gate, side.thread).await;
    let narrowed = scopes
        .iter()
        .filter(|scope| scope.as_str() != "provider.use")
        .cloned()
        .collect::<Vec<_>>();
    let mut tx = rig.world.pool.begin().await.unwrap();
    orch_db::queries::transaction_guard::lock_authority(&mut tx)
        .await
        .unwrap();
    assert_eq!(
        sqlx::query("UPDATE api_tokens SET scopes=$2 WHERE id=$1")
            .bind(token)
            .bind(narrowed)
            .execute(&mut *tx)
            .await
            .unwrap()
            .rows_affected(),
        1
    );
    tx.commit().await.unwrap();
    reenter_gap(&rig, &gate).await;
    retained_side_result(&rig, side.thread, &original, &native).await;
    rig.world
        .wait_input(side.submitted.input.id, "pending")
        .await;
    let cut = cuts::cut(&mut rig.world.pool.acquire().await.unwrap(), side.thread)
        .await
        .unwrap()
        .unwrap();
    assert!(
        cut.artifacts["side_authorization"]["scope"]["scopes"]
            .as_array()
            .unwrap()
            .contains(&json!("provider.use")),
        "the accepted scope stays frozen, but cannot widen the current token"
    );
    assert!(rig.world.turn_starts(side.thread).is_empty());
    assert_eq!(
        rig.opens()
            .iter()
            .filter(|open| open.thread_id == side.thread && open.mode != SessionMode::Resume)
            .count(),
        1
    );
    gate.release.add_permits(1);
    drop(gate);
    rig.world.shutdown().await;
    assert!(
        rig.world.turn_starts(side.thread).is_empty(),
        "the old frozen grant cannot deliver while its controller drains"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn empty_all_is_confirmed_before_birth_while_explicit_nothing_is_the_uncaptured_control() {
    for all in [true, false] {
        let rig = Rig::new(Can::default(), "fake", Can::default()).await;
        let source = if all {
            rig.world.thread("Fresh empty All").await
        } else {
            let source = rig.thread_with_history("Explicit Nothing source").await;
            accounted(&rig, source).await;
            source
        };
        rig.world.engine().stop();
        let request = if all {
            rig.fork()
        } else {
            ForkRequest {
                history: Some(orch_api_contract::forks::ForkHistory {
                    mode: orch_api_contract::forks::HistoryMode::Nothing,
                    from_turn: None,
                    to_turn: None,
                }),
                ..rig.fork()
            }
        };
        let plan = rig.plan(source, &request).await;
        assert_eq!(plan.history.method, HistoryMethod::Nothing);
        let job = rig.confirm(source, &plan).await;
        let target = rig.new_thread_of(&job);
        assert!(
            threads::thread(&mut rig.world.pool.acquire().await.unwrap(), target)
                .await
                .unwrap()
                .is_none()
        );
        let cut = cuts::cut(&mut rig.world.pool.acquire().await.unwrap(), job.id)
            .await
            .unwrap();
        if all {
            let cut = cut.unwrap();
            assert_eq!(cut.last_turn, 0);
            assert_eq!(
                cuts::item_count(&mut rig.world.pool.acquire().await.unwrap(), cut.id)
                    .await
                    .unwrap(),
                0
            );
        } else {
            assert!(cut.is_none());
        }
        rig.world.restart_api().await;
        rig.world.engine().attach_blobs(rig.store.clone());
        rig.wait_job(job.id, "succeeded").await;
        let finished = rig.job(job.id).await;
        assert_eq!(
            finished.plan.get("carried_history_range"),
            if all { Some(&Json::Null) } else { None },
            "confirmed empty All has a recorded empty boundary; explicit Nothing carries no history"
        );
        let row = threads::thread(&mut rig.world.pool.acquire().await.unwrap(), target)
            .await
            .unwrap()
            .unwrap();
        let segment = threads::segment(
            &mut rig.world.pool.acquire().await.unwrap(),
            row.current_segment_id.unwrap(),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(segment.thread_id, target);
        assert_eq!(segment.continuation_id, row.active_continuation_id);
        assert!(segment.native_session_ref.is_none());
        assert!(!row.queue_held);
        assert!(row.queue_held_by.is_none() && row.queue_held_at.is_none());
        assert_eq!(row.queue_revision, 1);
        let owner:(String,Option<Id>,bool,i64)=sqlx::query_as("SELECT status,current_segment_id,queue_held,queue_revision FROM thread_continuations WHERE thread_id=$1 AND id=$2")
            .bind(target).bind(row.active_continuation_id).fetch_one(rig.world.pool.pool()).await.unwrap();
        assert_eq!(owner, ("ready".into(), Some(segment.id), false, 1));
        assert!(
            rig.world
                .scalar::<bool>(
                    "SELECT ready FROM thread_native_guards WHERE thread_id=$1",
                    target
                )
                .await
        );
        let empty: Option<Id> = rig
            .world
            .scalar(
                "SELECT empty_segment_id FROM thread_native_guards WHERE thread_id=$1",
                target,
            )
            .await;
        assert_eq!(
            empty,
            if all { None } else { Some(segment.id) },
            "only explicit Nothing grants the intentional empty-segment override"
        );
        assert!(rig.world.sync(target).await.turns.is_empty());
        assert!(rig.world.turn_starts(target).is_empty());
        assert!(rig.opens().iter().all(|open| open.thread_id != target));
        assert_eq!(
            rig.world
                .count(
                    "SELECT count(*) FROM thread_inputs WHERE thread_id=$1",
                    target
                )
                .await,
            0
        );
        assert_eq!(
            rig.world
                .count(
                    "SELECT count(*) FROM usage_events WHERE thread_id=$1",
                    target
                )
                .await,
            0
        );
        assert_eq!(
            rig.world
                .count("SELECT count(*) FROM model_inputs WHERE node_id=$1", target)
                .await,
            0
        );
        rig.world.shutdown().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_nonempty_confirmation_with_a_missing_cut_refuses_without_losing_the_original_authored_queue()
 {
    let rig = Rig::new(
        Can {
            import_neutral: true,
            ..Default::default()
        },
        "fake",
        Can::default(),
    )
    .await;
    let source = rig.thread_with_history("Missing-cut checkpoint").await;
    accounted(&rig, source).await;
    let completed = threads::latest_turn(&mut rig.world.pool.acquire().await.unwrap(), source)
        .await
        .unwrap()
        .unwrap();
    rig.world.engine().stop();
    let input = rig
        .world
        .engine()
        .submit_input(
            &rig.caller(),
            source,
            queued("Keep my original stopped words"),
        )
        .await
        .unwrap();
    rig.world
        .engine()
        .interrupt(&rig.caller(), source, completed.id)
        .await
        .unwrap();
    // submit_input already admitted file preparation before Stop. Let that
    // actual housekeeping finish with the input still held, then freeze the
    // baseline for the separate missing-cut refusal; no turn is delivered.
    rig.world.restart_api().await;
    rig.world.engine().attach_blobs(rig.store.clone());
    eventually(
        "the stopped original input finishes its already admitted preparation",
        || async {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM thread_inputs WHERE id=$1
            AND state='pending' AND queue_held AND state_detail='files_ready' AND turn_id IS NULL)",
            )
            .bind(input.input.id)
            .fetch_one(rig.world.pool.pool())
            .await
            .unwrap_or(false)
        },
    )
    .await;
    assert_eq!(
        rig.world.turn_starts(source).len(),
        1,
        "preparation cannot deliver the stopped input"
    );
    rig.world.engine().stop();
    let original: Json = sqlx::query_scalar("SELECT to_jsonb(i) FROM thread_inputs i WHERE id=$1")
        .bind(input.input.id)
        .fetch_one(rig.world.pool.pool())
        .await
        .unwrap();
    assert_eq!(original["state"], "pending");
    assert_eq!(original["queue_held"], true);
    let queue:Json=sqlx::query_scalar("SELECT jsonb_build_object('held',queue_held,'by',queue_held_by,'at',queue_held_at,
        'revision',queue_revision,'continuation',active_continuation_id,'execution',execution_revision) FROM threads WHERE node_id=$1")
        .bind(source).fetch_one(rig.world.pool.pool()).await.unwrap();
    let plan = rig.plan(source, &rig.fork()).await;
    let job = rig.confirm(source, &plan).await;
    let target = rig.new_thread_of(&job);
    // Explicit reconstruction of a lost pre-run controller cut. No native
    // command, child or completed turn is invented by this metadata removal.
    cuts::release(&mut rig.world.pool.acquire().await.unwrap(), job.id)
        .await
        .unwrap();
    assert!(
        cuts::cut(&mut rig.world.pool.acquire().await.unwrap(), job.id)
            .await
            .unwrap()
            .is_none()
    );
    rig.world.restart_api().await;
    rig.world.engine().attach_blobs(rig.store.clone());
    let ended = rig.wait_job(job.id, "failed").await;
    assert_eq!(
        ended.error.as_ref().unwrap()["code"],
        "history_source_unconfirmed"
    );
    let kept: Json = sqlx::query_scalar("SELECT to_jsonb(i) FROM thread_inputs i WHERE id=$1")
        .bind(input.input.id)
        .fetch_one(rig.world.pool.pool())
        .await
        .unwrap();
    let changed_columns: Vec<_> = original
        .as_object()
        .unwrap()
        .keys()
        .filter(|column| kept.get(column.as_str()) != original.get(column.as_str()))
        .map(String::as_str)
        .collect();
    let detail_class = |row: &Json| match row["state_detail"].as_str() {
        None if row["state_detail"].is_null() => "none",
        Some("files") => "files",
        Some("files_ready") => "files_ready",
        Some("history_unavailable") => "history_unavailable",
        Some("native_action.unavailable") => "native_action.unavailable",
        _ => "other",
    };
    assert!(
        kept == original,
        "the original authored input changed columns: {changed_columns:?}; detail {} -> {}",
        detail_class(&original),
        detail_class(&kept)
    );
    assert!(rig.opens().iter().all(|open| open.thread_id != target));
    let after:Json=sqlx::query_scalar("SELECT jsonb_build_object('held',queue_held,'by',queue_held_by,'at',queue_held_at,
        'revision',queue_revision,'continuation',active_continuation_id,'execution',execution_revision) FROM threads WHERE node_id=$1")
        .bind(source).fetch_one(rig.world.pool.pool()).await.unwrap();
    assert!(after == queue);
    assert!(rig.world.turn_starts(target).is_empty());
    assert_eq!(rig.world.turn_starts(source).len(), 1);
    rig.world.shutdown().await;
}

fn words(turn: i64, text: String) -> RecordedEntry {
    RecordedEntry::Said(orch_agent_contract::discovery::RecordedWords {
        native_ref: format!("said:external-{turn}"),
        turn,
        text,
        left_out: 0,
        at: None,
    })
}

fn external_answer(turn: i64, text: &str) -> RecordedEntry {
    RecordedEntry::Item(RecordedItem {
        native_ref: format!("external-answer-{turn}"),
        turn,
        stream: "main".into(),
        parent_native_ref: None,
        status: orch_agent_contract::events::ItemStatus::Completed,
        content: ItemContent::AgentMessage(orch_agent_contract::events::AgentMessagePayload {
            text: text.into(),
            text_len: i64::try_from(text.len()).unwrap(),
            phase: None,
        }),
        at: None,
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_legitimately_bounded_adopted_suffix_does_not_certify_native_prefix_one_through_n() {
    let rig = Rig::new(
        Can {
            fork_native: true,
            import_neutral: true,
            ..Default::default()
        },
        "fake",
        Can::default(),
    )
    .await;
    let source = rig.world.thread("Reconstructed bounded adoption").await;
    let segment = rig.world.segment_of(source).await;
    rig.world.open_session_for_reports(source, segment).await;
    let native = threads::segment(&mut rig.world.pool.acquire().await.unwrap(), segment)
        .await
        .unwrap()
        .unwrap()
        .native_session_ref
        .unwrap();
    // A declared external-native history checkpoint, not executed model turns
    // or synthetic completed Orch turns. The normal record writer must omit
    // the first whole turn under its actual unchanged 32MiB bound.
    let entries = vec![
        words(
            1,
            "x".repeat(orch_agent_contract::discovery::RECORD_MAX_BYTES),
        ),
        words(2, "Retained external input2".into()),
        external_answer(2, "Retained external answer2"),
        words(3, "Retained external input3".into()),
        external_answer(3, "Retained external answer3"),
    ];
    rig.native.lock().unwrap().sessions.insert(
        native.clone(),
        NativeSession {
            turns: (1..=3).map(|ordinal| (new_id(), ordinal)).collect(),
            entries,
            next: 3,
            opaque: false,
            profile: Some((rig.world.provider, rig.world.agent)),
        },
    );
    let (bytes, read) = rig
        .native
        .lock()
        .unwrap()
        .reply(&native, rig.world.provider, rig.world.agent)
        .unwrap();
    let (retained, unknown) =
        orch_agent_contract::discovery::read_record(std::str::from_utf8(&bytes).unwrap());
    assert_eq!(unknown, 0);
    assert_eq!(
        (
            read.turns,
            read.entries,
            retained.first().unwrap().turn(),
            retained.last().unwrap().turn()
        ),
        (2, 4, 2, 3)
    );
    assert!(
        bytes.len() <= orch_agent_contract::discovery::RECORD_MAX_BYTES
            && !read.report.warnings.is_empty()
    );
    let imported = rig
        .world
        .engine()
        .take_in_record(source, segment, Some(rig.world.ada), &retained)
        .await
        .unwrap();
    assert_eq!(imported.items, 4);
    assert_eq!(
        rig.world
            .count("SELECT count(*) FROM turns WHERE thread_id=$1", source)
            .await,
        0
    );
    let plan = rig.plan(source, &rig.fork()).await;
    assert_eq!(plan.history.method, HistoryMethod::NativeFork);
    let job = rig.confirm(source, &plan).await;
    rig.wait_job(job.id, "succeeded").await;
    let target = rig.new_thread_of(&job);
    assert!(
        rig.opens()
            .iter()
            .all(|open| open.thread_id != target || open.mode != SessionMode::Fork),
        "a genuine K..N bounded record must refuse native creation before selecting a converted method"
    );
    assert_eq!(
        rig.opens()
            .iter()
            .filter(|open| open.thread_id == target && open.mode == SessionMode::ImportNeutral)
            .count(),
        1
    );
    assert!(rig.world.turn_starts(target).is_empty());
    assert!(rig.world.turn_starts(source).is_empty());
    let (_, after) = rig
        .native
        .lock()
        .unwrap()
        .reply(&native, rig.world.provider, rig.world.agent)
        .unwrap();
    assert_eq!(after, read);
    rig.world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_successful_child_with_a_legitimately_bounded_record_is_retained_and_reduced_without_replacement()
 {
    let rig = Rig::new(
        Can {
            fork_native: true,
            import_neutral: true,
            ..Default::default()
        },
        "fake",
        Can::default(),
    )
    .await;
    let source = rig
        .thread_with_history("Complete source, bounded child")
        .await;
    rig.say(source, "Second actual input").await;
    rig.world.run_turn(source, 1, "Second actual answer").await;
    accounted(&rig, source).await;
    rig.native.lock().unwrap().extra_next_fork = Some(words(
        1,
        "x".repeat(orch_agent_contract::discovery::RECORD_MAX_BYTES),
    ));
    let plan = rig.plan(source, &rig.fork()).await;
    let job = rig.confirm(source, &plan).await;
    let target = rig.new_thread_of(&job);
    let ended = rig.wait_job(job.id, "failed").await;
    assert!(ended.plan.get("commit_done").is_none());
    let original: Outgoing =
        serde_json::from_value(ended.plan["history_wire"]["open_0"].clone()).unwrap();
    let entry: orch_link::journal::Entry = serde_json::from_slice(
        &tokio::fs::read(
            rig.world
                .machine
                .data_dir()
                .join("journal")
                .join(format!("{}.json", original.id)),
        )
        .await
        .unwrap(),
    )
    .unwrap();
    assert_eq!(entry.cmd.id, original.id);
    assert!(entry.cmd.body == original.body);
    assert_eq!(Some(entry.cmd.deadline), original.deadline);
    assert_eq!(entry.cmd.kind, original.kind.as_str());
    assert_eq!(entry.cmd.key, original.key.wire());
    let native: SessionOpenResult =
        serde_json::from_value(entry.result().unwrap().outcome.clone().unwrap()).unwrap();
    let (bytes, read) = rig
        .native
        .lock()
        .unwrap()
        .reply(
            &native.native_session_ref,
            rig.world.provider,
            rig.world.agent,
        )
        .unwrap();
    let (suffix, unknown) =
        orch_agent_contract::discovery::read_record(std::str::from_utf8(&bytes).unwrap());
    assert_eq!(unknown, 0);
    assert_eq!(
        (
            suffix.first().unwrap().turn(),
            suffix.last().unwrap().turn()
        ),
        (2, 2)
    );
    assert_eq!(read.turns, 1);
    assert!(!read.report.warnings.is_empty());
    let cut = cuts::cut(&mut rig.world.pool.acquire().await.unwrap(), job.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        cut.artifacts["native_boundary"]["evidence"]["turns"], 2,
        "creation was admitted from complete source evidence"
    );
    assert_eq!(
        cut.artifacts[format!("native_result_{}", original.id)]["result"]["native_session_ref"],
        json!(native.native_session_ref)
    );
    assert!(
        cut.artifacts[format!("native_result_{}", original.id)]["command"]
            == serde_json::to_value(&original).unwrap()
    );
    assert_eq!(rig.world.machine.handler.runs_of(original.id), 1);
    assert_eq!(
        rig.opens()
            .iter()
            .filter(|open| open.thread_id == target && open.mode != SessionMode::Resume)
            .count(),
        1
    );
    assert!(rig.world.ordered.lock().unwrap().iter().any(|(_,command)|matches!(command,Command::SessionCloseFenced(close)
        if close.thread_id==target && close.segment_id==serde_json::from_value::<Id>(original.body["session"]["segment_id"].clone()).unwrap())));
    assert!(rig.world.turn_starts(target).is_empty());
    rig.world.shutdown().await;
}
