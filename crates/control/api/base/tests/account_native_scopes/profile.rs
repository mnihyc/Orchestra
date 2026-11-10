//! Native profile pins and places retain ordinary audience and same-operation source bounds.

use super::{checked, key};
use crate::{
    desk::Desk,
    places::{self, Who},
    support::World,
    usage_world::team,
};
use http::StatusCode;
use orch_api_contract::users::{
    MeProfilePinsSet, MeProfilePinsSetBody, MeProfileUpdate, MeProfileUpdateBody, ProfileAudience,
    ProfileGet, ProfilePinDocument, ProfilePinInput, ProfilePinNode,
};
use orch_types::{Id, Json};
use serde_json::json;

fn rule(operation: &str, kind: &str, id: Id) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"resources":[kind],"allow":[{"operation":operation,"actions":["read"]}]})
}
fn profile_rule(kind: &str, id: Id) -> Json {
    rule("ProfileGet", kind, id)
}
fn home(org: Id) -> Json {
    json!({"target":{"kind":"org","id":org,"reach":"subtree"},
        "resources":["org","project","topic","thread","document","document_container","provider","agent","environment","host","machine","template","mcp_server","mount","skill"],
        "allow":[{"operation":"ProfileGet","actions":["read"]}]})
}
async fn pins(world: &World, person: &Who, pins: Vec<ProfilePinInput>) {
    world
        .app
        .call(MeProfilePinsSet, &[])
        .token(&person.token)
        .body(&MeProfilePinsSetBody { pins })
        .send()
        .await
        .ok();
}

#[tokio::test]
async fn profile_account_read_keeps_identity_but_never_lends_pins_or_membership_names() {
    let world = World::new().await;
    let t = team(&world).await;
    pins(
        &world,
        &t.boss,
        vec![
            ProfilePinInput::Node(ProfilePinNode { node_id: t.acme }),
            ProfilePinInput::Node(ProfilePinNode {
                node_id: t.boss.person.personal_org,
            }),
        ],
    )
    .await;
    world
        .app
        .call(MeProfileUpdate, &[])
        .token(&t.boss.token)
        .body(&MeProfileUpdateBody {
            bio: Some("Public native biography".into()),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    let username = t.boss.person.username.as_str();
    for extra in [Vec::new(), vec![rule("NodeGet.read", "org", t.acme)]] {
        let token = key(
            &world,
            t.viewer.id(),
            std::iter::once(profile_rule("account", t.boss.id()))
                .chain(extra)
                .collect(),
        )
        .await;
        let body = checked(
            world
                .app
                .call(ProfileGet, &[&username])
                .token(&token)
                .send_bytes()
                .await,
            StatusCode::OK,
        );
        assert_eq!(body["user"]["id"], json!(t.boss.id()));
        assert_eq!(body["bio"], "Public native biography");
        assert_eq!(body["pins"], json!([]));
        assert_eq!(body["member_of"], json!([]));
        assert!(body["personal_org"].is_null());
    }
    let token = key(
        &world,
        t.viewer.id(),
        vec![profile_rule("account", t.boss.id()), home(t.acme)],
    )
    .await;
    let scoped = checked(
        world
            .app
            .call(ProfileGet, &[&username])
            .token(&token)
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    let native = world
        .app
        .call(ProfileGet, &[&username])
        .token(&t.viewer.token)
        .send()
        .await
        .ok();
    assert_eq!(scoped, serde_json::to_value(native).unwrap());
    assert_eq!(scoped["pins"].as_array().unwrap().len(), 1);
    assert_eq!(scoped["member_of"].as_array().unwrap().len(), 1);
    world
        .app
        .call(MeProfileUpdate, &[])
        .token(&t.boss.token)
        .body(&MeProfileUpdateBody {
            audience: Some(ProfileAudience::Private),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    places::admin_mode(&world, &t.admin, true).await;
    let native = world
        .app
        .call(ProfileGet, &[&username])
        .token(&t.admin.token)
        .send()
        .await
        .ok();
    assert!(native.inspected.is_some());
    let token = key(
        &world,
        t.admin.id(),
        vec![profile_rule("account", t.boss.id()), home(t.acme)],
    )
    .await;
    checked(
        world
            .app
            .call(ProfileGet, &[&username])
            .token(&token)
            .send_bytes()
            .await,
        StatusCode::NOT_FOUND,
    );
}

#[tokio::test]
async fn profile_node_pin_keeps_its_intrinsic_name_with_scope_bounded_ancestor_names() {
    let world = World::new().await;
    let t = team(&world).await;
    pins(
        &world,
        &t.boss,
        vec![ProfilePinInput::Node(ProfilePinNode { node_id: t.project })],
    )
    .await;
    let username = t.boss.person.username.as_str();
    let token = key(
        &world,
        t.viewer.id(),
        vec![
            profile_rule("account", t.boss.id()),
            profile_rule("project", t.project),
            rule("NodeGet.read", "org", t.acme),
        ],
    )
    .await;
    let body = checked(
        world
            .app
            .call(ProfileGet, &[&username])
            .token(&token)
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(body["pins"].as_array().unwrap().len(), 1);
    assert_eq!(body["pins"][0]["node"]["name"], "Billing");
    assert!(body["pins"][0]["node"]["path"][0]["name"].is_null());
    assert_eq!(body["pins"][0]["node"]["path"][0]["redacted"], true);
    assert_eq!(body["member_of"], json!([]));
    let stranger = key(
        &world,
        t.stranger.id(),
        vec![profile_rule("account", t.boss.id()), home(t.acme)],
    )
    .await;
    let hidden = checked(
        world
            .app
            .call(ProfileGet, &[&username])
            .token(&stranger)
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(hidden["pins"], json!([]));
    assert_eq!(hidden["member_of"], json!([]));
}

#[tokio::test]
async fn profile_document_pin_uses_its_actual_document_and_own_parent_name_sources() {
    let desk = Desk::new().await;
    let made = desk.make(&desk.ada, "Profile reference source").await;
    pins(
        &desk.world,
        &desk.ada,
        vec![ProfilePinInput::Document(ProfilePinDocument {
            document_id: made.base.id,
        })],
    )
    .await;
    let username = desk.ada.person.username.as_str();
    let token = key(
        &desk.world,
        desk.ada.id(),
        vec![
            profile_rule("account", desk.ada.id()),
            profile_rule("document", made.base.id),
            rule("NodeGet.read", "topic", desk.topic),
        ],
    )
    .await;
    let body = checked(
        desk.world
            .app
            .call(ProfileGet, &[&username])
            .token(&token)
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(body["pins"][0]["document"]["id"], json!(made.base.id));
    assert_eq!(
        body["pins"][0]["document"]["title"],
        "Profile reference source"
    );
    assert!(body["pins"][0]["document"]["node"]["name"].is_null());
    let token = key(
        &desk.world,
        desk.ada.id(),
        vec![
            profile_rule("account", desk.ada.id()),
            profile_rule("document", made.base.id),
            profile_rule("topic", desk.topic),
        ],
    )
    .await;
    let named = checked(
        desk.world
            .app
            .call(ProfileGet, &[&username])
            .token(&token)
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(named["pins"][0]["document"]["node"]["name"], "Plans");
    let wrong = key(
        &desk.world,
        desk.ada.id(),
        vec![
            profile_rule("account", desk.ada.id()),
            profile_rule("document", orch_types::new_id()),
            profile_rule("topic", desk.topic),
        ],
    )
    .await;
    let absent = checked(
        desk.world
            .app
            .call(ProfileGet, &[&username])
            .token(&wrong)
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(absent["pins"], json!([]));
}

#[tokio::test]
async fn profile_publication_keeps_native_privacy_after_its_actual_activity_wait() {
    let world = World::new().await;
    let t = team(&world).await;
    let username = t.boss.person.username.clone();
    let token = key(
        &world,
        t.viewer.id(),
        vec![profile_rule("account", t.boss.id())],
    )
    .await;
    let control = checked(
        world
            .app
            .call(ProfileGet, &[&username.as_str()])
            .token(&token)
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert!(control["local_time"].is_object());
    let mut blocker = world.db.pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let app = world.app.clone();
    let request = tokio::spawn(async move {
        let call = app.call(ProfileGet, &[&username.as_str()]).token(&token);
        call.send_bytes().await
    });
    let wait = std::time::Duration::from_secs(30);
    tokio::time::timeout(wait, async {
        loop {
            assert!(!request.is_finished(), "the native activity append must be blocked");
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                .bind(pid).fetch_one(&world.db.pool).await.unwrap();
            if blocked { break; } tokio::task::yield_now().await;
        }
    }).await.expect("the real profile publication wait must be observed");
    world
        .app
        .call(MeProfileUpdate, &[])
        .token(&t.boss.token)
        .body(&MeProfileUpdateBody {
            show_time: Some(false),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    blocker.commit().await.unwrap();
    let reply = tokio::time::timeout(wait, request).await.unwrap().unwrap();
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    assert!(!String::from_utf8_lossy(&reply.raw).contains("utc_offset_minutes"));
}

#[tokio::test]
async fn profile_publication_cannot_borrow_a_new_pin_or_its_path_for_the_old_pin() {
    let world = World::new().await;
    let t = team(&world).await;
    pins(
        &world,
        &t.boss,
        vec![ProfilePinInput::Node(ProfilePinNode { node_id: t.project })],
    )
    .await;
    let username = t.boss.person.username.clone();
    let token = key(
        &world,
        t.viewer.id(),
        vec![profile_rule("account", t.boss.id()), home(t.acme)],
    )
    .await;
    let control = checked(
        world
            .app
            .call(ProfileGet, &[&username.as_str()])
            .token(&token)
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(control["pins"][0]["node"]["id"], json!(t.project));
    let mut blocker = world.db.pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let app = world.app.clone();
    let request = tokio::spawn(async move {
        let call = app.call(ProfileGet, &[&username.as_str()]).token(&token);
        call.send_bytes().await
    });
    let wait = std::time::Duration::from_secs(30);
    tokio::time::timeout(wait, async {
        loop {
            assert!(!request.is_finished(), "the actual profile activity append must be blocked");
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                .bind(pid).fetch_one(&world.db.pool).await.unwrap();
            if blocked { break; } tokio::task::yield_now().await;
        }
    }).await.expect("the native profile publication wait must be observed");
    pins(
        &world,
        &t.boss,
        vec![ProfilePinInput::Node(ProfilePinNode { node_id: t.thread })],
    )
    .await;
    let current = world
        .app
        .call(ProfileGet, &[&t.boss.person.username.as_str()])
        .token(&t.viewer.token)
        .send()
        .await
        .ok();
    assert_eq!(current.pins.len(), 1);
    assert!(
        matches!(&current.pins[0].node, Some(orch_api_contract::nodes::Node::Thread(thread)) if thread.base.id == t.thread)
    );
    blocker.commit().await.unwrap();
    let reply = tokio::time::timeout(wait, request).await.unwrap().unwrap();
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    assert!(!String::from_utf8_lossy(&reply.raw).contains("\"pins\""));
}
