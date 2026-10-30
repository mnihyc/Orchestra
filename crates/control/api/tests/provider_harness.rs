//! The harness of a provider and what the machine that keeps it says: a harness that never ran says what it is when a thread first needs it, the machine is asked to look at a provider's credential, facts are written by hand over what it reports, a harness is brought up anew, and the conversations it holds are listed.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod rig;
mod support;

use orch_api_contract::binding::{EnvironmentChoice, ThreadBindingSet, ThreadBindingSetBody};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::nodes::{CreateThread, CreateThreadBinding, ThreadCreate};
use orch_api_contract::resources::{
    NativeSession, Provider, ProviderFactsOverridesSet, ProviderFactsOverridesSetBody, ProviderGet,
    ProviderHarnessRestart, ProviderHarnessSessionList, ProviderKeySet, ProviderKeySetBody,
    ProviderRefresh, ProviderStatus,
};
use orch_api_contract::threads::Thread;
use orch_link_contract::commands::{
    AgentDiscoverSessionsResult, AgentStartResult, CommandKind, CommandResult, Empty,
    ProviderDiscoverModelsResult, ProviderRefreshResult,
};
use orch_types::Id;
use places::{Who, missing_permissions};
use rig::{MODEL, Scene};
use serde_json::{Value, json};

// ---------------------------------------------------------------- the setting

/// The profile has never run: nothing says yet what its harness is.
async fn forget_what_the_harness_is(s: &Scene) {
    sqlx::query("UPDATE agents SET descriptor = '{}' WHERE id = $1")
        .bind(s.on.agent)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
}

async fn declared(s: &Scene) -> Value {
    sqlx::query_scalar("SELECT descriptor FROM agents WHERE id = $1")
        .bind(s.on.agent)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn a_provider_read_keeps_the_last_program_error_beside_a_restarted_online_profile() {
    let s = Scene::new().await;
    let thread = s.thread().await;
    s.exchange(thread.base.id, 0, "Hello", "Hi").await;
    sqlx::query("UPDATE harnesses SET status='online',status_detail=NULL,last_error_at=now(),last_error_detail='app-server exited: final diagnostic' WHERE provider_id=$1 AND agent_id=$2")
        .bind(s.on.provider).bind(s.on.agent).execute(s.rig.db.pool()).await.unwrap();
    let provider = s
        .rig
        .world
        .app
        .call(ProviderGet, &[&s.on.provider])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(
        provider.harnesses[0].status,
        orch_api_contract::resources::HarnessStatus::Online
    );
    assert_eq!(
        provider.harnesses[0]
            .last_error
            .as_ref()
            .unwrap()
            .detail
            .as_deref(),
        Some("app-server exited: final diagnostic")
    );
}

async fn try_thread(s: &Scene, by: &Who) -> orch_api_core::testing::Reply<Thread> {
    s.rig
        .world
        .app
        .call(ThreadCreate, &[&s.tree.topic.base.id])
        .token(&by.token)
        .body(&CreateThread {
            name: Some("First".to_owned()),
            binding: Some(CreateThreadBinding {
                agent_id: s.on.agent,
                provider_id: s.on.provider,
                arrangement: None,
            }),
            model: Some(MODEL.to_owned()),
            environment: Some(EnvironmentChoice::Use(s.on.environment)),
            directory: Some("web".to_owned()),
            ..CreateThread::default()
        })
        .send()
        .await
}

fn started(s: &Scene) -> usize {
    s.rig.seen.of_kind(CommandKind::AgentStart).len()
}

async fn go_away(s: &Scene) {
    sqlx::query("UPDATE machines SET online = false WHERE id = $1")
        .bind(s.rig.machine_id)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
}

// ---------------------------------------------------------------- a harness that never ran

#[tokio::test]
async fn the_first_thread_on_a_profile_whose_harness_never_ran_has_the_harness_say_what_it_is() {
    let s = Scene::new().await;
    forget_what_the_harness_is(&s).await;
    let made = try_thread(&s, &s.ada).await;
    assert!(made.status.is_success(), "{:?}", made.body);
    let thread = made.ok();
    assert_eq!(
        started(&s),
        1,
        "the harness was started once, to say what it is"
    );
    assert_eq!(declared(&s).await["kind"], "fake");
    let arrangement: String =
        sqlx::query_scalar("SELECT arrangement FROM threads WHERE node_id = $1")
            .bind(thread.base.id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(
        arrangement, "together",
        "what it declared is what the thread was put on"
    );
    // The first turn finds the harness up and starts nothing.
    s.exchange(thread.base.id, 0, "Hello", "Hi").await;
    assert_eq!(started(&s), 1);
}

/// The authorization epochs together: a write that decided under one is refused at its commit when it moved.
async fn epochs(s: &Scene) -> i64 {
    sqlx::query_scalar("SELECT COALESCE(sum(epoch), 0)::bigint FROM authz_epochs")
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn a_thread_made_once_its_harness_said_what_it_is_is_a_birth_that_moves_no_epoch() {
    let s = Scene::new().await;
    forget_what_the_harness_is(&s).await;
    let before = epochs(&s).await;
    let made = try_thread(&s, &s.ada).await;
    assert!(made.status.is_success(), "{:?}", made.body);
    assert_eq!(started(&s), 1, "the harness was asked what it is");
    assert_eq!(
        epochs(&s).await,
        before,
        "a new thread only adds authority, as it does when its harness had said what it is before: \
         nobody's write elsewhere is refused for it"
    );
}

async fn sign_out_during_discovery(s: &Scene) {
    use orch_api_core::middleware::authentication::token_hash;
    use orch_db::queries::identity;

    let session = identity::session_by_access_token(
        &mut s.rig.db.acquire().await.unwrap(),
        &token_hash(&s.ada.token),
        s.rig.world.now(),
    )
    .await
    .unwrap()
    .unwrap()
    .session_id;
    rig::eventually("the harness starts discovery", || async {
        s.rig
            .machine
            .handler
            .executions()
            .iter()
            .any(|command| command.kind == CommandKind::AgentStart)
    })
    .await;
    let mut tx = s.rig.db.begin().await.unwrap();
    identity::revoke_session(&mut tx, session, s.ada.id(), s.rig.world.now())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    s.rig.machine.handler.release();
}

#[tokio::test]
async fn signing_out_during_harness_discovery_prevents_the_thread_from_being_created() {
    let s = Scene::new().await;
    forget_what_the_harness_is(&s).await;
    s.rig.machine.handler.hold(CommandKind::AgentStart);
    let (made, ()) = tokio::time::timeout(rig::PATIENCE, async {
        tokio::join!(try_thread(&s, &s.ada), sign_out_during_discovery(&s))
    })
    .await
    .expect("discovery holds no credential lock that would prevent signing out");
    assert_eq!(
        made.status,
        http::StatusCode::UNAUTHORIZED,
        "{:?}",
        made.body
    );
    assert_eq!(made.problem().code, ErrorCode::AuthInvalidToken);
    let threads: i64 =
        sqlx::query_scalar("SELECT count(*) FROM nodes WHERE parent_id = $1 AND kind = 'thread'")
            .bind(s.tree.topic.base.id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(threads, 0, "a revoked credential cannot create a thread");
}

#[tokio::test]
async fn signing_out_during_harness_discovery_preserves_the_existing_binding() {
    let s = Scene::new().await;
    let thread = s.thread().await;
    let fresh = unreported_profile(&s).await;
    s.rig.machine.handler.hold(CommandKind::AgentStart);
    let bind = async {
        s.rig
            .world
            .app
            .call(ThreadBindingSet, &[&thread.base.id])
            .token(&s.ada.token)
            .body(&ThreadBindingSetBody {
                agent_id: fresh,
                provider_id: s.on.provider,
                arrangement: None,
                model: None,
                effort: None,
            })
            .send()
            .await
    };
    let (bound, ()) = tokio::time::timeout(rig::PATIENCE, async {
        tokio::join!(bind, sign_out_during_discovery(&s))
    })
    .await
    .expect("discovery releases the credential before waiting for the harness");
    assert_eq!(
        bound.status,
        http::StatusCode::UNAUTHORIZED,
        "{:?}",
        bound.body
    );
    assert_eq!(bound.problem().code, ErrorCode::AuthInvalidToken);
    let agent: Id = sqlx::query_scalar("SELECT agent_id FROM threads WHERE node_id = $1")
        .bind(thread.base.id)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(agent, s.on.agent, "the revoked request changed no binding");
}

#[tokio::test]
async fn a_caller_who_may_not_use_what_they_chose_does_not_have_a_harness_started() {
    let s = Scene::new().await;
    forget_what_the_harness_is(&s).await;
    let refused = try_thread(&s, &s.dan).await;
    assert_eq!(refused.problem().code, ErrorCode::AuthzDenied);
    assert_eq!(
        started(&s),
        0,
        "asking for what is not theirs starts nothing"
    );
}

#[tokio::test]
async fn a_harness_that_cannot_be_asked_leaves_the_profile_unreported_and_the_thread_refused() {
    let s = Scene::new().await;
    forget_what_the_harness_is(&s).await;
    go_away(&s).await;
    let refused = try_thread(&s, &s.ada).await.problem();
    assert_eq!(refused.code, ErrorCode::BindingIncompatible);
    assert_eq!(refused.extra["reason"], "harness_unreported");
    assert_eq!(started(&s), 0);
}

async fn unreported_profile(s: &Scene) -> Id {
    let fresh = orch_types::new_id();
    sqlx::query(
        "INSERT INTO agents (id, name, owner_id, org_id, home_id, harness_kind, profile, descriptor)
         VALUES ($1, 'Newcomer', $2, $3, $3, 'fake', '{}', '{}')",
    )
    .bind(fresh)
    .bind(s.ada.id())
    .bind(s.tree.org.base.id)
    .execute(s.rig.db.pool())
    .await
    .unwrap();
    s.rig
        .lets_use("agent", fresh, "agent.use", &s.ada, json!({}))
        .await;
    fresh
}

#[tokio::test]
async fn a_thread_without_history_put_on_a_profile_that_never_ran_has_its_harness_say_what_it_is() {
    let s = Scene::new().await;
    let thread = s.thread().await;
    let fresh = unreported_profile(&s).await;
    let before = started(&s);
    let bound = s
        .rig
        .world
        .app
        .call(ThreadBindingSet, &[&thread.base.id])
        .token(&s.ada.token)
        .body(&ThreadBindingSetBody {
            agent_id: fresh,
            provider_id: s.on.provider,
            arrangement: None,
            model: None,
            effort: None,
        })
        .send()
        .await;
    assert!(bound.status.is_success(), "{:?}", bound.body);
    assert_eq!(started(&s), before + 1);
    let kind: Value = sqlx::query_scalar("SELECT descriptor FROM agents WHERE id = $1")
        .bind(fresh)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(kind["kind"], "fake");
}

// ---------------------------------------------------------------- looking at a credential

fn the_machine_says(s: &Scene, status: &'static str, plan: &'static str) {
    s.rig
        .machine
        .handler
        .on(CommandKind::ProviderRefresh, move |_| {
            Ok(CommandResult::ProviderRefresh(ProviderRefreshResult {
                source:None,
                status: status.to_owned(),
                facts: serde_json::from_value(json!({ "plan": plan, "windows": [], "ordinary_usage_allowed": status == "ready" })).unwrap(),
            }))
        });
}

async fn refresh(s: &Scene, by: &Who) -> orch_api_core::testing::Reply<Provider> {
    s.rig
        .world
        .app
        .call(ProviderRefresh, &[&s.on.provider])
        .body(&orch_api_contract::resources::ProviderRefreshBody::default())
        .token(&by.token)
        .send()
        .await
}

#[tokio::test]
async fn refresh_returns_new_model_suggestions_without_accepting_them_and_dates_the_current_catalogue()
 {
    let s = Scene::new().await;
    the_machine_says(&s, "ready", "pro");
    s.rig.machine.handler.on(CommandKind::ProviderDiscoverModels, |_| {
        Ok(CommandResult::ProviderDiscoverModels(serde_json::from_value::<ProviderDiscoverModelsResult>(json!({
            "suggestions":[
                {"model_name":MODEL,"display_name":MODEL,"parameters":{},"listed":true},
                {"model_name":"new-model","display_name":"New model","parameters":{},"listed":true}
            ]
        })).unwrap()))
    });
    let first = refresh(&s, &s.ada).await.ok();
    let suggestions = first.facts.models.as_ref().unwrap();
    assert!(
        suggestions
            .iter()
            .any(|model| model.model_name == "new-model")
    );
    assert!(
        !first
            .models
            .iter()
            .any(|model| model.model_name == "new-model"),
        "discovery never accepts a model"
    );
    let observed = first.facts.field_times["models"];
    assert!(
        first
            .models
            .iter()
            .any(|model| model.model_name == MODEL && model.enabled)
    );
    s.rig.world.app.clock.advance(chrono::Duration::minutes(1));
    s.rig.machine.handler.on(CommandKind::ProviderDiscoverModels, |_| {
        Ok(CommandResult::ProviderDiscoverModels(serde_json::from_value::<ProviderDiscoverModelsResult>(json!({
            "suggestions":[{"model_name":"new-model","display_name":"New model","parameters":{},"listed":true}]
        })).unwrap()))
    });
    let next = refresh(&s, &s.ada).await.ok();
    assert!(next.facts.field_times["models"] > observed);
    assert!(
        !next
            .facts
            .models
            .as_ref()
            .unwrap()
            .iter()
            .any(|model| model.model_name == MODEL)
    );
    assert!(
        next.models
            .iter()
            .any(|model| model.model_name == MODEL && model.enabled),
        "a manager decides what to do with a missing model"
    );
    s.rig.world.app.clock.advance(chrono::Duration::minutes(1));
    s.rig
        .machine
        .handler
        .on(CommandKind::ProviderDiscoverModels, |_| {
            Err(orch_link_contract::commands::CommandError::new(
                orch_link_contract::commands::ErrorCode::Unsupported,
                "the model list is unavailable",
            ))
        });
    let unavailable = refresh(&s, &s.ada).await.ok();
    assert_eq!(unavailable.facts.models, next.facts.models);
    assert_eq!(
        unavailable.facts.field_times["models"], next.facts.field_times["models"],
        "a failed discovery does not date its old list as fresh"
    );
}

#[tokio::test]
async fn the_machine_is_asked_to_look_at_a_providers_credential_and_what_it_says_is_kept() {
    let s = Scene::new().await;
    the_machine_says(&s, "ready", "pro");
    let looked = refresh(&s, &s.ada).await.ok();
    assert_eq!(looked.facts.plan.as_deref(), Some("pro"));
    assert_eq!(looked.base.status, ProviderStatus::Ready);
    assert!(looked.facts_at.is_some());
    // The allowance is used up, and the provider stands so until the machine says otherwise.
    the_machine_says(&s, "exhausted", "pro");
    s.rig.world.app.clock.advance(chrono::Duration::minutes(1));
    assert_eq!(
        refresh(&s, &s.ada).await.ok().base.status,
        ProviderStatus::Exhausted
    );
    the_machine_says(&s, "ready", "pro");
    s.rig.world.app.clock.advance(chrono::Duration::minutes(1));
    assert_eq!(
        refresh(&s, &s.ada).await.ok().base.status,
        ProviderStatus::Ready
    );
    // A sign-in that is gone puts the provider back to needing one.
    the_machine_says(&s, "needs_setup", "pro");
    s.rig.world.app.clock.advance(chrono::Duration::minutes(1));
    let signed_out = refresh(&s, &s.ada).await.ok();
    assert_eq!(signed_out.base.status, ProviderStatus::NeedsSetup);
}

#[tokio::test]
async fn a_provider_is_looked_at_for_whoever_holds_any_permission_on_it_and_not_for_a_stranger() {
    let s = Scene::new().await;
    the_machine_says(&s, "ready", "pro");
    let user = refresh(&s, &s.eve).await;
    assert!(user.status.is_success(), "{:?}", user.body);
    let stranger = refresh(&s, &s.bea).await.problem();
    assert_eq!(stranger.code, ErrorCode::AuthzDenied);
    assert_eq!(
        missing_permissions(&stranger),
        ["provider.view"],
        "{stranger:?}"
    );
    let nobody = s
        .rig
        .world
        .app
        .call(ProviderRefresh, &[&s.on.provider])
        .body(&orch_api_contract::resources::ProviderRefreshBody::default())
        .send()
        .await
        .problem();
    assert_eq!(nobody.code, ErrorCode::AuthRequired);
}

#[tokio::test]
async fn a_provider_whose_machine_is_away_or_which_has_none_cannot_be_looked_at() {
    let s = Scene::new().await;
    go_away(&s).await;
    let away = refresh(&s, &s.ada).await.problem();
    assert_eq!(away.code, ErrorCode::SourceOffline);
    sqlx::query("UPDATE providers SET machine_id = NULL, key_machine_id = NULL WHERE id = $1")
        .bind(s.on.provider)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let none = refresh(&s, &s.ada).await.problem();
    assert_eq!(none.code, ErrorCode::ProviderUnavailable);
}

// ---------------------------------------------------------------- facts written by hand

fn by_hand(mut body: Value) -> ProviderFactsOverridesSetBody {
    body["expected_revision"] = json!(0);
    serde_json::from_value(body).unwrap()
}

async fn set_facts(
    s: &Scene,
    by: &Who,
    body: &ProviderFactsOverridesSetBody,
) -> orch_api_core::testing::Reply<Provider> {
    let mut body = body.clone();
    if body.expected_revision == 0 {
        body.expected_revision = sqlx::query_scalar("SELECT revision FROM providers WHERE id = $1")
            .bind(s.on.provider)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    }
    s.rig
        .world
        .app
        .call(ProviderFactsOverridesSet, &[&s.on.provider])
        .token(&by.token)
        .body(&body)
        .send()
        .await
}

#[tokio::test]
async fn facts_written_by_hand_win_over_what_the_machine_reports_until_they_are_taken_away() {
    let s = Scene::new().await;
    the_machine_says(&s, "ready", "pro");
    refresh(&s, &s.ada).await.ok();
    let set = set_facts(&s, &s.ada, &by_hand(json!({ "plan": "team" })))
        .await
        .ok();
    assert_eq!(set.facts.plan.as_deref(), Some("team"));
    assert_eq!(set.facts_overridden, ["plan"]);
    // The machine says its plan again; the hand-written one still wins.
    let looked = refresh(&s, &s.ada).await.ok();
    assert_eq!(looked.facts.plan.as_deref(), Some("team"));
    // What is written replaces what was written: left out, a member is as reported again.
    let cleared = set_facts(&s, &s.ada, &by_hand(json!({}))).await.ok();
    assert_eq!(cleared.facts.plan.as_deref(), Some("pro"));
    assert!(cleared.facts_overridden.is_empty());
    // Null is a value too: the person says that there is no plan.
    let none = set_facts(&s, &s.ada, &by_hand(json!({ "plan": null })))
        .await
        .ok();
    assert_eq!(none.facts.plan, None);
    assert_eq!(none.facts_overridden, ["plan"]);
}

#[tokio::test]
async fn facts_are_written_by_whoever_manages_the_provider_and_by_nobody_who_only_uses_it() {
    let s = Scene::new().await;
    let refused = set_facts(&s, &s.eve, &by_hand(json!({ "plan": "team" })))
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(
        missing_permissions(&refused),
        ["provider.manage"],
        "{refused:?}"
    );
    let read = s
        .rig
        .world
        .app
        .call(ProviderGet, &[&s.on.provider])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    assert!(read.facts_overridden.is_empty(), "nothing was written");
    let audited: Vec<String> = sqlx::query_scalar(
        "SELECT summary FROM audit_log WHERE action = 'provider.updated' AND object_id = $1",
    )
    .bind(s.on.provider)
    .fetch_all(s.rig.db.pool())
    .await
    .unwrap();
    assert!(audited.is_empty());
    set_facts(&s, &s.ada, &by_hand(json!({ "plan": "team" })))
        .await
        .ok();
    let audited: Vec<String> = sqlx::query_scalar(
        "SELECT summary FROM audit_log WHERE action = 'provider.updated' AND object_id = $1",
    )
    .bind(s.on.provider)
    .fetch_all(s.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(
        audited.len(),
        1,
        "a change by hand is told in the audit: {audited:?}"
    );
}

#[tokio::test]
async fn a_stale_fact_override_and_an_invalid_window_leave_the_current_facts_and_revision_alone() {
    let s = Scene::new().await;
    let current = s
        .rig
        .world
        .app
        .call(ProviderGet, &[&s.on.provider])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    let first = s
        .rig
        .world
        .app
        .call(ProviderFactsOverridesSet, &[&s.on.provider])
        .token(&s.ada.token)
        .raw_body(&json!({"expected_revision":current.revision, "plan":"current"}))
        .send()
        .await
        .ok();
    let stale = s
        .rig
        .world
        .app
        .call(ProviderFactsOverridesSet, &[&s.on.provider])
        .token(&s.ada.token)
        .raw_body(&json!({"expected_revision":current.revision, "plan":"old editor"}))
        .send()
        .await
        .problem();
    assert_eq!(stale.code, ErrorCode::ConflictStaleRevision);
    for window in [
        json!({"id":"weekly", "label":"Week", "duration_minutes":10080, "used_percent":-1, "resets_at":null}),
        json!({"id":"weekly", "label":"Week", "duration_minutes":0, "used_percent":20, "resets_at":null}),
        json!({"id":"weekly", "label":"Week", "duration_minutes":10080, "used_percent":20, "warning_percent":101, "resets_at":null}),
    ] {
        let invalid = s
            .rig
            .world
            .app
            .call(ProviderFactsOverridesSet, &[&s.on.provider])
            .token(&s.ada.token)
            .raw_body(&json!({"expected_revision":first.revision,"windows":[window]}))
            .send()
            .await
            .problem();
        assert_eq!(invalid.code, ErrorCode::RequestInvalid);
    }
    let after = s
        .rig
        .world
        .app
        .call(ProviderGet, &[&s.on.provider])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(after.revision, first.revision);
    assert_eq!(after.facts.plan.as_deref(), Some("current"));
    let stored: Value = sqlx::query_scalar("SELECT facts_overrides FROM providers WHERE id = $1")
        .bind(s.on.provider)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(
        stored,
        json!({"plan":"current"}),
        "the concurrency token is never stored as a fact"
    );
}

// ---------------------------------------------------------------- bringing a harness up anew

fn the_machine_stops_harnesses(s: &Scene) {
    s.rig.machine.handler.on(CommandKind::AgentStop, |_| {
        Ok(CommandResult::AgentStop(Empty {}))
    });
}

async fn restart(
    s: &Scene,
    by: &Who,
    provider: Id,
    agent: Id,
) -> orch_api_core::testing::Reply<Provider> {
    s.rig
        .world
        .app
        .call(ProviderHarnessRestart, &[&provider, &agent])
        .token(&by.token)
        .send()
        .await
}

#[tokio::test]
async fn a_harness_that_is_brought_up_anew_is_stopped_where_it_ran_and_started_again() {
    let s = Scene::new().await;
    the_machine_stops_harnesses(&s);
    let thread = s.thread().await;
    s.exchange(thread.base.id, 0, "Hello", "Hi").await;
    assert_eq!(started(&s), 1);
    let again = restart(&s, &s.ada, s.on.provider, s.on.agent).await.ok();
    assert_eq!(s.rig.commands.issued_of("agent.stop"), 1);
    assert_eq!(started(&s), 2, "it is started once more");
    assert_eq!(again.harnesses.len(), 1);
    assert_eq!(again.harnesses[0].agent.id, s.on.agent);
    // Its sessions are reopened at their next turn.
    s.exchange(thread.base.id, 1, "Again", "Yes").await;
    assert_eq!(s.rig.seen.of_kind(CommandKind::SessionOpen).len(), 2);
}

#[tokio::test]
async fn a_harness_is_restarted_by_whoever_manages_the_provider_and_only_one_it_serves() {
    let s = Scene::new().await;
    the_machine_stops_harnesses(&s);
    let refused = restart(&s, &s.eve, s.on.provider, s.on.agent)
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(
        missing_permissions(&refused),
        ["provider.manage"],
        "{refused:?}"
    );
    let unknown = restart(&s, &s.ada, s.on.provider, orch_types::new_id())
        .await
        .problem();
    assert_eq!(unknown.code, ErrorCode::NotFound);
    assert_eq!(s.rig.commands.issued_of("agent.stop"), 0);
    go_away(&s).await;
    let away = restart(&s, &s.ada, s.on.provider, s.on.agent)
        .await
        .problem();
    assert_eq!(away.code, ErrorCode::SourceOffline);
}

// ---------------------------------------------------------------- the conversations a harness holds

fn the_harness_holds(s: &Scene, native: &'static [&'static str]) {
    s.rig
        .machine
        .handler
        .on(CommandKind::AgentDiscoverSessions, move |_| {
            Ok(CommandResult::AgentDiscoverSessions(
                AgentDiscoverSessionsResult {
                    sessions: serde_json::from_value(serde_json::to_value(native
                        .iter()
                        .map(|each| {
                            json!({
                                "native_session_ref": each, "title": format!("About {each}"),
                                "cwd": "/work", "updated_at": "2026-10-12T08:00:00.000Z",
                                "adopted_thread_id": null
                            })
                        })
                        .collect::<Vec<_>>()).unwrap()).unwrap(),
                },
            ))
        });
}

async fn sessions(s: &Scene, by: &Who) -> orch_api_core::testing::Reply<Vec<NativeSession>> {
    s.rig
        .world
        .app
        .call(ProviderHarnessSessionList, &[&s.on.provider, &s.on.agent])
        .token(&by.token)
        .send()
        .await
}

#[tokio::test]
async fn saving_a_key_reads_models_before_returning_the_provider_without_adding_any() {
    orch_api_core::routes::resources::sealing::install_master_key(
        orch_db::secrets::MasterKey::new([7; 32]),
    );
    let s = Scene::new().await;
    sqlx::query(
        "UPDATE providers SET kind='api',harness_kind=NULL,agent_id=NULL,
            protocols=ARRAY['openai-responses'],upstreams=$2 WHERE id=$1",
    )
    .bind(s.on.provider)
    .bind(json!([{
        "protocol":"openai-responses","base_url":"https://model.example.test/v1","options":{}
    }]))
    .execute(s.rig.db.pool())
    .await
    .unwrap();
    let before = s
        .rig
        .world
        .app
        .call(ProviderGet, &[&s.on.provider])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    s.rig.machine.handler.on(CommandKind::ProviderStart, |_| {
        Ok(CommandResult::ProviderStart(Empty {}))
    });
    s.rig.machine.handler.on(CommandKind::ProviderDiscoverModels,|_|Ok(CommandResult::ProviderDiscoverModels(ProviderDiscoverModelsResult {
        source:None,suggestions:serde_json::from_value(json!([
            {"model_name":"from-new-key","display_name":"Fresh key model","listed":true,"parameters":{}}
        ])).unwrap(),
    })));
    let key = "sk-only-for-the-model-read-regression";
    let saved = s
        .rig
        .world
        .app
        .call(ProviderKeySet, &[&s.on.provider])
        .token(&s.ada.token)
        .body(&ProviderKeySetBody {
            api_key: key.into(),
        })
        .send()
        .await
        .ok();
    assert!(saved.key_set);
    let offers = saved.facts.models.as_ref().unwrap();
    assert_eq!(offers.len(), 1);
    assert_eq!(offers[0].display_name, "Fresh key model");
    assert_eq!(
        saved.models.len(),
        before.models.len(),
        "discovery suggests; only a manager accepts"
    );
    assert!(saved.facts.field_times.contains_key("models"));
    assert_eq!(
        serde_json::to_value(&saved).unwrap()["facts_state"]["models"],
        "told"
    );
    assert!(!serde_json::to_string(&saved).unwrap().contains(key));
    let executions = s.rig.machine.handler.executions();
    assert!(
        executions
            .iter()
            .any(|run| run.kind == CommandKind::ProviderDiscoverModels),
        "the key act asks without a separate Refresh"
    );
    let ready = executions
        .iter()
        .position(|run| run.kind == CommandKind::ProviderStart)
        .unwrap();
    let read = executions
        .iter()
        .position(|run| run.kind == CommandKind::ProviderDiscoverModels)
        .unwrap();
    assert!(
        ready < read,
        "the provider starts before its models are read"
    );
}

async fn declare_session_reader(s: &Scene) {
    let mut descriptor = rig::descriptor();
    descriptor["capabilities"]["discover_sessions"] = json!(true);
    sqlx::query("UPDATE agents SET descriptor=$2 WHERE id=$1")
        .bind(s.on.agent)
        .bind(&descriptor)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    s.rig.machine.handler.on(CommandKind::AgentStart, move |_| {
        Ok(CommandResult::AgentStart(AgentStartResult {
            reused: false,
            harness_version: "1.0.0".into(),
            descriptor: serde_json::from_value(descriptor.clone()).unwrap(),
        }))
    });
    the_harness_holds(s, &["native-a"]);
    // The endpoint reads only a provider/profile pair that is already associated by a thread.
    let _thread = s.thread().await;
}

#[tokio::test]
async fn a_ready_provider_lists_sessions_on_the_profiles_machine_without_moving_its_credential() {
    let s = Scene::new().await;
    declare_session_reader(&s).await;
    sqlx::query("DELETE FROM harnesses WHERE provider_id=$1")
        .bind(s.on.provider)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    sqlx::query(
        "UPDATE providers SET machine_id=NULL,key_machine_id=NULL,status='ready' WHERE id=$1",
    )
    .bind(s.on.provider)
    .execute(s.rig.db.pool())
    .await
    .unwrap();
    sqlx::query("UPDATE agents SET machine_id=$2 WHERE id=$1")
        .bind(s.on.agent)
        .bind(s.rig.machine_id)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let listed = sessions(&s, &s.ada).await.ok();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].title.as_deref(), Some("About native-a"));
    let provider = s
        .rig
        .world
        .app
        .call(ProviderGet, &[&s.on.provider])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(provider.base.status, ProviderStatus::Ready);
    let keeper: Option<Id> = sqlx::query_scalar("SELECT key_machine_id FROM providers WHERE id=$1")
        .bind(s.on.provider)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert!(
        keeper.is_none(),
        "reading sessions must not relocate the credential"
    );
}

#[tokio::test]
async fn a_missing_or_offline_agent_runtime_is_not_a_provider_readiness_failure() {
    let s = Scene::new().await;
    declare_session_reader(&s).await;
    sqlx::query("DELETE FROM harnesses WHERE provider_id=$1")
        .bind(s.on.provider)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    sqlx::query(
        "UPDATE providers SET machine_id=NULL,key_machine_id=NULL,status='ready' WHERE id=$1",
    )
    .bind(s.on.provider)
    .execute(s.rig.db.pool())
    .await
    .unwrap();
    sqlx::query("UPDATE agents SET machine_id=NULL WHERE id=$1")
        .bind(s.on.agent)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let starts = s.rig.seen.of_kind(CommandKind::AgentStart).len();
    assert_eq!(
        sessions(&s, &s.ada).await.problem().code,
        ErrorCode::HarnessSessionsNoRuntime
    );
    sqlx::query("UPDATE agents SET machine_id=$2 WHERE id=$1")
        .bind(s.on.agent)
        .bind(s.rig.machine_id)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    go_away(&s).await;
    assert_eq!(
        sessions(&s, &s.ada).await.problem().code,
        ErrorCode::SourceOffline
    );
    assert_eq!(
        s.rig.seen.of_kind(CommandKind::AgentStart).len(),
        starts,
        "a missing/offline runtime cannot be started by a read"
    );
    let provider = s
        .rig
        .world
        .app
        .call(ProviderGet, &[&s.on.provider])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(provider.base.status, ProviderStatus::Ready);
}

#[tokio::test]
async fn the_conversations_a_harness_holds_are_listed_with_the_thread_that_adopted_each() {
    let s = Scene::new().await;
    sqlx::query(
        "UPDATE agents SET descriptor = jsonb_set(descriptor, '{capabilities,discover_sessions}', 'true') WHERE id = $1",
    )
    .bind(s.on.agent)
    .execute(s.rig.db.pool())
    .await
    .unwrap();
    the_harness_holds(&s, &["native-a", "native-b"]);
    // The harness declares what it can do each time it starts, so that the machine says it too.
    let mut declares = rig::descriptor();
    declares["capabilities"]["discover_sessions"] = json!(true);
    s.rig.machine.handler.on(CommandKind::AgentStart, move |_| {
        Ok(CommandResult::AgentStart(AgentStartResult {
            reused: false,
            harness_version: "1.0.0".to_owned(),
            descriptor: serde_json::from_value(declares.clone()).unwrap(),
        }))
    });
    let thread = s.thread().await;
    sqlx::query("UPDATE segments SET native_session_ref = 'native-b' WHERE thread_id = $1")
        .bind(thread.base.id)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let listed = sessions(&s, &s.eve).await.ok();
    assert_eq!(
        listed
            .iter()
            .map(|each| (each.native_session_ref.as_str(), each.adopted_thread_id))
            .collect::<Vec<_>>(),
        [("native-a", None), ("native-b", Some(thread.base.id))]
    );
    assert_eq!(listed[0].title.as_deref(), Some("About native-a"));
}

#[tokio::test]
async fn a_harness_that_cannot_list_its_conversations_says_so_and_only_whoever_uses_the_provider_asks()
 {
    let s = Scene::new().await;
    the_harness_holds(&s, &["native-a"]);
    let unsupported = sessions(&s, &s.ada).await.problem();
    assert_eq!(unsupported.code, ErrorCode::CapabilityUnsupported);
    let viewer = sessions(&s, &s.cai).await.problem();
    assert_eq!(viewer.code, ErrorCode::AuthzDenied);
    assert_eq!(missing_permissions(&viewer), ["provider.use"], "{viewer:?}");
    let nobody = s
        .rig
        .world
        .app
        .call(ProviderHarnessSessionList, &[&s.on.provider, &s.on.agent])
        .send()
        .await
        .problem();
    assert_eq!(nobody.code, ErrorCode::AuthRequired);
}
