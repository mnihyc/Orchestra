//! Actual lifecycle settlement over the owned job and synthetic native program.
//! The journal/engine/DB are real; this is not vendor model-history proof.

use super::super::resume_trace::Trace;
use super::*;
use orch_db::queries::{hierarchy, lifecycle as controls};
use orch_link_contract::continuations::SessionCloseFenced;
use std::collections::BTreeMap;

#[derive(Clone, Copy)]
enum Ending {
    Archive,
    RemoveBinding,
    DisableHome,
}

fn strict_closer(fixture: &Fixture) {
    let slot = fixture.current.clone();
    let thread = fixture.thread;
    fixture
        .world
        .machine
        .handler
        .on(CommandKind::SessionCloseFenced, move |command| {
            let Command::SessionCloseFenced(body) = command else {
                return Err(CommandError::new(
                    ErrorCode::Unsupported,
                    "another cleanup command",
                ));
            };
            if body.thread_id != thread {
                return Err(CommandError::new(
                    ErrorCode::NotFound,
                    "another cleanup thread",
                ));
            }
            let mut slot = slot.lock().unwrap_or_else(PoisonError::into_inner);
            if slot
                .as_ref()
                .is_some_and(|held| held.segment_id != body.segment_id)
            {
                // Same exact producer contract as Process::check_segment: the other slot survives.
                let mut error =
                    CommandError::new(ErrorCode::Busy, "another native segment is open");
                error.code = "session.segment_changed".into();
                return Err(error);
            }
            *slot = None;
            Ok(CommandResult::SessionCloseFenced(Empty {}))
        });
}

async fn end(fixture: &Fixture, ending: Ending) -> Proof {
    let mut tx = fixture.world.pool.begin().await?;
    orch_db::queries::transaction_guard::lock_authority(&mut tx).await?;
    let now = fixture.world.clock.time();
    match ending {
        Ending::Archive => {
            fixture
                .engine
                .lock_archive_in(&mut tx, fixture.thread)
                .await?;
            assert!(
                hierarchy::archive(&mut tx, fixture.thread, fixture.world.ada, now.get()).await?
            );
            fixture
                .engine
                .archive_in(&mut tx, fixture.thread, fixture.world.ada, now)
                .await?;
        }
        Ending::RemoveBinding => {
            fixture
                .engine
                .remove_bindings_in(&mut tx, fixture.world.environment, fixture.world.ada, now)
                .await?;
        }
        Ending::DisableHome => {
            let origin = orch_db::audit::current_origin();
            fixture
                .engine
                .end_home_resources_in(
                    &mut tx,
                    fixture.world.root,
                    None,
                    fixture.world.ada,
                    now,
                    &origin,
                )
                .await?;
        }
    }
    tx.commit().await?;
    Ok(())
}

async fn facts(fixture: &Fixture) -> Proof<Json> {
    Ok(sqlx::query_scalar("SELECT jsonb_build_object(
        'turns',(SELECT jsonb_agg(to_jsonb(t) ORDER BY t.ordinal) FROM turns t WHERE t.thread_id=$1),
        'ledger',(SELECT jsonb_agg(to_jsonb(u) ORDER BY u.id) FROM usage_events u WHERE u.thread_id=$1),
        'leases',(SELECT jsonb_agg(to_jsonb(l) ORDER BY l.id) FROM leases l WHERE l.thread_id=$1))")
        .bind(fixture.thread).fetch_one(fixture.world.pool.pool()).await?)
}

async fn drain_controls(fixture: &Fixture) -> Proof {
    // This fixture manually claims jobs instead of starting the supervisor. Drive
    // its existing reducing-control supplier, always with the same journal IDs.
    tokio::time::timeout(common::PATIENCE, async {
        loop {
            fixture.engine.deliver_lifecycle_controls().await?;
            let closing = {
                let mut conn = fixture.world.pool.acquire().await?;
                controls::closing(&mut conn, fixture.thread).await?
            };
            if !closing {
                return Ok::<(), EngineError>(());
            }
            tokio::time::sleep(common::config().sweep_every).await;
        }
    })
    .await??;
    Ok(())
}

struct ResumeSnapshot {
    attempts: Json,
    issued: Json,
    journals: BTreeMap<Id, Vec<u8>>,
    executions: BTreeMap<Id, usize>,
}

async fn resume_snapshot(
    fixture: &Fixture,
    job: &Job,
    recipe: &Recipe,
    trace: &Trace,
    source_was_resumed: bool,
) -> Proof<ResumeSnapshot> {
    let attempts = trace.commands();
    assert_eq!(
        attempts.len(),
        usize::from(source_was_resumed),
        "the ending barrier has exactly its phase's native preparation resumes"
    );
    let mut journals = BTreeMap::new();
    let mut executions = BTreeMap::new();
    for outgoing in &attempts {
        assert_eq!(
            (outgoing.id, outgoing.machine, outgoing.kind),
            (
                recipe.commands.resume_source,
                recipe.source_lane.machine_id,
                CommandKind::SessionContinuationResume
            )
        );
        assert_eq!(outgoing.key, SerialKey::Thread(fixture.thread));
        assert_eq!(outgoing.deadline, Some(recipe.deadline));
        let body: SessionContinuationResume = serde_json::from_value(outgoing.body.clone())?;
        assert!(
            body.source == recipe.source_lane.source,
            "the prior resume names the exact captured source"
        );
        assert_eq!(body.thread_id, fixture.thread);
        assert_eq!(
            body.session.segment_id,
            recipe.source_lane.source.segment_id
        );
        assert_eq!(body.session.mode, SessionMode::Resume);
        assert_eq!(
            body.session.native_session_ref.as_deref(),
            Some(recipe.source_lane.source.native_session_ref.as_str())
        );
        assert_eq!(
            (body.replace_segment_id, body.replace_retained_source),
            (None, None)
        );
        let wire: Outgoing =
            serde_json::from_value(job.row().await?.plan["wire"]["resume_source"].clone())?;
        assert!(
            serde_json::to_value(&wire)? == serde_json::to_value(outgoing)?,
            "the producer uses its complete frozen outgoing command"
        );
        let bytes = tokio::fs::read(
            fixture
                .world
                .machine
                .data_dir()
                .join("journal")
                .join(format!("{}.json", outgoing.id)),
        )
        .await?;
        let entry: orch_link::journal::Entry = serde_json::from_slice(&bytes)?;
        assert_eq!(entry.cmd.id, outgoing.id);
        assert_eq!(entry.cmd.kind, outgoing.kind.as_str());
        assert_eq!(entry.cmd.key, outgoing.key.wire());
        assert_eq!(entry.cmd.deadline, recipe.deadline);
        assert!(
            entry.cmd.body == outgoing.body,
            "the actual native journal retains the exact prior body"
        );
        let result: SessionOpenResult = serde_json::from_value(
            entry
                .result()
                .ok_or("the source resume did not genuinely finish before the ending")?
                .outcome
                .clone()
                .map_err(|error| error.detail)?,
        )?;
        assert_eq!(
            result.native_session_ref,
            recipe.source_lane.source.native_session_ref
        );
        let runs = fixture.world.machine.handler.runs_of(outgoing.id);
        assert_eq!(runs, 1);
        journals.insert(outgoing.id, bytes);
        executions.insert(outgoing.id, runs);
    }
    assert!(
        fixture
            .engine
            .inner
            .commands
            .status(recipe.commands.restore_active)
            .await?
            .is_none()
    );
    let mut issued = fixture
        .world
        .commands
        .issued_commands(CommandKind::SessionContinuationResume.as_str());
    issued.sort_by_key(|row| row.1.id);
    assert_eq!(
        issued.len(),
        attempts.len(),
        "all actual native resumes at the barrier have an owned producer witness"
    );
    for command in &attempts {
        let (machine, sent) = issued
            .iter()
            .find(|(_, sent)| sent.id == command.id)
            .ok_or("the actual source resume transport command is absent")?;
        assert_eq!(*machine, command.machine);
        assert!(
            sent.body == command.body
                && sent.kind == command.kind.as_str()
                && sent.key == command.key.wire()
                && Some(sent.deadline) == command.deadline,
            "the prior transport retains every outgoing source/budget field"
        );
    }
    Ok(ResumeSnapshot {
        attempts: serde_json::to_value(attempts)?,
        issued: serde_json::to_value(issued)?,
        journals,
        executions,
    })
}

async fn resumes_unchanged(
    fixture: &Fixture,
    recipe: &Recipe,
    trace: &Trace,
    resumes: &ResumeSnapshot,
) -> Proof {
    assert!(
        serde_json::to_value(trace.commands())? == resumes.attempts,
        "an ending cannot add, replay or change any owned native resume attempt"
    );
    let mut issued = fixture
        .world
        .commands
        .issued_commands(CommandKind::SessionContinuationResume.as_str());
    issued.sort_by_key(|row| row.1.id);
    assert!(
        serde_json::to_value(issued)? == resumes.issued,
        "the actual native resume command set/body/key/deadline is unchanged after the ending"
    );
    for (id, bytes) in &resumes.journals {
        assert!(
            tokio::fs::read(
                fixture
                    .world
                    .machine
                    .data_dir()
                    .join("journal")
                    .join(format!("{id}.json"))
            )
            .await?
                == *bytes,
            "the genuine pre-ending native resume journal is immutable"
        );
        assert_eq!(
            fixture.world.machine.handler.runs_of(*id),
            resumes.executions[id],
            "the prior resume is never executed again"
        );
    }
    assert!(
        fixture
            .engine
            .inner
            .commands
            .status(recipe.commands.restore_active)
            .await?
            .is_none(),
        "no original-lane restore is admitted after the ending"
    );
    Ok(())
}

/// The authored input and factual history captured before the ending.
struct RetainedSnapshot<'a> {
    revised: &'a threads::InputRow,
    facts: &'a Json,
}

async fn kept(
    fixture: &Fixture,
    job: &Job,
    recipe: &Recipe,
    retained: RetainedSnapshot<'_>,
    ending: Ending,
    trace: &Trace,
    resumes: &ResumeSnapshot,
) -> Proof {
    let RetainedSnapshot {
        revised,
        facts: before,
    } = retained;
    assert_eq!(job.row().await?.state, "cancelled");
    let current = threads::thread(&mut *fixture.world.pool.acquire().await?, fixture.thread)
        .await?
        .ok_or("thread absent")?;
    assert_eq!(
        current.active_continuation_id,
        recipe.request.expected.active_continuation_id
    );
    assert_eq!(
        current.execution_revision,
        recipe.request.expected.execution_revision
    );
    assert_eq!(current.current_turn_id, None);
    match ending {
        Ending::Archive=>assert!(current.archived_at.is_some()),
        Ending::RemoveBinding=>assert_eq!(current.environment_id,None),
        Ending::DisableHome=>assert!(sqlx::query_scalar::<_,bool>("SELECT a.disabled AND p.status='disabled' AND e.status='disabled'
            FROM agents a CROSS JOIN providers p CROSS JOIN environments e WHERE a.id=$1 AND p.id=$2 AND e.id=$3")
            .bind(fixture.world.agent).bind(fixture.world.provider).bind(fixture.world.environment)
            .fetch_one(fixture.world.pool.pool()).await?),
    }
    let mut conn = fixture.world.pool.acquire().await?;
    let retained = threads::input(&mut conn, revised.id)
        .await?
        .ok_or("revised input absent")?;
    assert_eq!(
        retained, *revised,
        "inactive revised words and their owner/hold never migrate or disappear"
    );
    let old = threads::input(&mut conn, fixture.pending.id)
        .await?
        .ok_or("the old selected input disappeared")?;
    assert!(old.seq_updated > fixture.pending.seq_updated);
    let mut expected = fixture.pending.clone();
    expected.state = "cancelled".into();
    expected.state_detail = Some(
        match ending {
            Ending::Archive => "archived",
            Ending::RemoveBinding => "environment_removed",
            Ending::DisableHome => "disabled",
        }
        .into(),
    );
    expected.cancelled_by = Some(fixture.world.ada);
    expected.seq_updated = old.seq_updated;
    assert_eq!(
        old, expected,
        "the existing ending changes only its selected input's exact cancellation and stamp"
    );
    let version = rows::continuation(&mut conn, fixture.thread, recipe.target_continuation_id)
        .await?
        .ok_or("version absent")?;
    assert_eq!(version.status, "failed");
    assert_eq!(
        threads::segment(&mut conn, fixture.source.source.segment_id)
            .await?
            .ok_or("source segment absent")?
            .native_session_ref
            .as_deref(),
        Some(fixture.source.source.native_session_ref.as_str())
    );
    drop(conn);
    assert_eq!(
        facts(fixture).await?,
        *before,
        "closing a native lane cannot change existing turns/accounting/reservations"
    );
    assert_eq!(fixture.world.turn_starts(fixture.thread).len(), 1);
    resumes_unchanged(fixture, recipe, trace, resumes).await?;
    assert_eq!(
        tokio::fs::read(fixture.file.path().join("workspace.bin")).await?,
        b"retained\0workspace\xffbytes"
    );
    for (kind, count) in [
        CommandKind::FsMaterialize,
        CommandKind::FsRestore,
        CommandKind::FsRecord,
    ]
    .into_iter()
    .zip(fixture.file_work)
    {
        assert_eq!(
            fixture.world.began(kind),
            count,
            "retirement cannot implicitly undo files through {kind}"
        );
    }
    Ok(())
}

async fn after_creation(ending: Ending) -> Proof {
    let fixture = Fixture::new(false).await?;
    strict_closer(&fixture);
    let accepted = fixture
        .admission(
            fixture.source.continuation_id,
            "lifecycle-after-native-success",
        )
        .await?;
    let (job, recipe) = fixture.claim(accepted.operation_id).await?;
    let trace = Trace::new(job.id());
    let revised = threads::input(
        &mut *fixture.world.pool.acquire().await?,
        recipe.input_id.ok_or("revised identity absent")?,
    )
    .await?
    .ok_or("revised input absent")?;
    let before = facts(&fixture).await?;
    let (notice, reached) = oneshot::channel();
    let (release, resume) = oneshot::channel();
    let running = fixture.engine.clone();
    let claimed = job.clone();
    let work = tokio::spawn(AFTER_CREATION.scope(
        Mutex::new(Some(Boundary {
            reached: notice,
            resume,
        })),
        async move { running.drive_job(&claimed).await },
    ));
    tokio::time::timeout(common::PATIENCE, reached).await??;
    let path = fixture
        .world
        .machine
        .data_dir()
        .join("journal")
        .join(format!("{}.json", recipe.commands.open_prefix));
    let entry: orch_link::journal::Entry = serde_json::from_slice(&tokio::fs::read(path).await?)?;
    let opened: NativePrefixOpened = serde_json::from_value(
        entry
            .result()
            .ok_or("the actual native result is not durable")?
            .outcome
            .clone()
            .map_err(|error| error.detail)?,
    )?;
    assert_eq!(entry.cmd.id, recipe.commands.open_prefix);
    assert_eq!(
        fixture
            .current
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(|held| held.segment_id),
        Some(recipe.segment_id)
    );
    let resumes = resume_snapshot(&fixture, &job, &recipe, &trace, true).await?;
    end(&fixture, ending).await?;
    assert!(
        job.row()
            .await?
            .plan
            .get("continuation_retirement")
            .is_some()
    );
    release
        .send(())
        .map_err(|()| "the native-success latch was lost")?;
    tokio::time::timeout(common::PATIENCE, work).await???;
    kept(
        &fixture,
        &job,
        &recipe,
        RetainedSnapshot {
            revised: &revised,
            facts: &before,
        },
        ending,
        &trace,
        &resumes,
    )
    .await?;
    assert!(
        fixture
            .current
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_none()
    );
    let version = rows::continuation(
        &mut *fixture.world.pool.acquire().await?,
        fixture.thread,
        accepted.continuation_id,
    )
    .await?
    .ok_or("version absent")?;
    assert_eq!(
        serde_json::from_value::<NativePrefixOpened>(
            version
                .native_creation_result
                .ok_or("genuine native receipt was lost")?
        )?,
        opened
    );
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
            .inner
            .commands
            .status(recipe.commands.restore_active)
            .await?
            .is_none()
    );
    let closes: BTreeMap<Id, Json> =
        serde_json::from_value(job.row().await?.plan["continuation_retirement_closes"].clone())?;
    assert_eq!(closes.len(), 2);
    assert!(
        closes
            .values()
            .all(|receipt| receipt["finished"] == json!(true))
    );
    for (segment, receipt) in closes {
        let command: Outgoing = serde_json::from_value(receipt["command"].clone())?;
        let actual: orch_link::journal::Entry = serde_json::from_slice(
            &tokio::fs::read(
                fixture
                    .world
                    .machine
                    .data_dir()
                    .join("journal")
                    .join(format!("{}.json", command.id)),
            )
            .await?,
        )?;
        assert_eq!(actual.cmd.id, command.id);
        assert_eq!(
            actual.cmd.deadline,
            command
                .deadline
                .ok_or("the reducing command lost its original deadline")?
        );
        assert_eq!(actual.cmd.key, command.key.wire());
        assert_eq!(
            actual.cmd.body,
            serde_json::to_value(SessionCloseFenced {
                thread_id: fixture.thread,
                segment_id: segment
            })?
        );
        assert!(actual.result().is_some_and(|result| result.outcome.is_ok()
            || matches!(&result.outcome,Err(error) if error.code=="session.segment_changed")));
    }
    // A late ordinary archival control cannot close some other lane or reopen this one.
    drain_controls(&fixture).await?;
    assert!(
        fixture
            .current
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_none()
    );
    resumes_unchanged(&fixture, &recipe, &trace, &resumes).await?;
    assert_eq!(facts(&fixture).await?, before);
    drop(job);
    fixture.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn archive_after_a_real_native_child_succeeds_retires_only_owned_lanes_without_restoring_the_original()
-> Proof {
    after_creation(Ending::Archive).await
}

#[tokio::test]
async fn removing_the_actual_binding_after_native_success_can_retire_without_requiring_the_removed_binding_to_resume()
-> Proof {
    after_creation(Ending::RemoveBinding).await
}

#[tokio::test]
async fn withdrawing_the_actual_profile_provider_and_environment_keeps_native_cleanup_reducing_without_new_use_or_input()
-> Proof {
    after_creation(Ending::DisableHome).await
}

#[tokio::test]
async fn archive_before_replacement_keeps_the_revision_and_pending_version_without_admitting_a_child()
-> Proof {
    let fixture = Fixture::new(false).await?;
    strict_closer(&fixture);
    let accepted = fixture
        .admission(
            fixture.source.continuation_id,
            "archive-before-native-replacement",
        )
        .await?;
    let (job, recipe) = fixture.claim(accepted.operation_id).await?;
    let trace = Trace::new(job.id());
    let revised = threads::input(
        &mut *fixture.world.pool.acquire().await?,
        recipe.input_id.ok_or("revised identity absent")?,
    )
    .await?
    .ok_or("revised input absent")?;
    let before = facts(&fixture).await?;
    let resumes = resume_snapshot(&fixture, &job, &recipe, &trace, false).await?;
    end(&fixture, Ending::Archive).await?;
    tokio::time::timeout(common::PATIENCE, fixture.engine.drive_job(&job)).await??;
    assert_eq!(fixture.world.began(CommandKind::SessionPrefixOpen), 0);
    assert_eq!(
        fixture.world.began(CommandKind::SessionContinuationClose),
        0
    );
    kept(
        &fixture,
        &job,
        &recipe,
        RetainedSnapshot {
            revised: &revised,
            facts: &before,
        },
        Ending::Archive,
        &trace,
        &resumes,
    )
    .await?;
    // No replacement was admitted: the actual existing archive supplier owns its one close.
    drain_controls(&fixture).await?;
    assert!(
        fixture
            .current
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_none()
    );
    assert_eq!(fixture.world.began(CommandKind::SessionCloseFenced), 1);
    drop(job);
    fixture.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn an_archive_withdraws_a_durably_prepared_restore_before_issue_and_recovery_does_not_readmit_it()
-> Proof {
    let fixture = Fixture::new(false).await?;
    strict_closer(&fixture);
    let accepted = fixture
        .admission(
            fixture.source.continuation_id,
            "archive-at-original-restore-admission",
        )
        .await?;
    let (job, recipe) = fixture.claim(accepted.operation_id).await?;
    let trace = Trace::new(job.id());
    let revised = threads::input(
        &mut *fixture.world.pool.acquire().await?,
        recipe.input_id.ok_or("revised identity absent")?,
    )
    .await?
    .ok_or("revised input absent")?;
    let before = facts(&fixture).await?;
    // This is the real first-close checkpoint, followed by an admitted but not issued
    // housekeeping resume. No native result or machine action is manufactured here.
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
    let body = phase(
        Continuing::body(
            &job,
            &recipe,
            &fixture.original,
            fixture.original.source.segment_id,
            SessionMode::Resume,
            true,
        )
        .await,
    )?;
    let restore = SessionContinuationResume {
        thread_id: fixture.thread,
        session: Box::new(body),
        source: fixture.original.source.clone(),
        replace_segment_id: Some(recipe.segment_id),
        replace_retained_source: Some(recipe.source_lane.source.clone()),
    };
    let admitted = phase(
        Continuing::saved(
            &job,
            "restore_active",
            recipe.commands.restore_active,
            fixture.original.machine_id,
            &restore,
            None,
        )
        .await,
    )?;
    let saved =
        orch_db::queries::link::command(&mut *fixture.world.pool.acquire().await?, admitted.id)
            .await?
            .ok_or("the actual admitted restore is absent")?;
    assert_eq!(saved.state, "queued");
    assert!(
        fixture
            .engine
            .inner
            .commands
            .status(admitted.id)
            .await?
            .is_none()
    );
    let resumes = resume_snapshot(&fixture, &job, &recipe, &trace, false).await?;
    end(&fixture, Ending::Archive).await?;
    let withdrawn =
        orch_db::queries::link::command(&mut *fixture.world.pool.acquire().await?, admitted.id)
            .await?
            .ok_or("the withdrawal lost the admitted restore")?;
    assert_eq!(withdrawn.state, "cancelled");
    assert_eq!(
        (withdrawn.body, withdrawn.serial_key, withdrawn.deadline_at),
        (saved.body, saved.serial_key, saved.deadline_at)
    );
    let refused = Continuing::saved(
        &job,
        "restore_active",
        admitted.id,
        fixture.original.machine_id,
        &restore,
        None,
    )
    .await;
    assert!(matches!(refused,Err(Stop::Failed(failure)) if failure.code=="continuation.retiring"));
    tokio::time::timeout(common::PATIENCE, fixture.engine.drive_job(&job)).await??;
    kept(
        &fixture,
        &job,
        &recipe,
        RetainedSnapshot {
            revised: &revised,
            facts: &before,
        },
        Ending::Archive,
        &trace,
        &resumes,
    )
    .await?;
    assert!(
        fixture
            .engine
            .inner
            .commands
            .status(admitted.id)
            .await?
            .is_none()
    );
    assert!(
        fixture
            .world
            .machine
            .handler
            .executions()
            .iter()
            .all(|run| run.id != admitted.id)
    );
    assert!(
        !fixture
            .world
            .machine
            .data_dir()
            .join("journal")
            .join(format!("{}.json", admitted.id))
            .exists()
    );
    assert_eq!(fixture.world.began(CommandKind::SessionPrefixOpen), 0);
    drain_controls(&fixture).await?;
    drop(job);
    fixture.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn an_original_close_gate_cannot_authorize_a_candidate_body_or_bypass_fresh_source_validation()
-> Proof {
    let fixture = Fixture::new(true).await?;
    let (job, recipe) = fixture.formerly_accepted().await?;
    let before = facts(&fixture).await?;
    let candidate = SessionContinuationClose {
        thread_id: fixture.thread,
        provider_id: fixture.source.provider_id,
        agent_id: fixture.source.agent_id,
        source: fixture.source.source.clone(),
    };
    let wrong = Continuing::saved(
        &job,
        "close_active",
        recipe.commands.close_active,
        fixture.original.machine_id,
        &candidate,
        Some(recipe.deadline),
    )
    .await;
    assert!(matches!(wrong,Err(Stop::Failed(failure)) if failure.code=="continuation.identity"));
    let fresh = Continuing::close_with_gate(
        &job,
        &recipe,
        &fixture.original,
        "close_active",
        recipe.commands.close_active,
        false,
    )
    .await;
    assert!(
        matches!(fresh,Err(Stop::Failed(failure)) if failure.code=="continuation.source_changed")
    );
    assert!(job.row().await?.plan.get("wire").is_none());
    assert!(
        orch_db::queries::link::command(
            &mut *fixture.world.pool.acquire().await?,
            recipe.commands.close_active
        )
        .await?
        .is_none()
    );
    assert_eq!(
        fixture.world.began(CommandKind::SessionContinuationClose),
        0
    );
    assert_eq!(fixture.world.began(CommandKind::SessionPrefixOpen), 0);
    assert_eq!(
        fixture
            .current
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref(),
        Some(&fixture.original.source)
    );
    assert_eq!(facts(&fixture).await?, before);
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
    fixture.unchanged_workspace().await?;
    drop(job);
    fixture.shutdown().await;
    Ok(())
}
