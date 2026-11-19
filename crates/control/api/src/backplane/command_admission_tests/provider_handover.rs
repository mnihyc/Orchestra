//! Exact Return/Accept first-send controls through the existing real native journal.
//! These private source facts come from the owned database/actual Return result,
//! never from a product request, and no native vendor or real credential is used.

use super::*;
use orch_engine::credentials::handover_authority::{
    HandoverAuthority, HandoverCommandAuthority, ReturnSource,
};
use orch_link_contract::commands::{
    AgentCredentialReturn, MachineKey, ProviderCredentialAccept, SealedCredential,
};

const RETURN: &str = "ProviderHandoverReturn.provider";

#[derive(Clone, Copy, PartialEq, Eq)]
enum SourceChange {
    None,
    Revoke,
    Custody,
    Generation,
    Keeper,
    UnrelatedRuntime,
    ReturnKey,
    ReturnResult,
}

struct HandoverFixture {
    login: LoginFixture,
    credential: Credential,
    origin: HandoverAuthority,
    source: ReturnSource,
    returning: Outgoing,
}

impl HandoverFixture {
    async fn new() -> Proof<Self> {
        let login = LoginFixture::new().await?;
        let mut conn = login.transport.test.pool.acquire().await?;
        sqlx::query("UPDATE providers SET held_by_environment_id=$2,held_since=$3 WHERE id=$1")
            .bind(login.provider)
            .bind(login.transport.environment)
            .bind(login.transport.clock.now())
            .execute(&mut *conn)
            .await?;
        let scope: KeyScope = serde_json::from_value(json!({"rules":[{
            "target":{"kind":"provider","id":login.provider,"reach":"self"},
            "allow":[{"operation":RETURN,"actions":["manage"]}]
        }]}))?;
        let token = new_token(API_TOKEN_PREFIX);
        let token_id = new_id();
        identity::create_api_token_scoped(
            &mut conn,
            &identity::NewApiToken {
                id: token_id,
                user_id: login.owner,
                name: "Owned handover reconnect authority",
                token_hash: &token_hash(&token),
                scopes: &[],
                node_id: None,
                at: login.transport.clock.now(),
                expires_at: None,
            },
            Some(&serde_json::to_value(&scope)?),
        )
        .await?;
        let caller = Caller::token(
            login.owner,
            TokenGrant::from_stored_scope(&[], None, Some(scope), None)?.for_token(token_id),
        );
        let mut plan = orch_engine::operation_scope::Prepared::for_selection(RETURN)?;
        let provider = orch_engine::operation_scope::targets::page_object(
            &mut conn,
            ObjectRef {
                kind: ObjectKind::Provider,
                id: login.provider,
            },
        )
        .await?;
        plan.bind(0, provider.object.kind, &[provider])?;
        let credential = Credential {
            id: token_id,
            caller: plan.admit(&caller, false)?,
        };
        drop(conn);
        let mut tx = login.transport.link.inner.db.begin().await?;
        let origin = HandoverAuthority::capture(
            &mut tx,
            &credential.caller,
            login.provider,
            false,
            false,
            login.transport.clock.time(),
        )
        .await?;
        tx.commit().await?;
        let mut conn = login.transport.test.pool.acquire().await?;
        let state = provider_runtime::state(&mut conn, login.provider)
            .await?
            .ok_or("the actual handover provider is absent")?;
        let environment = orch_db::queries::environments::thread_engine::environment(
            &mut conn,
            login.transport.environment,
        )
        .await?
        .ok_or("the actual holder environment is absent")?;
        let since = provider_runtime::held_since(&mut conn, login.provider)
            .await?
            .ok_or("the actual custody timestamp is absent")?;
        let machine = resources::machine(&mut conn, login.transport.machine)
            .await?
            .ok_or("the actual custodian machine is absent")?;
        let custodian = MachineKey {
            machine_id: machine.id,
            public_key: machine.link().public_key.clone(),
        };
        let runtime = provider_runtime::live_runtime(&mut conn, login.provider)
            .await?
            .filter(|row| row.machine_id == Some(custodian.machine_id))
            .map(|row| (row.id, row.generation));
        let lease: Option<Id> = sqlx::query_scalar(
            "SELECT id FROM leases WHERE provider_id=$1 AND mode='handover'
                AND recipient_machine_id=$2 AND opened_at >= $3
                ORDER BY opened_at DESC,id DESC LIMIT 1",
        )
        .bind(login.provider)
        .bind(environment.machine_id)
        .bind(since)
        .fetch_optional(&mut *conn)
        .await?;
        let return_id = new_id();
        // Construction is private test evidence over actual stored facts. Production
        // captures this same shape in ReturnSource::capture, never through HTTP JSON.
        let source: ReturnSource = serde_json::from_value(json!({
            "provider":login.provider,"holder":environment.id,"since":Time::new(since),
            "holder_machine":environment.machine_id,"generation":environment.generation,
            "agent":login.agent,"custodian":custodian,"runtime":runtime,"lease":lease,
            "provider_machine":state.machine_id,"key_machine":state.key_machine_id,
            "return_command":return_id,
        }))?;
        let authority: HandoverCommandAuthority = serde_json::from_value(json!({
            "origin":origin,"source":source,"command":return_id,"returned":null
        }))?;
        let returning = Outgoing::new(
            return_id,
            login.transport.machine,
            login.key(),
            &AgentCredentialReturn {
                provider_id: login.provider,
                agent_id: login.agent,
                custodian,
            },
        )?
        .with_login_authority(LoginCommandAuthority::handover(authority));
        drop(conn);
        Ok(Self {
            login,
            credential,
            origin,
            source,
            returning,
        })
    }

    async fn accepting(&mut self) -> Proof<Outgoing> {
        within(self.login.transport.connect(false)).await??;
        DispatchCommands::new(self.login.transport.dispatcher.clone())
            .issue(self.returning.clone())
            .await?;
        let CommandOutcome::Succeeded(body) =
            within(self.login.transport.dispatcher.wait(self.returning.id)).await??
        else {
            return Err("the source Return must actually succeed before Accept is prepared".into());
        };
        let sealed: SealedCredential = serde_json::from_value(body.clone())?;
        assert_eq!(sealed, handover_returned());
        let row = self
            .login
            .transport
            .dispatcher
            .status(self.returning.id)
            .await?
            .ok_or("the actual succeeded Return row is absent")?;
        assert_eq!(row.state, "succeeded");
        assert_eq!(row.machine_id, self.returning.machine);
        assert_eq!(row.body, self.returning.body);
        assert_eq!(row.result, Some(body));
        assert!(
            matches!(self.login.transport.server.entry(self.returning.id)
            .ok_or("the source Return has no actual native journal entry")?.step,
            Step::Done { ref result } if result.outcome == Ok(serde_json::to_value(&sealed)?))
        );
        let id = new_id();
        let authority: HandoverCommandAuthority = serde_json::from_value(json!({
            "origin":self.origin,"source":self.source,"command":id,
            "returned":Hash::of(&serde_json::to_vec(&sealed)?),
        }))?;
        Ok(Outgoing::new(
            id,
            self.login.transport.machine,
            SerialKey::Provider(self.login.provider),
            &ProviderCredentialAccept {
                provider_id: self.login.provider,
                form: sealed.form,
                credential: sealed.credential,
                expires_at: sealed.expires_at,
            },
        )?
        .with_login_authority(LoginCommandAuthority::handover(authority)))
    }

    fn expected(command: &Outgoing) -> Proof<orch_types::Json> {
        Ok(if command.kind == CommandKind::AgentCredentialReturn {
            serde_json::to_value(handover_returned())?
        } else {
            json!({})
        })
    }

    fn received_once(&self, command: &Outgoing, deadline: Time) -> Proof {
        let native = self
            .login
            .transport
            .server
            .entry(command.id)
            .ok_or("the successful handover command has no native journal entry")?;
        assert!(matches!(native.step, Step::Done { ref result }
            if result.id == command.id && result.outcome == Ok(Self::expected(command)?)));
        let wire: Vec<Cmd> = self
            .login
            .transport
            .native
            .wire
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|sent| sent.id == command.id)
            .cloned()
            .collect();
        assert_eq!(wire.len(), 1);
        assert_eq!(native.cmd, wire[0]);
        assert_eq!(
            serde_json::to_value(&wire[0])?,
            json!({"id":command.id,"k":command.kind.as_str(),"key":command.key.wire(),
                "deadline":deadline,"b":command.body}),
            "private caller/source evidence never enters the native frame"
        );
        Ok(())
    }

    async fn change_source(&self, change: SourceChange) -> Proof {
        if change == SourceChange::Revoke {
            return self.login.revoke(&self.credential).await;
        }
        let mut conn = self.login.transport.test.pool.acquire().await?;
        match change {
            SourceChange::None | SourceChange::Revoke => {}
            SourceChange::Custody => {
                // One PostgreSQL timestamp unit distinguishes a newer same-holder
                // custody without consuming the command's original deadline.
                let now = self.login.transport.clock.now() + chrono::Duration::microseconds(1);
                self.login.transport.clock.set(now);
                sqlx::query("UPDATE providers SET held_since=$2 WHERE id=$1")
                    .bind(self.login.provider)
                    .bind(now)
                    .execute(&mut *conn)
                    .await?;
            }
            SourceChange::Generation => {
                sqlx::query("UPDATE environments SET generation=generation+1 WHERE id=$1")
                    .bind(self.login.transport.environment)
                    .execute(&mut *conn)
                    .await?;
            }
            SourceChange::Keeper => {
                let replacement = resources::testing::insert_machine(
                    &mut conn,
                    "owned-handover-replacement-keeper",
                    "connected",
                )
                .await?;
                assert_ne!(replacement, self.login.transport.machine);
                sqlx::query("UPDATE providers SET key_machine_id=$2 WHERE id=$1")
                    .bind(self.login.provider)
                    .bind(replacement)
                    .execute(&mut *conn)
                    .await?;
            }
            SourceChange::UnrelatedRuntime => {
                let runtime = resources::testing::insert_machine(
                    &mut conn,
                    "owned-unrelated-provider-runtime",
                    "connected",
                )
                .await?;
                assert_ne!(runtime, self.login.transport.machine);
                sqlx::query("UPDATE providers SET machine_id=$2 WHERE id=$1")
                    .bind(self.login.provider)
                    .bind(runtime)
                    .execute(&mut *conn)
                    .await?;
            }
            SourceChange::ReturnKey => {
                let replacement = SerialKey::Environment(self.login.transport.environment).wire();
                assert_ne!(replacement, self.returning.key.wire());
                sqlx::query("UPDATE commands SET serial_key=$2 WHERE id=$1")
                    .bind(self.returning.id)
                    .bind(replacement)
                    .execute(&mut *conn)
                    .await?;
            }
            SourceChange::ReturnResult => {
                let mut other = handover_returned();
                other.credential.value.push_str("changed");
                sqlx::query("UPDATE commands SET result=$2 WHERE id=$1")
                    .bind(self.returning.id)
                    .bind(serde_json::to_value(other)?)
                    .execute(&mut *conn)
                    .await?;
            }
        }
        Ok(())
    }

    async fn reconnect(&mut self, command: &Outgoing, deadline: Time, refused: bool) -> Proof {
        assert!(self.login.transport.clock.time() < deadline);
        within(self.login.transport.connect(false)).await??;
        let outcome = within(self.login.transport.dispatcher.wait(command.id)).await??;
        self.login.transport.completed_probe().await?;
        let row = self
            .login
            .transport
            .dispatcher
            .status(command.id)
            .await?
            .ok_or("the terminal handover command row is absent")?;
        assert!(row.is_over());
        assert_eq!(Time::new(row.deadline_at), deadline);
        assert_eq!(row.body, command.body);
        if refused {
            let CommandOutcome::Failed(error) = outcome else {
                return Err(
                    "a stale or withdrawn handover must refuse before native acceptance".into(),
                );
            };
            assert_eq!(error.code, ErrorCode::Conflict.as_str());
            assert_eq!(row.state, "failed");
            assert_eq!(row.attempts, 0);
            self.login.no_transmission(command.id);
        } else {
            assert!(matches!(outcome, CommandOutcome::Succeeded(ref body)
                if body == &Self::expected(command)?));
            self.received_once(command, deadline)?;
        }
        Ok(())
    }

    async fn retained_success(&self, command: &Outgoing, deadline: Time) -> Proof {
        let native = self
            .login
            .transport
            .server
            .entry(command.id)
            .ok_or("the completed native handover command is absent")?;
        let row = self
            .login
            .transport
            .dispatcher
            .status(command.id)
            .await?
            .ok_or("the completed durable handover row is absent")?;
        DispatchCommands::new(self.login.transport.dispatcher.clone())
            .issue(command.clone())
            .await?;
        assert!(
            matches!(within(self.login.transport.dispatcher.wait(command.id)).await??,
            CommandOutcome::Succeeded(ref body) if body == &Self::expected(command)?)
        );
        self.login.transport.completed_probe().await?;
        assert_eq!(self.login.transport.server.entry(command.id), Some(native));
        assert_eq!(
            self.login
                .transport
                .dispatcher
                .status(command.id)
                .await?
                .ok_or("the retained durable handover row is absent")?
                .result,
            row.result
        );
        self.received_once(command, deadline)
    }
}

#[tokio::test]
async fn queued_handover_return_and_accept_recheck_the_actual_key_before_first_reconnect_wire()
-> Proof {
    for accept in [false, true] {
        let changes = if accept {
            &[
                SourceChange::None,
                SourceChange::Revoke,
                SourceChange::Custody,
                SourceChange::Generation,
                SourceChange::Keeper,
                SourceChange::UnrelatedRuntime,
                SourceChange::ReturnKey,
                SourceChange::ReturnResult,
            ][..]
        } else {
            &[
                SourceChange::None,
                SourceChange::Revoke,
                SourceChange::Custody,
                SourceChange::Generation,
                SourceChange::Keeper,
                SourceChange::UnrelatedRuntime,
            ][..]
        };
        for change in changes {
            let mut fixture = HandoverFixture::new().await?;
            let command = if accept {
                fixture.accepting().await?
            } else {
                fixture.returning.clone()
            };
            let deadline = fixture.login.queue_offline(&command).await?;
            fixture.change_source(*change).await?;
            fixture
                .reconnect(
                    &command,
                    deadline,
                    !matches!(change, SourceChange::None | SourceChange::UnrelatedRuntime),
                )
                .await?;
            if matches!(change, SourceChange::None | SourceChange::UnrelatedRuntime) {
                fixture.login.revoke(&fixture.credential).await?;
                fixture.retained_success(&command, deadline).await?;
            }
            fixture.login.transport.close().await?;
        }
    }
    Ok(())
}

#[tokio::test]
async fn already_accepted_handover_results_remain_canonical_after_the_initiating_key_is_revoked()
-> Proof {
    for accept in [false, true] {
        let mut fixture = HandoverFixture::new().await?;
        let command = if accept {
            fixture.accepting().await?
        } else {
            fixture.returning.clone()
        };
        let deadline = fixture.login.queue_offline(&command).await?;
        let (entered, release) = gate(command.id, Phase::NativeHandover);
        within(fixture.login.transport.connect(false)).await??;
        within(entered).await??;
        within(async {
            loop {
                if fixture
                    .login
                    .transport
                    .dispatcher
                    .status(command.id)
                    .await?
                    .is_some_and(|row| row.state == "accepted")
                {
                    return Ok::<(), crate::dispatch::DispatchError>(());
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await??;
        fixture.login.revoke(&fixture.credential).await?;
        release
            .send(())
            .map_err(|_| "the accepted native handover gate disappeared")?;
        assert!(
            matches!(within(fixture.login.transport.dispatcher.wait(command.id)).await??,
            CommandOutcome::Succeeded(ref body) if body == &HandoverFixture::expected(&command)?)
        );
        fixture.retained_success(&command, deadline).await?;
        fixture.login.transport.close().await?;
    }
    Ok(())
}

#[tokio::test]
async fn a_succeeded_return_stays_for_its_live_custody_retry_and_is_reaped_after_release() -> Proof
{
    let mut fixture = HandoverFixture::new().await?;
    let accept = fixture.accepting().await?;
    DispatchCommands::new(fixture.login.transport.dispatcher.clone())
        .issue(accept.clone())
        .await?;
    assert!(
        matches!(within(fixture.login.transport.dispatcher.wait(accept.id)).await??,
        CommandOutcome::Succeeded(ref body) if body == &json!({}))
    );
    let mut conn = fixture.login.transport.test.pool.acquire().await?;
    let returned = rows::command(&mut conn, fixture.returning.id)
        .await?
        .ok_or("the actual succeeded Return row is absent")?;
    assert_eq!(returned.state, "succeeded");
    assert_eq!(
        returned.result,
        Some(serde_json::to_value(handover_returned())?)
    );
    assert!(!rows::RECORD_KINDS.contains(&returned.kind.as_str()));
    let accepted = rows::command(&mut conn, accept.id)
        .await?
        .ok_or("the actual succeeded Accept row is absent")?;
    assert_eq!(accepted.state, "succeeded");
    assert_eq!(accepted.result, Some(json!({})));
    assert!(!rows::RECORD_KINDS.contains(&accepted.kind.as_str()));
    let custody = provider_runtime::state(&mut conn, fixture.login.provider)
        .await?
        .ok_or("the actual provider custody is absent")?;
    assert_eq!(
        custody.held_by_environment_id,
        Some(fixture.login.transport.environment)
    );
    let since = provider_runtime::held_since(&mut conn, fixture.login.provider)
        .await?
        .ok_or("the actual custody timestamp is absent")?;
    assert!(since <= returned.created_at);
    // Cover this fixture's complete command inventory in one finite batch. The
    // cutoff is one SQL timestamp unit beyond its actual terminal row, not a wait.
    let limit: i64 = sqlx::query_scalar("SELECT count(*) FROM commands")
        .fetch_one(&mut *conn)
        .await?;
    assert!(limit > 0);
    let cutoff = returned.updated_at.max(accepted.updated_at) + chrono::Duration::microseconds(1);
    rows::purge_finished(&mut conn, cutoff, limit).await?;
    let retained = rows::command(&mut conn, fixture.returning.id)
        .await?
        .ok_or("reaping must retain the exact Return result while its custody is live")?;
    assert_eq!(retained.state, returned.state);
    assert_eq!(retained.machine_id, returned.machine_id);
    assert_eq!(retained.serial_key, returned.serial_key);
    assert_eq!(retained.body, returned.body);
    assert_eq!(retained.result, returned.result);
    let retained_accept = rows::command(&mut conn, accept.id)
        .await?
        .ok_or("reaping must retain the exact completed Accept for the same live custody")?;
    assert_eq!(retained_accept.machine_id, accepted.machine_id);
    assert_eq!(retained_accept.serial_key, accepted.serial_key);
    assert_eq!(retained_accept.body, accepted.body);
    assert_eq!(retained_accept.result, accepted.result);
    assert!(
        provider_runtime::release_credential(
            &mut conn,
            fixture.login.provider,
            fixture.login.transport.environment,
            fixture.login.transport.clock.now(),
        )
        .await?
    );
    assert!(rows::purge_finished(&mut conn, cutoff, limit).await? > 0);
    assert!(
        rows::command(&mut conn, fixture.returning.id)
            .await?
            .is_none()
    );
    assert!(rows::command(&mut conn, accept.id).await?.is_none());
    drop(conn);
    fixture.login.transport.close().await?;
    Ok(())
}
