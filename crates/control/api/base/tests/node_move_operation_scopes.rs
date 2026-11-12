//! Six real hierarchy operations share native movement rules and independent scoped effects/disclosure.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod places;
mod support;

use axum::body::{Body, to_bytes};
use bytes::Bytes;
use chrono::Duration;
use futures::stream;
use http::{Request, StatusCode};
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::nodes::{
    OrgMove, OrgMovePreview, ProjectMove, ProjectMovePreview, TopicMove, TopicMovePreview,
};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::registry::Registry;
use orch_api_core::testing::RawReply;
use orch_db::queries::{hierarchy, identity, transaction_guard};
use orch_settings::Target;
use orch_types::{Id, Json, new_id};
use places::{Who, make_department, make_org, make_project, make_topic, person};
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use support::World;
use tower::ServiceExt;

#[derive(Clone, Copy, Debug)]
enum Kind {
    Org,
    Project,
    Topic,
}
impl Kind {
    const ALL: [Self; 3] = [Self::Org, Self::Project, Self::Topic];
    fn word(self) -> &'static str {
        match self {
            Self::Org => "org",
            Self::Project => "project",
            Self::Topic => "topic",
        }
    }
    fn preview(self) -> &'static str {
        match self {
            Self::Org => "OrgMovePreview",
            Self::Project => "ProjectMovePreview",
            Self::Topic => "TopicMovePreview",
        }
    }
    fn operation(self) -> &'static str {
        match self {
            Self::Org => "OrgMove",
            Self::Project => "ProjectMove",
            Self::Topic => "TopicMove",
        }
    }
    fn destination_action(self) -> &'static str {
        if matches!(self, Self::Org) {
            "manage"
        } else {
            "write"
        }
    }
}
struct Key {
    id: Id,
    secret: String,
}
fn rule(operation: &str, business: &str, target_kind: &str, target: Id, actions: &[&str]) -> Json {
    json!({"target":{"kind":target_kind,"id":target,"reach":"self"},"resources":[business],"allow":[{"operation":operation,"actions":actions}]})
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
            name: "Node move scope control",
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
fn success(reply: RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
struct Scene {
    world: World,
    owner: Who,
    root: Id,
    left: Id,
    node: Id,
    mirror: Id,
    parent: Id,
    parent_kind: &'static str,
    kind: Kind,
}
impl Scene {
    async fn new(kind: Kind) -> Self {
        let world = World::new().await;
        let owner = person(&world, "owner").await;
        let root = make_org(&world, &owner, "Move office", "move-office")
            .await
            .base
            .id;
        let left = make_department(
            &world,
            &owner,
            root,
            "Source department",
            "source-department",
        )
        .await
        .base
        .id;
        let right = make_department(
            &world,
            &owner,
            root,
            "Destination department",
            "destination-department",
        )
        .await
        .base
        .id;
        let (node, mirror, parent, parent_kind) = match kind {
            Kind::Org => (
                make_department(
                    &world,
                    &owner,
                    left,
                    "Confidential moving org",
                    "moving-org",
                )
                .await
                .base
                .id,
                make_department(&world, &owner, left, "Ordinary mirror org", "mirror-org")
                    .await
                    .base
                    .id,
                right,
                "org",
            ),
            Kind::Project => (
                make_project(&world, &owner, left, "Confidential moving project")
                    .await
                    .base
                    .id,
                make_project(&world, &owner, left, "Ordinary mirror project")
                    .await
                    .base
                    .id,
                right,
                "org",
            ),
            Kind::Topic => {
                let from = make_project(&world, &owner, left, "Source project")
                    .await
                    .base
                    .id;
                let to = make_project(&world, &owner, right, "Destination project")
                    .await
                    .base
                    .id;
                (
                    make_topic(&world, &owner, from, "Confidential moving topic")
                        .await
                        .base
                        .id,
                    make_topic(&world, &owner, from, "Ordinary mirror topic")
                        .await
                        .base
                        .id,
                    to,
                    "project",
                )
            }
        };
        Self {
            world,
            owner,
            root,
            left,
            node,
            mirror,
            parent,
            parent_kind,
            kind,
        }
    }
    fn effects(&self, operation: &str) -> Vec<Json> {
        vec![
            rule(
                operation,
                self.kind.word(),
                self.kind.word(),
                self.node,
                &["manage"],
            ),
            rule(
                operation,
                self.kind.word(),
                self.parent_kind,
                self.parent,
                &[self.kind.destination_action()],
            ),
        ]
    }
    async fn key(&self, rules: Vec<Json>) -> Key {
        key(&self.world, self.owner.id(), rules).await
    }
    async fn revision(&self, node: Id) -> i64 {
        hierarchy::with_ancestors(&mut *self.world.conn().await, &[node])
            .await
            .unwrap()
            .into_iter()
            .find(|row| row.id == node)
            .unwrap()
            .revision
    }
    async fn parent_of(&self, node: Id) -> Option<Id> {
        hierarchy::with_ancestors(&mut *self.world.conn().await, &[node])
            .await
            .unwrap()
            .into_iter()
            .find(|row| row.id == node)
            .unwrap()
            .parent_id
    }
}
async fn preview(s: &Scene, token: &str, parent: Id) -> RawReply {
    let body = json!({"parent_id":parent});
    match s.kind {
        Kind::Org => {
            s.world
                .app
                .call(OrgMovePreview, &[&s.node])
                .token(token)
                .raw_body(&body)
                .send_bytes()
                .await
        }
        Kind::Project => {
            s.world
                .app
                .call(ProjectMovePreview, &[&s.node])
                .token(token)
                .raw_body(&body)
                .send_bytes()
                .await
        }
        Kind::Topic => {
            s.world
                .app
                .call(TopicMovePreview, &[&s.node])
                .token(token)
                .raw_body(&body)
                .send_bytes()
                .await
        }
    }
}
async fn move_node(s: &Scene, token: &str, node: Id, body: &Json) -> RawReply {
    match s.kind {
        Kind::Org => {
            s.world
                .app
                .call(OrgMove, &[&node])
                .token(token)
                .raw_body(body)
                .send_bytes()
                .await
        }
        Kind::Project => {
            s.world
                .app
                .call(ProjectMove, &[&node])
                .token(token)
                .raw_body(body)
                .send_bytes()
                .await
        }
        Kind::Topic => {
            s.world
                .app
                .call(TopicMove, &[&node])
                .token(token)
                .raw_body(body)
                .send_bytes()
                .await
        }
    }
}
fn observe(reply: RawReply, operation: &str, failures: &mut Vec<String>) -> Option<Json> {
    if reply.status != StatusCode::OK {
        failures.push(format!(
            "{operation}: expected200 got{} {}",
            reply.status,
            String::from_utf8_lossy(&reply.raw)
        ));
        None
    } else {
        Some(serde_json::from_slice(&reply.raw).unwrap())
    }
}

#[tokio::test]
async fn all_six_scoped_moves_and_previews_keep_native_parity_and_genuine_no_read_receipts() {
    let mut failures = Vec::new();
    for kind in Kind::ALL {
        let s = Scene::new(kind).await;
        // One ordinary mirror move proves the actual business kind/destination
        // combination works, without consuming the modern node's optional review.
        let mirror = move_node(
            &s,
            &s.owner.token,
            s.mirror,
            &json!({"parent_id":s.parent,"expected_revision":s.revision(s.mirror).await}),
        )
        .await;
        observe(
            mirror,
            &format!("ordinary {}", kind.operation()),
            &mut failures,
        );
        let ordinary = preview(&s, &s.owner.token, s.parent).await;
        if let Some(plan) = observe(
            ordinary,
            &format!("ordinary {}", kind.preview()),
            &mut failures,
        ) && !plan["blockers"].as_array().is_some_and(Vec::is_empty)
        {
            failures.push(format!(
                "ordinary {} has blockers: {}",
                kind.preview(),
                plan["blockers"]
            ));
        }
        let before = s.parent_of(s.node).await;
        let reader = s.key(s.effects(kind.preview())).await;
        if let Some(plan) = observe(
            preview(&s, &reader.secret, s.parent).await,
            kind.preview(),
            &mut failures,
        ) && (plan["projection"] != "receipt"
            || plan["node_id"] != json!(s.node)
            || plan["parent_id"] != json!(s.parent)
            || !plan["preflight_id"].is_string()
            || !plan["revision"].is_i64()
            || plan.to_string().contains("Confidential"))
        {
            failures.push(format!(
                "{} did not return its safe review receipt: {plan}",
                kind.preview()
            ));
        }
        assert_eq!(
            s.parent_of(s.node).await,
            before,
            "a preview never moves the node"
        );
        let writer = s.key(s.effects(kind.operation())).await;
        let reply = move_node(
            &s,
            &writer.secret,
            s.node,
            &json!({"parent_id":s.parent,"expected_revision":s.revision(s.node).await}),
        )
        .await;
        if let Some(value) = observe(reply, kind.operation(), &mut failures) {
            if value["projection"] != "receipt"
                || value["node_id"] != json!(s.node)
                || value["parent_id"] != json!(s.parent)
                || !value["revision"].is_i64()
                || value.to_string().contains("Confidential")
            {
                failures.push(format!(
                    "{} did not return its safe settled receipt: {value}",
                    kind.operation()
                ));
            }
            if s.parent_of(s.node).await != Some(s.parent) {
                failures.push(format!(
                    "{} returned success without the native move",
                    kind.operation()
                ));
            }
        }
    }
    // Accumulate observations so a missing first hook cannot hide the other
    // five actual endpoint attempts in the deciding Before run.
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn project_move_requires_its_exact_both_ended_facets_and_preserves_native_invalid_parent_branch()
 {
    let s = Scene::new(Kind::Project).await;
    let before = s.parent_of(s.node).await;
    let body = json!({"parent_id":s.parent,"expected_revision":s.revision(s.node).await});
    for rules in [
        vec![rule(
            "ProjectMove",
            "project",
            "project",
            s.node,
            &["manage"],
        )],
        vec![
            rule("ProjectMove", "project", "project", s.node, &["write"]),
            rule("ProjectMove", "project", "org", s.parent, &["manage"]),
        ],
        vec![
            rule("ProjectMove", "project", "project", s.node, &["manage"]),
            rule("OrgMove", "project", "org", s.parent, &["write"]),
        ],
    ] {
        let writer = s.key(rules).await;
        assert_eq!(
            move_node(&s, &writer.secret, s.node, &body)
                .await
                .problem()
                .code,
            ErrorCode::AuthzDenied
        );
        assert_eq!(s.parent_of(s.node).await, before);
    }
    let source_project = make_project(&s.world, &s.owner, s.left, "Invalid-parent holder")
        .await
        .base
        .id;
    let invalid = make_topic(&s.world, &s.owner, source_project, "Wrong kind parent")
        .await
        .base
        .id;
    let ordinary = success(preview(&s, &s.owner.token, invalid).await);
    assert!(
        ordinary["blockers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|blocker| blocker["code"] == "node.wrong_parent")
    );
    // Native permission_to_receive returns None for a Topic destination. A
    // blocked review must not invent an additional destination authority gate.
    let reviewer = s
        .key(vec![rule(
            "ProjectMovePreview",
            "project",
            "project",
            s.node,
            &["manage"],
        )])
        .await;
    let blocked = success(preview(&s, &reviewer.secret, invalid).await);
    assert_eq!(blocked["projection"], "receipt");
    assert!(
        blocked["blocker_codes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|code| code == "node.wrong_parent")
    );
    assert_eq!(s.parent_of(s.node).await, before);
}

#[tokio::test]
async fn native_full_move_review_stamp_is_independent_of_credential_projection_and_values_need_own_reads()
 {
    let s = Scene::new(Kind::Project).await;
    let old_value = "Existing source instruction content";
    let new_value = "Existing destination instruction content";
    s.world
        .set_at(Target::Node(s.left), "agent.instructions", json!(old_value))
        .await;
    s.world
        .set_at(
            Target::Node(s.parent),
            "agent.instructions",
            json!(new_value),
        )
        .await;
    let ordinary = success(preview(&s, &s.owner.token, s.parent).await);
    let changed = ordinary["settings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["key"] == "agent.instructions")
        .expect("the ordinary native preview must really contain the changed setting");
    assert!(changed["from"].to_string().contains(old_value));
    assert!(changed["to"].to_string().contains(new_value));
    let reviewer = s.key(s.effects("ProjectMovePreview")).await;
    let receipt = success(preview(&s, &reviewer.secret, s.parent).await);
    assert_eq!(receipt["projection"], "receipt");
    assert_eq!(receipt["stamp"], ordinary["stamp"]);
    assert!(!receipt.to_string().contains(old_value));
    assert!(!receipt.to_string().contains(new_value));
    let full_rule = |operation: &str| json!({"target":{"kind":"org","id":s.root,"reach":"subtree"},"allow":[{"operation":operation,"actions":["manage","write","read"]}]});
    let full_reader = s.key(vec![full_rule("ProjectMovePreview")]).await;
    let full = success(preview(&s, &full_reader.secret, s.parent).await);
    assert_eq!(full["settings"], ordinary["settings"]);
    assert_eq!(full["access"], ordinary["access"]);
    assert_eq!(full["roles"], ordinary["roles"]);
    assert_eq!(full["stamp"], ordinary["stamp"]);
    let writer = s.key(vec![full_rule("ProjectMove")]).await;
    let moved=success(move_node(&s,&writer.secret,s.node,&json!({"parent_id":s.parent,"expected_revision":receipt["revision"],"preflight_id":receipt["preflight_id"]})).await);
    assert_eq!(moved["id"], json!(s.node));
    assert_eq!(moved["parent_id"], json!(s.parent));
    assert!(moved.get("projection").is_none());
    assert_eq!(s.parent_of(s.node).await, Some(s.parent));
}

#[tokio::test]
async fn a_legitimate_move_commits_when_its_old_subtree_read_no_longer_reaches_the_result() {
    let s = Scene::new(Kind::Project).await;
    let mut rules = s.effects("ProjectMove");
    rules.push(json!({"target":{"kind":"org","id":s.left,"reach":"subtree"},"resources":["project"],"allow":[{"operation":"ProjectMove","actions":["read"]}]}));
    let writer = s.key(rules).await;
    let value = success(
        move_node(
            &s,
            &writer.secret,
            s.node,
            &json!({"parent_id":s.parent,"expected_revision":s.revision(s.node).await}),
        )
        .await,
    );
    assert_eq!(value["projection"], "receipt");
    assert_eq!(value["node_id"], json!(s.node));
    assert!(!value.to_string().contains("Confidential"));
    assert_eq!(s.parent_of(s.node).await, Some(s.parent));
}

async fn guarded_destination_scope(narrow: bool) {
    let s = Scene::new(Kind::Project).await;
    let before = s.parent_of(s.node).await;
    let writer = s.key(s.effects("ProjectMove")).await;
    let current: KeyScope = serde_json::from_value(
        json!({"rules":[rule("ProjectMove","project","project",s.node,&["manage"])]}),
    )
    .unwrap();
    assert!(current.validate().is_empty());
    let current = serde_json::to_value(current).unwrap();
    let mut blocker = s.world.db.pool.begin().await.unwrap();
    transaction_guard::lock_person(&mut blocker, s.owner.id())
        .await
        .unwrap()
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let body = json!({"parent_id":s.parent,"expected_revision":s.revision(s.node).await});
    let body_polled = Arc::new(AtomicBool::new(false));
    let recorded = Arc::clone(&body_polled);
    let pool = s.world.db.pool.clone();
    let token = writer.id;
    let actor = s.owner.id();
    let request = Request::builder()
        .method("POST")
        .uri(format!("/projects/{}/move", s.node))
        .header("authorization", format!("Bearer {}", writer.secret))
        .header("content-type", "application/json")
        .body(Body::from_stream(stream::once(async move {
            let authenticated: bool = sqlx::query_scalar(
                "SELECT last_used_at IS NOT NULL FROM api_tokens WHERE id=$1 AND user_id=$2",
            )
            .bind(token)
            .bind(actor)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert!(
                authenticated,
                "the key is authenticated before this body cut"
            );
            if narrow {
                // Commit before the action can SHARE-lock the token; only the
                // user row is blocked, so this never updates a guard-held token.
                sqlx::query("UPDATE api_tokens SET operation_scope=$3 WHERE id=$1 AND user_id=$2")
                    .bind(token)
                    .bind(actor)
                    .bind(current)
                    .execute(&pool)
                    .await
                    .unwrap();
            }
            recorded.store(true, Ordering::SeqCst);
            Ok::<_, std::io::Error>(Bytes::from(serde_json::to_vec(&body).unwrap()))
        })))
        .unwrap();
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let app = registry
        .into_router_for(orch_api_contract::surface::HttpSurface::Key)
        .with_state(s.world.app.app.clone());
    let mut action = tokio::spawn(async move { app.oneshot(request).await.unwrap() });
    // Same request watchdog as the existing identity-guard controls; elapsed
    // time is never permission or proof of reaching the intended checkpoint.
    let patience = std::time::Duration::from_secs(30);
    let reached=tokio::time::timeout(patience,async {
        loop {
            if action.is_finished() {return false;}
            let actual:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE 'WITH watched AS MATERIALIZED (%' AND query LIKE '%orch_authz_saw%')")
                .bind(pid).fetch_one(&s.world.db.pool).await.unwrap();
            if actual {return true;}
            tokio::task::yield_now().await;
        }
    }).await.unwrap_or(false);
    blocker.commit().await.unwrap();
    let response = match tokio::time::timeout(patience, &mut action).await {
        Ok(value) => value.unwrap(),
        Err(error) => {
            action.abort();
            let _ = action.await;
            panic!("{1}: {:?}", error, "the released move guard completes")
        }
    };
    let status = response.status();
    let raw = to_bytes(
        response.into_body(),
        orch_api_contract::common::MAX_JSON_BYTES,
    )
    .await
    .unwrap();
    assert!(
        body_polled.load(Ordering::SeqCst),
        "body boundary must actually be reached; an unbound Before is not current-guard evidence"
    );
    if narrow {
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{}",
            String::from_utf8_lossy(&raw)
        );
        assert_eq!(s.parent_of(s.node).await, before);
        // A safe earlier current-scope refusal also prevents the act. Do not
        // promote it to a deep guard witness if the common door rejected first.
    } else {
        assert!(
            reached,
            "the successful control must actually wait at its action identity guard"
        );
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&raw));
        let value: Json = serde_json::from_slice(&raw).unwrap();
        assert_eq!(value["projection"], "receipt");
        assert_eq!(s.parent_of(s.node).await, Some(s.parent));
    }
}

#[tokio::test]
async fn project_move_current_destination_scope_is_rechecked_after_authentication_before_effects() {
    guarded_destination_scope(false).await;
    guarded_destination_scope(true).await;
}

#[tokio::test]
async fn a_move_result_can_be_read_through_its_actual_destination_subtree_after_placement() {
    let s = Scene::new(Kind::Project).await;
    let mut rules = s.effects("ProjectMove");
    rules.push(json!({"target":{"kind":"org","id":s.parent,"reach":"subtree"},"resources":["project"],"allow":[{"operation":"ProjectMove","actions":["read"]}]}));
    let writer = s.key(rules).await;
    let value = success(
        move_node(
            &s,
            &writer.secret,
            s.node,
            &json!({"parent_id":s.parent,"expected_revision":s.revision(s.node).await}),
        )
        .await,
    );
    assert!(value.get("projection").is_none());
    assert_eq!(value["id"], json!(s.node));
    assert_eq!(value["parent_id"], json!(s.parent));
    assert_eq!(value["name"], "Confidential moving project");
    assert_eq!(s.parent_of(s.node).await, Some(s.parent));
}
