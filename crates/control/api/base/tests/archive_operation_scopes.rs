//! Scoped archive/review operations share the ordinary cascade and admit each current act independently.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod places;
mod support;

use chrono::Duration;
use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::nodes::{
    NodeArchive, NodeArchivePreview, NodeUnarchive, NodeUnarchivePreview,
};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_db::queries::{hierarchy, identity, resources, threads};
use orch_types::{Id, Json, new_id};
use places::{Who, make_org, make_project, make_topic, person};
use serde_json::json;
use support::World;

struct Key {
    id: Id,
    secret: String,
}

fn rule(operation: &str, kind: &str, id: Id, actions: &[&str]) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"allow":[{"operation":operation,"actions":actions}]})
}

async fn key(world: &World, owner: Id, rules: Vec<Json>) -> Key {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    let id = new_id();
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id,
            user_id: owner,
            name: "Archive operation control",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + Duration::hours(1)),
        },
        Some(&serde_json::to_value(scope).unwrap()),
    )
    .await
    .unwrap();
    Key { id, secret }
}

struct Office {
    world: World,
    owner: Who,
    org: Id,
    project: Id,
    topic: Id,
    thread: Id,
    environment: Id,
}

impl Office {
    async fn new() -> Self {
        let world = World::new().await;
        let owner = person(&world, "owner").await;
        let org = make_org(&world, &owner, "Office", "office").await.base.id;
        let project = make_project(&world, &owner, org, "Confidential project label")
            .await
            .base
            .id;
        let topic = make_topic(&world, &owner, project, "Confidential topic label")
            .await
            .base
            .id;
        let thread = new_id();
        assert!(
            threads::insert_thread(
                &mut *world.conn().await,
                thread,
                topic,
                "Confidential thread label",
                owner.id(),
                world.now()
            )
            .await
            .unwrap()
        );
        let environment = resources::access_fixtures::insert_environment(
            &mut *world.conn().await,
            owner.id(),
            Some(project),
            "Confidential environment label",
            "managed",
        )
        .await
        .unwrap();
        threads::set_environment(
            &mut *world.conn().await,
            thread,
            environment,
            "/workspace",
            1,
        )
        .await
        .unwrap();
        Self {
            world,
            owner,
            org,
            project,
            topic,
            thread,
            environment,
        }
    }
    async fn key(&self, rules: Vec<Json>) -> Key {
        key(&self.world, self.owner.id(), rules).await
    }
    async fn ordinary_plan(&self, archiving: bool) -> Json {
        success(preview(&self.world, &self.owner.token, self.project, archiving).await)
    }
    async fn archived(&self) -> bool {
        hierarchy::with_ancestors(&mut *self.world.conn().await, &[self.project])
            .await
            .unwrap()
            .into_iter()
            .find(|row| row.id == self.project)
            .unwrap()
            .archived_at
            .is_some()
    }
    async fn archive_ordinary(&self) -> Json {
        let plan = self.ordinary_plan(true).await;
        success(
            act(
                &self.world,
                &self.owner.token,
                self.project,
                true,
                &body(&plan),
            )
            .await,
        )
    }
}

async fn preview(world: &World, token: &str, node: Id, archiving: bool) -> RawReply {
    if archiving {
        world
            .app
            .call(NodeArchivePreview, &[&node])
            .token(token)
            .send_bytes()
            .await
    } else {
        world
            .app
            .call(NodeUnarchivePreview, &[&node])
            .token(token)
            .send_bytes()
            .await
    }
}
async fn act(world: &World, token: &str, node: Id, archiving: bool, body: &Json) -> RawReply {
    if archiving {
        world
            .app
            .call(NodeArchive, &[&node])
            .token(token)
            .raw_body(body)
            .send_bytes()
            .await
    } else {
        world
            .app
            .call(NodeUnarchive, &[&node])
            .token(token)
            .raw_body(body)
            .send_bytes()
            .await
    }
}
fn body(plan: &Json) -> Json {
    json!({"preflight_id":plan["preflight_id"],"stamp":plan["stamp"],"expected_revision":plan["revision"]})
}
fn success(reply: RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
fn safe_receipt(value: &Json, node: Id, archiving: bool) -> Json {
    assert_eq!(value["projection"], "receipt");
    assert_eq!(value["node_id"], json!(node));
    assert_eq!(value["archiving"], json!(archiving));
    assert!(value["revision"].is_i64());
    assert!(value.get("node").is_none());
    let inverse = value["undo"].clone();
    assert!(inverse["preflight_id"].is_string());
    assert!(inverse["stamp"].is_string());
    assert!(inverse["expected_revision"].is_i64());
    assert!(inverse.get("subtree").is_none());
    assert!(inverse.get("work").is_none());
    assert!(!value.to_string().contains("Confidential"));
    inverse
}

#[tokio::test]
async fn archive_preview_manage_only_keeps_the_complete_effect_set_with_typed_hidden_content() {
    let d = Office::new().await;
    let ordinary = d.ordinary_plan(true).await;
    let writer = d
        .key(vec![rule(
            "NodeArchivePreview",
            "project",
            d.project,
            &["manage"],
        )])
        .await;
    let plan = success(preview(&d.world, &writer.secret, d.project, true).await);
    assert_eq!(plan["counts"], ordinary["counts"]);
    assert_eq!(plan["subtree"].as_array().unwrap().len(), 3);
    assert_eq!(plan["work"].as_array().unwrap().len(), 1);
    assert_eq!(plan["environments"].as_array().unwrap().len(), 1);
    assert_eq!(plan["node"]["id"], json!(d.project));
    assert_eq!(plan["node"]["redacted"], true);
    assert!(plan["node"].get("name").is_none_or(Json::is_null));
    assert!(plan["work"][0]["turns"].is_null());
    assert!(plan["environments"][0]["name"].is_null());
    assert_eq!(plan["copies_kept"], ordinary["copies_kept"]);
    assert_eq!(plan["live_shares_kept"], ordinary["live_shares_kept"]);
    assert!(!d.archived().await);
}

#[tokio::test]
async fn unarchive_preview_keeps_independently_archived_children_and_restarts_nothing() {
    let d = Office::new().await;
    let child = success(preview(&d.world, &d.owner.token, d.topic, true).await);
    success(act(&d.world, &d.owner.token, d.topic, true, &body(&child)).await);
    d.archive_ordinary().await;
    let writer = d
        .key(vec![rule(
            "NodeUnarchivePreview",
            "project",
            d.project,
            &["manage"],
        )])
        .await;
    let plan = success(preview(&d.world, &writer.secret, d.project, false).await);
    assert_eq!(plan["archiving"], false);
    let child = plan["subtree"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["node"]["id"] == json!(d.topic))
        .unwrap();
    assert_eq!(child["own_archived"], true);
    assert_eq!(child["read_only_after"], true);
    assert!(d.archived().await);
}

#[tokio::test]
async fn archive_act_manage_only_commits_the_native_cascade_and_returns_a_safe_inverse_receipt() {
    let d = Office::new().await;
    let plan = d.ordinary_plan(true).await;
    let writer = d
        .key(vec![rule("NodeArchive", "project", d.project, &["manage"])])
        .await;
    let value = success(act(&d.world, &writer.secret, d.project, true, &body(&plan)).await);
    safe_receipt(&value, d.project, true);
    assert!(d.archived().await);
    assert!(
        threads::thread(&mut *d.world.conn().await, d.thread)
            .await
            .unwrap()
            .unwrap()
            .current_turn_id
            .is_none()
    );
}

#[tokio::test]
async fn unarchive_act_freshly_admits_its_own_operation_and_accepts_a_safe_inverse_confirmation() {
    let d = Office::new().await;
    let archived = d.archive_ordinary().await;
    let plan = archived["undo"].clone();
    let writer = d
        .key(vec![rule(
            "NodeUnarchive",
            "project",
            d.project,
            &["manage"],
        )])
        .await;
    let value = success(act(&d.world, &writer.secret, d.project, false, &body(&plan)).await);
    safe_receipt(&value, d.project, false);
    assert!(!d.archived().await);
}

#[tokio::test]
async fn archive_preview_thread_work_and_environment_names_need_the_same_operations_actual_source_reads()
 {
    let d = Office::new().await;
    let reader = d
        .key(vec![
            rule(
                "NodeArchivePreview",
                "project",
                d.project,
                &["manage", "read"],
            ),
            rule("NodeArchivePreview", "thread", d.thread, &["read"]),
            rule(
                "NodeArchivePreview",
                "environment",
                d.environment,
                &["read"],
            ),
        ])
        .await;
    let plan = success(preview(&d.world, &reader.secret, d.project, true).await);
    assert_eq!(plan["node"]["name"], "Confidential project label");
    assert_eq!(
        plan["work"][0]["thread"]["name"],
        "Confidential thread label"
    );
    assert_eq!(plan["work"][0]["turns"], 0);
    assert_eq!(
        plan["environments"][0]["name"],
        "Confidential environment label"
    );
    let unrelated = d
        .key(vec![
            rule(
                "NodeArchivePreview",
                "project",
                d.project,
                &["manage", "read"],
            ),
            rule("NodeGet.read", "thread", d.thread, &["read"]),
            rule("EnvironmentGet", "environment", d.environment, &["read"]),
        ])
        .await;
    let plan = success(preview(&d.world, &unrelated.secret, d.project, true).await);
    assert!(plan["work"][0]["turns"].is_null());
    assert!(plan["environments"][0]["name"].is_null());
    assert_eq!(plan["work"][0]["thread"]["id"], json!(d.thread));
}

#[tokio::test]
async fn full_archive_result_and_stale_current_plan_keep_truthful_native_fields_under_own_operation_reads()
 {
    let d = Office::new().await;
    let plan = d.ordinary_plan(true).await;
    make_topic(&d.world, &d.owner, d.project, "New child after review").await;
    let reader = d
        .key(vec![
            rule("NodeArchive", "project", d.project, &["manage", "read"]),
            rule("NodeArchive", "org", d.org, &["read"]),
            rule("NodeArchive", "topic", d.topic, &["read"]),
            rule("NodeArchive", "thread", d.thread, &["read"]),
            rule("NodeArchive", "environment", d.environment, &["read"]),
        ])
        .await;
    let stale = act(&d.world, &reader.secret, d.project, true, &body(&plan)).await;
    assert_eq!(stale.problem().code, ErrorCode::ConflictStaleRevision);
    let current = stale.problem().extra["current"].clone();
    assert_eq!(current["counts"]["topics"], 2);
    assert_eq!(current["node"]["name"], "Confidential project label");
    assert_eq!(current["work"][0]["turns"], 0);
    assert_eq!(
        current["environments"][0]["name"],
        "Confidential environment label"
    );
    assert!(!d.archived().await);
    let value = success(act(&d.world, &reader.secret, d.project, true, &body(&current)).await);
    assert!(value.get("projection").is_none());
    assert_eq!(value["node"]["name"], "Confidential project label");
    assert_eq!(value["node"]["archived"], true);
    assert_eq!(value["node"]["topic_count"], 2);
    assert_eq!(value["undo"]["subtree"].as_array().unwrap().len(), 4);
    assert_eq!(value["undo"]["work"][0]["turns"], 0);
}

#[tokio::test]
async fn a_review_from_another_same_account_credential_is_data_and_each_act_needs_its_own_current_scope()
 {
    let d = Office::new().await;
    let reviewer = d
        .key(vec![rule(
            "NodeArchivePreview",
            "project",
            d.project,
            &["manage"],
        )])
        .await;
    let plan = success(preview(&d.world, &reviewer.secret, d.project, true).await);
    assert!(
        !act(&d.world, &reviewer.secret, d.project, true, &body(&plan))
            .await
            .status
            .is_success()
    );
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
        .bind(reviewer.id)
        .bind(d.world.now())
        .execute(&d.world.db.pool)
        .await
        .unwrap();
    let writer = d
        .key(vec![rule("NodeArchive", "project", d.project, &["manage"])])
        .await;
    let value = success(act(&d.world, &writer.secret, d.project, true, &body(&plan)).await);
    let inverse = safe_receipt(&value, d.project, true);
    assert!(
        !act(&d.world, &writer.secret, d.project, false, &inverse)
            .await
            .status
            .is_success()
    );
    let undo = d
        .key(vec![rule(
            "NodeUnarchive",
            "project",
            d.project,
            &["manage"],
        )])
        .await;
    success(act(&d.world, &undo.secret, d.project, false, &inverse).await);
    assert!(!d.archived().await);
}

#[tokio::test]
async fn stale_manage_only_archive_supplies_a_bounded_current_review_and_never_archives_on_a_stale_stamp()
 {
    let d = Office::new().await;
    let plan = d.ordinary_plan(true).await;
    make_topic(&d.world, &d.owner, d.project, "Later confidential child").await;
    let writer = d
        .key(vec![rule("NodeArchive", "project", d.project, &["manage"])])
        .await;
    let stale = act(&d.world, &writer.secret, d.project, true, &body(&plan)).await;
    assert_eq!(stale.problem().code, ErrorCode::ConflictStaleRevision);
    let current = stale.problem().extra["current"].clone();
    assert_eq!(current["counts"]["topics"], 2);
    assert_eq!(current["node"]["redacted"], true);
    assert_eq!(current["subtree"].as_array().unwrap().len(), 4);
    assert!(current["work"][0]["turns"].is_null());
    assert!(current["environments"][0]["name"].is_null());
    assert!(!current.to_string().contains("Confidential"));
    assert!(!current.to_string().contains("Later confidential child"));
    assert!(!d.archived().await);
}

#[tokio::test]
async fn archiving_a_thread_keeps_its_native_environment_payload_only_with_this_operations_source_read()
 {
    for source_read in [false, true] {
        let d = Office::new().await;
        let ordinary = success(
            d.world
                .app
                .call(orch_api_contract::nodes::NodeGet, &[&d.thread])
                .token(&d.owner.token)
                .send_bytes()
                .await,
        );
        assert_eq!(ordinary["environment"]["id"], json!(d.environment));
        assert_eq!(
            ordinary["environment"]["name"],
            "Confidential environment label"
        );
        let plan = success(preview(&d.world, &d.owner.token, d.thread, true).await);
        let mut rules = vec![rule("NodeArchive", "thread", d.thread, &["manage", "read"])];
        if source_read {
            rules.push(rule("NodeArchive", "environment", d.environment, &["read"]));
        }
        let writer = d.key(rules).await;
        let value = success(act(&d.world, &writer.secret, d.thread, true, &body(&plan)).await);
        if source_read {
            assert!(value.get("projection").is_none());
            assert_eq!(value["node"]["id"], json!(d.thread));
            assert_eq!(value["node"]["environment"]["id"], json!(d.environment));
            assert_eq!(
                value["node"]["environment"]["name"],
                "Confidential environment label"
            );
            assert_eq!(value["node"]["archived"], true);
            assert!(
                value["node"]["environment_permissions"]
                    .as_array()
                    .is_some()
            );
        } else {
            safe_receipt(&value, d.thread, true);
        }
    }
}
