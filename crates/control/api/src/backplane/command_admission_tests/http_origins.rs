//! Actual raw authenticators, typed Host source, durable dispatcher and file-backed native journal.
//! This tests the shared carrier seam; owning families supply their complete endpoint controls.
use super::*;
use crate::{
    key_operations::native::http_origin as adapter,
    middleware::{authentication, authorization},
    registry::{Ctx, Registry},
    testing::TestApp,
};
use orch_access_contract::{
    decisions::ObjectRef,
    key_scope::{KeyAction, KeyOperation, KeyReach, KeyRule, KeyScope, KeyTarget, KeyTargetKind},
    permissions::ObjectKind,
};
use orch_api_contract::{resources::HostGet, routes::Endpoint, surface::HttpSurface};
use orch_authz::{CapturedOperation, ownership};
use orch_db::queries::{access, identity};
use orch_engine::{
    commands::{CommandAuthority, HostPortsProbe, Outgoing},
    operation_scope::{http_origin::HttpOrigin, request_origin::Credential, targets},
};
use orch_link_contract::ports::{HostPortsCheck, HostPortsCheckResult};
use orch_types::Json;

pub(super) fn reply(
    native: &Native,
    context: CommandContext,
    body: HostPortsCheck,
) -> Result<CommandResult, CommandError> {
    if context.key != SerialKey::Machine.wire() || body.ports.is_some() || body.revision != 0 {
        return Err(CommandError::new(
            ErrorCode::Conflict,
            "not the actual native proposal read",
        ));
    }
    native.reads.fetch_add(1, Ordering::SeqCst);
    let limits = orch_api_contract::ports::HostPortsLimits {
        incoming_address: None,
        listen: None,
        ranges: Some(vec![]),
        revision: 1,
    };
    Ok(CommandResult::HostPorts(HostPortsCheckResult {
        revision: 0,
        accepted: true,
        proposal: orch_api_contract::ports::HostPortsProposal {
            values: orch_api_contract::ports::HostPortsProposalValues {
                incoming_address: None,
                listen: "all".into(),
                ranges: vec![],
            },
            sources: orch_api_contract::ports::HostPortsProposalSources {
                incoming_address: "local observation".into(),
                listen: "local ceiling".into(),
                ranges: "no free block".into(),
            },
            as_of: native.clock.time(),
            reason: None,
            limits: limits.clone(),
        },
        limits,
        reason: None,
    }))
}
#[derive(Clone, Copy)]
enum Mode {
    Key,
    Session(bool),
}
struct HttpFixture {
    transport: Fixture,
    _app: TestApp,
    ctx: Ctx,
    host: Id,
    owner: Id,
    id: Id,
    bearer: String,
    digest: Vec<u8>,
    mode: Mode,
    raw: HttpOrigin,
    command: Outgoing,
}
fn scope(kind: ObjectKind, id: Id, reach: KeyReach) -> KeyScope {
    KeyScope {
        rules: vec![KeyRule {
            target: KeyTarget {
                kind: KeyTargetKind::Object(kind),
                id: Some(id),
                reach,
            },
            resources: None,
            allow: vec![KeyOperation {
                operation: "HostGet".into(),
                actions: vec![KeyAction::Read],
            }],
        }],
        ..KeyScope::default()
    }
}
impl HttpFixture {
    async fn new(mode: Mode) -> Proof<Self> {
        let transport = Fixture::new(false).await?;
        let app = TestApp::new(
            transport.link.inner.db.clone(),
            transport.clock.clone(),
            Registry::new(),
        );
        let mut conn = transport.test.pool.acquire().await?;
        let (owner, org): (Id, Id) =
            sqlx::query_as("SELECT owner_id,org_id FROM environments WHERE id=$1")
                .bind(transport.environment)
                .fetch_one(&mut *conn)
                .await?;
        access::testing::insert_member(&mut conn, org, owner, org).await?;
        access::testing::name_owner(&mut conn, org, ("user", owner), transport.clock.now()).await?;
        let host = new_id();
        resources::insert_host(
            &mut conn,
            &resources::NewHost {
                id: host,
                machine_id: transport.machine,
                owner_id: Some(owner),
                home_id: Some(org),
                name: "Original HTTP native source",
            },
            transport.clock.now(),
        )
        .await?;
        ownership::start_resource(
            &mut conn,
            ObjectRef {
                kind: ObjectKind::Host,
                id: host,
            },
            owner,
            ownership::NewAccess::OnlyMe,
            transport.clock.time(),
        )
        .await?;
        let id = new_id();
        let bearer = authentication::new_token(match mode {
            Mode::Key => authentication::API_TOKEN_PREFIX,
            Mode::Session(_) => authentication::ACCESS_TOKEN_PREFIX,
        });
        let digest = authentication::token_hash(&bearer);
        let session_end = transport.clock.now() + chrono::Duration::hours(1);
        match mode {
            Mode::Key => {
                let policy = scope(ObjectKind::Org, org, KeyReach::Subtree);
                identity::create_api_token_scoped(
                    &mut conn,
                    &identity::NewApiToken {
                        id,
                        user_id: owner,
                        name: "Raw native HTTP key",
                        token_hash: &digest,
                        scopes: &[],
                        node_id: None,
                        at: transport.clock.now(),
                        expires_at: Some(session_end),
                    },
                    Some(&json!(policy)),
                )
                .await?;
            }
            Mode::Session(admin) => {
                sqlx::query("UPDATE users SET is_system_admin=true WHERE id=$1")
                    .bind(owner)
                    .execute(&mut *conn)
                    .await?;
                identity::testing::insert_session(&mut conn, id, owner, &digest, session_end)
                    .await?;
                sqlx::query("UPDATE sessions SET as_admin=$2 WHERE id=$1")
                    .bind(id)
                    .bind(admin)
                    .execute(&mut *conn)
                    .await?;
                // The raw access token ends first, strictly inside the native kind's 30s allowance.
                sqlx::query("UPDATE access_tokens SET expires_at=$2 WHERE token_hash=$1")
                    .bind(&digest)
                    .bind(transport.clock.now() + chrono::Duration::seconds(5))
                    .execute(&mut *conn)
                    .await?;
            }
        }
        drop(conn);
        let surface = match mode {
            Mode::Key => HttpSurface::Key,
            Mode::Session(_) => HttpSurface::Browser,
        };
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::AUTHORIZATION,
            http::HeaderValue::from_str(&format!("Bearer {bearer}"))?,
        );
        let authenticated =
            authentication::authenticate_on(surface, &app.app, &headers, transport.clock.now())
                .await?;
        let authentication::Caller::User(user) = &authenticated else {
            return Err("the real raw authenticator returned no user".into());
        };
        let caller = authorization::caller_of(user)?;
        let mut ctx = Ctx::for_tool(app.app.clone(), authenticated.clone(), HostGet::ROUTE, host);
        // This native-seam fixture supplies the exact context of the real authenticator above.
        ctx.surface = Some(surface);
        ctx.headers = headers;
        let command = Outgoing::new(
            new_id(),
            transport.machine,
            SerialKey::Machine,
            &HostPortsCheck {
                ports: None,
                revision: 0,
            },
        )?;
        let mut tx = transport.test.pool.begin().await?;
        let session = match mode {
            Mode::Session(_) => Some(id),
            Mode::Key => None,
        };
        let native = HostPortsProbe::capture(
            &mut tx,
            &caller,
            session,
            Some(Time::new(session_end)),
            host,
            &command,
            &transport.clock,
        )
        .await?;
        let raw = adapter::capture_operation(&ctx, &mut tx, native.origin()).await?;
        let authority = adapter::bind(&raw, &command, CommandAuthority::host_ports_probe(native))?;
        let encoded = serde_json::to_value(&authority)?;
        assert!(!encoded.to_string().contains(&bearer));
        assert!(!format!("{raw:?}").contains(&bearer));
        let authority: CommandAuthority = serde_json::from_value(encoded)?;
        tx.commit().await?;
        Ok(Self {
            transport,
            _app: app,
            ctx,
            host,
            owner,
            id,
            bearer,
            digest,
            mode,
            raw,
            command: command.with_authority(authority),
        })
    }
    fn stored(&self) -> StoredCommand {
        StoredCommand {
            id: Some(self.command.id),
            machine: self.command.machine,
            kind: self.command.kind.as_str().into(),
            key: self.command.key.wire(),
            deadline: self.command.deadline,
            body: self.command.body.clone(),
            authority: self.command.authority.clone(),
        }
    }
    async fn before(&self) -> Proof<orch_engine::commands::BeforeCommand<'static>> {
        let mut tx = self.transport.test.pool.begin().await?;
        let current = self.raw.current(&mut tx, &self.transport.clock).await?;
        tx.commit().await?;
        let clock = self.transport.link.inner.clock.clone();
        Ok(Box::new(move || {
            current
                .check(clock.as_ref())
                .map_err(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>)
        }))
    }
    async fn rotate(&self) -> Proof {
        let mut tx = self.transport.test.pool.begin().await?;
        match self.mode {
            Mode::Key => {
                sqlx::query("UPDATE api_tokens SET token_hash=$2 WHERE id=$1")
                    .bind(self.id)
                    .bind(authentication::token_hash("a different owned test bearer"))
                    .execute(&mut *tx)
                    .await?;
            }
            Mode::Session(_) => {
                sqlx::query("DELETE FROM access_tokens WHERE token_hash=$1")
                    .bind(&self.digest)
                    .execute(&mut *tx)
                    .await?;
                identity::add_access_token(
                    &mut tx,
                    &authentication::token_hash("a different access token of this same session"),
                    self.id,
                    self.transport.clock.now() + chrono::Duration::hours(1),
                )
                .await?;
            }
        }
        tx.commit().await?;
        Ok(())
    }
}

#[tokio::test]
async fn raw_http_queue_checks_the_original_digest_after_the_real_entry_wait() -> Proof {
    for mode in [Mode::Key, Mode::Session(false)] {
        for rotate in [false, true] {
            let mut s = HttpFixture::new(mode).await?;
            let (reached, release) = gate(s.transport.machine, Phase::Entry);
            let dispatcher = s.transport.dispatcher.clone();
            let command = s.stored();
            let before = s.before().await?;
            let task =
                tokio::spawn(async move { dispatcher.issue_stored_before(command, before).await });
            within(reached).await??;
            if rotate {
                s.rotate().await?;
            }
            release.send(()).map_err(|_| "entry gate ended")?;
            let issued = within(task).await??;
            let mut conn = s.transport.test.pool.acquire().await?;
            if rotate {
                assert!(issued.is_err());
                assert!(rows::command(&mut conn, s.command.id).await?.is_none());
                assert_eq!(s.transport.native.reads.load(Ordering::SeqCst), 0);
                drop(conn);
            } else {
                issued?;
                drop(conn);
                within(s.transport.connect(false)).await??;
                assert!(matches!(
                    within(s.transport.dispatcher.wait(s.command.id)).await??,
                    CommandOutcome::Succeeded(_)
                ));
                assert_eq!(s.transport.native.reads.load(Ordering::SeqCst), 1);
            }
            s.transport.close().await?;
        }
    }
    Ok(())
}

#[tokio::test]
async fn raw_http_redelivery_refuses_rotated_bearers_and_changed_physical_sources() -> Proof {
    for change in ["live", "hash", "source", "scope", "null"] {
        let mut s = HttpFixture::new(Mode::Key).await?;
        s.transport
            .dispatcher
            .issue_stored_before(s.stored(), s.before().await?)
            .await?;
        match change {
            "hash" => s.rotate().await?,
            "source" => {
                let mut conn = s.transport.test.pool.acquire().await?;
                let other = resources::testing::insert_machine(
                    &mut conn,
                    "another physical custodian",
                    "connected",
                )
                .await?;
                sqlx::query("UPDATE hosts SET machine_id=$2 WHERE id=$1")
                    .bind(s.host)
                    .bind(other)
                    .execute(&mut *conn)
                    .await?;
            }
            "scope" => {
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(s.id)
                    .bind(json!({"rules":[]}))
                    .execute(&s.transport.test.pool)
                    .await?;
            }
            "null" => {
                sqlx::query("UPDATE commands SET login_authority=jsonb_set(login_authority,'{authority,accepted}','null') WHERE id=$1").bind(s.command.id).execute(&s.transport.test.pool).await?;
            }
            _ => {}
        }
        within(s.transport.connect(false)).await??;
        let result = within(s.transport.dispatcher.wait(s.command.id)).await??;
        assert_eq!(
            matches!(result, CommandOutcome::Succeeded(_)),
            change == "live",
            "{change}"
        );
        assert_eq!(
            s.transport.native.reads.load(Ordering::SeqCst),
            usize::from(change == "live"),
            "{change}"
        );
        for wire in s
            .transport
            .native
            .wire
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
        {
            assert!(!serde_json::to_string(wire)?.contains(&s.bearer));
            assert!(wire.body.get("authority").is_none());
        }
        s.transport.close().await?;
    }
    Ok(())
}

#[tokio::test]
async fn raw_http_access_deadline_survives_extension_and_is_checked_after_the_actual_wire_wait()
-> Proof {
    for expire in [false, true] {
        let mut s = HttpFixture::new(Mode::Session(false)).await?;
        sqlx::query("UPDATE access_tokens SET expires_at=$2 WHERE token_hash=$1")
            .bind(&s.digest)
            .bind(s.transport.clock.now() + chrono::Duration::hours(1))
            .execute(&s.transport.test.pool)
            .await?;
        let (reached, release) = gate(s.command.id, Phase::Wire);
        s.transport
            .dispatcher
            .issue_stored_before(s.stored(), s.before().await?)
            .await?;
        within(s.transport.connect(false)).await??;
        within(reached).await??;
        if expire {
            s.transport.clock.advance(chrono::Duration::seconds(6));
        }
        release.send(()).map_err(|_| "wire gate ended")?;
        let result = within(s.transport.dispatcher.wait(s.command.id)).await??;
        assert_eq!(matches!(result, CommandOutcome::Succeeded(_)), !expire);
        assert_eq!(
            s.transport.native.reads.load(Ordering::SeqCst),
            usize::from(!expire)
        );
        s.transport.close().await?;
    }
    Ok(())
}

#[tokio::test]
async fn raw_http_explicit_mode_is_a_ceiling_and_identity_delivery_does_not_rerun_a_completed_source()
-> Proof {
    for original_admin in [false, true] {
        let mut s = HttpFixture::new(Mode::Session(original_admin)).await?;
        sqlx::query("UPDATE sessions SET as_admin=$2 WHERE id=$1")
            .bind(s.id)
            .bind(!original_admin)
            .execute(&s.transport.test.pool)
            .await?;
        let mut tx = s.transport.test.pool.begin().await?;
        let restored = s.raw.current(&mut tx, &s.transport.clock).await;
        assert!(
            !restored?.caller().as_admin(),
            "neither a newly enabled mode nor a disabled original mode can lend privilege"
        );
        tx.rollback().await?;
        if !original_admin {
            s.transport
                .dispatcher
                .issue_stored_before(s.stored(), s.before().await?)
                .await?;
            within(s.transport.connect(false)).await??;
            assert!(matches!(
                within(s.transport.dispatcher.wait(s.command.id)).await??,
                CommandOutcome::Succeeded(_)
            ));
            // This actual source may be gone after the command completes. Identity delivery
            // remains separate; a full read still checks its own current source.
            sqlx::query("DELETE FROM hosts WHERE id=$1")
                .bind(s.host)
                .execute(&s.transport.test.pool)
                .await?;
            let mut tx = s.transport.test.pool.begin().await?;
            let delivered = s
                .command
                .authority
                .as_ref()
                .ok_or("HTTP source missing")?
                .http_delivery(&mut tx, &s.transport.clock, s.command.id)
                .await?;
            assert!(
                !adapter::current(&s.ctx, &mut tx, &delivered)
                    .await?
                    .caller()
                    .as_admin()
            );
            tx.commit().await?;
            s.rotate().await?;
            let mut tx = s.transport.test.pool.begin().await?;
            assert!(
                s.command
                    .authority
                    .as_ref()
                    .ok_or("HTTP source missing")?
                    .http_delivery(&mut tx, &s.transport.clock, s.command.id)
                    .await
                    .is_err()
            );
        }
        s.transport.close().await?;
    }
    Ok(())
}

#[tokio::test]
async fn raw_http_retains_an_accepted_scope_minimum_and_keeps_it_on_the_shared_response_opening()
-> Proof {
    let s = HttpFixture::new(Mode::Key).await?;
    s.transport
        .dispatcher
        .issue_stored_before(s.stored(), s.before().await?)
        .await?;
    let broad: Json = sqlx::query_scalar("SELECT operation_scope FROM api_tokens WHERE id=$1")
        .bind(s.id)
        .fetch_one(&s.transport.test.pool)
        .await?;
    let narrow = scope(ObjectKind::Host, s.host, KeyReach::SelfOnly);
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(s.id)
        .bind(json!(narrow))
        .execute(&s.transport.test.pool)
        .await?;
    let mut tx = s.transport.test.pool.begin().await?;
    let private = rows::lock_command_with_authority(&mut tx, s.command.id)
        .await?
        .ok_or("private command missing")?;
    let authority: CommandAuthority =
        serde_json::from_value(private.authority.ok_or("private evidence missing")?)?;
    let calls = authority
        .current_callers(
            &mut tx,
            &private.command,
            &s.transport.clock,
            &crate::thread_access::AuthzAccess::new(s.transport.link.inner.clock.clone()),
        )
        .await?;
    assert!(calls.is_some());
    tx.commit().await?;
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(s.id)
        .bind(broad)
        .execute(&s.transport.test.pool)
        .await?;
    let mut tx = s.transport.test.pool.begin().await?;
    let delivered = s
        .command
        .authority
        .as_ref()
        .ok_or("HTTP source missing")?
        .http_delivery(&mut tx, &s.transport.clock, s.command.id)
        .await?;
    let restored = adapter::current(&s.ctx, &mut tx, &delivered).await?;
    assert_eq!(restored.caller().user(), s.owner);
    let org: Id = sqlx::query_scalar("SELECT org_id FROM hosts WHERE id=$1")
        .bind(s.host)
        .fetch_one(&mut *tx)
        .await?;
    let unrelated = new_id();
    // A second real resource under the same original broad root is outside the accepted narrower key.
    let machine =
        resources::testing::insert_machine(&mut tx, "unrelated original-root host", "connected")
            .await?;
    resources::insert_host(
        &mut tx,
        &resources::NewHost {
            id: unrelated,
            machine_id: machine,
            owner_id: Some(s.owner),
            home_id: Some(org),
            name: "Unrelated host",
        },
        s.transport.clock.now(),
    )
    .await?;
    let target = targets::resource_identity(
        &mut tx,
        ObjectRef {
            kind: ObjectKind::Host,
            id: unrelated,
        },
    )
    .await?;
    let facet = orch_access_contract::key_scope::KeyFacet {
        operation: "HostGet".into(),
        action: KeyAction::Read,
        resource_kind: KeyTargetKind::Object(ObjectKind::Host),
        target,
    };
    assert!(
        !restored
            .caller()
            .permits_facets(std::slice::from_ref(&facet))
    );
    let again = crate::middleware::transaction_guard::current_http_identity(
        &s.ctx,
        &mut tx,
        crate::middleware::transaction_guard::HttpIdentityMode::Read,
    )
    .await?;
    assert!(
        !again.caller.permits_facets(&[facet]),
        "the final shared projector cannot borrow the later broader key"
    );
    tx.commit().await?;
    s.transport.close().await
}

#[tokio::test]
async fn raw_http_modern_codec_rejects_missing_identity_and_a_native_caller_is_not_a_bearer()
-> Proof {
    let s = HttpFixture::new(Mode::Key).await?;
    let encoded = serde_json::to_value(s.command.authority.as_ref().ok_or("HTTP source missing")?)?;
    for member in ["original", "accepted", "native", "canonical"] {
        let mut corrupt = encoded.clone();
        corrupt["authority"]
            .as_object_mut()
            .ok_or("private packet is not an object")?
            .remove(member);
        assert!(
            serde_json::from_value::<CommandAuthority>(corrupt).is_err(),
            "required modern member {member}"
        );
    }
    let origin: CapturedOperation = s.raw.origin().clone();
    assert!(HttpOrigin::capture(origin.clone(), Credential::Native, false, false, None).is_err());
    assert!(
        HttpOrigin::capture(
            origin,
            Credential::Token {
                id: s.id,
                hash: s.digest.clone()
            },
            true,
            true,
            None
        )
        .is_err()
    );
    s.transport.close().await
}

#[tokio::test]
async fn a_native_http_deadline_keeps_its_actual_microseconds_across_issue_wire_and_receipt()
-> Proof {
    for mode in [Mode::Key, Mode::Session(false)] {
        let mut s = HttpFixture::new(mode).await?;
        // An offset that is not a whole millisecond makes an ordinary DB timestamp exercise
        // the private identity; truncating it is neither the same frame nor a valid retime.
        s.transport
            .clock
            .advance(chrono::Duration::microseconds(137));
        s.transport
            .dispatcher
            .issue_stored_before(s.stored(), s.before().await?)
            .await?;
        let mut tx = s.transport.test.pool.begin().await?;
        let stored = orch_db::queries::link::command_with_authority(&mut tx, s.command.id)
            .await?
            .ok_or("actual issued source absent")?;
        assert_ne!(
            Time::new(stored.command.deadline_at).get(),
            stored.command.deadline_at
        );
        let authority: CommandAuthority =
            serde_json::from_value(stored.authority.ok_or("actual HTTP source absent")?)?;
        let mut changed = stored.command.clone();
        changed.deadline_at += chrono::Duration::microseconds(1);
        let access = crate::thread_access::AuthzAccess::new(s.transport.link.inner.clock.clone());
        assert!(
            authority
                .current_callers(&mut tx, &changed, &s.transport.clock, &access)
                .await?
                .is_none(),
            "a one-microsecond changed frame is still refused"
        );
        tx.rollback().await?;
        within(s.transport.connect(false)).await??;
        let result = within(s.transport.dispatcher.wait(s.command.id)).await??;
        assert!(
            matches!(result, CommandOutcome::Succeeded(_)),
            "the same genuine microsecond frame must execute"
        );
        assert_eq!(s.transport.native.reads.load(Ordering::SeqCst), 1);
        let mut tx = s.transport.test.pool.begin().await?;
        let (_, current) = adapter::delivery(
            &s.ctx,
            &mut tx,
            s.command
                .authority
                .as_ref()
                .ok_or("original HTTP frame absent")?,
            s.command.id,
        )
        .await?;
        current.check(&s.transport.clock)?;
        tx.commit().await?;
        s.transport.close().await?;
    }
    Ok(())
}
