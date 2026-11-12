//! Baseline-compatible controls for actual-target creation checkpoints.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]
mod creator_support;
mod places;
mod rig;
mod support;

use creator_support::{body, count, key, managed, rule, thread, topic};
use orch_api_contract::binding::{EnvironmentChoice, NewEnvironment};
use orch_api_contract::nodes::{AgentAccessChoice, AgentAccessWhere};
use orch_api_contract::share::{ShareDocumentsLevel, ShareLevel, ShareParts};
use orch_types::{Id, Json};
use rig::Scene;
use serde_json::json;

fn this_thread() -> AgentAccessChoice {
    AgentAccessChoice {
        where_: AgentAccessWhere::Thread,
        parts: ShareParts {
            documents: Some(ShareDocumentsLevel::Read),
            ..Default::default()
        },
    }
}

async fn gifts(s: &Scene, id: Id) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM permission_entries WHERE holder_kind='agents' AND holder_id=$1 AND removed_at IS NULL")
        .bind(id).fetch_one(s.rig.db.pool()).await.unwrap()
}
async fn counts(s: &Scene) -> (i64, i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM nodes WHERE kind='topic'),(SELECT count(*) FROM threads),(SELECT count(*) FROM environments),(SELECT count(*) FROM jobs)")
        .fetch_one(s.rig.db.pool()).await.unwrap()
}

#[tokio::test]
async fn explicit_this_thread_delegation_requires_manage_of_the_actual_created_thread() {
    let s = Scene::new().await;
    let parent = s.tree.topic.base.id;
    let write = rule(
        "ThreadCreate",
        "topic",
        parent,
        "self",
        &["thread"],
        &["write"],
    );
    let mut requested = body(&s);
    requested.agent_access = Some(this_thread());
    let before = counts(&s).await;
    let token = key(&s, s.ada.id(), vec![write.clone()]).await;
    let (status, reply) = thread(&s, &token, parent, &requested).await;
    assert_eq!(status, 403, "{reply}");
    assert_eq!(
        counts(&s).await,
        before,
        "the actual Thread prefix rolls back"
    );

    let token = key(
        &s,
        s.ada.id(),
        vec![
            write,
            rule(
                "ThreadCreate",
                "topic",
                parent,
                "subtree",
                &["thread"],
                &["manage"],
            ),
        ],
    )
    .await;
    let (status, reply) = thread(&s, &token, parent, &requested).await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["projection"], "receipt");
    let id: Id = serde_json::from_value(reply["thread_id"].clone()).unwrap();
    assert!(gifts(&s, id).await > 0);
    let inputs: i64 = sqlx::query_scalar("SELECT count(*) FROM thread_inputs WHERE thread_id=$1")
        .bind(id)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(inputs, 0, "the initial live gift adds no input");
    assert!(s.rig.turn_starts(id).is_empty());
}

#[tokio::test]
async fn omitted_this_thread_preset_keeps_the_existing_nothing_fallback() {
    let s = Scene::new().await;
    let parent = s.tree.topic.base.id;
    s.rig
        .world
        .set_at(
            orch_settings::Target::Node(parent),
            orch_settings_contract::registry::keys::THREADS_AGENT_ACCESS,
            json!(this_thread()),
        )
        .await;
    let token = key(
        &s,
        s.ada.id(),
        vec![rule(
            "ThreadCreate",
            "topic",
            parent,
            "self",
            &["thread"],
            &["write"],
        )],
    )
    .await;
    let mut requested = body(&s);
    requested.agent_access = None;
    let (status, reply) = thread(&s, &token, parent, &requested).await;
    assert_eq!(status, 200, "{reply}");
    let id: Id = serde_json::from_value(reply["thread_id"].clone()).unwrap();
    assert_eq!(gifts(&s, id).await, 0);
}

async fn new_environment_body(s: &Scene) -> orch_api_contract::nodes::CreateThread {
    let (host, template) = managed(s).await;
    let mut requested = body(s);
    requested.environment = Some(EnvironmentChoice::New(Box::new(NewEnvironment {
        name: Some("Scoped new environment".into()),
        home_node_id: Some(s.tree.topic.base.id),
        host_id: Some(Some(host)),
        template_id: Some(template),
        ..Default::default()
    })));
    requested
}

async fn retained_creator(s: &Scene, reply: &Json, operation: &str) {
    let id: Id = serde_json::from_value(reply["thread_id"].clone()).unwrap();
    let mut conn = s.rig.world.conn().await;
    let stored = orch_db::queries::threads::thread(&mut conn, id)
        .await
        .unwrap()
        .unwrap();
    let environment = stored.environment_id.unwrap();
    let plan: Json = sqlx::query_scalar(
        "SELECT plan FROM jobs WHERE kind='environment_provision' AND environment_id=$1",
    )
    .bind(environment)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    // The private retained origin lives on the exact provision job, rather than a
    // representative user/session or a new broad Environment-maintenance grant.
    assert_eq!(
        plan["creator_provision"]["origin"]["operation"], operation,
        "{plan}"
    );
    assert_eq!(plan["creator_provision"]["thread"], json!(id), "{plan}");
    assert_eq!(
        plan["creator_provision"]["environment"],
        json!(environment),
        "{plan}"
    );
}

#[tokio::test]
async fn new_environment_creation_requires_its_actual_home_write_before_any_job_escapes() {
    let s = Scene::new().await;
    let requested = new_environment_body(&s).await;
    let parent = s.tree.topic.base.id;
    let write = rule(
        "ThreadCreate",
        "topic",
        parent,
        "self",
        &["thread"],
        &["write"],
    );
    let before = counts(&s).await;
    let token = key(&s, s.ada.id(), vec![write.clone()]).await;
    let (status, reply) = thread(&s, &token, parent, &requested).await;
    assert_eq!(status, 403, "{reply}");
    assert_eq!(counts(&s).await, before);
    let token = key(
        &s,
        s.ada.id(),
        vec![
            write,
            rule(
                "ThreadCreate",
                "topic",
                parent,
                "self",
                &["environment"],
                &["write"],
            ),
        ],
    )
    .await;
    let (status, reply) = thread(&s, &token, parent, &requested).await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["projection"], "receipt");
    assert_eq!(count(&s, "thread").await, before.1 + 1);
    retained_creator(&s, &reply, "ThreadCreate").await;
}

#[tokio::test]
async fn compound_new_environment_and_this_thread_delegation_complete_as_one_actual_prefix() {
    let s = Scene::new().await;
    let mut requested = new_environment_body(&s).await;
    requested.agent_access = Some(this_thread());
    let project = s.tree.project.base.id;
    let base = rule(
        "TopicCreate.with_thread",
        "project",
        project,
        "subtree",
        &["topic", "thread"],
        &["write"],
    );
    let environment = rule(
        "TopicCreate.with_thread",
        "topic",
        s.tree.topic.base.id,
        "self",
        &["environment"],
        &["write"],
    );
    let before = counts(&s).await;
    let token = key(&s, s.ada.id(), vec![base.clone(), environment.clone()]).await;
    let (status, reply) = topic(&s, &token, &requested).await;
    assert_eq!(
        status, 403,
        "the actual new Thread still lacks Manage: {reply}"
    );
    assert_eq!(counts(&s).await, before);
    let token = key(
        &s,
        s.ada.id(),
        vec![
            base,
            environment,
            rule(
                "TopicCreate.with_thread",
                "project",
                project,
                "subtree",
                &["thread"],
                &["manage"],
            ),
        ],
    )
    .await;
    let (status, reply) = topic(&s, &token, &requested).await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["projection"], "receipt");
    assert_eq!(
        counts(&s).await,
        (before.0 + 1, before.1 + 1, before.2 + 1, before.3 + 1)
    );
    let id: Id = serde_json::from_value(reply["thread_id"].clone()).unwrap();
    let created_topic: Id = serde_json::from_value(reply["topic_id"].clone()).unwrap();
    assert_ne!(created_topic, s.tree.topic.base.id);
    assert!(gifts(&s, id).await > 0);
    retained_creator(&s, &reply, "TopicCreate.with_thread").await;
}

#[tokio::test]
async fn invalid_this_thread_timeline_keeps_the_ordinary_validation_error_for_browser_and_key() {
    let s = Scene::new().await;
    let parent = s.tree.topic.base.id;
    let token = key(
        &s,
        s.ada.id(),
        vec![
            rule(
                "ThreadCreate",
                "topic",
                parent,
                "self",
                &["thread"],
                &["write"],
            ),
            rule(
                "ThreadCreate",
                "topic",
                parent,
                "subtree",
                &["thread"],
                &["manage"],
            ),
        ],
    )
    .await;
    let mut requested = body(&s);
    requested.agent_access = Some(AgentAccessChoice {
        where_: AgentAccessWhere::Thread,
        parts: ShareParts {
            timeline: Some(ShareLevel::Read),
            ..Default::default()
        },
    });
    let before = counts(&s).await;
    let (browser_status, browser_reply) = thread(&s, &s.ada.token, parent, &requested).await;
    assert_eq!(browser_status, 400, "ordinary validation: {browser_reply}");
    assert_eq!(counts(&s).await, before);
    let (key_status, key_reply) = thread(&s, &token, parent, &requested).await;
    assert_eq!(
        key_status, 400,
        "same input under a complete key scope: {key_reply}"
    );
    assert_eq!(key_reply["code"], browser_reply["code"]);
    assert_eq!(counts(&s).await, before);
}
