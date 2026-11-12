//! Stored Machine renames keep their native governor, safe receipts and original registration sources.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod places;
mod support;

use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::resources::{MachineList, MachineUpdate, MachineUpdateBody};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_db::queries::{
    identity::{self, NewApiToken},
    resources::access_fixtures as fixtures,
};
use orch_types::{Id, Json, new_id};
use serde_json::json;
use support::World;

fn rule(operation: &str, kind: &str, id: Id, actions: &[&str]) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"allow":[{"operation":operation,"actions":actions}]})
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
            name: "Stored native Machine rename",
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
async fn rename(world: &World, token: &str, machine: Id, name: &str) -> RawReply {
    let request = world
        .app
        .call(MachineUpdate, &[&machine])
        .token(token)
        .body(&MachineUpdateBody { name: name.into() });
    request.send_bytes().await
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
async fn stored(world: &World, machine: Id) -> (String, String) {
    sqlx::query_as("SELECT name,xmin::text FROM machines WHERE id=$1")
        .bind(machine)
        .fetch_one(&world.db.pool)
        .await
        .unwrap()
}
async fn activity_wait(world: &World, pid: i32, request: &tokio::task::JoinHandle<RawReply>) {
    tokio::time::timeout(std::time::Duration::from_secs(30),async {
        loop {
            assert!(!request.is_finished(),"the committed Machine response did not reach activity bookkeeping");
            let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database()
                AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                .bind(pid).fetch_one(&world.db.pool).await.unwrap();
            if blocked {break;}
            tokio::task::yield_now().await;
        }
    }).await.expect("the completed native response must wait at its actual activity append");
}

#[tokio::test]
async fn machine_rename_keeps_manage_only_effects_and_requires_its_own_read_for_native_metadata() {
    let world = World::new().await;
    let owner = places::person(&world, "owner").await;
    let machine = fixtures::insert_persons_machine(
        &mut *world.conn().await,
        owner.id(),
        "Private old Machine name",
    )
    .await
    .unwrap();
    let writer = key(
        &world,
        owner.id(),
        vec![
            rule("MachineUpdate", "machine", machine, &["manage"]),
            rule("MachineGet", "machine", machine, &["read"]),
        ],
    )
    .await;
    assert_eq!(
        accepted(rename(&world, &writer, machine, " Write-only native rename ").await),
        json!({"projection":"receipt","machine_id":machine})
    );
    assert_eq!(stored(&world, machine).await.0, "Write-only native rename");
    let full = key(
        &world,
        owner.id(),
        vec![rule(
            "MachineUpdate",
            "machine",
            machine,
            &["manage", "read"],
        )],
    )
    .await;
    let body = accepted(rename(&world, &full, machine, "Full native rename").await);
    assert_eq!(body["id"], json!(machine));
    assert_eq!(body["name"], "Full native rename");
    let before = stored(&world, machine).await;
    let wrong = key(
        &world,
        owner.id(),
        vec![rule(
            "MachineUpdate",
            "environment",
            machine,
            &["manage", "read"],
        )],
    )
    .await;
    assert_eq!(
        rename(&world, &wrong, machine, "Must be refused")
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(stored(&world, machine).await, before);
    let ordinary = accepted(rename(&world, &owner.token, machine, "Ordinary native rename").await);
    assert_eq!(ordinary["name"], "Ordinary native rename");
}

#[tokio::test]
async fn machine_rename_full_readback_binds_the_actual_registered_provider_without_removing_the_effect()
 {
    let world = World::new().await;
    let owner = places::person(&world, "owner").await;
    let other = places::person(&world, "other").await;
    let machine = fixtures::insert_persons_machine(
        &mut *world.conn().await,
        owner.id(),
        "Native registration carrier",
    )
    .await
    .unwrap();
    let mut conn = world.conn().await;
    let provider = fixtures::insert_resource(
        &mut conn,
        "provider",
        Some(other.id()),
        Some(other.person.personal_org),
        "private-registration",
    )
    .await
    .unwrap();
    fixtures::register_on(&mut conn, "provider", provider, machine)
        .await
        .unwrap();
    drop(conn);
    let primary = rule("MachineUpdate", "machine", machine, &["manage", "read"]);
    let missing = key(
        &world,
        owner.id(),
        vec![
            primary.clone(),
            rule("MachineGet", "provider", provider, &["read"]),
        ],
    )
    .await;
    assert_eq!(
        accepted(rename(&world, &missing, machine, "Effect without borrowed content").await),
        json!({"projection":"receipt","machine_id":machine})
    );
    assert_eq!(
        stored(&world, machine).await.0,
        "Effect without borrowed content"
    );
    let complete = key(
        &world,
        owner.id(),
        vec![
            primary,
            rule("MachineUpdate", "provider", provider, &["read"]),
        ],
    )
    .await;
    let body = accepted(
        rename(
            &world,
            &complete,
            machine,
            "Exact native reference readback",
        )
        .await,
    );
    assert_eq!(body["providers"][0]["id"], json!(provider));
    assert_eq!(body["providers"][0]["name"], "private-registration");
}

#[tokio::test]
async fn machine_rename_rechecks_the_real_key_after_its_actual_physical_row_wait() {
    for expire in [false, true] {
        let world = World::new().await;
        let owner = places::person(&world, "owner").await;
        let machine = fixtures::insert_persons_machine(
            &mut *world.conn().await,
            owner.id(),
            "Original target name",
        )
        .await
        .unwrap();
        let token = key(
            &world,
            owner.id(),
            vec![rule("MachineUpdate", "machine", machine, &["manage"])],
        )
        .await;
        let mut blocker = world.db.pool.begin().await.unwrap();
        sqlx::query("SELECT id FROM machines WHERE id=$1 FOR UPDATE")
            .bind(machine)
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = world.app.clone();
        let request = tokio::spawn(async move {
            let request =
                app.call(MachineUpdate, &[&machine])
                    .token(&token)
                    .body(&MachineUpdateBody {
                        name: "After the native target wait".into(),
                    });
            request.send_bytes().await
        });
        tokio::time::timeout(std::time::Duration::from_secs(30),async {
            loop {
                assert!(!request.is_finished(),"the native physical target lock was not reached");
                let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database()
                    AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%FROM machines%' AND query LIKE '%FOR %')")
                    .bind(pid).fetch_one(&world.db.pool).await.unwrap();
                if blocked {break;}
                tokio::task::yield_now().await;
            }
        }).await.expect("the real native Machine target query must actually wait");
        if expire {
            world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request)
            .await
            .unwrap()
            .unwrap();
        if expire {
            assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
            assert_eq!(stored(&world, machine).await.0, "Original target name");
        } else {
            assert_eq!(
                accepted(reply),
                json!({"projection":"receipt","machine_id":machine})
            );
            assert_eq!(
                stored(&world, machine).await.0,
                "After the native target wait"
            );
        }
    }
}

#[tokio::test]
async fn machine_rename_publication_keeps_the_actual_registration_association_and_writer_xmin() {
    for move_registration in [false, true] {
        let world = World::new().await;
        let owner = places::person(&world, "owner").await;
        let other = places::person(&world, "other").await;
        let machine = fixtures::insert_persons_machine(
            &mut *world.conn().await,
            owner.id(),
            "Original machine",
        )
        .await
        .unwrap();
        let elsewhere = fixtures::insert_persons_machine(
            &mut *world.conn().await,
            other.id(),
            "Other registration carrier",
        )
        .await
        .unwrap();
        let mut conn = world.conn().await;
        let provider = fixtures::insert_resource(
            &mut conn,
            "provider",
            Some(other.id()),
            Some(other.person.personal_org),
            "held-private-registration",
        )
        .await
        .unwrap();
        fixtures::register_on(&mut conn, "provider", provider, machine)
            .await
            .unwrap();
        drop(conn);
        let token = key(
            &world,
            owner.id(),
            vec![
                rule("MachineUpdate", "machine", machine, &["manage", "read"]),
                rule("MachineUpdate", "provider", provider, &["read"]),
            ],
        )
        .await;
        let control = accepted(rename(&world, &token, machine, "Allowed control").await);
        assert_eq!(control["providers"][0]["id"], json!(provider));
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
            let request =
                app.call(MachineUpdate, &[&machine])
                    .token(&token)
                    .body(&MachineUpdateBody {
                        name: "Committed native Machine label".into(),
                    });
            request.send_bytes().await
        });
        activity_wait(&world, pid, &request).await;
        let before = stored(&world, machine).await;
        assert_eq!(before.0, "Committed native Machine label");
        if move_registration {
            fixtures::register_on(&mut *world.conn().await, "provider", provider, elsewhere)
                .await
                .unwrap();
        }
        blocker.commit().await.unwrap();
        let body = accepted(
            tokio::time::timeout(std::time::Duration::from_secs(30), request)
                .await
                .unwrap()
                .unwrap(),
        );
        if move_registration {
            assert_eq!(body, json!({"projection":"receipt","machine_id":machine}));
        } else {
            assert_eq!(body["providers"][0]["name"], "held-private-registration");
        }
        assert_eq!(
            stored(&world, machine).await,
            before,
            "publication never reruns the completed rename"
        );
        let current = {
            let request = world.app.call(MachineList, &[]).token(&owner.token);
            request.send().await
        }
        .ok();
        let current = current.iter().find(|row| row.base.id == machine).unwrap();
        assert_eq!(
            current.providers.is_empty(),
            move_registration,
            "the original native registration association actually changed"
        );
    }
}
