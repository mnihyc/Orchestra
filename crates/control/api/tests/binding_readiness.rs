//! Binding previews use stored reports, one request connection and no action locks, even while several browser pages are open.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod realtime_support;
mod rig;
mod support;

use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::ResourceKind;
use orch_api_contract::binding::{
    BindingOptions, NodeBindingOptionsGet, NodeBindingOptionsGetQuery, ThreadBindingOptionsGet,
};
use orch_api_contract::environment::EnvironmentGet;
use orch_api_contract::nodes::{Node, NodeUpdate, NodeUpdateBody, OrgTreeGet, OrgTreeGetQuery};
use orch_api_contract::resources::ProviderGet;
use orch_api_core::registry::Registry;
use orch_api_core::state::AppState;
use orch_api_core::testing::TestApp;
use orch_api_core::thread_access::AuthzAccess;
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::Db;
use orch_db::queries::resources::access_fixtures;
use orch_engine::{Engine, EngineParts};
use orch_settings::Target;
use orch_types::{Id, Time};
use realtime_support::{Client, Server};
use rig::Scene;
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
use std::time::{Duration, Instant};

// Reproduce the review's four pages against its eight-connection application pool.
const OPEN_PAGES: usize = 4;
const WORLD_CONNECTIONS: u32 = 8;
// A nested acquire must fail promptly in the one-connection case, rather than hide in a 30s wait.
const ACQUIRE_WAIT: Duration = Duration::from_secs(1);
// Allow the loaded integration host its usual scheduling margin, but never hang on a held lock.
const PATIENCE: Duration = rig::PATIENCE;
async fn host_and_template(s: &Scene) -> Id {
    let home = s.tree.topic.base.id;
    let mut conn = s.rig.db.acquire().await.unwrap();
    let host = access_fixtures::insert_resource(
        &mut conn,
        "host",
        Some(s.ada.id()),
        Some(home),
        "Build box",
    )
    .await
    .unwrap();
    let template = access_fixtures::insert_resource(
        &mut conn,
        "template",
        Some(s.ada.id()),
        Some(home),
        "Stored template",
    )
    .await
    .unwrap();
    for (kind, id) in [
        (ResourceKind::Host, host),
        (ResourceKind::Template, template),
    ] {
        start_resource(
            &mut conn,
            ObjectRef {
                kind: kind.into(),
                id,
            },
            s.ada.id(),
            NewAccess::Defaults,
            Time::new(s.rig.world.now()),
        )
        .await
        .unwrap();
    }
    sqlx::query("UPDATE hosts SET drivers = $2, capacity = $3 WHERE id = $1")
        .bind(host)
        .bind(json!({"test": {"spec_schemas": {"environment": {}}, "sources": ["image"]}}))
        .bind(json!({"cpus": 8, "memory_mb": 16384, "disk_gb": 100}))
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE machines SET online = true WHERE id = (SELECT machine_id FROM hosts WHERE id = $1)",
    )
    .bind(host)
    .execute(&mut *conn)
    .await
    .unwrap();
    sqlx::query("UPDATE env_templates SET spec = $2 WHERE id = $1")
        .bind(template).bind(json!({"image": "test:stored", "resources": {"memory_mb": 1024}, "exec": {"offers": []}}))
        .execute(&mut *conn).await.unwrap();
    drop(conn);
    s.rig
        .world
        .set_at(Target::Node(home), "env.template_id", json!(template))
        .await;
    host
}

async fn pool(s: &Scene, connections: u32) -> Db {
    Db::from_pool(
        PgPoolOptions::new()
            .max_connections(connections)
            .acquire_timeout(if connections == 1 {
                ACQUIRE_WAIT
            } else {
                PATIENCE
            })
            .connect(&s.rig.world.db.url)
            .await
            .unwrap(),
    )
}

fn http(db: Db, s: &Scene) -> TestApp {
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    TestApp::new(db, s.rig.world.app.clock.clone(), registry)
}

async fn node_options(app: &TestApp, s: &Scene, node: Id) -> BindingOptions {
    app.call(NodeBindingOptionsGet, &[&node])
        .token(&s.ada.token)
        .query(&NodeBindingOptionsGetQuery {
            environment_id: Some(s.on.environment),
            ..Default::default()
        })
        .send()
        .await
        .ok()
}

fn has_host(options: &BindingOptions, host: Id) {
    assert!(options.can_create.allowed);
    assert!(
        options.hosts.iter().any(|choice| choice.host.id == host),
        "the stored host choice is not discarded after an inner pool timeout"
    );
}

#[tokio::test]
async fn both_binding_previews_offer_the_stored_host_with_only_one_database_connection() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    s.rig.engine.stop();
    let host = host_and_template(&s).await;
    let db = pool(&s, 1).await;
    let app = http(db.clone(), &s);
    let started = Instant::now();
    has_host(&node_options(&app, &s, s.tree.topic.base.id).await, host);
    let options = app
        .call(ThreadBindingOptionsGet, &[&thread])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    has_host(&options, host);
    eprintln!(
        "both stored binding previews with one connection: {:?}",
        started.elapsed()
    );
    assert_eq!(db.pool().size(), 1);
}

#[tokio::test]
async fn a_binding_preview_does_not_wait_for_an_unrelated_actions_authority_lock() {
    let s = Scene::new().await;
    s.rig.engine.stop();
    let app = http(pool(&s, 1).await, &s);
    let mut action = s.rig.db.begin().await.unwrap();
    orch_db::queries::transaction_guard::lock_authority(&mut action)
        .await
        .unwrap();
    let answer = tokio::time::timeout(PATIENCE, node_options(&app, &s, s.tree.topic.base.id)).await;
    action.rollback().await.unwrap();
    let choices =
        answer.expect("reading stored choices must not take an action or credential lock");
    assert!(
        choices
            .providers
            .iter()
            .any(|provider| provider.provider.id == s.on.provider)
    );
}

#[tokio::test]
async fn resource_pages_use_stored_reports_on_one_connection_without_asking_the_host() {
    let s = Scene::new().await;
    s.rig.engine.stop();
    let host = host_and_template(&s).await;
    sqlx::query(
        "UPDATE hosts SET drivers = jsonb_set(drivers, '{test,live_changes}', $2) WHERE id = $1",
    )
    .bind(host)
    .bind(json!({"resources": true, "mounts": false}))
    .execute(s.rig.db.pool())
    .await
    .unwrap();
    for kind in ["environment", "provider"] {
        sqlx::query("INSERT INTO runtimes (id,kind,host_id,unit_name,environment_id,provider_id,driver,template_id,spec_hash,state)
             SELECT $1,$2,$3,$4,CASE WHEN $2='environment' THEN e.id END,CASE WHEN $2='provider' THEN $5::uuid END,'test',e.template_id,$6,'running'
               FROM environments e WHERE e.id=$7")
            .bind(orch_types::new_id()).bind(kind).bind(host).bind(format!("unit-{kind}"))
            .bind(s.on.provider).bind(vec![0_u8; 32]).bind(s.on.environment).execute(s.rig.db.pool()).await.unwrap();
    }
    let db = pool(&s, 1).await;
    let before = s.rig.commands.issued_total();
    let old = &s.rig.world.app.app;
    let engine = Engine::new(EngineParts {
        db: db.clone(),
        clock: old.clock.clone(),
        config: rig::engine_config(),
        commands: s.rig.commands.clone(),
        access: Arc::new(AuthzAccess::new(old.clock.clone())),
        live: Arc::new(orch_engine::live::Discard),
    });
    let app = Arc::new(AppState {
        host_ports: old.host_ports.clone(),
        builtin_sources: old.builtin_sources.clone(),
        db: db.clone(),
        config: old.config.clone(),
        clock: old.clock.clone(),
        link: old.link.clone(),
        engine,
        realtime: old.realtime.clone(),
        build: old.build,
        blobs: old.blobs.clone(),
        cache: old.cache.clone(),
    });
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let app = TestApp::over(app, s.rig.world.app.clock.clone(), registry);
    for provider in [false, true] {
        let request = async {
            if provider {
                app.call(ProviderGet, &[&s.on.provider])
                    .token(&s.ada.token)
                    .send()
                    .await
                    .ok()
                    .runtime_changes
            } else {
                app.call(EnvironmentGet, &[&s.on.environment])
                    .token(&s.ada.token)
                    .send()
                    .await
                    .ok()
                    .runtime_changes
            }
        };
        let changes = tokio::time::timeout(PATIENCE, request)
            .await
            .expect("reading a page needs neither a second connection nor a host reply")
            .expect("the stored runtime is shown");
        assert!(changes.live_changes.resources);
        assert_eq!(
            changes.memory_used_bytes, None,
            "a configured limit is not a measurement of current use"
        );
    }
    assert_eq!(
        s.rig.commands.issued_total(),
        before,
        "page reads issue no native commands"
    );
    assert_eq!(db.pool().size(), 1);
    let mut action = db.begin().await.unwrap();
    orch_db::queries::transaction_guard::lock_authority(&mut action)
        .await
        .unwrap();
    action.rollback().await.unwrap();
}

#[tokio::test]
async fn a_page_joining_an_in_flight_provider_observation_returns_stored_facts_on_one_connection() {
    let s = Scene::new().await;
    s.rig.engine.stop();
    let name = format!("provider.facts:{}", s.on.provider);
    let leader = orch_db::queries::job_leases::Lease::new(
        name.clone(),
        "observation-already-running",
        chrono::Duration::minutes(5),
    );
    assert!(leader.hold(&s.rig.db, s.rig.world.now()).await.unwrap());
    let app = http(pool(&s, 1).await, &s);
    let result = tokio::time::timeout(
        PATIENCE,
        Box::pin(app.app.engine.refresh_provider(s.on.provider)),
    )
    .await
    .expect("an ordinary observation joins the stored result without waiting for the leader");
    assert!(
        matches!(&result, Ok(orch_engine::provider_setup::Refreshed::Answered { status, reported_status: None, .. }) if status == "ready"),
        "another page uses the stored provider instead of acquiring a second connection: {result:?}"
    );
    let page = app
        .call(ProviderGet, &[&s.on.provider])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    let holder = orch_db::queries::job_leases::holder(
        &mut s.rig.db.acquire().await.unwrap(),
        &name,
        s.rig.world.now(),
    )
    .await
    .unwrap();
    assert_eq!(holder.as_deref(), Some("observation-already-running"));
    leader.release(&s.rig.db).await.unwrap();
    assert_eq!(page.base.id, s.on.provider);
}

#[tokio::test]
async fn an_unreported_harness_is_shown_from_storage_without_starting_or_probing_it() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    s.rig.engine.stop();
    sqlx::query("UPDATE agents SET descriptor = '{}'::jsonb WHERE id = $1")
        .bind(s.on.agent)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let before = s.rig.commands.issued_total();
    let app = &s.rig.world.app;
    let topic = node_options(app, &s, s.tree.topic.base.id).await;
    let folder = node_options(app, &s, s.tree.project.base.id).await;
    let existing = app
        .call(ThreadBindingOptionsGet, &[&thread])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    for options in [topic, folder, existing] {
        let provider = options
            .providers
            .iter()
            .find(|provider| provider.provider.id == s.on.provider)
            .unwrap();
        let agent = provider
            .agents
            .iter()
            .find(|agent| agent.agent.id == s.on.agent)
            .unwrap();
        assert!(!agent.usable);
        assert_eq!(agent.reason.as_deref(), Some("harness_unreported"));
        assert!(agent.arrangements.is_empty());
    }
    assert_eq!(
        s.rig.commands.issued_total(),
        before,
        "opening a starter never sends a discovery or inspection command to a machine"
    );
    assert_eq!(s.rig.commands.issued_of("turn.start"), 0);
}

#[tokio::test]
async fn four_open_pages_can_refresh_binding_and_tree_choices_while_a_write_succeeds_on_eight_connections()
 {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    s.rig.engine.stop();
    let host = host_and_template(&s).await;
    let db = pool(&s, WORLD_CONNECTIONS).await;
    let tuning = realtime_support::quick(&s.rig.world.app.app.config);
    let server = Server::over(&db, &s.rig.world.app.app.clock, tuning).await;
    let mut pages = Vec::new();
    for _ in 0..OPEN_PAGES {
        let mut page = Client::signed_in(server.addr, &s.ada.token).await;
        for (sub, feed) in [
            format!("org:{}", s.tree.org.base.id),
            format!("topic:{}", s.tree.topic.base.id),
            format!("thread:{thread}"),
        ]
        .iter()
        .enumerate()
        {
            page.sub(i64::try_from(sub + 1).unwrap(), feed, None).await;
            page.expect("subscribed").await;
        }
        pages.push(page);
    }
    assert_eq!(server.realtime.connections(), OPEN_PAGES);
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let app = TestApp::over(server.app.clone(), s.rig.world.app.clock.clone(), registry);
    let started = Instant::now();
    let work = async {
        let readers = futures::future::join_all((0..OPEN_PAGES).map(|_| async {
            let (topic, existing, tree) = tokio::join!(
                node_options(&app, &s, s.tree.topic.base.id),
                async {
                    app.call(ThreadBindingOptionsGet, &[&thread])
                        .token(&s.ada.token)
                        .send()
                        .await
                        .ok()
                },
                async {
                    app.call(OrgTreeGet, &[&s.tree.org.base.id])
                        .token(&s.ada.token)
                        .query(&OrgTreeGetQuery::default())
                        .send()
                        .await
                        .ok()
                },
            );
            has_host(&topic, host);
            has_host(&existing, host);
            assert!(tree.readable().iter().any(|node| node.id == thread));
        }));
        let writer = async {
            app.call(NodeUpdate, &[&s.tree.project.base.id])
                .token(&s.ada.token)
                .body(&NodeUpdateBody {
                    name: Some("Website".to_owned()),
                    description: None,
                    color: None,
                    custom_color: None,
                    readme_document_id: None,
                    expected_revision: s.tree.project.base.revision,
                })
                .send()
                .await
                .ok()
        };
        let ((), changed) = tokio::join!(
            async {
                readers.await;
            },
            writer
        );
        assert!(matches!(changed, Node::Project(project) if project.base.name == "Website"));
    };
    let result = tokio::time::timeout(PATIENCE, work).await;
    eprintln!(
        "four open pages, twelve reads and an unrelated write with eight connections: {:?}; pool size={}, idle={}",
        started.elapsed(),
        db.pool().size(),
        db.pool().num_idle()
    );
    assert!(
        result.is_ok(),
        "open browser pages must not exhaust the pool or hold the write behind a read transaction"
    );
    for page in &mut pages {
        page.ping().await;
    }
    assert_eq!(
        server.realtime.connections(),
        OPEN_PAGES,
        "the check did not free leases by closing the pages"
    );
    server.stop().await;
}
