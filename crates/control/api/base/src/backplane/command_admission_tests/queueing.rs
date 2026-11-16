//! Independent native work and Stop do not wait behind another command's API preparation.

use super::*;

async fn a_directory_read_reaches_the_machine_while_another_key_is_still_being_prepared() -> Proof {
    let mut fixture = Fixture::new(false).await?;
    within(fixture.connect(false)).await??;
    let original = fixture.original().await?;
    let id = original.id.ok_or("the original ID is absent")?;
    let (checked, release) = gate(id, Phase::Wire);
    fixture.dispatcher.issue_stored(original).await?;
    within(checked).await??;

    let read = fixture
        .dispatcher
        .issue(
            fixture.machine,
            &SerialKey::None,
            &orch_link_contract::commands::FsStat {
                environment_id: fixture.environment,
                mount: None,
                path: String::new(),
            },
        )
        .await?
        .id;
    let answered = within(fixture.dispatcher.wait(read)).await;
    let original_still_held = fixture.server.entry(id).is_none();
    let actual_answer = fixture.server.entry(read);

    // Release the original and close the real peer even on the failing baseline.
    release.send(()).map_err(|_| "the preparation gate ended")?;
    within(fixture.dispatcher.wait(id)).await??;
    within(fixture.dispatcher.wait(read)).await??;
    fixture.close().await?;
    assert!(
        original_still_held,
        "the first command's wire gate was not held"
    );
    assert!(
        matches!(answered, Ok(Ok(CommandOutcome::Succeeded(_)))),
        "an independent read waited for another key's API preparation: {answered:?}"
    );
    assert!(
        matches!(actual_answer.map(|entry| entry.step), Some(Step::Done { result }) if result.outcome.is_ok()),
        "the independent read needs its actual native journal result before releasing the other key"
    );
    Ok(())
}

#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_a_directory_read_reaches_the_machine_while_another_key_is_still_being_prepared()
-> futures::future::LocalBoxFuture<'static, Proof> {
    Box::pin(a_directory_read_reaches_the_machine_while_another_key_is_still_being_prepared())
}

async fn stop_reaches_the_machine_while_an_ordinary_command_is_still_being_prepared() -> Proof {
    let mut fixture = Fixture::new(false).await?;
    within(fixture.connect(false)).await??;
    let original = fixture.original().await?;
    let id = original.id.ok_or("the original ID is absent")?;
    let (checked, release) = gate(id, Phase::Wire);
    fixture.dispatcher.issue_stored(original.clone()).await?;
    within(checked).await??;
    let (cancelled, continue_peer) = gate(id, Phase::NativeCancel);
    assert!(fixture.dispatcher.cancel(id).await?);

    let reached = within(cancelled).await;
    let original_still_held = fixture.server.entry(id).is_none();
    // The native peer has recorded cancellation before it can see the held command.
    let _ = continue_peer.send(());
    release.send(()).map_err(|_| "the preparation gate ended")?;
    fixture.completed_probe().await?;
    until(|| fixture.server.in_hand() == 0).await?;
    let reads = fixture.native.reads.load(Ordering::SeqCst);
    let writes = fixture.native.writes.load(Ordering::SeqCst);
    fixture.original_retained(&original).await?;
    fixture.close().await?;

    assert!(
        original_still_held,
        "the first command's wire gate was not held"
    );
    assert!(
        matches!(reached, Ok(Ok(()))),
        "Stop waited for an ordinary command's API preparation: {reached:?}"
    );
    assert_eq!(
        (reads, writes),
        (0, 0),
        "withdrawn work must have no native effect"
    );
    Ok(())
}

#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_stop_reaches_the_machine_while_an_ordinary_command_is_still_being_prepared()
-> futures::future::LocalBoxFuture<'static, Proof> {
    Box::pin(stop_reaches_the_machine_while_an_ordinary_command_is_still_being_prepared())
}

async fn a_command_waiting_for_api_capacity_keeps_its_full_machine_work_budget() -> Proof {
    let mut fixture = Fixture::new(false).await?;
    orch_db::queries::settings::put(
        &mut *fixture.test.pool.acquire().await?,
        None,
        orch_settings_contract::registry::keys::LINK_COMMAND_PREPARATIONS,
        &orch_db::queries::settings::Stored {
            value: &json!(1),
            locked: false,
            by: None,
            at: fixture.clock.now(),
        },
        None,
    )
    .await?;
    within(fixture.connect(false)).await??;
    let original = fixture.original().await?;
    let id = original.id.ok_or("the original ID is absent")?;
    let (checked, release) = gate(id, Phase::Wire);
    fixture.dispatcher.issue_stored(original).await?;
    within(checked).await??;
    let read = fixture
        .dispatcher
        .issue(
            fixture.machine,
            &SerialKey::None,
            &orch_link_contract::commands::FsStat {
                environment_id: fixture.environment,
                mount: None,
                path: String::new(),
            },
        )
        .await?
        .id;
    // Two complete work allowances pass entirely in the API preparation queue.
    fixture.clock.advance(chrono::Duration::seconds(
        i64::from(CommandKind::FsStat.deadline_secs()) * 2,
    ));
    let entry = fixture
        .link
        .loaded_entry(fixture.machine)
        .ok_or("the actual loaded command link is absent")?;
    entry
        .controller
        .commands()
        .tick(tokio::time::Instant::now());
    let waiting = entry.controller.commands().progress(read);
    let not_on_machine = fixture.server.entry(read).is_none();
    release.send(()).map_err(|_| "the preparation gate ended")?;
    within(fixture.dispatcher.wait(id)).await??;
    let answered = within(fixture.dispatcher.wait(read)).await??;
    let actual = fixture.server.entry(read);
    fixture.close().await?;
    assert!(
        not_on_machine,
        "the held capacity did not exercise the API queue"
    );
    assert!(
        matches!(
            waiting,
            Some(orch_link::commands::Progress::Queued | orch_link::commands::Progress::Sent)
        ),
        "API queue time consumed the command's own work allowance: {waiting:?}"
    );
    assert!(
        matches!(answered, CommandOutcome::Succeeded(_)),
        "{answered:?}"
    );
    assert!(
        matches!(actual.map(|entry| entry.step), Some(Step::Done { result }) if result.outcome.is_ok())
    );
    Ok(())
}

#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_a_command_waiting_for_api_capacity_keeps_its_full_machine_work_budget()
-> futures::future::LocalBoxFuture<'static, Proof> {
    Box::pin(a_command_waiting_for_api_capacity_keeps_its_full_machine_work_budget())
}
