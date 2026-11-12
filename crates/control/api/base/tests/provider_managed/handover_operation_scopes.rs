//! Handover-return HTTP acceptance controls using the existing scripted machine.
//! Keys bound the same ordinary ProviderManage OR actual-holder EnvManage paths.

use super::*;
use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_db::queries::identity;
use orch_engine::commands::Commands;

const PROVIDER_RETURN: &str = "ProviderHandoverReturn.provider";
const HOLDER_RETURN: &str = "ProviderHandoverReturn.holding_environment";
// The synthetic credential supplied by the shared scripted-machine responder.
const RETURNED_CREDENTIAL: &str = "c2VhbGVkLWJ5LXRoZS1lbnZpcm9ubWVudA";

async fn revoke_key(s: &Scene, token: &str) {
    let id: Id = sqlx::query_scalar("SELECT id FROM api_tokens WHERE token_hash=$1")
        .bind(token_hash(token))
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert!(
        identity::revoke_api_token(
            &mut *s.rig.world.conn().await,
            id,
            s.ada.id(),
            s.rig.world.now()
        )
        .await
        .unwrap()
    );
}

async fn handler_started(s: &Scene, kind: K, provider: Id, token: &str, selection: &str) {
    let (token_id, user): (Id, Id) =
        sqlx::query_as("SELECT id,user_id FROM api_tokens WHERE token_hash=$1")
            .bind(token_hash(token))
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    eventually(
        "this key's exact accepted handover phase is executing, not an automatic return",
        || async {
            for execution in s
                .rig
                .machine
                .handler
                .executions()
                .into_iter()
                .filter(|entry| entry.kind == kind)
            {
                let Some(authority) = s.rig.commands.issued_authority(execution.id) else {
                    continue;
                };
                // The HTTP Rig issues directly to its in-memory ControllerLink. Match
                // the immutable actual Outgoing envelope, not an assumed database row.
                let authority = serde_json::to_value(authority).unwrap();
                let Some(body) = s.rig.commands.retained_body(execution.id).await.unwrap() else {
                    continue;
                };
                let handover = &authority["source"]["authority"];
                let origin = &handover["origin"];
                let caller = &origin["origin"]["caller"];
                let own_key = caller["credentials"].as_array().is_some_and(|credentials| {
                    credentials.iter().any(|credential| {
                        credential["type"] == "token"
                            && credential["token_ids"]
                                .as_array()
                                .is_some_and(|ids| ids.contains(&json!(token_id)))
                    })
                });
                if body["provider_id"] == json!(provider)
                    && authority["provider"] == json!(provider)
                    && authority["source"]["kind"] == "handover"
                    && handover["command"] == json!(execution.id)
                    && handover["source"]["provider"] == json!(provider)
                    && origin["provider"] == json!(provider)
                    && origin["origin"]["operation"] == selection
                    && caller["user"] == json!(user)
                    && own_key
                {
                    return true;
                }
            }
            false
        },
    )
    .await;
}

fn rule(kind: &str, id: Id, operation: &str, actions: &[&str]) -> Json {
    json!({
        "target":{"kind":kind,"id":id,"reach":"self"},
        "allow":[{"operation":operation,"actions":actions}]
    })
}

fn holder_rules(s: &Scene, provider: Id, actions: &[&str]) -> Vec<Json> {
    vec![
        rule("provider", provider, HOLDER_RETURN, actions),
        rule("environment", s.on.environment, HOLDER_RETURN, &["manage"]),
    ]
}

async fn key(s: &Scene, user: Id, rules: Vec<Json>) -> String {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    let hash = token_hash(&secret);
    identity::create_api_token_scoped(
        &mut *s.rig.world.conn().await,
        &identity::NewApiToken {
            id: new_id(),
            user_id: user,
            name: "Handover-return operation control",
            token_hash: &hash,
            scopes: &[],
            node_id: None,
            at: s.rig.world.now(),
            // Same fixed-clock ordinary lifetime as the other operation-key fixtures.
            expires_at: Some(s.rig.world.now() + chrono::Duration::hours(1)),
        },
        Some(&serde_json::to_value(scope).unwrap()),
    )
    .await
    .unwrap();
    secret
}

async fn bring(s: &Scene, token: &str, provider: Id, declare_lost: bool) -> RawReply {
    s.rig
        .world
        .app
        .call(ProviderHandoverReturn, &[&provider])
        .token(token)
        .raw_body(&json!({"interrupt":false,"declare_lost":declare_lost}))
        .send_bytes()
        .await
}

fn shown(reply: RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    let text = String::from_utf8_lossy(&reply.raw);
    assert!(!text.contains(RETURNED_CREDENTIAL));
    assert!(!text.contains("\"$sealed\""));
    serde_json::from_slice(&reply.raw).unwrap()
}

fn full(reply: RawReply, provider: Id) -> Json {
    let value = shown(reply);
    assert!(value.get("projection").is_none(), "{value}");
    assert_eq!(value["id"], json!(provider));
    assert!(value["held_by"].is_null());
    value
}

fn receipt(reply: RawReply, provider: Id) -> Json {
    let value = shown(reply);
    assert_eq!(value["projection"], "receipt");
    assert_eq!(value["provider_id"], json!(provider));
    for field in [
        "id",
        "name",
        "home",
        "owner",
        "facts",
        "runtime",
        "runtime_config",
        "upstreams",
        "secrets_set",
        "held_by",
    ] {
        assert!(
            value.get(field).is_none(),
            "receipt disclosed {field}: {value}"
        );
    }
    value
}

async fn custody(s: &Scene, provider: Id) -> Json {
    sqlx::query_scalar(
        "SELECT jsonb_build_object('held_by',held_by_environment_id,'held_since',held_since,
            'status',status,'login_state',login_state,'login_pending',login_pending,
            'native_commands',(SELECT count(*) FROM commands
                WHERE kind IN ('agent.credential.return','provider.credential.accept')
                AND body->>'provider_id'=$1::uuid::text),
            'accepted_requests',(SELECT count(*) FROM audit_log
                WHERE details->>'provider'=$1::uuid::text AND details->>'decision'='accepted'
                AND details->>'operation'='bring the credential home on'))
        FROM providers WHERE id=$1",
    )
    .bind(provider)
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap()
}

fn native_counts(heard: &Heard) -> (usize, usize) {
    (
        asked(heard, K::AgentCredentialReturn).len(),
        asked(heard, K::ProviderCredentialAccept).len(),
    )
}

async fn denied_without_effects(
    s: &Scene,
    heard: &Heard,
    token: &str,
    provider: Id,
    declare_lost: bool,
) {
    let before = custody(s, provider).await;
    let commands = native_counts(heard);
    let reply = bring(s, token, provider, declare_lost).await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN, "{:?}", reply);
    assert_eq!(reply.problem().code, ErrorCode::AuthzDenied);
    assert_eq!(custody(s, provider).await, before);
    assert_eq!(native_counts(heard), commands);
}

async fn assert_returned(s: &Scene, provider: Id, heard: &Heard, count: usize) {
    assert!(custody(s, provider).await["held_by"].is_null());
    assert_eq!(native_counts(heard), (count, count));
    for accepted in asked(heard, K::ProviderCredentialAccept) {
        assert_eq!(accepted["credential"]["$sealed"], RETURNED_CREDENTIAL);
    }
}

#[tokio::test]
async fn key_loss_after_return_keeps_moved_material_and_a_fresh_manager_can_finish_it() {
    let s = scene().await;
    let provider = managed_and_up(&s).await;
    let (heard, _) = the_environment_gives_back(&s);
    handed_over(&s, provider).await;
    let token = key(
        &s,
        s.ada.id(),
        vec![rule(
            "provider",
            provider,
            PROVIDER_RETURN,
            &["manage", "read"],
        )],
    )
    .await;
    s.rig.machine.handler.hold(K::AgentCredentialReturn);
    let (failed, ()) = tokio::join!(bring(&s, &token, provider, false), async {
        handler_started(
            &s,
            K::AgentCredentialReturn,
            provider,
            &token,
            PROVIDER_RETURN,
        )
        .await;
        revoke_key(&s, &token).await;
        s.rig.machine.handler.release();
    });
    assert_eq!(failed.problem().code, ErrorCode::CredentialReturnFailed);
    assert_eq!(
        native_counts(&heard),
        (1, 0),
        "native export is real; no new accept is authorized"
    );
    assert_eq!(
        custody(&s, provider).await["held_by"],
        json!(s.on.environment)
    );
    full(bring(&s, &s.ada.token, provider, false).await, provider);
    assert_eq!(
        native_counts(&heard),
        (1, 1),
        "a fresh valid manager resumes the same ciphertext, without exporting it twice"
    );
    assert!(custody(&s, provider).await["held_by"].is_null());
}

#[tokio::test]
async fn a_completed_native_accept_is_reconciled_after_key_loss_with_only_a_receipt() {
    let s = scene().await;
    let provider = managed_and_up(&s).await;
    let (heard, _) = the_environment_gives_back(&s);
    handed_over(&s, provider).await;
    let token = key(
        &s,
        s.ada.id(),
        vec![rule(
            "provider",
            provider,
            PROVIDER_RETURN,
            &["manage", "read"],
        )],
    )
    .await;
    s.rig.machine.handler.hold(K::ProviderCredentialAccept);
    let (done, ()) = tokio::join!(bring(&s, &token, provider, false), async {
        handler_started(
            &s,
            K::ProviderCredentialAccept,
            provider,
            &token,
            PROVIDER_RETURN,
        )
        .await;
        revoke_key(&s, &token).await;
        s.rig.machine.handler.release();
    });
    receipt(done, provider);
    assert_eq!(native_counts(&heard), (1, 1));
    assert!(
        custody(&s, provider).await["held_by"].is_null(),
        "the actual completed native outcome is recorded"
    );
    let returned: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action='provider.returned' AND object_id=$1",
    )
    .bind(provider)
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(returned, 1);
}

#[tokio::test]
async fn completed_manage_effects_are_not_required_again_for_the_current_response_read() {
    let s = scene().await;
    let provider = managed_and_up(&s).await;
    let (heard, _) = the_environment_gives_back(&s);
    handed_over(&s, provider).await;
    let token = key(
        &s,
        s.ada.id(),
        vec![rule(
            "provider",
            provider,
            PROVIDER_RETURN,
            &["manage", "read"],
        )],
    )
    .await;
    s.rig.machine.handler.hold(K::ProviderCredentialAccept);
    let (done, ()) = tokio::join!(bring(&s, &token, provider, false), async {
        handler_started(
            &s,
            K::ProviderCredentialAccept,
            provider,
            &token,
            PROVIDER_RETURN,
        )
        .await;
        let scope: KeyScope = serde_json::from_value(
            json!({"rules":[rule("provider", provider, PROVIDER_RETURN, &["read"])]}),
        )
        .unwrap();
        sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE token_hash=$1")
            .bind(token_hash(&token))
            .bind(serde_json::to_value(scope).unwrap())
            .execute(s.rig.db.pool())
            .await
            .unwrap();
        s.rig.machine.handler.release();
    });
    full(done, provider);
    assert_eq!(native_counts(&heard), (1, 1));
    assert!(custody(&s, provider).await["held_by"].is_null());
}

#[tokio::test]
async fn automatic_return_ciphertext_can_be_recovered_by_a_fresh_authorized_person() {
    let s = scene().await;
    let provider = managed_and_up(&s).await;
    let (heard, _) = the_environment_gives_back(&s);
    let fail_once = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let replies = Arc::clone(&heard);
    s.rig
        .machine
        .handler
        .on(K::ProviderCredentialAccept, move |command| {
            lock(&replies).push((K::ProviderCredentialAccept, command.body().unwrap()));
            if fail_once.swap(false, std::sync::atomic::Ordering::SeqCst) {
                Err(CommandError::new(
                    Wire::OfflinePeer,
                    "synthetic keeper refused after return",
                ))
            } else {
                Ok(CommandResult::ProviderCredentialAccept(Empty {}))
            }
        });
    handed_over(&s, provider).await;
    assert!(
        s.rig
            .engine
            .return_credential(
                provider,
                orch_engine::credentials::handover::ReturnWhy::Engine
            )
            .await
            .is_err()
    );
    assert_eq!(native_counts(&heard), (1, 1));
    full(bring(&s, &s.ada.token, provider, false).await, provider);
    assert_eq!(
        native_counts(&heard),
        (1, 2),
        "recovery keeps the actual system return source and does not export again"
    );
    assert!(custody(&s, provider).await["held_by"].is_null());
}

#[tokio::test]
async fn a_return_result_does_not_clear_a_newer_custody_on_the_same_environment() {
    let s = scene().await;
    let provider = managed_and_up(&s).await;
    let (heard, _) = the_environment_gives_back(&s);
    handed_over(&s, provider).await;
    let token = key(
        &s,
        s.ada.id(),
        vec![rule("provider", provider, PROVIDER_RETURN, &["manage"])],
    )
    .await;
    let next = s.rig.world.now() + chrono::Duration::seconds(1);
    s.rig.machine.handler.hold(K::AgentCredentialReturn);
    let (failed, ()) = tokio::join!(bring(&s, &token, provider, false), async {
        handler_started(
            &s,
            K::AgentCredentialReturn,
            provider,
            &token,
            PROVIDER_RETURN,
        )
        .await;
        sqlx::query("UPDATE providers SET held_since=$2 WHERE id=$1")
            .bind(provider)
            .bind(next)
            .execute(s.rig.db.pool())
            .await
            .unwrap();
        s.rig.machine.handler.release();
    });
    assert_eq!(failed.problem().code, ErrorCode::CredentialReturnFailed);
    assert_eq!(native_counts(&heard), (1, 0));
    let since: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT held_since FROM providers WHERE id=$1")
            .bind(provider)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(since, next);
    assert_eq!(
        custody(&s, provider).await["held_by"],
        json!(s.on.environment)
    );
}

#[tokio::test]
async fn an_accepted_variant_does_not_switch_but_a_fresh_request_can_choose_the_other_one() {
    for holder_first in [false, true] {
        let s = scene().await;
        let provider = managed_and_up(&s).await;
        let (heard, _) = the_environment_gives_back(&s);
        handed_over(&s, provider).await;
        let initial = if holder_first {
            holder_rules(&s, provider, &["manage"])
        } else {
            vec![rule("provider", provider, PROVIDER_RETURN, &["manage"])]
        };
        let token = key(&s, s.ada.id(), initial).await;
        s.rig.machine.handler.hold(K::AgentCredentialReturn);
        let (failed, ()) = tokio::join!(bring(&s, &token, provider, false), async {
            handler_started(
                &s,
                K::AgentCredentialReturn,
                provider,
                &token,
                if holder_first {
                    HOLDER_RETURN
                } else {
                    PROVIDER_RETURN
                },
            )
            .await;
            let next = if holder_first {
                vec![rule("provider", provider, PROVIDER_RETURN, &["manage"])]
            } else {
                holder_rules(&s, provider, &["manage"])
            };
            let scope: KeyScope = serde_json::from_value(json!({"rules":next})).unwrap();
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE token_hash=$1")
                .bind(token_hash(&token))
                .bind(serde_json::to_value(scope).unwrap())
                .execute(s.rig.db.pool())
                .await
                .unwrap();
            s.rig.machine.handler.release();
        });
        assert_eq!(failed.problem().code, ErrorCode::CredentialReturnFailed);
        assert_eq!(
            native_counts(&heard),
            (1, 0),
            "an in-progress request cannot change its accepted variant"
        );
        receipt(bring(&s, &token, provider, false).await, provider);
        assert_eq!(
            native_counts(&heard),
            (1, 1),
            "a new authorized request may select the remaining valid alternative and keep the physical source"
        );
    }
}

#[tokio::test]
async fn cancelling_an_owned_return_releases_its_gate_and_keeps_native_work_retryable() {
    let s = scene().await;
    let provider = managed_and_up(&s).await;
    let (heard, _) = the_environment_gives_back(&s);
    handed_over(&s, provider).await;
    let token = key(
        &s,
        s.ada.id(),
        vec![rule("provider", provider, PROVIDER_RETURN, &["manage"])],
    )
    .await;
    s.rig.machine.handler.hold(K::AgentCredentialReturn);
    let app = s.rig.world.app.clone();
    let observed_token = token.clone();
    let work = tokio::spawn(async move {
        let request = app
            .call(ProviderHandoverReturn, &[&provider])
            .token(&token)
            .raw_body(&json!({"interrupt":false,"declare_lost":false}));
        request.send_bytes().await
    });
    handler_started(
        &s,
        K::AgentCredentialReturn,
        provider,
        &observed_token,
        PROVIDER_RETURN,
    )
    .await;
    work.abort();
    assert!(work.await.unwrap_err().is_cancelled());
    s.rig.machine.handler.release();
    full(bring(&s, &s.ada.token, provider, false).await, provider);
    assert_eq!(
        native_counts(&heard),
        (1, 1),
        "the completed owned export is recovered rather than repeated or blocked by a leaked in-flight gate"
    );
    assert!(custody(&s, provider).await["held_by"].is_null());
}

#[tokio::test]
async fn busy_count_uses_its_own_read_bound_and_explicit_interrupt_keeps_existing_behavior() {
    let s = scene().await;
    let provider = managed_and_up(&s).await;
    let (heard, _) = the_environment_gives_back(&s);
    let thread = s
        .rig
        .world
        .app
        .call(ThreadCreate, &[&s.tree.topic.base.id])
        .token(&s.ada.token)
        .body(&CreateThread {
            name: Some("Owned handover busy control".into()),
            binding: Some(CreateThreadBinding {
                agent_id: s.on.agent,
                provider_id: provider,
                arrangement: None,
            }),
            model: Some(MODEL.into()),
            environment: Some(EnvironmentChoice::Use(s.on.environment)),
            directory: Some("web".into()),
            ..CreateThread::default()
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
    assert!(
        s.say(
            &s.ada,
            thread.base.id,
            "Owned running turn",
            Delivery::Queue,
            "handover-busy-1"
        )
        .await
        .status
        .is_success()
    );
    let turn = s.rig.turn_start(thread.base.id, 0).await;
    s.rig.say(thread.base.id, turn_began(turn.turn_id)).await;
    s.rig.wait_turn(turn.turn_id, "running").await;
    handed_over(&s, provider).await;
    let writer = key(
        &s,
        s.ada.id(),
        vec![rule("provider", provider, PROVIDER_RETURN, &["manage"])],
    )
    .await;
    let reader = key(
        &s,
        s.ada.id(),
        vec![rule(
            "provider",
            provider,
            PROVIDER_RETURN,
            &["manage", "read"],
        )],
    )
    .await;
    let hidden = bring(&s, &writer, provider, false).await.problem();
    assert_eq!(hidden.code, ErrorCode::ThreadNotIdle);
    assert!(
        hidden.extra.get("running").is_none(),
        "Manage does not disclose stored activity counts"
    );
    for who in [&reader, &s.ada.token] {
        let visible = bring(&s, who, provider, false).await.problem();
        assert_eq!(visible.code, ErrorCode::ThreadNotIdle);
        assert_eq!(visible.extra["running"], 1);
    }
    assert_eq!(native_counts(&heard), (0, 0));
    let call = s
        .rig
        .world
        .app
        .call(ProviderHandoverReturn, &[&provider])
        .token(&writer);
    let (interrupted, ()) = tokio::join!(
        call.raw_body(&json!({"interrupt":true,"declare_lost":false}))
            .send_bytes(),
        async {
            eventually("the owned turn interrupt has actually been issued", || async {
            s.rig.commands.issued_of("turn.interrupt") > 0 && s.rig.seen.of_kind(K::TurnInterrupt).iter().any(|command| matches!(command,
                Command::TurnInterrupt(body) if body.thread_id==thread.base.id && body.turn_id==turn.turn_id))
        }).await;
            // Match the existing provider stop/interrupt fixture's force-window advance.
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            s.rig.world.advance(chrono::Duration::seconds(10));
        }
    );
    receipt(interrupted, provider);
    assert_eq!(native_counts(&heard), (1, 1));
    assert!(custody(&s, provider).await["held_by"].is_null());
}

async fn holder_only(s: &Scene) {
    s.rig
        .lets_use(
            "environment",
            s.on.environment,
            "environment.manage",
            &s.bea,
            json!({}),
        )
        .await;
    let provider_read = s
        .rig
        .world
        .app
        .call(ProviderGet, &[&s.on.provider])
        .token(&s.bea.token)
        .send_bytes()
        .await;
    assert_eq!(provider_read.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn completed_projection_handover_snapshot_conflict_keeps_the_actual_terminal_revision() {
    let s = scene().await;
    let provider = managed_and_up(&s).await;
    let (heard, _) = the_environment_gives_back(&s);
    handed_over(&s, provider).await;
    full(bring(&s, &s.ada.token, provider, false).await, provider);
    assert_eq!(native_counts(&heard), (1, 1), "ordinary real return prime");
    // A genuine new custody has a new since stamp and therefore new durable command IDs.
    s.rig.world.advance(chrono::Duration::seconds(1));
    handed_over(&s, provider).await;
    let token = key(
        &s,
        s.ada.id(),
        vec![rule(
            "provider",
            provider,
            PROVIDER_RETURN,
            &["manage", "read"],
        )],
    )
    .await;
    s.rig.machine.handler.hold(K::ProviderCredentialAccept);
    let mut accepted_revision = None;
    let future = async {
        tokio::join!(bring(&s, &token, provider, false), async {
            // Exact actual HTTP operation/key/provider/phase correlation rejects the
            // prime's Session-origin return and any automatic/system-owned command.
            handler_started(
                &s,
                K::ProviderCredentialAccept,
                provider,
                &token,
                PROVIDER_RETURN,
            )
            .await;
            let mut blocker = s.rig.db.pool().begin().await.unwrap();
            let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *blocker)
                .await
                .unwrap();
            let token_id: Id =
                sqlx::query_scalar("SELECT id FROM api_tokens WHERE token_hash=$1 FOR UPDATE")
                    .bind(token_hash(&token))
                    .fetch_one(&mut *blocker)
                    .await
                    .unwrap();
            // Native accept was already authorized/issued. Terminal reconciliation must
            // finish without acquiring a new issuer lease; only optional metadata waits.
            s.rig.machine.handler.release();
            eventually(
                "actual completed return metadata waits on this exact credential blocker",
                || async {
                    let home: bool = sqlx::query_scalar(
                        "SELECT held_by_environment_id IS NULL FROM providers WHERE id=$1",
                    )
                    .bind(provider)
                    .fetch_one(s.rig.db.pool())
                    .await
                    .unwrap();
                    let waiting: bool = sqlx::query_scalar(
                        "SELECT EXISTS(SELECT 1 FROM pg_stat_activity a
                    WHERE a.datname=current_database() AND a.wait_event_type='Lock'
                      AND $1=ANY(pg_blocking_pids(a.pid)) AND a.query LIKE '%api_tokens%')",
                    )
                    .bind(pid)
                    .fetch_one(s.rig.db.pool())
                    .await
                    .unwrap();
                    home && waiting
                },
            )
            .await;
            assert_eq!(native_counts(&heard), (2, 2));
            let revision: i64 = sqlx::query_scalar("SELECT revision FROM providers WHERE id=$1")
                .bind(provider)
                .fetch_one(s.rig.db.pool())
                .await
                .unwrap();
            accepted_revision = Some(revision);
            // Same exact key and rights, new row version: the already-open RR metadata
            // query must report a genuine database retry after this blocker commits.
            let changed = sqlx::query("UPDATE api_tokens SET name=$2 WHERE id=$1")
                .bind(token_id)
                .bind("Actual unchanged-scope completed return key")
                .execute(&mut *blocker)
                .await
                .unwrap();
            assert_eq!(changed.rows_affected(), 1);
            blocker.commit().await.unwrap();
        })
    };
    let (reply, ()) = tokio::time::timeout(rig::PATIENCE, future)
        .await
        .expect("actual native accept/terminal metadata did not reach its rendezvous");
    let value = receipt(reply, provider);
    assert_eq!(
        value["revision"],
        json!(accepted_revision.unwrap()),
        "receipt needs actual terminal revision, never prepared old revision or guessed increment"
    );
    let revision: i64 = sqlx::query_scalar("SELECT revision FROM providers WHERE id=$1")
        .bind(provider)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(Some(revision), accepted_revision);
    assert_eq!(
        native_counts(&heard),
        (2, 2),
        "accepted native return/accept must not replay"
    );
    assert!(custody(&s, provider).await["held_by"].is_null());
    let returned: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action='provider.returned' AND object_id=$1",
    )
    .bind(provider)
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(returned, 2);
}

#[tokio::test]
async fn ordinary_provider_and_actual_holder_management_remain_alternatives() {
    for holder in [false, true] {
        let s = scene().await;
        let provider = managed_and_up(&s).await;
        let (heard, _) = the_environment_gives_back(&s);
        if holder {
            holder_only(&s).await;
        }
        handed_over(&s, provider).await;
        let token = if holder { &s.bea.token } else { &s.ada.token };
        full(bring(&s, token, provider, false).await, provider);
        assert_returned(&s, provider, &heard, 1).await;
    }
}

#[tokio::test]
async fn declaring_lost_requires_ordinary_provider_management() {
    let s = scene().await;
    let provider = managed_and_up(&s).await;
    let (heard, _) = the_environment_gives_back(&s);
    holder_only(&s).await;
    handed_over(&s, provider).await;
    denied_without_effects(&s, &heard, &s.bea.token, provider, true).await;
    denied_without_effects(&s, &heard, &s.cai.token, provider, false).await;
    let value = full(bring(&s, &s.ada.token, provider, true).await, provider);
    assert_eq!(value["status"], "needs_setup");
    assert!(custody(&s, provider).await["held_by"].is_null());
    assert_eq!(native_counts(&heard), (0, 0));
}

#[tokio::test]
async fn provider_variant_manage_without_read_returns_only_a_receipt() {
    let s = scene().await;
    let provider = managed_and_up(&s).await;
    let (heard, _) = the_environment_gives_back(&s);
    handed_over(&s, provider).await;
    let token = key(
        &s,
        s.ada.id(),
        vec![rule("provider", provider, PROVIDER_RETURN, &["manage"])],
    )
    .await;
    receipt(bring(&s, &token, provider, false).await, provider);
    assert_returned(&s, provider, &heard, 1).await;
}

#[tokio::test]
async fn holder_variant_read_keeps_the_ordinary_limited_provider_projection() {
    let s = scene().await;
    let provider = managed_and_up(&s).await;
    let (heard, _) = the_environment_gives_back(&s);
    holder_only(&s).await;
    handed_over(&s, provider).await;
    let ordinary = full(bring(&s, &s.bea.token, provider, false).await, provider);
    // Each physical handover has its own held_since identity. Advance only the
    // synthetic clock so the second return cannot reuse the first command ID.
    s.rig.world.advance(chrono::Duration::seconds(1));
    handed_over(&s, provider).await;
    let token = key(
        &s,
        s.bea.id(),
        holder_rules(&s, provider, &["manage", "read"]),
    )
    .await;
    let bounded = full(bring(&s, &token, provider, false).await, provider);
    for field in [
        "facts",
        "runtime_config",
        "upstreams",
        "secrets_set",
        "credential_keeper",
        "choice_usage",
    ] {
        assert_eq!(
            bounded[field], ordinary[field],
            "ordinary visibility: {field}"
        );
    }
    assert_eq!(bounded["runtime_config"], json!({}));
    assert_eq!(bounded["upstreams"], json!([]));
    assert_eq!(bounded["secrets_set"], json!([]));
    assert_returned(&s, provider, &heard, 2).await;
}

#[tokio::test]
async fn account_with_both_rights_can_use_a_key_allowing_only_the_holder_variant() {
    let s = scene().await;
    let provider = managed_and_up(&s).await;
    let (heard, _) = the_environment_gives_back(&s);
    handed_over(&s, provider).await;
    // Ada owns both resources. Only this exact selection is allowed by the key;
    // ordinary ProviderManage success must not force the provider selection.
    let token = key(&s, s.ada.id(), holder_rules(&s, provider, &["manage"])).await;
    receipt(bring(&s, &token, provider, false).await, provider);
    assert_returned(&s, provider, &heard, 1).await;
}

#[tokio::test]
async fn holder_variant_requires_both_exact_effect_scopes_and_ordinary_authority() {
    let s = scene().await;
    let provider = managed_and_up(&s).await;
    let (heard, _) = the_environment_gives_back(&s);
    holder_only(&s).await;
    handed_over(&s, provider).await;
    let complete = holder_rules(&s, provider, &["manage"]);
    let missing_provider = vec![complete[1].clone()];
    let missing_holder = vec![complete[0].clone()];
    let wrong_provider = vec![
        rule("provider", new_id(), HOLDER_RETURN, &["manage"]),
        complete[1].clone(),
    ];
    let wrong_holder = vec![
        complete[0].clone(),
        rule("environment", new_id(), HOLDER_RETURN, &["manage"]),
    ];
    let provider_selection = vec![rule("provider", provider, PROVIDER_RETURN, &["manage"])];
    for rules in [
        missing_provider,
        missing_holder,
        wrong_provider,
        wrong_holder,
        provider_selection,
    ] {
        let token = key(&s, s.bea.id(), rules).await;
        denied_without_effects(&s, &heard, &token, provider, false).await;
    }
    let holder = key(&s, s.bea.id(), complete.clone()).await;
    denied_without_effects(&s, &heard, &holder, provider, true).await;
    let viewer = key(&s, s.cai.id(), complete).await;
    denied_without_effects(&s, &heard, &viewer, provider, false).await;
    assert_eq!(native_counts(&heard), (0, 0));
}

#[tokio::test]
async fn only_same_operation_read_discloses_the_provider_after_return() {
    let s = scene().await;
    let provider = managed_and_up(&s).await;
    let (heard, _) = the_environment_gives_back(&s);
    handed_over(&s, provider).await;
    let unrelated = key(
        &s,
        s.ada.id(),
        vec![
            rule("provider", provider, PROVIDER_RETURN, &["manage"]),
            rule("provider", provider, "ProviderGet", &["read"]),
        ],
    )
    .await;
    receipt(bring(&s, &unrelated, provider, false).await, provider);
    // A distinct synthetic custody attempt avoids lending a completed native ID.
    s.rig.world.advance(chrono::Duration::seconds(1));
    handed_over(&s, provider).await;
    let allowed = key(
        &s,
        s.ada.id(),
        vec![rule(
            "provider",
            provider,
            PROVIDER_RETURN,
            &["manage", "read"],
        )],
    )
    .await;
    let value = full(bring(&s, &allowed, provider, false).await, provider);
    assert!(value["name"].as_str().is_some());
    assert!(value.get("runtime_config").is_some());
    assert_returned(&s, provider, &heard, 2).await;
}
