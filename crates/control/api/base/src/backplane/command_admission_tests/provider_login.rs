//! Actual first transmission after a connected route is rebuilt. Private login origins
//! travel through DispatchCommands and the durable row; the peer uses the parent's real
//! CommandServer and file-backed journal, without running a vendor login or model.

use super::*;

use crate::thread_engine::DispatchCommands;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::key_scope::KeyScope;

use orch_access_contract::permissions::ObjectKind;

use orch_api_core::middleware::authentication::API_TOKEN_PREFIX;

use orch_api_core::middleware::authentication::new_token;

use orch_api_core::middleware::authentication::token_hash;

use orch_authz::Caller;

use orch_authz::CapturedOperation;

use orch_authz::TokenGrant;

use orch_authz::ownership;

use orch_db::queries::access;

use orch_db::queries::identity;

use orch_db::queries::resources::provider_runtime;

use orch_engine::commands::Commands;

use orch_engine::commands::Outgoing;

use orch_engine::provider_setup::LoginAuthority;

use orch_engine::provider_setup::LoginCommandAuthority;

use orch_engine::provider_setup::Pending;

use orch_engine::provider_setup::login_operation;

use orch_engine::provider_setup::provider_login_authorized;

use orch_link_contract::commands::ProviderLoginCancel;

use orch_link_contract::commands::ProviderLoginInput;

use orch_link_contract::commands::ProviderLoginStart;

use std::collections::BTreeMap;
const START: &str = "ProviderLoginStart.interactive";

const INPUT: &str = "ProviderLoginInput";

const CANCEL: &str = "ProviderLoginCancel";

const METHOD: &str = "device_code";

struct Credential {
    id: Id,
    caller: Caller,
}

impl Credential {
    fn origin(&self) -> Proof<CapturedOperation> {
        Ok(CapturedOperation::capture(&self.caller)?)
    }
}

struct LoginFixture {
    transport: Fixture,
    owner: Id,
    org: Id,
    provider: Id,
    agent: Id,
}

impl LoginFixture {
    async fn new() -> Proof<Self> {
        let transport = Fixture::new(false).await?;
        let mut conn = transport.test.pool.acquire().await?;
        let (owner, org): (Id, Id) =
            sqlx::query_as("SELECT owner_id,org_id FROM environments WHERE id=$1")
                .bind(transport.environment)
                .fetch_one(&mut *conn)
                .await?;
        // The ordinary Provider owner is a person in this actual team root. This
        // also makes a subsequently required second step apply to that person.
        access::testing::insert_member(&mut conn, org, owner, org).await?;
        access::testing::name_owner(&mut conn, org, ("user", owner), transport.clock.now()).await?;
        let (provider, agent) = (new_id(), new_id());
        let descriptor: orch_agent_contract::descriptor::AgentDescriptor =
            serde_json::from_value(json!({
                "kind":"fake", "adapter_version":1,
                "harness":{"name":"Owned login peer", "version":"1.0.0",
                    "supported_range":">=1.0.0", "status":"supported"},
                "arrangements":[{"type":"separated", "executor":"orch-exec",
                    "executor_version":"*", "tool_sets":["plain"]}],
                "native_provider":{"harness_kind":"fake", "title":"Owned subscription",
                    "arrangements":["separated"], "login_methods":[{
                        "id":METHOD, "title":"Sign in with a code", "summary":"Shows a code.",
                        "kind":"interactive", "yields":"login", "lifetime_secs":null,
                        "requires":null, "recommended":true, "fields":[]
                    }]}
            }))?;
        sqlx::query(
            "INSERT INTO agents(id,name,owner_id,org_id,home_id,harness_kind,descriptor)
            VALUES($1,'Owned login profile',$2,$3,$3,'fake',$4)",
        )
        .bind(agent)
        .bind(owner)
        .bind(org)
        .bind(serde_json::to_value(descriptor)?)
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "INSERT INTO providers(id,name,owner_id,org_id,home_id,kind,harness_kind,
            agent_id,machine_id,key_machine_id,arrangements,status)
            VALUES($1,'Owned subscription',$2,$3,$3,'subscription','fake',$4,$5,$5,
                ARRAY['separated'],'needs_setup')",
        )
        .bind(provider)
        .bind(owner)
        .bind(org)
        .bind(agent)
        .bind(transport.machine)
        .execute(&mut *conn)
        .await?;
        ownership::start_resource(
            &mut conn,
            ObjectRef {
                kind: ObjectKind::Provider,
                id: provider,
            },
            owner,
            ownership::NewAccess::OnlyMe,
            transport.clock.time(),
        )
        .await?;
        // The declared adapter owns its credential home; no permissive default
        // substitutes for the machine facts used by provider_sign_in.
        sqlx::query("UPDATE machines SET facts=$2 WHERE id=$1")
            .bind(transport.machine)
            .bind(json!({"adapters":[{"kind":"fake","default_home_owned":true}]}))
            .execute(&mut *conn)
            .await?;
        drop(conn);
        Ok(Self {
            transport,
            owner,
            org,
            provider,
            agent,
        })
    }

    async fn credential(&self, operation: &str, expires: Option<Time>) -> Proof<Credential> {
        let scope: KeyScope = serde_json::from_value(json!({"rules":[{
            "target":{"kind":"provider","id":self.provider,"reach":"self"},
            "allow":[{"operation":operation,"actions":["manage"]}]
        }]}))?;
        let token = new_token(API_TOKEN_PREFIX);
        let id = new_id();
        let mut conn = self.transport.test.pool.acquire().await?;
        identity::create_api_token_scoped(
            &mut conn,
            &identity::NewApiToken {
                id,
                user_id: self.owner,
                name: "Owned reconnect authority",
                token_hash: &token_hash(&token),
                scopes: &[],
                node_id: None,
                at: self.transport.clock.now(),
                expires_at: expires.map(Time::get),
            },
            Some(&serde_json::to_value(&scope)?),
        )
        .await?;
        let caller = Caller::token(
            self.owner,
            TokenGrant::from_stored_scope(&[], None, Some(scope), expires)?.for_token(id),
        );
        let plan = login_operation(&mut conn, self.provider, operation).await?;
        let credential = Credential {
            id,
            caller: plan.admit(&caller, false)?,
        };
        assert!(
            provider_login_authorized(
                &mut conn,
                self.provider,
                &credential.origin()?,
                self.transport.clock.time(),
            )
            .await?,
            "the stored key and ordinary Provider owner start authorized"
        );
        Ok(credential)
    }

    fn pending(&self, starter: Option<&Credential>) -> Proof<Pending> {
        let pending = Pending::new(METHOD, &BTreeMap::new(), self.owner);
        Ok(match starter {
            Some(starter) => pending.with_authority(&starter.caller)?,
            None => pending,
        })
    }

    async fn retain(&self, pending: &Pending, active: bool) -> Proof {
        let mut conn = self.transport.test.pool.acquire().await?;
        provider_runtime::keep_pending(
            &mut conn,
            self.provider,
            &pending.to_bytes()?,
            self.transport.clock.now(),
        )
        .await?;
        let authority = pending.authority_state()?;
        provider_runtime::keep_login_authority(&mut conn, self.provider, authority.as_ref())
            .await?;
        if active {
            assert!(provider_runtime::clear_pending(&mut conn, self.provider).await?);
        }
        Ok(())
    }

    async fn active(&self) -> Proof<Option<LoginAuthority>> {
        Ok(LoginAuthority::load(
            &mut *self.transport.test.pool.acquire().await?,
            self.provider,
        )
        .await?)
    }

    async fn revoke(&self, credential: &Credential) -> Proof {
        assert!(
            identity::revoke_api_token(
                &mut *self.transport.test.pool.acquire().await?,
                credential.id,
                self.owner,
                self.transport.clock.now(),
            )
            .await?
        );
        Ok(())
    }

    fn key(&self) -> SerialKey {
        SerialKey::Harness {
            provider_id: self.provider,
            agent_id: self.agent,
        }
    }

    fn start(&self, pending: &Pending) -> Proof<Outgoing> {
        Ok(Outgoing::new(
            new_id(),
            self.transport.machine,
            self.key(),
            &ProviderLoginStart {
                provider_id: self.provider,
                agent_id: self.agent,
                method: pending.method.clone(),
                values: Some(BTreeMap::new()),
                config: json!({}),
            },
        )?
        .with_login_authority(LoginCommandAuthority::start(self.provider, pending)))
    }

    async fn input(&self, answerer: &Credential) -> Proof<Outgoing> {
        Ok(Outgoing::new(
            new_id(),
            self.transport.machine,
            self.key(),
            &ProviderLoginInput {
                provider_id: self.provider,
                agent_id: self.agent,
                values: BTreeMap::new(),
            },
        )?
        .with_login_authority(LoginCommandAuthority::input(
            self.provider,
            self.active().await?,
            Some(answerer.origin()?),
        )))
    }

    async fn cancel(&self, canceller: &Credential) -> Proof<Outgoing> {
        // The cancellation producer clears the old pending/active attempt before
        // queueing cleanup. Its own origin survives; the old Start is not needed.
        let mut conn = self.transport.test.pool.acquire().await?;
        provider_runtime::clear_pending(&mut conn, self.provider).await?;
        provider_runtime::keep_login_authority(&mut conn, self.provider, None).await?;
        Ok(Outgoing::new(
            new_id(),
            self.transport.machine,
            self.key(),
            &ProviderLoginCancel {
                provider_id: self.provider,
                agent_id: self.agent,
            },
        )?
        .with_login_authority(LoginCommandAuthority::cancel(
            self.provider,
            Some(canceller.origin()?),
        )))
    }

    fn no_transmission(&self, id: Id) {
        assert!(
            self.transport.server.entry(id).is_none(),
            "the refused command never entered the actual native journal"
        );
        assert!(
            !self
                .transport
                .native
                .wire
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .iter()
                .any(|command| command.id == id),
            "the refused command never reached the gateway wire"
        );
    }

    async fn queue_offline(&mut self, command: &Outgoing) -> Proof<Time> {
        within(self.transport.connect(false)).await??;
        self.transport.completed_probe().await?;
        self.transport.link.rebuild(self.transport.machine).await;
        DispatchCommands::new(self.transport.dispatcher.clone())
            .issue(command.clone())
            .await?;
        let row = self
            .transport
            .dispatcher
            .status(command.id)
            .await?
            .ok_or("the actual Outgoing was not durably queued")?;
        assert_eq!(row.state, "queued");
        assert_eq!(row.attempts, 0);
        assert_eq!(row.body, command.body);
        assert_eq!(row.serial_key, command.key.wire());
        assert_eq!(
            rows::command_login_authority(
                &mut *self.transport.test.pool.acquire().await?,
                command.id,
            )
            .await?,
            command
                .authority
                .as_ref()
                .map(serde_json::to_value)
                .transpose()?
        );
        self.no_transmission(command.id);
        Ok(Time::new(row.deadline_at))
    }

    async fn reconnect(&mut self, command: &Outgoing, deadline: Time, succeeds: bool) -> Proof {
        assert!(
            self.transport.clock.time() < deadline,
            "authority refusal must precede the original command deadline"
        );
        let retained_attempt = self.active().await?;
        within(self.transport.connect(false)).await??;
        let outcome = within(self.transport.dispatcher.wait(command.id)).await??;
        // A successful command on the same route proves reconnect readiness even
        // when the protected login command was refused before native acceptance.
        self.transport.completed_probe().await?;
        let row = self
            .transport
            .dispatcher
            .status(command.id)
            .await?
            .ok_or("the terminal login row is absent")?;
        assert!(row.is_over());
        assert_eq!(Time::new(row.deadline_at), deadline);
        assert_eq!(row.body, command.body);
        assert_eq!(
            self.active().await?,
            retained_attempt,
            "dispatch admission cannot clear or replace the current login attempt"
        );
        if succeeds {
            assert!(matches!(outcome, CommandOutcome::Succeeded(ref body) if body == &json!({})));
            assert_eq!(row.state, "succeeded");
            let native = self
                .transport
                .server
                .entry(command.id)
                .ok_or("the successful login has no actual native journal entry")?;
            assert!(matches!(native.step, Step::Done { ref result }
                if result.id == command.id && result.outcome == Ok(json!({}))));
            let wire: Vec<Cmd> = self
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
            // Compare the complete serialized frame: neither the private origin
            // nor its key policy or attempt can hide in an extra wire member.
            assert_eq!(
                serde_json::to_value(&wire[0])?,
                json!({
                    "id":command.id, "k":command.kind.as_str(), "key":command.key.wire(),
                    "deadline":deadline, "b":command.body,
                })
            );
        } else {
            let CommandOutcome::Failed(error) = outcome else {
                return Err("a withdrawn login must finish as an authorization refusal".into());
            };
            assert_eq!(error.code, ErrorCode::Conflict.as_str());
            assert_eq!(
                error.detail,
                "the accepted login command is no longer authorized"
            );
            assert_eq!(row.state, "failed");
            assert_eq!(row.error, Some(serde_json::to_value(error)?));
            assert_eq!(row.attempts, 0);
            self.no_transmission(command.id);
        }
        Ok(())
    }

    async fn retained_success(&self, command: &Outgoing) -> Proof {
        let before = self
            .transport
            .server
            .entry(command.id)
            .ok_or("the previously successful native command is absent")?;
        DispatchCommands::new(self.transport.dispatcher.clone())
            .issue(command.clone())
            .await?;
        assert!(
            matches!(within(self.transport.dispatcher.wait(command.id)).await??,
            CommandOutcome::Succeeded(ref body) if body == &json!({}))
        );
        self.transport.completed_probe().await?;
        assert_eq!(self.transport.server.entry(command.id), Some(before));
        assert_eq!(
            self.transport
                .native
                .wire
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .iter()
                .filter(|sent| sent.id == command.id)
                .count(),
            1,
            "revocation after native completion cannot cause a replay or replace its result"
        );
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartChange {
    None,
    Revoke,
    Expire,
    ReplacePending,
    RequireSecondStep,
}

#[cfg(any(test, feature = "test-support"))]
async fn a_queued_provider_start_rechecks_its_real_key_pending_attempt_and_setup_on_reconnect()
-> Proof {
    for change in [
        StartChange::None,
        StartChange::Revoke,
        StartChange::Expire,
        StartChange::ReplacePending,
        StartChange::RequireSecondStep,
    ] {
        let mut fixture = LoginFixture::new().await?;
        // One virtual second is a real future key expiration while leaving the
        // standard Start command budget intact; no wall-clock race is needed.
        let expires = (change == StartChange::Expire)
            .then(|| Time::new(fixture.transport.clock.now() + chrono::Duration::seconds(1)));
        let starter = fixture.credential(START, expires).await?;
        let pending = fixture.pending(Some(&starter))?;
        fixture.retain(&pending, false).await?;
        let command = fixture.start(&pending)?;
        let deadline = fixture.queue_offline(&command).await?;
        match change {
            StartChange::None => {}
            StartChange::Revoke => {
                // Replaying a durable identity without metadata must retain its
                // original evidence; replacing it with None would bypass refusal.
                let mut replay = command.clone();
                replay.authority = None;
                DispatchCommands::new(fixture.transport.dispatcher.clone())
                    .issue(replay)
                    .await?;
                assert_eq!(
                    rows::command_login_authority(
                        &mut *fixture.transport.test.pool.acquire().await?,
                        command.id,
                    )
                    .await?,
                    command
                        .authority
                        .as_ref()
                        .map(serde_json::to_value)
                        .transpose()?
                );
                fixture.no_transmission(command.id);
                fixture.revoke(&starter).await?;
            }
            StartChange::Expire => fixture.transport.clock.set(
                expires
                    .ok_or("the expiration control has no actual key deadline")?
                    .get(),
            ),
            StartChange::ReplacePending => {
                let replacement = fixture.pending(Some(&starter))?;
                assert_ne!(replacement.attempt, pending.attempt);
                fixture.retain(&replacement, false).await?;
            }
            StartChange::RequireSecondStep => {
                orch_settings::write::write(
                    &mut *fixture.transport.test.pool.acquire().await?,
                    &orch_settings::write::Write {
                        actor: orch_db::audit::Actor::User(fixture.owner),
                        target: orch_settings::Target::Node(fixture.org),
                        key: "auth.second_step",
                        value: json!("required"),
                        locked: None,
                        expected_revision: None,
                        may_lock: true,
                        origin: &orch_db::audit::Origin::default(),
                    },
                    fixture.transport.clock.now(),
                )
                .await?;
            }
        }
        fixture
            .reconnect(&command, deadline, change == StartChange::None)
            .await?;
        if change == StartChange::None {
            fixture.revoke(&starter).await?;
            fixture.retained_success(&command).await?;
        }
        fixture.transport.close().await?;
    }
    Ok(())
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_a_queued_provider_start_rechecks_its_real_key_pending_attempt_and_setup_on_reconnect()
-> ::futures::future::LocalBoxFuture<'static, crate::backplane::command_admission_tests::Proof> {
    ::std::boxed::Box::pin(
        self::a_queued_provider_start_rechecks_its_real_key_pending_attempt_and_setup_on_reconnect(
        ),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputChange {
    None,
    RevokeStart,
    RevokeInput,
    ReplaceActive,
    ReplaceOrdinaryActive,
}

#[cfg(any(test, feature = "test-support"))]
async fn a_queued_provider_input_rechecks_both_real_keys_and_the_actual_active_attempt_on_reconnect()
-> Proof {
    for change in [
        InputChange::None,
        InputChange::RevokeStart,
        InputChange::RevokeInput,
        InputChange::ReplaceActive,
        InputChange::ReplaceOrdinaryActive,
    ] {
        let mut fixture = LoginFixture::new().await?;
        let starter = fixture.credential(START, None).await?;
        let answerer = fixture.credential(INPUT, None).await?;
        assert_ne!(starter.id, answerer.id);
        let ordinary = change == InputChange::ReplaceOrdinaryActive;
        let pending = fixture.pending((!ordinary).then_some(&starter))?;
        fixture.retain(&pending, true).await?;
        let command = fixture.input(&answerer).await?;
        let deadline = fixture.queue_offline(&command).await?;
        match change {
            InputChange::None => {}
            InputChange::RevokeStart => fixture.revoke(&starter).await?,
            InputChange::RevokeInput => fixture.revoke(&answerer).await?,
            InputChange::ReplaceActive | InputChange::ReplaceOrdinaryActive => {
                let replacement = fixture.pending((!ordinary).then_some(&starter))?;
                assert_ne!(replacement.attempt, pending.attempt);
                fixture.retain(&replacement, true).await?;
            }
        }
        if change == InputChange::RevokeInput {
            // An independent live Start cannot authorize a revoked Input key.
            assert!(
                provider_login_authorized(
                    &mut *fixture.transport.test.pool.acquire().await?,
                    fixture.provider,
                    &starter.origin()?,
                    fixture.transport.clock.time(),
                )
                .await?
            );
        }
        if change == InputChange::RevokeStart {
            assert!(
                provider_login_authorized(
                    &mut *fixture.transport.test.pool.acquire().await?,
                    fixture.provider,
                    &answerer.origin()?,
                    fixture.transport.clock.time(),
                )
                .await?,
                "an independent live Input key cannot replace the revoked Start origin"
            );
        }
        fixture
            .reconnect(&command, deadline, change == InputChange::None)
            .await?;
        fixture.transport.close().await?;
    }
    Ok(())
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_a_queued_provider_input_rechecks_both_real_keys_and_the_actual_active_attempt_on_reconnect()
-> ::futures::future::LocalBoxFuture<'static, crate::backplane::command_admission_tests::Proof> {
    ::std::boxed::Box::pin(self::a_queued_provider_input_rechecks_both_real_keys_and_the_actual_active_attempt_on_reconnect())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CancelChange {
    RevokedStartOnly,
    RevokeCancel,
    ReplaceAttempt,
}

#[cfg(any(test, feature = "test-support"))]
async fn a_queued_provider_cancel_uses_its_own_key_and_cannot_cancel_a_replacement_on_reconnect()
-> Proof {
    for change in [
        CancelChange::RevokedStartOnly,
        CancelChange::RevokeCancel,
        CancelChange::ReplaceAttempt,
    ] {
        let mut fixture = LoginFixture::new().await?;
        let starter = fixture.credential(START, None).await?;
        let canceller = fixture.credential(CANCEL, None).await?;
        assert_ne!(starter.id, canceller.id);
        let pending = fixture.pending(Some(&starter))?;
        fixture.retain(&pending, true).await?;
        let command = fixture.cancel(&canceller).await?;
        let deadline = fixture.queue_offline(&command).await?;
        fixture.revoke(&starter).await?;
        match change {
            CancelChange::RevokedStartOnly => {}
            CancelChange::RevokeCancel => fixture.revoke(&canceller).await?,
            CancelChange::ReplaceAttempt => {
                // A valid ordinary replacement has no original key to revoke;
                // its actual attempt alone must keep the old cleanup away.
                let replacement = fixture.pending(None)?;
                assert_ne!(replacement.attempt, pending.attempt);
                fixture.retain(&replacement, true).await?;
            }
        }
        fixture
            .reconnect(&command, deadline, change == CancelChange::RevokedStartOnly)
            .await?;
        fixture.transport.close().await?;
    }
    Ok(())
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_a_queued_provider_cancel_uses_its_own_key_and_cannot_cancel_a_replacement_on_reconnect()
-> ::futures::future::LocalBoxFuture<'static, crate::backplane::command_admission_tests::Proof> {
    ::std::boxed::Box::pin(self::a_queued_provider_cancel_uses_its_own_key_and_cannot_cancel_a_replacement_on_reconnect())
}

#[cfg(any(test, feature = "test-support"))]
async fn a_native_login_ack_before_its_database_mirror_survives_a_later_local_authority_refusal()
-> Proof {
    use orch_link::commands::Progress;
    use orch_link_contract::commands::{ACK_RESENDS, ACK_TIMEOUT_SECS};

    let mut fixture = LoginFixture::new().await?;
    let starter = fixture.credential(START, None).await?;
    let pending = fixture.pending(Some(&starter))?;
    fixture.retain(&pending, false).await?;
    let command = fixture.start(&pending)?;
    within(fixture.transport.connect(false)).await??;
    fixture.transport.completed_probe().await?;

    // Hold the actual native handler and the Accepted database write separately.
    // The peer's production route_up must already deliver the real journal ACK
    // to the controller while the durable command still says queued or sent.
    let (native_started, release_native) = gate(command.id, Phase::NativeLogin);
    let (accepted_mirror, release_mirror) = gate(command.id, Phase::MirrorAccepted);
    let (local_refusal, release_refusal) = gate(command.id, Phase::LocalRefusal);
    DispatchCommands::new(fixture.transport.dispatcher.clone())
        .issue(command.clone())
        .await?;
    within(native_started).await??;
    within(accepted_mirror).await??;

    let entry = fixture
        .transport
        .link
        .loaded_entry(fixture.transport.machine)
        .ok_or("the connected login controller is absent")?;
    let handle = entry
        .state
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .tracked
        .get(&command.id)
        .map(|tracked| tracked.handle.clone())
        .ok_or("the acknowledged login has no tracked controller handle")?;
    assert_eq!(handle.progress(), Progress::Accepted);
    assert_eq!(
        entry.controller.commands().progress(command.id),
        Some(Progress::Accepted)
    );
    assert!(!entry.controller.commands().answered(command.id));
    let before = fixture
        .transport
        .dispatcher
        .status(command.id)
        .await?
        .ok_or("the acknowledged login has no durable command row")?;
    assert!(matches!(before.state.as_str(), "queued" | "sent"));
    assert!(fixture.transport.clock.now() < before.deadline_at);
    let native = fixture
        .transport
        .server
        .entry(command.id)
        .ok_or("the real ACK has no native journal acceptance")?;
    assert!(
        native.result().is_none(),
        "the native handler is still held"
    );
    assert_eq!(native.cmd.body, command.body);

    fixture.revoke(&starter).await?;
    // This is the existing issuer resend path for the same acknowledged ID.
    // The stale database projection makes its normal authority preparation fail.
    entry.controller.commands().session_ready();
    within(local_refusal).await??;
    assert_eq!(
        entry.controller.commands().progress(command.id),
        Some(Progress::Accepted),
        "a synthetic local refusal cannot overwrite the actual native ACK"
    );
    assert_eq!(handle.progress(), Progress::Accepted);
    assert!(
        !entry.controller.commands().answered(command.id),
        "a local refusal is not a native handler return"
    );
    // Keep LocalRefusal holding the output loop while synthetic timer ticks
    // cross every retry interval and the final timeout. The contract uses >=,
    // so exact ACK_TIMEOUT_SECS intervals need no wall-clock delay or padding.
    let tick_base = tokio::time::Instant::now();
    let retry_interval = Duration::from_secs(u64::from(ACK_TIMEOUT_SECS));
    let unchanged_clock = fixture.transport.clock.time();
    for step in 1..=ACK_RESENDS + 2 {
        entry
            .controller
            .commands()
            .tick(tick_base + retry_interval * step);
        assert_eq!(
            entry.controller.commands().progress(command.id),
            Some(Progress::Accepted),
            "preserving the native ACK must also clear the refused resend's ACK timer"
        );
        assert_eq!(handle.progress(), Progress::Accepted);
        assert!(!entry.controller.commands().answered(command.id));
    }
    assert_eq!(fixture.transport.clock.time(), unchanged_clock);
    assert_eq!(
        fixture
            .transport
            .native
            .wire
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|sent| sent.id == command.id)
            .count(),
        1,
        "the refused resend never reaches the native peer"
    );
    let still_unmirrored = fixture
        .transport
        .dispatcher
        .status(command.id)
        .await?
        .ok_or("the native-accepted command disappeared during local refusal")?;
    assert!(matches!(still_unmirrored.state.as_str(), "queued" | "sent"));
    assert!(still_unmirrored.error.is_none());
    assert_eq!(still_unmirrored.deadline_at, before.deadline_at);
    assert_eq!(still_unmirrored.body, before.body);

    release_refusal
        .send(())
        .map_err(|_| "the local refusal gate ended before release")?;
    release_native
        .send(())
        .map_err(|_| "the native login gate ended before release")?;
    until(|| matches!(handle.progress(), Progress::Succeeded(_))).await?;
    let completed = fixture
        .transport
        .server
        .entry(command.id)
        .ok_or("the completed native login lost its actual journal entry")?;
    let result = completed
        .result()
        .ok_or("controller success preceded the native journal result")?;
    assert_eq!(result.id, command.id);
    let native_body = result
        .outcome
        .as_ref()
        .map_err(|_| "the owned native login did not succeed")?;
    assert_eq!(native_body, &json!({}));
    assert_eq!(handle.progress(), Progress::Succeeded(native_body.clone()));
    // The result's production ingress may mirror success independently. Keep the
    // original Accepted mirror paused until its actual controller result is proven,
    // then verify that releasing the stale write cannot replace the native result.
    release_mirror
        .send(())
        .map_err(|_| "the Accepted mirror gate ended before release")?;
    assert!(matches!(
        within(fixture.transport.dispatcher.wait(command.id)).await??,
        CommandOutcome::Succeeded(ref body) if body == native_body
    ));
    fixture.transport.completed_probe().await?;
    let saved = fixture
        .transport
        .dispatcher
        .status(command.id)
        .await?
        .ok_or("the completed login lost its durable command result")?;
    assert_eq!(saved.state, "succeeded");
    assert_eq!(saved.result.as_ref(), Some(native_body));
    assert!(saved.error.is_none());
    assert_eq!(saved.deadline_at, before.deadline_at);
    assert_eq!(saved.body, command.body);
    assert_eq!(
        fixture
            .transport
            .native
            .wire
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|sent| sent.id == command.id)
            .count(),
        1
    );
    assert_eq!(fixture.transport.server.entry(command.id), Some(completed));
    fixture.transport.close().await?;
    Ok(())
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_a_native_login_ack_before_its_database_mirror_survives_a_later_local_authority_refusal()
-> ::futures::future::LocalBoxFuture<'static, crate::backplane::command_admission_tests::Proof> {
    ::std::boxed::Box::pin(self::a_native_login_ack_before_its_database_mirror_survives_a_later_local_authority_refusal())
}

#[path = "provider_handover.rs"]
pub mod provider_handover;
