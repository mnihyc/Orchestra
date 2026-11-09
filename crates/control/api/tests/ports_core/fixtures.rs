//! The same actual people, native registered environment and entry keys for port API source cases.
use crate::places::{Who, join, make_org, make_project, person};
use crate::support::World;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::environment::{MachineEnvironmentCreate, MachineEnvironmentCreateBody};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_db::queries::identity;
use orch_types::{Id, new_id};
use serde_json::json;

pub(super) async fn setup() -> (World, Who, Who, Id, Id, Id) {
    setup_with_domain("ports.test").await
}

pub(super) async fn setup_with_domain(domain: &str) -> (World, Who, Who, Id, Id, Id) {
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
            home_id: Some(manager.person.personal_org),
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
        .into_full()
        .expect("ordinary directory registration retains full native content")
        .base
        .id;
    (world, manager, colleague, env, project, machine)
}
pub(super) async fn token(
    world: &World,
    user: Id,
    environment: Id,
    operation: &str,
    actions: &[&str],
) -> (Id, String) {
    let scope:KeyScope=serde_json::from_value(json!({"rules":[{"target":{"kind":"environment","id":environment,"reach":"self"},"allow":[{"operation":operation,"actions":actions}]}]})).unwrap();
    assert!(scope.validate().is_empty());
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id,
            user_id: user,
            name: "Native browser port entry",
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
    (id, secret)
}
