//! Stored openings govern the same port writers and views under an extra credential boundary.

use super::*;
use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_db::queries::{identity, ports as rows};
use orch_types::Json;
use std::collections::BTreeSet;

const PORT: u16 = 9310;
const LABEL: &str = "Private opening label";

#[derive(Clone, Copy, Debug)]
enum Operation {
    Get,
    Update,
    Delete,
}

impl Operation {
    const WRITERS: [Self; 2] = [Self::Update, Self::Delete];
    fn name(self) -> &'static str {
        match self {
            Self::Get => "PortOpeningGet",
            Self::Update => "PortOpeningUpdate",
            Self::Delete => "PortOpeningDelete",
        }
    }
    fn action(self) -> &'static str {
        if matches!(self, Self::Get) {
            "read"
        } else {
            "manage"
        }
    }
    async fn call(self, scene: &Scene, opening: &PortOpening, token: &str) -> RawReply {
        match self {
            Self::Get => {
                scene
                    .world
                    .app
                    .call(PortOpeningGet, &[&opening.id])
                    .token(token)
                    .send_bytes()
                    .await
            }
            Self::Update => {
                scene
                    .world
                    .app
                    .call(PortOpeningUpdate, &[&opening.id])
                    .token(token)
                    .body(&PortOpeningUpdateBody {
                        until: Some(Time::new(scene.world.now() + chrono::Duration::hours(2))),
                        expected_revision: opening.revision,
                    })
                    .send_bytes()
                    .await
            }
            Self::Delete => {
                scene
                    .world
                    .app
                    .call(PortOpeningDelete, &[&opening.id])
                    .token(token)
                    .body(&PortRevisionBody {
                        expected_revision: opening.revision,
                    })
                    .send_bytes()
                    .await
            }
        }
    }
}

struct Key {
    id: Id,
    secret: String,
    user: Id,
}

fn rule(operation: &str, kind: &str, id: Id, reach: &str, actions: &[&str]) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":reach},"allow":[{"operation":operation,"actions":actions}]})
}
fn env_rule(operation: Operation, environment: Id, actions: &[&str]) -> Json {
    rule(
        operation.name(),
        "environment",
        environment,
        "self",
        actions,
    )
}

async fn key(scene: &Scene, user: Id, rules: Vec<Json>) -> Key {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(
        scope.validate().is_empty(),
        "valid credential restriction fixture"
    );
    let known = orch_api_contract::operations::catalogue()
        .unwrap()
        .into_iter()
        .flat_map(|operation| {
            operation
                .variants
                .into_iter()
                .map(|variant| variant.operation)
        })
        .collect::<BTreeSet<_>>();
    assert!(
        scope
            .rules
            .iter()
            .flat_map(|rule| &rule.allow)
            .all(|allow| known.contains(&allow.operation))
    );
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *scene.world.conn().await,
        &identity::NewApiToken {
            id,
            user_id: user,
            name: "Stored port opening boundary",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: scene.world.now(),
            expires_at: Some(scene.world.now() + chrono::Duration::hours(1)),
        },
        Some(&json!(scope)),
    )
    .await
    .unwrap();
    Key { id, secret, user }
}

async fn legacy(scene: &Scene) -> Key {
    let scopes = vec!["ports.view".to_owned(), "ports.manage".to_owned()];
    orch_authz::TokenGrant::from_stored(&scopes, None).unwrap();
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *scene.world.conn().await,
        &identity::NewApiToken {
            id,
            user_id: scene.manager.id(),
            name: "Legacy stored opening control",
            token_hash: &token_hash(&secret),
            scopes: &scopes,
            node_id: None,
            at: scene.world.now(),
            expires_at: Some(scene.world.now() + chrono::Duration::hours(1)),
        },
        None,
    )
    .await
    .unwrap();
    Key {
        id,
        secret,
        user: scene.manager.id(),
    }
}

struct Scene {
    world: World,
    manager: Who,
    colleague: Who,
    environment: Id,
    place: Id,
    machine: Id,
    opening: PortOpening,
    proof: Option<String>,
}

async fn link(world: &World, manager: &Who, environment: Id, port: u16) -> PortOpenReply {
    let wanted = PortOpeningRequest {
        kind: PortOpeningKind::Link,
        place_id: None,
        label: Some(LABEL.into()),
        until: Some(Time::new(world.now() + chrono::Duration::hours(1))),
        entry_methods: Some(vec![PortLinkMethod::Helper]),
        ..request(port, new_id(), "reviewed link fixture")
    };
    let plan = review(world, manager, environment, &wanted).await;
    assert!(
        plan.blockers.is_empty(),
        "ordinary reviewed link: {:?}",
        plan.blockers
    );
    confirm(world, manager, environment, &plan, new_id()).await
}

impl Scene {
    async fn link() -> Self {
        let (world, manager, colleague, environment, place, machine) = setup().await;
        links_asked(&world).await;
        let made = link(&world, &manager, environment, PORT).await;
        let proof = made
            .link
            .unwrap()
            .helper_link_once
            .unwrap()
            .split('#')
            .nth(1)
            .unwrap()
            .to_owned();
        Self {
            world,
            manager,
            colleague,
            environment,
            place,
            machine,
            opening: made.opening,
            proof: Some(proof),
        }
    }
    async fn place() -> Self {
        let (world, manager, colleague, environment, place, machine) = setup().await;
        let plan = review(
            &world,
            &manager,
            environment,
            &request(PORT, place, "readable place fixture"),
        )
        .await;
        let made = confirm(&world, &manager, environment, &plan, new_id()).await;
        Self {
            world,
            manager,
            colleague,
            environment,
            place,
            machine,
            opening: made.opening,
            proof: None,
        }
    }
    async fn host() -> Self {
        let (world, manager, colleague, environment, place, machine) = setup().await;
        port_policy(&world, "port_host", "ask").await;
        let ports = orch_link_contract::policy::MachinePorts {
            allowed: vec![PortOpeningKind::Host],
            publish: Some(HostPorts {
                incoming_address: "publisher.example.test".into(),
                listen: "127.0.0.1".into(),
                ranges: vec![PortRange {
                    from: 50000,
                    to: 50002,
                }],
            }),
            limits: None,
        };
        sqlx::query("UPDATE machines SET facts=jsonb_set(facts,'{policy,ports}',$2) WHERE id=$1")
            .bind(machine)
            .bind(serde_json::to_value(ports).unwrap())
            .execute(&world.db.pool)
            .await
            .unwrap();
        let wanted = PortOpeningRequest {
            kind: PortOpeningKind::Host,
            place_id: None,
            transports: Some(vec![PortTransport::Tcp]),
            host_port: Some(50000),
            ..request(PORT, place, "owned host fixture")
        };
        let plan = review(&world, &manager, environment, &wanted).await;
        assert!(
            plan.blockers.is_empty(),
            "ordinary host fixture: {:?}",
            plan.blockers
        );
        let made = confirm(&world, &manager, environment, &plan, new_id()).await;
        assert_eq!(made.opening.publisher.as_ref().unwrap().id, machine);
        assert!(
            made.opening.host.is_none(),
            "a personal directory fabricates no Host"
        );
        Self {
            world,
            manager,
            colleague,
            environment,
            place,
            machine,
            opening: made.opening,
            proof: None,
        }
    }
    async fn row(&self) -> rows::Opening {
        rows::opening(&mut *self.world.conn().await, self.opening.id)
            .await
            .unwrap()
            .unwrap()
    }
    async fn ordinary_writer(&self, operation: Operation) {
        let other = link(&self.world, &self.manager, self.environment, PORT + 1)
            .await
            .opening;
        let response = operation.call(self, &other, &self.manager.token).await;
        assert_eq!(
            response.status,
            StatusCode::OK,
            "ordinary {} baseline: {}",
            operation.name(),
            String::from_utf8_lossy(&response.raw)
        );
        let full: PortOpening = serde_json::from_slice(&response.raw).unwrap();
        assert_eq!(full.id, other.id);
        assert_eq!(full.environment_id, self.environment);
        assert_eq!(full.revision, other.revision + 1);
    }
}

fn receipt(response: RawReply, scene: &Scene, revision: i64) {
    assert_eq!(
        response.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&response.raw)
    );
    let value: Json = serde_json::from_slice(&response.raw).unwrap();
    assert_eq!(value["projection"], "receipt");
    assert_eq!(value["opening_id"], json!(scene.opening.id));
    assert_eq!(value["environment_id"], json!(scene.environment));
    assert_eq!(value["revision"], revision);
    assert_eq!(
        value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["projection", "opening_id", "environment_id", "revision"])
    );
    assert!(!value.to_string().contains(LABEL));
    if let Some(proof) = &scene.proof {
        assert!(!value.to_string().contains(proof));
    }
}

#[tokio::test]
async fn opening_get_exact_read_uses_the_stored_environment_and_preserves_full_link_view() {
    let scene = Scene::link().await;
    let ordinary = Operation::Get
        .call(&scene, &scene.opening, &scene.manager.token)
        .await;
    assert_eq!(
        ordinary.status,
        StatusCode::OK,
        "ordinary link Get baseline"
    );
    let expected: Json = serde_json::from_slice(&ordinary.raw).unwrap();
    let issued = key(
        &scene,
        scene.manager.id(),
        vec![env_rule(Operation::Get, scene.environment, &["read"])],
    )
    .await;
    let response = Operation::Get
        .call(&scene, &scene.opening, &issued.secret)
        .await;
    assert_eq!(
        response.status,
        StatusCode::OK,
        "exact modern Get: {}",
        String::from_utf8_lossy(&response.raw)
    );
    assert_eq!(
        serde_json::from_slice::<Json>(&response.raw).unwrap(),
        expected
    );
    assert!(!String::from_utf8_lossy(&response.raw).contains(scene.proof.as_ref().unwrap()));
}

async fn manage_only(operation: Operation) {
    let scene = Scene::link().await;
    scene.ordinary_writer(operation).await;
    let issued = key(
        &scene,
        scene.manager.id(),
        vec![env_rule(operation, scene.environment, &["manage"])],
    )
    .await;
    receipt(
        operation.call(&scene, &scene.opening, &issued.secret).await,
        &scene,
        scene.opening.revision + 1,
    );
    let row = scene.row().await;
    if matches!(operation, Operation::Delete) {
        assert_eq!(row.state, "closed");
        assert!(row.secret_hash.is_none() && row.secret_sealed.is_none());
    } else {
        assert_eq!(
            row.until_at.unwrap(),
            scene.world.now() + chrono::Duration::hours(2)
        );
        assert!(row.secret_hash.is_some() && row.secret_sealed.is_none());
    }
}
#[tokio::test]
async fn opening_update_manage_only_changes_lapse_without_returning_existing_content() {
    manage_only(Operation::Update).await;
}
#[tokio::test]
async fn opening_delete_manage_only_closes_without_returning_existing_content() {
    manage_only(Operation::Delete).await;
}

async fn full_write(operation: Operation) {
    let scene = Scene::link().await;
    scene.ordinary_writer(operation).await;
    let issued = key(
        &scene,
        scene.manager.id(),
        vec![env_rule(operation, scene.environment, &["manage", "read"])],
    )
    .await;
    let response = operation.call(&scene, &scene.opening, &issued.secret).await;
    assert_eq!(
        response.status,
        StatusCode::OK,
        "modern own-operation Full: {}",
        String::from_utf8_lossy(&response.raw)
    );
    let full: PortOpening = serde_json::from_slice(&response.raw).unwrap();
    assert_eq!(full.id, scene.opening.id);
    assert_eq!(full.label, LABEL);
    assert_eq!(full.revision, scene.opening.revision + 1);
    assert!(!String::from_utf8_lossy(&response.raw).contains(scene.proof.as_ref().unwrap()));
}
#[tokio::test]
async fn opening_update_own_read_keeps_the_ordinary_full_response() {
    full_write(Operation::Update).await;
}
#[tokio::test]
async fn opening_delete_own_read_keeps_the_ordinary_full_response() {
    full_write(Operation::Delete).await;
}

#[tokio::test]
async fn another_operations_read_never_lends_mutation_full_content() {
    for operation in Operation::WRITERS {
        let scene = Scene::link().await;
        scene.ordinary_writer(operation).await;
        let issued = key(
            &scene,
            scene.manager.id(),
            vec![
                env_rule(operation, scene.environment, &["manage"]),
                env_rule(Operation::Get, scene.environment, &["read"]),
            ],
        )
        .await;
        receipt(
            operation.call(&scene, &scene.opening, &issued.secret).await,
            &scene,
            scene.opening.revision + 1,
        );
    }
}

#[tokio::test]
async fn opening_scope_operation_action_kind_and_environment_are_independent_from_ordinary_rights()
{
    for operation in [Operation::Get, Operation::Update, Operation::Delete] {
        let scene = Scene::link().await;
        let legacy = legacy(&scene).await;
        let ordinary = operation
            .call(&scene, &scene.opening, &scene.manager.token)
            .await;
        assert_eq!(ordinary.status, StatusCode::OK, "ordinary actor premise");
        // A fresh opening keeps every negative's revision/body valid.
        let active = link(&scene.world, &scene.manager, scene.environment, PORT + 1)
            .await
            .opening;
        let legacy_view = Operation::Get.call(&scene, &active, &legacy.secret).await;
        assert_eq!(
            legacy_view.status,
            StatusCode::OK,
            "legacy actual Permission names"
        );
        for rules in [
            vec![env_rule(operation, new_id(), &[operation.action()])],
            vec![rule(
                operation.name(),
                "thread",
                scene.environment,
                "self",
                &[operation.action()],
            )],
            vec![env_rule(
                operation,
                scene.environment,
                &[if matches!(operation, Operation::Get) {
                    "manage"
                } else {
                    "read"
                }],
            )],
            vec![env_rule(
                if matches!(operation, Operation::Update) {
                    Operation::Delete
                } else {
                    Operation::Update
                },
                scene.environment,
                &[operation.action()],
            )],
        ] {
            let before = rows::opening(&mut *scene.world.conn().await, active.id)
                .await
                .unwrap()
                .unwrap();
            let issued = key(&scene, scene.manager.id(), rules).await;
            let response = operation.call(&scene, &active, &issued.secret).await;
            assert_eq!(
                response.status,
                StatusCode::FORBIDDEN,
                "{} {}",
                operation.name(),
                String::from_utf8_lossy(&response.raw)
            );
            assert_eq!(response.problem().code, ErrorCode::AuthzDenied);
            let after = rows::opening(&mut *scene.world.conn().await, active.id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(after.revision, before.revision);
            assert_eq!(after.state, before.state);
            assert_eq!(after.until_at, before.until_at);
        }
    }
}

async fn credential_changes_before_write(operation: Operation) {
    use tower::ServiceExt;
    let scene = Scene::link().await;
    scene.ordinary_writer(operation).await;
    let issued = key(
        &scene,
        scene.manager.id(),
        vec![env_rule(operation, scene.environment, &["manage"])],
    )
    .await;
    let before = scene.row().await;
    let body = if matches!(operation, Operation::Update) {
        json!(PortOpeningUpdateBody {
            until: Some(Time::new(scene.world.now() + chrono::Duration::hours(2))),
            expected_revision: before.revision
        })
    } else {
        json!(PortRevisionBody {
            expected_revision: before.revision
        })
    };
    let (db, id, user, now) = (
        scene.world.app.app.db.clone(),
        issued.id,
        issued.user,
        scene.world.now(),
    );
    let changed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let observed = changed.clone();
    let delivered = axum::body::Body::from_stream(futures::stream::once(async move {
        assert!(
            identity::revoke_api_token(&mut db.acquire().await.unwrap(), id, user, now)
                .await
                .unwrap()
        );
        observed.store(true, std::sync::atomic::Ordering::SeqCst);
        Ok::<_, std::io::Error>(bytes::Bytes::from(serde_json::to_vec(&body).unwrap()))
    }));
    let mut registry = orch_api_core::registry::Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let response = registry
        .into_router_for(orch_api_contract::surface::HttpSurface::Key)
        .with_state(scene.world.app.app.clone())
        .oneshot(
            http::Request::builder()
                .method(if matches!(operation, Operation::Update) {
                    "PATCH"
                } else {
                    "DELETE"
                })
                .uri(format!("/port-openings/{}", scene.opening.id))
                .header("authorization", format!("Bearer {}", issued.secret))
                .header("content-type", "application/json")
                .body(delivered)
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        changed.load(std::sync::atomic::Ordering::SeqCst),
        "the real authenticated request must reach the body/guard rendezvous"
    );
    assert!(
        matches!(
            response.status(),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ),
        "actual current credential refusal"
    );
    let after = scene.row().await;
    assert_eq!(after.revision, before.revision);
    assert_eq!(after.state, before.state);
    assert_eq!(after.until_at, before.until_at);
    assert_eq!(after.secret_hash, before.secret_hash);
}

#[tokio::test]
async fn opening_update_rechecks_the_real_key_after_authentication_before_writing() {
    credential_changes_before_write(Operation::Update).await;
}

#[tokio::test]
async fn opening_delete_rechecks_the_real_key_after_authentication_before_writing() {
    credential_changes_before_write(Operation::Delete).await;
}

#[tokio::test]
async fn restricted_receipts_do_not_skip_revision_expiry_or_irreversible_close_rules() {
    let scene = Scene::link().await;
    let issued = key(
        &scene,
        scene.manager.id(),
        vec![
            env_rule(Operation::Update, scene.environment, &["manage"]),
            env_rule(Operation::Delete, scene.environment, &["manage"]),
        ],
    )
    .await;
    for until in [
        None,
        Some(Time::new(scene.world.now() - chrono::Duration::seconds(1))),
        Some(Time::new(scene.world.now() + chrono::Duration::days(2))),
    ] {
        let result = scene
            .world
            .app
            .call(PortOpeningUpdate, &[&scene.opening.id])
            .token(&issued.secret)
            .body(&PortOpeningUpdateBody {
                until,
                expected_revision: scene.opening.revision,
            })
            .send_bytes()
            .await;
        assert!(
            matches!(
                result.status,
                StatusCode::BAD_REQUEST | StatusCode::FORBIDDEN
            ),
            "ordinary lapse validation remains"
        );
        assert_eq!(scene.row().await.revision, scene.opening.revision);
    }
    let stale = scene
        .world
        .app
        .call(PortOpeningDelete, &[&scene.opening.id])
        .token(&issued.secret)
        .body(&PortRevisionBody {
            expected_revision: scene.opening.revision + 1,
        })
        .send_bytes()
        .await;
    assert_eq!(stale.status, StatusCode::CONFLICT);
    receipt(
        Operation::Delete
            .call(&scene, &scene.opening, &issued.secret)
            .await,
        &scene,
        scene.opening.revision + 1,
    );
    receipt(
        Operation::Delete
            .call(&scene, &scene.opening, &issued.secret)
            .await,
        &scene,
        scene.opening.revision + 1,
    );
    let ended = scene.row().await;
    let revive = scene
        .world
        .app
        .call(PortOpeningUpdate, &[&scene.opening.id])
        .token(&issued.secret)
        .body(&PortOpeningUpdateBody {
            until: Some(Time::new(scene.world.now() + chrono::Duration::hours(2))),
            expected_revision: ended.revision,
        })
        .send_bytes()
        .await;
    assert!(
        !revive.status.is_success(),
        "no receipt branch revives closed admission"
    );
    assert_eq!(scene.row().await.revision, ended.revision);
}

#[tokio::test]
async fn ending_an_updaters_key_does_not_replace_an_accepted_openings_lifetime_authority() {
    let scene = Scene::link().await;
    let issued = key(
        &scene,
        scene.manager.id(),
        vec![env_rule(Operation::Update, scene.environment, &["manage"])],
    )
    .await;
    receipt(
        Operation::Update
            .call(&scene, &scene.opening, &issued.secret)
            .await,
        &scene,
        scene.opening.revision + 1,
    );
    assert!(
        identity::revoke_api_token(
            &mut *scene.world.conn().await,
            issued.id,
            issued.user,
            scene.world.now()
        )
        .await
        .unwrap()
    );
    assert!(
        reconcile(&scene.world, scene.environment).await.is_empty(),
        "Once/Grant authority is the existing producer's rule, not a new updater-key lease"
    );
    let current = Operation::Get
        .call(&scene, &scene.opening, &scene.manager.token)
        .await;
    assert_eq!(current.status, StatusCode::OK);
    let current: PortOpening = serde_json::from_slice(&current.raw).unwrap();
    assert_ne!(current.state, PortOpeningState::Closed);
    assert_eq!(current.allowed_by.unwrap().id, scene.manager.id());
}

#[tokio::test]
async fn named_place_viewer_reads_only_that_opening_with_no_environment_door_or_borrowed_governor()
{
    let scene = Scene::place().await;
    let no_door = scene
        .world
        .app
        .call(EnvironmentPortGet, &[&scene.environment, &PORT.to_string()])
        .token(&scene.colleague.token)
        .query(&EnvironmentPortGetQuery::default())
        .send()
        .await;
    assert_eq!(
        no_door.status,
        StatusCode::FORBIDDEN,
        "named place grants no Env door"
    );
    let ordinary = Operation::Get
        .call(&scene, &scene.opening, &scene.colleague.token)
        .await;
    assert_eq!(ordinary.status, StatusCode::OK, "ordinary named-place path");
    let opened: PortOpening = serde_json::from_slice(&ordinary.raw).unwrap();
    assert!(!opened.can.close && !opened.can.change);
    let wrong = key(
        &scene,
        scene.colleague.id(),
        vec![rule(
            Operation::Get.name(),
            "project",
            scene.place,
            "subtree",
            &["read"],
        )],
    )
    .await;
    assert_eq!(
        Operation::Get
            .call(&scene, &scene.opening, &wrong.secret)
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    let issued = key(
        &scene,
        scene.colleague.id(),
        vec![env_rule(Operation::Get, scene.environment, &["read"])],
    )
    .await;
    let response = Operation::Get
        .call(&scene, &scene.opening, &issued.secret)
        .await;
    assert_eq!(
        response.status,
        StatusCode::OK,
        "no extra EnvView is invented"
    );
    let cut: PortOpening = serde_json::from_slice(&response.raw).unwrap();
    let place = cut.place.unwrap();
    assert_eq!(place.id, scene.place);
    assert!(place.redacted && place.name.is_none());
    let named = key(
        &scene,
        scene.colleague.id(),
        vec![
            env_rule(Operation::Get, scene.environment, &["read"]),
            rule(
                Operation::Get.name(),
                "project",
                scene.place,
                "self",
                &["read"],
            ),
        ],
    )
    .await;
    let named = Operation::Get
        .call(&scene, &scene.opening, &named.secret)
        .await;
    assert_eq!(named.status, StatusCode::OK);
    let named: PortOpening = serde_json::from_slice(&named.raw).unwrap();
    assert_eq!(named.place.unwrap().name.as_deref(), Some("Preview"));
}

#[tokio::test]
async fn a_named_place_viewer_gets_no_manager_or_private_link_metadata_right() {
    let scene = Scene::link().await;
    assert_eq!(
        Operation::Get
            .call(&scene, &scene.opening, &scene.colleague.token)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    let issued = key(
        &scene,
        scene.colleague.id(),
        vec![env_rule(Operation::Get, scene.environment, &["read"])],
    )
    .await;
    assert_eq!(
        Operation::Get
            .call(&scene, &scene.opening, &issued.secret)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    for operation in Operation::WRITERS {
        assert_eq!(
            operation
                .call(&scene, &scene.opening, &scene.colleague.token)
                .await
                .status,
            StatusCode::FORBIDDEN
        );
        let issued = key(
            &scene,
            scene.colleague.id(),
            vec![env_rule(operation, scene.environment, &["manage", "read"])],
        )
        .await;
        assert_eq!(
            operation
                .call(&scene, &scene.opening, &issued.secret)
                .await
                .status,
            StatusCode::FORBIDDEN
        );
    }
}

#[tokio::test]
async fn host_opening_full_requires_its_actual_personal_machine_source_read() {
    let scene = Scene::host().await;
    let ordinary = Operation::Get
        .call(&scene, &scene.opening, &scene.manager.token)
        .await;
    assert_eq!(ordinary.status, StatusCode::OK, "ordinary host metadata");
    let reference: PortOpening = serde_json::from_slice(&ordinary.raw).unwrap();
    assert!(reference.host.is_none());
    assert_eq!(reference.publisher.unwrap().id, scene.machine);
    let env_only = key(
        &scene,
        scene.manager.id(),
        vec![env_rule(Operation::Get, scene.environment, &["read"])],
    )
    .await;
    assert_eq!(
        Operation::Get
            .call(&scene, &scene.opening, &env_only.secret)
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    let wrong_operation = key(
        &scene,
        scene.manager.id(),
        vec![
            env_rule(Operation::Get, scene.environment, &["read"]),
            rule("MachineGet", "machine", scene.machine, "self", &["read"]),
        ],
    )
    .await;
    assert_eq!(
        Operation::Get
            .call(&scene, &scene.opening, &wrong_operation.secret)
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    let full = key(
        &scene,
        scene.manager.id(),
        vec![
            env_rule(Operation::Get, scene.environment, &["read"]),
            rule(
                Operation::Get.name(),
                "machine",
                scene.machine,
                "self",
                &["read"],
            ),
        ],
    )
    .await;
    let response = Operation::Get
        .call(&scene, &scene.opening, &full.secret)
        .await;
    assert_eq!(
        response.status,
        StatusCode::OK,
        "actual machine source: {}",
        String::from_utf8_lossy(&response.raw)
    );
    assert_eq!(
        serde_json::from_slice::<Json>(&response.raw).unwrap(),
        serde_json::from_slice::<Json>(&ordinary.raw).unwrap()
    );
}

#[tokio::test]
async fn host_close_needs_no_new_machine_write_or_read_when_it_returns_a_receipt() {
    let scene = Scene::host().await;
    let issued = key(
        &scene,
        scene.manager.id(),
        vec![env_rule(Operation::Delete, scene.environment, &["manage"])],
    )
    .await;
    receipt(
        Operation::Delete
            .call(&scene, &scene.opening, &issued.secret)
            .await,
        &scene,
        scene.opening.revision + 1,
    );
    assert_eq!(scene.row().await.state, "closed");
}

#[tokio::test]
async fn host_projection_binds_the_current_host_facade_not_the_old_allocation_hint() {
    let scene = Scene::host().await;
    let host = new_id();
    sqlx::query("INSERT INTO hosts(id,machine_id,owner_id,org_id,home_id,name) VALUES($1,$2,$3,$4,$4,'Current private host')")
        .bind(host).bind(scene.machine).bind(scene.manager.id()).bind(scene.manager.person.personal_org)
        .execute(&scene.world.db.pool).await.unwrap();
    let ordinary = Operation::Get
        .call(&scene, &scene.opening, &scene.manager.token)
        .await;
    assert_eq!(
        ordinary.status,
        StatusCode::OK,
        "current facade ordinary source"
    );
    let value: PortOpening = serde_json::from_slice(&ordinary.raw).unwrap();
    assert_eq!(value.host.unwrap().id, host);
    assert!(scene.row().await.host_id.is_none());
    let old_machine = key(
        &scene,
        scene.manager.id(),
        vec![
            env_rule(Operation::Get, scene.environment, &["read"]),
            rule(
                Operation::Get.name(),
                "machine",
                scene.machine,
                "self",
                &["read"],
            ),
        ],
    )
    .await;
    assert_eq!(
        Operation::Get
            .call(&scene, &scene.opening, &old_machine.secret)
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    let actual = key(
        &scene,
        scene.manager.id(),
        vec![
            env_rule(Operation::Get, scene.environment, &["read"]),
            rule(Operation::Get.name(), "host", host, "self", &["read"]),
        ],
    )
    .await;
    let response = Operation::Get
        .call(&scene, &scene.opening, &actual.secret)
        .await;
    assert_eq!(
        response.status,
        StatusCode::OK,
        "actual Host facade identity"
    );
    assert_eq!(
        serde_json::from_slice::<PortOpening>(&response.raw)
            .unwrap()
            .host
            .unwrap()
            .name,
        "Current private host"
    );
}
