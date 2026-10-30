//! Owned native-result publication, source-generation refusal and retained original-lane recovery.

use super::*;
use crate::test_common as common;
use crate::{
    Engine, EngineParts,
    continuations::recipe::{self, Expected, Request},
};
use orch_agent_contract::history::{NativeInputPrefix, NativePrefixOpened, NativePrefixSource};
use orch_api_contract::threads::{
    ContentPart, ThreadContinuationActivateBody, ThreadPromptVersionCreateBody,
};
use orch_authz::Principal;
use orch_link_contract::commands::{
    Command, CommandError, CommandKind, CommandResult, Empty, ErrorCode,
};
use orch_types::{Hash, Seq, new_id};
use std::{
    collections::HashSet,
    sync::{Arc, Mutex, PoisonError},
};
use tokio::sync::oneshot;

type Proof<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

mod lifecycle;

struct Boundary {
    reached: oneshot::Sender<()>,
    resume: oneshot::Receiver<()>,
}
tokio::task_local! {static AFTER_CREATION:Mutex<Option<Boundary>>;}

pub(super) async fn after_creation() {
    let boundary = AFTER_CREATION
        .try_with(|slot| slot.lock().unwrap_or_else(PoisonError::into_inner).take())
        .ok()
        .flatten();
    if let Some(boundary) = boundary {
        let _ = boundary.reached.send(());
        let _ = boundary.resume.await;
    }
}

fn phase<T>(result: Result<T, Stop>) -> Proof<T> {
    result.map_err(|error| format!("owned continuation phase: {error:?}").into())
}

/// Only this exact issued command is supplied by the recovered production journal.
/// Every other native command follows the existing real link fixture unchanged.
struct RecoveredCommands {
    inner: Arc<dyn crate::commands::Commands>,
    server: orch_link::commands::CommandServer,
    expected: Outgoing,
    command: orch_link_contract::commands::Cmd,
    issued: Mutex<Option<Outgoing>>,
}

#[async_trait]
impl crate::commands::Commands for RecoveredCommands {
    async fn issue(&self, command: Outgoing) -> Result<(), crate::commands::CommandsError> {
        if command.id != self.expected.id {
            return self.inner.issue(command).await;
        }
        if command != self.expected {
            return Err(crate::commands::CommandsError::Body(
                "The recovered command identity/body/key/machine changed.".into(),
            ));
        }
        *self.issued.lock().unwrap_or_else(PoisonError::into_inner) = Some(command);
        self.server.on_cmd(self.command.clone()).await;
        Ok(())
    }
    async fn retained_body(&self, id: Id) -> Result<Option<Json>, crate::commands::CommandsError> {
        if id == self.expected.id {
            Ok(Some(self.expected.body.clone()))
        } else {
            self.inner.retained_body(id).await
        }
    }
    async fn retained_command(
        &self,
        id: Id,
    ) -> Result<Option<crate::commands::RetainedCommand>, crate::commands::CommandsError> {
        if id == self.expected.id {
            Ok(Some(crate::commands::RetainedCommand {
                machine: self.expected.machine,
                body: self.expected.body.clone(),
            }))
        } else {
            self.inner.retained_command(id).await
        }
    }
    async fn status(
        &self,
        id: Id,
    ) -> Result<Option<crate::commands::Info>, crate::commands::CommandsError> {
        if id != self.expected.id {
            return self.inner.status(id).await;
        }
        Ok(self.server.entry(id).map(|entry| crate::commands::Info {
            stage: if entry.result().is_some() {
                crate::commands::Stage::Over
            } else {
                crate::commands::Stage::Accepted
            },
            issued_at: entry.accepted_at.get(),
            outcome: entry.result().map(|result| match &result.outcome {
                Ok(value) => crate::commands::Outcome::Succeeded(value.clone()),
                Err(error) => crate::commands::Outcome::Failed(error.clone()),
            }),
        }))
    }
    async fn wait(
        &self,
        id: Id,
    ) -> Result<crate::commands::Outcome, crate::commands::CommandsError> {
        if id != self.expected.id {
            return self.inner.wait(id).await;
        }
        loop {
            if let Some(result) = self.status(id).await?.and_then(|info| info.outcome) {
                return Ok(result);
            }
            // Reuse the fixture supervisor cadence; the caller retains its original25s proof budget.
            tokio::time::sleep(common::config().sweep_every).await;
        }
    }
    async fn cancel(&self, id: Id) -> Result<bool, crate::commands::CommandsError> {
        if id != self.expected.id {
            return self.inner.cancel(id).await;
        }
        let active = self
            .server
            .entry(id)
            .is_some_and(|entry| entry.result().is_none());
        if active {
            self.server
                .on_cancel(orch_link_contract::commands::CmdCancel { id })
                .await;
        }
        Ok(active)
    }
    fn hears_behind(&self, machine: Id, stream: &orch_link_contract::streams::Stream) -> bool {
        self.inner.hears_behind(machine, stream)
    }
}

struct Fixture {
    world: common::World,
    engine: Engine,
    thread: Id,
    source: Lane,
    original: Lane,
    source_turn: Id,
    source_input: Id,
    current: Arc<Mutex<Option<NativePrefixSource>>>,
    file: tempfile::TempDir,
    pending: threads::InputRow,
    file_work: [usize; 3],
    frozen_turns: Vec<Json>,
    leases: i64,
}

impl Fixture {
    async fn new(stale_source: bool) -> Proof<Self> {
        let world = common::World::new().await;
        let thread = world.thread("Native continuation ownership").await;
        world
            .engine()
            .submit_input(
                &Caller::session(world.ada),
                thread,
                common::queued("Original retained words"),
            )
            .await?;
        let turn = world.run_turn(thread, 0, "Retained original answer").await;
        common::eventually("the source turn's native accounting finishes", || async {
            match world.pool.acquire().await {
                Ok(mut conn) => !threads::turn_end_work_pending(&mut conn, thread)
                    .await
                    .unwrap_or(true),
                Err(_) => false,
            }
        })
        .await;
        let (source, input) = {
            let mut conn = world.pool.acquire().await?;
            let row = threads::thread(&mut conn, thread)
                .await?
                .ok_or("thread missing")?;
            let source = admission::lane(
                &mut conn,
                thread,
                row.active_continuation_id,
                row.current_segment_id.ok_or("source segment missing")?,
            )
            .await?
            .0;
            let input = threads::turn(&mut conn, turn.turn_id)
                .await?
                .ok_or("native source turn missing")?
                .input_id;
            (source, input)
        };
        if stale_source {
            let mut tx = world.pool.begin().await?;
            let row = threads::lock(&mut tx, thread)
                .await?
                .ok_or("thread missing")?;
            let seq = orch_db::sequences::next(&mut tx, thread)
                .await?
                .ok_or("thread sequence missing")?;
            let active = new_id();
            rows::insert(
                &mut tx,
                &rows::NewContinuation {
                    id: active,
                    thread,
                    parent: source.continuation_id,
                    before_input: input,
                    actor: world.ada,
                    at: world.clock.now(),
                    seq,
                    queue: rows::QueueRow {
                        held: false,
                        held_by: None,
                        held_at: None,
                        revision: 1,
                    },
                },
            )
            .await?;
            sqlx::query("UPDATE thread_continuations SET status='ready' WHERE id=$1")
                .bind(active)
                .execute(&mut *tx)
                .await?;
            sqlx::query("UPDATE environments SET generation=generation+1 WHERE id=$1")
                .bind(world.environment)
                .execute(&mut *tx)
                .await?;
            let changed=sqlx::query("UPDATE threads SET active_continuation_id=$2,execution_revision=execution_revision+1,
                current_segment_id=NULL,seq_state=$3 WHERE node_id=$1 AND current_turn_id IS NULL")
                .bind(thread).bind(active).bind(seq).execute(&mut *tx).await?.rows_affected();
            assert_eq!(
                changed, 1,
                "the synthetic stale-source checkpoint must replace exactly its idle selected lane"
            );
            assert_eq!(row.active_continuation_id, source.continuation_id);
            tx.commit().await?;
            world.engine().forget_open_sessions(thread);
            world
                .engine()
                .submit_input(
                    &Caller::session(world.ada),
                    thread,
                    common::queued("Valid current generation"),
                )
                .await?;
            world.run_turn(thread, 1, "Valid current answer").await;
            common::eventually(
                "the selected generation finishes its native accounting",
                || async {
                    match world.pool.acquire().await {
                        Ok(mut conn) => !threads::turn_end_work_pending(&mut conn, thread)
                            .await
                            .unwrap_or(true),
                        Err(_) => false,
                    }
                },
            )
            .await;
        }
        let old = world.engine();
        old.stop();
        common::eventually(
            "the original dispatcher drains before manually claiming jobs",
            || {
                let clear = !old
                    .inner
                    .flights
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .contains_key(&thread);
                async move { clear }
            },
        )
        .await;
        let original = {
            let mut conn = world.pool.acquire().await?;
            let row = threads::thread(&mut conn, thread)
                .await?
                .ok_or("thread missing")?;
            admission::lane(
                &mut conn,
                thread,
                row.active_continuation_id,
                row.current_segment_id.ok_or("current segment missing")?,
            )
            .await?
            .0
        };
        let mut descriptor = common::descriptor();
        descriptor["capabilities"]["native_input_prefix"] = json!("turn_start");
        sqlx::query("UPDATE agents SET descriptor=$2 WHERE id=$1")
            .bind(world.agent)
            .bind(&descriptor)
            .execute(world.pool.pool())
            .await?;
        let engine = Engine::new(EngineParts {
            db: world.pool.clone(),
            clock: world.clock.clone(),
            config: common::config(),
            commands: world.commands.as_engine(),
            access: world.access.clone(),
            live: world.live.clone(),
        });
        let current = Arc::new(Mutex::new(Some(original.source.clone())));
        let known = Arc::new(Mutex::new(HashSet::from([
            source.source.native_session_ref.clone(),
            original.source.native_session_ref.clone(),
        ])));
        let described = descriptor.clone();
        world.machine.handler.on(CommandKind::AgentStart, move |_| {
            Ok(CommandResult::AgentStart(
                orch_link_contract::commands::AgentStartResult {
                    reused: true,
                    harness_version: "1.0.0".into(),
                    descriptor: serde_json::from_value(described.clone()).map_err(|error| {
                        CommandError::new(ErrorCode::Internal, error.to_string())
                    })?,
                },
            ))
        });
        let prefix = NativeInputPrefix {
            harness_kind: "fake".into(),
            source: source.source.clone(),
            before_input_ref: turn.turn_id.to_string(),
            fingerprint: Hash::of(b"synthetic exact native checkpoint"),
            empty: true,
        };
        let expected = prefix.clone();
        let slot = current.clone();
        world
            .machine
            .handler
            .on(CommandKind::SessionPrefixInspect, move |command| {
                let Command::SessionPrefixInspect(body) = command else {
                    return Err(CommandError::new(ErrorCode::Unsupported, "wrong command"));
                };
                if body.source != expected.source
                    || body.before_input_ref != expected.before_input_ref
                    || slot.lock().unwrap_or_else(PoisonError::into_inner).as_ref()
                        != Some(&expected.source)
                {
                    return Err(CommandError::new(
                        ErrorCode::Busy,
                        "The synthetic native source does not match its recorded lane.",
                    ));
                }
                Ok(CommandResult::SessionPrefixInspect(expected.clone()))
            });
        let expected = prefix;
        let slot = current.clone();
        let registry = known.clone();
        world
            .machine
            .handler
            .on(CommandKind::SessionPrefixOpen, move |command| {
                let Command::SessionPrefixOpen(body) = command else {
                    return Err(CommandError::new(ErrorCode::Unsupported, "wrong command"));
                };
                let mut slot = slot.lock().unwrap_or_else(PoisonError::into_inner);
                if body.prefix != expected || slot.is_some() {
                    return Err(CommandError::new(
                        ErrorCode::Busy,
                        "The exact synthetic prefix needs its closed source lane.",
                    ));
                }
                let native = format!("synthetic-child-{}", body.session.segment_id);
                registry
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(native.clone());
                *slot = Some(NativePrefixSource {
                    thread_id: body.thread_id,
                    segment_id: body.session.segment_id,
                    native_session_ref: native.clone(),
                });
                Ok(CommandResult::SessionPrefixOpen(NativePrefixOpened {
                    prefix: body.prefix.clone(),
                    session: orch_agent_contract::session::SessionOpened {
                        native_session_ref: native,
                        resumed: false,
                        import_report: None,
                        instruction_sources: Vec::new(),
                        approval_reviewer: None,
                    },
                }))
            });
        let slot = current.clone();
        let registry = known;
        world
            .machine
            .handler
            .on(CommandKind::SessionContinuationResume, move |command| {
                let Command::SessionContinuationResume(body) = command else {
                    return Err(CommandError::new(ErrorCode::Unsupported, "wrong command"));
                };
                let mut slot = slot.lock().unwrap_or_else(PoisonError::into_inner);
                if !registry
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .contains(&body.source.native_session_ref)
                    || slot.as_ref().is_some_and(|held| {
                        held != &body.source
                            && body.replace_segment_id != Some(held.segment_id)
                            && body.replace_retained_source.as_ref() != Some(held)
                    })
                {
                    return Err(CommandError::new(
                        ErrorCode::Busy,
                        "A different synthetic native lane is retained.",
                    ));
                }
                *slot = Some(NativePrefixSource {
                    thread_id: body.thread_id,
                    segment_id: body.session.segment_id,
                    native_session_ref: body.source.native_session_ref.clone(),
                });
                Ok(CommandResult::SessionContinuationResume(
                    SessionOpenResult {
                        native_session_ref: body.source.native_session_ref.clone(),
                        resumed: true,
                        import_report: None,
                        instruction_sources: Vec::new(),
                        approval_reviewer: None,
                    },
                ))
            });
        let slot = current.clone();
        world
            .machine
            .handler
            .on(CommandKind::SessionContinuationClose, move |command| {
                let Command::SessionContinuationClose(body) = command else {
                    return Err(CommandError::new(ErrorCode::Unsupported, "wrong command"));
                };
                let mut slot = slot.lock().unwrap_or_else(PoisonError::into_inner);
                if slot.as_ref().is_some_and(|held| held != &body.source) {
                    return Err(CommandError::new(
                        ErrorCode::Busy,
                        "The cleanup names another native generation.",
                    ));
                }
                *slot = None;
                Ok(CommandResult::SessionContinuationClose(Empty {}))
            });
        let submitted = engine
            .submit_input(
                &Caller::session(world.ada),
                thread,
                common::queued("Unrelated retained queued input"),
            )
            .await?;
        let latest = {
            let mut conn = world.pool.acquire().await?;
            threads::latest_turn(&mut conn, thread)
                .await?
                .ok_or("latest turn absent")?
        };
        engine
            .interrupt(&Caller::session(world.ada), thread, latest.id)
            .await?;
        let pending = {
            let mut conn = world.pool.acquire().await?;
            threads::input(&mut conn, submitted.input.id)
                .await?
                .ok_or("pending input absent")?
        };
        assert!(
            pending.queue_held,
            "the genuine Stop must hold the existing old queue"
        );
        let file_work = [
            CommandKind::FsMaterialize,
            CommandKind::FsRestore,
            CommandKind::FsRecord,
        ]
        .map(|kind| world.began(kind));
        let frozen_turns=sqlx::query_scalar("SELECT jsonb_build_object('id',id,'input',input_id,'usage',usage,'completed_at',completed_at,'state',state)
            FROM turns WHERE thread_id=$1 ORDER BY ordinal").bind(thread).fetch_all(world.pool.pool()).await?;
        let leases = sqlx::query_scalar("SELECT count(*) FROM leases WHERE thread_id=$1")
            .bind(thread)
            .fetch_one(world.pool.pool())
            .await?;
        let file = tempfile::tempdir()?;
        tokio::fs::write(
            file.path().join("workspace.bin"),
            b"retained\0workspace\xffbytes",
        )
        .await?;
        Ok(Self {
            world,
            engine,
            thread,
            source,
            original,
            source_turn: turn.turn_id,
            source_input: input,
            current,
            file,
            pending,
            file_work,
            frozen_turns,
            leases,
        })
    }

    async fn admission(
        &self,
        source: Id,
        client: &str,
    ) -> Result<orch_api_contract::threads::ThreadContinuationAccepted, crate::continuations::Error>
    {
        let mut tx = self.world.pool.begin().await?;
        let row = threads::thread(&mut tx, self.thread)
            .await?
            .ok_or(crate::continuations::Error::NotFound)?;
        let principal = Principal::load(
            &mut tx,
            Caller::session(self.world.ada),
            self.world.clock.time(),
        )
        .await
        .map_err(EngineError::from)?;
        self.engine
            .create_prompt_version_guarded(
                tx,
                principal,
                self.thread,
                self.source_input,
                ThreadPromptVersionCreateBody {
                    client_id: client.into(),
                    source_continuation_id: source,
                    content: vec![ContentPart::Text {
                        text: "Revised retained words".into(),
                    }],
                    expected_active_continuation_id: row.active_continuation_id,
                    expected_execution_revision: row.execution_revision,
                    expected_queue_revision: row.queue_revision,
                    active_turn: None,
                },
            )
            .await
    }

    async fn claim(&self, id: Id) -> Proof<(Job, Recipe)> {
        let mut tx = self.world.pool.begin().await?;
        let runner = new_id();
        let lease = chrono::Duration::from_std(self.engine.config().hosting.job_lease)?;
        assert!(
            jobs::claim(
                &mut tx,
                id,
                runner,
                self.world.clock.now() + lease,
                self.world.clock.now()
            )
            .await?
        );
        let row = jobs::job(&mut tx, id).await?.ok_or("job missing")?;
        let recipe = serde_json::from_value(row.plan["recipe"].clone())?;
        tx.commit().await?;
        Ok((Job::claimed(self.engine.clone(), id, runner), recipe))
    }

    /// Reconstruct the formerly accepted stale-source receipt, not a bypass of current admission.
    async fn formerly_accepted(&self) -> Proof<(Job, Recipe)> {
        let mut tx = self.world.pool.begin().await?;
        let thread = threads::lock(&mut tx, self.thread)
            .await?
            .ok_or("thread absent")?;
        let source = threads::segment(&mut tx, self.source.source.segment_id)
            .await?
            .ok_or("source absent")?;
        let turn = threads::turn(&mut tx, self.source_turn)
            .await?
            .ok_or("source turn absent")?;
        let position: Seq = sqlx::query_scalar(
            "SELECT seq_created FROM items WHERE turn_id=$1 AND kind='user_message'
            AND payload->>'input_id'=$2 ORDER BY seq_created LIMIT 1",
        )
        .bind(turn.id)
        .bind(self.source_input.to_string())
        .fetch_one(&mut *tx)
        .await?;
        let options = crate::fork_plan::applied_options(&mut tx, self.thread, None)
            .await
            .map_err(|error| error.to_string())?;
        let now = self.world.clock.now();
        let operation = new_id();
        let target = new_id();
        let input = new_id();
        let segment = new_id();
        let seq = orch_db::sequences::next(&mut tx, self.thread)
            .await?
            .ok_or("sequence absent")?;
        let request = Request {
            kind: Kind::Edit,
            source_continuation_id: self.source.continuation_id,
            before_input_id: Some(self.source_input),
            target_continuation_id: None,
            expected: Expected {
                active_continuation_id: thread.active_continuation_id,
                execution_revision: thread.execution_revision,
                queue_revision: thread.queue_revision,
            },
            active_turn: None,
            content: Some(vec![ContentPart::Text {
                text: "Kept revised words".into(),
            }]),
        };
        rows::insert(
            &mut tx,
            &rows::NewContinuation {
                id: target,
                thread: self.thread,
                parent: self.source.continuation_id,
                before_input: self.source_input,
                actor: self.world.ada,
                at: now,
                seq,
                queue: rows::QueueRow {
                    held: thread.queue_held,
                    held_by: thread.queue_held_by,
                    held_at: thread.queue_held_at,
                    revision: 1,
                },
            },
        )
        .await?;
        rows::freeze_prefix(
            &mut tx,
            self.thread,
            self.source.continuation_id,
            target,
            turn.ordinal,
            position,
            seq,
        )
        .await?;
        threads::insert_segment_in(
            &mut tx,
            &threads::NewSegment {
                id: segment,
                thread_id: self.thread,
                agent_id: source.agent_id,
                provider_id: source.provider_id,
                environment_id: source.environment_id,
                env_generation: source.env_generation,
                arrangement: &source.arrangement,
                executor: source.executor.as_deref(),
                tool_set: source.tool_set.as_deref(),
                credential_path: &source.credential_path,
                native_session_ref: None,
                started_by: Some(self.world.ada),
                started_at: now,
            },
            Some(target),
            seq,
        )
        .await?;
        let content = serde_json::to_value(&request.content)?;
        threads::insert_input_in(
            &mut tx,
            &threads::NewInput {
                plan: false,
                id: input,
                thread_id: self.thread,
                author_id: Some(self.world.ada),
                submission_authority: Some(
                    &serde_json::json!({"credentials":[{"type":"session","as_admin":false}]}),
                ),
                run_as: None,
                run_as_authority: None,
                source: "user",
                content: &content,
                delivery: "queue",
                state: "pending",
                turn_id: None,
                client_id: None,
                limit_bypass: None,
                place: threads::Place::End,
                at: now,
            },
            Some(target),
            seq,
        )
        .await?
        .ok_or("revised input absent")?;
        let recipe = Recipe {
            operation_id: operation,
            thread_id: self.thread,
            actor_id: self.world.ada,
            authorization_token: None,
            authorization_scope: None,
            request_hash: recipe::request_hash(&request)?,
            request,
            needs_prefix: true,
            source_continuation_id: self.source.continuation_id,
            target_continuation_id: target,
            input_id: Some(input),
            segment_id: segment,
            original_lane: Some(self.original.clone()),
            source_lane: self.source.clone(),
            source_turn_id: self.source_turn,
            wait_minutes: options.wait_minutes,
            commands: recipe::Commands::of(operation),
            accepted_at: Time::new(now),
            deadline: Time::new(
                now + chrono::Duration::minutes(options.wait_minutes)
                    + chrono::Duration::seconds(options.verify_timeout_secs),
            ),
            prefix: None,
            opened: None,
            activation_seq: None,
        };
        let job = jobs::insert(
            &mut tx,
            &jobs::NewJob {
                id: operation,
                kind: KIND,
                node_id: Some(self.thread),
                environment_id: Some(self.source.environment_id),
                provider_id: Some(self.source.provider_id),
                plan: &json!({"recipe":recipe}),
                progress: &serde_json::to_value(super::super::runner::initial_progress(PHASES))?,
                created_by: Some(self.world.ada),
            },
            now,
        )
        .await?;
        rows::create_operation(
            &mut tx,
            &rows::OperationRow {
                id: operation,
                job_id: Some(operation),
                job_snapshot: serde_json::to_value(crate::locks::job_ref(&job))?,
                thread_id: self.thread,
                actor_id: self.world.ada,
                client_id: format!("reconstructed-{operation}"),
                body_hash: recipe.request_hash.as_bytes().to_vec(),
                kind: "edit".into(),
                source_continuation_id: self.source.continuation_id,
                target_continuation_id: target,
                input_id: Some(input),
                segment_id: segment,
                expected_active_continuation_id: thread.active_continuation_id,
                expected_execution_revision: thread.execution_revision,
                expected_queue_revision: thread.queue_revision,
                status: "preparing".into(),
                activation_seq: None,
                created_at: now,
            },
        )
        .await?;
        tx.commit().await?;
        self.claim(operation).await
    }

    async fn unchanged_workspace(&self) -> Proof {
        assert_eq!(
            tokio::fs::read(self.file.path().join("workspace.bin")).await?,
            b"retained\0workspace\xffbytes"
        );
        for (kind, before) in [
            CommandKind::FsMaterialize,
            CommandKind::FsRestore,
            CommandKind::FsRecord,
        ]
        .into_iter()
        .zip(self.file_work)
        {
            assert_eq!(
                self.world.began(kind),
                before,
                "native prompt preparation cannot undo or recapture files through {kind}"
            );
        }
        let mut conn = self.world.pool.acquire().await?;
        assert_eq!(
            threads::input(&mut conn, self.pending.id).await?,
            Some(self.pending.clone()),
            "the old Stop/queued input cannot migrate or disappear"
        );
        let turns:Vec<Json>=sqlx::query_scalar("SELECT jsonb_build_object('id',id,'input',input_id,'usage',usage,'completed_at',completed_at,'state',state)
            FROM turns WHERE thread_id=$1 ORDER BY ordinal").bind(self.thread).fetch_all(&mut *conn).await?;
        assert_eq!(
            turns, self.frozen_turns,
            "preparation cannot rewrite or rebill existing native turns"
        );
        let leases: i64 = sqlx::query_scalar("SELECT count(*) FROM leases WHERE thread_id=$1")
            .bind(self.thread)
            .fetch_one(&mut *conn)
            .await?;
        assert_eq!(
            leases, self.leases,
            "native history preparation cannot reserve a paid turn"
        );
        Ok(())
    }

    async fn shutdown(self) {
        self.engine.stop();
        drop(self.engine);
        self.world.shutdown().await;
    }
}

#[tokio::test]
async fn a_stale_inactive_source_refuses_before_any_valid_selected_lane_is_closed() -> Proof {
    let fixture = Fixture::new(true).await?;
    assert_eq!(
        (
            fixture.source.environment_generation,
            fixture.original.environment_generation
        ),
        (1, 2)
    );
    let mut conn = fixture.world.pool.acquire().await?;
    let principal = Principal::load(
        &mut conn,
        Caller::session(fixture.world.ada),
        fixture.world.clock.time(),
    )
    .await
    .map_err(EngineError::from)?;
    let stale = crate::continuations::projection::summaries(
        &fixture.engine,
        &mut conn,
        &principal,
        fixture.thread,
        fixture.source.continuation_id,
        [fixture.source_input],
    )
    .await?;
    assert_eq!(stale.len(), 1);
    assert!(!stale[0].edit.available);
    assert_eq!(
        stale[0].edit.reason,
        Some(orch_api_contract::threads::PromptEditReason::NativeSourceUnavailable)
    );
    let current_input: Id = sqlx::query_scalar(
        "SELECT input_id FROM turns WHERE segment_id=$1 ORDER BY ordinal DESC LIMIT 1",
    )
    .bind(fixture.original.source.segment_id)
    .fetch_one(&mut *conn)
    .await?;
    let current = crate::continuations::projection::summaries(
        &fixture.engine,
        &mut conn,
        &principal,
        fixture.thread,
        fixture.original.continuation_id,
        [current_input],
    )
    .await?;
    assert_eq!(current.len(), 1);
    assert!(current[0].edit.available);
    drop(conn);
    // Read-fixture version lineage over the two actual admitted prompts. This is
    // an availability projection test, not a fabricated native preparation proof.
    let mut version_tx = fixture.world.pool.begin().await?;
    let seq = orch_db::sequences::next(&mut version_tx, fixture.thread)
        .await?
        .ok_or("version sequence missing")?;
    sqlx::query(
        "INSERT INTO thread_prompt_groups(id,thread_id,total,seq_updated) VALUES($1,$2,2,$3)",
    )
    .bind(fixture.source_input)
    .bind(fixture.thread)
    .bind(seq)
    .execute(&mut *version_tx)
    .await?;
    sqlx::query(
        "INSERT INTO thread_prompt_versions(input_id,group_id,continuation_id,ordinal)
      VALUES($1,$1,$2,1),($3,$1,$4,2)",
    )
    .bind(fixture.source_input)
    .bind(fixture.source.continuation_id)
    .bind(current_input)
    .bind(fixture.original.continuation_id)
    .execute(&mut *version_tx)
    .await?;
    let neighbors = crate::continuations::projection::versions(
        &fixture.engine,
        &mut version_tx,
        &principal,
        fixture.thread,
        current_input,
        &orch_api_contract::threads::ThreadPromptVersionListQuery {
            continuation_id: Some(fixture.original.continuation_id),
            projection_revision: Some(1),
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(neighbors.rows.len(), 2);
    let stale_neighbor = neighbors
        .rows
        .iter()
        .find(|row| row.input_id == fixture.source_input)
        .ok_or("stale neighbor absent")?;
    assert_eq!(
        stale_neighbor.edit.reason,
        Some(orch_api_contract::threads::PromptEditReason::NativeSourceUnavailable)
    );
    assert!(
        !stale_neighbor.edit.available,
        "a valid requesting view cannot lend source eligibility to another neighbor"
    );
    assert!(
        neighbors
            .rows
            .iter()
            .any(|row| row.input_id == current_input && row.edit.available)
    );
    version_tx.commit().await?;
    let before: (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM jobs),(SELECT count(*) FROM thread_inputs),
        (SELECT count(*) FROM thread_continuations)",
    )
    .fetch_one(fixture.world.pool.pool())
    .await?;
    let refused = fixture
        .admission(fixture.source.continuation_id, "stale-source-edit")
        .await;
    assert!(
        matches!(refused,Err(crate::continuations::Error::Unsupported(detail)) if detail.contains("older environment generation"))
    );
    let after: (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM jobs),(SELECT count(*) FROM thread_inputs),
        (SELECT count(*) FROM thread_continuations)",
    )
    .fetch_one(fixture.world.pool.pool())
    .await?;
    assert_eq!(after, before);
    assert_eq!(
        fixture
            .current
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref(),
        Some(&fixture.original.source)
    );
    assert_eq!(
        fixture.world.began(CommandKind::SessionContinuationClose),
        0
    );
    assert_eq!(fixture.world.began(CommandKind::SessionPrefixOpen), 0);
    fixture.unchanged_workspace().await?;
    fixture.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn owned_undo_restores_the_valid_original_generation_independently_of_its_stale_candidate()
-> Proof {
    let fixture = Fixture::new(true).await?;
    let (job, recipe) = fixture.formerly_accepted().await?;
    let input = recipe.input_id.ok_or("revised input absent")?;
    let original = {
        let mut conn = fixture.world.pool.acquire().await?;
        threads::thread(&mut conn, fixture.thread)
            .await?
            .ok_or("thread absent")?
    };
    // This is the former first-close crash checkpoint. The command really closes its exact lane.
    job.remember(&json!({"native_changed":true})).await?;
    phase(
        Continuing::close(
            &job,
            &recipe,
            &fixture.original,
            "close_active",
            recipe.commands.close_active,
        )
        .await,
    )?;
    assert!(
        fixture
            .current
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_none()
    );
    fixture
        .engine
        .end_job(
            &job,
            Some(&Continuing),
            failed(
                "continuation.source_changed",
                "The inactive source is stale.",
            ),
        )
        .await?;
    let ended = job.row().await?;
    assert_eq!(ended.state, "failed");
    assert_eq!(
        fixture
            .current
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref(),
        Some(&fixture.original.source)
    );
    let mut conn = fixture.world.pool.acquire().await?;
    let after = threads::thread(&mut conn, fixture.thread)
        .await?
        .ok_or("thread absent")?;
    assert_eq!(
        (after.active_continuation_id, after.execution_revision),
        (original.active_continuation_id, original.execution_revision)
    );
    assert_eq!(
        (after.queue_held, after.queue_revision),
        (original.queue_held, original.queue_revision)
    );
    let kept = threads::input(&mut conn, input)
        .await?
        .ok_or("revised input was discarded")?;
    assert_eq!(
        (kept.continuation_id, kept.state.as_str()),
        (recipe.target_continuation_id, "pending")
    );
    assert_eq!(fixture.world.began(CommandKind::SessionPrefixOpen), 0);
    assert_eq!(
        fixture.world.began(CommandKind::SessionContinuationResume),
        1
    );
    drop(conn);
    fixture.unchanged_workspace().await?;
    drop(job);
    fixture.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn a_formerly_accepted_stale_candidate_is_revalidated_before_the_first_close() -> Proof {
    let fixture = Fixture::new(true).await?;
    let (job, recipe) = fixture.formerly_accepted().await?;
    let result = Continuing::prepare(&job, recipe).await;
    assert!(
        matches!(&result,Err(Stop::Failed(failure)) if failure.code=="continuation.source_changed")
    );
    let Err(reason) = result else {
        return Err("the stale accepted source unexpectedly prepared".into());
    };
    fixture
        .engine
        .end_job(&job, Some(&Continuing), reason)
        .await?;
    assert_eq!(job.row().await?.state, "failed");
    assert_eq!(
        fixture
            .current
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref(),
        Some(&fixture.original.source)
    );
    assert_eq!(
        fixture.world.began(CommandKind::SessionContinuationClose),
        0
    );
    assert_eq!(fixture.world.began(CommandKind::SessionPrefixOpen), 0);
    fixture.unchanged_workspace().await?;
    drop(job);
    fixture.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn cancellation_after_durable_native_success_keeps_one_child_and_the_same_revised_input_on_retry()
-> Proof {
    let fixture = Fixture::new(false).await?;
    let accepted = fixture
        .admission(
            fixture.source.continuation_id,
            "native-success-before-metadata",
        )
        .await?;
    let (job, recipe) = fixture.claim(accepted.operation_id).await?;
    let revised = recipe.input_id.ok_or("revised input absent")?;
    let revised_before = {
        let mut conn = fixture.world.pool.acquire().await?;
        threads::input(&mut conn, revised)
            .await?
            .ok_or("revised input absent")?
    };
    let (reached, wait) = oneshot::channel();
    let (resume, paused) = oneshot::channel();
    let preparing = job.clone();
    let preparing_recipe = recipe.clone();
    let running = fixture.engine.clone();
    let work = tokio::spawn(AFTER_CREATION.scope(
        Mutex::new(Some(Boundary {
            reached,
            resume: paused,
        })),
        async move {
            assert_eq!(preparing_recipe.operation_id, preparing.id());
            running.drive_job(&preparing).await
        },
    ));
    tokio::time::timeout(common::PATIENCE, wait).await??;
    let path = fixture
        .world
        .machine
        .data_dir()
        .join("journal")
        .join(format!("{}.json", recipe.commands.open_prefix));
    let entry: orch_link::journal::Entry = serde_json::from_slice(&tokio::fs::read(path).await?)?;
    let result = entry
        .result()
        .ok_or("native success must be durable before its metadata checkpoint")?;
    let opened: NativePrefixOpened =
        serde_json::from_value(result.outcome.clone().map_err(|error| error.detail)?)?;
    let durable_prefix: NativeInputPrefix =
        serde_json::from_value(job.row().await?.plan["recipe"]["prefix"].clone())?;
    assert_eq!(opened.prefix, durable_prefix);
    assert_eq!(entry.cmd.id, recipe.commands.open_prefix);
    assert_eq!(
        fixture
            .world
            .machine
            .handler
            .runs_of(recipe.commands.open_prefix),
        1
    );
    assert!(
        fixture
            .engine
            .cancel_job(job.id(), fixture.world.ada)
            .await?
    );
    resume
        .send(())
        .map_err(|()| "native-result publication latch ended early")?;
    tokio::time::timeout(common::PATIENCE, work).await???;
    assert_eq!(job.row().await?.state, "cancelled");
    let failed = {
        let mut conn = fixture.world.pool.acquire().await?;
        rows::continuation(&mut conn, fixture.thread, accepted.continuation_id)
            .await?
            .ok_or("failed version absent")?
    };
    assert_eq!(failed.status, "failed");
    assert_eq!(
        serde_json::from_value::<NativePrefixOpened>(
            failed
                .native_creation_result
                .ok_or("successful native receipt lost")?
        )?,
        opened
    );
    assert_eq!(
        fixture
            .current
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref(),
        Some(&fixture.original.source)
    );
    let row = {
        let mut conn = fixture.world.pool.acquire().await?;
        threads::thread(&mut conn, fixture.thread)
            .await?
            .ok_or("thread absent")?
    };
    let mut tx = fixture.world.pool.begin().await?;
    let principal = Principal::load(
        &mut tx,
        Caller::session(fixture.world.ada),
        fixture.world.clock.time(),
    )
    .await
    .map_err(EngineError::from)?;
    let retried = fixture
        .engine
        .activate_continuation_guarded(
            tx,
            principal,
            fixture.thread,
            accepted.continuation_id,
            ThreadContinuationActivateBody {
                client_id: "resume-known-native-child".into(),
                expected_active_continuation_id: row.active_continuation_id,
                expected_execution_revision: row.execution_revision,
                expected_queue_revision: row.queue_revision,
                active_turn: None,
                content: None,
            },
        )
        .await?;
    assert_eq!(
        retried.input_id, None,
        "retrying a retained version does not author another copy of its words"
    );
    let (retry, retry_recipe) = fixture.claim(retried.operation_id).await?;
    assert!(!retry_recipe.needs_prefix);
    assert_eq!(
        retry_recipe.source_lane.source.native_session_ref,
        opened.session.native_session_ref
    );
    tokio::time::timeout(common::PATIENCE, fixture.engine.drive_job(&retry)).await??;
    assert_eq!(retry.row().await?.state, "succeeded");
    assert_eq!(fixture.world.began(CommandKind::SessionPrefixOpen), 1);
    assert_eq!(
        fixture
            .world
            .machine
            .handler
            .runs_of(recipe.commands.open_prefix),
        1
    );
    let inputs: Vec<(Id, Id, String)> = sqlx::query_as(
        "SELECT id,continuation_id,state FROM thread_inputs WHERE continuation_id=$1",
    )
    .bind(accepted.continuation_id)
    .fetch_all(fixture.world.pool.pool())
    .await?;
    assert_eq!(
        inputs,
        vec![(revised, accepted.continuation_id, "pending".into())]
    );
    let retained = {
        let mut conn = fixture.world.pool.acquire().await?;
        let input = threads::input(&mut conn, revised)
            .await?
            .ok_or("revised words disappeared")?;
        assert_eq!(
            (
                input.content,
                input.author_id,
                input.client_id,
                input.limit_bypass
            ),
            (
                revised_before.content,
                revised_before.author_id,
                revised_before.client_id,
                revised_before.limit_bypass
            )
        );
        rows::continuation(&mut conn, fixture.thread, accepted.continuation_id)
            .await?
            .ok_or("retained version disappeared")?
    };
    let creation: NativeCreation = serde_json::from_value(
        retained
            .native_creation
            .ok_or("original creation identity disappeared")?,
    )?;
    assert_eq!(
        (
            creation.operation_id,
            creation.command_id,
            creation.segment_id
        ),
        (job.id(), recipe.commands.open_prefix, recipe.segment_id)
    );
    assert_eq!(
        fixture
            .current
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(|lane| lane.native_session_ref.clone()),
        Some(opened.session.native_session_ref)
    );
    assert_eq!(
        fixture.world.turn_starts(fixture.thread).len(),
        1,
        "neither cancellation nor read/selection replay starts an autonomous turn"
    );
    fixture.unchanged_workspace().await?;
    drop(job);
    drop(retry);
    fixture.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn an_actual_recovered_started_journal_refusal_never_authorizes_another_prefix_creation()
-> Proof {
    let mut fixture = Fixture::new(false).await?;
    let accepted = fixture
        .admission(
            fixture.source.continuation_id,
            "unknown-original-native-result",
        )
        .await?;
    let (job, mut recipe) = fixture.claim(accepted.operation_id).await?;
    let inspect = SessionPrefixInspect {
        thread_id: fixture.thread,
        provider_id: fixture.source.provider_id,
        agent_id: fixture.source.agent_id,
        source: fixture.source.source.clone(),
        before_input_ref: fixture.source_turn.to_string(),
    };
    let inspect = phase(
        Continuing::saved(
            &job,
            "preinspect",
            recipe.commands.preinspect,
            fixture.source.machine_id,
            &inspect,
            Some(recipe.deadline),
        )
        .await,
    )?;
    let prefix: NativeInputPrefix = serde_json::from_value(phase(job.command(inspect).await)?)?;
    recipe.prefix = Some(prefix.clone());
    // Reconstruct the durable pre-issue checkpoint through the owned command supplier.
    // No native process crash is injected; the real journal recovery below supplies its ambiguity.
    let mut tx = fixture.world.pool.begin().await?;
    job.lock_owned(&mut tx).await?.ok_or("owned claim absent")?;
    sqlx::query("UPDATE thread_continuations SET native_prefix=$2,native_source=$3 WHERE id=$1")
        .bind(recipe.target_continuation_id)
        .bind(serde_json::to_value(&prefix)?)
        .bind(serde_json::to_value(&prefix.source)?)
        .execute(&mut *tx)
        .await?;
    jobs::merge_plan(
        &mut tx,
        job.id(),
        &json!({"recipe":recipe,"native_changed":true}),
        job.now(),
    )
    .await?;
    tx.commit().await?;
    let body = phase(
        Continuing::body(
            &job,
            &recipe,
            &recipe.source_lane,
            recipe.segment_id,
            SessionMode::Fork,
            false,
        )
        .await,
    )?;
    phase(
        Continuing::close(
            &job,
            &recipe,
            &fixture.original,
            "close_active",
            recipe.commands.close_active,
        )
        .await,
    )?;
    let creation = SessionPrefixOpen {
        thread_id: fixture.thread,
        session: Box::new(body),
        prefix,
    };
    let outgoing = phase(
        Continuing::saved(
            &job,
            "open_prefix",
            recipe.commands.open_prefix,
            fixture.source.machine_id,
            &creation,
            Some(recipe.deadline),
        )
        .await,
    )?;
    let command = orch_link_contract::commands::Cmd {
        id: outgoing.id,
        kind: outgoing.kind.as_str().into(),
        key: Some(fixture.thread.to_string()),
        deadline: recipe.deadline,
        body: outgoing.body.clone(),
    };
    let directory = tempfile::tempdir()?;
    let mut journal = orch_link::journal::Journal::open(
        directory.path(),
        Duration::from_secs(24 * 3600),
        fixture.world.clock.time(),
    )?;
    journal.accept(&command, fixture.world.clock.time())?;
    journal.started(command.id)?;
    drop(journal);
    let journal = orch_link::journal::Journal::open(
        directory.path(),
        Duration::from_secs(24 * 3600),
        fixture.world.clock.time(),
    )?;
    let server = orch_link::commands::CommandServer::new(
        journal,
        fixture.world.clock.clone(),
        orch_link::commands::ServerOptions {
            roles: vec![orch_link_contract::session::Role::Agent],
            connected: true,
        },
    );
    let handler = Arc::new(orch_testkit::doubles::machine::ScriptedHandler::default());
    server.serve(handler.clone());
    let runner: Id = serde_json::from_value(job.row().await?.plan["runner"]["id"].clone())?;
    let recovered = Arc::new(RecoveredCommands {
        inner: fixture.engine.inner.commands.clone(),
        server: server.clone(),
        expected: outgoing.clone(),
        command: command.clone(),
        issued: Mutex::new(None),
    });
    fixture.engine.stop();
    drop(job);
    fixture.engine = Engine::new(EngineParts {
        db: fixture.world.pool.clone(),
        clock: fixture.world.clock.clone(),
        config: common::config(),
        commands: recovered.clone(),
        access: fixture.world.access.clone(),
        live: fixture.world.live.clone(),
    });
    let job = Job::claimed(fixture.engine.clone(), recipe.operation_id, runner);
    // The real issued engine command receives the actual recovered Started failure.
    let failed = tokio::time::timeout(common::PATIENCE, job.command(outgoing.clone())).await?;
    let Err(reason) = failed else {
        return Err("a recovered Started command unexpectedly succeeded".into());
    };
    let result = server
        .entry(command.id)
        .and_then(|entry| entry.result().cloned())
        .ok_or("recovered command has no durable terminal result")?;
    let Err(refusal) = result.outcome else {
        return Err(
            "a recovered Started command must not manufacture successful native work".into(),
        );
    };
    assert!(refusal.retryable);
    assert!(
        refusal
            .detail
            .contains("stopped while the command was running")
    );
    assert!(
        handler.executions().is_empty(),
        "the actual production journal refuses instead of invoking prefix creation again"
    );
    assert_eq!(
        recovered
            .issued
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref(),
        Some(&outgoing)
    );
    let observed = fixture
        .engine
        .inner
        .commands
        .status(command.id)
        .await?
        .ok_or("the engine lost its recovered command")?;
    assert_eq!(
        observed.outcome,
        Some(crate::commands::Outcome::Failed(refusal.clone()))
    );
    assert!(
        matches!(&reason,Stop::Failed(failure) if failure.code==refusal.code && failure.message==refusal.detail)
    );
    server.on_cmd(command).await;
    assert!(
        handler.executions().is_empty(),
        "replaying the interrupted durable outcome also never executes it"
    );
    fixture
        .engine
        .end_job(&job, Some(&Continuing), reason)
        .await?;
    assert_eq!(job.row().await?.state, "failed");
    assert_eq!(
        fixture
            .current
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref(),
        Some(&fixture.original.source)
    );
    let mut tx = fixture.world.pool.begin().await?;
    let thread = threads::thread(&mut tx, fixture.thread)
        .await?
        .ok_or("thread absent")?;
    let principal = Principal::load(
        &mut tx,
        Caller::session(fixture.world.ada),
        fixture.world.clock.time(),
    )
    .await
    .map_err(EngineError::from)?;
    let retry = fixture
        .engine
        .activate_continuation_guarded(
            tx,
            principal,
            fixture.thread,
            accepted.continuation_id,
            ThreadContinuationActivateBody {
                client_id: "do-not-repeat-unknown-creation".into(),
                expected_active_continuation_id: thread.active_continuation_id,
                expected_execution_revision: thread.execution_revision,
                expected_queue_revision: thread.queue_revision,
                active_turn: None,
                content: None,
            },
        )
        .await;
    assert!(
        matches!(retry,Err(crate::continuations::Error::Conflict(detail)) if detail.contains("unconfirmed"))
    );
    assert_eq!(fixture.world.began(CommandKind::SessionPrefixOpen), 0);
    let kept = {
        let mut conn = fixture.world.pool.acquire().await?;
        threads::input(&mut conn, recipe.input_id.ok_or("revised input absent")?)
            .await?
            .ok_or("revised input lost")?
    };
    assert_eq!(
        (kept.continuation_id, kept.state.as_str()),
        (accepted.continuation_id, "pending")
    );
    fixture.unchanged_workspace().await?;
    drop(job);
    drop(recovered);
    fixture.shutdown().await;
    Ok(())
}
