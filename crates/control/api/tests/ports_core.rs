//! Real database requests keep stable targets, one reviewed admission, safe audiences and irreversible endings.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod resources;
mod support;

use orch_api_contract::common::ErrorCode;
use orch_api_contract::environment::{
    EnvironmentGet, MachineEnvironmentCreate, MachineEnvironmentCreateBody,
};
use orch_api_contract::ports::*;
use orch_types::{Id, Time, new_id};
use places::{Who, join, make_org, make_project, person};
use serde_json::json;
use support::World;

async fn setup() -> (World, Who, Who, Id, Id, Id) {
    setup_with_domain("ports.test").await
}

async fn setup_with_domain(domain: &str) -> (World, Who, Who, Id, Id, Id) {
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    let mut config = orch_config::ServerConfig::default();
    config.edge.ports_domain = Some(domain.into());
    let mut world = World::with(config).await;
    let mut registry = orch_api_core::registry::Registry::new();
    orch_api_core::routes::mount(&mut registry);
    world.app = orch_api_core::testing::TestApp::over(
        world.app.app.clone(),
        world.app.clock.clone(),
        registry,
    );
    orch_api_core::routes::resources::sealing::install_master_key(
        orch_db::secrets::MasterKey::new([7; 32]),
    );
    let manager = person(&world, "manager").await;
    let colleague = person(&world, "colleague").await;
    let team = make_org(&world, &manager, "Team", "ports-team")
        .await
        .base
        .id;
    join(&world, team, team, &colleague, "viewer", &manager).await;
    let project = make_project(&world, &manager, team, "Preview")
        .await
        .base
        .id;
    let machine = orch_db::queries::resources::access_fixtures::insert_persons_machine(
        &mut *world.conn().await,
        manager.id(),
        "ports fixture",
    )
    .await
    .unwrap();
    let policy = orch_link_contract::policy::LocalPolicy {
        environment_roots: vec!["/srv/ports-core".into()],
        ports: orch_link_contract::policy::MachinePorts {
            allowed: vec![
                orch_api_contract::ports::PortOpeningKind::Place,
                orch_api_contract::ports::PortOpeningKind::Link,
            ],
            publish: None,
            limits: None,
        },
        ..Default::default()
    };
    let descriptor = orch_driver_contract::descriptor::DriverDescriptor::registered_directories(
        "test",
        orch_driver_contract::descriptor::EnvironmentDataStorage {
            made_by_orch: Some(orch_driver_contract::descriptor::MadeDataKind::Directory),
            existing_directory: true,
            reason: None,
        },
    );
    sqlx::query("UPDATE machines SET facts=jsonb_build_object('policy',$2::jsonb,'environment_descriptor',$3::jsonb) WHERE id=$1")
        .bind(machine)
        .bind(serde_json::to_value(policy).unwrap())
        .bind(serde_json::to_value(descriptor).unwrap())
        .execute(&world.db.pool)
        .await
        .unwrap();
    let env = world
        .app
        .call(MachineEnvironmentCreate, &[&machine])
        .token(&manager.token)
        .body(&MachineEnvironmentCreateBody {
            data_sources: None,
            home_id: manager.person.personal_org,
            name: "Preview service".into(),
            root: "/srv/ports-core".into(),
            home_node_id: None,
            remote_write: None,
            terminal: None,
            access: Some(orch_api_contract::binding::NewEnvironmentAccess::OnlyMe),
        })
        .send()
        .await
        .ok()
        .base
        .id;
    (world, manager, colleague, env, project, machine)
}

#[tokio::test]
async fn a_port_browser_entry_stops_with_its_session_and_environment_generation() {
    use tower::ServiceExt;

    for loss in ["session", "generation"] {
        let (world, manager, _, env, _, _) = setup().await;
        sqlx::query("UPDATE environments SET status='running', facts=jsonb_set(COALESCE(facts,'{}'::jsonb),'{ports}', $2::jsonb) WHERE id=$1")
            .bind(env)
            .bind(json!({"transports":["tcp"]}))
            .execute(&world.db.pool)
            .await
            .unwrap();
        let entry = world
            .app
            .call(EnvironmentPortEnter, &[&env, &"41042"])
            .token(&manager.token)
            .body(&EnvironmentPortEnterBody::default())
            .send()
            .await
            .ok();
        let url = url::Url::parse(&entry.url).unwrap();
        if loss == "session" {
            sqlx::query("UPDATE sessions SET revoked_at=$2 WHERE user_id=$1")
                .bind(manager.id())
                .bind(world.now())
                .execute(&world.db.pool)
                .await
                .unwrap();
        } else {
            sqlx::query("UPDATE environments SET generation=generation+1 WHERE id=$1")
                .bind(env)
                .execute(&world.db.pool)
                .await
                .unwrap();
        }
        let mut registry = orch_api_core::registry::Registry::new();
        orch_api_core::routes::mount(&mut registry);
        let router = orch_api_core::server::router(world.app.app.clone(), registry);
        let response = router
            .oneshot(
                http::Request::builder()
                    .uri(format!("{}?{}", url.path(), url.query().unwrap()))
                    .header("host", url.host_str().unwrap())
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            403,
            "a {loss} change invalidates the code"
        );
        assert!(response.headers().get(http::header::SET_COOKIE).is_none());
    }
}

#[tokio::test]
async fn a_closed_port_link_cannot_be_exchanged_for_a_browser_cookie() {
    use tower::ServiceExt;
    let (world, manager, _, env, _, _) = setup().await;
    links_asked(&world).await;
    let mut asked = request(41043, new_id(), "preview link");
    asked.kind = PortOpeningKind::Link;
    asked.place_id = None;
    asked.until = Some(Time::new(world.now() + chrono::Duration::hours(1)));
    asked.entry_methods = Some(vec![PortLinkMethod::Browser]);
    let plan = review(&world, &manager, env, &asked).await;
    let opened = confirm(&world, &manager, env, &plan, new_id()).await;
    let url = url::Url::parse(&opened.link.unwrap().browser_url_once.unwrap()).unwrap();
    sqlx::query("UPDATE port_openings SET state='closed' WHERE id=$1")
        .bind(opened.opening.id)
        .execute(&world.db.pool)
        .await
        .unwrap();
    let mut registry = orch_api_core::registry::Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let router = orch_api_core::server::router(world.app.app.clone(), registry);
    let response = router
        .oneshot(
            http::Request::builder()
                .uri(format!("{}?{}", url.path(), url.query().unwrap()))
                .header("host", url.host_str().unwrap())
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 403);
    assert!(response.headers().get(http::header::SET_COOKIE).is_none());
}
/// Sets `capability.policy` at the system for one port capability, with its approvers the holders of
/// Ports Manage on the environment, as shipped.
async fn port_policy(world: &World, key: &str, decision: &str) {
    world
        .set(
            "capability.policy",
            json!({ key: { "decision": decision, "approver_scope": "environment" } }),
        )
        .await;
}

/// Lets people open links (shipped denied): the system's policy asks for `port_link`.
async fn links_asked(world: &World) {
    port_policy(world, "port_link", "ask").await;
}

fn request(port: u16, place: Id, reason: &str) -> PortOpeningRequest {
    PortOpeningRequest {
        port,
        kind: PortOpeningKind::Place,
        place_id: Some(place),
        label: None,
        transports: None,
        entry_methods: None,
        until: None,
        host_port: None,
        program: None,
        reason: reason.into(),
        replacement_of: None,
        close_replaced: None,
    }
}
async fn review(
    world: &World,
    manager: &Who,
    env: Id,
    request: &PortOpeningRequest,
) -> PortOpeningPlan {
    world
        .app
        .call(
            EnvironmentPortOpeningPreflight,
            &[&env, &request.port.to_string()],
        )
        .token(&manager.token)
        .body(request)
        .send()
        .await
        .ok()
}
async fn confirm(
    world: &World,
    manager: &Who,
    env: Id,
    plan: &PortOpeningPlan,
    client: Id,
) -> PortOpenReply {
    world
        .app
        .call(
            EnvironmentPortOpeningCreate,
            &[&env, &plan.request.port.to_string()],
        )
        .token(&manager.token)
        .body(&EnvironmentPortOpeningCreateBody {
            client_id: client,
            preflight_id: plan.preflight_id,
        })
        .send()
        .await
        .ok()
}
async fn reconcile(world: &World, env: Id) -> Vec<Id> {
    let mut tx = world.app.app.db.begin().await.unwrap();
    orch_db::queries::transaction_guard::lock_authority(&mut tx)
        .await
        .unwrap();
    let result = orch_engine::ports::reconcile(&mut tx, env, Time::new(world.now()))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    result
}

#[tokio::test]
async fn a_persons_port_opening_ends_with_its_deciding_credential() {
    use orch_api_core::middleware::authentication::{
        ACCESS_TOKEN_PREFIX, API_TOKEN_PREFIX, new_token, token_hash,
    };

    for loss in ["session", "token", "token_scopes"] {
        let (world, manager, _, env, _, _) = setup().await;
        links_asked(&world).await;

        // The same person also has a distinct session: its opening must survive the loss.
        let other_secret = new_token(ACCESS_TOKEN_PREFIX);
        let other_session = new_id();
        orch_db::queries::identity::testing::insert_session(
            &mut *world.conn().await,
            other_session,
            manager.id(),
            &token_hash(&other_secret),
            world.now() + chrono::Duration::days(400),
        )
        .await
        .unwrap();
        let mut other = manager.clone();
        other.token = other_secret;

        let mut decider = manager.clone();
        let token_id = if loss != "session" {
            let secret = new_token(API_TOKEN_PREFIX);
            let id = orch_db::queries::identity::testing::insert_api_token(
                &mut *world.conn().await,
                manager.id(),
                &token_hash(&secret),
            )
            .await
            .unwrap();
            sqlx::query("UPDATE api_tokens SET scopes=ARRAY['ports.manage'] WHERE id=$1")
                .bind(id)
                .execute(&world.db.pool)
                .await
                .unwrap();
            decider.token = secret;
            Some(id)
        } else {
            None
        };

        let mut asked = request(41051, new_id(), "token-bound opening");
        asked.kind = PortOpeningKind::Link;
        asked.place_id = None;
        asked.until = Some(Time::new(world.now() + chrono::Duration::hours(1)));
        let plan = review(&world, &decider, env, &asked).await;
        let first = confirm(&world, &decider, env, &plan, new_id()).await;
        asked.port = 41052;
        let plan = review(&world, &other, env, &asked).await;
        let second = confirm(&world, &other, env, &plan, new_id()).await;

        match loss {
            "session" => {
                sqlx::query("UPDATE sessions SET revoked_at=$2 WHERE id=(SELECT session_id FROM access_tokens WHERE token_hash=$1)")
                    .bind(token_hash(&manager.token)).bind(world.now()).execute(&world.db.pool).await.unwrap();
            }
            "token" => {
                sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
                    .bind(token_id.unwrap())
                    .bind(world.now())
                    .execute(&world.db.pool)
                    .await
                    .unwrap();
            }
            "token_scopes" => {
                sqlx::query("UPDATE api_tokens SET scopes=ARRAY[]::text[] WHERE id=$1")
                    .bind(token_id.unwrap())
                    .execute(&world.db.pool)
                    .await
                    .unwrap();
            }
            _ => unreachable!(),
        }
        assert_eq!(
            reconcile(&world, env).await,
            vec![first.opening.id],
            "{loss}"
        );
        let state: String = sqlx::query_scalar("SELECT state FROM port_openings WHERE id=$1")
            .bind(second.opening.id)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
        assert_ne!(
            state, "closed",
            "the independent session still holds its opening"
        );
    }
}

#[tokio::test]
async fn a_port_decider_cannot_keep_using_a_place_that_now_requires_their_second_step() {
    let (world, manager, _, env, project, _) = setup().await;
    let plan = review(
        &world,
        &manager,
        env,
        &request(41055, project, "team preview"),
    )
    .await;
    let opened = confirm(&world, &manager, env, &plan, new_id()).await;
    let org: Id = sqlx::query_scalar("SELECT org_id FROM nodes WHERE id=$1")
        .bind(project)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    world
        .set_at(
            orch_settings::Target::Node(org),
            "auth.second_step",
            json!("required"),
        )
        .await;
    assert_eq!(reconcile(&world, env).await, vec![opened.opening.id]);
}

#[tokio::test]
async fn a_port_opening_loses_administrator_reach_when_its_session_turns_the_mode_off() {
    let (world, _, colleague, env, _, _) = setup().await;
    links_asked(&world).await;
    world.make_admin(&colleague.person).await;
    sqlx::query("UPDATE sessions SET as_admin=true WHERE id=(SELECT session_id FROM access_tokens WHERE token_hash=$1)")
        .bind(orch_api_core::middleware::authentication::token_hash(&colleague.token))
        .execute(&world.db.pool).await.unwrap();
    let mut asked = request(41056, new_id(), "temporary admin review");
    asked.kind = PortOpeningKind::Link;
    asked.place_id = None;
    asked.until = Some(Time::new(world.now() + chrono::Duration::hours(1)));
    let plan = review(&world, &colleague, env, &asked).await;
    let opened = confirm(&world, &colleague, env, &plan, new_id()).await;
    sqlx::query("UPDATE sessions SET as_admin=false WHERE id=(SELECT session_id FROM access_tokens WHERE token_hash=$1)")
        .bind(orch_api_core::middleware::authentication::token_hash(&colleague.token))
        .execute(&world.db.pool).await.unwrap();
    assert_eq!(reconcile(&world, env).await, vec![opened.opening.id]);
}

#[tokio::test]
async fn a_directory_reviews_and_reserves_a_public_number_on_its_machine_and_rechecks_the_same_policy()
 {
    let (world, manager, _, env, project, machine) = setup().await;
    port_policy(&world, "port_host", "ask").await;
    let declaration = HostPorts {
        incoming_address: "dedicated-vm.example.test".into(),
        listen: "127.0.0.1".into(),
        ranges: vec![PortRange {
            from: 50000,
            to: 50002,
        }],
    };
    let ports = orch_link_contract::policy::MachinePorts {
        allowed: vec![PortOpeningKind::Host],
        publish: Some(declaration.clone()),
        limits: None,
    };
    sqlx::query("UPDATE machines SET facts=jsonb_set(facts,'{policy,ports}',$2) WHERE id=$1")
        .bind(machine)
        .bind(serde_json::to_value(&ports).unwrap())
        .execute(&world.db.pool)
        .await
        .unwrap();
    let mut wanted = request(8080, project, "Publish the directory's preview");
    wanted.kind = PortOpeningKind::Host;
    wanted.place_id = None;
    wanted.transports = Some(vec![PortTransport::Tcp]);
    wanted.host_port = Some(50000);
    let plan = review(&world, &manager, env, &wanted).await;
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    let candidate = plan.candidate.as_ref().unwrap();
    assert_eq!(candidate.machine.id, machine);
    assert!(
        candidate.host.is_none(),
        "a directory does not acquire a fabricated Host resource"
    );
    assert_eq!(candidate.host_revision, ports.revision());
    let opened = confirm(&world, &manager, env, &plan, new_id()).await;
    assert_eq!(opened.opening.publisher.as_ref().unwrap().id, machine);
    assert!(opened.opening.host.is_none());
    let (publisher, allocated): (Option<Id>, bool) =
        sqlx::query_as("SELECT publish_machine_id,host_allocated FROM port_openings WHERE id=$1")
            .bind(opened.opening.id)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(publisher, Some(machine));
    assert!(allocated);
    wanted.port = 8081;
    let taken = review(&world, &manager, env, &wanted).await;
    assert!(
        taken
            .blockers
            .iter()
            .any(|blocker| blocker.code == ErrorCode::PortsPortInUse.as_str())
    );
    wanted.host_port = Some(50001);
    let before_change = review(&world, &manager, env, &wanted).await;
    assert!(before_change.blockers.is_empty());
    sqlx::query("UPDATE machines SET facts=jsonb_set(facts,'{policy,ports,publish,incoming_address}','\"changed.example.test\"') WHERE id=$1")
        .bind(machine).execute(&world.db.pool).await.unwrap();
    let stale = world
        .app
        .call(
            EnvironmentPortOpeningCreate,
            &[&env, &wanted.port.to_string()],
        )
        .token(&manager.token)
        .body(&EnvironmentPortOpeningCreateBody {
            client_id: new_id(),
            preflight_id: before_change.preflight_id,
        })
        .send()
        .await
        .problem();
    assert_eq!(
        stale.code,
        ErrorCode::ConflictStaleRevision,
        "a review does not silently publish on a changed address"
    );
    sqlx::query("UPDATE environments SET status='stopped' WHERE id=$1")
        .bind(env)
        .execute(&world.db.pool)
        .await
        .unwrap();
    orch_api_core::host_ports::follow_environments(&world.app.app, false).await;
    let held = orch_db::queries::ports::publisher_ports_held(&mut *world.conn().await, machine)
        .await
        .unwrap();
    assert!(
        held.is_empty(),
        "stopping a directory releases its machine's public number too"
    );
    sqlx::query(
        "UPDATE machines SET facts=jsonb_set(facts,'{policy,ports,allowed}','[]') WHERE id=$1",
    )
    .bind(machine)
    .execute(&world.db.pool)
    .await
    .unwrap();
    let refused = review(&world, &manager, env, &wanted).await;
    assert!(
        refused
            .blockers
            .iter()
            .any(|blocker| blocker.message.contains("ports")
                && blocker.message.contains("environment role")
                && blocker.message.contains("operator"))
    );
}

#[tokio::test]
async fn rule_3_a_project_audience_before_listening_is_one_guarded_act_and_no_other_door() {
    let (world, manager, colleague, env, project, _) = setup().await;
    let target = world
        .app
        .call(EnvironmentPortGet, &[&env, &"9000"])
        .token(&manager.token)
        .query(&EnvironmentPortGetQuery::default())
        .send()
        .await
        .ok();
    assert!(
        target
            .address
            .unwrap()
            .contains(&format!("e{}-p9000.ports.test", env.simple()))
    );
    assert_eq!(target.revision, 0);
    assert_eq!(target.state, PortTargetState::Stopped);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM port_targets")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    let asked = request(9000, project, "let the project see the application");
    let plan = review(&world, &manager, env, &asked).await;
    assert!(plan.audience.sentence.contains("Preview"));
    assert_eq!(plan.audience.count, None);
    let client = new_id();
    let first = confirm(&world, &manager, env, &plan, client).await;
    let twice = confirm(&world, &manager, env, &plan, client).await;
    assert_eq!(first.opening.id, twice.opening.id);
    assert_eq!(twice.opening.state, PortOpeningState::Stopped);
    let changed_reason = review(
        &world,
        &manager,
        env,
        &request(9000, project, "same application, a clearer reason"),
    )
    .await;
    let canonical = confirm(&world, &manager, env, &changed_reason, new_id()).await;
    assert_eq!(canonical.opening.id, first.opening.id);
    let mismatch = world
        .app
        .call(EnvironmentPortOpeningCreate, &[&env, &"9000"])
        .token(&manager.token)
        .body(&EnvironmentPortOpeningCreateBody {
            client_id: client,
            preflight_id: changed_reason.preflight_id,
        })
        .send()
        .await
        .problem();
    assert_eq!(mismatch.code, ErrorCode::ConflictIdempotencyMismatch);
    let visible = world
        .app
        .call(PortOpeningGet, &[&first.opening.id])
        .token(&colleague.token)
        .send()
        .await
        .ok();
    assert!(!visible.can.close);
    assert!(visible.counts.is_none());
    let denied = world
        .app
        .call(EnvironmentPortList, &[&env])
        .token(&colleague.token)
        .query(&EnvironmentPortListQuery::default())
        .send()
        .await
        .problem();
    assert_eq!(denied.code, ErrorCode::AuthzDenied);
    let program_denied = world
        .app
        .call(EnvironmentPortProgramUpdate, &[&env, &"9000"])
        .token(&colleague.token)
        .body(&EnvironmentPortProgramUpdateBody {
            expected_revision: 0,
            program: PortProgram::default(),
        })
        .send()
        .await
        .problem();
    assert_eq!(program_denied.code, ErrorCode::AuthzDenied);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM port_targets")
            .fetch_one(&world.db.pool)
            .await
            .unwrap(),
        0
    );
    let env_view = world
        .app
        .call(EnvironmentGet, &[&env])
        .token(&manager.token)
        .send()
        .await
        .ok();
    assert!(env_view.base.ports_exposed);
    let audit: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action='ports.opened'")
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(audit, 1);
}

#[tokio::test]
async fn rule_3_an_empty_edge_domain_disables_browser_names_and_keeps_the_ordinary_helper_link() {
    let (world, manager, _, env, project, _) = setup_with_domain("").await;
    links_asked(&world).await;
    let target = world
        .app
        .call(EnvironmentPortGet, &[&env, &"9001"])
        .token(&manager.token)
        .query(&EnvironmentPortGetQuery::default())
        .send()
        .await
        .ok();
    assert!(target.address.is_none());
    assert!(
        target
            .own_entries
            .iter()
            .all(|entry| entry.method != PortEntryMethod::Browser)
    );
    let mut asked = request(9001, project, "one ordinary helper link");
    asked.kind = PortOpeningKind::Link;
    asked.place_id = None;
    asked.until = Some(Time::new(world.now() + chrono::Duration::hours(1)));
    let plan = review(&world, &manager, env, &asked).await;
    assert!(plan.blockers.is_empty());
    assert_eq!(
        plan.request.entry_methods,
        Some(vec![PortLinkMethod::Helper])
    );
    let opened = confirm(&world, &manager, env, &plan, new_id()).await;
    assert_eq!(opened.opening.entry_methods, vec![PortEntryMethod::Helper]);
    let delivered = opened.link.unwrap();
    assert!(delivered.browser_url_once.is_none());
    assert!(delivered.helper_link_once.is_some());
}

#[tokio::test]
async fn rule_3_program_revision_zero_becomes_one_and_a_stale_competing_review_refuses() {
    let (world, manager, _, env, project, _) = setup().await;
    let plan = review(&world, &manager, env, &request(9001, project, "preview")).await;
    let program = PortProgram {
        http_scheme: PortHttpScheme::Https,
        host_header: Some("dev.local:9001".into()),
        proxy_protocol: false,
    };
    let written = world
        .app
        .call(EnvironmentPortProgramUpdate, &[&env, &"9001"])
        .token(&manager.token)
        .body(&EnvironmentPortProgramUpdateBody {
            program: program.clone(),
            expected_revision: 0,
        })
        .send()
        .await
        .ok();
    assert_eq!(written.revision, 1);
    assert_eq!(written.program, program);
    let refused = world
        .app
        .call(EnvironmentPortOpeningCreate, &[&env, &"9001"])
        .token(&manager.token)
        .body(&EnvironmentPortOpeningCreateBody {
            client_id: new_id(),
            preflight_id: plan.preflight_id,
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::ConflictStaleRevision);
    let injection = world
        .app
        .call(EnvironmentPortProgramUpdate, &[&env, &"9001"])
        .token(&manager.token)
        .body(&EnvironmentPortProgramUpdateBody {
            program: PortProgram {
                host_header: Some("local\r\nX-Test: yes".into()),
                ..Default::default()
            },
            expected_revision: 1,
        })
        .send()
        .await
        .problem();
    assert_eq!(injection.code, ErrorCode::RequestInvalid);
}

#[tokio::test]
async fn rule_6_stop_generation_change_and_an_owned_restart_preserve_admission_but_close_never_revives()
 {
    let (world, manager, _, env, project, _) = setup().await;
    let plan = review(&world, &manager, env, &request(9002, project, "preview")).await;
    let opened = confirm(&world, &manager, env, &plan, new_id())
        .await
        .opening;
    for (status, generation) in [
        ("running", 1),
        ("stopped", 1),
        ("recreating", 2),
        ("running", 2),
    ] {
        sqlx::query("UPDATE environments SET status=$2,generation=$3 WHERE id=$1")
            .bind(env)
            .bind(status)
            .bind(generation)
            .execute(&world.db.pool)
            .await
            .unwrap();
        assert!(reconcile(&world, env).await.is_empty());
        let current = world
            .app
            .call(PortOpeningGet, &[&opened.id])
            .token(&manager.token)
            .send()
            .await
            .ok();
        assert_ne!(current.state, PortOpeningState::Closed);
        assert_eq!(current.revision, opened.revision);
    }
    let closed = world
        .app
        .call(PortOpeningDelete, &[&opened.id])
        .token(&manager.token)
        .body(&PortRevisionBody {
            expected_revision: opened.revision,
        })
        .send()
        .await
        .ok();
    assert_eq!(closed.state, PortOpeningState::Closed);
    let twice = world
        .app
        .call(PortOpeningDelete, &[&opened.id])
        .token(&manager.token)
        .body(&PortRevisionBody {
            expected_revision: opened.revision,
        })
        .send()
        .await
        .ok();
    assert_eq!(twice.revision, closed.revision);
    assert!(reconcile(&world, env).await.is_empty());
    let env_view = world
        .app
        .call(EnvironmentGet, &[&env])
        .token(&manager.token)
        .send()
        .await
        .ok();
    assert!(!env_view.base.ports_exposed);
    let audits: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action='ports.closed'")
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(audits, 1);
}

#[tokio::test]
async fn rule_6_local_policy_and_named_place_deletion_end_admission_irreversibly() {
    let (world, manager, _, env, project, machine) = setup().await;
    let plan = review(&world, &manager, env, &request(9003, project, "preview")).await;
    let opened = confirm(&world, &manager, env, &plan, new_id())
        .await
        .opening;
    sqlx::query(
        "UPDATE machines SET facts=jsonb_set(facts,'{policy,ports,allowed}','[]') WHERE id=$1",
    )
    .bind(machine)
    .execute(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(reconcile(&world, env).await, vec![opened.id]);
    sqlx::query("UPDATE machines SET facts=jsonb_set(facts,'{policy,ports,allowed}','[\"place\",\"link\"]') WHERE id=$1").bind(machine).execute(&world.db.pool).await.unwrap();
    assert!(reconcile(&world, env).await.is_empty());
    let plan = review(
        &world,
        &manager,
        env,
        &request(9004, project, "another preview"),
    )
    .await;
    let next = confirm(&world, &manager, env, &plan, new_id())
        .await
        .opening;
    sqlx::query("UPDATE nodes SET deleted_at=$2 WHERE id=$1")
        .bind(project)
        .bind(world.now())
        .execute(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(reconcile(&world, env).await, vec![next.id]);
}

#[tokio::test]
async fn rule_3_sixteen_distinct_ports_include_pending_and_concurrent_seventeenth_requests_cannot_both_pass()
 {
    let (world, manager, _, env, project, _) = setup().await;
    for port in 9010..9026 {
        let plan = review(&world, &manager, env, &request(port, project, "preview")).await;
        let opening = confirm(&world, &manager, env, &plan, new_id())
            .await
            .opening;
        if port == 9010 {
            sqlx::query("UPDATE port_openings SET state='waiting',allowed_by=NULL WHERE id=$1")
                .bind(opening.id)
                .execute(&world.db.pool)
                .await
                .unwrap();
        }
    }
    let a = review(&world, &manager, env, &request(9026, project, "extra")).await;
    let b = review(&world, &manager, env, &request(9027, project, "extra")).await;
    let body_a = EnvironmentPortOpeningCreateBody {
        client_id: new_id(),
        preflight_id: a.preflight_id,
    };
    let body_b = EnvironmentPortOpeningCreateBody {
        client_id: new_id(),
        preflight_id: b.preflight_id,
    };
    let (a, b) = tokio::join!(
        world
            .app
            .call(EnvironmentPortOpeningCreate, &[&env, &"9026"])
            .token(&manager.token)
            .body(&body_a)
            .send(),
        world
            .app
            .call(EnvironmentPortOpeningCreate, &[&env, &"9027"])
            .token(&manager.token)
            .body(&body_b)
            .send()
    );
    assert_eq!(a.problem().code, ErrorCode::PortsTooMany);
    assert_eq!(b.problem().code, ErrorCode::PortsTooMany);
    let used: i64 = sqlx::query_scalar("SELECT count(DISTINCT port) FROM port_openings")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(used, 16);
}

#[tokio::test]
async fn rule_6_a_persons_link_is_once_only_retains_its_digest_through_stop_and_lapses_without_traffic_audit()
 {
    let (world, manager, _, env, _, _) = setup().await;
    links_asked(&world).await;
    let request = PortOpeningRequest {
        kind: PortOpeningKind::Link,
        place_id: None,
        until: Some(Time::new(world.now() + chrono::Duration::hours(1))),
        entry_methods: Some(vec![PortLinkMethod::Helper]),
        ..request(9030, new_id(), "webhook")
    };
    let plan = review(&world, &manager, env, &request).await;
    let client = new_id();
    let first = confirm(&world, &manager, env, &plan, client).await;
    let secret = first.link.unwrap().helper_link_once.unwrap();
    let replay = confirm(&world, &manager, env, &plan, client).await;
    assert_eq!(first.opening.id, replay.opening.id);
    assert!(replay.link.is_none());
    let (digest, sealed): (Option<Vec<u8>>, Option<Vec<u8>>) =
        sqlx::query_as("SELECT secret_hash,secret_sealed FROM port_openings WHERE id=$1")
            .bind(first.opening.id)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert!(digest.is_some());
    assert!(sealed.is_none());
    let visible = world
        .app
        .call(PortOpeningGet, &[&first.opening.id])
        .token(&manager.token)
        .send()
        .await;
    assert!(!String::from_utf8_lossy(&visible.raw).contains(secret.split('#').nth(1).unwrap()));
    assert!(reconcile(&world, env).await.is_empty());
    world.advance(chrono::Duration::hours(2));
    assert_eq!(reconcile(&world, env).await, vec![first.opening.id]);
    let details: Vec<serde_json::Value> =
        sqlx::query_scalar("SELECT details FROM audit_log WHERE action LIKE 'ports.%'")
            .fetch_all(&world.db.pool)
            .await
            .unwrap();
    assert!(
        details
            .iter()
            .all(|d| !d.to_string().contains(secret.split('#').nth(1).unwrap()))
    );
}

#[tokio::test]
async fn rule_3_a_request_waiting_for_authority_reloads_the_suspended_manager_and_writes_nothing() {
    use tower::ServiceExt;
    let (world, manager, _, env, project, _) = setup().await;
    let plan = review(&world, &manager, env, &request(9040, project, "preview")).await;
    let body = EnvironmentPortOpeningCreateBody {
        client_id: new_id(),
        preflight_id: plan.preflight_id,
    };
    // No barrier is shared: the manager is suspended once the request's door has let it in and
    // before its body arrives, and the action asks for its authority again when it begins.
    let pool = world.db.pool.clone();
    let suspended = manager.id();
    let delivered = axum::body::Body::from_stream(futures::stream::once(async move {
        sqlx::query("UPDATE users SET status='suspended' WHERE id=$1")
            .bind(suspended)
            .execute(&pool)
            .await
            .unwrap();
        Ok::<_, std::io::Error>(bytes::Bytes::from(serde_json::to_vec(&body).unwrap()))
    }));
    let mut registry = orch_api_core::registry::Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let response = registry
        .into_router()
        .with_state(world.app.app.clone())
        .oneshot(
            http::Request::builder()
                .method("POST")
                .uri(format!("/environments/{env}/ports/9040/openings"))
                .header("authorization", format!("Bearer {}", manager.token))
                .header("content-type", "application/json")
                .body(delivered)
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let denied: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(denied["code"], "auth.suspended", "{denied}");
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM port_openings")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn rule_6_an_administrator_lists_stopped_outside_admission_and_closes_it_only_as_admin() {
    let (world, manager, admin, env, project, _) = setup().await;
    let plan = review(&world, &manager, env, &request(9050, project, "preview")).await;
    let opening = confirm(&world, &manager, env, &plan, new_id())
        .await
        .opening;
    sqlx::query("UPDATE users SET is_system_admin=true WHERE id=$1")
        .bind(admin.id())
        .execute(&world.db.pool)
        .await
        .unwrap();
    // With the session's As admin switch off a system admin holds nothing of the environment.
    let refused = world
        .app
        .call(PortOpeningDelete, &[&opening.id])
        .token(&admin.token)
        .body(&PortRevisionBody {
            expected_revision: opening.revision,
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    // Acting as one, they hold View and Manage natively, Ports Manage among them: they list what
    // is open outside admission and close it with no override of their own.
    places::admin_mode(&world, &admin, true).await;
    let outside = world
        .app
        .call(NodePortOpeningList, &[&manager.person.personal_org])
        .token(&admin.token)
        .query(&NodePortOpeningListQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(outside.items.len(), 1);
    assert_eq!(outside.items[0].id, opening.id);
    assert_eq!(outside.items[0].state, PortOpeningState::Stopped);
    assert!(outside.items[0].can.close);
    let closed = world
        .app
        .call(PortOpeningDelete, &[&opening.id])
        .token(&admin.token)
        .body(&PortRevisionBody {
            expected_revision: opening.revision,
        })
        .send()
        .await
        .ok();
    assert_eq!(closed.state, PortOpeningState::Closed);
    let used_override: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action='permission.added' AND override",
    )
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(used_override, 0, "native reach names nobody on the page");
}

#[tokio::test]
async fn rules_3_6_an_agents_approved_request_is_one_act_and_only_that_requester_can_close_it_after_leaving()
 {
    use orch_engine::ports::{self as core, Requester};
    let (world, manager, _, env, project, _) = setup().await;
    let topic = places::make_topic(&world, &manager, project, "Try the service")
        .await
        .base
        .id;
    let thread = new_id();
    let another = new_id();
    {
        let mut conn = world.conn().await;
        for id in [thread, another] {
            assert!(
                orch_db::queries::threads::insert_thread(
                    &mut conn,
                    id,
                    topic,
                    "requesting agent",
                    manager.id(),
                    world.now()
                )
                .await
                .unwrap()
            );
            orch_db::queries::threads::set_environment(&mut conn, id, env, "", 0)
                .await
                .unwrap();
        }
    }
    let asked = request(9051, project, "let colleagues try it");
    let client = new_id();
    let mut tx = world.app.app.db.begin().await.unwrap();
    orch_db::queries::transaction_guard::lock_authority(&mut tx)
        .await
        .unwrap();
    let principal = orch_authz::Principal::load(
        &mut tx,
        orch_authz::tokens::Caller::session(manager.id()),
        Time::new(world.now()),
    )
    .await
    .unwrap();
    let environment = orch_db::queries::environments::routes::environments(&mut tx, &[env])
        .await
        .unwrap()
        .remove(0);
    let resolved = core::resolve_request(&mut tx, &principal, &environment, &asked, true)
        .await
        .unwrap();
    let body = serde_json::to_value(&asked).unwrap();
    let session_id: Id =
        sqlx::query_scalar("SELECT session_id FROM access_tokens WHERE token_hash=$1")
            .bind(orch_api_core::middleware::authentication::token_hash(
                &manager.token,
            ))
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    let credential = core::DecisionCredential::Session {
        id: session_id,
        as_admin: false,
    };
    let first = core::produce(
        &mut tx,
        &principal,
        credential,
        &environment,
        Requester::Thread(thread),
        client,
        &body,
        &resolved,
        0,
        &Default::default(),
    )
    .await
    .unwrap();
    assert!(first.created);
    let twice = core::produce(
        &mut tx,
        &principal,
        credential,
        &environment,
        Requester::Thread(thread),
        client,
        &body,
        &resolved,
        0,
        &Default::default(),
    )
    .await
    .unwrap();
    assert!(!twice.created);
    assert_eq!(twice.opening.id, first.opening.id);
    let mut retry = resolved.clone();
    retry.reason = "a clearer reason for the same service".into();
    let canonical = core::produce(
        &mut tx,
        &principal,
        credential,
        &environment,
        Requester::Thread(thread),
        new_id(),
        &serde_json::to_value(&retry).unwrap(),
        &retry,
        0,
        &Default::default(),
    )
    .await
    .unwrap();
    assert_eq!(canonical.opening.id, first.opening.id);
    tx.commit().await.unwrap();
    assert!(reconcile(&world, env).await.is_empty());
    sqlx::query("UPDATE threads SET environment_id=NULL WHERE node_id=$1")
        .bind(thread)
        .execute(&world.db.pool)
        .await
        .unwrap();
    let mut tx = world.app.app.db.begin().await.unwrap();
    orch_db::queries::transaction_guard::lock_authority(&mut tx)
        .await
        .unwrap();
    assert!(
        core::close_requested(&mut tx, another, first.opening.id, Time::new(world.now()))
            .await
            .is_err()
    );
    let closed = core::close_requested(&mut tx, thread, first.opening.id, Time::new(world.now()))
        .await
        .unwrap();
    assert_eq!(closed.state, "closed");
    let repeated = core::close_requested(&mut tx, thread, first.opening.id, Time::new(world.now()))
        .await
        .unwrap();
    assert_eq!(repeated.revision, closed.revision);
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn rule_6_a_named_place_deleted_and_restored_at_once_ended_its_admission_before_the_delete_answered()
 {
    use orch_api_contract::ownership::{NodeDelete, NodeDeleteBody, NodeRestore, NodeRestoreBody};
    let (world, manager, _, env, project, _) = setup().await;
    let plan = review(
        &world,
        &manager,
        env,
        &request(9004, project, "the project's preview"),
    )
    .await;
    let opened = confirm(&world, &manager, env, &plan, new_id()).await;
    let deleted = world
        .app
        .call(NodeDelete, &[&project])
        .token(&manager.token)
        .body(&NodeDeleteBody::default())
        .send()
        .await;
    assert_eq!(deleted.status, 200, "{:?}", deleted.body);
    // No background pass runs in this world: the delete itself closed the admission it ended.
    let state: String = sqlx::query_scalar("SELECT state FROM port_openings WHERE id=$1")
        .bind(opened.opening.id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(state, "closed");
    let restored = world
        .app
        .call(NodeRestore, &[&project])
        .token(&manager.token)
        .body(&NodeRestoreBody::default())
        .send()
        .await;
    assert_eq!(restored.status, 200, "{:?}", restored.body);
    let after = world
        .app
        .call(PortOpeningGet, &[&opened.opening.id])
        .token(&manager.token)
        .send()
        .await
        .ok();
    assert_eq!(after.state, PortOpeningState::Closed);
    assert!(reconcile(&world, env).await.is_empty());
}

/// A thread of the manager's in the environment, as an agent's thread is.
async fn asking_thread(world: &World, manager: &Who, project: Id, env: Id) -> Id {
    let topic = places::make_topic(world, manager, project, "Share the preview")
        .await
        .base
        .id;
    let thread = new_id();
    let mut conn = world.conn().await;
    assert!(
        orch_db::queries::threads::insert_thread(
            &mut conn,
            thread,
            topic,
            "asking agent",
            manager.id(),
            world.now()
        )
        .await
        .unwrap()
    );
    // Its page is started as making a thread starts it: its maker is in its Owners row.
    orch_authz::pages::start(
        &mut conn,
        orch_access_contract::pages::StoredKind::Node,
        thread,
        manager.id(),
        Time::new(world.now()),
    )
    .await
    .unwrap();
    orch_db::queries::threads::set_environment(&mut conn, thread, env, "", 0)
        .await
        .unwrap();
    thread
}

/// A decision on a port card, for the revision of the opening that was reviewed.
fn port_decision(
    decision: PortDecisionChoice,
    recipient: Option<PortLinkRecipient>,
    revision: orch_types::Revision,
) -> orch_api_contract::approvals::ApprovalDecision {
    orch_api_contract::approvals::ApprovalDecision::Port(PortDecision {
        decision,
        link_recipient: recipient,
        expected_revision: revision,
    })
}

#[tokio::test]
async fn a_once_decision_keeps_its_session_but_a_capability_grant_keeps_its_own_authority() {
    use orch_api_contract::approvals::ApprovalDecide;
    let (world, manager, _, env, project, _) = setup().await;
    links_asked(&world).await;
    let thread = asking_thread(&world, &manager, project, env).await;
    let mut asked = PortOpeningRequest {
        kind: PortOpeningKind::Link,
        place_id: None,
        until: Some(Time::new(world.now() + chrono::Duration::hours(1))),
        entry_methods: Some(vec![PortLinkMethod::Helper]),
        ..request(41053, project, "a service callback")
    };
    let once = world
        .app
        .call(ThreadPortRequestCreate, &[&thread])
        .token(&manager.token)
        .body(&asked)
        .send()
        .await
        .ok();
    asked.port = 41054;
    let grant = world
        .app
        .call(ThreadPortRequestCreate, &[&thread])
        .token(&manager.token)
        .body(&asked)
        .send()
        .await
        .ok();
    for (made, choice) in [
        (&once, PortDecisionChoice::Once),
        (&grant, PortDecisionChoice::Grant),
    ] {
        world
            .app
            .call(ApprovalDecide, &[&made.request.as_ref().unwrap().id])
            .token(&manager.token)
            .body(&port_decision(choice, None, made.opening.revision))
            .send()
            .await
            .ok();
    }
    sqlx::query("UPDATE sessions SET revoked_at=$2 WHERE id=(SELECT session_id FROM access_tokens WHERE token_hash=$1)")
        .bind(orch_api_core::middleware::authentication::token_hash(&manager.token))
        .bind(world.now()).execute(&world.db.pool).await.unwrap();
    assert_eq!(reconcile(&world, env).await, vec![once.opening.id]);
    let state: String = sqlx::query_scalar("SELECT state FROM port_openings WHERE id=$1")
        .bind(grant.opening.id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_ne!(
        state, "closed",
        "the separate capability grant remains admitted"
    );
}

#[tokio::test]
async fn b3_a_threads_link_card_kept_by_its_decider_gives_the_link_in_the_decision_alone() {
    use orch_api_contract::approvals::{ApprovalDecide, ApprovalState};
    let (world, manager, _, env, project, _) = setup().await;
    links_asked(&world).await;
    let thread = asking_thread(&world, &manager, project, env).await;
    let asked = PortOpeningRequest {
        kind: PortOpeningKind::Link,
        place_id: None,
        until: Some(Time::new(world.now() + chrono::Duration::hours(1))),
        entry_methods: Some(vec![PortLinkMethod::Helper]),
        ..request(9071, project, "a reviewer tries the preview")
    };
    let made = world
        .app
        .call(ThreadPortRequestCreate, &[&thread])
        .token(&manager.token)
        .body(&asked)
        .send()
        .await
        .ok();
    assert_eq!(made.opening.state, PortOpeningState::Waiting);
    assert!(made.link.is_none(), "a thread's request carries no link");
    let card = made
        .request
        .clone()
        .expect("the request waits for a decision");
    let again = world
        .app
        .call(ThreadPortRequestCreate, &[&thread])
        .token(&manager.token)
        .body(&asked)
        .send()
        .await
        .ok();
    assert_eq!(
        again.opening.id, made.opening.id,
        "the same request makes no second card"
    );

    let decided = world
        .app
        .call(ApprovalDecide, &[&card.id])
        .token(&manager.token)
        .body(&port_decision(
            PortDecisionChoice::Once,
            Some(PortLinkRecipient::Person),
            made.opening.revision,
        ))
        .send()
        .await
        .ok();
    assert_eq!(decided.state, ApprovalState::Approved);
    let result = decided
        .opening_result
        .clone()
        .expect("the opening as the decision left it");
    assert_eq!(result.opening.id, made.opening.id);
    assert_ne!(result.opening.state, PortOpeningState::Waiting);
    let link = result.link.expect("the kept link, once");
    assert_eq!(link.recipient, PortLinkRecipient::Person);
    assert!(
        link.helper_link_once
            .as_deref()
            .is_some_and(|url| url.contains(&format!("/api/v1/ports/links/{}#", made.opening.id)))
    );
    let (digest, sealed, recipient): (Option<Vec<u8>>, Option<Vec<u8>>, Option<String>) =
        sqlx::query_as(
            "SELECT secret_hash, secret_sealed, link_recipient FROM port_openings WHERE id = $1",
        )
        .bind(made.opening.id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert!(digest.is_some(), "the link lets its holder in");
    assert!(
        sealed.is_none(),
        "nothing of the link is kept for the thread to read"
    );
    assert_eq!(recipient.as_deref(), Some("person"));

    let twice = world
        .app
        .call(ApprovalDecide, &[&card.id])
        .token(&manager.token)
        .body(&port_decision(
            PortDecisionChoice::Once,
            Some(PortLinkRecipient::Person),
            made.opening.revision,
        ))
        .send()
        .await
        .problem();
    assert_eq!(twice.code, ErrorCode::ApprovalAlreadyDecided);
}

#[tokio::test]
async fn b3_a_refused_card_ends_its_opening_and_the_same_request_is_refused_again_without_a_card() {
    use orch_api_contract::approvals::{ApprovalDecide, ApprovalState};
    let (world, manager, _, env, project, _) = setup().await;
    let thread = asking_thread(&world, &manager, project, env).await;
    let asked = request(9072, project, "let colleagues try it");
    let made = world
        .app
        .call(ThreadPortRequestCreate, &[&thread])
        .token(&manager.token)
        .body(&asked)
        .send()
        .await
        .ok();
    let card = made.request.clone().expect("a card");
    let denied = world
        .app
        .call(ApprovalDecide, &[&card.id])
        .token(&manager.token)
        .body(&port_decision(
            PortDecisionChoice::Deny,
            None,
            made.opening.revision,
        ))
        .send()
        .await
        .ok();
    assert_eq!(denied.state, ApprovalState::Denied);
    let result = denied.opening_result.clone().expect("the refused opening");
    assert_eq!(result.opening.state, PortOpeningState::Refused);
    assert!(result.link.is_none());
    let replayed = world
        .app
        .call(ThreadPortRequestCreate, &[&thread])
        .token(&manager.token)
        .body(&asked)
        .send()
        .await
        .ok();
    assert_eq!(
        replayed.opening.id, made.opening.id,
        "a refusal stands for an hour"
    );
    assert_eq!(replayed.opening.state, PortOpeningState::Refused);
    let cards: i64 = sqlx::query_scalar("SELECT count(*) FROM approvals WHERE kind = 'port'")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(cards, 1, "no second card");
}

#[tokio::test]
async fn b3_the_agent_learns_an_unstarted_ports_address_and_a_persons_no_is_a_structured_outcome() {
    use orch_api_contract::approvals::ApprovalDecide;
    let (world, manager, _, env, project, _) = setup().await;
    let id = asking_thread(&world, &manager, project, env).await;
    let row = orch_db::queries::threads::thread(&mut *world.conn().await, id)
        .await
        .unwrap()
        .unwrap();
    let tools = world
        .app
        .app
        .engine
        .port_tools()
        .expect("the API attached its tools");
    let listed: EnvironmentPorts =
        serde_json::from_value(tools.list_ports(&row, &json!({"port":9081})).await.unwrap())
            .unwrap();
    assert_eq!(listed.environment_id, env);
    assert_eq!(listed.targets.len(), 1);
    assert!(
        serde_json::to_string(&listed.targets[0])
            .unwrap()
            .contains(&format!("e{}-p9081.ports.test", env.simple()))
    );
    let task = {
        let tools = tools.clone();
        let row = row.clone();
        tokio::spawn(async move {
            tools
                .open_port(
                    &row,
                    &json!({"port":9081,"kind":"place","reason":"try the service"}),
                )
                .await
        })
    };
    // This bounds a hang, not the speed of creating a card on a loaded test machine.
    let card = tokio::time::timeout(std::time::Duration::from_secs(60), async {
        loop {
            let row: Option<(Id, i64)> = sqlx::query_as("SELECT approval_id,revision FROM port_openings WHERE requested_by_thread=$1 AND state='waiting'")
                .bind(id).fetch_optional(&world.db.pool).await.unwrap();
            if let Some(card) = row { break card }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    }).await.expect("the card is created");
    let decided = world
        .app
        .call(ApprovalDecide, &[&card.0])
        .token(&manager.token)
        .body(&port_decision(PortDecisionChoice::Deny, None, card.1))
        .send()
        .await
        .ok();
    assert_eq!(
        decided.opening_result.as_ref().unwrap().opening.state,
        PortOpeningState::Refused
    );
    let result: PortOpenReply = serde_json::from_value(
        task.await
            .unwrap()
            .expect("a human No is a result, not a tool error"),
    )
    .unwrap();
    assert_eq!(result.opening.state, PortOpeningState::Refused);
    assert!(result.link.is_none());
    let repeated: PortOpenReply = serde_json::from_value(
        tools
            .open_port(
                &row,
                &json!({"port":9081,"kind":"place","reason":"a clearer explanation"}),
            )
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(repeated.opening.id, result.opening.id);
    let cards: i64 =
        sqlx::query_scalar("SELECT count(*) FROM approvals WHERE kind='port' AND thread_id=$1")
            .bind(id)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(cards, 1);
}

#[tokio::test]
async fn d360_a_granted_port_place_opens_for_the_agent_without_a_card_and_only_its_requester_can_close_after_leaving()
 {
    let (world, manager, _, env, project, _) = setup().await;
    let id = asking_thread(&world, &manager, project, env).await;
    let another = asking_thread(&world, &manager, project, env).await;
    let granted = world
        .app
        .call(EnvironmentPortCapabilityGrant, &[&env, &"port_place"])
        .token(&manager.token)
        .send()
        .await
        .ok();
    assert!(granted.granted && granted.can_revoke);
    let source = granted.admission.as_ref().map(|admission| admission.source);
    assert_eq!(source, Some(PortAdmissionSource::Grant));
    let mut conn = world.conn().await;
    let row = orch_db::queries::threads::thread(&mut conn, id)
        .await
        .unwrap()
        .unwrap();
    let other = orch_db::queries::threads::thread(&mut conn, another)
        .await
        .unwrap()
        .unwrap();
    drop(conn);
    let tools = world.app.app.engine.port_tools().unwrap();
    let opened: PortOpenReply = serde_json::from_value(
        tools
            .open_port(
                &row,
                &json!({"port":9082,"kind":"place","reason":"the manager granted previews"}),
            )
            .await
            .unwrap(),
    )
    .unwrap();
    assert_ne!(opened.opening.state, PortOpeningState::Waiting);
    assert!(opened.request.is_none(), "no card was made");
    assert!(opened.opening.allowed_by.is_none(), "no person decided it");
    let admission = opened
        .opening
        .admission
        .clone()
        .expect("the grant that admitted it");
    assert_eq!(admission.capability, "port_place");
    assert_eq!(admission.source, PortAdmissionSource::Grant);
    assert_eq!(
        admission.by.map(|by| by.id),
        Some(manager.id()),
        "whose grant it was"
    );
    let cards: i64 = sqlx::query_scalar("SELECT count(*) FROM approvals WHERE kind='port'")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(cards, 0);
    let audit: String = sqlx::query_scalar("SELECT actor_kind FROM audit_log WHERE action='ports.opened' AND details->>'opening_id'=$1")
        .bind(opened.opening.id.to_string()).fetch_one(&world.db.pool).await.unwrap();
    assert_eq!(
        audit, "agent",
        "the trail names the agent that opened it under the grant"
    );
    let listed: EnvironmentPorts =
        serde_json::from_value(tools.list_ports(&row, &json!({})).await.unwrap()).unwrap();
    let place = listed
        .capabilities
        .iter()
        .find(|capability| capability.kind == PortOpeningKind::Place)
        .unwrap();
    assert!(place.granted, "the Ports view shows the grant");
    let refused = tools
        .close_port(&other, &json!({"opening":opened.opening.id}))
        .await
        .unwrap_err();
    assert_eq!(
        refused.code,
        orch_engine::tools::definitions::Code::NotYours
    );
    sqlx::query("UPDATE threads SET environment_id=NULL WHERE node_id=$1")
        .bind(id)
        .execute(&world.db.pool)
        .await
        .unwrap();
    let closed: PortOpening = serde_json::from_value(
        tools
            .close_port(&row, &json!({"opening":opened.opening.id}))
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(closed.state, PortOpeningState::Closed);
    let twice: PortOpening = serde_json::from_value(
        tools
            .close_port(&row, &json!({"opening":opened.opening.id}))
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(twice.revision, closed.revision);
}

#[tokio::test]
async fn b3_a_late_decision_is_told_once_to_an_idle_thread_without_starting_a_turn_or_releasing_its_queue()
 {
    use orch_api_contract::approvals::ApprovalDecide;
    let (world, manager, _, env, project, _) = setup().await;
    let id = asking_thread(&world, &manager, project, env).await;
    let made = world
        .app
        .call(ThreadPortRequestCreate, &[&id])
        .token(&manager.token)
        .body(&request(9083, project, "a late preview"))
        .send()
        .await
        .ok();
    world.advance(chrono::Duration::seconds(61));
    world
        .app
        .call(ApprovalDecide, &[&made.request.as_ref().unwrap().id])
        .token(&manager.token)
        .body(&port_decision(
            PortDecisionChoice::Deny,
            None,
            made.opening.revision,
        ))
        .send()
        .await
        .ok();
    orch_api_core::port_tools::tell_late(&world.app.app)
        .await
        .unwrap();
    orch_api_core::port_tools::tell_late(&world.app.app)
        .await
        .unwrap();
    let row = orch_db::queries::threads::thread(&mut *world.conn().await, id)
        .await
        .unwrap()
        .unwrap();
    let notices = row.pending_notices.as_array().unwrap();
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0]["kind"], "port_outcome");
    let text = notices[0]["text"].as_str().unwrap();
    assert!(text.contains("9083") && text.contains("refused"));
    let turns: i64 = sqlx::query_scalar("SELECT count(*) FROM turns WHERE thread_id=$1")
        .bind(id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    let inputs: i64 = sqlx::query_scalar("SELECT count(*) FROM thread_inputs WHERE thread_id=$1")
        .bind(id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!((turns, inputs), (0, 0), "a late decision starts no work");
}

#[tokio::test]
async fn port_tools_consume_common_options_and_refuse_a_small_change_before_its_card_or_close_commits()
 {
    use orch_engine::tools::definitions::Code;
    let (world, manager, _, env, project, _) = setup().await;
    // A valid long place name appears in the readable place and label; UTF-8 bytes count.
    sqlx::query("UPDATE nodes SET name=$2 WHERE id=$1")
        .bind(project)
        .bind("界".repeat(100))
        .execute(&world.db.pool)
        .await
        .unwrap();
    let id = asking_thread(&world, &manager, project, env).await;
    let row = orch_db::queries::threads::thread(&mut *world.conn().await, id)
        .await
        .unwrap()
        .unwrap();
    let tools = world.app.app.engine.port_tools().unwrap();
    let arguments =
        json!({"port":9086,"kind":"place","reason":"try a preview","max_bytes":1000,"ids":false});
    let refused = tools.open_port(&row, &arguments).await.unwrap_err();
    assert_eq!(
        refused.code,
        Code::TooLarge,
        "common answer options are not unknown request fields"
    );
    let counts:(i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM port_openings WHERE requested_by_thread=$1),(SELECT count(*) FROM approvals WHERE thread_id=$1 AND kind='port')")
        .bind(id).fetch_one(&world.db.pool).await.unwrap();
    assert_eq!(
        counts,
        (0, 0),
        "a too-small answer made neither an opening nor a card"
    );
    let page = tools
        .list_ports(&row, &json!({"port":9086,"max_bytes":40000,"ids":true}))
        .await
        .unwrap();
    assert!(orch_agent_contract::tool_answers::fits(
        "list_ports",
        &json!({"max_bytes":40000,"ids":true}),
        &page
    ));
    let page: EnvironmentPorts = serde_json::from_value(page).unwrap();
    assert_eq!(page.targets[0].port, 9086);
    world
        .app
        .call(EnvironmentPortCapabilityGrant, &[&env, &"port_place"])
        .token(&manager.token)
        .send()
        .await
        .ok();
    let mut allowed = arguments.clone();
    allowed["max_bytes"] = json!(40000);
    let opened: PortOpenReply =
        serde_json::from_value(tools.open_port(&row, &allowed).await.unwrap()).unwrap();
    assert!(opened.request.is_none());
    let refused = tools
        .close_port(
            &row,
            &json!({"opening":opened.opening.id,"max_bytes":1000,"ids":false}),
        )
        .await
        .unwrap_err();
    assert_eq!(refused.code, Code::TooLarge);
    let state: String = sqlx::query_scalar("SELECT state FROM port_openings WHERE id=$1")
        .bind(opened.opening.id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_ne!(
        state, "closed",
        "a size refusal rolled back the close and its audit"
    );
    let closed: PortOpening = serde_json::from_value(
        tools
            .close_port(
                &row,
                &json!({"opening":opened.opening.id,"max_bytes":40000,"ids":false}),
            )
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(closed.state, PortOpeningState::Closed);
}

#[tokio::test]
async fn d360_a_denied_kind_refuses_a_person_too_and_closes_what_is_open_of_it() {
    let (world, manager, _, env, project, _) = setup().await;
    let plan = review(
        &world,
        &manager,
        env,
        &request(9091, project, "the project's preview"),
    )
    .await;
    let opened = confirm(&world, &manager, env, &plan, new_id()).await;
    assert_ne!(opened.opening.state, PortOpeningState::Closed);
    port_policy(&world, "port_place", "deny").await;
    // Denied at the environment's place: what is open of the kind closes, and nobody opens another.
    assert_eq!(reconcile(&world, env).await, vec![opened.opening.id]);
    let again = review(
        &world,
        &manager,
        env,
        &request(9092, project, "another preview"),
    )
    .await;
    assert!(
        again
            .blockers
            .iter()
            .any(|blocker| blocker.code == ErrorCode::PortsNotAllowed.as_str()),
        "{:?}",
        again.blockers
    );
    let refused = world
        .app
        .call(EnvironmentPortOpeningCreate, &[&env, &"9092"])
        .token(&manager.token)
        .body(&EnvironmentPortOpeningCreateBody {
            client_id: new_id(),
            preflight_id: again.preflight_id,
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::PortsNotAllowed);
    let listed = world
        .app
        .call(EnvironmentPortList, &[&env])
        .token(&manager.token)
        .query(&EnvironmentPortListQuery::default())
        .send()
        .await
        .ok();
    let place = listed
        .allowed
        .iter()
        .find(|allowed| allowed.kind == PortOpeningKind::Place)
        .unwrap();
    assert!(!place.allowed);
    assert!(
        place
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("capability.policy"))
    );
    let capability = listed
        .capabilities
        .iter()
        .find(|capability| capability.kind == PortOpeningKind::Place)
        .unwrap();
    assert!(
        !capability.can_grant,
        "a denied capability is granted by nobody"
    );
}

#[tokio::test]
async fn d360_a_link_under_ask_makes_a_card_whose_grant_lets_the_next_link_open_without_one() {
    use orch_api_contract::approvals::{ApprovalDecide, ApprovalState};
    let (world, manager, _, env, project, _) = setup().await;
    links_asked(&world).await;
    let thread = asking_thread(&world, &manager, project, env).await;
    let until = Time::new(world.now() + chrono::Duration::hours(1));
    let link = |port: u16| PortOpeningRequest {
        kind: PortOpeningKind::Link,
        place_id: None,
        until: Some(until),
        entry_methods: Some(vec![PortLinkMethod::Helper]),
        ..request(port, project, "a service outside calls back")
    };
    let made = world
        .app
        .call(ThreadPortRequestCreate, &[&thread])
        .token(&manager.token)
        .body(&link(9093))
        .send()
        .await
        .ok();
    assert_eq!(
        made.opening.state,
        PortOpeningState::Waiting,
        "port_link under ask makes a card"
    );
    let card_id = made.request.as_ref().expect("a card").id;
    let card: serde_json::Value = sqlx::query_scalar("SELECT request FROM approvals WHERE id=$1")
        .bind(card_id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        card["can_grant"],
        json!(true),
        "the card offers the grant to its deciders"
    );
    let decided = world
        .app
        .call(ApprovalDecide, &[&card_id])
        .token(&manager.token)
        .body(&port_decision(
            PortDecisionChoice::Grant,
            None,
            made.opening.revision,
        ))
        .send()
        .await
        .ok();
    assert_eq!(decided.state, ApprovalState::Approved);
    let result = decided.opening_result.clone().unwrap();
    assert!(
        result.link.is_none(),
        "a link for the thread is the thread's, not the decider's"
    );
    let admission = result
        .opening
        .admission
        .clone()
        .expect("admitted under the grant it gave");
    assert_eq!(admission.source, PortAdmissionSource::Grant);
    assert_eq!(admission.by.map(|by| by.id), Some(manager.id()));
    assert_eq!(
        result.opening.allowed_by.as_ref().map(|by| by.id),
        Some(manager.id())
    );
    let granted: Vec<String> =
        sqlx::query_scalar("SELECT granted_capabilities FROM environments WHERE id=$1")
            .bind(env)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert!(granted.contains(&"port_link".to_owned()));
    let next = world
        .app
        .call(ThreadPortRequestCreate, &[&thread])
        .token(&manager.token)
        .body(&link(9094))
        .send()
        .await
        .ok();
    assert_ne!(
        next.opening.state,
        PortOpeningState::Waiting,
        "the grant opens the next link at once"
    );
    assert!(next.request.is_none());
    let (sealed, recipient): (Option<Vec<u8>>, Option<String>) =
        sqlx::query_as("SELECT secret_sealed, link_recipient FROM port_openings WHERE id=$1")
            .bind(next.opening.id)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert!(sealed.is_some(), "kept sealed until the thread is told");
    assert_eq!(recipient.as_deref(), Some("thread"));
    let cards: i64 = sqlx::query_scalar("SELECT count(*) FROM approvals WHERE kind='port'")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(cards, 1);
}

#[tokio::test]
async fn d360_taking_a_grant_back_makes_the_next_request_ask_and_leaves_what_it_opened() {
    let (world, manager, _, env, project, _) = setup().await;
    let thread = asking_thread(&world, &manager, project, env).await;
    world
        .app
        .call(EnvironmentPortCapabilityGrant, &[&env, &"port_place"])
        .token(&manager.token)
        .send()
        .await
        .ok();
    let opened = world
        .app
        .call(ThreadPortRequestCreate, &[&thread])
        .token(&manager.token)
        .body(&request(9096, project, "a granted preview"))
        .send()
        .await
        .ok();
    assert_ne!(opened.opening.state, PortOpeningState::Waiting);
    // Taken back as every capability is; the gateway gives it, so no unit is made anew.
    let job = world
        .app
        .call(
            orch_api_contract::environment::EnvironmentCapabilityRevoke,
            &[&env, &"port_place"],
        )
        .token(&manager.token)
        .send()
        .await
        .ok();
    assert!(
        job.is_none(),
        "no recreation for a capability the gateway gives"
    );
    let listed = world
        .app
        .call(EnvironmentPortList, &[&env])
        .token(&manager.token)
        .query(&EnvironmentPortListQuery::default())
        .send()
        .await
        .ok();
    let place = listed
        .capabilities
        .iter()
        .find(|capability| capability.kind == PortOpeningKind::Place)
        .unwrap();
    assert!(!place.granted && place.admission.is_none() && place.can_grant);
    assert!(
        reconcile(&world, env).await.is_empty(),
        "what the grant opened stays open"
    );
    let kept = world
        .app
        .call(PortOpeningGet, &[&opened.opening.id])
        .token(&manager.token)
        .send()
        .await
        .ok();
    assert_ne!(kept.state, PortOpeningState::Closed);
    let asked = world
        .app
        .call(ThreadPortRequestCreate, &[&thread])
        .token(&manager.token)
        .body(&request(9097, project, "another preview"))
        .send()
        .await
        .ok();
    assert_eq!(
        asked.opening.state,
        PortOpeningState::Waiting,
        "once taken back, the agent asks again"
    );
    let mut actions: Vec<String> = sqlx::query_scalar(
        "SELECT action FROM audit_log WHERE environment_id=$1 AND action LIKE 'capability.%'",
    )
    .bind(env)
    .fetch_all(&world.db.pool)
    .await
    .unwrap();
    actions.sort();
    assert_eq!(actions, vec!["capability.granted", "capability.revoked"]);
}

#[tokio::test]
async fn d357_no_approval_mode_decides_a_port_and_its_card_lives_the_shortest_requests_expiry() {
    let (world, manager, _, env, project, _) = setup().await;
    world
        .set_at(
            orch_settings::Target::Node(project),
            "approval.mode",
            json!("bypass"),
        )
        .await;
    world.set("requests.expiry_hours", json!(2)).await;
    let thread = asking_thread(&world, &manager, project, env).await;
    let made = world
        .app
        .call(ThreadPortRequestCreate, &[&thread])
        .token(&manager.token)
        .body(&request(
            9095,
            project,
            "a preview from a thread whose harness never asks",
        ))
        .send()
        .await
        .ok();
    assert_eq!(
        made.opening.state,
        PortOpeningState::Waiting,
        "a thread in bypass asks as one in ask does"
    );
    let card = made.request.clone().expect("a card");
    let expires = card.expires_at.expect("its card lapses");
    let off = expires.get() - (world.now() + chrono::Duration::hours(2));
    assert!(
        off.num_seconds().abs() <= 1,
        "requests.expiry_hours, not a port setting of its own: {off}"
    );
    world.advance(chrono::Duration::hours(2) + chrono::Duration::seconds(2));
    assert_eq!(reconcile(&world, env).await, vec![made.opening.id]);
    let state: String = sqlx::query_scalar("SELECT state FROM approvals WHERE id=$1")
        .bind(card.id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(state, "expired");
}
