//! Actual native shell creation keeps its own scope through reservation, commands and first ticket.

mod places;
mod rig;
mod support;
mod terminals_support;
mod thread_history_scope;

use orch_api_contract::{
    environment::ThroughThreadQuery,
    terminals::{EnvironmentTerminalCreate, EnvironmentTerminalCreateBody},
};
use orch_api_core::testing::RawReply;
use orch_engine::commands::CommandAuthority;
use orch_link_contract::commands::CommandKind;
use orch_types::Id;
use serde_json::{Value, json};
use terminals_support::{Floor, Heard, PATIENCE};
use thread_history_scope::{Proof, key, narrow, rule};

const OP: &str = "EnvironmentTerminalCreate";
fn rules(env: Id) -> Vec<Value> {
    vec![
        rule(OP, "environment", env, "self", "read"),
        rule(OP, "environment", env, "self", "manage"),
    ]
}
fn body(close: Vec<Id>) -> EnvironmentTerminalCreateBody {
    EnvironmentTerminalCreateBody {
        cols: 100,
        rows: 30,
        cwd: None,
        close: Some(close),
    }
}
async fn create(floor: &Floor, secret: &str, body: &EnvironmentTerminalCreateBody) -> RawReply {
    let request = floor
        .scene
        .rig
        .world
        .app
        .call(EnvironmentTerminalCreate, &[&floor.environment])
        .token(secret)
        .query(&ThroughThreadQuery {
            thread: Some(floor.thread),
        })
        .body(body);
    request.send_bytes().await
}
fn full(reply: &RawReply) -> Proof<Value> {
    assert_eq!(
        reply.status.as_u16(),
        201,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    let value: Value = serde_json::from_slice(&reply.raw)?;
    assert!(value.get("projection").is_none());
    Ok(value)
}
fn receipt(reply: &RawReply, env: Id) -> Proof<Value> {
    assert_eq!(
        reply.status.as_u16(),
        201,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    let value: Value = serde_json::from_slice(&reply.raw)?;
    assert_eq!(value["projection"], "receipt");
    assert_eq!(value["environment_id"], json!(env));
    for field in ["session", "cwd", "shell", "ticket", "ticket_expires_at"] {
        assert!(value.get(field).is_none());
    }
    Ok(value)
}

#[tokio::test(flavor = "multi_thread")]
async fn creation_requires_its_exact_environment_read_manage_and_returns_a_usable_origin_bound_ticket()
-> Proof {
    let floor = Floor::new().await;
    for grants in [
        vec![rule(OP, "environment", floor.environment, "self", "read")],
        vec![rule(OP, "environment", floor.environment, "self", "manage")],
        rules(orch_types::new_id()),
        vec![
            rule(
                "TerminalTicketCreate",
                "environment",
                floor.environment,
                "self",
                "read",
            ),
            rule(
                "TerminalTicketCreate",
                "environment",
                floor.environment,
                "self",
                "manage",
            ),
        ],
    ] {
        let selected = key(&floor.scene.rig.world, floor.scene.dan.id(), grants).await?;
        assert!(
            !create(&floor, &selected.secret, &body(Vec::new()))
                .await
                .status
                .is_success()
        );
    }
    let no_native = key(
        &floor.scene.rig.world,
        floor.scene.cai.id(),
        rules(floor.environment),
    )
    .await?;
    assert!(
        !create(&floor, &no_native.secret, &body(Vec::new()))
            .await
            .status
            .is_success()
    );
    assert_eq!(floor.scene.rig.commands.issued_of("pty.open"), 0);
    let selected = key(
        &floor.scene.rig.world,
        floor.scene.dan.id(),
        rules(floor.environment),
    )
    .await?;
    let made = full(&create(&floor, &selected.secret, &body(Vec::new())).await)?;
    let id: Id = serde_json::from_value(made["session"]["id"].clone())?;
    assert_eq!(made["session"]["environment_id"], json!(floor.environment));
    assert_eq!(made["session"]["thread_id"], json!(floor.thread));
    let command = orch_engine::ids::derived("pty.open", &[id.as_bytes().as_slice()]);
    let authority = floor
        .scene
        .rig
        .commands
        .issued_authority(command)
        .ok_or("actual native open omitted its creator")?;
    assert!(matches!(&authority, CommandAuthority::TerminalCreation(_)));
    let private = serde_json::to_value(authority)?;
    assert_eq!(private["authority"]["creator"]["origin"]["operation"], OP);
    assert!(serde_json::to_string(&private)?.contains(&selected.id.to_string()));
    let mut screen = floor
        .key_screen(
            id,
            made["ticket"]
                .as_str()
                .ok_or("actual creation ticket is absent")?,
        )
        .await;
    screen.ready().await;
    screen.type_("echo scoped-created-shell\n").await;
    screen.until("scoped-created-shell\r\n").await;
    screen.drop_it().await;
    assert_eq!(floor.scene.rig.commands.issued_of("pty.open"), 1);
    floor.stop().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn creation_preserves_native_limits_and_owns_every_close_governor_in_the_same_environment()
-> Proof {
    let floor = Floor::new().await;
    let old = floor.shell_of(&floor.scene.dan).await.session.id;
    floor
        .scene
        .rig
        .world
        .set("terminal.max_per_person", json!(1))
        .await;
    let selected = key(
        &floor.scene.rig.world,
        floor.scene.dan.id(),
        rules(floor.environment),
    )
    .await?;
    let before = floor.scene.rig.commands.issued_of("pty.status");
    let limited = create(&floor, &selected.secret, &body(Vec::new())).await;
    assert_eq!(limited.status.as_u16(), 409);
    let problem: Value = serde_json::from_slice(&limited.raw)?;
    assert_eq!(problem["mine"][0]["id"], json!(old));
    assert_eq!(
        floor.scene.rig.commands.issued_of("pty.status"),
        before + 1,
        "native limit supplier runs once"
    );
    let other = floor.shell_of(&floor.scene.eve).await.session.id;
    assert!(
        !create(&floor, &selected.secret, &body(vec![other]))
            .await
            .status
            .is_success()
    );
    assert!(floor.row(other).await.is_live());
    let made = full(&create(&floor, &selected.secret, &body(vec![old])).await)?;
    assert_eq!(made["closed"], json!([old]));
    assert!(!floor.row(old).await.is_live());
    let close_id = orch_engine::ids::derived("pty.close", &[old.as_bytes().as_slice()]);
    let private = serde_json::to_value(
        floor
            .scene
            .rig
            .commands
            .issued_authority(close_id)
            .ok_or("owned given-close omitted its creator")?,
    )?;
    assert_eq!(private["authority"]["creator"]["origin"]["operation"], OP);
    assert_eq!(
        private["authority"]["creator"]["environment"],
        json!(floor.environment)
    );
    floor.stop().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn creation_rechecks_its_key_after_the_actual_environment_slot_lock_before_reserving_anything()
-> Proof {
    for expire in [false, true] {
        let floor = Floor::new().await;
        let selected = key(
            &floor.scene.rig.world,
            floor.scene.dan.id(),
            rules(floor.environment),
        )
        .await?;
        let mut blocker = floor.pool().begin().await?;
        orch_db::queries::terminals::lock(&mut blocker, floor.environment).await?;
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await?;
        let app = floor.scene.rig.world.app.clone();
        let env = floor.environment;
        let thread = floor.thread;
        let pending = tokio::spawn(async move {
            let request = app
                .call(EnvironmentTerminalCreate, &[&env])
                .token(&selected.secret)
                .query(&ThroughThreadQuery {
                    thread: Some(thread),
                })
                .body(&body(Vec::new()));
            request.send_bytes().await
        });
        tokio::time::timeout(std::time::Duration::from_secs(30),async{loop{
            assert!(!pending.is_finished(),"the native environment slot lock must be reached");
            let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query ILIKE '%pg_advisory_xact_lock%terminals:%')").bind(pid).fetch_one(floor.pool()).await?;
            if waiting{break;}tokio::task::yield_now().await;
        }Ok::<(),Box<dyn std::error::Error+Send+Sync>>(())}).await??;
        if expire {
            floor.scene.rig.world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await?;
        let actual = tokio::time::timeout(std::time::Duration::from_secs(30), pending).await??;
        assert_eq!(actual.status.as_u16(), if expire { 401 } else { 201 });
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM terminal_sessions WHERE environment_id=$1")
                .bind(env)
                .fetch_one(floor.pool())
                .await?;
        assert_eq!(count, i64::from(!expire));
        assert_eq!(
            floor.scene.rig.commands.issued_of("pty.open"),
            usize::from(!expire)
        );
        floor.stop().await;
    }
    Ok(())
}

struct Release(std::sync::Arc<orch_testkit::doubles::machine::ScriptedHandler>);
impl Drop for Release {
    fn drop(&mut self) {
        self.0.release();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn accepted_native_open_keeps_its_resource_receipt_and_never_mints_control_after_creator_loss()
-> Proof {
    for mode in [
        "unchanged",
        "read_removed",
        "manage_removed",
        "revoke",
        "expire",
    ] {
        let floor = Floor::new().await;
        let selected = key(
            &floor.scene.rig.world,
            floor.scene.dan.id(),
            rules(floor.environment),
        )
        .await?;
        let handler = floor.scene.rig.machine.handler.clone();
        let release = Release(handler.clone());
        handler.hold(CommandKind::PtyOpen);
        let change = async {
            rig::eventually(
                "the actual accepted native open is waiting for its result",
                || async {
                    handler
                        .executions()
                        .iter()
                        .any(|execution| execution.kind == CommandKind::PtyOpen)
                },
            )
            .await;
            match mode {
                "read_removed" => {
                    narrow(
                        &floor.scene.rig.world,
                        &selected,
                        vec![rule(OP, "environment", floor.environment, "self", "manage")],
                    )
                    .await?
                }
                "manage_removed" => {
                    narrow(
                        &floor.scene.rig.world,
                        &selected,
                        vec![rule(OP, "environment", floor.environment, "self", "read")],
                    )
                    .await?
                }
                "revoke" => {
                    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
                        .bind(selected.id)
                        .bind(floor.scene.rig.world.now())
                        .execute(floor.pool())
                        .await?;
                }
                "expire" => floor.scene.rig.world.advance(chrono::Duration::hours(2)),
                _ => {}
            }
            handler.release();
            Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
        };
        let request_body = body(Vec::new());
        let (actual, changed) = tokio::time::timeout(std::time::Duration::from_secs(30), async {
            tokio::join!(create(&floor, &selected.secret, &request_body), change)
        })
        .await?;
        changed?;
        drop(release);
        let value = if mode == "unchanged" {
            full(&actual)?
        } else {
            receipt(&actual, floor.environment)?
        };
        let id: Id = serde_json::from_value(if mode == "unchanged" {
            value["session"]["id"].clone()
        } else {
            value["terminal_id"].clone()
        })?;
        assert!(floor.row(id).await.is_live());
        assert_eq!(floor.scene.rig.commands.issued_of("pty.open"), 1);
        assert_eq!(floor.details("terminal.opened").await.len(), 1);
        if mode != "unchanged" {
            assert_eq!(
                floor.scene.rig.commands.issued_of("pty.status"),
                0,
                "withdrawn authority precedes response IO/ticket issue"
            );
        }
        floor.stop().await;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn creation_keeps_the_original_key_after_an_accepted_given_close_before_a_new_process_can_start()
-> Proof {
    let floor = Floor::new().await;
    let old = floor.shell_of(&floor.scene.dan).await.session.id;
    floor
        .scene
        .rig
        .world
        .set("terminal.max_per_person", json!(1))
        .await;
    let selected = key(
        &floor.scene.rig.world,
        floor.scene.dan.id(),
        rules(floor.environment),
    )
    .await?;
    let before = floor.scene.rig.commands.issued_of("pty.open");
    let handler = floor.scene.rig.machine.handler.clone();
    let release = Release(handler.clone());
    handler.hold(CommandKind::PtyClose);
    let change = async {
        rig::eventually(
            "the owned given-close is actually accepted and waiting",
            || async {
                handler
                    .executions()
                    .iter()
                    .any(|execution| execution.kind == CommandKind::PtyClose)
            },
        )
        .await;
        sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
            .bind(selected.id)
            .bind(floor.scene.rig.world.now())
            .execute(floor.pool())
            .await?;
        handler.release();
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    };
    let request_body = body(vec![old]);
    let (actual, changed) = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        tokio::join!(create(&floor, &selected.secret, &request_body), change)
    })
    .await?;
    changed?;
    drop(release);
    assert!(!actual.status.is_success());
    assert_eq!(floor.scene.rig.commands.issued_of("pty.open"), before);
    assert!(!floor.row(old).await.is_live());
    let opening: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM terminal_sessions WHERE environment_id=$1 AND state='opening'",
    )
    .bind(floor.environment)
    .fetch_one(floor.pool())
    .await?;
    assert_eq!(
        opening, 0,
        "failed remaining work cleans its own accepted reservation"
    );
    floor.stop().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn created_ticket_redemption_retains_the_actual_creator_operation_key_surface_and_shell_association()
-> Proof {
    let floor = Floor::new().await;
    let selected = key(
        &floor.scene.rig.world,
        floor.scene.dan.id(),
        rules(floor.environment),
    )
    .await?;
    let made = full(&create(&floor, &selected.secret, &body(Vec::new())).await)?;
    let id: Id = serde_json::from_value(made["session"]["id"].clone())?;
    narrow(&floor.scene.rig.world, &selected, Vec::new()).await?;
    let mut screen = floor
        .key_screen(
            id,
            made["ticket"]
                .as_str()
                .ok_or("actual creation ticket absent")?,
        )
        .await;
    let heard = screen.hear_within(PATIENCE).await;
    screen.drop_it().await;
    assert!(
        matches!(heard,Some(Heard::Text(ref frame))if frame["op"]=="closed"&&frame["reason"]=="revoked"),
        "no ready/replay after original creator loss: {heard:?}"
    );
    assert!(
        floor.row(id).await.is_live(),
        "key loss ends the subscription, not the accepted owned shell"
    );
    floor.stop().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn created_full_result_never_replays_status_or_lends_a_changed_source_after_the_actual_activity_wait()
-> Proof {
    for mode in ["unchanged", "scope", "association"] {
        let floor = Floor::new().await;
        let selected = key(
            &floor.scene.rig.world,
            floor.scene.dan.id(),
            rules(floor.environment),
        )
        .await?;
        let mut blocker = floor.pool().begin().await?;
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *blocker)
            .await?;
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await?;
        let app = floor.scene.rig.world.app.clone();
        let env = floor.environment;
        let thread = floor.thread;
        let secret = selected.secret.clone();
        let pending = tokio::spawn(async move {
            let request = app
                .call(EnvironmentTerminalCreate, &[&env])
                .token(&secret)
                .query(&ThroughThreadQuery {
                    thread: Some(thread),
                })
                .body(&body(Vec::new()));
            request.send_bytes().await
        });
        tokio::time::timeout(std::time::Duration::from_secs(30),async{loop{
            assert!(!pending.is_finished(),"the actual creation activity wait must be reached");
            let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query ILIKE '%INSERT INTO api_token_activity%')").bind(pid).fetch_one(floor.pool()).await?;
            if waiting{break;}tokio::task::yield_now().await;
        }Ok::<(),Box<dyn std::error::Error+Send+Sync>>(())}).await??;
        let (id, version): (Id, String) =
            sqlx::query_as("SELECT id,xmin::text FROM terminal_sessions WHERE environment_id=$1")
                .bind(env)
                .fetch_one(floor.pool())
                .await?;
        let status = floor.scene.rig.commands.issued_of("pty.status");
        assert_eq!(status, 1);
        match mode {
            "scope" => {
                narrow(
                    &floor.scene.rig.world,
                    &selected,
                    vec![rule(OP, "environment", env, "self", "read")],
                )
                .await?
            }
            "association" => {
                sqlx::query("UPDATE terminal_sessions SET thread_id=NULL WHERE id=$1")
                    .bind(id)
                    .execute(floor.pool())
                    .await?;
            }
            _ => {}
        }
        blocker.commit().await?;
        let actual = tokio::time::timeout(std::time::Duration::from_secs(30), pending).await??;
        if mode == "unchanged" {
            assert_eq!(full(&actual)?["session"]["id"], json!(id));
        } else {
            assert_eq!(receipt(&actual, env)?["terminal_id"], json!(id));
        }
        assert_eq!(
            floor.scene.rig.commands.issued_of("pty.status"),
            status,
            "publication does not replay the native status supplier"
        );
        assert_eq!(floor.scene.rig.commands.issued_of("pty.open"), 1);
        assert_eq!(floor.details("terminal.opened").await.len(), 1);
        if mode != "association" {
            let after: String =
                sqlx::query_scalar("SELECT xmin::text FROM terminal_sessions WHERE id=$1")
                    .bind(id)
                    .fetch_one(floor.pool())
                    .await?;
            assert_eq!(after, version);
        }
        floor.stop().await;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_shell_creation_key_replays_its_receipt_without_another_process_or_control_ticket()
-> Proof {
    let floor = Floor::new().await;
    let actor = floor.scene.dan.id();
    let writer = key(&floor.scene.rig.world, actor, rules(floor.environment)).await?;
    let reader = key(
        &floor.scene.rig.world,
        actor,
        vec![rule(OP, "environment", floor.environment, "self", "read")],
    )
    .await?;
    let header = orch_types::new_id().to_string();
    let input = body(Vec::new());
    let send = |secret: String| {
        let app = floor.scene.rig.world.app.clone();
        let header = header.clone();
        let input = input.clone();
        let environment = floor.environment;
        let thread = floor.thread;
        async move {
            let request = app
                .call(EnvironmentTerminalCreate, &[&environment])
                .token(&secret)
                .header("Idempotency-Key", &header)
                .query(&ThroughThreadQuery {
                    thread: Some(thread),
                })
                .body(&input);
            request.send_bytes().await
        }
    };
    let first = send(writer.secret).await;
    let first = full(&first)?;
    let terminal: Id = serde_json::from_value(first["session"]["id"].clone())?;
    assert!(first["ticket"].is_string());
    let opened = floor.fake.opened.lock().unwrap().len();
    let repeated = send(reader.secret).await;
    let repeated = receipt(&repeated, floor.environment)?;
    assert_eq!(repeated["terminal_id"], json!(terminal));
    assert_eq!(repeated["closed"], json!([]));
    assert_eq!(floor.fake.opened.lock().unwrap().len(), opened);
    assert_eq!(opened, 1);
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM terminal_sessions WHERE opened_by=$1")
            .bind(actor)
            .fetch_one(floor.pool())
            .await?;
    assert_eq!(count, 1);
    let record: Value =
        sqlx::query_scalar("SELECT response FROM idempotency_keys WHERE user_id=$1 AND key=$2")
            .bind(actor)
            .bind(header)
            .fetch_one(floor.pool())
            .await?;
    assert_eq!(record["outcome"]["id"], json!(terminal));
    assert!(
        !serde_json::to_string(&record)?
            .contains(first["ticket"].as_str().ok_or("first ticket missing")?)
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lost_shell_reservation_replays_its_real_identity_before_any_machine_open() -> Proof {
    let floor = Floor::new().await;
    let actor = floor.scene.dan.id();
    let writer = key(&floor.scene.rig.world, actor, rules(floor.environment)).await?;
    let reader = key(
        &floor.scene.rig.world,
        actor,
        vec![rule(OP, "environment", floor.environment, "self", "read")],
    )
    .await?;
    let header = orch_types::new_id().to_string();
    // This transport issues shell creation directly; no command INSERT precedes delivery.
    // Stop the actual next issue after the reservation commits, without blocking policy reads.
    let stopped = floor
        .scene
        .rig
        .commands
        .stop_next_issue(CommandKind::PtyOpen);
    let app = floor.scene.rig.world.app.clone();
    let environment = floor.environment;
    let thread = floor.thread;
    let retry_key = header.clone();
    let pending = tokio::spawn(async move {
        let request = app
            .call(EnvironmentTerminalCreate, &[&environment])
            .token(&writer.secret)
            .header("Idempotency-Key", &retry_key)
            .query(&ThroughThreadQuery {
                thread: Some(thread),
            })
            .body(&body(Vec::new()));
        request.send_bytes().await
    });
    let reached = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        let command = stopped.await?;
        assert!(!pending.is_finished(), "the actual native issue is stopped");
        let terminal: Id = sqlx::query_scalar(
            "SELECT id FROM terminal_sessions WHERE opened_by=$1 AND state='opening'",
        )
        .bind(actor)
        .fetch_one(floor.pool())
        .await?;
        assert_eq!(
            command,
            orch_engine::ids::derived("pty.open", &[terminal.as_bytes().as_slice()]),
            "the attempted command belongs to this committed reservation"
        );
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(terminal)
    })
    .await;
    pending.abort();
    assert!(pending.await.unwrap_err().is_cancelled());
    let terminal: Id =
        reached.map_err(|_| "the native reservation did not reach its stopped issue")??;
    assert!(floor.fake.opened.lock().unwrap().is_empty());
    assert_eq!(floor.scene.rig.commands.issued_of("pty.open"), 0);
    let outcome: Value =
        sqlx::query_scalar("SELECT response FROM idempotency_keys WHERE user_id=$1 AND key=$2")
            .bind(actor)
            .bind(&header)
            .fetch_one(floor.pool())
            .await?;
    assert_eq!(outcome["outcome"]["id"], json!(terminal));
    let input = body(Vec::new());
    let request = floor
        .scene
        .rig
        .world
        .app
        .call(EnvironmentTerminalCreate, &[&environment])
        .token(&reader.secret)
        .header("Idempotency-Key", &header)
        .query(&ThroughThreadQuery {
            thread: Some(thread),
        })
        .body(&input);
    let repeated = request.send_bytes().await;
    assert_eq!(
        receipt(&repeated, environment)?["terminal_id"],
        json!(terminal)
    );
    assert!(floor.fake.opened.lock().unwrap().is_empty());
    assert_eq!(floor.scene.rig.commands.issued_of("pty.open"), 0);
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM terminal_sessions WHERE opened_by=$1")
            .bind(actor)
            .fetch_one(floor.pool())
            .await?;
    assert_eq!(count, 1);
    Ok(())
}
