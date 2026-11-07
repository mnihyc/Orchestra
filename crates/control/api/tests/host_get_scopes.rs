//! A native Host read and its once-only proposal keep their exact operation and sources.
// The direct native command verifier has the same future depth as the Host ports fixture.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod resources;
mod support;

use http::StatusCode;
use orch_access_contract::{decisions::ObjectRef, key_scope::KeyScope, permissions::ResourceKind};
use orch_api_contract::resources::HostGet;
use orch_api_core::{
    middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash},
    testing::RawReply,
    thread_access::AuthzAccess,
};
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::queries::{identity, link, resources::access_fixtures};
use orch_engine::commands::CommandAuthority;
use orch_types::{Id, Json, Time, new_id};
use serde_json::json;
use support::World;

const WAIT: std::time::Duration = std::time::Duration::from_secs(30);

struct Key {
    id: Id,
    secret: String,
}
async fn key(world: &World, user: Id, rules: Vec<Json>) -> Key {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id,
            user_id: user,
            name: "Host native read",
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
    Key { id, secret }
}
fn rule(operation: &str, kind: &str, id: Id) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},
        "allow":[{"operation":operation,"actions":["read"]}]})
}
async fn make_host(world: &World, owner: Id, home: Option<Id>) -> (Id, Id) {
    let mut conn = world.conn().await;
    let id = access_fixtures::insert_resource(&mut conn, "host", Some(owner), home, "Native Host")
        .await
        .unwrap();
    start_resource(
        &mut conn,
        ObjectRef {
            kind: ResourceKind::Host.into(),
            id,
        },
        owner,
        NewAccess::OnlyMe,
        Time::new(world.now()),
    )
    .await
    .unwrap();
    let machine: Id = sqlx::query_scalar("SELECT machine_id FROM hosts WHERE id=$1")
        .bind(id)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    (id, machine)
}
async fn get(world: &World, token: &str, id: Id) -> RawReply {
    world
        .app
        .call(HostGet, &[&id])
        .token(token)
        .send_bytes()
        .await
}
fn full(reply: RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
async fn online(world: &World, machine: Id) {
    sqlx::query("UPDATE machines SET online=true WHERE id=$1")
        .bind(machine)
        .execute(&world.db.pool)
        .await
        .unwrap();
}
async fn proposal_command(world: &World, machine: Id) -> link::CommandWithAuthority {
    let id: Id = tokio::time::timeout(WAIT, async {
        loop {
            let id: Option<Id> = sqlx::query_scalar(
                "SELECT id FROM commands WHERE machine_id=$1 AND kind='host.ports'",
            )
            .bind(machine)
            .fetch_optional(&world.db.pool)
            .await
            .unwrap();
            if let Some(id) = id {
                break id;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the actual HTTP proposal must queue its native command");
    link::command_with_authority(&mut *world.conn().await, id)
        .await
        .unwrap()
        .unwrap()
}
async fn current(world: &World, saved: &link::CommandWithAuthority) -> bool {
    let authority: CommandAuthority =
        serde_json::from_value(saved.authority.clone().unwrap()).unwrap();
    let mut tx = world.db.pool.begin().await.unwrap();
    let allowed = authority
        .current_callers(
            &mut tx,
            &saved.command,
            world.app.app.clock.as_ref(),
            &AuthzAccess::new(world.app.app.clock.clone()),
        )
        .await
        .unwrap()
        .is_some();
    tx.commit().await.unwrap();
    allowed
}

#[tokio::test]
async fn host_get_binds_its_exact_subject_and_keeps_the_native_body_with_sufficient_names() {
    let world = World::new().await;
    let company = resources::company(&world).await;
    let (host, _) = make_host(&world, company.viewer.id(), Some(company.acme)).await;
    let reader = key(
        &world,
        company.viewer.id(),
        vec![
            rule("HostGet", "host", host),
            rule("HostGet", "org", company.acme),
        ],
    )
    .await;
    assert_eq!(
        full(get(&world, &reader.secret, host).await),
        full(get(&world, &company.viewer.token, host).await)
    );
    for rules in [
        vec![rule("HostList", "host", host)],
        vec![rule("HostGet", "machine", host)],
        vec![rule("HostGet", "host", new_id())],
    ] {
        let wrong = key(&world, company.viewer.id(), rules).await;
        assert_eq!(
            get(&world, &wrong.secret, host).await.status,
            StatusCode::FORBIDDEN
        );
    }
    let outsider = key(
        &world,
        company.stranger.id(),
        vec![rule("HostGet", "host", host)],
    )
    .await;
    assert_eq!(
        get(&world, &outsider.secret, host).await.status,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn host_get_keeps_actual_retained_sources_and_its_own_name_namespace() {
    let world = World::new().await;
    let company = resources::company(&world).await;
    let (host, _) = make_host(&world, company.viewer.id(), None).await;
    let environment = access_fixtures::insert_resource(
        &mut *world.conn().await,
        "environment",
        Some(company.viewer.id()),
        Some(company.acme),
        "Retained source",
    )
    .await
    .unwrap();
    sqlx::query("UPDATE hosts SET kept_images=$2 WHERE id=$1").bind(host)
        .bind(json!({"epoch":1,"images":[{"system":"test","environment":environment,"generation":4,
            "capture":new_id(),"reference":format!("orch-keep-{environment}:4"),"bytes":768,"total_bytes":900,"layers":2}]}))
        .execute(&world.db.pool).await.unwrap();
    let mut rules = vec![
        rule("HostGet", "host", host),
        rule("HostList", "environment", environment),
    ];
    let missing = key(&world, company.viewer.id(), rules.clone()).await;
    assert_eq!(
        get(&world, &missing.secret, host).await.status,
        StatusCode::FORBIDDEN
    );
    rules.push(rule("HostGet", "environment", environment));
    rules.push(rule("NodeGet", "org", company.acme));
    let without_names = key(&world, company.viewer.id(), rules.clone()).await;
    let read = full(get(&world, &without_names.secret, host).await);
    assert_eq!(
        read["kept_layers"][0]["environment"]["name"],
        "Retained source"
    );
    assert!(
        !read["kept_layers"][0]["environment"]["address"]
            .as_str()
            .unwrap()
            .contains("acme")
    );
    rules.push(rule("HostGet", "org", company.acme));
    let complete = key(&world, company.viewer.id(), rules).await;
    let read = full(get(&world, &complete.secret, host).await);
    assert!(
        read["kept_layers"][0]["environment"]["address"]
            .as_str()
            .unwrap()
            .contains("acme")
    );
}

#[tokio::test]
async fn a_host_get_probe_is_deduplicated_and_retains_its_real_read_origin_outside_the_wire_body() {
    let world = World::new().await;
    let company = resources::company(&world).await;
    let (host, machine) = make_host(&world, company.viewer.id(), None).await;
    let reader = key(
        &world,
        company.viewer.id(),
        vec![rule("HostGet", "host", host)],
    )
    .await;
    online(&world, machine).await;
    assert!(full(get(&world, &reader.secret, host).await)["ports_proposal"].is_null());
    let saved = proposal_command(&world, machine).await;
    assert_eq!(saved.command.body, json!({"ports":null,"revision":0}));
    let origin = saved.authority.as_ref().unwrap();
    assert_eq!(origin["kind"], "host_ports_probe");
    assert_eq!(origin["authority"]["origin"]["operation"], "HostGet");
    assert!(origin.to_string().contains(&reader.id.to_string()));
    assert!(!origin.to_string().contains(&reader.secret));
    assert!(current(&world, &saved).await);
    full(get(&world, &reader.secret, host).await);
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM commands WHERE machine_id=$1 AND kind='host.ports'",
    )
    .bind(machine)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(count, 1, "an in-flight native proposal is asked only once");
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(reader.id)
        .bind(json!(KeyScope::default()))
        .execute(&world.db.pool)
        .await
        .unwrap();
    assert!(
        !current(&world, &saved).await,
        "a stored unaccepted command cannot reuse its old scope admission"
    );
}

#[tokio::test]
async fn a_host_probe_rechecks_its_reader_after_the_native_host_lock_wait_before_queueing() {
    for expire in [false, true] {
        let world = World::new().await;
        let company = resources::company(&world).await;
        let (host, machine) = make_host(&world, company.viewer.id(), None).await;
        let reader = key(
            &world,
            company.viewer.id(),
            vec![rule("HostGet", "host", host)],
        )
        .await;
        online(&world, machine).await;
        let mut blocker = world.db.pool.begin().await.unwrap();
        sqlx::query("SELECT id FROM hosts WHERE id=$1 FOR UPDATE")
            .bind(host)
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        // The foreground read still completes; its one background supplier waits independently.
        full(get(&world, &reader.secret, host).await);
        tokio::time::timeout(WAIT,async {
            loop {
                let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%SELECT id FROM hosts%FOR SHARE%')")
                    .bind(pid).fetch_one(&world.db.pool).await.unwrap();
                if blocked {break;}
                tokio::task::yield_now().await;
            }
        }).await.expect("the native background supplier must reach its actual Host source lock");
        if expire {
            world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        if !expire {
            let saved = proposal_command(&world, machine).await;
            assert!(current(&world, &saved).await);
        } else {
            // A subsequent own Host lock waits for the supplier's original guarded frame to end.
            let mut conn = world.conn().await;
            sqlx::query("SELECT id FROM hosts WHERE id=$1 FOR UPDATE")
                .bind(host)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
            let count: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM commands WHERE machine_id=$1 AND kind='host.ports'",
            )
            .bind(machine)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
            assert_eq!(
                count, 0,
                "no native queue effect follows expiration during source locking"
            );
            assert!(world.app.app.host_ports.get(host).is_none());
        }
    }
}

#[tokio::test]
async fn a_queued_host_probe_rechecks_native_management_and_its_physical_association() {
    for change in ["management", "machine"] {
        let world = World::new().await;
        let company = resources::company(&world).await;
        let (host, machine) = make_host(&world, company.boss.id(), None).await;
        resources::name_person(&world, "host", host, "host.view", company.viewer.id()).await;
        let manage =
            resources::name_person(&world, "host", host, "host.manage", company.viewer.id()).await;
        let reader = key(
            &world,
            company.viewer.id(),
            vec![rule("HostGet", "host", host)],
        )
        .await;
        online(&world, machine).await;
        full(get(&world, &reader.secret, host).await);
        let saved = proposal_command(&world, machine).await;
        assert!(current(&world, &saved).await);
        if change == "management" {
            sqlx::query("UPDATE permission_entries SET removed_at=$2,removed_by=$3 WHERE id=$1")
                .bind(manage)
                .bind(world.now())
                .bind(company.boss.id())
                .execute(&world.db.pool)
                .await
                .unwrap();
            assert!(
                !current(&world, &saved).await,
                "the read key cannot lend an old native management role"
            );
            full(get(&world, &reader.secret, host).await);
        } else {
            let other_machine = orch_db::queries::resources::testing::insert_machine(
                &mut *world.conn().await,
                "Replacement supplier",
                "managed",
            )
            .await
            .unwrap();
            sqlx::query("UPDATE hosts SET machine_id=$2 WHERE id=$1")
                .bind(host)
                .bind(other_machine)
                .execute(&world.db.pool)
                .await
                .unwrap();
            assert!(
                !current(&world, &saved).await,
                "another physical supplier cannot validate the queued reader"
            );
        }
    }
}

#[tokio::test]
async fn a_browser_host_probe_retains_its_actual_session_and_refuses_its_revocation() {
    let world = World::new().await;
    let company = resources::company(&world).await;
    let (host, machine) = make_host(&world, company.viewer.id(), None).await;
    online(&world, machine).await;
    full(get(&world, &company.viewer.token, host).await);
    let saved = proposal_command(&world, machine).await;
    assert!(current(&world, &saved).await);
    let origin = saved.authority.as_ref().unwrap();
    assert_eq!(origin["authority"]["origin"]["operation"], "HostGet");
    assert!(!origin["authority"]["raw_deadline"].is_null());
    let session: Id = origin["authority"]["session"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let user: Id = sqlx::query_scalar("SELECT user_id FROM sessions WHERE id=$1")
        .bind(session)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        user,
        company.viewer.id(),
        "the original raw session belongs to the reader"
    );
    sqlx::query("UPDATE sessions SET revoked_at=$2 WHERE id=$1")
        .bind(session)
        .bind(world.now())
        .execute(&world.db.pool)
        .await
        .unwrap();
    assert!(
        !current(&world, &saved).await,
        "an account identity cannot replace its revoked browser origin"
    );
}
