//! Native physical Machine inventory, facade governors and actual registered reference sources.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod places;
mod support;

use http::StatusCode;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::KeyScope;
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::resources::{Machine, MachineList};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::queries::{
    identity::{self, NewApiToken},
    resources::access_fixtures as fixtures,
};
use orch_types::{Id, Json, Time, new_id};
use serde_json::json;
use support::World;

struct Key {
    id: Id,
    secret: String,
}
fn rule(operation: &str, kind: &str, id: Id) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"allow":[{"operation":operation,"actions":["read"]}]})
}
async fn key(world: &World, user: Id, rules: Vec<Json>) -> Key {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let key = Key {
        id: new_id(),
        secret: new_token(API_TOKEN_PREFIX),
    };
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &NewApiToken {
            id: key.id,
            user_id: user,
            name: "Native Machine inventory scope",
            token_hash: &token_hash(&key.secret),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        Some(&serde_json::to_value(scope).unwrap()),
    )
    .await
    .unwrap();
    key
}
async fn list(world: &World, token: &str) -> RawReply {
    world
        .app
        .call(MachineList, &[])
        .token(token)
        .send_bytes()
        .await
}
fn shown(reply: RawReply) -> Vec<Machine> {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
async fn owned(world: &World, owner: &places::Who, name: &str) -> Id {
    fixtures::insert_persons_machine(&mut *world.conn().await, owner.id(), name)
        .await
        .unwrap()
}
async fn activity_wait(world: &World, pid: i32, request: &tokio::task::JoinHandle<RawReply>) {
    tokio::time::timeout(std::time::Duration::from_secs(30),async {
        loop {
            assert!(!request.is_finished(),"the inventory did not reach activity bookkeeping");
            let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database()
                AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                .bind(pid).fetch_one(&world.db.pool).await.unwrap();
            if blocked {break;}
            tokio::task::yield_now().await;
        }
    }).await.expect("the actual native response must wait at its activity append");
}

#[tokio::test]
async fn machine_list_binds_typed_selected_rows_and_keeps_ordinary_visibility_and_current_key() {
    let world = World::new().await;
    let owner = places::person(&world, "owner").await;
    let other = places::person(&world, "other").await;
    let machine = owned(&world, &owner, "Native selected machine").await;
    owned(&world, &other, "Other private machine").await;
    let ordinary = shown(list(&world, &owner.token).await);
    assert_eq!(ordinary.len(), 1);
    let selected = key(
        &world,
        owner.id(),
        vec![rule("MachineList", "machine", machine)],
    )
    .await;
    let body = shown(list(&world, &selected.secret).await);
    assert_eq!(
        serde_json::to_value(&body).unwrap(),
        serde_json::to_value(&ordinary).unwrap()
    );
    let wrong = key(
        &world,
        owner.id(),
        vec![rule("MachineList", "environment", machine)],
    )
    .await;
    assert!(shown(list(&world, &wrong.secret).await).is_empty());
    let unviewed = key(
        &world,
        other.id(),
        vec![rule("MachineList", "machine", machine)],
    )
    .await;
    assert!(shown(list(&world, &unviewed.secret).await).is_empty());
    let different = key(
        &world,
        owner.id(),
        vec![rule("MachineGet", "machine", machine)],
    )
    .await;
    assert_eq!(
        list(&world, &different.secret).await.status,
        StatusCode::FORBIDDEN
    );
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(selected.id)
        .bind(serde_json::to_value(KeyScope::default()).unwrap())
        .execute(&world.db.pool)
        .await
        .unwrap();
    let denied = list(&world, &selected.secret).await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN);
    assert!(!String::from_utf8_lossy(&denied.raw).contains("Native selected machine"));
}

#[tokio::test]
async fn machine_list_registered_references_need_read_from_the_same_operation() {
    let world = World::new().await;
    let owner = places::person(&world, "owner").await;
    let machine = owned(&world, &owner, "Registered native machine").await;
    let mut sources = Vec::new();
    let mut conn = world.conn().await;
    for kind in ["provider", "agent", "environment"] {
        let id = fixtures::insert_resource(
            &mut conn,
            kind,
            Some(owner.id()),
            Some(owner.person.personal_org),
            &format!("registered-{kind}"),
        )
        .await
        .unwrap();
        fixtures::register_on(&mut conn, kind, id, machine)
            .await
            .unwrap();
        sources.push((kind, id));
    }
    drop(conn);
    let ordinary = shown(list(&world, &owner.token).await);
    assert_eq!(ordinary.len(), 1);
    assert_eq!(
        (
            ordinary[0].providers.len(),
            ordinary[0].agents.len(),
            ordinary[0].environments.len()
        ),
        (1, 1, 1)
    );
    let mut own = vec![rule("MachineList", "machine", machine)];
    own.extend(
        sources
            .iter()
            .map(|(kind, id)| rule("MachineList", kind, *id)),
    );
    let complete = key(&world, owner.id(), own).await;
    let body = shown(list(&world, &complete.secret).await);
    assert_eq!(body[0].base.id, machine);
    assert_eq!(body[0].providers[0].name, "registered-provider");
    assert_eq!(body[0].agents[0].name, "registered-agent");
    assert_eq!(body[0].environments[0].name, "registered-environment");
    let mut borrowed = vec![rule("MachineList", "machine", machine)];
    borrowed.extend(
        sources
            .iter()
            .map(|(kind, id)| rule("MachineGet", kind, *id)),
    );
    let borrowed = key(&world, owner.id(), borrowed).await;
    let denied = list(&world, &borrowed.secret).await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN);
    assert!(!String::from_utf8_lossy(&denied.raw).contains("registered-provider"));
}

async fn facade(world: &World, owner: &places::Who) -> (Id, Id) {
    let mut conn = world.conn().await;
    let host = fixtures::insert_resource(
        &mut conn,
        "host",
        Some(owner.id()),
        Some(owner.person.personal_org),
        "native-host",
    )
    .await
    .unwrap();
    start_resource(
        &mut conn,
        ObjectRef {
            kind: ObjectKind::Host,
            id: host,
        },
        owner.id(),
        NewAccess::OnlyMe,
        Time::new(world.now()),
    )
    .await
    .unwrap();
    let machine: Id = sqlx::query_scalar("SELECT machine_id FROM hosts WHERE id=$1")
        .bind(host)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    (host, machine)
}

#[tokio::test]
async fn machine_list_keeps_machine_business_kind_and_the_actual_host_facade_governor() {
    let world = World::new().await;
    let owner = places::person(&world, "owner").await;
    let (host, machine) = facade(&world, &owner).await;
    let ordinary = shown(list(&world, &owner.token).await);
    assert_eq!(ordinary.len(), 1);
    assert_eq!(ordinary[0].base.id, machine);
    let actual = key(&world, owner.id(), vec![rule("MachineList", "host", host)]).await;
    let rows = shown(list(&world, &actual.secret).await);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].base.id, machine);
    assert_eq!(rows[0].host.as_ref().unwrap().id, host);
    let path = key(
        &world,
        owner.id(),
        vec![rule("MachineList", "machine", machine)],
    )
    .await;
    assert!(shown(list(&world, &path.secret).await).is_empty());
    let family = key(
        &world,
        owner.id(),
        vec![json!({"target":{"kind":"host","id":host,"reach":"self"},
        "allow":[{"operation":"MachineList","actions":["read"],"resources":["host"]}]})],
    )
    .await;
    assert_eq!(
        list(&world, &family.secret).await.status,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn machine_list_publication_keeps_the_actual_physical_machine_when_its_facade_selects_a_replacement()
 {
    let world = World::new().await;
    let owner = places::person(&world, "owner").await;
    let (host, original) = facade(&world, &owner).await;
    let replacement = owned(&world, &owner, "Replacement physical source").await;
    let key = key(&world, owner.id(), vec![rule("MachineList", "host", host)]).await;
    let control = shown(list(&world, &key.secret).await);
    assert_eq!(control.len(), 1);
    assert_eq!(control[0].base.id, original);
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
    let secret = key.secret.clone();
    let request = tokio::spawn(async move {
        let request = app.call(MachineList, &[]).token(&secret);
        request.send_bytes().await
    });
    activity_wait(&world, pid, &request).await;
    sqlx::query("UPDATE hosts SET machine_id=$2 WHERE id=$1")
        .bind(host)
        .bind(replacement)
        .execute(&world.db.pool)
        .await
        .unwrap();
    blocker.commit().await.unwrap();
    let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    assert!(!String::from_utf8_lossy(&reply.raw).contains("native-host"));
    assert!(!String::from_utf8_lossy(&reply.raw).contains("Replacement physical source"));
    let current = shown(list(&world, &key.secret).await);
    assert_eq!(current.len(), 1);
    assert_eq!(current[0].base.id, replacement);
    assert_eq!(current[0].host.as_ref().unwrap().id, host);
}
