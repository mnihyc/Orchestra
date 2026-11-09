//! A rolled-back authority epoch check never becomes a native effect or a new command.

use super::*;

async fn change_epoch(fixture: &Fixture) -> Proof {
    let mut tx = fixture.test.pool.begin().await?;
    orch_db::queries::access::bump_epoch(&mut tx).await?;
    tx.commit().await?;
    Ok(())
}

async fn admission_race(change_twice: bool) -> Proof {
    let mut fixture = Fixture::new(false).await?;
    within(fixture.connect(false)).await??;
    let original = fixture.original().await?;
    let id = original.id.ok_or("the original command has no identity")?;
    let (checked, release) = gate(id, Phase::AdmissionCommit);
    fixture.dispatcher.issue_stored(original.clone()).await?;
    within(checked).await??;
    assert!(fixture.server.entry(id).is_none());
    assert!(!fixture.native.destination.exists());
    change_epoch(&fixture).await?;

    if change_twice {
        let (fresh_checked, fresh_release) = gate(id, Phase::AdmissionCommit);
        release
            .send(())
            .map_err(|_| "the first admission was lost")?;
        within(fresh_checked).await??;
        assert!(fixture.server.entry(id).is_none());
        assert!(!fixture.native.destination.exists());
        change_epoch(&fixture).await?;
        fresh_release
            .send(())
            .map_err(|_| "the fresh admission was lost")?;
    } else {
        release
            .send(())
            .map_err(|_| "the first admission was lost")?;
    }

    let outcome = within(fixture.dispatcher.wait(id)).await??;
    if change_twice {
        assert!(
            matches!(outcome, CommandOutcome::Failed(ref error) if error.code == "internal"),
            "the one fresh attempt is bounded: {outcome:?}"
        );
        assert!(fixture.server.entry(id).is_none());
        assert!(!fixture.native.destination.exists());
        assert_eq!(fixture.native.reads.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.native.writes.load(Ordering::SeqCst), 0);
    } else {
        assert!(
            matches!(outcome, CommandOutcome::Succeeded(_)),
            "{outcome:?}"
        );
        assert_eq!(fixture.native.reads.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.native.writes.load(Ordering::SeqCst), 1);
        assert_eq!(
            tokio::fs::read(&fixture.native.destination).await?,
            b"original owned input\0\xff"
        );
    }
    let frames = fixture
        .native
        .wire
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .filter(|frame| frame.id == id)
        .count();
    assert_eq!(frames, usize::from(!change_twice));
    let saved = fixture
        .dispatcher
        .status(id)
        .await?
        .ok_or("the canonical command was lost")?;
    assert_eq!(saved.body, original.body);
    assert_eq!(saved.serial_key(), original.key.as_deref());
    assert_eq!(Some(Time::new(saved.deadline_at)), original.deadline);
    assert_eq!(saved.kind, original.kind);
    fixture.close().await
}

#[tokio::test]
async fn a_changed_admission_epoch_rechecks_the_same_command_before_its_only_native_effect() -> Proof
{
    admission_race(false).await
}

#[tokio::test]
async fn a_second_changed_admission_epoch_refuses_without_a_native_effect() -> Proof {
    admission_race(true).await
}

#[tokio::test]
async fn a_cancelled_command_stays_cancelled_during_fresh_admission() -> Proof {
    let mut fixture = Fixture::new(false).await?;
    within(fixture.connect(false)).await??;
    let original = fixture.original().await?;
    let id = original.id.ok_or("the original command has no identity")?;
    let (checked, release) = gate(id, Phase::AdmissionCommit);
    let (rolled_back, retry) = gate(id, Phase::AdmissionRetry);
    fixture.dispatcher.issue_stored(original.clone()).await?;
    within(checked).await??;
    change_epoch(&fixture).await?;
    release
        .send(())
        .map_err(|_| "the first admission was lost")?;
    within(rolled_back).await??;
    assert!(fixture.dispatcher.cancel(id).await?);
    retry.send(()).map_err(|_| "the fresh admission was lost")?;
    fixture.completed_probe().await?;
    fixture.no_old_effect(&original).await?;
    fixture.close().await
}
