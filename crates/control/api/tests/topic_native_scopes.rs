//! Topic readers select the actual native styles and heterogeneous placement sources.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]
mod places;
mod resources;
mod support;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::environment::PlaceListQuery;
use orch_api_contract::topics::{TopicEnvironmentList, TopicPlaceList};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_db::queries::identity::{self, NewApiToken};
use orch_db::queries::resources::access_fixtures as fixtures;
use orch_types::{Id, Json, Time, new_id};
use serde_json::json;
use support::World;
async fn key(world: &World, user: Id, rules: Vec<Json>) -> String {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &NewApiToken {
            id: new_id(),
            user_id: user,
            name: "Native topic sources",
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
fn rule(kind: &str, id: Id, operation: &str) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"allow":[{"operation":operation,"actions":["read"]}]})
}
fn value(reply: orch_api_core::testing::RawReply) -> Json {
    assert_eq!(reply.status, 200, "{}", String::from_utf8_lossy(&reply.raw));
    serde_json::from_slice(&reply.raw).unwrap()
}
async fn topic(world: &World, c: &resources::Company) -> Id {
    orch_api_core::routes::install_master_key(&orch_db::secrets::MasterKey::new([7; 32]));
    let project = places::make_project(world, &c.boss, c.acme, "Topic source")
        .await
        .base
        .id;
    places::make_topic(world, &c.boss, project, "Selected topic")
        .await
        .base
        .id
}
async fn environment(world: &World, c: &resources::Company, topic: Id, name: &str) -> (Id, Id) {
    let mut conn = world.conn().await;
    let id = fixtures::insert_environment(&mut conn, c.admin.id(), Some(topic), name, "managed")
        .await
        .unwrap();
    let template: Id = sqlx::query_scalar("SELECT template_id FROM environments WHERE id=$1")
        .bind(id)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    orch_authz::ownership::start_resource(
        &mut conn,
        orch_access_contract::decisions::ObjectRef {
            kind: orch_access_contract::permissions::ObjectKind::Environment,
            id,
        },
        c.admin.id(),
        orch_authz::ownership::NewAccess::Defaults,
        Time::new(world.now()),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE environments SET config=$2 WHERE id=$1")
        .bind(id)
        .bind(json!({"env.variables":{"TOPIC_NATIVE_MARKER":name}}))
        .execute(&mut *conn)
        .await
        .unwrap();
    (id, template)
}
async fn private(world: &World, id: Id, owner: Id) {
    use orch_db::queries::access::{PageKey, RowWrite, write_row};
    let mut conn = world.conn().await;
    let page = PageKey::new("environment", id, "live");
    for permission in [
        "environment.view",
        "environment.manage",
        "files.view",
        "files.manage",
        "terminal.view",
        "terminal.manage",
        "browser.view",
        "browser.manage",
        "vnc.view",
        "vnc.manage",
        "ports.view",
        "ports.manage",
    ] {
        write_row(
            &mut conn,
            &RowWrite {
                page: &page,
                profile: "any",
                permission,
                inherit: false,
                locked: false,
            },
            Some(owner),
            world.now(),
        )
        .await
        .unwrap();
    }
}
#[tokio::test]
async fn topic_environment_whole_style_keeps_its_real_template_and_native_default_selection() {
    let world = World::new().await;
    let c = resources::company(&world).await;
    let topic = topic(&world, &c).await;
    let (first, template) = environment(&world, &c, topic, "Whole environment").await;
    let (other, _) = environment(&world, &c, topic, "Other environment").await;
    let ordinary = value(
        world
            .app
            .call(TopicEnvironmentList, &[&topic])
            .token(&c.admin.token)
            .send_bytes()
            .await,
    );
    assert_eq!(ordinary.as_array().unwrap().len(), 2);
    assert_eq!(
        ordinary
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == json!(first))
            .unwrap()["config"]["env.variables"]["TOPIC_NATIVE_MARKER"],
        "Whole environment"
    );
    let without_template = key(
        &world,
        c.admin.id(),
        vec![rule("environment", first, "TopicEnvironmentList")],
    )
    .await;
    assert_eq!(
        value(
            world
                .app
                .call(TopicEnvironmentList, &[&topic])
                .token(&without_template)
                .send_bytes()
                .await
        ),
        json!([]),
        "a mandatory native template reference is not silently replaced"
    );
    let scoped = key(
        &world,
        c.admin.id(),
        vec![
            rule("environment", first, "TopicEnvironmentList"),
            rule("template", template, "TopicEnvironmentList"),
        ],
    )
    .await;
    let result = value(
        world
            .app
            .call(TopicEnvironmentList, &[&topic])
            .token(&scoped)
            .send_bytes()
            .await,
    );
    assert_eq!(result.as_array().unwrap().len(), 1);
    assert_eq!(result[0]["id"], json!(first));
    assert_ne!(result[0]["id"], json!(other));
    assert_eq!(
        result[0]["config"]["env.variables"]["TOPIC_NATIVE_MARKER"],
        "Whole environment"
    );
    assert_eq!(result[0]["template"]["id"], json!(template));
    assert!(result[0]["home"]["name"].is_null());
    assert!(!result[0]["address"].as_str().unwrap().contains("acme"));
    let unrelated = key(
        &world,
        c.admin.id(),
        vec![rule("environment", first, "EnvironmentGet")],
    )
    .await;
    assert_eq!(
        world
            .app
            .call(TopicEnvironmentList, &[&topic])
            .token(&unrelated)
            .send_bytes()
            .await
            .problem()
            .code,
        orch_api_contract::common::ErrorCode::AuthzDenied
    );
}
#[tokio::test]
async fn topic_environment_through_thread_style_needs_no_unemitted_template_or_machine_and_never_becomes_whole()
 {
    let world = World::new().await;
    let c = resources::company(&world).await;
    let topic = topic(&world, &c).await;
    let (env, template) = environment(&world, &c, topic, "Thread-only environment").await;
    private(&world, env, c.admin.id()).await;
    let thread = new_id();
    {
        let mut conn = world.conn().await;
        orch_db::queries::hierarchy::testing::insert_node(
            &mut conn,
            thread,
            "thread",
            Some(topic),
            "Visible bound thread",
            c.boss.id(),
        )
        .await
        .unwrap();
        sqlx::query("UPDATE threads SET environment_id=$2 WHERE node_id=$1")
            .bind(thread)
            .bind(env)
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    let ordinary = value(
        world
            .app
            .call(TopicEnvironmentList, &[&topic])
            .token(&c.viewer.token)
            .send_bytes()
            .await,
    );
    assert_eq!(ordinary.as_array().unwrap().len(), 1);
    assert!(ordinary[0]["template"].is_null());
    assert_eq!(ordinary[0]["facts"], json!({}));
    let scoped = key(
        &world,
        c.viewer.id(),
        vec![rule("environment", env, "TopicEnvironmentList")],
    )
    .await;
    let result = value(
        world
            .app
            .call(TopicEnvironmentList, &[&topic])
            .token(&scoped)
            .send_bytes()
            .await,
    );
    assert_eq!(result.as_array().unwrap().len(), 1);
    assert_eq!(result[0]["id"], json!(env));
    assert!(result[0]["template"].is_null() && result[0]["machine"].is_null());
    assert_eq!(result[0]["config"], json!({}));
    assert_eq!(result[0]["facts"], json!({}));
    assert_eq!(result[0]["workspace_root"], "");
    let with_template = key(
        &world,
        c.viewer.id(),
        vec![
            rule("environment", env, "TopicEnvironmentList"),
            rule("template", template, "TopicEnvironmentList"),
        ],
    )
    .await;
    let same = value(
        world
            .app
            .call(TopicEnvironmentList, &[&topic])
            .token(&with_template)
            .send_bytes()
            .await,
    );
    assert_eq!(
        same, result,
        "scope cannot enlarge the person's native style"
    );
}
async fn host(world: &World, c: &resources::Company, name: &str, memory: i64) -> Id {
    let mut conn = world.conn().await;
    let id = fixtures::insert_resource(&mut conn, "host", Some(c.admin.id()), Some(c.acme), name)
        .await
        .unwrap();
    orch_authz::ownership::start_resource(
        &mut conn,
        orch_access_contract::decisions::ObjectRef {
            kind: orch_access_contract::permissions::ObjectKind::Host,
            id,
        },
        c.admin.id(),
        orch_authz::ownership::NewAccess::Defaults,
        Time::new(world.now()),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE hosts SET drivers=$2,capacity=$3,storages=$4 WHERE id=$1").bind(id)
        .bind(json!({"docker":{"name":"docker","title":"Docker","spec_schemas":{"environment":{},"provider":{}},"runtime_classes":["runc"],"sources":["image","build","archive"]}}))
        .bind(json!({"cpus":8,"memory_mb":memory,"disk_gb":500}))
        .bind(json!([{"name":"fast","default":true,"capacity_gb":500,"free_gb":300,"private_views":false}]))
        .execute(&mut *conn).await.unwrap();
    sqlx::query(
        "UPDATE machines SET online=true WHERE id=(SELECT machine_id FROM hosts WHERE id=$1)",
    )
    .bind(id)
    .execute(&mut *conn)
    .await
    .unwrap();
    id
}
async fn template(world: &World, c: &resources::Company) -> Id {
    let mut conn = world.conn().await;
    let id = fixtures::insert_resource(
        &mut conn,
        "template",
        Some(c.admin.id()),
        Some(c.acme),
        "Chosen template",
    )
    .await
    .unwrap();
    orch_authz::ownership::start_resource(
        &mut conn,
        orch_access_contract::decisions::ObjectRef {
            kind: orch_access_contract::permissions::ObjectKind::Template,
            id,
        },
        c.admin.id(),
        orch_authz::ownership::NewAccess::Defaults,
        Time::new(world.now()),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE env_templates SET driver='docker',spec=$2 WHERE id=$1").bind(id).bind(json!({"image":"busybox:latest","runtime":"runc","user":{"name":"agent","uid":1000,"gid":1000},"resources":{"cpu_millis":2000,"memory_mb":2048,"pids":512,"disk_gb":20},"needs":{"cpu_millis":500,"memory_mb":1024,"disk_gb":5},"exec":{"offers":["orch-exec"],"orch_tools":true},"colocated_harnesses":[],"host_labels":[],"init":null})).execute(&mut *conn).await.unwrap();
    id
}
#[tokio::test]
async fn topic_places_bind_the_actual_template_and_heterogeneous_rows_before_native_scheduler_choice()
 {
    let world = World::new().await;
    let c = resources::company(&world).await;
    let topic = topic(&world, &c).await;
    let chosen = template(&world, &c).await;
    let big = host(&world, &c, "Unselected larger host", 32768).await;
    let small = host(&world, &c, "Selected smaller host", 16384).await;
    let (env, _) = environment(&world, &c, topic, "Joinable environment").await;
    let query = PlaceListQuery {
        template_id: Some(chosen),
        config: None,
    };
    let ordinary = value(
        world
            .app
            .call(TopicPlaceList, &[&topic])
            .token(&c.admin.token)
            .query(&query)
            .send_bytes()
            .await,
    );
    assert!(
        ordinary
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["kind"] == "host"
                && row["host"]["id"] == json!(big)
                && row["automatic"] == true)
    );
    let scoped = key(
        &world,
        c.admin.id(),
        vec![
            rule("template", chosen, "TopicPlaceList"),
            rule("host", small, "TopicPlaceList"),
            rule("environment", env, "TopicPlaceList"),
        ],
    )
    .await;
    let result = value(
        world
            .app
            .call(TopicPlaceList, &[&topic])
            .token(&scoped)
            .query(&query)
            .send_bytes()
            .await,
    );
    assert_eq!(result.as_array().unwrap().len(), 2);
    assert_eq!(result[0]["kind"], "host");
    assert_eq!(result[0]["host"]["id"], json!(small));
    assert_eq!(result[0]["automatic"], true);
    assert_eq!(result[1]["kind"], "environment");
    assert_eq!(result[1]["environment"]["id"], json!(env));
    let no_template = key(
        &world,
        c.admin.id(),
        vec![
            rule("host", small, "TopicPlaceList"),
            rule("environment", env, "TopicPlaceList"),
        ],
    )
    .await;
    assert_eq!(
        world
            .app
            .call(TopicPlaceList, &[&topic])
            .token(&no_template)
            .query(&query)
            .send_bytes()
            .await
            .problem()
            .code,
        orch_api_contract::common::ErrorCode::AuthzDenied
    );
    world
        .app
        .call(
            orch_api_contract::settings::NodeSettingSet,
            &[&topic, &"env.template_id"],
        )
        .token(&c.admin.token)
        .body(&orch_api_contract::settings::NodeSettingSetBody {
            value: json!(chosen),
            locked: None,
            expected_revision: None,
        })
        .send()
        .await
        .ok();
    let defaulted = value(
        world
            .app
            .call(TopicPlaceList, &[&topic])
            .token(&scoped)
            .query(&PlaceListQuery {
                template_id: None,
                config: None,
            })
            .send_bytes()
            .await,
    );
    assert_eq!(
        defaulted, result,
        "the inherited actual template uses the same native plan"
    );
}
const WAIT_NATIVE: std::time::Duration = std::time::Duration::from_secs(30);
#[tokio::test]
async fn topic_environment_publication_keeps_the_original_native_reference_after_activity_wait() {
    for change in [false, true] {
        let world = World::new().await;
        let c = resources::company(&world).await;
        let topic = topic(&world, &c).await;
        let (env, old_template) = environment(&world, &c, topic, "Held environment").await;
        let new_template = template(&world, &c).await;
        let scoped = key(
            &world,
            c.admin.id(),
            vec![
                rule("environment", env, "TopicEnvironmentList"),
                rule("template", old_template, "TopicEnvironmentList"),
            ],
        )
        .await;
        let mut blocker = world.db.pool.begin().await.unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let app = world.app.clone();
        let action = tokio::spawn(async move {
            let request = app.call(TopicEnvironmentList, &[&topic]).token(&scoped);
            request.send_bytes().await
        });
        let reached=tokio::time::timeout(WAIT_NATIVE,async {loop {
            if action.is_finished() {break false;}
            let found:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                .bind(pid).fetch_one(&world.db.pool).await.unwrap();if found {break true;}tokio::task::yield_now().await;
        }}).await.unwrap();
        assert!(reached, "the actual successful read bookkeeping must wait");
        if change {
            sqlx::query("UPDATE environments SET template_id=$2 WHERE id=$1")
                .bind(env)
                .bind(new_template)
                .execute(&world.db.pool)
                .await
                .unwrap();
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(WAIT_NATIVE, action)
            .await
            .unwrap()
            .unwrap();
        if change {
            assert_eq!(
                reply.problem().code,
                orch_api_contract::common::ErrorCode::AuthzDenied
            );
            assert!(!String::from_utf8_lossy(&reply.raw).contains("Held environment"));
        } else {
            let result = value(reply);
            assert_eq!(result[0]["id"], json!(env));
            assert_eq!(result[0]["template"]["id"], json!(old_template));
        }
    }
}
