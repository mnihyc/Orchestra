//! Actual native confirmation, one-time generated delivery and protected existing readback.

use super::*;
use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_core::{
    middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash},
    testing::RawReply,
};
use orch_db::queries::identity;
use orch_types::Json;

const OP: &str = "EnvironmentPortOpeningCreate";

fn rule(operation: &str, kind: &str, id: Id, reach: &str, actions: &[&str]) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":reach},"allow":[{"operation":operation,"actions":actions}]})
}
async fn key(world: &World, user: Id, rules: Vec<Json>) -> (Id, String) {
    let selected: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(selected.validate().is_empty());
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id,
            user_id: user,
            name: "Native port confirmation",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        Some(&serde_json::to_value(selected).unwrap()),
    )
    .await
    .unwrap();
    (id, secret)
}
fn rules(env: Id, project: Option<Id>, read: bool) -> Vec<Json> {
    let mut rules = vec![rule(
        OP,
        "environment",
        env,
        "self",
        if read {
            &["manage", "read"]
        } else {
            &["manage"]
        },
    )];
    if let Some(project) = project {
        rules.push(rule(OP, "project", project, "self", &["read"]));
    }
    rules
}
async fn call(
    world: &World,
    env: Id,
    port: u16,
    body: &EnvironmentPortOpeningCreateBody,
    token: &str,
) -> RawReply {
    let request = world
        .app
        .call(EnvironmentPortOpeningCreate, &[&env, &port.to_string()])
        .token(token)
        .body(body);
    request.send_bytes().await
}
fn receipt(actual: &RawReply, env: Id, created: bool) -> Json {
    assert_eq!(
        actual.status,
        if created {
            StatusCode::CREATED
        } else {
            StatusCode::OK
        },
        "{}",
        String::from_utf8_lossy(&actual.raw)
    );
    let value: Json = serde_json::from_slice(&actual.raw).unwrap();
    assert_eq!(value["projection"], "receipt");
    assert_eq!(value["environment_id"], json!(env));
    assert_eq!(value["created"], created);
    for field in [
        "opening",
        "program",
        "place",
        "publisher",
        "host",
        "request",
    ] {
        assert!(value.get(field).is_none());
    }
    value
}
fn link_request(world: &World, port: u16) -> PortOpeningRequest {
    PortOpeningRequest {
        port,
        kind: PortOpeningKind::Link,
        place_id: None,
        label: Some("Existing link metadata needs this act's Read".into()),
        transports: None,
        entry_methods: None,
        until: Some(Time::new(world.now() + chrono::Duration::hours(1))),
        host_port: None,
        program: None,
        reason: "a reviewed generated proof".into(),
        replacement_of: None,
        close_replaced: None,
    }
}
async fn openings(world: &World, env: Id) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM port_openings WHERE environment_id=$1")
        .bind(env)
        .fetch_one(&world.db.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn create_manage_only_keeps_the_native_audience_and_own_read_returns_the_original_full_opening()
 {
    for full in [false, true] {
        let (world, manager, _, env, project, _) = setup().await;
        let plan = review(
            &world,
            &manager,
            env,
            &request(9401, project, "native confirmation"),
        )
        .await;
        let mut grants = rules(env, Some(project), full);
        grants.push(rule(
            "EnvironmentPortGet",
            "environment",
            env,
            "self",
            &["read"],
        ));
        if full {
            let root: Id = sqlx::query_scalar("SELECT org_id FROM nodes WHERE id=$1")
                .bind(project)
                .fetch_one(&world.db.pool)
                .await
                .unwrap();
            grants.push(rule(OP, "org", root, "subtree", &["read"]));
        }
        let (_, token) = key(&world, manager.id(), grants).await;
        let body = EnvironmentPortOpeningCreateBody {
            preflight_id: plan.preflight_id,
            client_id: new_id(),
        };
        let made = call(&world, env, 9401, &body, &token).await;
        let id: Id = if full {
            assert_eq!(made.status, StatusCode::CREATED);
            let value: Json = serde_json::from_slice(&made.raw).unwrap();
            assert!(value.get("projection").is_none());
            let id: Id = serde_json::from_value(value["opening"]["id"].clone()).unwrap();
            let native = world
                .app
                .call(PortOpeningGet, &[&id])
                .token(&manager.token)
                .send()
                .await
                .ok();
            assert_eq!(value["opening"], serde_json::to_value(native).unwrap());
            id
        } else {
            serde_json::from_value(receipt(&made, env, true)["opening_id"].clone()).unwrap()
        };
        let replay = call(&world, env, 9401, &body, &token).await;
        if full {
            assert_eq!(replay.status, StatusCode::OK);
            assert_eq!(
                serde_json::from_slice::<Json>(&replay.raw).unwrap()["opening"]["id"],
                json!(id)
            );
        } else {
            assert_eq!(receipt(&replay, env, false)["opening_id"], json!(id));
        }
        assert_eq!(openings(&world, env).await, 1);
    }
}

#[tokio::test]
async fn create_requires_exact_operation_environment_action_named_input_and_native_door_before_any_effect()
 {
    let (world, manager, colleague, env, project, _) = setup().await;
    let plan = review(
        &world,
        &manager,
        env,
        &request(9402, project, "exact native work"),
    )
    .await;
    for (user, grants) in [
        (
            manager.id(),
            vec![
                rule(
                    "EnvironmentPortOpeningPreflight",
                    "environment",
                    env,
                    "self",
                    &["manage"],
                ),
                rule(OP, "project", project, "self", &["read"]),
            ],
        ),
        (manager.id(), rules(new_id(), Some(project), true)),
        (
            manager.id(),
            vec![
                rule(OP, "environment", env, "self", &["read"]),
                rule(OP, "project", project, "self", &["read"]),
            ],
        ),
        (manager.id(), rules(env, None, true)),
        (colleague.id(), rules(env, Some(project), true)),
    ] {
        let (_, token) = key(&world, user, grants).await;
        let actual = call(
            &world,
            env,
            9402,
            &EnvironmentPortOpeningCreateBody {
                preflight_id: plan.preflight_id,
                client_id: new_id(),
            },
            &token,
        )
        .await;
        assert!(!actual.status.is_success());
        assert_eq!(openings(&world, env).await, 0);
    }
}

#[tokio::test]
async fn create_and_review_keys_pass_their_own_separate_operations_without_lending_one_anothers_scope()
 {
    let (world, manager, _, env, project, _) = setup().await;
    let review_op = "EnvironmentPortOpeningPreflight";
    let (reviewer_id, reviewer) = key(
        &world,
        manager.id(),
        vec![
            rule(review_op, "environment", env, "self", &["manage"]),
            rule(review_op, "project", project, "self", &["read"]),
        ],
    )
    .await;
    let (_, confirmer) = key(&world, manager.id(), rules(env, Some(project), false)).await;
    for (port, revoke) in [(9410, false), (9411, true)] {
        let request = world
            .app
            .call(EnvironmentPortOpeningPreflight, &[&env, &port.to_string()])
            .token(&reviewer)
            .body(&super::request(port, project, "separate native stages"));
        let made = request.send_bytes().await;
        assert_eq!(made.status, StatusCode::OK);
        let value: Json = serde_json::from_slice(&made.raw).unwrap();
        assert_eq!(value["projection"], "receipt");
        let id: Id = serde_json::from_value(value["preflight_id"].clone()).unwrap();
        if revoke {
            sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
                .bind(reviewer_id)
                .bind(world.now())
                .execute(&world.db.pool)
                .await
                .unwrap();
        }
        let actual = call(
            &world,
            env,
            port,
            &EnvironmentPortOpeningCreateBody {
                preflight_id: id,
                client_id: new_id(),
            },
            &confirmer,
        )
        .await;
        if revoke {
            assert_eq!(actual.status, StatusCode::FORBIDDEN);
        } else {
            receipt(&actual, env, true);
        }
    }
    assert_eq!(openings(&world, env).await, 1);
}

#[tokio::test]
async fn create_link_keeps_empty_named_place_and_delivers_only_its_new_secret_in_a_write_only_receipt()
 {
    let (world, manager, _, env, _, _) = setup().await;
    links_asked(&world).await;
    let plan = review(&world, &manager, env, &link_request(&world, 9403)).await;
    let (_, token) = key(&world, manager.id(), rules(env, None, false)).await;
    let body = EnvironmentPortOpeningCreateBody {
        preflight_id: plan.preflight_id,
        client_id: new_id(),
    };
    let made = receipt(&call(&world, env, 9403, &body, &token).await, env, true);
    let id: Id = serde_json::from_value(made["opening_id"].clone()).unwrap();
    let address = url::Url::parse(made["link"]["url_once"].as_str().unwrap()).unwrap();
    let proof = address.fragment().unwrap();
    let digest: Vec<u8> = sqlx::query_scalar("SELECT secret_hash FROM port_openings WHERE id=$1")
        .bind(id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        digest,
        orch_types::Hash::of(proof.as_bytes()).as_bytes().to_vec()
    );
    assert!(
        !serde_json::to_string(&made)
            .unwrap()
            .contains("Existing link metadata")
    );
    let replay = receipt(&call(&world, env, 9403, &body, &token).await, env, false);
    assert_eq!(replay["opening_id"], json!(id));
    assert!(replay["link"].is_null());
    let again = review(&world, &manager, env, &link_request(&world, 9403)).await;
    let coalesced = receipt(
        &call(
            &world,
            env,
            9403,
            &EnvironmentPortOpeningCreateBody {
                preflight_id: again.preflight_id,
                client_id: new_id(),
            },
            &token,
        )
        .await,
        env,
        false,
    );
    assert_eq!(coalesced["opening_id"], json!(id));
    assert!(coalesced["link"].is_null());
    assert_eq!(openings(&world, env).await, 1);
}

async fn host_plan(
    world: &World,
    manager: &Who,
    env: Id,
    project: Id,
    machine: Id,
    port: u16,
) -> PortOpeningPlan {
    port_policy(world, "port_host", "ask").await;
    let policy = orch_link_contract::policy::MachinePorts {
        allowed: vec![PortOpeningKind::Host],
        publish: Some(HostPorts {
            incoming_address: "scoped-create-publisher.example.test".into(),
            listen: "127.0.0.1".into(),
            ranges: vec![PortRange {
                from: 50300,
                to: 50302,
            }],
        }),
        limits: None,
    };
    sqlx::query("UPDATE machines SET facts=jsonb_set(facts,'{policy,ports}',$2) WHERE id=$1")
        .bind(machine)
        .bind(serde_json::to_value(policy).unwrap())
        .execute(&world.db.pool)
        .await
        .unwrap();
    let mut body = request(port, project, "native publisher");
    body.kind = PortOpeningKind::Host;
    body.place_id = None;
    body.transports = Some(vec![PortTransport::Tcp]);
    body.host_port = Some(50300);
    let plan = review(world, manager, env, &body).await;
    assert!(plan.blockers.is_empty());
    plan
}

#[tokio::test]
async fn create_host_keeps_native_publication_and_requires_own_publisher_read_only_for_full_content()
 {
    for full in [false, true] {
        let (world, manager, _, env, project, machine) = setup().await;
        let plan = host_plan(&world, &manager, env, project, machine, 9404).await;
        let mut grants = rules(env, None, true);
        grants.push(rule(
            if full { OP } else { "MachineGet" },
            "machine",
            machine,
            "self",
            &["read"],
        ));
        let (_, token) = key(&world, manager.id(), grants).await;
        let made = call(
            &world,
            env,
            9404,
            &EnvironmentPortOpeningCreateBody {
                preflight_id: plan.preflight_id,
                client_id: new_id(),
            },
            &token,
        )
        .await;
        if full {
            assert_eq!(made.status, StatusCode::CREATED);
            let value: Json = serde_json::from_slice(&made.raw).unwrap();
            assert_eq!(value["opening"]["publisher"]["id"], json!(machine));
            assert!(value["opening"]["host"].is_null());
        } else {
            receipt(&made, env, true);
        }
        let held: i64 = sqlx::query_scalar("SELECT count(*) FROM port_openings WHERE environment_id=$1 AND publish_machine_id=$2 AND host_allocated").bind(env).bind(machine).fetch_one(&world.db.pool).await.unwrap();
        assert_eq!(held, 1);
    }
}

async fn waiting(
    world: &World,
    pid: i32,
    pending: &tokio::task::JoinHandle<RawReply>,
    query: &str,
) {
    tokio::time::timeout(std::time::Duration::from_secs(30), async { loop {
        assert!(!pending.is_finished(), "the actual native SQL wait must be reached");
        let found: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query ILIKE $2)").bind(pid).bind(query).fetch_one(&world.db.pool).await.unwrap();
        if found { break; } tokio::task::yield_now().await;
    }}).await.unwrap();
}

#[tokio::test]
async fn create_refreshes_its_exact_key_after_the_native_environment_and_publisher_locks() {
    for publisher in [false, true] {
        for expire in [false, true] {
            let (world, manager, _, env, project, machine) = setup().await;
            let port = if publisher { 9406 } else { 9405 };
            let plan = if publisher {
                host_plan(&world, &manager, env, project, machine, port).await
            } else {
                review(
                    &world,
                    &manager,
                    env,
                    &request(port, project, "actual lock refresh"),
                )
                .await
            };
            let (key_id, token) = key(
                &world,
                manager.id(),
                rules(env, (!publisher).then_some(project), false),
            )
            .await;
            sqlx::query("UPDATE api_tokens SET expires_at=$2 WHERE id=$1")
                .bind(key_id)
                .bind(world.now() + chrono::Duration::minutes(2))
                .execute(&world.db.pool)
                .await
                .unwrap();
            let mut blocker = world.db.pool.begin().await.unwrap();
            if publisher {
                orch_db::queries::ports::lock_publisher_machine(&mut blocker, machine)
                    .await
                    .unwrap();
            } else {
                sqlx::query("SELECT id FROM environments WHERE id=$1 FOR UPDATE")
                    .bind(env)
                    .execute(&mut *blocker)
                    .await
                    .unwrap();
            }
            let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *blocker)
                .await
                .unwrap();
            let app = world.app.clone();
            let body = EnvironmentPortOpeningCreateBody {
                preflight_id: plan.preflight_id,
                client_id: new_id(),
            };
            let pending = tokio::spawn(async move {
                let request = app
                    .call(EnvironmentPortOpeningCreate, &[&env, &port.to_string()])
                    .token(&token)
                    .body(&body);
                request.send_bytes().await
            });
            waiting(
                &world,
                pid,
                &pending,
                if publisher {
                    "%SELECT id FROM machines%FOR NO KEY UPDATE%"
                } else {
                    "%SELECT id FROM environments%FOR UPDATE%"
                },
            )
            .await;
            if expire {
                world.advance(chrono::Duration::minutes(3));
            }
            blocker.commit().await.unwrap();
            let actual = tokio::time::timeout(std::time::Duration::from_secs(30), pending)
                .await
                .unwrap()
                .unwrap();
            if expire {
                assert_eq!(actual.status, StatusCode::UNAUTHORIZED);
                assert_eq!(openings(&world, env).await, 0);
            } else {
                receipt(&actual, env, true);
                assert_eq!(openings(&world, env).await, 1);
            }
        }
    }
}

#[tokio::test]
async fn create_rolls_back_native_work_when_its_key_expires_during_the_actual_audit_insert() {
    for expire in [false, true] {
        let (world, manager, _, env, project, _) = setup().await;
        let plan = review(
            &world,
            &manager,
            env,
            &request(9407, project, "audit-bound native write"),
        )
        .await;
        let (key_id, token) = key(&world, manager.id(), rules(env, Some(project), false)).await;
        sqlx::query("UPDATE api_tokens SET expires_at=$2 WHERE id=$1")
            .bind(key_id)
            .bind(world.now() + chrono::Duration::minutes(2))
            .execute(&world.db.pool)
            .await
            .unwrap();
        let mut blocker = world.db.pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE audit_log IN SHARE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = world.app.clone();
        let body = EnvironmentPortOpeningCreateBody {
            preflight_id: plan.preflight_id,
            client_id: new_id(),
        };
        let pending = tokio::spawn(async move {
            let request = app
                .call(EnvironmentPortOpeningCreate, &[&env, &"9407"])
                .token(&token)
                .body(&body);
            request.send_bytes().await
        });
        waiting(&world, pid, &pending, "%INSERT INTO audit_log%").await;
        if expire {
            world.advance(chrono::Duration::minutes(3));
        }
        blocker.commit().await.unwrap();
        let actual = tokio::time::timeout(std::time::Duration::from_secs(30), pending)
            .await
            .unwrap()
            .unwrap();
        if expire {
            assert_eq!(actual.status, StatusCode::UNAUTHORIZED);
            assert_eq!(openings(&world, env).await, 0);
        } else {
            receipt(&actual, env, true);
            assert_eq!(openings(&world, env).await, 1);
        }
    }
}

#[tokio::test]
async fn accepted_create_preserves_its_new_secret_and_receipt_after_the_final_activity_wait_without_replaying_work()
 {
    for mode in ["unchanged", "read_removed", "manage_removed", "expire"] {
        let (world, manager, _, env, _, _) = setup().await;
        links_asked(&world).await;
        let plan = review(&world, &manager, env, &link_request(&world, 9408)).await;
        let (key_id, token) = key(&world, manager.id(), rules(env, None, true)).await;
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
        let body = EnvironmentPortOpeningCreateBody {
            preflight_id: plan.preflight_id,
            client_id: new_id(),
        };
        let pending = tokio::spawn(async move {
            let request = app
                .call(EnvironmentPortOpeningCreate, &[&env, &"9408"])
                .token(&token)
                .body(&body);
            request.send_bytes().await
        });
        waiting(&world, pid, &pending, "%INSERT INTO api_token_activity%").await;
        let (id, version): (Id, String) =
            sqlx::query_as("SELECT id,xmin::text FROM port_openings WHERE environment_id=$1")
                .bind(env)
                .fetch_one(&world.db.pool)
                .await
                .unwrap();
        match mode {
            "read_removed" => {
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(key_id)
                    .bind(json!({"rules":rules(env,None,false)}))
                    .execute(&world.db.pool)
                    .await
                    .unwrap();
            }
            "manage_removed" => {
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(key_id)
                    .bind(json!({"rules":[rule(OP,"environment",env,"self", &["read"])]}))
                    .execute(&world.db.pool)
                    .await
                    .unwrap();
            }
            "expire" => world.advance(chrono::Duration::hours(2)),
            _ => {}
        }
        blocker.commit().await.unwrap();
        let actual = tokio::time::timeout(std::time::Duration::from_secs(30), pending)
            .await
            .unwrap()
            .unwrap();
        let value: Json = serde_json::from_slice(&actual.raw).unwrap();
        assert_eq!(actual.status, StatusCode::CREATED);
        if matches!(mode, "read_removed" | "expire") {
            assert_eq!(receipt(&actual, env, true)["opening_id"], json!(id));
        } else {
            assert_eq!(value["opening"]["id"], json!(id));
            assert!(value.get("projection").is_none());
        }
        let link = url::Url::parse(value["link"]["url_once"].as_str().unwrap()).unwrap();
        let proof = link.fragment().unwrap();
        let (after, digest): (String, Vec<u8>) =
            sqlx::query_as("SELECT xmin::text,secret_hash FROM port_openings WHERE id=$1")
                .bind(id)
                .fetch_one(&world.db.pool)
                .await
                .unwrap();
        assert_eq!(after, version);
        assert_eq!(
            digest,
            orch_types::Hash::of(proof.as_bytes()).as_bytes().to_vec()
        );
        assert_eq!(openings(&world, env).await, 1);
    }
}

#[tokio::test]
async fn accepted_create_does_not_loan_a_replacement_association_to_its_already_rendered_opening() {
    for replace in [false, true] {
        let (world, manager, _, env, project, _) = setup().await;
        let root: Id = sqlx::query_scalar("SELECT org_id FROM nodes WHERE id=$1")
            .bind(project)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
        let other = make_project(&world, &manager, root, "a different current source")
            .await
            .base
            .id;
        let plan = review(
            &world,
            &manager,
            env,
            &request(9409, project, "immutable readback"),
        )
        .await;
        let (_, token) = key(
            &world,
            manager.id(),
            vec![
                rule(OP, "environment", env, "self", &["manage", "read"]),
                rule(OP, "org", root, "subtree", &["read"]),
            ],
        )
        .await;
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
        let body = EnvironmentPortOpeningCreateBody {
            preflight_id: plan.preflight_id,
            client_id: new_id(),
        };
        let pending = tokio::spawn(async move {
            let request = app
                .call(EnvironmentPortOpeningCreate, &[&env, &"9409"])
                .token(&token)
                .body(&body);
            request.send_bytes().await
        });
        waiting(&world, pid, &pending, "%INSERT INTO api_token_activity%").await;
        let id: Id = sqlx::query_scalar("SELECT id FROM port_openings WHERE environment_id=$1")
            .bind(env)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
        if replace {
            sqlx::query("UPDATE port_openings SET place_id=$2 WHERE id=$1")
                .bind(id)
                .bind(other)
                .execute(&world.db.pool)
                .await
                .unwrap();
        }
        blocker.commit().await.unwrap();
        let actual = tokio::time::timeout(std::time::Duration::from_secs(30), pending)
            .await
            .unwrap()
            .unwrap();
        if replace {
            receipt(&actual, env, true);
        } else {
            assert_eq!(actual.status, StatusCode::CREATED);
            assert_eq!(
                serde_json::from_slice::<Json>(&actual.raw).unwrap()["opening"]["place"]["id"],
                json!(project)
            );
        }
        assert_eq!(openings(&world, env).await, 1);
    }
}
