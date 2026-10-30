//! A thread that has no history is put on another agent profile, provider and environment by whoever manages it; one that has history is sent to a switch; whoever binds is asked for their own use of what they choose, all of it at once, and never again.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod binding_support;
mod places;
mod rig;
mod support;

use binding_support::{
    a_directory_on_a_machine, another_environment, another_provider, names, offering_plain,
};
use orch_api_contract::binding::{
    ArrangementType, EnvironmentChoice, ThreadBindingSet, ThreadBindingSetBody,
    ThreadEnvironmentSet, ThreadEnvironmentSetBody,
};
use orch_api_contract::common::{ErrorCode, Problem};
use orch_api_contract::nodes::{CreateThread, CreateThreadBinding, ThreadCreate};
use orch_api_contract::threads::Thread;
use orch_types::Id;
use places::{Who, missing_permissions};
use rig::{MODEL, Scene};
use serde_json::json;

async fn bind(
    s: &Scene,
    who: &Who,
    thread: Id,
    agent: Id,
    provider: Id,
    arrangement: Option<ArrangementType>,
) -> orch_api_core::testing::Reply<Thread> {
    s.rig
        .world
        .app
        .call(ThreadBindingSet, &[&thread])
        .token(&who.token)
        .body(&ThreadBindingSetBody {
            agent_id: agent,
            provider_id: provider,
            arrangement,
            model: None,
            effort: None,
        })
        .send()
        .await
}

async fn place(
    s: &Scene,
    who: &Who,
    thread: Id,
    choice: EnvironmentChoice,
    directory: Option<&str>,
) -> orch_api_core::testing::Reply<Thread> {
    s.rig
        .world
        .app
        .call(ThreadEnvironmentSet, &[&thread])
        .token(&who.token)
        .body(&ThreadEnvironmentSetBody {
            choice,
            directory: directory.map(str::to_owned),
        })
        .send()
        .await
}

async fn segment_of(s: &Scene, thread: Id) -> (Id, Id, Id, String) {
    sqlx::query_as(
        "SELECT agent_id, provider_id, environment_id, arrangement FROM segments WHERE thread_id = $1",
    )
    .bind(thread)
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap()
}

async fn bound_lines(s: &Scene, thread: Id) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'thread.bound' AND node_id = $1",
    )
    .bind(thread)
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap()
}

#[tokio::test]
async fn a_thread_without_history_is_put_on_another_profile_and_provider_and_its_first_segment_goes_with_it()
 {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let (agent, provider) = another_provider(&s, "Second", &["separated"]).await;
    let put = s
        .rig
        .world
        .app
        .call(ThreadBindingSet, &[&thread])
        .token(&s.ada.token)
        .body(&ThreadBindingSetBody {
            agent_id: agent,
            provider_id: provider,
            arrangement: None,
            model: Some(MODEL.to_owned()),
            effort: Some("high".to_owned()),
        })
        .send()
        .await
        .ok();
    let binding = put.binding.expect("the thread is bound");
    assert_eq!(binding.provider.id, provider);
    assert_eq!(binding.agent.id, agent);
    assert_eq!(binding.arrangement, ArrangementType::Together);
    assert_eq!(put.model.as_deref(), Some(MODEL));
    assert_eq!(put.effort.as_deref(), Some("high"));
    let (segment_agent, segment_provider, _, arrangement) = segment_of(&s, thread).await;
    assert_eq!(
        (segment_agent, segment_provider, arrangement.as_str()),
        (agent, provider, "together"),
        "no session was opened in the first segment: it is put on the same"
    );
    assert_eq!(bound_lines(&s, thread).await, 1, "a line in its history");
}

#[tokio::test]
async fn a_thread_with_history_is_sent_to_a_switch_and_is_left_as_it_is() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    s.exchange(thread, 0, "hello", "hi").await;
    let (agent, provider) = another_provider(&s, "Second", &["separated"]).await;
    let refused = bind(&s, &s.ada, thread, agent, provider, None)
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::ThreadUseSwitch);
    let elsewhere = another_environment(&s, "Second", Some(s.rig.machine_id)).await;
    let refused = place(&s, &s.ada, thread, EnvironmentChoice::Use(elsewhere), None)
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::ThreadUseSwitch);
    let (segment_agent, segment_provider, segment_environment, _) = segment_of(&s, thread).await;
    assert_eq!(
        (segment_agent, segment_provider, segment_environment),
        (s.on.agent, s.on.provider, s.on.environment)
    );
    assert_eq!(bound_lines(&s, thread).await, 0);
}

#[tokio::test]
async fn an_input_that_waited_for_a_provider_goes_on_when_the_thread_is_put_on_one_that_works() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    sqlx::query("UPDATE providers SET status = 'disabled' WHERE id = $1")
        .bind(s.on.provider)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let sent = s
        .say(
            &s.ada,
            thread,
            "hello",
            orch_api_contract::threads::Delivery::Queue,
            "first",
        )
        .await;
    assert!(sent.status.is_success(), "{:?}", sent.body);
    s.rig.wait_state(thread, "blocked").await;
    assert_eq!(
        s.rig.state(thread).await.1.as_deref(),
        Some("provider_unavailable")
    );
    let (agent, provider) = another_provider(&s, "Second", &["separated"]).await;
    bind(&s, &s.ada, thread, agent, provider, None).await.ok();
    let start = s.rig.turn_start(thread, 0).await;
    assert!(start.text.contains("hello"), "{}", start.text);
    s.rig.answer(thread, &start, "m0", "hi").await;
    s.rig
        .say(
            thread,
            rig::turn_ended(
                start.turn_id,
                orch_link_contract::events::TurnOutcome::Completed,
            ),
        )
        .await;
    s.rig.wait_turn(start.turn_id, "completed").await;
}

#[tokio::test]
async fn whoever_binds_is_asked_for_their_own_use_of_both_at_once_and_is_never_asked_again() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let (agent, provider) = another_provider(&s, "Second", &["separated"]).await;
    names(&s, "node", thread, "thread.manage", &s.dan, json!({})).await;
    let refused = bind(&s, &s.dan, thread, agent, provider, None)
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    let missing = missing_permissions(&refused);
    assert!(
        missing.contains(&"agent.use".to_owned()) && missing.contains(&"provider.use".to_owned()),
        "everything that is lacking is told together: {missing:?}"
    );
    assert_eq!(bound_lines(&s, thread).await, 0, "nothing was changed");

    names(&s, "agent", agent, "agent.use", &s.dan, json!({})).await;
    names(&s, "provider", provider, "provider.use", &s.dan, json!({})).await;
    let put = bind(&s, &s.dan, thread, agent, provider, None).await.ok();
    assert_eq!(put.binding.expect("bound").provider.id, provider);

    // Dan's use is taken back: the thread holds nothing of it, and stays where it was put.
    sqlx::query("DELETE FROM permission_entries WHERE holder_id = $1 AND permission IN ('agent.use', 'provider.use')")
        .bind(s.dan.id())
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let read = s
        .rig
        .world
        .app
        .call(orch_api_contract::nodes::NodeGet, &[&thread])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    match read {
        orch_api_contract::nodes::Node::Thread(found) => {
            assert_eq!(found.binding.expect("bound").provider.id, provider);
        }
        other => panic!("not a thread: {other:?}"),
    }
}

#[tokio::test]
async fn somebody_who_does_not_manage_the_thread_is_refused_with_what_is_missing() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let (agent, provider) = another_provider(&s, "Second", &["separated"]).await;
    let refused = bind(&s, &s.cai, thread, agent, provider, None)
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert!(missing_permissions(&refused).contains(&"thread.manage".to_owned()));
    let refused = place(
        &s,
        &s.cai,
        thread,
        EnvironmentChoice::Use(s.on.environment),
        None,
    )
    .await
    .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(bound_lines(&s, thread).await, 0);
}

#[tokio::test]
async fn a_model_or_an_effort_the_provider_does_not_offer_is_refused() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let (agent, provider) = another_provider(&s, "Second", &["separated"]).await;
    for (model, effort, reason) in [
        ("no-such-model", None, "model"),
        (MODEL, Some("ultra"), "effort"),
    ] {
        let refused = s
            .rig
            .world
            .app
            .call(ThreadBindingSet, &[&thread])
            .token(&s.ada.token)
            .body(&ThreadBindingSetBody {
                agent_id: agent,
                provider_id: provider,
                arrangement: None,
                model: Some(model.to_owned()),
                effort: effort.map(str::to_owned),
            })
            .send()
            .await
            .problem();
        assert_eq!(refused.code, ErrorCode::ModelNotAllowed, "{model}");
        assert_eq!(refused.extra.get("reason"), Some(&json!(reason)));
    }
    assert_eq!(bound_lines(&s, thread).await, 0);
}

#[tokio::test]
async fn an_arrangement_that_is_not_permitted_is_refused_and_says_whose_decision_that_is() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    offering_plain(&s, s.on.environment).await;
    let (agent, provider) = another_provider(&s, "Closed", &["key_separated"]).await;
    let refused = bind(
        &s,
        &s.ada,
        thread,
        agent,
        provider,
        Some(ArrangementType::Separated),
    )
    .await
    .problem();
    assert_eq!(refused.code, ErrorCode::BindingIncompatible);
    assert_eq!(
        refused.extra.get("reason"),
        Some(&json!("arrangement_not_allowed"))
    );
    assert_eq!(
        refused.extra.get("not_allowed_by"),
        Some(&json!("provider")),
        "the provider's owner has not allowed it"
    );
    // The way that needs no consent is still open.
    let put = bind(
        &s,
        &s.ada,
        thread,
        agent,
        provider,
        Some(ArrangementType::Together),
    )
    .await
    .ok();
    assert_eq!(
        put.binding.expect("bound").arrangement,
        ArrangementType::Together
    );
}

#[tokio::test]
async fn a_profile_that_never_reported_what_its_harness_is_cannot_be_chosen_while_its_harness_cannot_be_asked()
 {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let (agent, provider) = another_provider(&s, "Second", &["separated"]).await;
    sqlx::query("UPDATE agents SET descriptor = '{}' WHERE id = $1")
        .bind(agent)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    // The machine that would run the harness is away, so nothing can say what the harness is.
    sqlx::query("UPDATE machines SET online = false WHERE id = $1")
        .bind(s.rig.machine_id)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let refused = bind(&s, &s.ada, thread, agent, provider, None)
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::BindingIncompatible);
    assert_eq!(
        refused.extra.get("reason"),
        Some(&json!("harness_unreported"))
    );
}

#[tokio::test]
async fn a_provider_that_is_switched_off_is_not_one_a_thread_can_be_put_on() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let (agent, provider) = another_provider(&s, "Second", &["separated"]).await;
    sqlx::query("UPDATE providers SET status = 'disabled' WHERE id = $1")
        .bind(provider)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let refused = bind(&s, &s.ada, thread, agent, provider, None)
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
}

#[tokio::test]
async fn a_thread_without_history_is_put_into_another_environment_that_its_caller_manages() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let second = another_environment(&s, "Second", Some(s.rig.machine_id)).await;
    let put = place(
        &s,
        &s.ada,
        thread,
        EnvironmentChoice::Use(second),
        Some("api/v2"),
    )
    .await
    .ok();
    assert_eq!(put.environment.expect("in one").id, second);
    assert_eq!(put.directory, "api/v2");
    let (_, _, segment_environment, _) = segment_of(&s, thread).await;
    assert_eq!(
        segment_environment, second,
        "its first segment goes with it"
    );
    assert_eq!(bound_lines(&s, thread).await, 1);
}

#[tokio::test]
async fn a_thread_that_joins_an_environment_without_naming_a_directory_gets_the_directory_of_the_thread_it_joins()
 {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let second = another_environment(&s, "Second", Some(s.rig.machine_id)).await;
    s.rig
        .world
        .app
        .call(ThreadCreate, &[&s.tree.topic.base.id])
        .token(&s.ada.token)
        .body(&CreateThread {
            name: Some("Sibling".to_owned()),
            binding: Some(CreateThreadBinding {
                agent_id: s.on.agent,
                provider_id: s.on.provider,
                arrangement: None,
            }),
            model: Some(MODEL.to_owned()),
            environment: Some(EnvironmentChoice::Use(second)),
            directory: Some("shared/dir".to_owned()),
            ..CreateThread::default()
        })
        .send()
        .await
        .ok();
    let put = place(&s, &s.ada, thread, EnvironmentChoice::Use(second), None)
        .await
        .ok();
    assert_eq!(put.directory, "shared/dir");
}

#[tokio::test]
async fn a_directory_that_leaves_the_workspace_is_refused() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let second = another_environment(&s, "Second", Some(s.rig.machine_id)).await;
    for directory in ["../elsewhere", "/etc", "a//b"] {
        let refused = place(
            &s,
            &s.ada,
            thread,
            EnvironmentChoice::Use(second),
            Some(directory),
        )
        .await
        .problem();
        assert_eq!(refused.code, ErrorCode::RequestInvalid, "{directory}");
    }
}

#[tokio::test]
async fn an_environment_the_caller_does_not_manage_is_refused_with_what_is_missing() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    names(&s, "node", thread, "thread.manage", &s.eve, json!({})).await;
    let hers = a_directory_on_a_machine(&s, "Ada's laptop").await;
    let refused = place(&s, &s.eve, thread, EnvironmentChoice::Use(hers), None)
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert!(missing_permissions(&refused).contains(&"environment.manage".to_owned()));
}

#[tokio::test]
async fn an_environment_the_agent_cannot_work_in_is_refused_and_the_thread_stays_where_it_is() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    // On no machine, the harness cannot run beside its tools, and nothing offers it a way.
    let nowhere = another_environment(&s, "Nowhere", None).await;
    let refused = place(&s, &s.ada, thread, EnvironmentChoice::Use(nowhere), None)
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::BindingIncompatible);
    assert!(refused.extra.get("reason").is_some());
    let (_, _, segment_environment, _) = segment_of(&s, thread).await;
    assert_eq!(segment_environment, s.on.environment);
}

#[tokio::test]
async fn an_environment_whose_removal_is_scheduled_takes_no_thread() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let doomed = another_environment(&s, "Doomed", Some(s.rig.machine_id)).await;
    sqlx::query("UPDATE environments SET remove_at = now() WHERE id = $1")
        .bind(doomed)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let refused = place(&s, &s.ada, thread, EnvironmentChoice::Use(doomed), None)
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
}

#[tokio::test]
async fn a_new_environment_is_made_by_the_request_that_makes_environments_and_named_here() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let refused = place(
        &s,
        &s.ada,
        thread,
        EnvironmentChoice::New(Box::default()),
        None,
    )
    .await
    .problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
    assert_eq!(bound_lines(&s, thread).await, 0);
}

#[tokio::test]
async fn a_thread_that_is_archived_cannot_be_put_on_anything_else() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    sqlx::query("UPDATE nodes SET archived_at = now(), archived_by = $2 WHERE id = $1")
        .bind(thread)
        .bind(s.ada.id())
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let (agent, provider) = another_provider(&s, "Second", &["separated"]).await;
    let refused: Problem = bind(&s, &s.ada, thread, agent, provider, None)
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::NodeArchived);
}
