//! Native physical subjects, selected storage references and current exposure associations.

use super::*;
use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_access_contract::permissions::ResourceKind;
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_db::queries::identity::{self, NewApiToken};
use orch_db::queries::resources::{access_fixtures as fixtures, mounts as rows};
use orch_types::{Json, new_id};

fn rule(operation: &str, kind: &str, id: Id) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"allow":[{"operation":operation,"actions":["read"]}]})
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
            name: "Native machine snapshot scopes",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        Some(&json!(scope)),
    )
    .await
    .unwrap();
    secret
}
async fn mounts(world: &World, token: &str, machine: Id) -> RawReply {
    let request = world.app.call(MachineMountList, &[&machine]).token(token);
    request.send_bytes().await
}
async fn data(world: &World, token: &str, machine: Id) -> RawReply {
    let request = world
        .app
        .call(MachineEnvironmentDataGet, &[&machine])
        .token(token);
    request.send_bytes().await
}
fn accepted<T: serde::de::DeserializeOwned>(reply: RawReply) -> T {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
async fn machine(world: &World, owner: Id) -> Id {
    let id = fixtures::insert_persons_machine(&mut *world.conn().await, owner, "Snapshot machine")
        .await
        .unwrap();
    sqlx::query("UPDATE machines SET facts=$2 WHERE id=$1").bind(id).bind(json!({"policy":{"environment.roots":["/srv/work"]},"environment_descriptor":{"lifecycle":"attach","data_storage":{"made_by_orch":"directory","existing_directory":true,"reason":null}},"storages":[{"name":"fast","default":true}]})).execute(&world.db.pool).await.unwrap();
    id
}
async fn host(world: &World, owner: Id, home: Id) -> (Id, Id) {
    let id = fixtures::insert_resource(
        &mut *world.conn().await,
        "host",
        Some(owner),
        Some(home),
        "Snapshot host",
    )
    .await
    .unwrap();
    own_page(world, ResourceKind::Host, id, owner).await;
    let machine = sqlx::query_scalar("SELECT machine_id FROM hosts WHERE id=$1")
        .bind(id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    (id, machine)
}
async fn location(world: &World, machine: Id, mount: Id, owner: Id, source: &str) {
    rows::set_location(
        &mut *world.conn().await,
        &rows::LocationRow {
            mount_id: mount,
            machine_id: machine,
            host_id: None,
            host_name: None,
            kind: "path".into(),
            source: source.into(),
            storage: None,
            data_kind: None,
            created_by: owner,
            created_at: world.now(),
        },
    )
    .await
    .unwrap();
}
async fn environment(
    world: &World,
    machine: Id,
    owner: Id,
    home: Id,
    name: &str,
    path: &str,
) -> Id {
    let id = fixtures::insert_environment(
        &mut *world.conn().await,
        owner,
        Some(home),
        name,
        "connected",
    )
    .await
    .unwrap();
    sqlx::query("UPDATE environments SET machine_id=$2,workspace_root=$3 WHERE id=$1")
        .bind(id)
        .bind(machine)
        .bind(path)
        .execute(&world.db.pool)
        .await
        .unwrap();
    own_page(world, ResourceKind::Environment, id, owner).await;
    id
}

#[tokio::test]
async fn machine_snapshots_require_the_actual_physical_subject_and_keep_real_empty_reference_proofs()
 {
    let world = World::new().await;
    let c = company(&world).await;
    let machine = machine(&world, c.viewer.id()).await;
    for operation in ["MachineMountList", "MachineEnvironmentDataGet"] {
        let token = key(
            &world,
            c.viewer.id(),
            vec![rule(operation, "machine", machine)],
        )
        .await;
        if operation == "MachineMountList" {
            let value: MachineMounts = accepted(mounts(&world, &token, machine).await);
            assert!(value.locations.is_empty());
            assert!(value.available.is_empty());
        } else {
            let value: MachineEnvironmentData = accepted(data(&world, &token, machine).await);
            assert_eq!(value.roots[0].path, "/srv/work");
            assert!(value.roots[0].uses.is_empty());
        }
        let wrong = key(
            &world,
            c.viewer.id(),
            vec![rule(operation, "environment", machine)],
        )
        .await;
        let reply = if operation == "MachineMountList" {
            mounts(&world, &wrong, machine).await
        } else {
            data(&world, &wrong, machine).await
        };
        assert_eq!(reply.status, StatusCode::FORBIDDEN);
        let other = key(
            &world,
            c.admin.id(),
            vec![rule(operation, "machine", machine)],
        )
        .await;
        let reply = if operation == "MachineMountList" {
            mounts(&world, &other, machine).await
        } else {
            data(&world, &other, machine).await
        };
        assert_eq!(reply.status, StatusCode::NOT_FOUND);
    }
}

#[tokio::test]
async fn machine_snapshot_host_facade_is_not_a_same_id_machine_or_an_unrelated_operation() {
    let world = World::new().await;
    let c = company(&world).await;
    let (host, machine) = host(&world, c.viewer.id(), c.acme).await;
    for operation in ["MachineMountList", "MachineEnvironmentDataGet"] {
        let token = key(&world, c.viewer.id(), vec![rule(operation, "host", host)]).await;
        let reply = if operation == "MachineMountList" {
            mounts(&world, &token, machine).await
        } else {
            data(&world, &token, machine).await
        };
        assert_eq!(
            reply.status,
            StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&reply.raw)
        );
        for row in [
            rule(operation, "machine", machine),
            rule(
                if operation == "MachineMountList" {
                    "MachineEnvironmentDataGet"
                } else {
                    "MachineMountList"
                },
                "host",
                host,
            ),
        ] {
            let token = key(&world, c.viewer.id(), vec![row]).await;
            let reply = if operation == "MachineMountList" {
                mounts(&world, &token, machine).await
            } else {
                data(&world, &token, machine).await
            };
            assert_eq!(reply.status, StatusCode::FORBIDDEN);
        }
    }
}

#[tokio::test]
async fn machine_mount_snapshot_selects_actual_locations_choices_and_attached_environment_sources()
{
    let world = World::new().await;
    let c = company(&world).await;
    let (host, machine) = host(&world, c.viewer.id(), c.acme).await;
    let visible = named_mount(&world, &c.viewer, c.acme, "scoped-location").await;
    let excluded = named_mount(&world, &c.viewer, c.acme, "excluded-location").await;
    let available = named_mount(&world, &c.viewer, c.acme, "scoped-choice").await;
    location(
        &world,
        machine,
        visible.base.id,
        c.viewer.id(),
        "/srv/scoped-location",
    )
    .await;
    location(
        &world,
        machine,
        excluded.base.id,
        c.viewer.id(),
        "/srv/excluded-location",
    )
    .await;
    let env = attached_on_host(
        &world,
        host,
        c.acme,
        c.viewer.id(),
        "Scoped attached environment",
        visible.base.id,
    )
    .await;
    attached_on_host(
        &world,
        host,
        c.acme,
        c.viewer.id(),
        "Excluded attached environment",
        visible.base.id,
    )
    .await;
    let token = key(
        &world,
        c.viewer.id(),
        vec![
            rule("MachineMountList", "host", host),
            rule("MachineMountList", "mount", visible.base.id),
            rule("MachineMountList", "mount", available.base.id),
            rule("MachineMountList", "environment", env),
        ],
    )
    .await;
    let value: MachineMounts = accepted(mounts(&world, &token, machine).await);
    assert_eq!(value.locations.len(), 1);
    assert_eq!(value.locations[0].mount.id, visible.base.id);
    assert_eq!(
        value.locations[0]
            .environments
            .iter()
            .map(|row| row.id)
            .collect::<Vec<_>>(),
        vec![env]
    );
    assert_eq!(
        value
            .available
            .iter()
            .map(|row| row.mount.id)
            .collect::<Vec<_>>(),
        vec![available.base.id]
    );
    assert!(!serde_json::to_string(&value).unwrap().contains("Excluded"));
}

#[tokio::test]
async fn machine_mount_snapshot_bounds_its_own_address_components_and_keeps_native_source_masking()
{
    let world = World::new().await;
    let c = company(&world).await;
    let machine = machine(&world, c.viewer.id()).await;
    let mount = named_mount(&world, &c.boss, c.acme, "native-masked-source").await;
    name_person(&world, "mount", mount.base.id, "mount.view", c.viewer.id()).await;
    location(
        &world,
        machine,
        mount.base.id,
        c.boss.id(),
        "/srv/private-source-path",
    )
    .await;
    let ordinary: MachineMounts = accepted(mounts(&world, &c.viewer.token, machine).await);
    let address = ordinary.locations[0].mount.address.clone();
    for operation in [None, Some("MountGet"), Some("MachineMountList")] {
        let mut rules = vec![
            rule("MachineMountList", "machine", machine),
            rule("MachineMountList", "mount", mount.base.id),
        ];
        if let Some(operation) = operation {
            rules.push(rule(operation, "org", c.acme));
        }
        let token = key(&world, c.viewer.id(), rules).await;
        let value: MachineMounts = accepted(mounts(&world, &token, machine).await);
        assert!(value.locations[0].source.is_none());
        assert!(!value.locations[0].can_remove);
        if operation == Some("MachineMountList") {
            assert_eq!(value.locations[0].mount.address, address);
        } else {
            assert_ne!(value.locations[0].mount.address, address);
        }
    }
}

#[tokio::test]
async fn machine_directory_snapshot_selects_only_actual_readable_uses_under_the_configured_roots() {
    let world = World::new().await;
    let c = company(&world).await;
    let machine = machine(&world, c.viewer.id()).await;
    let env = environment(
        &world,
        machine,
        c.viewer.id(),
        c.acme,
        "Scoped directory use",
        "/srv/work/scoped",
    )
    .await;
    environment(
        &world,
        machine,
        c.viewer.id(),
        c.acme,
        "Excluded directory use",
        "/srv/work/excluded",
    )
    .await;
    let unused = environment(
        &world,
        machine,
        c.viewer.id(),
        c.acme,
        "Outside configured roots",
        "/outside/work",
    )
    .await;
    let token = key(
        &world,
        c.viewer.id(),
        vec![
            rule("MachineEnvironmentDataGet", "machine", machine),
            rule("MachineEnvironmentDataGet", "environment", env),
        ],
    )
    .await;
    let value: MachineEnvironmentData = accepted(data(&world, &token, machine).await);
    assert_eq!(value.roots[0].uses.len(), 1);
    assert_eq!(value.roots[0].uses[0].environment.id, env);
    assert_eq!(value.roots[0].uses[0].path, "/srv/work/scoped");
    assert!(value.direct.is_some());
    assert!(!serde_json::to_string(&value).unwrap().contains("Excluded"));
    let token = key(
        &world,
        c.viewer.id(),
        vec![
            rule("MachineEnvironmentDataGet", "machine", machine),
            rule("MachineEnvironmentDataGet", "environment", unused),
        ],
    )
    .await;
    let value: MachineEnvironmentData = accepted(data(&world, &token, machine).await);
    assert!(value.roots[0].uses.is_empty());
}

async fn activity_wait(world: &World, pid: i32, request: &tokio::task::JoinHandle<RawReply>) {
    tokio::time::timeout(std::time::Duration::from_secs(30),async{loop{assert!(!request.is_finished(),"the native snapshot must reach actual activity bookkeeping");let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')").bind(pid).fetch_one(&world.db.pool).await.unwrap();if blocked{break;}tokio::task::yield_now().await;}}).await.expect("the actual final machine snapshot activity INSERT must wait on the held backend");
}

#[tokio::test]
async fn machine_mount_publication_rechecks_native_source_disclosure_rights_not_only_same_ids() {
    let world = World::new().await;
    let c = company(&world).await;
    let machine = machine(&world, c.viewer.id()).await;
    let mount = named_mount(&world, &c.boss, c.acme, "rights-sensitive-location").await;
    name_person(&world, "mount", mount.base.id, "mount.view", c.viewer.id()).await;
    let use_entry = name_person(&world, "mount", mount.base.id, "mount.use", c.viewer.id()).await;
    location(
        &world,
        machine,
        mount.base.id,
        c.boss.id(),
        "/srv/rights-sensitive-path",
    )
    .await;
    let token = key(
        &world,
        c.viewer.id(),
        vec![
            rule("MachineMountList", "machine", machine),
            rule("MachineMountList", "mount", mount.base.id),
        ],
    )
    .await;
    let before: MachineMounts = accepted(mounts(&world, &token, machine).await);
    assert!(before.locations[0].source.is_some());
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
    let request_token = token.clone();
    let request = tokio::spawn(async move {
        let request = app
            .call(MachineMountList, &[&machine])
            .token(&request_token);
        request.send_bytes().await
    });
    activity_wait(&world, pid, &request).await;
    sqlx::query("UPDATE permission_entries SET removed_at=$2 WHERE id=$1")
        .bind(use_entry)
        .bind(world.now())
        .execute(&world.db.pool)
        .await
        .unwrap();
    blocker.commit().await.unwrap();
    let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    assert!(!String::from_utf8_lossy(&reply.raw).contains("rights-sensitive-path"));
    let current: MachineMounts = accepted(mounts(&world, &token, machine).await);
    assert!(current.locations[0].source.is_none());
    assert!(!current.locations[0].can_remove);
}

#[tokio::test]
async fn machine_mount_publication_rechecks_the_exact_location_source_after_activity_wait() {
    for change in ["unchanged", "expired", "replacement"] {
        let world = World::new().await;
        let c = company(&world).await;
        let machine = machine(&world, c.viewer.id()).await;
        let mount = named_mount(&world, &c.viewer, c.acme, "held-location-source").await;
        location(
            &world,
            machine,
            mount.base.id,
            c.viewer.id(),
            "/srv/held-location-source",
        )
        .await;
        let token = key(
            &world,
            c.viewer.id(),
            vec![
                rule("MachineMountList", "machine", machine),
                rule("MachineMountList", "mount", mount.base.id),
            ],
        )
        .await;
        let control: MachineMounts = accepted(mounts(&world, &token, machine).await);
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
        let request_token = token.clone();
        let request = tokio::spawn(async move {
            let request = app
                .call(MachineMountList, &[&machine])
                .token(&request_token);
            request.send_bytes().await
        });
        activity_wait(&world, pid, &request).await;
        if change == "expired" {
            world.advance(chrono::Duration::hours(2));
        }
        if change == "replacement" {
            sqlx::query("UPDATE mount_locations SET source='/srv/replacement-location-source' WHERE machine_id=$1 AND mount_id=$2").bind(machine).bind(mount.base.id).execute(&world.db.pool).await.unwrap();
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request)
            .await
            .unwrap()
            .unwrap();
        if change == "unchanged" {
            let value: MachineMounts = accepted(reply);
            assert_eq!(value, control);
        } else {
            assert_eq!(
                reply.status,
                if change == "expired" {
                    StatusCode::UNAUTHORIZED
                } else {
                    StatusCode::FORBIDDEN
                }
            );
            assert!(!String::from_utf8_lossy(&reply.raw).contains("held-location-source"));
        }
        if change == "replacement" {
            let value: MachineMounts = accepted(mounts(&world, &token, machine).await);
            assert_eq!(
                value.locations[0].source.as_deref(),
                Some("/srv/replacement-location-source")
            );
        }
    }
}

#[tokio::test]
async fn machine_directory_publication_rechecks_actual_use_path_association_after_activity_wait() {
    for change in ["unchanged", "expired", "relocated_path"] {
        let world = World::new().await;
        let c = company(&world).await;
        let machine = machine(&world, c.viewer.id()).await;
        let env = environment(
            &world,
            machine,
            c.viewer.id(),
            c.acme,
            "Held directory use",
            "/srv/work/held-use",
        )
        .await;
        let token = key(
            &world,
            c.viewer.id(),
            vec![
                rule("MachineEnvironmentDataGet", "machine", machine),
                rule("MachineEnvironmentDataGet", "environment", env),
            ],
        )
        .await;
        let control: MachineEnvironmentData = accepted(data(&world, &token, machine).await);
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
        let request_token = token.clone();
        let request = tokio::spawn(async move {
            let request = app
                .call(MachineEnvironmentDataGet, &[&machine])
                .token(&request_token);
            request.send_bytes().await
        });
        activity_wait(&world, pid, &request).await;
        if change == "expired" {
            world.advance(chrono::Duration::hours(2));
        }
        if change == "relocated_path" {
            sqlx::query(
                "UPDATE environments SET workspace_root='/srv/work/current-use' WHERE id=$1",
            )
            .bind(env)
            .execute(&world.db.pool)
            .await
            .unwrap();
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request)
            .await
            .unwrap()
            .unwrap();
        if change == "unchanged" {
            let value: MachineEnvironmentData = accepted(reply);
            assert_eq!(value, control);
        } else {
            assert_eq!(
                reply.status,
                if change == "expired" {
                    StatusCode::UNAUTHORIZED
                } else {
                    StatusCode::FORBIDDEN
                }
            );
        }
        if change == "relocated_path" {
            let value: MachineEnvironmentData = accepted(data(&world, &token, machine).await);
            assert_eq!(value.roots[0].uses[0].path, "/srv/work/current-use");
        }
    }
}
