//! Native Mount creation keeps real home policy and immutable completed outcomes.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]
mod places;
mod resources;
mod support;
use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::access::NewResourceAccess;
use orch_api_contract::mounts::{MountCreate, MountCreateBody, MountFate};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_db::queries::{
    identity::{self, NewApiToken},
    resources::mounts as rows,
};
use orch_types::{Id, Json, new_id};
use serde_json::json;
use support::World;
fn rule(operation: &str, kind: &str, id: Option<Id>, reach: &str, actions: &[&str]) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":reach},"allow":[{"operation":operation,"actions":actions}]})
}
async fn key(world: &World, user: Id, rules: Vec<Json>) -> String {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &NewApiToken {
            id: new_id(),
            user_id: user,
            name: "Native mount creation",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        Some(&serde_json::to_value(scope).unwrap()),
    )
    .await
    .unwrap();
    secret
}
fn body(home: Option<Id>, name: &str) -> MountCreateBody {
    MountCreateBody {
        home_id: home,
        name: name.into(),
        description: Some("Stored native data description".into()),
        target: "/mnt/project-data".into(),
        read_only: Some(true),
        default_fate: Some(MountFate::AsIs),
        access: Some(NewResourceAccess::OnlyMe),
    }
}
async fn create(world: &World, token: &str, body: &MountCreateBody) -> RawReply {
    world
        .app
        .call(MountCreate, &[])
        .token(token)
        .body(body)
        .send_bytes()
        .await
}
fn accepted(reply: RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
async fn saved(world: &World, name: &str) -> Option<(Id, Option<Id>, Id, String)> {
    sqlx::query_as("SELECT id,home_id,owner_id,xmin::text FROM mounts WHERE name=$1")
        .bind(name)
        .fetch_optional(&world.db.pool)
        .await
        .unwrap()
}
#[tokio::test]
async fn mount_creation_manage_only_keeps_its_native_definition_without_borrowing_get_read() {
    let world = World::new().await;
    let c = resources::company(&world).await;
    let token = key(
        &world,
        c.boss.id(),
        vec![
            rule("MountCreate", "org", Some(c.acme), "self", &["manage"]),
            rule("MountGet", "org", Some(c.acme), "subtree", &["read"]),
        ],
    )
    .await;
    let reply = accepted(create(&world, &token, &body(Some(c.acme), "manage-only-data")).await);
    let row = saved(&world, "manage-only-data").await.unwrap();
    assert_eq!(reply, json!({"projection":"receipt","mount_id":row.0}));
    assert_eq!((row.1, row.2), (Some(c.acme), c.boss.id()));
    let definition: (String, bool, String) =
        sqlx::query_as("SELECT target,read_only,default_fate FROM mounts WHERE id=$1")
            .bind(row.0)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(
        definition,
        ("/mnt/project-data".into(), true, "as_is".into())
    );
    let locations: i64 =
        sqlx::query_scalar("SELECT count(*) FROM mount_locations WHERE mount_id=$1")
            .bind(row.0)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(
        locations, 0,
        "ordinary creation runs no location supplier or command"
    );
}
#[tokio::test]
async fn mount_creation_own_subtree_read_keeps_current_member_policy_and_actual_kind() {
    let world = World::new().await;
    let c = resources::company(&world).await;
    let token = key(
        &world,
        c.viewer.id(),
        vec![rule(
            "MountCreate",
            "org",
            Some(c.acme),
            "subtree",
            &["manage", "read"],
        )],
    )
    .await;
    let full = accepted(create(&world, &token, &body(Some(c.acme), "member-data")).await);
    assert!(full.get("projection").is_none());
    assert_eq!(full["name"], "member-data");
    assert_eq!(full["target"], "/mnt/project-data");
    assert_eq!(full["locations"], json!([]));
    assert!(full.to_string().contains("Stored native data description"));
    let outside = key(
        &world,
        c.stranger.id(),
        vec![rule(
            "MountCreate",
            "org",
            Some(c.acme),
            "subtree",
            &["manage", "read"],
        )],
    )
    .await;
    assert_eq!(
        create(&world, &outside, &body(Some(c.acme), "outside-data"))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    let wrong = key(
        &world,
        c.boss.id(),
        vec![rule(
            "MountCreate",
            "mount",
            Some(c.acme),
            "self",
            &["manage", "read"],
        )],
    )
    .await;
    assert_eq!(
        create(&world, &wrong, &body(Some(c.acme), "wrong-kind-data"))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    world
        .set_at(
            orch_settings::Target::Node(c.acme),
            "resources.created_by_members",
            json!([]),
        )
        .await;
    assert_eq!(
        create(&world, &token, &body(Some(c.acme), "policy-denied-data"))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert!(saved(&world, "outside-data").await.is_none());
    assert!(saved(&world, "wrong-kind-data").await.is_none());
    assert!(saved(&world, "policy-denied-data").await.is_none());
}
#[tokio::test]
async fn mount_system_creation_needs_the_actual_system_scope_and_current_admin_standing() {
    let world = World::new().await;
    let c = resources::company(&world).await;
    let system = || rule("MountCreate", "system", None, "self", &["manage", "read"]);
    let token = key(&world, c.boss.id(), vec![system()]).await;
    assert_eq!(
        create(&world, &token, &body(None, "no-standing-data"))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    sqlx::query("UPDATE users SET is_system_admin=true WHERE id=$1")
        .bind(c.boss.id())
        .execute(&world.db.pool)
        .await
        .unwrap();
    let org = key(
        &world,
        c.boss.id(),
        vec![rule(
            "MountCreate",
            "org",
            Some(c.acme),
            "subtree",
            &["manage", "read"],
        )],
    )
    .await;
    assert_eq!(
        create(&world, &org, &body(None, "org-scope-system-data"))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    let reply = accepted(create(&world, &token, &body(None, "actual-system-data")).await);
    let row = saved(&world, "actual-system-data").await.unwrap();
    assert_eq!(row.1, None);
    assert_eq!(reply, json!({"projection":"receipt","mount_id":row.0}));
    assert!(saved(&world, "org-scope-system-data").await.is_none());
}
#[tokio::test]
async fn mount_creation_rechecks_expiry_after_the_native_root_and_name_footprint() {
    for expire in [false, true] {
        let world = World::new().await;
        let c = resources::company(&world).await;
        let token = key(
            &world,
            c.boss.id(),
            vec![rule(
                "MountCreate",
                "org",
                Some(c.acme),
                "self",
                &["manage"],
            )],
        )
        .await;
        let mut blocker = world.db.pool.begin().await.unwrap();
        rows::lock_creation_name(&mut blocker, Some(c.acme), "waiting-data")
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = world.app.clone();
        let home = c.acme;
        let request = tokio::spawn(async move {
            let request = app
                .call(MountCreate, &[])
                .token(&token)
                .body(&body(Some(home), "waiting-data"));
            request.send_bytes().await
        });
        let wait = std::time::Duration::from_secs(30);
        tokio::time::timeout(wait,async {
            loop {
                assert!(!request.is_finished(),"the native root/name footprint was not reached");
                let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database()
                    AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%SELECT pg_advisory_xact_lock(hashtextextended(%')")
                    .bind(pid).fetch_one(&world.db.pool).await.unwrap();
                if blocked {break;}
                tokio::task::yield_now().await;
            }
        }).await.expect("the real native Mount root/name namespace must wait");
        assert!(saved(&world, "waiting-data").await.is_none());
        if expire {
            world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(wait, request).await.unwrap().unwrap();
        if expire {
            assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
            assert!(saved(&world, "waiting-data").await.is_none());
        } else {
            let row = saved(&world, "waiting-data").await.unwrap();
            assert_eq!(
                accepted(reply),
                json!({"projection":"receipt","mount_id":row.0})
            );
        }
    }
}
#[tokio::test]
async fn mount_creation_publication_keeps_committed_identity_definition_and_writer_xmin() {
    for expire in [false, true] {
        let world = World::new().await;
        let c = resources::company(&world).await;
        let token = key(
            &world,
            c.boss.id(),
            vec![rule(
                "MountCreate",
                "org",
                Some(c.acme),
                "subtree",
                &["manage", "read"],
            )],
        )
        .await;
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
        let home = c.acme;
        let request = tokio::spawn(async move {
            let request = app
                .call(MountCreate, &[])
                .token(&token)
                .body(&body(Some(home), "held-native-data"));
            request.send_bytes().await
        });
        let wait = std::time::Duration::from_secs(30);
        tokio::time::timeout(wait,async {
            loop {
                assert!(!request.is_finished(),"the completed native Mount did not reach bookkeeping");
                let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database()
                    AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                    .bind(pid).fetch_one(&world.db.pool).await.unwrap();
                if blocked {break;}
                tokio::task::yield_now().await;
            }
        }).await.expect("the actual completed Mount response must wait at activity append");
        let before = saved(&world, "held-native-data").await.unwrap();
        if expire {
            world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let reply = accepted(tokio::time::timeout(wait, request).await.unwrap().unwrap());
        if expire {
            assert_eq!(reply, json!({"projection":"receipt","mount_id":before.0}));
        } else {
            assert!(reply.get("projection").is_none());
            assert_eq!(reply["name"], "held-native-data");
            assert_eq!(reply["target"], "/mnt/project-data");
        }
        assert_eq!(saved(&world, "held-native-data").await.unwrap(), before);
        let audits: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='mount.created'",
        )
        .bind(before.0)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
        assert_eq!(audits, 1);
    }
}
