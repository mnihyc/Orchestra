//! Actual-key creator controls. Raw replies remain baseline-compatible before receipt DTOs exist.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]
mod places;
mod rig;
mod support;
use orch_api_contract::binding::EnvironmentChoice;
use orch_api_contract::nodes::{
    CreateThread, CreateThreadBinding, FirstInput, ThreadCreate, TopicCreate, TopicCreateBody,
};
use orch_api_contract::threads::ContentPart;
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_db::queries::identity::{NewApiToken, create_api_token_scoped};
use orch_types::{Id, Json, new_id};
use rig::{MODEL, Scene};
use serde_json::json;

fn rule(op: &str, kind: &str, id: Id, reach: &str, resources: &[&str], actions: &[&str]) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":reach},"resources":resources,"allow":[{"operation":op,"actions":actions}]})
}
async fn key(s: &Scene, actor: Id, rules: Vec<Json>) -> String {
    let scope: orch_access_contract::key_scope::KeyScope =
        serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    let scope = serde_json::to_value(scope).unwrap();
    create_api_token_scoped(
        &mut *s.rig.world.conn().await,
        &NewApiToken {
            id: new_id(),
            user_id: actor,
            name: "Creator control",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: s.rig.world.now(),
            expires_at: Some(s.rig.world.now() + chrono::Duration::hours(1)),
        },
        Some(&scope),
    )
    .await
    .unwrap();
    secret
}
fn body(s: &Scene) -> CreateThread {
    CreateThread {
        name: Some("Scoped creator".into()),
        binding: Some(CreateThreadBinding {
            agent_id: s.on.agent,
            provider_id: s.on.provider,
            arrangement: None,
        }),
        model: Some(MODEL.into()),
        environment: Some(EnvironmentChoice::Use(s.on.environment)),
        prepare_source: Some(false),
        agent_access: Some(orch_api_contract::nodes::AgentAccessChoice {
            where_: orch_api_contract::nodes::AgentAccessWhere::Nothing,
            parts: Default::default(),
        }),
        ..Default::default()
    }
}
async fn thread(s: &Scene, token: &str, parent: Id, body: &CreateThread) -> (u16, Json) {
    let request = s
        .rig
        .world
        .app
        .call(ThreadCreate, &[&parent])
        .token(token)
        .body(body);
    let response = request.send_bytes().await;
    (
        response.status.as_u16(),
        serde_json::from_slice(&response.raw).unwrap(),
    )
}
async fn topic(s: &Scene, token: &str, body: &CreateThread) -> (u16, Json) {
    let request = s
        .rig
        .world
        .app
        .call(TopicCreate, &[&s.tree.project.base.id])
        .token(token)
        .body(&TopicCreateBody {
            name: Some("Compound creator".into()),
            first_thread: Some(body.clone()),
            ..Default::default()
        });
    let response = request.send_bytes().await;
    (
        response.status.as_u16(),
        serde_json::from_slice(&response.raw).unwrap(),
    )
}
async fn count(s: &Scene, kind: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM nodes WHERE kind=$1")
        .bind(kind)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap()
}
#[tokio::test]
async fn an_existing_environment_thread_write_returns_actual_ids_and_no_content() {
    let s = Scene::new().await;
    let token = key(
        &s,
        s.ada.id(),
        vec![rule(
            "ThreadCreate",
            "topic",
            s.tree.topic.base.id,
            "self",
            &["thread"],
            &["write"],
        )],
    )
    .await;
    let (status, reply) = thread(&s, &token, s.tree.topic.base.id, &body(&s)).await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["projection"], "receipt");
    assert_eq!(reply["topic_id"], json!(s.tree.topic.base.id));
    assert_eq!(reply.as_object().unwrap().len(), 4);
    let id: Id = serde_json::from_value(reply["thread_id"].clone()).unwrap();
    let stored = orch_db::queries::threads::thread(&mut *s.rig.world.conn().await, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.topic_id, s.tree.topic.base.id);
    assert!(reply.get("name").is_none());
}
#[tokio::test]
async fn full_creation_requires_its_own_actual_thread_and_environment_read() {
    let s = Scene::new().await;
    let rules = vec![rule(
        "ThreadCreate",
        "topic",
        s.tree.topic.base.id,
        "subtree",
        &["thread"],
        &["write", "read"],
    )];
    let token = key(&s, s.ada.id(), rules.clone()).await;
    let (status, reply) = thread(&s, &token, s.tree.topic.base.id, &body(&s)).await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["projection"], "receipt");
    let mut complete = rules;
    complete.push(rule(
        "ThreadCreate",
        "environment",
        s.on.environment,
        "self",
        &["environment"],
        &["read"],
    ));
    let token = key(&s, s.ada.id(), complete).await;
    let (status, reply) = thread(&s, &token, s.tree.topic.base.id, &body(&s)).await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["name"], "Scoped creator");
    assert!(reply.get("projection").is_none());
    let parent = reply["path"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == json!(s.tree.topic.base.id))
        .unwrap();
    assert!(parent["name"].is_null());
    assert_eq!(parent["redacted"], true);
}
#[tokio::test]
async fn compound_creation_rolls_back_the_topic_before_missing_thread_authority_can_escape() {
    let s = Scene::new().await;
    let before = count(&s, "topic").await;
    let token = key(
        &s,
        s.ada.id(),
        vec![rule(
            "TopicCreate.with_thread",
            "project",
            s.tree.project.base.id,
            "self",
            &["topic"],
            &["write"],
        )],
    )
    .await;
    let (status, _) = topic(&s, &token, &body(&s)).await;
    assert_eq!(status, 403);
    assert_eq!(count(&s, "topic").await, before);
    let token = key(
        &s,
        s.ada.id(),
        vec![rule(
            "TopicCreate.with_thread",
            "project",
            s.tree.project.base.id,
            "subtree",
            &["topic", "thread"],
            &["write"],
        )],
    )
    .await;
    let (status, reply) = topic(&s, &token, &body(&s)).await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["projection"], "receipt");
    assert_eq!(count(&s, "topic").await, before + 1);
    let thread_id: Id = serde_json::from_value(reply["thread_id"].clone()).unwrap();
    let topic_id: Id = serde_json::from_value(reply["topic_id"].clone()).unwrap();
    assert_ne!(thread_id, topic_id);
    assert_eq!(
        orch_db::queries::threads::thread(&mut *s.rig.world.conn().await, thread_id)
            .await
            .unwrap()
            .unwrap()
            .topic_id,
        topic_id
    );
}
#[tokio::test]
async fn receipt_first_input_is_accepted_and_dispatch_is_projection_independent() {
    let s = Scene::new().await;
    let token = key(
        &s,
        s.ada.id(),
        vec![rule(
            "ThreadCreate",
            "topic",
            s.tree.topic.base.id,
            "self",
            &["thread"],
            &["write"],
        )],
    )
    .await;
    let mut body = body(&s);
    body.first_input = Some(FirstInput {
        content: vec![ContentPart::Text {
            text: "Hello".into(),
        }],
        client_id: "creator-first".into(),
    });
    let (status, reply) = thread(&s, &token, s.tree.topic.base.id, &body).await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["projection"], "receipt");
    let id: Id = serde_json::from_value(reply["thread_id"].clone()).unwrap();
    let input = orch_db::queries::threads::input_by_client(
        &mut *s.rig.world.conn().await,
        id,
        Some(s.ada.id()),
        "creator-first",
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(reply["input_id"], json!(input.id));
    let turn = s.rig.turn_start(id, 0).await;
    let accepted: Option<Id> = sqlx::query_scalar("SELECT input_id FROM turns WHERE id=$1")
        .bind(turn.turn_id)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(accepted, Some(input.id));
}
#[tokio::test]
async fn scope_does_not_replace_ordinary_creation_permission_or_allow_a_different_parent() {
    let s = Scene::new().await;
    let powerless = key(
        &s,
        s.cai.id(),
        vec![rule(
            "ThreadCreate",
            "topic",
            s.tree.topic.base.id,
            "self",
            &["thread"],
            &["write"],
        )],
    )
    .await;
    let before = count(&s, "thread").await;
    assert_eq!(
        thread(&s, &powerless, s.tree.topic.base.id, &body(&s))
            .await
            .0,
        403
    );
    assert_eq!(count(&s, "thread").await, before);
    let unrelated = key(
        &s,
        s.ada.id(),
        vec![rule(
            "ThreadCreate",
            "topic",
            new_id(),
            "self",
            &["thread"],
            &["write"],
        )],
    )
    .await;
    assert_eq!(
        thread(&s, &unrelated, s.tree.topic.base.id, &body(&s))
            .await
            .0,
        403
    );
    assert_eq!(count(&s, "thread").await, before);
}
#[tokio::test]
async fn explicit_agent_delegation_cannot_borrow_write_but_a_preset_can_fall_back_to_nothing() {
    use orch_api_contract::nodes::{AgentAccessChoice, AgentAccessWhere};
    use orch_api_contract::share::{ShareLevel, ShareParts};
    let s = Scene::new().await;
    let token = key(
        &s,
        s.ada.id(),
        vec![rule(
            "ThreadCreate",
            "topic",
            s.tree.topic.base.id,
            "self",
            &["thread"],
            &["write"],
        )],
    )
    .await;
    let mut body = body(&s);
    body.agent_access = Some(AgentAccessChoice {
        where_: AgentAccessWhere::Topic,
        parts: ShareParts {
            timeline: Some(ShareLevel::Read),
            ..Default::default()
        },
    });
    let before = count(&s, "thread").await;
    assert_eq!(thread(&s, &token, s.tree.topic.base.id, &body).await.0, 403);
    assert_eq!(count(&s, "thread").await, before);
    s.rig
        .world
        .set_at(
            orch_settings::Target::Node(s.tree.topic.base.id),
            orch_settings_contract::registry::keys::THREADS_AGENT_ACCESS,
            json!(body.agent_access.unwrap()),
        )
        .await;
    body.agent_access = None;
    let (status, reply) = thread(&s, &token, s.tree.topic.base.id, &body).await;
    assert_eq!(status, 200, "{reply}");
    let thread_id: Id = serde_json::from_value(reply["thread_id"].clone()).unwrap();
    let gifts:i64=sqlx::query_scalar("SELECT count(*) FROM permission_entries WHERE holder_kind='agents' AND holder_id=$1 AND removed_at IS NULL").bind(thread_id).fetch_one(s.rig.db.pool()).await.unwrap();
    assert_eq!(
        gifts, 0,
        "an omitted preset outside the credential falls back to Nothing"
    );
}

#[tokio::test]
async fn an_explicit_source_contribution_uses_its_same_operation_manage_without_waking_an_agent() {
    use orch_api_contract::nodes::{AgentAccessChoice, AgentAccessWhere};
    use orch_api_contract::share::{ShareLevel, ShareParts};
    let s = Scene::new().await;
    let token = key(
        &s,
        s.ada.id(),
        vec![
            rule(
                "ThreadCreate",
                "topic",
                s.tree.topic.base.id,
                "self",
                &["thread"],
                &["write"],
            ),
            rule(
                "ThreadCreate",
                "topic",
                s.tree.topic.base.id,
                "self",
                &["topic"],
                &["manage"],
            ),
        ],
    )
    .await;
    let mut body = body(&s);
    body.agent_access = Some(AgentAccessChoice {
        where_: AgentAccessWhere::Topic,
        parts: ShareParts {
            timeline: Some(ShareLevel::Read),
            ..Default::default()
        },
    });
    let (status, reply) = thread(&s, &token, s.tree.topic.base.id, &body).await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["projection"], "receipt");
    let thread_id: Id = serde_json::from_value(reply["thread_id"].clone()).unwrap();
    let gifts:i64=sqlx::query_scalar("SELECT count(*) FROM permission_entries WHERE holder_kind='agents' AND holder_id=$1 AND removed_at IS NULL").bind(thread_id).fetch_one(s.rig.db.pool()).await.unwrap();
    assert!(gifts > 0);
    let inputs: i64 = sqlx::query_scalar("SELECT count(*) FROM thread_inputs WHERE thread_id=$1")
        .bind(thread_id)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(inputs, 0, "a pure initial contribution adds no input");
    assert!(s.rig.turn_starts(thread_id).is_empty());
}
