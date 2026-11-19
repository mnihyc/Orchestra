//! Native directory registration, actual capacity waits and completed singleton publication.

use super::*;
use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_db::queries::identity::{self, NewApiToken};
use orch_types::{Json, new_id};
use std::time::Duration;

const OP: &str = "MachineEnvironmentCreate";
const WAIT: Duration = Duration::from_secs(30);

fn rule(op: &str, kind: &str, id: Id, reach: &str, resources: &[&str], actions: &[&str]) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":reach},"resources":resources,
        "allow":[{"operation":op,"actions":actions}]})
}
fn act(machine: Id, home: Id) -> Vec<Json> {
    vec![
        rule(OP, "machine", machine, "self", &["machine"], &["write"]),
        rule(OP, "org", home, "subtree", &["environment"], &["manage"]),
    ]
}
fn full(machine: Id, home: Id) -> Vec<Json> {
    vec![
        rule(
            OP,
            "machine",
            machine,
            "self",
            &["machine"],
            &["write", "read"],
        ),
        rule(
            OP,
            "org",
            home,
            "subtree",
            &["environment"],
            &["manage", "read"],
        ),
        rule(
            OP,
            "org",
            home,
            "subtree",
            &["org", "project", "topic", "thread"],
            &["read"],
        ),
    ]
}
async fn key(world: &World, user: Id, rules: Vec<Json>) -> (Id, String) {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &NewApiToken {
            id,
            user_id: user,
            name: "Directory registration source",
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
    (id, secret)
}
async fn call(
    world: &World,
    token: &str,
    machine: Id,
    wanted: &MachineEnvironmentCreateBody,
) -> RawReply {
    world
        .app
        .call(MachineEnvironmentCreate, &[&machine])
        .token(token)
        .body(wanted)
        .send_bytes()
        .await
}
fn success(reply: &RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
async fn counts(world: &World) -> (i64, i64, i64, i64) {
    sqlx::query_as(
        "SELECT (SELECT count(*) FROM environments),
        (SELECT count(*) FROM permission_entries WHERE object_kind='environment'),
        (SELECT count(*) FROM audit_log WHERE action='environment.created'),
        (SELECT count(*) FROM jobs)",
    )
    .fetch_one(&world.db.pool)
    .await
    .unwrap()
}
async fn receipt(world: &World, reply: &RawReply, name: &str) -> Id {
    let value = success(reply);
    let id: Id = serde_json::from_value(value["environment_id"].clone()).unwrap();
    let (revision, stored_name): (i64, String) =
        sqlx::query_as("SELECT revision,name FROM environments WHERE id=$1")
            .bind(id)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(stored_name, name);
    assert_eq!(
        value,
        json!({"projection":"receipt","environment_id":id,"revision":revision})
    );
    id
}
async fn wait_sql(
    world: &World,
    pid: i32,
    pattern: &str,
    request: &tokio::task::JoinHandle<RawReply>,
) {
    tokio::time::timeout(WAIT, async {
        loop {
            assert!(
                !request.is_finished(),
                "the native request must reach the actual SQL wait"
            );
            let blocked: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity
                WHERE datname=current_database() AND wait_event_type='Lock'
                AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE $2)",
            )
            .bind(pid)
            .bind(pattern)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
            if blocked {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the real registration SQL wait was not observed");
}

#[tokio::test]
async fn registration_requires_the_physical_machine_and_actual_home_manage_without_effects_on_denial()
 {
    let world = world().await;
    let (c, laptop) = setup(&world).await;
    let project = make_project(&world, &c.admin, c.acme, "Registration settings").await;
    let home = project.base.id;
    let mut wanted = body(c.acme, "Actual selected home", "/srv/ada/actual");
    wanted.home_node_id = Some(home);
    let valid = vec![
        rule(OP, "machine", laptop, "self", &["machine"], &["write"]),
        rule(OP, "project", home, "self", &["environment"], &["manage"]),
    ];
    let before = counts(&world).await;
    for rules in [
        vec![valid[0].clone()],
        vec![valid[1].clone()],
        vec![
            valid[0].clone(),
            rule(OP, "org", c.acme, "self", &["environment"], &["manage"]),
        ],
        vec![
            rule(OP, "host", laptop, "self", &["machine"], &["write"]),
            valid[1].clone(),
        ],
        vec![
            valid[0].clone(),
            rule(OP, "project", home, "self", &["environment"], &["write"]),
        ],
        vec![
            rule(
                "MachineProviderCreate",
                "machine",
                laptop,
                "self",
                &["machine"],
                &["write"],
            ),
            valid[1].clone(),
        ],
    ] {
        let (_, token) = key(&world, c.admin.id(), rules).await;
        let denied = call(&world, &token, laptop, &wanted).await;
        assert_eq!(
            denied.status,
            StatusCode::FORBIDDEN,
            "{}",
            String::from_utf8_lossy(&denied.raw)
        );
        assert_eq!(
            counts(&world).await,
            before,
            "a refused role must not register or reserve anything"
        );
    }
    let (_, token) = key(&world, c.admin.id(), valid).await;
    let id = receipt(
        &world,
        &call(&world, &token, laptop, &wanted).await,
        &wanted.name,
    )
    .await;
    let selected: (Id, Id, Id) =
        sqlx::query_as("SELECT owner_id,home_node_id,machine_id FROM environments WHERE id=$1")
            .bind(id)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(selected, (c.admin.id(), home, laptop));
}

#[tokio::test]
async fn registration_write_and_manage_keep_receipts_and_borrow_no_get_read() {
    let world = world().await;
    let (c, laptop) = setup(&world).await;
    let mut rules = act(laptop, c.acme);
    rules.push(rule(
        "EnvironmentGet.read",
        "org",
        c.acme,
        "subtree",
        &["environment"],
        &["read"],
    ));
    let (_, token) = key(&world, c.admin.id(), rules).await;
    let mut wanted = body(
        c.acme,
        "Receipt registration marker",
        "/srv/ada/receipt-marker",
    );
    wanted.access = Some(NewEnvironmentAccess::OnlyMe);
    let id = receipt(
        &world,
        &call(&world, &token, laptop, &wanted).await,
        &wanted.name,
    )
    .await;
    let ordinary = world
        .app
        .call(EnvironmentGet, &[&id])
        .token(&c.admin.token)
        .send()
        .await
        .ok();
    assert_eq!(ordinary.base.owner.id, c.admin.id());
    assert_eq!(ordinary.workspace_root, wanted.root);
    assert_eq!(ordinary.revision, 1);
    let owners:Vec<Id>=sqlx::query_scalar("SELECT holder_id FROM permission_entries WHERE object_kind='environment' AND object_id=$1 AND permission='access.own' AND removed_at IS NULL ORDER BY holder_id")
        .bind(id).fetch_all(&world.db.pool).await.unwrap();
    assert_eq!(owners, [c.admin.id()]);
    let (_, reader) = key(&world, c.admin.id(), full(laptop, c.acme)).await;
    let native = success(
        &call(
            &world,
            &reader,
            laptop,
            &body(c.acme, "Full native registration", "/srv/ada/full"),
        )
        .await,
    );
    assert_eq!(native["name"], "Full native registration");
    assert_eq!(native["machine"]["id"], json!(laptop));
    assert_eq!(native["machine"]["name"], "laptop");
    assert_eq!(native["home"]["id"], json!(c.acme));
    assert!(native.get("projection").is_none());
}

#[tokio::test]
async fn registration_does_not_replace_native_domain_local_roots_or_machine_use() {
    let world = world().await;
    let (c, laptop) = setup(&world).await;
    let before = counts(&world).await;
    let (_, token) = key(&world, c.admin.id(), act(laptop, c.acme)).await;
    assert_eq!(
        call(
            &world,
            &token,
            laptop,
            &body(c.acme, "Outside roots", "/outside/work")
        )
        .await
        .status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(counts(&world).await, before);
    let home = places::make_org(
        &world,
        &c.stranger,
        "Independent registration home",
        "independent-registration",
    )
    .await
    .base
    .id;
    let (_, stranger) = key(&world, c.stranger.id(), act(laptop, home)).await;
    let wanted = body(home, "Needs native machine use", "/srv/ada/borrowed");
    assert_eq!(
        call(&world, &stranger, laptop, &wanted).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(counts(&world).await, before);
    let (_, outside) = key(&world, c.admin.id(), act(laptop, home)).await;
    assert_eq!(
        call(&world, &outside, laptop, &wanted).await.status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(counts(&world).await, before);
    let entry = name_person(&world, "machine", laptop, "machine.use", c.stranger.id()).await;
    let id = receipt(
        &world,
        &call(&world, &stranger, laptop, &wanted).await,
        &wanted.name,
    )
    .await;
    let stored: (Id, Option<Id>) =
        sqlx::query_as("SELECT owner_id,host_entry_id FROM environments WHERE id=$1")
            .bind(id)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(stored, (c.stranger.id(), Some(entry)));
    orch_db::queries::access::remove_entry(
        &mut *world.conn().await,
        entry,
        c.admin.id(),
        world.now(),
    )
    .await
    .unwrap();
    let after = counts(&world).await;
    assert_eq!(
        call(
            &world,
            &stranger,
            laptop,
            &body(home, "After withdrawal", "/srv/ada/after")
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(counts(&world).await, after);
}

#[tokio::test]
async fn registration_rechecks_the_original_deadline_after_actual_machine_and_home_locks() {
    for (on_machine, expired) in [(true, false), (true, true), (false, false), (false, true)] {
        let world = world().await;
        let (c, laptop) = setup(&world).await;
        let (_, token) = key(&world, c.admin.id(), act(laptop, c.acme)).await;
        let before = counts(&world).await;
        let mut blocker = world.db.pool.begin().await.unwrap();
        if on_machine {
            sqlx::query("SELECT id FROM machines WHERE id=$1 FOR UPDATE")
                .bind(laptop)
                .execute(&mut *blocker)
                .await
                .unwrap();
        } else {
            sqlx::query("SELECT id FROM nodes WHERE id=$1 FOR UPDATE")
                .bind(c.acme)
                .execute(&mut *blocker)
                .await
                .unwrap();
        }
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = world.app.clone();
        let wanted = body(c.acme, "Guarded native registration", "/srv/ada/guarded");
        let request = tokio::spawn(async move {
            let call = app.call(MachineEnvironmentCreate, &[&laptop]);
            call.token(&token).body(&wanted).send_bytes().await
        });
        wait_sql(
            &world,
            pid,
            if on_machine {
                "%FROM machines%FOR SHARE%"
            } else {
                "%FROM nodes%FOR SHARE%"
            },
            &request,
        )
        .await;
        assert_eq!(
            counts(&world).await,
            before,
            "the observed wait is before the actual registration"
        );
        if expired {
            world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(WAIT, request).await.unwrap().unwrap();
        if expired {
            assert!(
                matches!(
                    reply.status,
                    StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
                ),
                "{}",
                String::from_utf8_lossy(&reply.raw)
            );
            assert_eq!(counts(&world).await, before);
        } else {
            receipt(&world, &reply, "Guarded native registration").await;
        }
    }
}

#[tokio::test]
async fn registration_rechecks_native_bounded_use_after_its_real_capacity_entry_wait() {
    for mode in ["live", "expired", "capacity_lost"] {
        let world = world().await;
        let (c, laptop) = setup(&world).await;
        let home = places::make_org(&world, &c.stranger, "Capacity home", "capacity-home")
            .await
            .base
            .id;
        let entry = name_person(&world, "machine", laptop, "machine.use", c.stranger.id()).await;
        sqlx::query("UPDATE permission_entries SET options=$2 WHERE id=$1")
            .bind(entry)
            .bind(json!({"max_environments":1}))
            .execute(&world.db.pool)
            .await
            .unwrap();
        let (_, token) = key(&world, c.stranger.id(), act(laptop, home)).await;
        let before = counts(&world).await;
        let mut blocker = world.db.pool.begin().await.unwrap();
        sqlx::query("SELECT id FROM permission_entries WHERE id=$1 FOR UPDATE")
            .bind(entry)
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = world.app.clone();
        let wanted = body(home, "Capacity registration", "/srv/ada/capacity");
        let request = tokio::spawn(async move {
            let call = app.call(MachineEnvironmentCreate, &[&laptop]);
            call.token(&token).body(&wanted).send_bytes().await
        });
        wait_sql(
            &world,
            pid,
            "%permission_entries%FOR NO KEY UPDATE%",
            &request,
        )
        .await;
        assert_eq!(counts(&world).await, before);
        match mode {
            "expired" => world.advance(chrono::Duration::hours(2)),
            "capacity_lost" => {
                sqlx::query("UPDATE permission_entries SET options=$2 WHERE id=$1")
                    .bind(entry)
                    .bind(json!({"max_environments":0}))
                    .execute(&mut *blocker)
                    .await
                    .unwrap();
            }
            _ => {}
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(WAIT, request).await.unwrap().unwrap();
        if mode == "live" {
            let id = receipt(&world, &reply, "Capacity registration").await;
            let used: Option<Id> =
                sqlx::query_scalar("SELECT host_entry_id FROM environments WHERE id=$1")
                    .bind(id)
                    .fetch_one(&world.db.pool)
                    .await
                    .unwrap();
            assert_eq!(used, Some(entry));
        } else {
            assert!(
                matches!(
                    reply.status,
                    StatusCode::NOT_FOUND | StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
                ),
                "{}",
                String::from_utf8_lossy(&reply.raw)
            );
            assert_eq!(
                counts(&world).await,
                before,
                "closed native preparation cannot consume the slot"
            );
        }
    }
}

#[tokio::test]
async fn completed_registration_keeps_its_act_and_refreshes_own_read_and_actual_native_associations()
 {
    for mode in [
        "live",
        "read_lost",
        "foreign_read_lost",
        "write_lost",
        "expired",
        "home_changed",
    ] {
        let world = world().await;
        let (c, laptop) = setup(&world).await;
        let another = make_project(&world, &c.admin, c.acme, "Later home")
            .await
            .base
            .id;
        let (key_id, token) = key(&world, c.admin.id(), full(laptop, c.acme)).await;
        let control = success(
            &call(
                &world,
                &token,
                laptop,
                &body(c.acme, "Full native control", "/srv/ada/control"),
            )
            .await,
        );
        assert_eq!(control["machine"]["id"], json!(laptop));
        assert_eq!(control["name"], "Full native control");
        let before = counts(&world).await;
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
        let secret = token.clone();
        let wanted = body(
            c.acme,
            "Committed registration marker",
            "/srv/ada/committed-marker",
        );
        let request = tokio::spawn(async move {
            let call = app.call(MachineEnvironmentCreate, &[&laptop]);
            call.token(&secret).body(&wanted).send_bytes().await
        });
        wait_sql(&world, pid, "%INSERT INTO api_token_activity%", &request).await;
        let (id, revision): (Id, i64) = sqlx::query_as(
            "SELECT id,revision FROM environments WHERE name='Committed registration marker'",
        )
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
        let after = counts(&world).await;
        assert_eq!(
            after.0,
            before.0 + 1,
            "the observed publication wait follows real commit"
        );
        assert_eq!(after.2, before.2 + 1);
        if mode == "expired" {
            world.advance(chrono::Duration::hours(2));
        }
        if mode == "home_changed" {
            sqlx::query("UPDATE environments SET home_node_id=$2 WHERE id=$1")
                .bind(id)
                .bind(another)
                .execute(&world.db.pool)
                .await
                .unwrap();
        }
        if matches!(mode, "read_lost" | "foreign_read_lost" | "write_lost") {
            let rules = match mode {
                "read_lost" => {
                    let mut rules = act(laptop, c.acme);
                    rules.push(rule(
                        "EnvironmentGet.read",
                        "org",
                        c.acme,
                        "subtree",
                        &["environment"],
                        &["read"],
                    ));
                    rules
                }
                "foreign_read_lost" => {
                    let mut rules = full(laptop, c.acme);
                    rules[0] = act(laptop, c.acme).remove(0);
                    rules
                }
                _ => full(laptop, c.acme)
                    .into_iter()
                    .map(|mut row| {
                        row["allow"][0]["actions"] = json!(["read"]);
                        row
                    })
                    .collect(),
            };
            let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
            assert!(scope.validate().is_empty());
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                .bind(key_id)
                .bind(json!(scope))
                .execute(&world.db.pool)
                .await
                .unwrap();
        }
        let xmin: String = sqlx::query_scalar("SELECT xmin::text FROM environments WHERE id=$1")
            .bind(id)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(WAIT, request).await.unwrap().unwrap();
        if matches!(mode, "live" | "write_lost") {
            let native = success(&reply);
            assert_eq!(native["id"], json!(id));
            assert_eq!(native["workspace_root"], "/srv/ada/committed-marker");
            assert_eq!(native["machine"]["name"], "laptop");
        } else {
            assert_eq!(
                success(&reply),
                json!({"projection":"receipt","environment_id":id,"revision":revision})
            );
        }
        let current: String = sqlx::query_scalar("SELECT xmin::text FROM environments WHERE id=$1")
            .bind(id)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
        assert_eq!(
            current, xmin,
            "completed publication must not replay registration"
        );
        assert_eq!(
            counts(&world).await,
            after,
            "no extra registration, page, audit or provisioning job"
        );
    }
}
