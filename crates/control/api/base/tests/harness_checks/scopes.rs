//! Native exact-version intent, claim identity and actual checker stage boundaries.
use super::*;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::admin::{HarnessVersionFetch, HarnessVersionFetchBody};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_db::queries::identity::{self, NewApiToken};
use orch_engine::build_checks::{Claim, Work};
use orch_types::{Json, new_id};

pub(super) async fn key(world: &World, user: Id, rules: Vec<Json>) -> String {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &NewApiToken {
            id: new_id(),
            user_id: user,
            name: "Native exact-version check",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::seconds(10)),
        },
        Some(&serde_json::to_value(scope).unwrap()),
    )
    .await
    .unwrap();
    secret
}
pub(super) fn system(operation: &str, action: &str) -> Json {
    json!({"target":{"kind":"system","reach":"self"},"allow":[{"operation":operation,"actions":[action]}]})
}
fn object(kind: &str, id: Id, operation: &str, action: &str) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"allow":[{"operation":operation,"actions":[action]}]})
}
async fn fetch(
    world: &World,
    secret: &str,
    body: &HarnessVersionFetchBody,
) -> orch_api_core::testing::RawReply {
    world
        .app
        .call(HarnessVersionFetch, &[&"codex", &VERSION])
        .token(secret)
        .body(body)
        .send_bytes()
        .await
}
fn accepted(reply: orch_api_core::testing::RawReply) -> Json {
    assert_eq!(reply.status, 202, "{}", String::from_utf8_lossy(&reply.raw));
    serde_json::from_slice(&reply.raw).unwrap()
}
async fn check_as(world: &World, secret: &str, program: bool) -> orch_api_core::testing::RawReply {
    if program {
        world
            .app
            .call(
                orch_api_contract::program_admin::AdminProgramBuildCheck,
                &[&"codex", &VERSION],
            )
            .token(secret)
            .send_bytes()
            .await
    } else {
        world
            .app
            .call(AdminHarnessCheck, &[&"codex", &VERSION])
            .token(secret)
            .send_bytes()
            .await
    }
}

#[tokio::test]
async fn administrative_checks_bind_their_actual_operation_before_retaining_a_native_claim() {
    for (program, operation, other) in [
        (false, "AdminHarnessCheck", "AdminProgramBuildCheck"),
        (true, "AdminProgramBuildCheck", "AdminHarnessCheck"),
    ] {
        for ordinary in [false, true] {
            let world = World::new().await;
            let c = cast(&world).await;
            let build = channel_build();
            keep(&world, &build).await;
            let row_key = orch_api_core::harness_channels::channel_key(&build);
            for (user, rules) in [
                (c.root.id(), vec![system(operation, "read")]),
                (c.root.id(), vec![system(other, "manage")]),
                (
                    c.boss.id(),
                    vec![system(operation, "manage"), system(operation, "read")],
                ),
            ] {
                let secret = key(&world, user, rules).await;
                let denied = check_as(&world, &secret, program).await;
                assert_eq!(
                    denied.status.as_u16(),
                    403,
                    "{}",
                    String::from_utf8_lossy(&denied.raw)
                );
                assert!(
                    Claim::load(&mut *world.conn().await, &row_key)
                        .await
                        .unwrap()
                        .is_none(),
                    "a refused check cannot claim a native build"
                );
            }
            let secret = if ordinary {
                c.root.token.clone()
            } else {
                let mut rules = vec![system(operation, "manage")];
                if !program {
                    rules.push(system(operation, "read"));
                }
                key(&world, c.root.id(), rules).await
            };
            let reply = check_as(&world, &secret, program).await;
            assert_eq!(
                reply.status.as_u16(),
                200,
                "{}",
                String::from_utf8_lossy(&reply.raw)
            );
            let body: Json = serde_json::from_slice(&reply.raw).unwrap();
            if program {
                assert_eq!(body, json!({"queued":1}));
            } else {
                assert!(
                    body.as_array()
                        .unwrap()
                        .iter()
                        .any(|build| { build["kind"] == "codex" && build["version"] == VERSION })
                );
            }
            let claim = Claim::load(&mut *world.conn().await, &row_key)
                .await
                .unwrap()
                .unwrap();
            let retained = serde_json::to_value(claim).unwrap();
            assert_eq!(retained["origin"]["kind"], "requested");
            assert_eq!(
                retained["origin"]["request"]["origin"]["operation"], operation,
                "the worker keeps this route's admitted operation"
            );
            assert_eq!(
                checked(&world, &c.root).await.status,
                HarnessBuildStatus::Untested,
                "without a host the same native check stays untested"
            );
        }
    }
}

async fn command(world: &World, machine: Id, kind: &str) -> Id {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if let Some(id) = sqlx::query_scalar("SELECT id FROM commands WHERE machine_id=$1 AND kind=$2 ORDER BY created_at,id LIMIT 1")
                .bind(machine).bind(kind).fetch_optional(&world.db.pool).await.unwrap() { break id; }
            tokio::task::yield_now().await;
        }
    }).await.expect("the real native command insertion")
}
async fn answer(world: &World, id: Id, kind: &str) {
    let result = match kind {
        "node.tool.ensure" => json!({"path":format!("/tools/codex/{VERSION}")}),
        "node.tool.check" => json!({"missing":[]}),
        _ => {
            json!({"cases":orch_agent_programs::Programs::builtin().program("codex").unwrap().checks.cases()
            .map(|(name,_)| json!({"name":name,"passed":true,"elapsed_ms":1})).collect::<Vec<_>>()})
        }
    };
    sqlx::query("UPDATE commands SET state='succeeded',result=$2 WHERE id=$1")
        .bind(id)
        .bind(result)
        .execute(&world.db.pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn exact_fetch_binds_native_destination_variants_and_its_own_optional_catalogue_read() {
    let world = World::new().await;
    let c = cast(&world).await;
    keep(&world, &channel_build()).await;
    let environment = orch_db::queries::resources::access_fixtures::insert_resource(
        &mut *world.conn().await,
        "environment",
        Some(c.boss.id()),
        Some(c.acme),
        "native-fetch-environment",
    )
    .await
    .unwrap();
    for (actor, operation, body, target) in [
        (
            c.root.id(),
            "HarnessVersionFetch.system",
            HarnessVersionFetchBody::default(),
            system("HarnessVersionFetch.system", "manage"),
        ),
        (
            c.boss.id(),
            "HarnessVersionFetch.node",
            HarnessVersionFetchBody {
                environment_id: None,
                node_id: Some(c.acme),
            },
            object("org", c.acme, "HarnessVersionFetch.node", "manage"),
        ),
        (
            c.boss.id(),
            "HarnessVersionFetch.environment",
            HarnessVersionFetchBody {
                environment_id: Some(environment),
                node_id: None,
            },
            object(
                "environment",
                environment,
                "HarnessVersionFetch.environment",
                "manage",
            ),
        ),
    ] {
        let write = key(
            &world,
            actor,
            vec![target.clone(), system("HarnessBuildList", "read")],
        )
        .await;
        let receipt = accepted(fetch(&world, &write, &body).await);
        assert_eq!(
            receipt["projection"], "receipt",
            "another operation cannot lend catalogue Read"
        );
        assert_eq!(receipt["kind"], "codex");
        assert_eq!(receipt["version"], VERSION);
        assert!(receipt.get("sha256").is_none() && receipt.get("check").is_none());
        let with_read = key(&world, actor, vec![target, system(operation, "read")]).await;
        let full = accepted(fetch(&world, &with_read, &body).await);
        assert!(
            full.as_array()
                .unwrap()
                .iter()
                .any(|build| build["kind"] == "codex" && build["version"] == VERSION)
        );
    }
    let invalid_key = key(
        &world,
        c.root.id(),
        vec![
            system("HarnessVersionFetch.system", "manage"),
            object("org", c.acme, "HarnessVersionFetch.node", "manage"),
            object(
                "environment",
                environment,
                "HarnessVersionFetch.environment",
                "manage",
            ),
        ],
    )
    .await;
    let invalid = fetch(
        &world,
        &invalid_key,
        &HarnessVersionFetchBody {
            environment_id: Some(environment),
            node_id: Some(c.acme),
        },
    )
    .await
    .problem();
    assert!(
        invalid.extra["errors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|fault| fault["path"] == "environment_id"),
        "the native body rejects both destinations rather than selecting one"
    );
    let read = key(
        &world,
        c.root.id(),
        vec![system("HarnessVersionFetch.system", "read")],
    )
    .await;
    assert_eq!(
        fetch(&world, &read, &HarnessVersionFetchBody::default())
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
    let outsider = key(
        &world,
        c.ada.id(),
        vec![object("org", c.acme, "HarnessVersionFetch.node", "manage")],
    )
    .await;
    assert_eq!(
        fetch(
            &world,
            &outsider,
            &HarnessVersionFetchBody {
                environment_id: None,
                node_id: Some(c.acme)
            }
        )
        .await
        .problem()
        .code,
        ErrorCode::AuthzDenied
    );
    let no_standing = key(
        &world,
        c.boss.id(),
        vec![system("HarnessVersionFetch.system", "manage")],
    )
    .await;
    assert_eq!(
        fetch(&world, &no_standing, &HarnessVersionFetchBody::default())
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
}

#[tokio::test]
async fn exact_fetch_keeps_original_authority_through_each_command_and_positive_certification() {
    for stop in [
        None,
        Some("node.tool.ensure"),
        Some("node.tool.check"),
        Some("node.tool.conformance"),
    ] {
        let world = World::new().await;
        let c = cast(&world).await;
        keep(&world, &channel_build()).await;
        let machine = system_host(&world).await;
        let secret = key(
            &world,
            c.root.id(),
            vec![system("HarnessVersionFetch.system", "manage")],
        )
        .await;
        assert_eq!(
            accepted(fetch(&world, &secret, &HarnessVersionFetchBody::default()).await)["projection"],
            "receipt"
        );
        let mut expected = 0;
        for kind in [
            "node.tool.ensure",
            "node.tool.check",
            "node.tool.conformance",
        ] {
            let id = command(&world, machine, kind).await;
            let origin: Json =
                sqlx::query_scalar("SELECT login_authority FROM commands WHERE id=$1")
                    .bind(id)
                    .fetch_one(&world.db.pool)
                    .await
                    .unwrap();
            assert_eq!(origin["kind"], "harness_check");
            assert_eq!(
                origin["authority"]["work"]["claim"]["origin"]["request"]["origin"]["operation"],
                "HarnessVersionFetch.system"
            );
            if stop == Some(kind) {
                world.advance(chrono::Duration::seconds(11));
            }
            answer(&world, id, kind).await;
            expected += 1;
            if stop == Some(kind) {
                break;
            }
        }
        let result = checked(&world, &c.root).await;
        assert_eq!(
            result.status,
            if stop.is_none() {
                HarnessBuildStatus::Checked
            } else {
                HarnessBuildStatus::Untested
            }
        );
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM commands WHERE machine_id=$1 AND kind LIKE 'node.tool.%'",
        )
        .bind(machine)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
        assert_eq!(
            count, expected,
            "a closed origin starts no subsequent command or positive certification"
        );
    }
}

#[tokio::test]
async fn exact_fetch_worker_rechecks_scope_revocation_and_current_system_standing_after_download() {
    for change in ["scope", "revoke", "standing"] {
        let world = World::new().await;
        let c = cast(&world).await;
        keep(&world, &channel_build()).await;
        let machine = system_host(&world).await;
        let secret = key(
            &world,
            c.root.id(),
            vec![system("HarnessVersionFetch.system", "manage")],
        )
        .await;
        accepted(fetch(&world, &secret, &HarnessVersionFetchBody::default()).await);
        let id = command(&world, machine, "node.tool.ensure").await;
        match change {
            "scope" => {
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE token_hash=$1")
                    .bind(token_hash(&secret))
                    .bind(json!({"rules":[]}))
                    .execute(&world.db.pool)
                    .await
                    .unwrap();
            }
            "revoke" => {
                sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE token_hash=$1")
                    .bind(token_hash(&secret))
                    .bind(world.now())
                    .execute(&world.db.pool)
                    .await
                    .unwrap();
            }
            _ => {
                sqlx::query("UPDATE users SET is_system_admin=false WHERE id=$1")
                    .bind(c.root.id())
                    .execute(&world.db.pool)
                    .await
                    .unwrap();
            }
        }
        answer(&world, id, "node.tool.ensure").await;
        let key = orch_api_core::harness_channels::channel_key(&channel_build());
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let status: String = sqlx::query_scalar(
                    "SELECT value->>'status' FROM settings WHERE node_id IS NULL AND key=$1",
                )
                .bind(&key)
                .fetch_one(&world.db.pool)
                .await
                .unwrap();
                if status == "untested" {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let commands: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM commands WHERE machine_id=$1 AND kind LIKE 'node.tool.%'",
        )
        .bind(machine)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
        assert_eq!(
            commands, 1,
            "the original {change} must stop interface execution after the accepted download"
        );
    }
}

async fn work_current(world: &World, work: &Work) -> bool {
    let mut tx = world.app.app.db.begin().await.unwrap();
    let valid = match work.checkpoint(&mut tx, world.app.app.clock.as_ref()).await {
        Ok(Some(current)) => current.before.check(world.app.app.clock.as_ref()).is_ok(),
        Ok(None) => true,
        Err(_) => false,
    };
    tx.commit().await.unwrap();
    valid
}

#[tokio::test]
async fn native_build_work_cannot_borrow_a_new_claim_or_a_replacement_physical_host() {
    let world = World::new().await;
    let c = cast(&world).await;
    let build = channel_build();
    keep(&world, &build).await;
    let machine = system_host(&world).await;
    let secret = key(
        &world,
        c.root.id(),
        vec![system("HarnessVersionFetch.system", "manage")],
    )
    .await;
    accepted(fetch(&world, &secret, &HarnessVersionFetchBody::default()).await);
    let id = command(&world, machine, "node.tool.ensure").await;
    let key = orch_api_core::harness_channels::channel_key(&build);
    let old = Claim::load(&mut *world.conn().await, &key)
        .await
        .unwrap()
        .unwrap();
    let host = orch_db::queries::resources::hosting::hosts(&mut *world.conn().await, None)
        .await
        .unwrap()
        .into_iter()
        .find(|host| host.machine_id == machine)
        .unwrap();
    let work = Work::new(old.clone()).on_host(&host);
    assert!(work_current(&world, &work).await);
    assert!(
        orch_api_core::harness_checks::finish(
            &world.app.app,
            "another-build-row",
            &build,
            old.clone()
        )
        .await
        .is_err(),
        "a writer must use the exact row owned by its admitted native claim"
    );
    let wrong = orch_engine::commands::Outgoing::new(
        new_id(),
        machine,
        orch_link_contract::commands::SerialKey::Tool(build.kind.clone()),
        &orch_link_contract::commands::NodeToolEnsure {
            job_id: None,
            name: build.kind.clone(),
            version: build.version.clone(),
            url: build.url.clone(),
            sha256: "d".repeat(64),
        },
    )
    .unwrap();
    assert!(
        orch_engine::build_checks::CheckCommand::capture(&work, &wrong).is_err(),
        "an allowed destination cannot validate another build's physical command"
    );
    sqlx::query("UPDATE hosts SET disabled=true WHERE id=$1")
        .bind(host.id)
        .execute(&world.db.pool)
        .await
        .unwrap();
    assert!(
        !work_current(&world, &work).await,
        "current physical eligibility is part of the actual native source"
    );
    sqlx::query("UPDATE hosts SET disabled=false WHERE id=$1")
        .bind(host.id)
        .execute(&world.db.pool)
        .await
        .unwrap();
    assert!(work_current(&world, &work).await);
    orch_db::queries::resources::management::set_harness_build_status(
        &mut *world.conn().await,
        &key,
        "untested",
        None,
        world.now(),
    )
    .await
    .unwrap();
    assert!(
        orch_api_core::harness_checks::claim(&world.app.app, &key)
            .await
            .unwrap()
    );
    let next = Claim::load(&mut *world.conn().await, &key)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(
        old, next,
        "the explicit scheduled claim has a new immutable identity"
    );
    assert!(
        !work_current(&world, &work).await,
        "the old requested work cannot borrow that scheduled authority"
    );
    orch_api_core::harness_checks::finish(&world.app.app, &key, &build, old)
        .await
        .unwrap();
    assert_eq!(
        Claim::load(&mut *world.conn().await, &key).await.unwrap(),
        Some(next)
    );
    let status: String = sqlx::query_scalar(
        "SELECT value->>'status' FROM settings WHERE node_id IS NULL AND key=$1",
    )
    .bind(&key)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(
        status, "checking",
        "old accepted cleanup must not release the new check"
    );
    answer(&world, id, "node.tool.ensure").await;
}
