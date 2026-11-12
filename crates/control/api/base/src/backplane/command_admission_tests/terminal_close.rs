//! Actual journal/dispatcher proof for a new captured close meeting canonical legacy None.
//! This is the native carrier seam, not an HTTP TerminalClose or local CAS/audit proof.

use super::*;

use crate::thread_engine::DispatchCommands;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::key_scope::KeyScope;

use orch_access_contract::permissions::ObjectKind;

use orch_authz::Caller;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_authz::TokenGrant;

use orch_db::queries::identity;

use orch_db::queries::terminals;

use orch_engine::commands::CommandAuthority;

use orch_engine::commands::Commands;

use orch_engine::commands::Outgoing;

use orch_engine::commands::TerminalCloseAuthority;

use orch_engine::operation_scope::Prepared;

use orch_engine::operation_scope::targets;

use orch_link_contract::commands::PtyClose;

use orch_link_contract::commands::PtyCloseResult;
fn result() -> PtyCloseResult {
    PtyCloseResult {
        exit_code: Some(0),
        bytes_out: 5,
        kept: None,
    }
}

pub(super) async fn reply(
    native: &Native,
    context: CommandContext,
    _body: PtyClose,
) -> Result<CommandResult, CommandError> {
    if context.key.is_some() {
        return Err(CommandError::new(
            ErrorCode::Conflict,
            "pty.close has no serial key",
        ));
    }
    native.entered.store(true, Ordering::SeqCst);
    if native.hold {
        native
            .release
            .acquire()
            .await
            .map_err(|_| CommandError::new(ErrorCode::Internal, "the owned native gate ended"))?
            .forget();
    }
    Ok(CommandResult::PtyClose(result()))
}

struct Close {
    key: Id,
    command: Outgoing,
}

async fn close(fixture: &Fixture) -> Proof<Close> {
    let mut conn = fixture.test.pool.acquire().await?;
    let owner: Id = sqlx::query_scalar("SELECT owner_id FROM environments WHERE id=$1")
        .bind(fixture.environment)
        .fetch_one(&mut *conn)
        .await?;
    let terminal = new_id();
    terminals::insert(
        &mut conn,
        &terminals::NewSession {
            id: terminal,
            thread_id: None,
            environment_id: fixture.environment,
            env_generation: 1,
            opened_by: owner,
            cwd: ".",
            cols: 80,
            rows: 24,
        },
        fixture.clock.now(),
    )
    .await?;
    let session = terminals::opened(&mut conn, terminal, "owned-shell", fixture.clock.now())
        .await?
        .ok_or("the actual terminal did not become open")?;
    let scope: KeyScope = serde_json::from_value(json!({"rules":[{
        "target":{"kind":"environment","id":fixture.environment,"reach":"self"},
        "allow":[{"operation":"TerminalClose","actions":["manage"]}]
    }]}))?;
    assert!(scope.validate().is_empty());
    let key = new_id();
    let expires = fixture.clock.now() + chrono::Duration::hours(1);
    identity::create_api_token_scoped(
        &mut conn,
        &identity::NewApiToken {
            id: key,
            user_id: owner,
            name: "Owned terminal close carrier",
            token_hash: Hash::of(key.as_bytes()).as_bytes(),
            scopes: &[],
            node_id: None,
            at: fixture.clock.now(),
            expires_at: Some(expires),
        },
        Some(&json!(scope)),
    )
    .await?;
    let caller = Caller::token(
        owner,
        TokenGrant::from_stored_scope(&[], None, Some(scope), Some(Time::new(expires)))?
            .for_token(key),
    );
    let target = targets::resource_identity(
        &mut conn,
        ObjectRef {
            kind: ObjectKind::Environment,
            id: session.environment_id,
        },
    )
    .await?;
    let mut plan = Prepared::for_selection("TerminalClose")?;
    plan.bind(
        0,
        orch_access_contract::key_scope::KeyTargetKind::Object(ObjectKind::Environment),
        &[target],
    )?;
    let caller = plan.admit(&caller, false)?;
    let principal = Principal::load(&mut conn, caller.clone(), fixture.clock.time()).await?;
    assert!(
        orch_authz::environments::may_close_shell(
            &mut conn,
            &principal,
            &orch_authz::environments::Shell {
                environment: session.environment_id,
                thread: session.thread_id,
                opened_by: session.opened_by,
            },
            Policy::default()
        )
        .await?,
        "actual opener ordinary close premise"
    );
    let command = Outgoing::new(
        orch_engine::ids::derived("pty.close", &[session.id.as_bytes().as_slice()]),
        fixture.machine,
        SerialKey::None,
        &PtyClose {
            terminal_id: session.id,
        },
    )?;
    let authority = TerminalCloseAuthority::capture(&caller, &session, fixture.machine, &command)?;
    Ok(Close {
        key,
        command: command.with_authority(CommandAuthority::terminal_close(authority)),
    })
}

fn legacy(command: &Outgoing) -> StoredCommand {
    StoredCommand {
        authority: None,
        id: Some(command.id),
        machine: command.machine,
        kind: command.kind.as_str().into(),
        key: command.key.wire(),
        deadline: command.deadline,
        body: command.body.clone(),
    }
}

async fn same_path_positive() -> Proof {
    let mut fixture = Fixture::new(false).await?;
    let close = close(&fixture).await?;
    DispatchCommands::new(fixture.dispatcher.clone())
        .issue(close.command.clone())
        .await?;
    within(fixture.connect(false)).await??;
    assert!(
        matches!(within(fixture.dispatcher.wait(close.command.id)).await??,
        CommandOutcome::Succeeded(ref body) if body == &serde_json::to_value(result())?),
        "typed same-path positive must reach actual native Accepted/result first"
    );
    fixture.completed_probe().await?;
    let entry = fixture
        .server
        .entry(close.command.id)
        .ok_or("typed positive has no actual native journal entry")?;
    assert!(matches!(entry.step, Step::Done { ref result } if result.outcome.is_ok()));
    assert_eq!(entry.cmd.body, close.command.body);
    let metadata =
        rows::command_authority(&mut *fixture.test.pool.acquire().await?, close.command.id)
            .await?
            .ok_or("typed positive private envelope absent")?;
    assert_eq!(metadata["kind"], "terminal_close");
    assert_eq!(
        metadata["authority"]["terminal_id"],
        close.command.body["terminal_id"]
    );
    fixture.close().await
}

async fn revoke(fixture: &Fixture, key: Id) -> Proof {
    let changed =
        sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1 AND revoked_at IS NULL")
            .bind(key)
            .bind(fixture.clock.now())
            .execute(&fixture.test.pool)
            .await?
            .rows_affected();
    assert_eq!(changed, 1);
    Ok(())
}

#[cfg(any(test, feature = "test-support"))]
async fn new_scoped_terminal_close_refuses_unaccepted_legacy_none_without_cancelling_old_work()
-> Proof {
    same_path_positive().await?;
    let mut fixture = Fixture::new(false).await?;
    let close = close(&fixture).await?;
    fixture
        .dispatcher
        .issue_stored(legacy(&close.command))
        .await?;
    let before = fixture
        .dispatcher
        .status(close.command.id)
        .await?
        .ok_or("canonical legacy command absent")?;
    assert_eq!(before.state, "queued");
    assert_eq!(before.attempts, 0);
    assert!(
        rows::command_authority(&mut *fixture.test.pool.acquire().await?, before.id)
            .await?
            .is_none()
    );
    assert!(fixture.server.entry(before.id).is_none());
    let attempted = DispatchCommands::new(fixture.dispatcher.clone())
        .issue(close.command.clone())
        .await;
    assert!(
        matches!(
            attempted,
            Err(orch_engine::commands::CommandsError::AuthorityMismatch)
        ),
        "new scoped attempt must truthfully refuse canonical legacy None: {attempted:?}"
    );
    let after = fixture
        .dispatcher
        .status(before.id)
        .await?
        .ok_or("old command lost")?;
    assert_eq!(
        after, before,
        "new attempt cannot alter canonical tuple, None, state or budget"
    );
    assert!(
        rows::command_authority(&mut *fixture.test.pool.acquire().await?, before.id)
            .await?
            .is_none()
    );
    assert!(fixture.server.entry(before.id).is_none());
    revoke(&fixture, close.key).await?;
    within(fixture.connect(false)).await??;
    assert!(
        matches!(within(fixture.dispatcher.wait(before.id)).await??,
        CommandOutcome::Succeeded(ref body) if body == &serde_json::to_value(result())?),
        "old legacy/system work must remain eligible after refusing the new scoped attempt"
    );
    fixture.completed_probe().await?;
    let journal = fixture
        .server
        .entry(before.id)
        .ok_or("old work never reached actual native journal")?;
    assert_eq!(journal.cmd.body, before.body);
    assert!(matches!(journal.step, Step::Done { ref result } if result.outcome.is_ok()));
    assert!(
        rows::command_authority(&mut *fixture.test.pool.acquire().await?, before.id)
            .await?
            .is_none()
    );
    fixture.close().await
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_new_scoped_terminal_close_refuses_unaccepted_legacy_none_without_cancelling_old_work()
-> ::futures::future::LocalBoxFuture<'static, crate::backplane::command_admission_tests::Proof> {
    ::std::boxed::Box::pin(
        self::new_scoped_terminal_close_refuses_unaccepted_legacy_none_without_cancelling_old_work(
        ),
    )
}

#[cfg(any(test, feature = "test-support"))]
async fn new_scoped_attempt_never_relabels_accepted_legacy_none_or_vetoes_its_result() -> Proof {
    same_path_positive().await?;
    let mut fixture = Fixture::new(true).await?;
    let close = close(&fixture).await?;
    fixture
        .dispatcher
        .issue_stored(legacy(&close.command))
        .await?;
    within(fixture.connect(false)).await??;
    until(|| fixture.native.entered.load(Ordering::SeqCst)).await?;
    within(async {
        loop {
            let row = fixture
                .dispatcher
                .status(close.command.id)
                .await?
                .ok_or("actual close row absent")?;
            if row.state == "accepted" {
                break Ok::<(), Box<dyn std::error::Error + Send + Sync>>(());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await??;
    let before = fixture
        .dispatcher
        .status(close.command.id)
        .await?
        .ok_or("accepted close absent")?;
    let journal = fixture
        .server
        .entry(before.id)
        .ok_or("Accepted must be actual native journal custody")?;
    let attempted = DispatchCommands::new(fixture.dispatcher.clone())
        .issue(close.command.clone())
        .await;
    assert!(
        matches!(
            attempted,
            Err(orch_engine::commands::CommandsError::AuthorityMismatch)
        ),
        "new attempt cannot reinterpret already accepted None as its own origin: {attempted:?}"
    );
    assert_eq!(
        fixture.dispatcher.status(before.id).await?.as_ref(),
        Some(&before)
    );
    assert!(
        rows::command_authority(&mut *fixture.test.pool.acquire().await?, before.id)
            .await?
            .is_none()
    );
    revoke(&fixture, close.key).await?;
    fixture.native.release.add_permits(1);
    assert!(matches!(within(fixture.dispatcher.wait(before.id)).await??,
        CommandOutcome::Succeeded(ref body) if body == &serde_json::to_value(result())?));
    fixture.completed_probe().await?;
    let ended = fixture
        .server
        .entry(before.id)
        .ok_or("accepted native result lost")?;
    assert_eq!(ended.cmd, journal.cmd);
    assert!(matches!(ended.step, Step::Done { ref result } if result.outcome.is_ok()));
    assert!(
        rows::command_authority(&mut *fixture.test.pool.acquire().await?, before.id)
            .await?
            .is_none()
    );
    fixture.close().await
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_new_scoped_attempt_never_relabels_accepted_legacy_none_or_vetoes_its_result()
-> ::futures::future::LocalBoxFuture<'static, crate::backplane::command_admission_tests::Proof> {
    ::std::boxed::Box::pin(
        self::new_scoped_attempt_never_relabels_accepted_legacy_none_or_vetoes_its_result(),
    )
}

#[derive(Clone, Copy)]
enum Withdraw {
    Key,
    Generation,
}

async fn queued_refusal(withdraw: Withdraw) -> Proof {
    same_path_positive().await?;
    let mut fixture = Fixture::new(false).await?;
    let close = close(&fixture).await?;
    DispatchCommands::new(fixture.dispatcher.clone())
        .issue(close.command.clone())
        .await?;
    let before = fixture
        .dispatcher
        .status(close.command.id)
        .await?
        .ok_or("actual typed queued close absent")?;
    assert_eq!(before.state, "queued");
    assert_eq!(before.attempts, 0);
    assert!(fixture.server.entry(before.id).is_none());
    let metadata =
        rows::command_authority(&mut *fixture.test.pool.acquire().await?, before.id).await?;
    assert!(metadata.is_some());
    match withdraw {
        Withdraw::Key => revoke(&fixture, close.key).await?,
        Withdraw::Generation => {
            assert_eq!(
                sqlx::query("UPDATE environments SET generation=generation+1 WHERE id=$1")
                    .bind(fixture.environment)
                    .execute(&fixture.test.pool)
                    .await?
                    .rows_affected(),
                1
            );
        }
    }
    within(fixture.connect(false)).await??;
    let outcome = within(fixture.dispatcher.wait(before.id)).await??;
    fixture.completed_probe().await?;
    assert!(
        matches!(outcome, CommandOutcome::Failed(ref error) if error.code == ErrorCode::Conflict.as_str()),
        "current withdrawal must refuse fresh native acceptance: {outcome:?}"
    );
    let after = fixture
        .dispatcher
        .status(before.id)
        .await?
        .ok_or("refused canonical close absent")?;
    assert_eq!(after.state, "failed");
    assert_eq!(after.attempts, 0);
    assert_eq!(after.body, before.body);
    assert_eq!(after.serial_key, before.serial_key);
    assert_eq!(after.deadline_at, before.deadline_at);
    assert_eq!(
        rows::command_authority(&mut *fixture.test.pool.acquire().await?, before.id).await?,
        metadata
    );
    assert!(
        fixture.server.entry(before.id).is_none(),
        "withdrawn close must not enter native journal"
    );
    assert!(
        !fixture
            .native
            .wire
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .any(|frame| frame.id == before.id),
        "withdrawn close must not cross actual gateway"
    );
    fixture.close().await
}

#[cfg(any(test, feature = "test-support"))]
async fn captured_terminal_close_rechecks_its_real_key_before_first_wire() -> Proof {
    queued_refusal(Withdraw::Key).await
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_captured_terminal_close_rechecks_its_real_key_before_first_wire()
-> ::futures::future::LocalBoxFuture<'static, crate::backplane::command_admission_tests::Proof> {
    ::std::boxed::Box::pin(self::captured_terminal_close_rechecks_its_real_key_before_first_wire())
}

#[cfg(any(test, feature = "test-support"))]
async fn captured_terminal_close_rechecks_its_actual_environment_generation_before_first_wire()
-> Proof {
    queued_refusal(Withdraw::Generation).await
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_captured_terminal_close_rechecks_its_actual_environment_generation_before_first_wire()
-> ::futures::future::LocalBoxFuture<'static, crate::backplane::command_admission_tests::Proof> {
    ::std::boxed::Box::pin(
        self::captured_terminal_close_rechecks_its_actual_environment_generation_before_first_wire(
        ),
    )
}
