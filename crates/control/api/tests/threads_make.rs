//! A thread is made in a topic: bound to a provider with model and effort, an agent profile and an environment, each checked for the caller by the access engine with everything missing in the refusal; with what the topic defaults to for what the request leaves out; and read back as a node.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod rig;
mod support;

use orch_api_contract::access::{AccessPreset, ThreadAccess};
use orch_api_contract::binding::EnvironmentChoice;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::nodes::{
    CreateThread, CreateThreadBinding, FieldSource, FirstInput, Node, NodeChildList,
    NodeChildListQuery, NodeGet, ThreadCreate,
};
use orch_api_contract::threads::{ContentPart, ThreadState};
use orch_settings::Target;
use places::{first_missing, missing_permissions};
use rig::{MODEL, Scene};
use serde_json::json;

fn binding(scene: &Scene) -> CreateThreadBinding {
    CreateThreadBinding {
        agent_id: scene.on.agent,
        provider_id: scene.on.provider,
        arrangement: None,
    }
}

#[tokio::test]
async fn a_temporary_environment_records_its_creating_thread_in_the_same_thread_creation_request() {
    let scene = Scene::new().await;
    sqlx::query("UPDATE environments SET config=config || $2 WHERE id=$1")
        .bind(scene.on.environment)
        .bind(json!({"env.lifetime":"thread"}))
        .execute(scene.rig.db.pool())
        .await
        .unwrap();
    let body = CreateThread {
        binding: Some(binding(&scene)),
        model: Some(MODEL.to_owned()),
        environment: Some(EnvironmentChoice::Use(scene.on.environment)),
        ..CreateThread::default()
    };
    let first = scene
        .rig
        .world
        .app
        .call(ThreadCreate, &[&scene.tree.topic.base.id])
        .token(&scene.ada.token)
        .body(&body)
        .send()
        .await
        .ok();
    let second = scene
        .rig
        .world
        .app
        .call(ThreadCreate, &[&scene.tree.topic.base.id])
        .token(&scene.ada.token)
        .body(&body)
        .send()
        .await
        .ok();
    assert_ne!(first.base.id, second.base.id);
    let creator: Option<orch_types::Id> =
        sqlx::query_scalar("SELECT thread_id FROM environment_lifetimes WHERE environment_id=$1")
            .bind(scene.on.environment)
            .fetch_one(scene.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(
        creator,
        Some(first.base.id),
        "using it later must not transfer its archive trigger"
    );
}

#[tokio::test]
async fn a_thread_is_made_by_a_developer_and_read_back_as_a_node_that_its_maker_owns() {
    let s = Scene::new().await;
    let thread = s.thread().await;
    assert_eq!(thread.base.owner.username, "ada");
    assert!(thread.base.access.owns);
    assert_eq!(thread.state, ThreadState::Idle);
    assert!(thread.can.fork && thread.can.archive && thread.can.place);
    assert_eq!(thread.sides.as_ref().map(|sides| sides.open), Some(0));
    assert!(
        !thread.base.chat_id.is_nil(),
        "a node has its chat from the start"
    );
    let read = s
        .rig
        .world
        .app
        .call(NodeGet, &[&thread.base.id])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    assert!(matches!(&read, Node::Thread(found) if found.base.id == thread.base.id));
    let children = s
        .rig
        .world
        .app
        .call(NodeChildList, &[&s.tree.topic.base.id])
        .token(&s.ada.token)
        .query(&NodeChildListQuery::default())
        .send()
        .await
        .ok();
    assert!(
        children
            .items
            .iter()
            .filter_map(orch_api_contract::common::Listed::as_visible)
            .any(|node| matches!(node, Node::Thread(found) if found.base.id == thread.base.id)),
        "the topic lists its thread"
    );
}

#[tokio::test]
async fn a_named_thread_keeps_its_name_from_automation_and_an_unnamed_one_is_new_thread() {
    let s = Scene::new().await;
    let named = s.thread().await;
    assert_eq!(named.base.title_source, FieldSource::User);
    let unnamed = s
        .rig
        .world
        .app
        .call(ThreadCreate, &[&s.tree.topic.base.id])
        .token(&s.ada.token)
        .body(&CreateThread {
            binding: Some(binding(&s)),
            model: Some(MODEL.to_owned()),
            environment: Some(EnvironmentChoice::Use(s.on.environment)),
            ..CreateThread::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(unnamed.base.name, "New thread");
    assert_eq!(unnamed.base.title_source, FieldSource::Auto);
}

#[tokio::test]
async fn what_the_request_leaves_out_comes_from_the_topic_when_the_caller_may_use_it() {
    let s = Scene::new().await;
    let topic = s.tree.topic.base.id;
    s.rig
        .world
        .set_at(
            Target::Node(topic),
            "agent.default_provider_id",
            json!(s.on.provider),
        )
        .await;
    s.rig
        .world
        .set_at(
            Target::Node(topic),
            "agent.default_agent_id",
            json!(s.on.agent),
        )
        .await;
    s.rig
        .world
        .set_at(Target::Node(topic), "agent.model", json!(MODEL))
        .await;
    sqlx::query("INSERT INTO settings (node_id,key,value) VALUES ($1,'env.default_id',to_jsonb($2::uuid::text)) ON CONFLICT (node_id,key) DO UPDATE SET value=excluded.value")
        .bind(topic)
        .bind(s.on.environment)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let make = |who: &places::Who| {
        let call = s
            .rig
            .world
            .app
            .call(ThreadCreate, &[&topic])
            .token(&who.token)
            .body(&CreateThread::default());
        async move { call.send().await }
    };
    // Where the caller may use all of it, `{}` makes a thread that is ready.
    let thread = make(&s.eve).await.ok();
    assert_eq!(
        thread.binding.as_ref().map(|each| each.provider.id),
        Some(s.on.provider)
    );
    assert_eq!(thread.model.as_deref(), Some(MODEL));
    assert_eq!(
        thread
            .environment
            .as_ref()
            .map(|environment| environment.id),
        Some(s.on.environment)
    );
    // Where the caller may not use the default provider, there is no binding to make.
    let refused = make(&s.dan).await.problem();
    assert_eq!(refused.code, ErrorCode::BindingIncompatible);
    assert_eq!(refused.extra["reason"], "unbound");
}

#[tokio::test]
async fn a_caller_who_lacks_what_the_thread_runs_on_is_refused_with_everything_missing() {
    let s = Scene::new().await;
    // Dan may start work in the topic and manage environments made from a template, and has neither
    // the profile nor the provider.
    let refused = s
        .rig
        .world
        .app
        .call(ThreadCreate, &[&s.tree.topic.base.id])
        .token(&s.dan.token)
        .body(&CreateThread {
            binding: Some(binding(&s)),
            model: Some(MODEL.to_owned()),
            environment: Some(EnvironmentChoice::Use(s.on.environment)),
            ..CreateThread::default()
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    let lacking = missing_permissions(&refused);
    assert!(lacking.contains(&"agent.use".to_owned()), "{lacking:?}");
    assert!(lacking.contains(&"provider.use".to_owned()), "{lacking:?}");
    let provider = refused.extra["missing"]
        .as_array()
        .unwrap()
        .iter()
        .find(|missing| missing["permissions"] == json!(["provider.use"]))
        .unwrap();
    assert_eq!(
        provider["ask"][0]["username"], "ada",
        "the owner can give it"
    );
    assert_eq!(
        s.rig.turn_starts(s.on.agent).len(),
        0,
        "nothing was made, nothing was started"
    );
}

#[tokio::test]
async fn somebody_who_may_not_start_work_in_the_topic_is_refused_before_anything_else() {
    let s = Scene::new().await;
    for who in [&s.cai, &s.bea] {
        let refused = s
            .rig
            .world
            .app
            .call(ThreadCreate, &[&s.tree.topic.base.id])
            .token(&who.token)
            .body(&CreateThread {
                binding: Some(binding(&s)),
                ..CreateThread::default()
            })
            .send()
            .await
            .problem();
        assert_eq!(refused.code, ErrorCode::AuthzDenied, "{who:?}");
        assert_eq!(refused.extra["required"], "topic.interact");
        assert_eq!(
            first_missing(&refused)["permissions"],
            json!(["topic.interact"])
        );
    }
}

#[tokio::test]
async fn a_thread_made_private_is_its_makers_alone() {
    let s = Scene::new().await;
    let private = s
        .rig
        .world
        .app
        .call(ThreadCreate, &[&s.tree.topic.base.id])
        .token(&s.eve.token)
        .body(&CreateThread {
            name: Some("Quiet".to_owned()),
            access: Some(ThreadAccess::Preset(AccessPreset::Private)),
            binding: Some(binding(&s)),
            model: Some(MODEL.to_owned()),
            environment: Some(EnvironmentChoice::Use(s.on.environment)),
            ..CreateThread::default()
        })
        .send()
        .await
        .ok();
    let read = |who: &places::Who| {
        let call = s
            .rig
            .world
            .app
            .call(NodeGet, &[&private.base.id])
            .token(&who.token);
        async move { call.send().await }
    };
    assert!(read(&s.eve).await.body.is_ok(), "its maker reads it");
    assert_eq!(read(&s.dan).await.problem().code, ErrorCode::AuthzDenied);
    assert_eq!(
        read(&s.cai).await.problem().code,
        ErrorCode::AuthzDenied,
        "not even a viewer of the org"
    );
}

#[tokio::test]
async fn a_model_and_an_effort_must_be_ones_the_provider_lists() {
    let s = Scene::new().await;
    let make = |model: &str, effort: Option<&str>| {
        let call = s
            .rig
            .world
            .app
            .call(ThreadCreate, &[&s.tree.topic.base.id])
            .token(&s.ada.token)
            .body(&CreateThread {
                binding: Some(binding(&s)),
                model: Some(model.to_owned()),
                effort: effort.map(str::to_owned),
                environment: Some(EnvironmentChoice::Use(s.on.environment)),
                ..CreateThread::default()
            });
        async move { call.send().await }
    };
    let unknown = make("nonesuch", None).await.problem();
    assert_eq!(unknown.code, ErrorCode::ModelNotAllowed);
    assert_eq!(unknown.extra["reason"], "model");
    let odd = make(MODEL, Some("ultra")).await.problem();
    assert_eq!(odd.code, ErrorCode::ModelNotAllowed);
    assert_eq!(odd.extra["reason"], "effort");
    let made = make(MODEL, Some("high")).await.ok();
    assert_eq!(made.effort.as_deref(), Some("high"));
}

#[tokio::test]
async fn an_input_sent_with_the_thread_starts_it_at_once() {
    let s = Scene::new().await;
    let made = s
        .rig
        .world
        .app
        .call(ThreadCreate, &[&s.tree.topic.base.id])
        .token(&s.ada.token)
        .body(&CreateThread {
            binding: Some(binding(&s)),
            model: Some(MODEL.to_owned()),
            environment: Some(EnvironmentChoice::Use(s.on.environment)),
            first_input: Some(FirstInput {
                content: vec![ContentPart::Text {
                    text: "Begin".to_owned(),
                }],
                client_id: "first".to_owned(),
            }),
            ..CreateThread::default()
        })
        .send()
        .await
        .ok();
    let start = s.rig.turn_start(made.base.id, 0).await;
    assert_eq!(start.thread_id, made.base.id);
}

#[tokio::test]
async fn creating_a_thread_does_not_offer_an_instruction_file_write() {
    let s = Scene::new().await;
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM threads")
        .fetch_one(&s.rig.world.db.pool)
        .await
        .unwrap();
    let refused = s
        .rig
        .world
        .app
        .call(ThreadCreate, &[&s.tree.topic.base.id])
        .token(&s.ada.token)
        .raw_body(
            &json!({"binding":binding(&s),"model":MODEL,"environment":{"use":s.on.environment},
            "instruction_file":{"text":"Be brief.","expected":"none"}}),
        )
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM threads")
        .fetch_one(&s.rig.world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        after, before,
        "an undeclared field cannot create a thread or write a file"
    );
}

#[tokio::test]
async fn an_unmanageable_environment_and_an_invalid_new_environment_are_refused() {
    let s = Scene::new().await;
    let make = |environment: EnvironmentChoice| {
        let call = s
            .rig
            .world
            .app
            .call(ThreadCreate, &[&s.tree.topic.base.id])
            .token(&s.eve.token)
            .body(&CreateThread {
                binding: Some(binding(&s)),
                environment: Some(environment),
                ..CreateThread::default()
            });
        async move { call.send().await }
    };
    let missing_one = make(EnvironmentChoice::Use(orch_types::new_id()))
        .await
        .problem();
    assert_eq!(missing_one.code, ErrorCode::RequestInvalid);
    let fresh = make(EnvironmentChoice::New(Box::new(
        orch_api_contract::binding::NewEnvironment {
            name: Some(String::new()),
            ..Default::default()
        },
    )))
    .await
    .problem();
    assert_eq!(fresh.code, ErrorCode::RequestInvalid);
    assert_eq!(fresh.extra["errors"][0]["path"], json!("name"));
    // Without one named, and none the topic shares, there is none to use.
    let none = s
        .rig
        .world
        .app
        .call(ThreadCreate, &[&s.tree.topic.base.id])
        .token(&s.eve.token)
        .body(&CreateThread {
            binding: Some(binding(&s)),
            ..CreateThread::default()
        })
        .send()
        .await
        .problem();
    assert_eq!(none.code, ErrorCode::EnvironmentNoneUsable);
}

#[tokio::test]
async fn a_directory_stays_in_the_workspace_and_an_archived_topic_takes_no_thread() {
    let s = Scene::new().await;
    let refused = s
        .rig
        .world
        .app
        .call(ThreadCreate, &[&s.tree.topic.base.id])
        .token(&s.ada.token)
        .body(&CreateThread {
            binding: Some(binding(&s)),
            environment: Some(EnvironmentChoice::Use(s.on.environment)),
            directory: Some("../elsewhere".to_owned()),
            ..CreateThread::default()
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
    s.rig
        .world
        .app
        .call(
            orch_api_contract::nodes::NodeArchive,
            &[&s.tree.topic.base.id],
        )
        .token(&s.ada.token)
        .body(&places::archive_body(&s.rig.world, &s.ada.token, s.tree.topic.base.id, true).await)
        .send()
        .await
        .ok();
    let archived = s
        .rig
        .world
        .app
        .call(ThreadCreate, &[&s.tree.topic.base.id])
        .token(&s.ada.token)
        .body(&CreateThread {
            binding: Some(binding(&s)),
            environment: Some(EnvironmentChoice::Use(s.on.environment)),
            ..CreateThread::default()
        })
        .send()
        .await
        .problem();
    assert_eq!(archived.code, ErrorCode::NodeArchived);
}

#[tokio::test]
async fn a_thread_is_made_in_a_topic_and_nowhere_else() {
    let s = Scene::new().await;
    let refused = s
        .rig
        .world
        .app
        .call(ThreadCreate, &[&s.tree.project.base.id])
        .token(&s.ada.token)
        .body(&CreateThread {
            binding: Some(binding(&s)),
            ..CreateThread::default()
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::NotFound);
}
