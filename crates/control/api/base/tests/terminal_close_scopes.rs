//! Actual shell closes admit their stored governor and keep accepted cleanup separate from readback.

mod places;
mod rig;
mod support;
mod terminals_support;
mod thread_history_scope;

use orch_api_contract::terminals::TerminalClose;
use orch_api_core::testing::RawReply;
use orch_engine::commands::CommandAuthority;
use orch_link_contract::commands::CommandKind;
use orch_types::Id;
use serde_json::{Value, json};
use terminals_support::Floor;
use thread_history_scope::{Proof, key, narrow, rule};

const OP: &str = "TerminalClose";

fn grants(environment: Id, read: bool) -> Vec<Value> {
    let mut rules = vec![rule(OP, "environment", environment, "self", "manage")];
    if read {
        rules.push(rule(OP, "environment", environment, "self", "read"));
    }
    rules
}

async fn close(floor: &Floor, id: Id, secret: &str) -> RawReply {
    floor
        .scene
        .rig
        .world
        .app
        .call(TerminalClose, &[&id])
        .token(secret)
        .send_bytes()
        .await
}

fn receipt(reply: &RawReply, id: Id, environment: Id) -> Proof {
    assert_eq!(
        reply.status.as_u16(),
        200,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    let value: Value = serde_json::from_slice(&reply.raw)?;
    assert_eq!(
        value,
        json!({"projection":"receipt","terminal_id":id,"environment_id":environment,"closed":true})
    );
    Ok(())
}

fn native_id(id: Id) -> Id {
    orch_engine::ids::derived("pty.close", &[id.as_bytes().as_slice()])
}

#[tokio::test(flavor = "multi_thread")]
async fn close_manage_only_has_a_truthful_receipt_and_only_its_own_read_returns_native_metadata()
-> Proof {
    for disclose in [false, true] {
        let floor = Floor::new().await;
        let id = floor.shell_of(&floor.scene.dan).await.session.id;
        let mut rules = grants(floor.environment, disclose);
        rules.push(rule(
            "TerminalRecordingGet",
            "environment",
            floor.environment,
            "self",
            "read",
        ));
        let selected = key(&floor.scene.rig.world, floor.scene.dan.id(), rules).await?;
        let answer = close(&floor, id, &selected.secret).await;
        if disclose {
            assert_eq!(answer.status.as_u16(), 200);
            let native = floor.close(&floor.scene.dan, id).await.ok();
            assert_eq!(
                serde_json::from_slice::<Value>(&answer.raw)?,
                serde_json::to_value(native)?
            );
        } else {
            receipt(&answer, id, floor.environment)?;
        }
        assert!(!floor.row(id).await.is_live());
        let before = floor.row(id).await;
        let replay = close(&floor, id, &selected.secret).await;
        if disclose {
            assert_eq!(answer.raw, replay.raw);
        } else {
            receipt(&replay, id, floor.environment)?;
        }
        assert_eq!(before, floor.row(id).await);
        assert_eq!(floor.scene.rig.commands.issued_of("pty.close"), 1);
        assert_eq!(
            floor
                .fake
                .closed
                .lock()
                .map_err(|_| "native close observer is poisoned")?
                .as_slice(),
            &[id]
        );
        assert_eq!(floor.details("terminal.closed").await.len(), 1);
        let authority = floor
            .scene
            .rig
            .commands
            .issued_authority(native_id(id))
            .ok_or("the HTTP close lost its actual private origin")?;
        assert!(matches!(&authority, CommandAuthority::TerminalClose(_)));
        let private = serde_json::to_value(authority)?;
        assert_eq!(private["authority"]["origin"]["operation"], OP);
        assert_eq!(
            private["authority"]["environment_id"],
            json!(floor.environment)
        );
        assert!(serde_json::to_string(&private)?.contains(&selected.id.to_string()));
        assert!(!serde_json::to_string(&private)?.contains(&selected.secret));
        floor.stop().await;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn close_needs_its_exact_environment_manage_and_the_original_opener_or_environment_manager()
-> Proof {
    let floor = Floor::new().await;
    let id = floor.shell_of(&floor.scene.dan).await.session.id;
    for (user, rules) in [
        (floor.scene.dan.id(), grants(orch_types::new_id(), false)),
        (
            floor.scene.dan.id(),
            vec![rule(OP, "environment", floor.environment, "self", "read")],
        ),
        (
            floor.scene.dan.id(),
            vec![rule(
                "TerminalTicketCreate",
                "environment",
                floor.environment,
                "self",
                "manage",
            )],
        ),
        (
            floor.scene.dan.id(),
            vec![rule(OP, "thread", floor.thread, "self", "manage")],
        ),
        (floor.scene.cai.id(), grants(floor.environment, false)),
    ] {
        let selected = key(&floor.scene.rig.world, user, rules).await?;
        assert!(
            !close(&floor, id, &selected.secret)
                .await
                .status
                .is_success()
        );
        assert!(floor.row(id).await.is_live());
    }
    assert_eq!(floor.scene.rig.commands.issued_of("pty.close"), 0);
    assert!(floor.details("terminal.closed").await.is_empty());
    let manager = key(
        &floor.scene.rig.world,
        floor.scene.eve.id(),
        grants(floor.environment, false),
    )
    .await?;
    receipt(
        &close(&floor, id, &manager.secret).await,
        id,
        floor.environment,
    )?;
    assert!(!floor.row(id).await.is_live());
    floor.stop().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn close_of_an_old_shell_ends_its_metadata_without_touching_a_new_environment_generation()
-> Proof {
    let floor = Floor::new().await;
    let id = floor.shell_of(&floor.scene.dan).await.session.id;
    sqlx::query("UPDATE environments SET generation=generation+1 WHERE id=$1")
        .bind(floor.environment)
        .execute(floor.pool())
        .await?;
    let selected = key(
        &floor.scene.rig.world,
        floor.scene.dan.id(),
        grants(floor.environment, false),
    )
    .await?;
    receipt(
        &close(&floor, id, &selected.secret).await,
        id,
        floor.environment,
    )?;
    assert!(!floor.row(id).await.is_live());
    assert_eq!(floor.scene.rig.commands.issued_of("pty.close"), 0);
    assert_eq!(floor.details("terminal.closed").await.len(), 1);
    floor.stop().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn ordinary_close_retains_its_actual_raw_session_in_the_native_private_origin() -> Proof {
    let floor = Floor::new().await;
    let id = floor.shell_of(&floor.scene.dan).await.session.id;
    assert_eq!(
        close(&floor, id, &floor.scene.dan.token)
            .await
            .status
            .as_u16(),
        200
    );
    let private = serde_json::to_value(
        floor
            .scene
            .rig
            .commands
            .issued_authority(native_id(id))
            .ok_or("the ordinary HTTP close omitted its native origin")?,
    )?;
    let session: Id = serde_json::from_value(private["authority"]["session"].clone())?;
    let actual: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sessions WHERE id=$1 AND user_id=$2)")
            .bind(session)
            .bind(floor.scene.dan.id())
            .fetch_one(floor.pool())
            .await?;
    assert!(actual);
    assert_eq!(private["authority"]["origin"]["operation"], OP);
    floor.stop().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn close_rechecks_scope_and_expiry_after_its_actual_native_session_lock() -> Proof {
    for mode in ["unchanged", "refused", "expire"] {
        let floor = Floor::new().await;
        let selected = key(
            &floor.scene.rig.world,
            floor.scene.dan.id(),
            grants(floor.environment, true),
        )
        .await?;
        let first = floor.shell_of(&floor.scene.dan).await.session.id;
        assert_eq!(
            close(&floor, first, &selected.secret).await.status.as_u16(),
            200
        );
        let id = floor.shell_of(&floor.scene.dan).await.session.id;
        let before = floor.scene.rig.commands.issued_of("pty.close");
        let mut blocker = floor.pool().begin().await?;
        orch_db::queries::terminals::lock(&mut blocker, id).await?;
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await?;
        let app = floor.scene.rig.world.app.clone();
        let secret = selected.secret.clone();
        let pending = tokio::spawn(async move {
            let request = app.call(TerminalClose, &[&id]).token(&secret);
            request.send_bytes().await
        });
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                assert!(!pending.is_finished(), "the native session lock must be reached");
                let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query ILIKE '%pg_advisory_xact_lock%terminals:%')").bind(pid).fetch_one(floor.pool()).await?;
                if waiting { break; }
                tokio::task::yield_now().await;
            }
            Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
        }).await??;
        if mode == "refused" {
            // A change of the key's own rules is ordered after the request that holds the key's row, so it cannot
            // land while the request waits at the session's lock; the org refusing the key can.
            floor
                .scene
                .rig
                .world
                .keep_key_out(
                    selected.id,
                    floor.scene.tree.org.base.id,
                    floor.scene.dan.id(),
                )
                .await;
        }
        if mode == "expire" {
            floor.scene.rig.world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await?;
        let actual = tokio::time::timeout(std::time::Duration::from_secs(30), pending).await??;
        assert_eq!(
            actual.status.as_u16(),
            match mode {
                "expire" => 401,
                "refused" => 403,
                _ => 200,
            },
            "{mode}: {}",
            String::from_utf8_lossy(&actual.raw)
        );
        assert_eq!(floor.row(id).await.is_live(), mode != "unchanged");
        assert_eq!(
            floor.scene.rig.commands.issued_of("pty.close"),
            before + usize::from(mode == "unchanged")
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
async fn accepted_native_close_finishes_once_and_keeps_a_receipt_after_read_key_or_deadline_loss()
-> Proof {
    for mode in ["read_removed", "manage_removed", "revoke", "expire"] {
        let floor = Floor::new().await;
        let selected = key(
            &floor.scene.rig.world,
            floor.scene.dan.id(),
            grants(floor.environment, true),
        )
        .await?;
        let first = floor.shell_of(&floor.scene.dan).await.session.id;
        assert_eq!(
            close(&floor, first, &selected.secret).await.status.as_u16(),
            200
        );
        let id = floor.shell_of(&floor.scene.dan).await.session.id;
        let handler = floor.scene.rig.machine.handler.clone();
        let release = Release(handler.clone());
        let before = handler.executions().len();
        handler.hold(CommandKind::PtyClose);
        let change = async {
            rig::eventually(
                "the actual accepted close is awaiting the machine answer",
                || async {
                    handler
                        .executions()
                        .iter()
                        .skip(before)
                        .any(|execution| execution.kind == CommandKind::PtyClose)
                },
            )
            .await;
            match mode {
                "read_removed" => {
                    narrow(
                        &floor.scene.rig.world,
                        &selected,
                        grants(floor.environment, false),
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
                _ => floor.scene.rig.world.advance(chrono::Duration::hours(2)),
            }
            handler.release();
            Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
        };
        let (actual, changed) = tokio::time::timeout(std::time::Duration::from_secs(30), async {
            tokio::join!(close(&floor, id, &selected.secret), change)
        })
        .await?;
        changed?;
        drop(release);
        if mode == "manage_removed" {
            assert_eq!(actual.status.as_u16(), 200);
            assert_eq!(
                serde_json::from_slice::<Value>(&actual.raw)?["state"],
                "closed"
            );
        } else {
            receipt(&actual, id, floor.environment)?;
        }
        assert!(!floor.row(id).await.is_live());
        assert_eq!(floor.scene.rig.commands.issued_of("pty.close"), 2);
        assert_eq!(floor.details("terminal.closed").await.len(), 2);
        floor.stop().await;
    }
    Ok(())
}
