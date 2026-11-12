//! Real scoped-key group producers; raw replies keep these controls compatible with the baseline.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod places;
mod support;

use crate::support::FullPermissionPageReply;
use axum::body::{Body, to_bytes};
use chrono::Duration;
use http::{Request, StatusCode};
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::common::{ErrorCode, MAX_JSON_BYTES};
use orch_api_contract::directory::{
    GroupHolderAdd, GroupHolderList, GroupUpdate, OrgGroupCreate, OrgGroupCreateBody,
};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::registry::Registry;
use orch_api_core::testing::RawReply;
use orch_db::queries::{access, identity, transaction_guard};
use orch_types::{Id, Json, new_id};
use serde_json::json;
use support::World;
use tower::ServiceExt;

struct Key {
    id: Id,
    secret: String,
}

async fn key(world: &World, user: Id, rules: Vec<Json>) -> Key {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    let hash = token_hash(&secret);
    let id = new_id();
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id,
            user_id: user,
            name: "Group operation control",
            token_hash: &hash,
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

fn rule(operation: &str, kind: &str, id: Id, actions: &[&str]) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"resources":[kind],
        "allow":[{"operation":operation,"actions":actions}]})
}

fn names(operation: &str, org: Id) -> Json {
    json!({"target":{"kind":"org","id":org,"reach":"subtree"},"resources":["org","project","topic","thread"],
        "allow":[{"operation":operation,"actions":["read"]}]})
}

fn success(reply: &RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}

async fn update(world: &World, token: &str, group: Id, body: &Json) -> RawReply {
    world
        .app
        .call(GroupUpdate, &[&group])
        .token(token)
        .raw_body(body)
        .send_bytes()
        .await
}

async fn holders(world: &World, token: &str, group: Id) -> RawReply {
    world
        .app
        .call(GroupHolderList, &[&group])
        .token(token)
        .send_bytes()
        .await
}

async fn add(world: &World, token: &str, group: Id, body: &Json) -> RawReply {
    world
        .app
        .call(GroupHolderAdd, &[&group])
        .token(token)
        .raw_body(body)
        .send_bytes()
        .await
}

async fn make_group(world: &World, boss: &places::Who, org: Id, name: &str) -> Id {
    world
        .app
        .call(OrgGroupCreate, &[&org])
        .token(&boss.token)
        .body(&OrgGroupCreateBody {
            name: name.into(),
            description: Some(format!("Private description of {name}")),
        })
        .send()
        .await
        .ok()
        .into_full()
        .expect("this ordinary control requires the full native creation response")
        .id
}

struct Desk {
    world: World,
    boss: places::Who,
    member: places::Who,
    org: Id,
    department: Id,
    outer: Id,
    inner: Id,
}

impl Desk {
    async fn new() -> Self {
        let world = World::new().await;
        let boss = places::person(&world, "owner").await;
        let org = places::make_org(&world, &boss, "Root names sentinel", "group-root")
            .await
            .base
            .id;
        let department =
            places::make_department(&world, &boss, org, "Keeping org sentinel", "keepers")
                .await
                .base
                .id;
        let member = places::person(&world, "member").await;
        places::join(&world, org, department, &member, "viewer", &boss).await;
        let outer = make_group(&world, &boss, department, "Outer group sentinel").await;
        let inner = make_group(&world, &boss, department, "Inner group sentinel").await;
        Self {
            world,
            boss,
            member,
            org,
            department,
            outer,
            inner,
        }
    }
    async fn key(&self, rules: Vec<Json>) -> Key {
        key(&self.world, self.boss.id(), rules).await
    }
    async fn seed_holders(&self) {
        for (group, body) in [
            (self.inner, json!({"kind":"user","id":self.member.id()})),
            (self.outer, json!({"kind":"user","id":self.member.id()})),
            (self.outer, json!({"kind":"group","id":self.inner})),
            (self.outer, json!({"kind":"members","id":self.department})),
            (self.outer, json!({"kind":"role","role":"viewer"})),
        ] {
            assert_eq!(
                add(&self.world, &self.boss.token, group, &body)
                    .await
                    .status,
                StatusCode::NO_CONTENT
            );
        }
    }
}

async fn stored(world: &World, group: Id) -> (String, String) {
    sqlx::query_as("SELECT name,description FROM groups WHERE id=$1")
        .bind(group)
        .fetch_one(&world.db.pool)
        .await
        .unwrap()
}

async fn audits(world: &World, action: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action=$1")
        .bind(action)
        .fetch_one(&world.db.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn group_update_manage_only_returns_an_exact_receipt_and_keeps_partial_and_empty_updates() {
    let d = Desk::new().await;
    d.seed_holders().await;
    let writer = d
        .key(vec![rule("GroupUpdate", "group", d.outer, &["manage"])])
        .await;
    let before = audits(&d.world, "group.updated").await;
    let first = success(
        &update(
            &d.world,
            &writer.secret,
            d.outer,
            &json!({"name":"Renamed by bounded key"}),
        )
        .await,
    );
    assert_eq!(first, json!({"projection":"receipt","group_id":d.outer}));
    assert_eq!(
        stored(&d.world, d.outer).await,
        (
            "Renamed by bounded key".into(),
            "Private description of Outer group sentinel".into()
        )
    );
    assert_eq!(audits(&d.world, "group.updated").await, before + 1);
    // The existing no-op still writes its ordinary audit; there is no invented revision field.
    let empty = success(&update(&d.world, &writer.secret, d.outer, &json!({})).await);
    assert_eq!(empty, first);
    assert_eq!(audits(&d.world, "group.updated").await, before + 2);
    let ordinary = success(&update(&d.world, &d.boss.token, d.outer, &json!({})).await);
    assert_eq!(ordinary["name"], "Renamed by bounded key");
    assert!(ordinary["member_count"].as_i64().unwrap() > 0);
    assert!(ordinary.get("projection").is_none());
}

#[tokio::test]
async fn group_update_full_content_and_keeper_names_need_their_own_same_operation_reads() {
    let d = Desk::new().await;
    d.seed_holders().await;
    let both = rule("GroupUpdate", "group", d.outer, &["manage", "read"]);
    let limited_names = d.key(vec![both.clone()]).await;
    let shown = success(&update(&d.world, &limited_names.secret, d.outer, &json!({})).await);
    let ordinary = success(&update(&d.world, &d.boss.token, d.outer, &json!({})).await);
    for field in ["id", "org_id", "name", "description", "member_count"] {
        assert_eq!(shown[field], ordinary[field]);
    }
    assert!(shown["place"].is_null());
    assert!(
        ordinary["place"]
            .as_str()
            .unwrap()
            .contains("Keeping org sentinel")
    );
    let full_names = d.key(vec![both, names("GroupUpdate", d.org)]).await;
    let full = success(&update(&d.world, &full_names.secret, d.outer, &json!({})).await);
    let current_revision: i64 = sqlx::query_scalar("SELECT revision FROM groups WHERE id=$1")
        .bind(d.outer)
        .fetch_one(&d.world.db.pool)
        .await
        .unwrap();
    assert_eq!(full["revision"], json!(current_revision));
    assert!(full["revision"].as_i64().unwrap() > ordinary["revision"].as_i64().unwrap());
    assert!(ordinary["revision"].as_i64().unwrap() > shown["revision"].as_i64().unwrap());
    let mut expected = ordinary.clone();
    expected["revision"] = json!(current_revision);
    assert_eq!(full, expected);
    let wrong_operation = d
        .key(vec![
            rule("GroupUpdate", "group", d.outer, &["manage"]),
            rule("GroupHolderList", "group", d.outer, &["read"]),
        ])
        .await;
    assert_eq!(
        success(&update(&d.world, &wrong_operation.secret, d.outer, &json!({})).await),
        json!({"projection":"receipt","group_id":d.outer})
    );
}

#[tokio::test]
async fn update_refusals_preserve_collision_and_ordinary_management_without_private_payloads() {
    let d = Desk::new().await;
    let writer = d
        .key(vec![rule("GroupUpdate", "group", d.outer, &["manage"])])
        .await;
    let before = (
        stored(&d.world, d.outer).await,
        audits(&d.world, "group.updated").await,
    );
    let conflict = update(
        &d.world,
        &writer.secret,
        d.outer,
        &json!({"name":"Inner group sentinel"}),
    )
    .await;
    assert_eq!(conflict.problem().code, ErrorCode::ConflictExists);
    assert_eq!(
        (
            stored(&d.world, d.outer).await,
            audits(&d.world, "group.updated").await
        ),
        before
    );
    let other_target = update(
        &d.world,
        &writer.secret,
        d.inner,
        &json!({"name":"Must not change"}),
    )
    .await;
    assert_eq!(other_target.problem().code, ErrorCode::AuthzDenied);
    let viewer = key(
        &d.world,
        d.member.id(),
        vec![rule("GroupUpdate", "group", d.outer, &["manage"])],
    )
    .await;
    let denied = update(
        &d.world,
        &viewer.secret,
        d.outer,
        &json!({"name":"No actual manager"}),
    )
    .await;
    let wire = String::from_utf8_lossy(&denied.raw);
    assert!(!wire.contains("Outer group sentinel") && !wire.contains("Keeping org sentinel"));
    assert_eq!(denied.problem().code, ErrorCode::AuthzDenied);
    assert_eq!(stored(&d.world, d.outer).await, before.0);
}

#[tokio::test]
async fn holder_list_requires_each_actual_group_payload_but_preserves_public_people_and_redacted_places()
 {
    let d = Desk::new().await;
    d.seed_holders().await;
    let primary = rule("GroupHolderList", "group", d.outer, &["read"]);
    for extra in [None, Some(rule("GroupUpdate", "group", d.inner, &["read"]))] {
        let mut rules = vec![primary.clone()];
        rules.extend(extra);
        let missing = d.key(rules).await;
        assert_eq!(
            holders(&d.world, &missing.secret, d.outer)
                .await
                .problem()
                .code,
            ErrorCode::AuthzDenied
        );
    }
    let nested = rule("GroupHolderList", "group", d.inner, &["read"]);
    let narrow = d.key(vec![primary.clone(), nested.clone()]).await;
    let actual = success(&holders(&d.world, &narrow.secret, d.outer).await);
    let ordinary = success(&holders(&d.world, &d.boss.token, d.outer).await);
    assert_eq!(
        actual.as_array().unwrap().len(),
        ordinary.as_array().unwrap().len()
    );
    let find = |value: &Json, kind: &str| {
        value
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["kind"] == kind)
            .unwrap()
            .clone()
    };
    let mut public_user = find(&actual, "user");
    let mut domain_user = find(&ordinary, "user");
    assert_eq!(public_user["user"]["address"], "@member");
    assert!(
        domain_user["user"]["address"]
            .as_str()
            .unwrap()
            .ends_with(".member")
    );
    public_user["user"]
        .as_object_mut()
        .unwrap()
        .remove("address");
    domain_user["user"]
        .as_object_mut()
        .unwrap()
        .remove("address");
    assert_eq!(
        public_user, domain_user,
        "public identity fields remain public; domain qualification follows the existing name bound"
    );
    let group = find(&actual, "group");
    let original_group = find(&ordinary, "group");
    for field in ["id", "org_id", "name", "description", "member_count"] {
        assert_eq!(group["group"][field], original_group["group"][field]);
    }
    assert!(group["group"]["place"].is_null());
    for kind in ["members", "role"] {
        let holder = find(&actual, kind);
        let text = serde_json::to_string(&holder).unwrap();
        assert!(!text.contains("Keeping org sentinel") && !text.contains("Root names sentinel"));
        assert!(text.contains("\"redacted\":true"));
    }
    let full = d
        .key(vec![primary, nested, names("GroupHolderList", d.org)])
        .await;
    assert_eq!(
        success(&holders(&d.world, &full.secret, d.outer).await),
        ordinary
    );
}

#[tokio::test]
async fn holder_add_manage_only_keeps_all_four_existing_body_branches_and_empty_replies() {
    let d = Desk::new().await;
    let writer = d
        .key(vec![rule("GroupHolderAdd", "group", d.outer, &["manage"])])
        .await;
    for input in [
        json!({"kind":"user","id":d.member.id()}),
        json!({"kind":"group","id":d.inner}),
        json!({"kind":"members","id":d.department}),
        json!({"kind":"role","role":"viewer"}),
    ] {
        let before = access::group_holders(&mut *d.world.conn().await, d.outer)
            .await
            .unwrap()
            .len();
        let reply = add(&d.world, &writer.secret, d.outer, &input).await;
        assert_eq!(
            reply.status,
            StatusCode::NO_CONTENT,
            "{}",
            String::from_utf8_lossy(&reply.raw)
        );
        assert!(reply.raw.is_empty());
        assert_eq!(
            access::group_holders(&mut *d.world.conn().await, d.outer)
                .await
                .unwrap()
                .len(),
            before + 1
        );
    }
    let rows = access::group_holders(&mut *d.world.conn().await, d.outer)
        .await
        .unwrap();
    assert!(
        rows.iter()
            .any(|row| row.member_node_id == Some(d.department)
                && row.member_role.as_deref() == Some("viewer")),
        "omitted role.at means the actual keeping org"
    );
    // This writer returns no nested Group DTO, so it does not acquire a holder-list Read requirement.
    assert!(rows.iter().any(|row| row.member_group_id == Some(d.inner)));
    let before = rows.len();
    let cycle = add(
        &d.world,
        &writer.secret,
        d.outer,
        &json!({"kind":"group","id":d.outer}),
    )
    .await;
    assert_eq!(cycle.problem().code, ErrorCode::RequestInvalid);
    assert_eq!(
        access::group_holders(&mut *d.world.conn().await, d.outer)
            .await
            .unwrap()
            .len(),
        before
    );
    let wrong = add(
        &d.world,
        &writer.secret,
        d.inner,
        &json!({"kind":"user","id":d.member.id()}),
    )
    .await;
    assert_eq!(wrong.problem().code, ErrorCode::AuthzDenied);
    assert!(
        access::group_holders(&mut *d.world.conn().await, d.inner)
            .await
            .unwrap()
            .is_empty()
    );
    let mut chain = Vec::new();
    for depth in 0..6 {
        chain.push(make_group(&d.world, &d.boss, d.department, &format!("Depth {depth}")).await);
    }
    for pair in chain[..5].windows(2) {
        assert_eq!(
            add(
                &d.world,
                &d.boss.token,
                pair[0],
                &json!({"kind":"group","id":pair[1]})
            )
            .await
            .status,
            StatusCode::NO_CONTENT
        );
    }
    let last = d
        .key(vec![rule("GroupHolderAdd", "group", chain[4], &["manage"])])
        .await;
    for source in [chain[0], chain[5]] {
        let denied = add(
            &d.world,
            &last.secret,
            chain[4],
            &json!({"kind":"group","id":source}),
        )
        .await;
        assert_eq!(
            denied.problem().code,
            ErrorCode::RequestInvalid,
            "both a cycle and a sixth group remain invalid"
        );
    }
    assert!(
        access::group_holders(&mut *d.world.conn().await, chain[4])
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn holder_add_still_checks_the_actual_source_place_and_preserves_public_validation() {
    let d = Desk::new().await;
    let stranger = places::person(&d.world, "stranger").await;
    let hidden = places::make_org(
        &d.world,
        &stranger,
        "Foreign source sentinel",
        "foreign-source",
    )
    .await
    .base
    .id;
    let foreign = make_group(&d.world, &stranger, hidden, "Foreign group sentinel").await;
    let writer = d
        .key(vec![rule(
            "GroupHolderAdd",
            "group",
            d.outer,
            &["manage", "read"],
        )])
        .await;
    for input in [
        json!({"kind":"group","id":foreign}),
        json!({"kind":"members","id":hidden}),
        json!({"kind":"role","role":"viewer","at":hidden}),
    ] {
        let ordinary = add(&d.world, &d.boss.token, d.outer, &input).await;
        let denied = add(&d.world, &writer.secret, d.outer, &input).await;
        let wire = String::from_utf8_lossy(&denied.raw);
        assert!(
            !wire.contains("Foreign source sentinel") && !wire.contains("Foreign group sentinel")
        );
        assert_eq!(denied.problem().code, ordinary.problem().code);
        assert!(
            access::group_holders(&mut *d.world.conn().await, d.outer)
                .await
                .unwrap()
                .is_empty()
        );
    }
    let no_person = add(
        &d.world,
        &writer.secret,
        d.outer,
        &json!({"kind":"user","id":new_id()}),
    )
    .await;
    assert_eq!(no_person.problem().code, ErrorCode::RequestInvalid);
    let unsupported = add(
        &d.world,
        &writer.secret,
        d.outer,
        &json!({"kind":"everyone"}),
    )
    .await;
    assert_eq!(unsupported.problem().code, ErrorCode::RequestInvalid);
    d.world
        .set_at(
            orch_settings::Target::Node(d.department),
            "sharing.allow_external",
            json!(false),
        )
        .await;
    // The owner already holds access.external through the shipped role defaults.
    // The policy only disables the owner shortcut; it does not revoke that right.
    // Use a member who can manage groups but has no independent external authority.
    d.world
        .app
        .call(
            orch_api_contract::access::PermissionPageUpdate,
            &[&"nodes", &d.department],
        )
        .token(&d.boss.token)
        .raw_body(&json!({"rows":[{"permission":"members.manage","add":[{"holder":{"kind":"user","id":d.member.id()}}]}]}))
        .send()
        .await.full_permission_page()
        .ok();
    let limited = key(
        &d.world,
        d.member.id(),
        vec![rule("GroupHolderAdd", "group", d.outer, &["manage"])],
    )
    .await;
    let outside = json!({"kind":"user","id":stranger.id()});
    let ordinary = add(&d.world, &d.member.token, d.outer, &outside).await;
    let scoped = add(&d.world, &limited.secret, d.outer, &outside).await;
    assert_eq!(scoped.problem().code, ordinary.problem().code);
    assert!(
        access::group_holders(&mut *d.world.conn().await, d.outer)
            .await
            .unwrap()
            .is_empty()
    );
    // Existing explicit authority remains available with the same policy disabled.
    let ordinary = add(&d.world, &d.boss.token, d.outer, &outside).await;
    let scoped = add(&d.world, &writer.secret, d.outer, &outside).await;
    assert_eq!(ordinary.status, StatusCode::NO_CONTENT);
    assert_eq!(scoped.status, ordinary.status);
    assert!(
        access::group_holders(&mut *d.world.conn().await, d.outer)
            .await
            .unwrap()
            .iter()
            .any(|holder| holder.user_id == Some(stranger.id()))
    );
}

async fn guarded_change(narrow: bool) {
    let d = Desk::new().await;
    let writer = d
        .key(vec![rule("GroupUpdate", "group", d.outer, &["manage"])])
        .await;
    let before = stored(&d.world, d.outer).await;
    let mut blocker = d.world.db.pool.begin().await.unwrap();
    transaction_guard::lock_person(&mut blocker, d.boss.id())
        .await
        .unwrap()
        .unwrap();
    sqlx::query("SELECT id FROM api_tokens WHERE id=$1 AND user_id=$2 FOR UPDATE")
        .bind(writer.id)
        .bind(d.boss.id())
        .execute(&mut *blocker)
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let app = registry
        .into_router_for(orch_api_contract::surface::HttpSurface::Key)
        .with_state(d.world.app.app.clone());
    let request = Request::builder()
        .method("PATCH")
        .uri(format!("/groups/{}", d.outer))
        .header("authorization", format!("Bearer {}", writer.secret))
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"name":"Changed after fresh guard"}).to_string(),
        ))
        .unwrap();
    let action = tokio::spawn(async move { app.oneshot(request).await.unwrap() });
    // Existing bounded lock-state rendezvous; elapsed time never grants authority.
    let wait = std::time::Duration::from_secs(30);
    tokio::time::timeout(wait, async {
        loop {
            if action.is_finished() { break; }
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)))")
                .bind(pid).fetch_one(&d.world.db.pool).await.unwrap();
            if blocked { break; }
            tokio::task::yield_now().await;
        }
    }).await.unwrap();
    if narrow {
        sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1 AND user_id=$3")
            .bind(writer.id)
            .bind(serde_json::to_value(KeyScope::default()).unwrap())
            .bind(d.boss.id())
            .execute(&mut *blocker)
            .await
            .unwrap();
    }
    blocker.commit().await.unwrap();
    let response = tokio::time::timeout(wait, action).await.unwrap().unwrap();
    let status = response.status();
    let wire = to_bytes(response.into_body(), MAX_JSON_BYTES)
        .await
        .unwrap();
    if narrow {
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(stored(&d.world, d.outer).await, before);
    } else {
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&wire));
        assert_eq!(
            serde_json::from_slice::<Json>(&wire).unwrap(),
            json!({"projection":"receipt","group_id":d.outer})
        );
        assert_eq!(
            stored(&d.world, d.outer).await.0,
            "Changed after fresh guard"
        );
    }
}

#[tokio::test]
async fn group_update_rechecks_current_key_scope_at_the_existing_action_guard() {
    guarded_change(false).await;
    guarded_change(true).await;
}

#[tokio::test]
async fn an_expiring_response_authority_keeps_the_accepted_update_receipt_without_keeper_names() {
    for expires in [false, true] {
        let d = Desk::new().await;
        // The existing viewer receives one ordinary permission, without admin mode.
        access::testing::add_entry(
            &mut *d.world.conn().await,
            &access::PageKey::new("node", d.department, "live"),
            "any",
            "members.manage",
            ("user", Some(d.member.id()), None),
            expires.then_some(d.world.now() + Duration::seconds(1)),
            d.world.now(),
        )
        .await
        .unwrap();
        let writer = key(
            &d.world,
            d.member.id(),
            vec![rule("GroupUpdate", "group", d.outer, &["manage", "read"])],
        )
        .await;
        let mut blocker = d.world.db.pool.begin().await.unwrap();
        sqlx::query("SELECT id FROM groups WHERE id=$1 FOR UPDATE")
            .bind(d.outer)
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let mut registry = Registry::new();
        orch_api_core::routes::mount(&mut registry);
        let app = registry
            .into_router_for(orch_api_contract::surface::HttpSurface::Key)
            .with_state(d.world.app.app.clone());
        let request = Request::builder()
            .method("PATCH")
            .uri(format!("/groups/{}", d.outer))
            .header("authorization", format!("Bearer {}", writer.secret))
            .header("content-type", "application/json")
            .body(Body::from(
                json!({"name":"Accepted before response expiry"}).to_string(),
            ))
            .unwrap();
        let action = tokio::spawn(async move { app.oneshot(request).await.unwrap() });
        let wait = std::time::Duration::from_secs(30);
        let blocked = tokio::time::timeout(wait, async {
            loop {
                if action.is_finished() { return None; }
                let queries: Vec<String> = sqlx::query_scalar("SELECT query FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid))")
                    .bind(pid).fetch_all(&d.world.db.pool).await.unwrap();
                if let Some(query) = queries.into_iter().next() { return Some(query); }
                tokio::task::yield_now().await;
            }
        }).await.unwrap();
        // The writer checked MembersManage before this UPDATE; the response has not run.
        // Advance only the manual clock, leaving every identity/permission row intact.
        d.world.advance(Duration::seconds(2));
        blocker.commit().await.unwrap();
        let response = tokio::time::timeout(wait, action).await.unwrap().unwrap();
        assert!(
            blocked
                .as_deref()
                .is_some_and(|query| query.contains("UPDATE groups SET")),
            "the real update must reach its post-authorization row lock: {blocked:?}"
        );
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), MAX_JSON_BYTES)
            .await
            .unwrap();
        let value: Json = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            stored(&d.world, d.outer).await.0,
            "Accepted before response expiry"
        );
        if expires {
            assert_eq!(value, json!({"projection":"receipt","group_id":d.outer}));
        } else {
            assert!(value.get("projection").is_none());
            assert_eq!(value["name"], "Accepted before response expiry");
            assert!(value["place"].is_null());
        }
    }
}

async fn group_revision(world: &World, group: Id) -> i64 {
    sqlx::query_scalar("SELECT revision FROM groups WHERE id=$1")
        .bind(group)
        .fetch_one(&world.db.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn holder_put_guards_the_current_revision_and_keeps_legacy_no_op_and_removal_writes_visible()
{
    use orch_api_contract::directory::GroupHolderRemove;
    let d = Desk::new().await;
    let original = group_revision(&d.world, d.outer).await;
    let manager = d
        .key(vec![rule("GroupHolderAdd", "group", d.outer, &["manage"])])
        .await;
    let body = json!({"kind":"user","id":d.member.id(),"expected_revision":original});
    assert_eq!(
        add(&d.world, &manager.secret, d.outer, &body).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(group_revision(&d.world, d.outer).await, original + 1);
    let before = access::group_holders(&mut *d.world.conn().await, d.outer)
        .await
        .unwrap();
    let stale = add(&d.world, &manager.secret, d.outer, &body).await;
    assert_eq!(stale.status, StatusCode::CONFLICT);
    let problem: orch_api_contract::common::Problem = serde_json::from_slice(&stale.raw).unwrap();
    assert_eq!(problem.code, ErrorCode::ConflictStaleRevision);
    assert_eq!(
        problem.extra["current"],
        json!({"id":d.outer,"revision":original+1})
    );
    assert!(!String::from_utf8_lossy(&stale.raw).contains("Outer group sentinel"));
    assert_eq!(
        access::group_holders(&mut *d.world.conn().await, d.outer)
            .await
            .unwrap()
            .len(),
        before.len()
    );
    assert_eq!(group_revision(&d.world, d.outer).await, original + 1);
    // A duplicate accepted PUT is still a write, and the old flat body remains valid.
    assert_eq!(
        add(
            &d.world,
            &manager.secret,
            d.outer,
            &json!({"kind":"user","id":d.member.id()})
        )
        .await
        .status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(group_revision(&d.world, d.outer).await, original + 2);
    assert_eq!(
        d.world
            .app
            .call(GroupHolderRemove, &[&d.outer])
            .token(&d.boss.token)
            .body(&orch_api_contract::access::HolderInput::User { id: d.member.id() })
            .send()
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(group_revision(&d.world, d.outer).await, original + 3);
    let removed = add(
        &d.world,
        &manager.secret,
        d.outer,
        &json!({"kind":"user","id":d.member.id(),"expected_revision":original+2}),
    )
    .await;
    assert_eq!(
        removed.status,
        StatusCode::CONFLICT,
        "a stale add cannot silently reverse the later removal"
    );
    assert!(
        access::group_holders(&mut *d.world.conn().await, d.outer)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn simultaneous_holder_puts_from_one_revision_admit_only_one_and_expose_the_new_group_revision()
 {
    use orch_api_contract::directory::{Group, OrgGroupList, OrgGroupListQuery};
    let d = Desk::new().await;
    let revision = group_revision(&d.world, d.outer).await;
    let mut blocker = d.world.db.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM groups WHERE id=$1 FOR UPDATE")
        .bind(d.outer)
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let app = d.world.app.clone();
    let group = d.outer;
    let token = d.boss.token.clone();
    let first_body = json!({"kind":"user","id":d.member.id(),"expected_revision":revision});
    let first = tokio::spawn(async move {
        let call = app.call(GroupHolderAdd, &[&group]);
        call.token(&token).raw_body(&first_body).send_bytes().await
    });
    let app = d.world.app.clone();
    let token = d.boss.token.clone();
    let second_body = json!({"kind":"user","id":d.boss.id(),"expected_revision":revision});
    let second = tokio::spawn(async move {
        let call = app.call(GroupHolderAdd, &[&group]);
        call.token(&token).raw_body(&second_body).send_bytes().await
    });
    blocker.commit().await.unwrap();
    let (first, second) = (first.await.unwrap(), second.await.unwrap());
    let statuses = [first.status, second.status];
    assert_eq!(
        statuses
            .iter()
            .filter(|status| **status == StatusCode::NO_CONTENT)
            .count(),
        1
    );
    assert_eq!(
        statuses
            .iter()
            .filter(|status| **status == StatusCode::CONFLICT)
            .count(),
        1
    );
    assert_eq!(group_revision(&d.world, d.outer).await, revision + 1);
    let shown: Vec<Group> = d
        .world
        .app
        .call(OrgGroupList, &[&d.department])
        .token(&d.boss.token)
        .query(&OrgGroupListQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(
        shown
            .iter()
            .find(|group| group.id == d.outer)
            .unwrap()
            .revision,
        revision + 1
    );
}
