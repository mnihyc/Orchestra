//! Stored Machine metadata keeps its native inventory, readiness and actual source associations.
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
use orch_api_contract::resources::{Machine, MachineGet};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::queries::identity::{self, NewApiToken};
use orch_db::queries::resources::access_fixtures as fixtures;
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
async fn key(world: &World, owner: Id, rules: Vec<Json>) -> Key {
    issue(world, owner, Some(rules)).await
}
async fn issue(world: &World, owner: Id, rules: Option<Vec<Json>>) -> Key {
    let scope = rules.map(|rules| {
        let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
        assert!(scope.validate().is_empty());
        json!(scope)
    });
    let legacy = if scope.is_none() {
        vec!["*".to_owned()]
    } else {
        Vec::new()
    };
    let key = Key {
        id: new_id(),
        secret: new_token(API_TOKEN_PREFIX),
    };
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &NewApiToken {
            id: key.id,
            user_id: owner,
            name: "Native individual Machine sources",
            token_hash: &token_hash(&key.secret),
            scopes: &legacy,
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        scope.as_ref(),
    )
    .await
    .unwrap();
    key
}
async fn get(world: &World, token: &str, id: Id) -> RawReply {
    let request = world.app.call(MachineGet, &[&id]).token(token);
    request.send_bytes().await
}
fn shown(reply: RawReply) -> Machine {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
async fn machine(world: &World, owner: &places::Who, name: &str) -> Id {
    fixtures::insert_persons_machine(&mut *world.conn().await, owner.id(), name)
        .await
        .unwrap()
}
async fn own(world: &World, kind: ObjectKind, id: Id, owner: Id) {
    start_resource(
        &mut *world.conn().await,
        ObjectRef { kind, id },
        owner,
        NewAccess::OnlyMe,
        Time::new(world.now()),
    )
    .await
    .unwrap();
}
async fn host(world: &World, owner: &places::Who, name: &str) -> (Id, Id) {
    let host = fixtures::insert_resource(
        &mut *world.conn().await,
        "host",
        Some(owner.id()),
        Some(owner.person.personal_org),
        name,
    )
    .await
    .unwrap();
    own(world, ObjectKind::Host, host, owner.id()).await;
    let machine = sqlx::query_scalar("SELECT machine_id FROM hosts WHERE id=$1")
        .bind(host)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    (host, machine)
}
async fn environment(world: &World, owner: &places::Who, machine: Id) -> Id {
    let environment = fixtures::insert_environment(
        &mut *world.conn().await,
        owner.id(),
        Some(owner.person.personal_org),
        "Registered readiness environment",
        "connected",
    )
    .await
    .unwrap();
    fixtures::register_on(
        &mut *world.conn().await,
        "environment",
        environment,
        machine,
    )
    .await
    .unwrap();
    own(world, ObjectKind::Environment, environment, owner.id()).await;
    environment
}
async fn hosted(world: &World, owner: &places::Who, host: Id) -> (Id, Id) {
    use orch_db::queries::environments::hosting::{NewRuntime, insert_runtime};
    let environment = fixtures::insert_environment(
        &mut *world.conn().await,
        owner.id(),
        Some(owner.person.personal_org),
        "Hosted readiness environment",
        "managed",
    )
    .await
    .unwrap();
    let template: Id = sqlx::query_scalar("SELECT template_id FROM environments WHERE id=$1")
        .bind(environment)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    let connector = machine(world, owner, "Native managed unit").await;
    let runtime = new_id();
    let mut conn = world.conn().await;
    insert_runtime(
        &mut conn,
        &NewRuntime {
            id: runtime,
            host_id: host,
            unit_name: &format!("machine-readiness-{runtime}"),
            environment_id: environment,
            driver: "docker",
            template_id: template,
            spec_hash: &[0; 32],
            generation: 1,
            volumes: &json!({}),
        },
        world.now(),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE machines SET placement='managed',owner_id=NULL,host_id=$2,runtime_id=$3 WHERE id=$1").bind(connector).bind(host).bind(runtime).execute(&mut *conn).await.unwrap();
    sqlx::query("UPDATE runtimes SET machine_id=$2,state='running' WHERE id=$1")
        .bind(runtime)
        .bind(connector)
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE environments SET machine_id=$2,status='running' WHERE id=$1")
        .bind(environment)
        .bind(connector)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    own(world, ObjectKind::Environment, environment, owner.id()).await;
    (environment, runtime)
}
async fn activity_wait(world: &World, pid: i32, request: &tokio::task::JoinHandle<RawReply>) {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            assert!(!request.is_finished(), "the native read did not reach activity bookkeeping");
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')").bind(pid).fetch_one(&world.db.pool).await.unwrap();
            if blocked { break; }
            tokio::task::yield_now().await;
        }
    }).await.expect("the real response must reach the actual blocked activity INSERT");
}

#[tokio::test]
async fn machine_get_binds_the_actual_stored_subject_and_keeps_its_native_view_door() {
    let world = World::new().await;
    let owner = places::person(&world, "owner").await;
    let outsider = places::person(&world, "outsider").await;
    let machine = machine(&world, &owner, "Native Machine page").await;
    let ordinary = shown(get(&world, &owner.token, machine).await);
    let scoped = key(
        &world,
        owner.id(),
        vec![rule("MachineGet", "machine", machine)],
    )
    .await;
    assert_eq!(
        json!(shown(get(&world, &scoped.secret, machine).await)),
        json!(ordinary)
    );
    for rule in [
        rule("MachineGet", "environment", machine),
        rule("MachineList", "machine", machine),
    ] {
        let wrong = key(&world, owner.id(), vec![rule]).await;
        assert_eq!(
            get(&world, &wrong.secret, machine).await.status,
            StatusCode::FORBIDDEN
        );
    }
    let unviewed = key(
        &world,
        outsider.id(),
        vec![rule("MachineGet", "machine", machine)],
    )
    .await;
    assert_eq!(
        get(&world, &unviewed.secret, machine).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&world, &scoped.secret, new_id()).await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn machine_get_registered_sources_need_its_own_read_and_keep_native_sandbox_readiness() {
    let world = World::new().await;
    let owner = places::person(&world, "owner").await;
    let machine = machine(&world, &owner, "Readiness physical source").await;
    let environment = environment(&world, &owner, machine).await;
    let mut sources = vec![("environment", environment)];
    for kind in ["provider", "agent"] {
        let id = fixtures::insert_resource(
            &mut *world.conn().await,
            kind,
            Some(owner.id()),
            Some(owner.person.personal_org),
            &format!("registered-{kind}"),
        )
        .await
        .unwrap();
        fixtures::register_on(&mut *world.conn().await, kind, id, machine)
            .await
            .unwrap();
        sources.push((kind, id));
    }
    let mut rules = vec![
        rule("MachineGet", "machine", machine),
        rule("MachineGet", "org", owner.person.personal_org),
    ];
    rules.extend(
        sources
            .iter()
            .map(|(kind, id)| rule("MachineGet", kind, *id)),
    );
    let complete = key(&world, owner.id(), rules).await;
    let ordinary = shown(get(&world, &owner.token, machine).await);
    let scoped = shown(get(&world, &complete.secret, machine).await);
    assert_eq!(json!(scoped), json!(ordinary));
    assert_eq!(scoped.environment_sandboxes.len(), 1);
    assert_eq!(scoped.environment_sandboxes[0].environment.id, environment);
    assert_eq!(
        scoped.environment_sandboxes[0].machine.as_ref().unwrap().id,
        machine
    );
    for operation in ["MachineList", "EnvironmentGet"] {
        let mut rules = vec![rule("MachineGet", "machine", machine)];
        rules.extend(sources.iter().map(|(kind, id)| rule(operation, kind, *id)));
        let borrowed = key(&world, owner.id(), rules).await;
        let bounded = shown(get(&world, &borrowed.secret, machine).await);
        assert_eq!(bounded.base.id, machine);
        assert!(bounded.providers.is_empty());
        assert!(bounded.agents.is_empty());
        assert!(bounded.environments.is_empty());
        assert!(bounded.environment_sandboxes.is_empty());
    }
    let commands: i64 = sqlx::query_scalar("SELECT count(*) FROM commands")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        commands, 0,
        "passive readiness must not probe or launch work"
    );
}

#[tokio::test]
async fn machine_get_keeps_hosted_environment_readiness_and_the_actual_host_facade() {
    let world = World::new().await;
    let owner = places::person(&world, "owner").await;
    let (host, machine) = host(&world, &owner, "Native readiness host").await;
    let (environment, _) = hosted(&world, &owner, host).await;
    let token = key(
        &world,
        owner.id(),
        vec![
            rule("MachineGet", "host", host),
            rule("MachineGet", "environment", environment),
        ],
    )
    .await;
    let shown = shown(get(&world, &token.secret, machine).await);
    assert_eq!(shown.base.id, machine);
    assert_eq!(shown.host.as_ref().unwrap().id, host);
    assert!(
        shown.environments.is_empty(),
        "a hosted unit is not a direct registration on the host machine"
    );
    assert_eq!(shown.environment_sandboxes.len(), 1);
    assert_eq!(shown.environment_sandboxes[0].environment.id, environment);
    assert_eq!(
        shown.environment_sandboxes[0].machine.as_ref().unwrap().id,
        machine
    );
    for primary in [
        rule("MachineGet", "machine", machine),
        rule("HostGet", "host", host),
    ] {
        let wrong = key(
            &world,
            owner.id(),
            vec![primary, rule("MachineGet", "environment", environment)],
        )
        .await;
        assert_eq!(
            get(&world, &wrong.secret, machine).await.status,
            StatusCode::FORBIDDEN
        );
    }
    let missing = key(&world, owner.id(), vec![rule("MachineGet", "host", host)]).await;
    assert!(
        self::shown(get(&world, &missing.secret, machine).await)
            .environment_sandboxes
            .is_empty()
    );
}

#[tokio::test]
async fn machine_get_reference_addresses_use_only_its_own_protected_home_read() {
    let world = World::new().await;
    let owner = places::person(&world, "owner").await;
    let machine = machine(&world, &owner, "Native bounded references").await;
    let environment = environment(&world, &owner, machine).await;
    let ordinary = shown(get(&world, &owner.token, machine).await);
    let address = &ordinary.environments[0].address;
    for operation in [None, Some("MachineList"), Some("MachineGet")] {
        let mut rules = vec![
            rule("MachineGet", "machine", machine),
            rule("MachineGet", "environment", environment),
        ];
        if let Some(operation) = operation {
            rules.push(rule(operation, "org", owner.person.personal_org));
        }
        let token = key(&world, owner.id(), rules).await;
        let value = shown(get(&world, &token.secret, machine).await);
        assert_eq!(value.environments[0].id, environment);
        assert_eq!(
            value.environment_sandboxes[0].environment.address,
            value.environments[0].address
        );
        if operation == Some("MachineGet") {
            assert_eq!(&value.environments[0].address, address);
        } else {
            assert_ne!(&value.environments[0].address, address);
        }
    }
}

#[tokio::test]
async fn machine_get_publication_rechecks_the_actual_hosted_source_after_activity_wait() {
    for (credential, change) in [
        ("scoped", "none"),
        ("scoped", "expired"),
        ("scoped", "runtime_host"),
        ("scoped", "new_unread_environment"),
        ("legacy", "runtime_host"),
    ] {
        let world = World::new().await;
        let owner = places::person(&world, "owner").await;
        let (host, machine) = host(&world, &owner, "Original readiness host").await;
        let (other_host, other_machine) = self::host(&world, &owner, "Other readiness host").await;
        let (environment, runtime) = hosted(&world, &owner, host).await;
        let token = issue(
            &world,
            owner.id(),
            (credential == "scoped").then(|| {
                vec![
                    rule("MachineGet", "host", host),
                    rule("MachineGet", "environment", environment),
                ]
            }),
        )
        .await;
        assert_eq!(
            shown(get(&world, &token.secret, machine).await)
                .environment_sandboxes
                .len(),
            1
        );
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
        let secret = token.secret.clone();
        let request = tokio::spawn(async move {
            let request = app.call(MachineGet, &[&machine]).token(&secret);
            request.send_bytes().await
        });
        activity_wait(&world, pid, &request).await;
        if change == "expired" {
            sqlx::query("UPDATE api_tokens SET expires_at=$2 WHERE id=$1")
                .bind(token.id)
                .bind(world.now())
                .execute(&world.db.pool)
                .await
                .unwrap();
        } else if change == "runtime_host" {
            let mut moved = world.db.pool.begin().await.unwrap();
            sqlx::query("UPDATE runtimes SET host_id=$2 WHERE id=$1")
                .bind(runtime)
                .bind(other_host)
                .execute(&mut *moved)
                .await
                .unwrap();
            sqlx::query("UPDATE machines SET host_id=$2 WHERE runtime_id=$1")
                .bind(runtime)
                .bind(other_host)
                .execute(&mut *moved)
                .await
                .unwrap();
            moved.commit().await.unwrap();
        } else if change == "new_unread_environment" {
            hosted(&world, &owner, host).await;
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request)
            .await
            .unwrap()
            .unwrap();
        match change {
            "none" | "new_unread_environment" => {
                let value = shown(reply);
                assert_eq!(value.environment_sandboxes[0].environment.id, environment);
                assert_eq!(value.environment_sandboxes.len(), 1);
            }
            "expired" => {
                assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
            }
            _ => {
                assert_eq!(reply.status, StatusCode::FORBIDDEN);
                assert!(
                    !String::from_utf8_lossy(&reply.raw).contains("Hosted readiness environment")
                );
                assert!(
                    shown(get(&world, &token.secret, machine).await)
                        .environment_sandboxes
                        .is_empty()
                );
                let current = key(
                    &world,
                    owner.id(),
                    vec![
                        rule("MachineGet", "host", other_host),
                        rule("MachineGet", "environment", environment),
                    ],
                )
                .await;
                assert_eq!(
                    shown(get(&world, &current.secret, other_machine).await).environment_sandboxes
                        [0]
                    .environment
                    .id,
                    environment
                );
            }
        }
    }
}

#[tokio::test]
async fn machine_get_publication_rechecks_the_current_governor_of_the_same_registered_environment()
{
    let world = World::new().await;
    let owner = places::person(&world, "owner").await;
    let other = places::person(&world, "other").await;
    let machine = machine(&world, &owner, "Stable requested machine").await;
    let environment = environment(&world, &owner, machine).await;
    let token = key(&world, owner.id(), vec![rule("MachineGet", "machine", machine), json!({"target":{"kind":"org","id":owner.person.personal_org,"reach":"subtree"},"allow":[{"operation":"MachineGet","actions":["read"],"resources":["environment"]}]})]).await;
    assert_eq!(
        shown(get(&world, &token.secret, machine).await).environments[0].id,
        environment
    );
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
    let secret = token.secret.clone();
    let request = tokio::spawn(async move {
        let request = app.call(MachineGet, &[&machine]).token(&secret);
        request.send_bytes().await
    });
    activity_wait(&world, pid, &request).await;
    sqlx::query("UPDATE environments SET home_node_id=$2,org_id=(SELECT org_id FROM nodes WHERE id=$2) WHERE id=$1")
        .bind(environment)
        .bind(other.person.personal_org)
        .execute(&world.db.pool)
        .await
        .unwrap();
    blocker.commit().await.unwrap();
    let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    let current = key(
        &world,
        owner.id(),
        vec![
            rule("MachineGet", "machine", machine),
            rule("MachineGet", "environment", environment),
        ],
    )
    .await;
    assert_eq!(
        shown(get(&world, &current.secret, machine).await).environment_sandboxes[0]
            .environment
            .id,
        environment
    );
}
