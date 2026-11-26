//! A thread is made in a topic: bound to a provider with model and effort, an agent profile and an environment, each checked for the caller by the access engine with everything missing in the refusal; with what the topic defaults to for what the request leaves out; and read back as a node.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod rig;
mod support;

use crate::support::FullArchiveReply as _;
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
    CreateThreadBinding::provider(scene.on.agent, scene.on.provider, None)
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
        environment: Some(EnvironmentChoice::existing(scene.on.environment)),
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
        .ok()
        .into_full()
        .unwrap();
    let second = scene
        .rig
        .world
        .app
        .call(ThreadCreate, &[&scene.tree.topic.base.id])
        .token(&scene.ada.token)
        .body(&body)
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
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
            environment: Some(EnvironmentChoice::existing(s.on.environment)),
            ..CreateThread::default()
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
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
            json!({"type":"single","resource":{"kind":"provider","id":s.on.provider}}),
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
        .set_at(
            Target::Node(topic),
            "agent.model",
            json!({"type":"offering","name":MODEL}),
        )
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
    let thread = make(&s.eve).await.ok().into_full().unwrap();
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
            environment: Some(EnvironmentChoice::existing(s.on.environment)),
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
        assert_eq!(refused.extra["required"], "topic.start_threads");
        assert_eq!(
            first_missing(&refused)["permissions"],
            json!(["topic.start_threads"])
        );
    }
}

#[tokio::test]
async fn a_person_given_start_threads_alone_can_create_and_own_a_thread() {
    let s = Scene::new().await;
    for (kind, id, permission) in [
        ("node", s.tree.topic.base.id, "topic.start_threads"),
        ("provider", s.on.provider, "provider.use"),
        ("agent", s.on.agent, "agent.use"),
        ("environment", s.on.environment, "environment.use"),
    ] {
        s.rig
            .lets_use(kind, id, permission, &s.cai, json!({}))
            .await;
    }
    let shown = s
        .rig
        .world
        .app
        .call(NodeGet, &[&s.tree.topic.base.id])
        .token(&s.cai.token)
        .send()
        .await
        .ok();
    let Node::Topic(topic) = shown else {
        panic!("the topic is visible")
    };
    assert!(
        topic
            .base
            .access
            .permissions
            .contains(&"topic.start_threads".to_owned())
    );
    assert!(
        !topic
            .base
            .access
            .permissions
            .contains(&"topic.interact".to_owned())
    );
    let thread = s
        .rig
        .world
        .app
        .call(ThreadCreate, &[&s.tree.topic.base.id])
        .token(&s.cai.token)
        .body(&CreateThread {
            binding: Some(binding(&s)),
            model: Some(MODEL.to_owned()),
            environment: Some(EnvironmentChoice::existing(s.on.environment)),
            ..CreateThread::default()
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
    assert_eq!(thread.base.owner.id, s.cai.id());
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
            environment: Some(EnvironmentChoice::existing(s.on.environment)),
            ..CreateThread::default()
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
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
                environment: Some(EnvironmentChoice::existing(s.on.environment)),
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
    let made = make(MODEL, Some("high")).await.ok().into_full().unwrap();
    assert_eq!(made.effort.as_deref(), Some("high"));
}

#[tokio::test]
async fn a_thread_can_start_with_a_generation_model_and_explains_why_an_embedding_model_cannot() {
    let s = Scene::new().await;
    let request = CreateThread {
        binding: Some(binding(&s)),
        model: Some(MODEL.to_owned()),
        environment: Some(EnvironmentChoice::existing(s.on.environment)),
        ..CreateThread::default()
    };
    for purpose in ["embedding", "generation"] {
        sqlx::query("UPDATE provider_models SET parameters=parameters || $3 WHERE provider_id=$1 AND model_name=$2")
            .bind(s.on.provider)
            .bind(MODEL)
            .bind(json!({"purposes":[purpose]}))
            .execute(s.rig.db.pool()).await.unwrap();
        let result = s
            .rig
            .world
            .app
            .call(ThreadCreate, &[&s.tree.topic.base.id])
            .token(&s.ada.token)
            .body(&request)
            .send()
            .await;
        if purpose == "embedding" {
            let refusal = result.problem();
            assert_eq!(refusal.code, ErrorCode::ModelNotAllowed);
            assert_eq!(refusal.extra["reason"], "generation_unsupported");
            assert_eq!(refusal.extra["model"], MODEL);
        } else {
            assert_eq!(
                result.ok().into_full().unwrap().model.as_deref(),
                Some(MODEL)
            );
        }
    }
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
            environment: Some(EnvironmentChoice::existing(s.on.environment)),
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
        .ok()
        .into_full()
        .unwrap();
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
            &json!({"binding":binding(&s),"model":MODEL,"environment":orch_api_contract::binding::EnvironmentChoice::existing(s.on.environment),
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
    let missing_one = make(EnvironmentChoice::existing(orch_types::new_id()))
        .await
        .problem();
    assert_eq!(missing_one.code, ErrorCode::EnvironmentUnavailable);
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
    assert_eq!(none.code, ErrorCode::EnvironmentNoneUsable, "{none:?}");
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
            environment: Some(EnvironmentChoice::existing(s.on.environment)),
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
        .full_archive()
        .ok();
    let archived = s
        .rig
        .world
        .app
        .call(ThreadCreate, &[&s.tree.topic.base.id])
        .token(&s.ada.token)
        .body(&CreateThread {
            binding: Some(binding(&s)),
            environment: Some(EnvironmentChoice::existing(s.on.environment)),
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
#[tokio::test]
async fn a_new_thread_uses_its_environment_pool_once_even_when_the_request_is_replayed() {
    use orch_api_contract::pools::PoolCreate;
    use orch_provider_contract::pools::{Kind, Leaf, Target as ResourceTarget};

    let scene = Scene::new().await;
    let leaf = Leaf::Environment {
        id: scene.on.environment,
    };
    let pool = scene.rig.world.app.call(PoolCreate, &[]).token(&scene.ada.token)
        .raw_body(&json!({
            "name": "Thread environments", "description": "", "home_id": scene.ada.person.personal_org, "access": "only_me",
            "definition": {
                "kind": "environment", "enabled": true, "policy": {"type": "round_robin"},
                "lend_members": false, "config": {}, "members": [{
                    "id": orch_types::new_id(), "enabled": true, "rule": {"type": "single", "resource": &leaf}
                }]
            }
        })).send().await.ok();
    let target = ResourceTarget::Pool {
        id: pool.id,
        kind: Kind::Environment,
    };
    let mut body = serde_json::to_value(CreateThread {
        binding: Some(binding(&scene)),
        model: Some(MODEL.to_owned()),
        ..CreateThread::default()
    })
    .unwrap();
    body["environment"] = json!({"use": &target});
    use orch_api_contract::binding::{NodeBindingOptionsGet, NodeBindingOptionsGetQuery};
    let options = scene
        .rig
        .world
        .app
        .call(NodeBindingOptionsGet, &[&scene.tree.topic.base.id])
        .token(&scene.ada.token)
        .query(&NodeBindingOptionsGetQuery {
            environment_pool_id: Some(pool.id),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert!(
        options
            .environments
            .iter()
            .any(|row| row.environment.id == scene.on.environment)
    );
    let mut invalid = body.clone();
    invalid["directory"] = json!("../../outside");
    let refused = scene
        .rig
        .world
        .app
        .call(ThreadCreate, &[&scene.tree.topic.base.id])
        .token(&scene.ada.token)
        .raw_body(&invalid)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
    let draws: i64 = sqlx::query_scalar("SELECT count(*) FROM pool_choices WHERE target=$1")
        .bind(json!(&target))
        .fetch_one(scene.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(
        draws, 0,
        "setup and refused creation must leave the pool untouched"
    );
    let key = orch_types::new_id().to_string();
    let mut previous = None;
    for _ in 0..2 {
        let made = scene
            .rig
            .world
            .app
            .call(ThreadCreate, &[&scene.tree.topic.base.id])
            .token(&scene.ada.token)
            .header("Idempotency-Key", &key)
            .raw_body(&body)
            .send()
            .await
            .ok()
            .into_full()
            .unwrap();
        assert_eq!(
            made.environment.as_ref().map(|env| env.id),
            Some(scene.on.environment)
        );
        if let Some(id) = previous {
            assert_eq!(made.base.id, id);
        }
        if previous.is_none() {
            sqlx::query("UPDATE pools SET definition=jsonb_set(definition,'{enabled}','false'),revision=revision+1 WHERE id=$1")
                .bind(pool.id).execute(scene.rig.db.pool()).await.unwrap();
        }
        previous = Some(made.base.id);
    }
    let refused = scene
        .rig
        .world
        .app
        .call(ThreadCreate, &[&scene.tree.topic.base.id])
        .token(&scene.ada.token)
        .header("Idempotency-Key", &orch_types::new_id().to_string())
        .raw_body(&body)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::EnvironmentNoneUsable);
    let leaves: Vec<serde_json::Value> =
        sqlx::query_scalar("SELECT leaf FROM pool_choices WHERE target=$1 AND state='selected'")
            .bind(json!(target))
            .fetch_all(&scene.rig.world.db.pool)
            .await
            .unwrap();
    assert_eq!(leaves, vec![json!(leaf)]);
}

#[tokio::test]
async fn a_thread_keeps_the_model_pool_member_and_fixed_agent_on_creation_replay() {
    use orch_api_contract::pools::PoolCreate;

    let scene = Scene::new().await;
    let leaf = json!({"kind":"model","backend":{"route":"generation.model","offering":{
        "provider":scene.on.provider,"offering":MODEL},"config":{}}});
    let pool = scene.rig.world.app.call(PoolCreate, &[]).token(&scene.ada.token)
        .raw_body(&json!({
            "name":"Thread models","description":"","home_id":scene.ada.person.personal_org,"access":"only_me",
            "definition":{"kind":"model","enabled":true,"policy":{"type":"round_robin"},"lend_members":false,
                "config":{},"members":[{"id":orch_types::new_id(),"enabled":true,"rule":{"type":"single","resource": &leaf}}]}
        })).send().await.ok();
    let target = json!({"type":"pool","kind":"model","id":pool.id});
    let topic = scene.tree.topic.base.id;
    let key = orch_types::new_id().to_string();
    let create = json!({
        "binding":{"agent_id":scene.on.agent,"target": &target},
        "environment": EnvironmentChoice::existing(scene.on.environment),
        "effort":"high"
    });
    let mut accepted = None;
    for _ in 0..2 {
        let made = scene
            .rig
            .world
            .app
            .call(ThreadCreate, &[&topic])
            .token(&scene.ada.token)
            .header("Idempotency-Key", &key)
            .raw_body(&create)
            .send()
            .await
            .ok()
            .into_full()
            .unwrap();
        let binding = made.binding.as_ref().unwrap();
        assert_eq!(binding.agent.id, scene.on.agent);
        assert_eq!(binding.provider.id, scene.on.provider);
        assert_eq!(made.model.as_deref(), Some(MODEL));
        assert_eq!(made.effort.as_deref(), Some("high"));
        if let Some(original) = accepted {
            assert_eq!(made.base.id, original);
        } else {
            accepted = Some(made.base.id);
            let selected: orch_types::Id = sqlx::query_scalar(
                "SELECT b.choice_id FROM segment_pool_bindings b JOIN segments s ON s.id=b.segment_id WHERE s.thread_id=$1",
            )
            .bind(made.base.id)
            .fetch_one(scene.rig.db.pool())
            .await
            .unwrap();
            for (index, words) in ["First pool message", "Second pool message"]
                .iter()
                .enumerate()
            {
                scene
                    .say(
                        &scene.ada,
                        made.base.id,
                        words,
                        orch_api_contract::threads::Delivery::Queue,
                        words,
                    )
                    .await
                    .ok();
                let started = scene.rig.turn_start(made.base.id, index).await;
                scene
                    .rig
                    .say(made.base.id, rig::turn_began(started.turn_id))
                    .await;
                scene
                    .rig
                    .say(
                        made.base.id,
                        rig::turn_ended(
                            started.turn_id,
                            orch_link_contract::events::TurnOutcome::Completed,
                        ),
                    )
                    .await;
                scene.rig.wait_turn(started.turn_id, "completed").await;
                let admitted: Vec<orch_types::Id> = sqlx::query_scalar(
                    "SELECT DISTINCT choice_id FROM pool_attempt_scopes WHERE attempt_id=$1",
                )
                .bind(started.turn_id)
                .fetch_all(scene.rig.db.pool())
                .await
                .unwrap();
                assert_eq!(admitted, [selected]);
            }

            // Editing today's policy cannot change the rule reported for either completed turn.
            sqlx::query("UPDATE pools SET definition=jsonb_set(definition,'{policy}', $2),revision=revision+1 WHERE id=$1")
                .bind(pool.id).bind(json!({"type":"ordered"})).execute(scene.rig.db.pool()).await.unwrap();
            let receipt: orch_types::Id =
                sqlx::query_scalar("SELECT read_id FROM pool_choices WHERE id=$1")
                    .bind(selected)
                    .fetch_one(scene.rig.db.pool())
                    .await
                    .unwrap();
            places::give_role(
                &scene.rig.world,
                made.base.id,
                &scene.cai,
                "viewer",
                &scene.ada,
            )
            .await;
            for reader in [&scene.ada, &scene.cai] {
                let page = scene
                    .rig
                    .world
                    .app
                    .call(orch_api_contract::threads::ThreadTurnList, &[&made.base.id])
                    .token(&reader.token)
                    .query(&orch_api_contract::threads::ThreadHistoryListQuery::default())
                    .send()
                    .await
                    .ok();
                assert_eq!(page.turns.len(), 2);
                for turn in page.turns {
                    let wire = serde_json::to_value(turn).unwrap();
                    if reader.id() == scene.ada.id() {
                        assert_eq!(
                            wire["pool_choice"],
                            json!({
                                "pool_id":pool.id, "pool_name":"Thread models", "choice_id":receipt,
                                "member":MODEL, "policy":{"type":"round_robin"}
                            })
                        );
                    } else {
                        assert!(
                            wire.get("pool_choice").is_none(),
                            "a thread reader cannot see the maker's private pool"
                        );
                    }
                }
            }

            sqlx::query("UPDATE pools SET definition=jsonb_set(definition,'{enabled}','false'),revision=revision+1 WHERE id=$1")
                .bind(pool.id).execute(scene.rig.db.pool()).await.unwrap();
        }
    }
    let draws: Vec<(orch_types::Id, serde_json::Value)> = sqlx::query_as(
        "SELECT actor_id,leaf FROM pool_choices WHERE target=$1 AND state='selected'",
    )
    .bind(&target)
    .fetch_all(scene.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(draws, vec![(scene.ada.id(), leaf)]);
}

#[tokio::test]
async fn a_topic_can_supply_the_model_or_provider_pool_for_a_new_thread() {
    use orch_api_contract::{pools::PoolCreate, settings::NodeSettingSet};
    for (kind, key) in [
        ("model", "agent.model"),
        ("provider", "agent.default_provider_id"),
    ] {
        let scene = Scene::new().await;
        let topic = scene.tree.topic.base.id;
        let leaf = if kind == "model" {
            json!({"kind":"model","backend":{"route":"generation.model","offering":{
                "provider":scene.on.provider,"offering":MODEL},"config":{}}})
        } else {
            json!({"kind":"provider","id":scene.on.provider})
        };
        let pool = scene.rig.world.app.call(PoolCreate,&[]).token(&scene.ada.token).raw_body(&json!({
            "name":"Topic default","description":"","home_id":scene.ada.person.personal_org,"access":"only_me",
            "definition":{"kind":kind,"enabled":true,"policy":{"type":"ordered"},"lend_members":false,
            "config":{},"members":[{"id":orch_types::new_id(),"enabled":true,"rule":{"type":"single","resource":leaf}}]}
        })).send().await.ok();
        let target = json!({"type":"pool","kind":kind,"id":pool.id});
        scene
            .rig
            .world
            .set_at(
                Target::Node(topic),
                "agent.default_agent_id",
                json!(scene.on.agent),
            )
            .await;
        scene
            .rig
            .world
            .app
            .call(NodeSettingSet, &[&topic, &key])
            .token(&scene.ada.token)
            .raw_body(&json!({"value":target,"locked":true}))
            .send()
            .await
            .ok();
        let request = CreateThread {
            environment: Some(EnvironmentChoice::existing(scene.on.environment)),
            model: (kind == "provider").then(|| MODEL.to_owned()),
            ..Default::default()
        };
        let idempotency = orch_types::new_id().to_string();
        let mut accepted = None;
        for _ in 0..2 {
            let made = scene
                .rig
                .world
                .app
                .call(ThreadCreate, &[&topic])
                .token(&scene.ada.token)
                .header("Idempotency-Key", &idempotency)
                .body(&request)
                .send()
                .await
                .ok()
                .into_full()
                .unwrap();
            let binding = made.binding.unwrap();
            assert_eq!(
                (binding.provider.id, binding.agent.id),
                (scene.on.provider, scene.on.agent)
            );
            assert_eq!(made.model.as_deref(), Some(MODEL));
            assert_eq!(*accepted.get_or_insert(made.base.id), made.base.id);
        }
        let used: Vec<orch_types::Id> = sqlx::query_scalar(
            "SELECT b.choice_id FROM segment_pool_bindings b JOIN segments s ON s.id=b.segment_id JOIN pool_choices c ON c.id=b.choice_id WHERE s.thread_id=$1 AND c.target=$2 AND c.actor_id=$3 AND c.state='selected'",
        ).bind(accepted.unwrap()).bind(&target).bind(scene.ada.id()).fetch_all(scene.rig.db.pool()).await.unwrap();
        assert_eq!(used.len(), 1);
    }
}
