//! A requested human-audience gift is checked before birth and committed with the actual thread.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod binding_support;
mod places;
mod rig;
mod support;
mod thread_history_scope;

use orch_api_contract::{
    access::{PageChange, PageChangeWhole, PermissionPageGet, PermissionPageUpdate, ThreadAccess},
    binding::{EnvironmentChoice, NodeBindingOptionsGet, NodeBindingOptionsGetQuery},
    common::ErrorCode,
    environment::EnvironmentGet,
    nodes::{
        AgentAccessChoice, AgentAccessWhere, CreateThread, CreateThreadBinding, ThreadCreate,
        TopicCreate, TopicCreateBody, TopicCreateReply,
    },
};
use orch_types::{Id, Json};
use rig::{MODEL, Scene};
use serde_json::json;
use thread_history_scope::Proof;

fn body(s: &Scene, environment: Id) -> CreateThread {
    CreateThread {
        let_into_environment: Some(true),
        name: Some("Readers welcomed at birth".into()),
        binding: Some(CreateThreadBinding { agent_id: s.on.agent, provider_id: s.on.provider, arrangement: None }),
        model: Some(MODEL.into()),
        environment: Some(EnvironmentChoice::Use(environment)),
        prepare_source: Some(false),
        agent_access: Some(AgentAccessChoice { where_: AgentAccessWhere::Nothing, parts: Default::default() }),
        access: Some(ThreadAccess::Change(serde_json::from_value(json!({
            "whole":"private", "name":[{"choice":"full_view","holders":[{"kind":"user","id":s.cai.id()}]}]
        })).unwrap())),
        ..Default::default()
    }
}

async fn counts(s: &Scene) -> (i64, i64) {
    let threads = sqlx::query_scalar("SELECT count(*) FROM threads")
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    let entries = sqlx::query_scalar("SELECT count(*) FROM permission_entries")
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    (threads, entries)
}

#[tokio::test(flavor = "multi_thread")]
async fn the_prospective_reader_gift_is_refused_before_birth_and_succeeds_atomically_when_giveable()
{
    let s = Scene::new().await;
    let environment = binding_support::a_directory_on_a_machine(&s, "Private directory").await;
    binding_support::offering_plain(&s, environment).await;
    binding_support::names(
        &s,
        "environment",
        environment,
        "environment.manage",
        &s.eve,
        json!({}),
    )
    .await;
    let body = body(&s, environment);
    let home: Id = sqlx::query_scalar("SELECT home_node_id FROM environments WHERE id=$1")
        .bind(environment)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    for allowed in [false, true] {
        orch_db::queries::settings::put(
            &mut *s.rig.world.conn().await,
            Some(home),
            "sharing.allow_holders",
            &orch_db::queries::settings::Stored {
                value: &json!(allowed),
                locked: false,
                by: Some(s.ada.id()),
                at: s.rig.world.now(),
            },
            None,
        )
        .await
        .unwrap();
        let before = counts(&s).await;
        let options = s
            .rig
            .world
            .app
            .call(NodeBindingOptionsGet, &[&s.tree.topic.base.id])
            .token(&s.eve.token)
            .query(&NodeBindingOptionsGetQuery {
                environment_id: Some(environment),
                thread_access: body.access.clone(),
                ..Default::default()
            })
            .send()
            .await
            .ok();
        let offer = options
            .environment_readers
            .expect("selected existing environment offer");
        assert_eq!(offer.allowed, allowed);
        assert_eq!(offer.reason.is_none(), allowed);
        assert_eq!(
            counts(&s).await,
            before,
            "a page trial leaves no thread or entries"
        );
        let reply = s
            .rig
            .world
            .app
            .call(ThreadCreate, &[&s.tree.topic.base.id])
            .token(&s.eve.token)
            .body(&body)
            .send()
            .await;
        if !allowed {
            assert_eq!(reply.problem().code, ErrorCode::AuthzDenied);
            assert_eq!(
                counts(&s).await,
                before,
                "a refused gift leaves no birth or partial grant"
            );
        } else {
            let thread = reply.ok().into_full().unwrap();
            assert_eq!(
                thread.environment.as_ref().map(|on| on.id),
                Some(environment)
            );
            assert_eq!(counts(&s).await.0, before.0 + 1);
            let view = s
                .rig
                .world
                .app
                .call(EnvironmentGet, &[&environment])
                .token(&s.cai.token)
                .send()
                .await
                .ok();
            assert_eq!(view.workspace_root, "/work/ada");
            assert!(
                view.access
                    .permissions
                    .iter()
                    .any(|permission| permission == "files.view")
            );
            assert!(
                !view
                    .access
                    .permissions
                    .iter()
                    .any(|permission| permission == "environment.manage")
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_scoped_creator_needs_the_same_operations_environment_manage_for_the_requested_gift()
-> Proof {
    let s = Scene::new().await;
    let environment = binding_support::a_directory_on_a_machine(&s, "Scoped reader gift").await;
    binding_support::offering_plain(&s, environment).await;
    let body = body(&s, environment);
    let write = json!({"target":{"kind":"topic","id":s.tree.topic.base.id,"reach":"self"},
        "resources":["thread"],"allow":[{"operation":"ThreadCreate","actions":["write"]}]});
    for (operation, expected) in [("EnvironmentGet", false), ("ThreadCreate", true)] {
        let selected = thread_history_scope::key(
            &s.rig.world,
            s.ada.id(),
            vec![
                write.clone(),
                json!({"target":{"kind":"environment","id":environment,"reach":"self"},
                "allow":[{"operation":operation,"actions":["manage"]}]}),
            ],
        )
        .await?;
        let before = counts(&s).await;
        let reply = s
            .rig
            .world
            .app
            .call(ThreadCreate, &[&s.tree.topic.base.id])
            .token(&selected.secret)
            .body(&body)
            .send_bytes()
            .await;
        if expected {
            assert_eq!(
                reply.status.as_u16(),
                200,
                "{}",
                String::from_utf8_lossy(&reply.raw)
            );
            let receipt: Json = serde_json::from_slice(&reply.raw)?;
            assert_eq!(receipt["projection"], "receipt");
            assert_eq!(receipt["topic_id"], json!(s.tree.topic.base.id));
            assert_eq!(
                receipt.as_object().ok_or("receipt object missing")?.len(),
                4
            );
            for content in [
                "name",
                "environment",
                "access",
                "binding",
                "environment_readers",
            ] {
                assert!(
                    receipt.get(content).is_none(),
                    "receipt disclosed {content}"
                );
            }
            assert_eq!(counts(&s).await.0, before.0 + 1);
        } else {
            assert_eq!(reply.status.as_u16(), 403);
            assert_eq!(counts(&s).await, before);
        }
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn the_compound_topic_route_keeps_the_reader_choice_and_rolls_back_its_whole_creation_on_refusal()
 {
    let s = Scene::new().await;
    let environment = binding_support::a_directory_on_a_machine(&s, "Compound audience").await;
    binding_support::offering_plain(&s, environment).await;
    binding_support::names(
        &s,
        "environment",
        environment,
        "environment.manage",
        &s.eve,
        json!({}),
    )
    .await;
    let body = TopicCreateBody {
        name: Some("Atomic topic and reader gift".into()),
        first_thread: Some(body(&s, environment)),
        ..Default::default()
    };
    for allowed in [false, true] {
        orch_db::queries::settings::put(
            &mut *s.rig.world.conn().await,
            Some(s.tree.topic.base.id),
            "sharing.allow_holders",
            &orch_db::queries::settings::Stored {
                value: &json!(allowed),
                locked: false,
                by: Some(s.ada.id()),
                at: s.rig.world.now(),
            },
            None,
        )
        .await
        .unwrap();
        let before = counts(&s).await;
        let topics: i64 = sqlx::query_scalar("SELECT count(*) FROM nodes WHERE kind='topic'")
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
        let reply = s
            .rig
            .world
            .app
            .call(TopicCreate, &[&s.tree.project.base.id])
            .token(&s.eve.token)
            .body(&body)
            .send()
            .await;
        if !allowed {
            assert_eq!(reply.problem().code, ErrorCode::AuthzDenied);
            assert_eq!(counts(&s).await, before);
            let after: i64 = sqlx::query_scalar("SELECT count(*) FROM nodes WHERE kind='topic'")
                .fetch_one(s.rig.db.pool())
                .await
                .unwrap();
            assert_eq!(
                after, topics,
                "the new topic is part of the same rejected birth"
            );
        } else {
            let TopicCreateReply::WithThread(made) = reply.ok() else {
                panic!("the ordinary compound route must return its full result");
            };
            assert_eq!(made.thread.topic_id, made.topic.base.id);
            assert_eq!(
                made.thread.environment.as_ref().map(|on| on.id),
                Some(environment)
            );
            assert_eq!(counts(&s).await.0, before.0 + 1);
            let view = s
                .rig
                .world
                .app
                .call(EnvironmentGet, &[&environment])
                .token(&s.cai.token)
                .send()
                .await
                .ok();
            assert_eq!(view.workspace_root, "/work/ada");
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn omitting_the_reader_gift_keeps_creation_and_the_environment_page_independent() {
    let s = Scene::new().await;
    let environment =
        binding_support::a_directory_on_a_machine(&s, "No implicit reader gift").await;
    binding_support::offering_plain(&s, environment).await;
    // The home already has a Viewer. Cut the default View doors before testing a missing gift.
    let page = s
        .rig
        .world
        .app
        .call(PermissionPageGet, &[&"environments", &environment])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    s.rig
        .world
        .app
        .call(PermissionPageUpdate, &[&"environments", &environment])
        .token(&s.ada.token)
        .body(&PageChange {
            whole: Some(PageChangeWhole::Private),
            expected_revision: Some(page.revision),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    binding_support::names(
        &s,
        "environment",
        environment,
        "environment.manage",
        &s.eve,
        json!({}),
    )
    .await;
    orch_db::queries::settings::put(
        &mut *s.rig.world.conn().await,
        Some(s.tree.topic.base.id),
        "sharing.allow_holders",
        &orch_db::queries::settings::Stored {
            value: &json!(false),
            locked: false,
            by: Some(s.ada.id()),
            at: s.rig.world.now(),
        },
        None,
    )
    .await
    .unwrap();
    let before: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM permission_entries WHERE object_kind='environment' AND object_id=$1",
    )
    .bind(environment)
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    let mut body = body(&s, environment);
    body.let_into_environment = None;
    let owner = s
        .rig
        .world
        .app
        .call(EnvironmentGet, &[&environment])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(owner.workspace_root, "/work/ada");
    assert_eq!(
        s.rig
            .world
            .app
            .call(EnvironmentGet, &[&environment])
            .token(&s.cai.token)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied,
        "without a bound thread or a gift the reader has no Environment door"
    );
    let thread = s
        .rig
        .world
        .app
        .call(ThreadCreate, &[&s.tree.topic.base.id])
        .token(&s.eve.token)
        .body(&body)
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
    assert_eq!(
        thread.environment.as_ref().map(|on| on.id),
        Some(environment)
    );
    let after: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM permission_entries WHERE object_kind='environment' AND object_id=$1",
    )
    .bind(environment)
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(after, before, "no environment gift was requested");
    let reader = s
        .rig
        .world
        .app
        .call(EnvironmentGet, &[&environment])
        .token(&s.cai.token)
        .send()
        .await
        .ok();
    assert!(reader.workspace_root.is_empty());
    assert!(reader.sandbox_readiness.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reader_gift_expiring_on_the_actual_environment_lock_leaves_no_thread_or_grant() -> Proof
{
    let s = Scene::new().await;
    let environment = binding_support::a_directory_on_a_machine(&s, "A real native wait").await;
    binding_support::offering_plain(&s, environment).await;
    let body = body(&s, environment);
    let topic = s.tree.topic.base.id;
    let selected = thread_history_scope::key(
        &s.rig.world,
        s.ada.id(),
        vec![
            json!({"target":{"kind":"topic","id":topic,"reach":"self"},"resources":["thread"],
            "allow":[{"operation":"ThreadCreate","actions":["write"]}]}),
            json!({"target":{"kind":"environment","id":environment,"reach":"self"},
            "allow":[{"operation":"ThreadCreate","actions":["manage"]}]}),
        ],
    )
    .await?;
    let before = counts(&s).await;
    let mut lock = s.rig.db.begin().await?;
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *lock)
        .await?;
    sqlx::query("SELECT id FROM environments WHERE id=$1 FOR UPDATE")
        .bind(environment)
        .fetch_one(&mut *lock)
        .await?;
    let app = s.rig.world.app.clone();
    let secret = selected.secret;
    let pending = tokio::spawn(async move {
        let request = app.call(ThreadCreate, &[&topic]).token(&secret).body(&body);
        request.send_bytes().await
    });
    tokio::time::timeout(rig::PATIENCE, async {
        loop {
            assert!(!pending.is_finished(), "the creator must reach its real native source lock");
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query ILIKE '%FROM environments%FOR UPDATE%')")
                .bind(pid).fetch_one(s.rig.db.pool()).await.unwrap();
            if waiting { break; }
            tokio::task::yield_now().await;
        }
    }).await?;
    s.rig.world.advance(chrono::Duration::hours(2));
    lock.commit().await?;
    let refused = pending.await?;
    assert!(
        matches!(refused.status.as_u16(), 401 | 403),
        "{}",
        String::from_utf8_lossy(&refused.raw)
    );
    assert_eq!(
        counts(&s).await,
        before,
        "the expired source lends neither birth nor gift"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stricter_environment_external_lifetime_refuses_the_gift_without_hiding_binding_options()
{
    let s = Scene::new().await;
    let environment =
        binding_support::a_directory_on_a_machine(&s, "Bounded environment audience").await;
    binding_support::offering_plain(&s, environment).await;
    let topic = places::make_topic(
        &s.rig.world,
        &s.ada,
        s.tree.project.base.id,
        "Another topic",
    )
    .await
    .base
    .id;
    for (place, key, value) in [
        (s.tree.org.base.id, "sharing.allow_external", json!(true)),
        (s.tree.topic.base.id, "sharing.external_max_days", json!(1)),
    ] {
        orch_db::queries::settings::put(
            &mut *s.rig.world.conn().await,
            Some(place),
            key,
            &orch_db::queries::settings::Stored {
                value: &value,
                locked: false,
                by: Some(s.ada.id()),
                at: s.rig.world.now(),
            },
            None,
        )
        .await
        .unwrap();
    }
    let mut body = body(&s, environment);
    body.access = Some(ThreadAccess::Change(serde_json::from_value(json!({
        "whole":"private", "name":[{"choice":"full_view","holders":[{"kind":"user","id":s.bea.id()}]}]
    })).unwrap()));
    let before = counts(&s).await;
    let options = s
        .rig
        .world
        .app
        .call(NodeBindingOptionsGet, &[&topic])
        .token(&s.ada.token)
        .query(&NodeBindingOptionsGetQuery {
            environment_id: Some(environment),
            thread_access: body.access.clone(),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    let offer = options
        .environment_readers
        .expect("a precise optional offer");
    assert!(!offer.allowed);
    assert!(offer.reason.is_some());
    assert!(
        !options.providers.is_empty(),
        "the separate binding choices remain available"
    );
    assert_eq!(counts(&s).await, before);
    let refused = s
        .rig
        .world
        .app
        .call(ThreadCreate, &[&topic])
        .token(&s.ada.token)
        .body(&body)
        .send()
        .await;
    assert!(!refused.status.is_success());
    assert_eq!(counts(&s).await, before);
    body.let_into_environment = None;
    s.rig
        .world
        .app
        .call(ThreadCreate, &[&topic])
        .token(&s.ada.token)
        .body(&body)
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
    assert_eq!(counts(&s).await.0, before.0 + 1);
}
