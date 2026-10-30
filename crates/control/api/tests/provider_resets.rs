//! Confirmed provider resets require current browser management, reject stale facts and leave one durable audit receipt per attempt.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod rig;
mod support;

use chrono::Duration;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::resources::{ProviderResetConsume, ProviderResetConsumeBody};
use orch_link_contract::commands::{
    CommandKind, CommandResult, ProviderRefreshResult, ProviderResetConsumeResult,
};
use orch_types::{Clock, Id, Json, Time, new_id};
use rig::Scene;
use serde_json::json;
use std::sync::{Arc, Mutex};

fn facts(count: i64, used: f64) -> Json {
    json!({"plan":"plus","identity":"ada@example.invalid","windows":[{"id":"primary","label":"5h","duration_minutes":300,"used_percent":used,"resets_at":null}],
        "available_resets":{"available_count":count,"credits":[{"id":"credit-1","title":"Full reset","description":"Five hours and the week","reset_type":"codexRateLimits","status":"available","granted_at":null,"expires_at":null}]},
        "ordinary_usage_allowed":true})
}

fn authenticated_reset_read(mut facts: Json, at: Time) -> Json {
    facts["credential_verified"] = json!(true);
    facts["field_times"]["credential_verified"] = json!(at);
    facts["field_times"]["available_resets"] = json!(at);
    facts
}

async fn setup() -> (Scene, Arc<Mutex<Json>>, Arc<Mutex<Vec<Json>>>) {
    let s = Scene::new().await;
    sqlx::query("UPDATE agents SET descriptor=jsonb_set(descriptor,'{capabilities,provider_facts}','[\"available_resets\"]'::jsonb) WHERE id=$1")
        .bind(s.on.agent).execute(s.rig.db.pool()).await.unwrap();
    let current = Arc::new(Mutex::new(facts(3, 80.0)));
    let initial = current.lock().unwrap().clone();
    sqlx::query("UPDATE providers SET facts = $2,facts_at = $3 WHERE id = $1")
        .bind(s.on.provider)
        .bind(initial)
        .bind(s.rig.world.now())
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let read = current.clone();
    let read_clock = s.rig.world.app.clock.clone();
    s.rig
        .machine
        .handler
        .on(CommandKind::ProviderRefresh, move |_| {
            Ok(CommandResult::ProviderRefresh(ProviderRefreshResult {
                source: None,
                status: "ready".into(),
                facts: serde_json::from_value(authenticated_reset_read(
                    read.lock().unwrap().clone(),
                    read_clock.time(),
                ))
                .unwrap(),
            }))
        });
    let consumed: Arc<Mutex<Vec<Json>>> = Arc::default();
    let seen = consumed.clone();
    let state = current.clone();
    let reset_clock = s.rig.world.app.clock.clone();
    s.rig
        .machine
        .handler
        .on(CommandKind::ProviderResetConsume, move |command| {
            seen.lock().unwrap().push(command.body().unwrap());
            *state.lock().unwrap() = facts(2, 0.0);
            Ok(CommandResult::ProviderResetConsume(
                ProviderResetConsumeResult {
                    source: None,
                    outcome: "reset".into(),
                    facts: serde_json::from_value(authenticated_reset_read(
                        state.lock().unwrap().clone(),
                        reset_clock.time(),
                    ))
                    .unwrap(),
                },
            ))
        });
    (s, current, consumed)
}

fn confirmed() -> ProviderResetConsumeBody {
    ProviderResetConsumeBody {
        request_id: new_id(),
        reset_id: "credit-1".into(),
        available_count: 3,
    }
}

#[tokio::test]
async fn a_programs_reset_for_a_key_uses_its_source_without_credential_proof_or_windows_and_keeps_its_own_outcome_words()
 {
    use orch_link_contract::commands::{Command, ProviderSourceRead};
    let (s, _current, consumed) = setup().await;
    let mut descriptor = orch_provider_contract::SourceDescriptor::new(
        "plan",
        "Plan details",
        "Reports and renews the team's plan.",
    );
    descriptor.facts = Some(orch_provider_contract::source::SourceDescriptorFacts {
        reports: vec![
            orch_provider_contract::FactMember::AvailableResets,
            orch_provider_contract::FactMember::OrdinaryUsageAllowed,
        ],
        spends_resets: true,
        ..Default::default()
    });
    // An API provider names at least one upstream (the schema's own check).
    sqlx::query("UPDATE providers SET kind='api',agent_id=NULL,harness_kind=NULL,status='exhausted',sources=$2,facts=$3,upstreams=$4 WHERE id=$1")
        .bind(s.on.provider).bind(json!({"facts":{"by":"program","name":"plan"}}))
        .bind(json!({"ordinary_usage_allowed":false,"limit_reached":{"type":"weekly"}}))
        .bind(json!([{"protocol":"openai-responses","base_url":"https://api.example.test","options":{}}]))
        .execute(s.rig.db.pool()).await.unwrap();
    let read_clock = s.rig.world.app.clock.clone();
    let before = descriptor.clone();
    s.rig.machine.handler.on(CommandKind::ProviderRefresh,move |command| {
        let Command::ProviderRefresh(body)=command else {panic!("wrong command");};assert_eq!(body.facts.as_ref().unwrap().name.as_deref(),Some("plan"));
        let available=json!({"available_resets":{"available_count":3,"credits":[{"id":"credit-1","reset_type":"plan","status":"available","expires_at":null}]},"ordinary_usage_allowed":false,"field_times":{"available_resets":read_clock.time(),"ordinary_usage_allowed":read_clock.time()}});
        Ok(CommandResult::ProviderRefresh(ProviderRefreshResult {source:Some(ProviderSourceRead {descriptor:Some(before.clone()),error:None,supported:Some(true)}),status:"exhausted".into(),facts:serde_json::from_value(available).unwrap()}))
    });
    let after_clock = s.rig.world.app.clock.clone();
    let spent = consumed.clone();
    s.rig.machine.handler.on(CommandKind::ProviderResetConsume,move |command| {
        spent.lock().unwrap().push(command.body().unwrap());
        let Command::ProviderResetConsume(body)=command else {panic!("wrong command");};assert_eq!(body.facts.as_ref().unwrap().name.as_deref(),Some("plan"));
        Ok(CommandResult::ProviderResetConsume(ProviderResetConsumeResult {
            source:Some(ProviderSourceRead{descriptor:Some(descriptor.clone()),error:None,supported:Some(true)}),outcome:"Plan renewed".into(),
            facts:serde_json::from_value(json!({"ordinary_usage_allowed":true,"available_resets":{"available_count":2,"credits":[]},"field_times":{"ordinary_usage_allowed":after_clock.time(),"available_resets":after_clock.time()}})).unwrap(),
        }))
    });
    let body = confirmed();
    for _ in 0..2 {
        let result = s
            .rig
            .world
            .app
            .call(ProviderResetConsume, &[&s.on.provider])
            .token(&s.ada.token)
            .body(&body)
            .send()
            .await
            .ok();
        assert_eq!(result.outcome, "Plan renewed");
        assert_eq!(
            result.provider.base.status,
            orch_api_contract::resources::ProviderStatus::Ready
        );
        assert!(result.provider.facts.windows.is_empty());
        assert_ne!(result.provider.facts.credential_verified, Some(true));
        assert!(result.provider.last_reset.is_some());
    }
    assert_eq!(
        consumed.lock().unwrap().len(),
        1,
        "the confirmed request spends once even with a custom outcome sentence"
    );
}

#[tokio::test]
async fn an_untold_or_cached_reset_group_cannot_authorize_consumption_and_does_not_fault_the_provider()
 {
    for cached in [false, true] {
        let (s, current, consumed) = setup().await;
        let old = s.rig.world.app.clock.time();
        if !cached {
            sqlx::query("UPDATE providers SET status = 'error',status_detail = 'the credential was refused',updated_at = $2 WHERE id = $1")
                .bind(s.on.provider).bind(old.get()).execute(s.rig.db.pool()).await.unwrap();
        }
        s.rig.world.app.clock.advance(Duration::minutes(5));
        let mut response = authenticated_reset_read(
            current.lock().unwrap().clone(),
            s.rig.world.app.clock.time(),
        );
        if cached {
            response["field_times"]["available_resets"] = json!(old);
        } else {
            response["available_resets"] = Json::Null;
            response["field_times"]
                .as_object_mut()
                .unwrap()
                .remove("available_resets");
        }
        s.rig
            .machine
            .handler
            .on(CommandKind::ProviderRefresh, move |_| {
                Ok(CommandResult::ProviderRefresh(ProviderRefreshResult {
                    source: None,
                    status: "ready".into(),
                    facts: serde_json::from_value(response.clone()).unwrap(),
                }))
            });
        let refusal = s
            .rig
            .world
            .app
            .call(ProviderResetConsume, &[&s.on.provider])
            .body(&confirmed())
            .token(&s.ada.token)
            .send()
            .await
            .problem();
        assert_eq!(refusal.code, ErrorCode::ProviderResetUnavailable);
        assert_eq!(refusal.extra["current"]["status"], "ready");
        assert!(consumed.lock().unwrap().is_empty());
        let (status, kept): (String, Json) =
            sqlx::query_as("SELECT status,facts FROM providers WHERE id = $1")
                .bind(s.on.provider)
                .fetch_one(s.rig.db.pool())
                .await
                .unwrap();
        assert_eq!(status, "ready");
        assert_eq!(kept["credential_verified"], true);
        assert_eq!(kept["available_resets"]["available_count"], 3);
    }
}

#[tokio::test]
async fn a_source_that_lists_credits_but_cannot_spend_them_is_not_asked_to_consume_one() {
    let (s, _current, consumed) = setup().await;
    let mut descriptor = orch_provider_contract::SourceDescriptor::new(
        "plan",
        "Plan details",
        "Lists reset credits without spending them.",
    );
    descriptor.facts = Some(orch_provider_contract::source::SourceDescriptorFacts {
        reports: vec![orch_provider_contract::FactMember::AvailableResets],
        spends_resets: false,
        ..Default::default()
    });
    sqlx::query("UPDATE providers SET sources=$2 WHERE id=$1")
        .bind(s.on.provider)
        .bind(json!({"facts":{"by":"program","name":"plan","descriptor":descriptor}}))
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let refusal = s
        .rig
        .world
        .app
        .call(ProviderResetConsume, &[&s.on.provider])
        .token(&s.ada.token)
        .body(&confirmed())
        .send()
        .await
        .problem();
    assert_eq!(refusal.code, ErrorCode::ProviderResetUnavailable);
    assert!(consumed.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_lost_control_reply_joins_the_durable_reset_request_after_the_vendor_changed_its_count() {
    let (s, _current, consumed) = setup().await;
    let body = confirmed();
    let result = s
        .rig
        .engine
        .consume_provider_reset(
            s.on.provider,
            &body.reset_id,
            s.ada.id(),
            body.request_id,
            body.available_count,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.outcome, "reset");
    // This test's scripted link keeps its handles in memory; persist the actual safe command/result
    // as the production command journal does, leaving no control-plane receipt for the lost reply.
    let operation = orch_engine::provider_fact_reads::reset_operation(
        s.on.provider,
        s.ada.id(),
        body.request_id,
    );
    let request = consumed.lock().unwrap()[0].clone();
    sqlx::query("INSERT INTO commands(id,machine_id,kind,body,state,result,deadline_at) VALUES($1,$2,'provider.reset',$3,'succeeded',$4,$5)")
        .bind(operation).bind(s.rig.machine_id).bind(request).bind(serde_json::to_value(&result).unwrap())
        .bind(s.rig.world.now()+Duration::seconds(30)).execute(s.rig.db.pool()).await.unwrap();
    let replied = s
        .rig
        .world
        .app
        .call(ProviderResetConsume, &[&s.on.provider])
        .token(&s.ada.token)
        .body(&body)
        .send()
        .await
        .ok();
    assert_eq!(replied.outcome, "reset");
    assert_eq!(
        replied
            .provider
            .facts
            .available_resets
            .unwrap()
            .available_count,
        2
    );
    assert_eq!(consumed.lock().unwrap().len(), 1);
    let receipts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'provider.reset_used' AND object_id = $1",
    )
    .bind(s.on.provider)
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(receipts, 1);
}

#[tokio::test]
async fn recovery_proof_lifetime_resets_admission_and_source_times_cannot_be_written_by_a_manager()
{
    let (s, _current, _) = setup().await;
    for changes in [
        json!({"credential_verified":true}),
        json!({"credential":{"method":null,"yields":"token","expires_at":"2099-01-01T00:00:00.000Z"}}),
        json!({"available_resets":{"available_count":9,"credits":[]}}),
        json!({"ordinary_usage_allowed":true}),
        json!({"field_times":{"windows":"2099-01-01T00:00:00.000Z"}}),
    ] {
        let refused = s
            .rig
            .world
            .app
            .call(
                orch_api_contract::resources::ProviderFactsOverridesSet,
                &[&s.on.provider],
            )
            .token(&s.ada.token)
            .raw_body(&changes)
            .send()
            .await
            .problem();
        assert_eq!(refused.code, ErrorCode::RequestInvalid);
    }
}

async fn token(s: &Scene, user: Id) -> String {
    use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
    let token = new_token(API_TOKEN_PREFIX);
    let mut conn = s.rig.db.acquire().await.unwrap();
    orch_db::queries::identity::testing::insert_api_token(&mut conn, user, &token_hash(&token))
        .await
        .unwrap();
    sqlx::query("UPDATE api_tokens SET scopes = ARRAY['provider.manage'] WHERE token_hash = $1")
        .bind(token_hash(&token))
        .execute(&mut *conn)
        .await
        .unwrap();
    token
}

#[tokio::test]
async fn a_token_or_agent_holding_provider_manage_and_a_browser_without_it_cannot_use_a_reset() {
    let (s, _current, consumed) = setup().await;
    let api = token(&s, s.ada.id()).await;
    let body = confirmed();
    let denied = s
        .rig
        .world
        .app
        .call(ProviderResetConsume, &[&s.on.provider])
        .token(&api)
        .body(&body)
        .send()
        .await
        .problem();
    assert_eq!(denied.code, ErrorCode::AuthSessionRequired);
    let denied = s
        .rig
        .world
        .app
        .call(ProviderResetConsume, &[&s.on.provider])
        .token(&s.bea.token)
        .body(&body)
        .send()
        .await
        .problem();
    assert_eq!(denied.code, ErrorCode::AuthzDenied);
    let agent = new_id();
    let mut conn = s.rig.db.acquire().await.unwrap();
    orch_db::queries::identity::create_service_account(
        &mut conn,
        agent,
        s.tree.org.base.id,
        "reset-agent",
        "Reset agent",
        s.ada.id(),
        s.rig.world.now(),
    )
    .await
    .unwrap();
    orch_db::queries::access::testing::add_entry(
        &mut conn,
        &orch_db::queries::access::PageKey::new("provider", s.on.provider, "live"),
        "any",
        "provider.manage",
        ("user", Some(agent), None),
        None,
        s.rig.world.now(),
    )
    .await
    .unwrap();
    drop(conn);
    let agent_token = token(&s, agent).await;
    let denied = s
        .rig
        .world
        .app
        .call(ProviderResetConsume, &[&s.on.provider])
        .token(&agent_token)
        .body(&body)
        .send()
        .await
        .problem();
    assert_eq!(
        denied.code,
        ErrorCode::AuthSessionRequired,
        "an agent's scoped management token cannot confirm a reset"
    );
    assert!(consumed.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_stale_reset_or_count_is_refused_with_the_current_runtime_facts() {
    let (s, current, consumed) = setup().await;
    for body in [
        ProviderResetConsumeBody {
            available_count: 2,
            ..confirmed()
        },
        ProviderResetConsumeBody {
            reset_id: "credit-no-longer-listed".into(),
            ..confirmed()
        },
    ] {
        let denied = s
            .rig
            .world
            .app
            .call(ProviderResetConsume, &[&s.on.provider])
            .token(&s.ada.token)
            .body(&body)
            .send()
            .await
            .problem();
        assert_eq!(denied.code, ErrorCode::ProviderResetStale);
        assert_eq!(
            denied.extra["current"]["facts"]["available_resets"]["available_count"],
            3
        );
    }
    *current.lock().unwrap() = facts(1, 80.0);
    let denied = s
        .rig
        .world
        .app
        .call(ProviderResetConsume, &[&s.on.provider])
        .token(&s.ada.token)
        .body(&confirmed())
        .send()
        .await
        .problem();
    assert_eq!(
        denied.extra["current"]["facts"]["available_resets"]["available_count"],
        1
    );
    assert!(consumed.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_confirmed_request_sent_twice_uses_one_reset_and_records_who_and_the_windows() {
    let (s, current, consumed) = setup().await;
    // A material fall before a known reset must reuse this route's receipt, even without a clock tick.
    let mut before = current.lock().unwrap().clone();
    before["windows"][0]["resets_at"] = json!(Time::new(s.rig.world.now() + Duration::hours(5)));
    *current.lock().unwrap() = before.clone();
    sqlx::query("UPDATE providers SET facts=$2 WHERE id=$1")
        .bind(s.on.provider)
        .bind(before)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let body = confirmed();
    for _ in 0..2 {
        let result = s
            .rig
            .world
            .app
            .call(ProviderResetConsume, &[&s.on.provider])
            .token(&s.ada.token)
            .body(&body)
            .send()
            .await
            .ok();
        assert_eq!(result.outcome, "reset");
        assert_eq!(
            result
                .provider
                .facts
                .available_resets
                .unwrap()
                .available_count,
            2
        );
        assert_eq!(result.provider.last_reset.unwrap().user.id, s.ada.id());
    }
    assert_eq!(consumed.lock().unwrap().len(), 1);
    let (actor,details):(Id,Json) = sqlx::query_as("SELECT actor_id,details FROM audit_log WHERE action = 'provider.reset_used' AND object_id = $1").bind(s.on.provider).fetch_one(s.rig.db.pool()).await.unwrap();
    assert_eq!(actor, s.ada.id());
    assert_eq!(details["reset_id"], "credit-1");
    assert_eq!(details["windows_before"][0]["used_percent"], 80.0);
    assert_eq!(details["windows_after"][0]["used_percent"], 0.0);
    let external: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action='provider.window_started_over' AND object_id=$1")
        .bind(s.on.provider)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(external, 0, "Orch's receipt is the only start-over record");
    let mut conn = s.rig.db.acquire().await.unwrap();
    use orch_db::queries::provider_plan::{self, PlanRead, WindowDefinition};
    let windows = [WindowDefinition {
        window_id: "primary".to_owned(),
        natural_start: None,
        lookback_seconds: 3600,
    }];
    let readings = provider_plan::chart(
        &mut conn,
        &PlanRead {
            provider: s.on.provider,
            windows: &windows,
            from: s.rig.world.now() - Duration::minutes(1),
            to: s.rig.world.now() + Duration::milliseconds(1),
            now: s.rig.world.now(),
            bucket: "hour",
        },
    )
    .await
    .unwrap();
    let markers = readings[0].resets.as_array().unwrap();
    assert_eq!(markers.len(), 1);
    assert_eq!(markers[0]["cause"], "orch");
    drop(conn);
    s.rig.world.advance(Duration::hours(5));
    let provider = s
        .rig
        .world
        .app
        .call(orch_api_contract::resources::ProviderGet, &[&s.on.provider])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    assert!(
        provider.last_reset.is_some(),
        "an unknown cycle keeps its dated receipt without an invented expiry"
    );
    let reset = orch_types::Time::new(s.rig.world.now() + Duration::hours(5));
    current.lock().unwrap()["windows"][0]["resets_at"] = json!(reset);
    current.lock().unwrap()["windows"][0]["used_percent"] = json!(5.0);
    s.rig.world.advance(Duration::minutes(1));
    // A page read never asks the machine; a person's Refresh does, past the minute that coalesces it.
    let next = s
        .rig
        .world
        .app
        .call(
            orch_api_contract::resources::ProviderRefresh,
            &[&s.on.provider],
        )
        .token(&s.ada.token)
        .body(&orch_api_contract::resources::ProviderRefreshBody::default())
        .send()
        .await
        .ok();
    assert!(
        next.last_reset.is_none(),
        "the native report now proves the next cycle began"
    );
}

#[tokio::test]
async fn management_is_checked_again_at_the_act_after_a_runtime_read_waits() {
    let (s, current, consumed) = setup().await;
    s.rig
        .lets_use(
            "provider",
            s.on.provider,
            "provider.manage",
            &s.bea,
            json!({}),
        )
        .await;
    sqlx::query("UPDATE permission_entries SET expires_at = $3 WHERE object_kind = 'provider' AND object_id = $1 AND holder_id = $2 AND permission = 'provider.manage'")
        .bind(s.on.provider).bind(s.bea.id()).bind(s.rig.world.now()+Duration::seconds(1)).execute(s.rig.db.pool()).await.unwrap();
    let clock = s.rig.world.app.clock.clone();
    s.rig
        .machine
        .handler
        .on(CommandKind::ProviderRefresh, move |_| {
            clock.advance(Duration::seconds(2));
            Ok(CommandResult::ProviderRefresh(ProviderRefreshResult {
                source: None,
                status: "ready".into(),
                facts: serde_json::from_value(current.lock().unwrap().clone()).unwrap(),
            }))
        });
    let denied = s
        .rig
        .world
        .app
        .call(ProviderResetConsume, &[&s.on.provider])
        .token(&s.bea.token)
        .body(&confirmed())
        .send()
        .await
        .problem();
    assert_eq!(denied.code, ErrorCode::AuthzDenied);
    assert!(consumed.lock().unwrap().is_empty());
}
