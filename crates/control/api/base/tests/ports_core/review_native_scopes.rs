//! Native review storage and rich-content disclosure are separate scoped capabilities.

use super::*;
use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_db::queries::{identity, jobs::forks};
use orch_types::Json;

const OP: &str = "EnvironmentPortOpeningPreflight";
fn env_rule(env: Id, actions: &[&str]) -> Json {
    json!({"target":{"kind":"environment","id":env,"reach":"self"},"allow":[{"operation":OP,"actions":actions}]})
}
fn node_rule(kind: &str, id: Id, reach: &str, operation: &str) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":reach},"allow":[{"operation":operation,"actions":["read"]}]})
}
async fn key_record(world: &World, user: Id, rules: Vec<Json>) -> (Id, String) {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id,
            user_id: user,
            name: "Native port review scope",
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
async fn key(world: &World, user: Id, rules: Vec<Json>) -> String {
    key_record(world, user, rules).await.1
}
async fn review_call(
    world: &World,
    env: Id,
    token: &str,
    body: &PortOpeningRequest,
) -> orch_api_core::testing::RawReply {
    world
        .app
        .call(
            EnvironmentPortOpeningPreflight,
            &[&env, &body.port.to_string()],
        )
        .token(token)
        .body(body)
        .send_bytes()
        .await
}

#[tokio::test]
async fn review_keeps_named_place_read_as_input_and_rich_readback_belongs_to_the_same_operation() {
    let (world, manager, colleague, env, project, _) = setup().await;
    let body = request(9321, project, "scope review");
    let root: Id = sqlx::query_scalar("SELECT org_id FROM nodes WHERE id=$1")
        .bind(project)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    for full in [false, true] {
        let mut rules = vec![env_rule(
            env,
            if full {
                &["manage", "read"]
            } else {
                &["manage"]
            },
        )];
        rules.push(node_rule("project", project, "self", OP));
        if full {
            rules.push(node_rule("org", root, "subtree", OP));
        } else {
            rules.push(node_rule("org", root, "subtree", "NodeGet.read"));
        }
        let token = key(&world, manager.id(), rules).await;
        let response = review_call(&world, env, &token, &body).await;
        assert_eq!(
            response.status,
            StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&response.raw)
        );
        let value: Json = serde_json::from_slice(&response.raw).unwrap();
        let id: Id = serde_json::from_value(value["preflight_id"].clone()).unwrap();
        let stored = forks::preflight(&mut *world.conn().await, id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.environment_id, Some(env));
        assert_eq!(stored.created_by, manager.id());
        assert_eq!(stored.plan["creator_origin"]["operation"], OP);
        assert_eq!(stored.plan["body"], serde_json::to_value(&body).unwrap());
        if full {
            assert!(value.get("target").is_some() && value.get("audience").is_some());
            assert!(value.get("projection").is_none());
        } else {
            assert_eq!(value["projection"], "receipt");
            assert_eq!(value["environment_id"], json!(env));
            assert_eq!(value["port"], 9321);
            assert!(!value.as_object().unwrap().contains_key("target"));
            assert!(!value.as_object().unwrap().contains_key("audience"));
        }
    }
    let foreign_name_read = key(
        &world,
        manager.id(),
        vec![
            env_rule(env, &["manage", "read"]),
            node_rule("project", project, "self", OP),
            node_rule("org", root, "subtree", "NodeGet.read"),
        ],
    )
    .await;
    let projected = review_call(&world, env, &foreign_name_read, &body).await;
    assert_eq!(projected.status, StatusCode::OK);
    assert_eq!(
        serde_json::from_slice::<Json>(&projected.raw).unwrap()["projection"],
        "receipt",
        "another operation cannot lend protected path names"
    );
    let no_native = key(
        &world,
        colleague.id(),
        vec![
            env_rule(env, &["manage", "read"]),
            node_rule("org", root, "subtree", OP),
        ],
    )
    .await;
    assert!(
        !review_call(&world, env, &no_native, &body)
            .await
            .status
            .is_success()
    );
    for rules in [
        vec![env_rule(env, &["manage"])],
        vec![
            env_rule(env, &["read"]),
            node_rule("org", root, "subtree", OP),
        ],
        vec![
            env_rule(new_id(), &["manage", "read"]),
            node_rule("org", root, "subtree", OP),
        ],
    ] {
        let token = key(&world, manager.id(), rules).await;
        assert_eq!(
            review_call(&world, env, &token, &body).await.status,
            StatusCode::FORBIDDEN
        );
    }
}

#[tokio::test]
async fn link_and_host_reviews_have_genuinely_empty_named_place_sources_and_keep_safe_receipts() {
    let (world, manager, _, env, _, _) = setup().await;
    links_asked(&world).await;
    let token = key(&world, manager.id(), vec![env_rule(env, &["manage"])]).await;
    for kind in [PortOpeningKind::Link, PortOpeningKind::Host] {
        let body = PortOpeningRequest {
            port: 9322,
            kind,
            place_id: None,
            label: None,
            transports: None,
            entry_methods: None,
            until: (kind == PortOpeningKind::Link)
                .then(|| Time::new(world.now() + chrono::Duration::hours(1))),
            host_port: None,
            program: None,
            reason: "review without a named place".into(),
            replacement_of: None,
            close_replaced: None,
        };
        let response = review_call(&world, env, &token, &body).await;
        assert_eq!(
            response.status,
            StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&response.raw)
        );
        let value: Json = serde_json::from_slice(&response.raw).unwrap();
        assert_eq!(value["projection"], "receipt");
        assert!(value.get("candidate").is_none() && value.get("blockers").is_none());
    }
}

#[tokio::test]
async fn review_creation_rechecks_its_key_after_the_actual_environment_lock() {
    for expire in [false, true] {
        let (world, manager, _, env, project, _) = setup().await;
        let token = key(
            &world,
            manager.id(),
            vec![
                env_rule(env, &["manage"]),
                node_rule("project", project, "self", OP),
            ],
        )
        .await;
        let mut blocker = world.db.pool.begin().await.unwrap();
        sqlx::query("SELECT id FROM environments WHERE id=$1 FOR UPDATE")
            .bind(env)
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = world.app.clone();
        let body = request(9323, project, "lock-bound review");
        let action = tokio::spawn(async move {
            let call = app
                .call(EnvironmentPortOpeningPreflight, &[&env, &"9323"])
                .token(&token)
                .body(&body);
            call.send_bytes().await
        });
        tokio::time::timeout(std::time::Duration::from_secs(30),async {loop {
            assert!(!action.is_finished(),"the actual review lock was not reached");
            let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query ILIKE '%SELECT id FROM environments%FOR UPDATE%')").bind(pid).fetch_one(&world.db.pool).await.unwrap();
            if blocked {break;}tokio::task::yield_now().await;
        }}).await.unwrap();
        if expire {
            world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let response = tokio::time::timeout(std::time::Duration::from_secs(30), action)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            response.status,
            if expire {
                StatusCode::UNAUTHORIZED
            } else {
                StatusCode::OK
            }
        );
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM preflights WHERE kind='port_opening' AND environment_id=$1",
        )
        .bind(env)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
        assert_eq!(count, if expire { 0 } else { 1 });
    }
}

async fn legacy(world: &World, user: Id) -> String {
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id: new_id(),
            user_id: user,
            name: "Independent ordinary confirmation",
            token_hash: &token_hash(&secret),
            scopes: &["*".to_owned()],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
    )
    .await
    .unwrap();
    secret
}

async fn confirm_call(
    world: &World,
    env: Id,
    port: u16,
    preflight: Id,
    token: &str,
) -> orch_api_core::testing::RawReply {
    let call = world
        .app
        .call(EnvironmentPortOpeningCreate, &[&env, &port.to_string()])
        .token(token)
        .body(&EnvironmentPortOpeningCreateBody {
            preflight_id: preflight,
            client_id: new_id(),
        });
    call.send_bytes().await
}

#[tokio::test]
async fn confirmation_keeps_the_saved_preflight_operation_and_original_key_independently_of_its_confirmer()
 {
    for mode in ["unchanged", "revoke", "scope", "expire"] {
        let (world, manager, _, env, project, _) = setup().await;
        let (key_id, creator) = key_record(
            &world,
            manager.id(),
            vec![
                env_rule(env, &["manage"]),
                node_rule("project", project, "self", OP),
            ],
        )
        .await;
        sqlx::query("UPDATE api_tokens SET expires_at=$2 WHERE id=$1")
            .bind(key_id)
            .bind(world.now() + chrono::Duration::minutes(2))
            .execute(&world.db.pool)
            .await
            .unwrap();
        let made = review_call(
            &world,
            env,
            &creator,
            &request(9331, project, "saved origin"),
        )
        .await;
        assert_eq!(made.status, StatusCode::OK);
        let saved: Json = serde_json::from_slice(&made.raw).unwrap();
        assert_eq!(
            saved["projection"], "receipt",
            "the creator never needs response Read or Create scope"
        );
        let preflight: Id = serde_json::from_value(saved["preflight_id"].clone()).unwrap();
        match mode {
            "revoke" => {
                sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
                    .bind(key_id)
                    .bind(world.now())
                    .execute(&world.db.pool)
                    .await
                    .unwrap();
            }
            "scope" => {
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1").bind(key_id).bind(json!({"rules":[env_rule(env, &["read"]),node_rule("project",project,"self",OP)]})).execute(&world.db.pool).await.unwrap();
            }
            "expire" => world.advance(chrono::Duration::minutes(3)),
            _ => {}
        }
        let actual = confirm_call(&world, env, 9331, preflight, &manager.token).await;
        assert_eq!(
            actual.status,
            if mode == "unchanged" {
                StatusCode::CREATED
            } else {
                StatusCode::FORBIDDEN
            }
        );
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM port_openings WHERE environment_id=$1")
                .bind(env)
                .fetch_one(&world.db.pool)
                .await
                .unwrap();
        assert_eq!(count, i64::from(mode == "unchanged"));
    }
}

#[tokio::test]
async fn confirmation_never_replaces_a_recorded_raw_review_session_with_the_persons_other_credential()
 {
    for remove in [false, true] {
        let (world, manager, _, env, project, _) = setup().await;
        let plan = review(
            &world,
            &manager,
            env,
            &request(9332, project, "raw review session"),
        )
        .await;
        let stored = forks::preflight(&mut *world.conn().await, plan.preflight_id)
            .await
            .unwrap()
            .unwrap();
        assert!(stored.plan["creator_session"].is_string());
        let confirmer = legacy(&world, manager.id()).await;
        if remove {
            sqlx::query("DELETE FROM sessions WHERE user_id=$1")
                .bind(manager.id())
                .execute(&world.db.pool)
                .await
                .unwrap();
        }
        let actual = confirm_call(&world, env, 9332, plan.preflight_id, &confirmer).await;
        assert_eq!(
            actual.status,
            if remove {
                StatusCode::FORBIDDEN
            } else {
                StatusCode::CREATED
            }
        );
    }
}

#[tokio::test]
async fn confirmation_rechecks_the_original_reviewer_after_the_actual_environment_wait() {
    for expire in [false, true] {
        let (world, manager, _, env, project, _) = setup().await;
        let (key_id, creator) = key_record(
            &world,
            manager.id(),
            vec![
                env_rule(env, &["manage"]),
                node_rule("project", project, "self", OP),
            ],
        )
        .await;
        sqlx::query("UPDATE api_tokens SET expires_at=$2 WHERE id=$1")
            .bind(key_id)
            .bind(world.now() + chrono::Duration::minutes(2))
            .execute(&world.db.pool)
            .await
            .unwrap();
        let made = review_call(
            &world,
            env,
            &creator,
            &request(9333, project, "waited saved review"),
        )
        .await;
        assert_eq!(made.status, StatusCode::OK);
        let preflight: Id = serde_json::from_value(
            serde_json::from_slice::<Json>(&made.raw).unwrap()["preflight_id"].clone(),
        )
        .unwrap();
        let mut blocker = world.db.pool.begin().await.unwrap();
        sqlx::query("SELECT id FROM environments WHERE id=$1 FOR UPDATE")
            .bind(env)
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = world.app.clone();
        let token = manager.token.clone();
        let pending = tokio::spawn(async move {
            let call = app
                .call(EnvironmentPortOpeningCreate, &[&env, &"9333"])
                .token(&token)
                .body(&EnvironmentPortOpeningCreateBody {
                    preflight_id: preflight,
                    client_id: new_id(),
                });
            call.send_bytes().await
        });
        tokio::time::timeout(std::time::Duration::from_secs(30), async { loop {
            assert!(!pending.is_finished(), "the actual confirmation lock must be reached");
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query ILIKE '%SELECT id FROM environments%FOR UPDATE%')").bind(pid).fetch_one(&world.db.pool).await.unwrap();
            if waiting { break; } tokio::task::yield_now().await;
        }}).await.unwrap();
        if expire {
            world.advance(chrono::Duration::minutes(3));
        }
        blocker.commit().await.unwrap();
        let actual = tokio::time::timeout(std::time::Duration::from_secs(30), pending)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            actual.status,
            if expire {
                StatusCode::FORBIDDEN
            } else {
                StatusCode::CREATED
            }
        );
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM port_openings WHERE environment_id=$1")
                .bind(env)
                .fetch_one(&world.db.pool)
                .await
                .unwrap();
        assert_eq!(count, i64::from(!expire));
    }
}

#[tokio::test]
async fn review_host_content_needs_its_actual_publisher_read_and_no_fabricated_host() {
    let (world, manager, _, env, project, machine) = setup().await;
    port_policy(&world, "port_host", "ask").await;
    let ports = orch_link_contract::policy::MachinePorts {
        allowed: vec![PortOpeningKind::Host],
        publish: Some(HostPorts {
            incoming_address: "review-native-publisher.example.test".into(),
            listen: "127.0.0.1".into(),
            ranges: vec![PortRange {
                from: 50200,
                to: 50202,
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
    let mut body = request(9334, project, "the review's actual publisher");
    body.kind = PortOpeningKind::Host;
    body.place_id = None;
    body.transports = Some(vec![PortTransport::Tcp]);
    for publisher in [
        None,
        Some(node_rule("machine", machine, "self", "MachineGet")),
        Some(node_rule("machine", machine, "self", OP)),
    ] {
        let full = publisher
            .as_ref()
            .is_some_and(|rule| rule["allow"][0]["operation"] == OP);
        let mut rules = vec![env_rule(env, &["manage", "read"])];
        rules.extend(publisher);
        let token = key(&world, manager.id(), rules).await;
        let actual = review_call(&world, env, &token, &body).await;
        assert_eq!(actual.status, StatusCode::OK);
        let value: Json = serde_json::from_slice(&actual.raw).unwrap();
        if full {
            assert_eq!(value["candidate"]["machine"]["id"], json!(machine));
            assert!(value["candidate"]["host"].is_null());
        } else {
            assert_eq!(value["projection"], "receipt");
            assert!(value.get("candidate").is_none() && value.get("blockers").is_none());
        }
    }
}

#[tokio::test]
async fn confirmation_rolls_back_all_native_sql_if_its_recorded_reviewer_ends_during_the_actual_audit_wait()
 {
    for expire in [false, true] {
        let (world, manager, _, env, project, _) = setup().await;
        let (key_id, creator) = key_record(
            &world,
            manager.id(),
            vec![
                env_rule(env, &["manage"]),
                node_rule("project", project, "self", OP),
            ],
        )
        .await;
        sqlx::query("UPDATE api_tokens SET expires_at=$2 WHERE id=$1")
            .bind(key_id)
            .bind(world.now() + chrono::Duration::minutes(2))
            .execute(&world.db.pool)
            .await
            .unwrap();
        let made = review_call(
            &world,
            env,
            &creator,
            &request(9335, project, "commit-bound original reviewer"),
        )
        .await;
        assert_eq!(made.status, StatusCode::OK);
        let preflight: Id = serde_json::from_value(
            serde_json::from_slice::<Json>(&made.raw).unwrap()["preflight_id"].clone(),
        )
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
        let token = manager.token.clone();
        let pending = tokio::spawn(async move {
            let call = app
                .call(EnvironmentPortOpeningCreate, &[&env, &"9335"])
                .token(&token)
                .body(&EnvironmentPortOpeningCreateBody {
                    preflight_id: preflight,
                    client_id: new_id(),
                });
            call.send_bytes().await
        });
        tokio::time::timeout(std::time::Duration::from_secs(30), async { loop {
            assert!(!pending.is_finished(), "the actual native audit wait must be reached");
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query ILIKE '%INSERT INTO audit_log%')").bind(pid).fetch_one(&world.db.pool).await.unwrap();
            if waiting { break; } tokio::task::yield_now().await;
        }}).await.unwrap();
        if expire {
            world.advance(chrono::Duration::minutes(3));
        }
        blocker.commit().await.unwrap();
        let actual = tokio::time::timeout(std::time::Duration::from_secs(30), pending)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            actual.status,
            if expire {
                StatusCode::FORBIDDEN
            } else {
                StatusCode::CREATED
            }
        );
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM port_openings WHERE environment_id=$1")
                .bind(env)
                .fetch_one(&world.db.pool)
                .await
                .unwrap();
        assert_eq!(count, i64::from(!expire));
    }
}

#[tokio::test]
async fn completed_review_keeps_its_receipt_after_publication_wait_and_never_replays_its_planner_or_writer()
 {
    for mode in ["unchanged", "read_removed", "manage_removed", "expire"] {
        let (world, manager, _, env, project, _) = setup().await;
        let root: Id = sqlx::query_scalar("SELECT org_id FROM nodes WHERE id=$1")
            .bind(project)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
        let initial = vec![
            env_rule(env, &["manage", "read"]),
            node_rule("org", root, "subtree", OP),
        ];
        let (key_id, token) = key_record(&world, manager.id(), initial).await;
        // Establish the positive source before installing a barrier an unbound route cannot reach.
        let warm = review_call(
            &world,
            env,
            &token,
            &request(9336, project, "first native review"),
        )
        .await;
        assert_eq!(warm.status, StatusCode::OK);
        let old: Id = serde_json::from_value(
            serde_json::from_slice::<Json>(&warm.raw).unwrap()["preflight_id"].clone(),
        )
        .unwrap();
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
        let body = request(9337, project, "frozen native review");
        let pending = tokio::spawn(async move {
            let call = app
                .call(EnvironmentPortOpeningPreflight, &[&env, &"9337"])
                .token(&token)
                .body(&body);
            call.send_bytes().await
        });
        tokio::time::timeout(std::time::Duration::from_secs(30), async { loop {
            assert!(!pending.is_finished(), "the actual review activity wait must be reached");
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query ILIKE '%INSERT INTO api_token_activity%')").bind(pid).fetch_one(&world.db.pool).await.unwrap();
            if waiting { break; } tokio::task::yield_now().await;
        }}).await.unwrap();
        let (saved, version): (Id, String) = sqlx::query_as(
            "SELECT id,xmin::text FROM preflights WHERE environment_id=$1 AND id<>$2",
        )
        .bind(env)
        .bind(old)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
        match mode {
            "read_removed" => {
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1").bind(key_id).bind(json!({"rules":[env_rule(env,&["manage"]),node_rule("org",root,"subtree",OP)]})).execute(&world.db.pool).await.unwrap();
            }
            "manage_removed" => {
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1").bind(key_id).bind(json!({"rules":[env_rule(env,&["read"]),node_rule("org",root,"subtree",OP)]})).execute(&world.db.pool).await.unwrap();
            }
            "expire" => world.advance(chrono::Duration::hours(2)),
            _ => {}
        }
        blocker.commit().await.unwrap();
        let actual = tokio::time::timeout(std::time::Duration::from_secs(30), pending)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(actual.status, StatusCode::OK);
        let value: Json = serde_json::from_slice(&actual.raw).unwrap();
        assert_eq!(value["preflight_id"], json!(saved));
        if matches!(mode, "read_removed" | "expire") {
            assert_eq!(value["projection"], "receipt");
            assert!(value.get("target").is_none() && value.get("candidate").is_none());
        } else {
            assert!(value.get("target").is_some() && value.get("projection").is_none());
        }
        let unchanged: String = sqlx::query_scalar("SELECT xmin::text FROM preflights WHERE id=$1")
            .bind(saved)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
        assert_eq!(unchanged, version);
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM preflights WHERE environment_id=$1")
                .bind(env)
                .fetch_one(&world.db.pool)
                .await
                .unwrap();
        assert_eq!(count, 2);
    }
}

#[tokio::test]
async fn completed_confirmation_replay_keeps_its_native_outcome_after_the_original_review_key_ends()
{
    let (world, manager, _, env, project, _) = setup().await;
    let (key_id, creator) = key_record(
        &world,
        manager.id(),
        vec![
            env_rule(env, &["manage"]),
            node_rule("project", project, "self", OP),
        ],
    )
    .await;
    let made = review_call(
        &world,
        env,
        &creator,
        &request(9338, project, "completed original review"),
    )
    .await;
    assert_eq!(made.status, StatusCode::OK);
    let preflight: Id = serde_json::from_value(
        serde_json::from_slice::<Json>(&made.raw).unwrap()["preflight_id"].clone(),
    )
    .unwrap();
    let body = EnvironmentPortOpeningCreateBody {
        preflight_id: preflight,
        client_id: new_id(),
    };
    let call = || {
        let request = world
            .app
            .call(EnvironmentPortOpeningCreate, &[&env, &"9338"])
            .token(&manager.token)
            .body(&body);
        request.send_bytes()
    };
    let first = call().await;
    assert_eq!(first.status, StatusCode::CREATED);
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
        .bind(key_id)
        .bind(world.now())
        .execute(&world.db.pool)
        .await
        .unwrap();
    let replay = call().await;
    assert_eq!(replay.status, StatusCode::OK);
    let a: Json = serde_json::from_slice(&first.raw).unwrap();
    let b: Json = serde_json::from_slice(&replay.raw).unwrap();
    assert_eq!(a["opening"]["id"], b["opening"]["id"]);
    assert!(b["link"].is_null());
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM port_openings WHERE environment_id=$1")
            .bind(env)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(count, 1);
}
