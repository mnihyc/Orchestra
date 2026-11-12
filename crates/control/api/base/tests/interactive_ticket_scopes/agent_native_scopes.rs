//! Real agent tickets use the stored Environment and retain that original association.

use super::*;
use orch_api_contract::terminals::{AgentTerminalTicketCreate, AgentTerminalTicketCreateBody};
use orch_api_core::testing::RawReply;
use serde_json::json;

async fn declare(floor: &Floor, capability: &str) -> Proof {
    let changed = sqlx::query("UPDATE agents SET descriptor=jsonb_set(descriptor,'{capabilities,agent_terminal}',to_jsonb($2::text)) WHERE id=$1")
        .bind(floor.scene.on.agent).bind(capability).execute(floor.pool()).await?;
    assert_eq!(changed.rows_affected(), 1);
    Ok(())
}

async fn request(floor: &Floor, secret: &str, control: bool) -> RawReply {
    floor
        .scene
        .rig
        .world
        .app
        .call(AgentTerminalTicketCreate, &[&floor.thread])
        .token(secret)
        .body(&AgentTerminalTicketCreateBody {
            control: Some(control),
        })
        .send_bytes()
        .await
}

#[tokio::test(flavor = "multi_thread")]
async fn agent_ticket_binds_the_stored_environment_with_its_exact_view_or_control_actions() -> Proof
{
    let floor = Floor::new().await;
    declare(&floor, "interactive").await?;
    for control in [false, true] {
        let who = if control {
            &floor.scene.dan
        } else {
            &floor.scene.cai
        };
        let selection = if control {
            "AgentTerminalTicketCreate.control"
        } else {
            "AgentTerminalTicketCreate.view"
        };
        let selected = key(
            &floor.scene.rig.world,
            who.id(),
            rules(selection, floor.environment, control),
        )
        .await?;
        ticket(
            &request(&floor, &who.token, control).await,
            "ordinary agent ticket premise",
        )?;
        let minted = ticket(
            &request(&floor, &selected.secret, control).await,
            "exact modern agent ticket",
        )?;
        let mut screen = floor.key_agent_screen(&minted).await;
        screen.ready().await;
        if control {
            screen.type_("echo scoped-agent-control\n").await;
            screen.until("scoped-agent-control\r\n").await;
        }
        screen.drop_it().await;
        for grants in [
            rules(selection, orch_types::new_id(), control),
            vec![rule(selection, "thread", floor.thread, "self", "read")],
            rules("TerminalTicketCreate", floor.environment, true),
            vec![rule(
                selection,
                "environment",
                floor.environment,
                "self",
                if control { "read" } else { "manage" },
            )],
        ] {
            let denied = key(&floor.scene.rig.world, who.id(), grants).await?;
            let answer = request(&floor, &denied.secret, control).await;
            assert!(!answer.status.is_success());
            assert!(
                serde_json::from_slice::<Json>(&answer.raw)?
                    .get("ticket")
                    .is_none()
            );
        }
    }
    // Matching scope cannot turn a native watcher into a controller.
    let watcher = key(
        &floor.scene.rig.world,
        floor.scene.cai.id(),
        rules("AgentTerminalTicketCreate.control", floor.environment, true),
    )
    .await?;
    assert!(
        !request(&floor, &watcher.secret, true)
            .await
            .status
            .is_success()
    );
    // Native capability negotiation is still authoritative.
    declare(&floor, "view").await?;
    let controller = key(
        &floor.scene.rig.world,
        floor.scene.dan.id(),
        rules("AgentTerminalTicketCreate.control", floor.environment, true),
    )
    .await?;
    assert_eq!(
        request(&floor, &controller.secret, true)
            .await
            .status
            .as_u16(),
        409
    );
    floor.stop().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn agent_ticket_redeem_keeps_the_original_key_and_environment_association() -> Proof {
    for replace_environment in [false, true] {
        let floor = Floor::new().await;
        declare(&floor, "interactive").await?;
        let other = floor
            .scene
            .rig
            .resources(&floor.scene.bea, &floor.scene.tree)
            .await
            .environment;
        let mut grants = rules("AgentTerminalTicketCreate.view", floor.environment, false);
        grants.extend(rules("AgentTerminalTicketCreate.view", other, false));
        let selected = key(&floor.scene.rig.world, floor.scene.cai.id(), grants).await?;
        let held = ticket(
            &request(&floor, &selected.secret, false).await,
            "retained modern agent ticket",
        )?;
        if replace_environment {
            let changed = sqlx::query("UPDATE threads SET environment_id=$2 WHERE node_id=$1")
                .bind(floor.thread)
                .bind(other)
                .execute(floor.pool())
                .await?;
            assert_eq!(changed.rows_affected(), 1);
        } else {
            let changed = sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
                .bind(selected.id)
                .bind(floor.scene.rig.world.now())
                .execute(floor.pool())
                .await?;
            assert_eq!(changed.rows_affected(), 1);
        }
        let mut screen = floor.key_agent_screen(&held).await;
        let actual = screen.hear_within(PATIENCE).await;
        screen.drop_it().await;
        assert!(
            matches!(actual, Some(Heard::Text(ref frame)) if frame["op"]=="closed" && frame["reason"]=="revoked"),
            "no ready or replay from a substituted source: {actual:?}"
        );
        assert!(
            floor
                .fake
                .agent_opened
                .lock()
                .map_err(|_| "agent-open observer is poisoned")?
                .is_empty()
        );
        floor.stop().await;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn agent_control_stream_keeps_native_view_when_the_same_keys_manage_is_withdrawn() -> Proof {
    let floor = Floor::new().await;
    declare(&floor, "interactive").await?;
    let selected = key(
        &floor.scene.rig.world,
        floor.scene.dan.id(),
        rules("AgentTerminalTicketCreate.control", floor.environment, true),
    )
    .await?;
    let held = ticket(
        &request(&floor, &selected.secret, true).await,
        "modern controller ticket",
    )?;
    let mut screen = floor.key_agent_screen(&held).await;
    screen.ready().await;
    screen.type_("echo before-narrowing\n").await;
    screen.until("before-narrowing\r\n").await;
    session_ticket_support::current_scope(
        &floor.scene.rig.world,
        selected.id,
        vec![rule(
            "AgentTerminalTicketCreate.control",
            "environment",
            floor.environment,
            "self",
            "read",
        )],
    )
    .await?;
    let shell = floor
        .fake
        .shell(floor.thread)
        .ok_or("actual agent terminal is absent")?;
    let before = shell.typed_text();
    screen.type_("echo must-not-type\n").await;
    screen.send(json!({"op":"ping","n":91})).await;
    loop {
        if screen.frame().await["op"] == "pong" {
            break;
        }
    }
    assert_eq!(shell.typed_text(), before);
    assert!(
        shell.print_attached("retained native view\r\n"),
        "the native attachment stays open for process output"
    );
    screen.until("retained native view\r\n").await;
    screen.drop_it().await;
    floor.stop().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn agent_ticket_refreshes_its_exact_key_after_the_actual_thread_row_wait() -> Proof {
    for expire in [false, true] {
        let floor = Floor::new().await;
        declare(&floor, "interactive").await?;
        let selected = key(
            &floor.scene.rig.world,
            floor.scene.dan.id(),
            rules("AgentTerminalTicketCreate.control", floor.environment, true),
        )
        .await?;
        let mut blocker = floor.pool().begin().await?;
        sqlx::query("SELECT node_id FROM threads WHERE node_id=$1 FOR UPDATE")
            .bind(floor.thread)
            .execute(&mut *blocker)
            .await?;
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await?;
        let app = floor.scene.rig.world.app.clone();
        let thread = floor.thread;
        let pending = tokio::spawn(async move {
            let request = app
                .call(AgentTerminalTicketCreate, &[&thread])
                .token(&selected.secret)
                .body(&AgentTerminalTicketCreateBody {
                    control: Some(true),
                });
            request.send_bytes().await
        });
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                assert!(!pending.is_finished(), "the native thread lock must be reached");
                let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query ILIKE '%FOR UPDATE OF t%')")
                    .bind(pid).fetch_one(floor.pool()).await?;
                if waiting { break; }
                tokio::task::yield_now().await;
            }
            Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
        }).await??;
        if expire {
            floor.scene.rig.world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await?;
        let answer = tokio::time::timeout(std::time::Duration::from_secs(30), pending).await??;
        assert_eq!(answer.status.as_u16(), if expire { 401 } else { 200 });
        if expire {
            assert!(
                serde_json::from_slice::<Json>(&answer.raw)?
                    .get("ticket")
                    .is_none()
            );
        }
        floor.stop().await;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn agent_ticket_headers_keep_both_native_modes_without_minting_or_extending_their_first_capability()
-> Proof {
    for control in [false, true] {
        let floor = Floor::new().await;
        declare(&floor, "interactive").await?;
        let who = if control {
            &floor.scene.dan
        } else {
            &floor.scene.cai
        };
        let operation = if control {
            "AgentTerminalTicketCreate.control"
        } else {
            "AgentTerminalTicketCreate.view"
        };
        let writer = key(
            &floor.scene.rig.world,
            who.id(),
            rules(operation, floor.environment, control),
        )
        .await?;
        let act = orch_types::new_id();
        let input = AgentTerminalTicketCreateBody {
            control: Some(control),
        };
        let call = floor
            .scene
            .rig
            .world
            .app
            .call(AgentTerminalTicketCreate, &[&floor.thread])
            .token(&writer.secret)
            .header("Idempotency-Key", &act.to_string())
            .body(&input);
        let first = call.send_bytes().await;
        let secret = ticket(&first, "one actual native agent ticket")?;
        let value: Json = serde_json::from_slice(&first.raw)?;
        let before: (Json, String) = sqlx::query_as(
            "SELECT response,xmin::text FROM idempotency_keys WHERE user_id=$1 AND key=$2",
        )
        .bind(who.id())
        .bind(act.to_string())
        .fetch_one(floor.pool())
        .await?;
        let id: orch_types::Id = serde_json::from_value(before.0["outcome"]["id"].clone())?;
        assert_eq!(before.0["outcome"]["kind"], "agent_terminal_ticket");
        assert_eq!(
            before.0["outcome"]["data"],
            json!({"ticket_expires_at":value["ticket_expires_at"]})
        );
        assert!(!before.0.to_string().contains(&secret));
        sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
            .bind(writer.id)
            .bind(floor.scene.rig.world.now())
            .execute(floor.pool())
            .await?;
        floor.scene.rig.world.advance(chrono::Duration::minutes(2));
        let reader = key(
            &floor.scene.rig.world,
            who.id(),
            vec![rule(
                "EnvironmentTerminalList",
                "environment",
                floor.environment,
                "self",
                "read",
            )],
        )
        .await?;
        let call = floor
            .scene
            .rig
            .world
            .app
            .call(AgentTerminalTicketCreate, &[&floor.thread])
            .token(&reader.secret)
            .header("Idempotency-Key", &act.to_string())
            .body(&input);
        let replay = call.send_bytes().await;
        assert_eq!(replay.status.as_u16(), 200);
        assert_eq!(
            serde_json::from_slice::<Json>(&replay.raw)?,
            json!({
                "projection":"receipt","ticket_id":id,"ticket_expires_at":value["ticket_expires_at"]
            })
        );
        let call = floor
            .scene
            .rig
            .world
            .app
            .call(AgentTerminalTicketCreate, &[&floor.thread])
            .token(&reader.secret)
            .header("Idempotency-Key", &orch_types::new_id().to_string())
            .body(&input);
        assert_eq!(call.send_bytes().await.status.as_u16(), 403);
        let after: (Json, String) = sqlx::query_as(
            "SELECT response,xmin::text FROM idempotency_keys WHERE user_id=$1 AND key=$2",
        )
        .bind(who.id())
        .bind(act.to_string())
        .fetch_one(floor.pool())
        .await?;
        assert_eq!(after, before);
        floor.stop().await;
    }
    Ok(())
}
