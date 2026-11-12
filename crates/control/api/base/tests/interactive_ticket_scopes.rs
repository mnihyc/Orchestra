//! Actual ticket issuance uses each native door's own operation and current account bounds.

mod native_close_support;
mod places;
mod rig;
mod session_ticket_support;
mod support;
mod terminals_support;
mod thread_history_scope;

#[path = "interactive_ticket_scopes/agent_native_scopes.rs"]
mod agent_native_scopes;

use orch_api_contract::terminals::TerminalTicketCreate;
use orch_types::Json;
use session_ticket_support::{Proof, Surface, rules, selection, ticket};
use terminals_support::{Floor, Heard, PATIENCE, Screen};
use thread_history_scope::{key, rule};

async fn scoped(surface: Surface, control: bool) -> Proof {
    let scene = session_ticket_support::scene(surface).await?;
    let id = surface.make(&scene).await?;
    ticket(
        &session_ticket_support::request(&scene, surface, id, &scene.cai.token, control).await,
        "ordinary actual-session ticket premise",
    )?;
    let operation = selection(surface, control);
    let selected = key(
        &scene.rig.world,
        scene.cai.id(),
        rules(operation, scene.on.environment, control),
    )
    .await?;
    ticket(
        &session_ticket_support::request(&scene, surface, id, &selected.secret, control).await,
        "actual modern key ticket",
    )?;
    let other = scene
        .rig
        .resources(&scene.bea, &scene.tree)
        .await
        .environment;
    let thread = scene
        .thread_by(&scene.ada, "A Thread is not the stored session Environment")
        .await
        .base
        .id;
    let bad = [
        vec![rule(operation, "environment", other, "self", "read")],
        vec![rule(operation, "thread", thread, "self", "read")],
        vec![rule(
            surface.read_operation(),
            "environment",
            scene.on.environment,
            "self",
            "read",
        )],
        vec![rule(
            operation,
            "environment",
            scene.on.environment,
            "self",
            if control { "read" } else { "manage" },
        )],
    ];
    for rules in bad {
        let denied = key(&scene.rig.world, scene.cai.id(), rules).await?;
        let reply =
            session_ticket_support::request(&scene, surface, id, &denied.secret, control).await;
        assert!(
            !reply.status.is_success(),
            "wrong operation/action/actual Environment cannot mint a ticket"
        );
        assert!(
            serde_json::from_slice::<Json>(&reply.raw)?
                .get("ticket")
                .is_none()
        );
    }
    let stranger = key(
        &scene.rig.world,
        scene.bea.id(),
        rules(operation, scene.on.environment, control),
    )
    .await?;
    assert!(
        !session_ticket_support::request(&scene, surface, id, &stranger.secret, control)
            .await
            .status
            .is_success(),
        "matching key scope cannot supply the missing ordinary door"
    );
    Ok(())
}

#[tokio::test]
async fn browser_view_ticket_uses_own_read_on_actual_stored_environment() -> Proof {
    scoped(Surface::Browser, false).await
}
#[tokio::test]
async fn browser_control_ticket_requires_own_read_and_manage_on_actual_stored_environment() -> Proof
{
    scoped(Surface::Browser, true).await
}
#[tokio::test]
async fn display_view_ticket_uses_own_read_on_actual_stored_environment() -> Proof {
    scoped(Surface::Display, false).await
}
#[tokio::test]
async fn display_control_ticket_requires_own_read_and_manage_on_actual_stored_environment() -> Proof
{
    scoped(Surface::Display, true).await
}

#[tokio::test(flavor = "multi_thread")]
async fn terminal_ticket_requires_own_read_and_manage_and_ordinary_opener() -> Proof {
    let floor = Floor::new().await;
    let made = floor.shell_of(&floor.scene.dan).await;
    let id = made.session.id;
    let ordinary = floor
        .scene
        .rig
        .world
        .app
        .call(TerminalTicketCreate, &[&id])
        .token(&floor.scene.dan.token)
        .send_bytes()
        .await;
    ticket(&ordinary, "ordinary actual opener ticket premise")?;
    let selected = key(
        &floor.scene.rig.world,
        floor.scene.dan.id(),
        rules("TerminalTicketCreate", floor.environment, true),
    )
    .await?;
    let reply = floor
        .scene
        .rig
        .world
        .app
        .call(TerminalTicketCreate, &[&id])
        .token(&selected.secret)
        .send_bytes()
        .await;
    ticket(&reply, "actual modern opener key ticket")?;
    let other = floor
        .scene
        .rig
        .resources(&floor.scene.bea, &floor.scene.tree)
        .await
        .environment;
    for rules in [
        rules("TerminalTicketCreate", other, true),
        vec![rule(
            "TerminalTicketCreate",
            "environment",
            floor.environment,
            "self",
            "read",
        )],
        vec![rule(
            "TerminalTicketCreate",
            "environment",
            floor.environment,
            "self",
            "manage",
        )],
        rules("TerminalClose", floor.environment, true),
    ] {
        let denied = key(&floor.scene.rig.world, floor.scene.dan.id(), rules).await?;
        let reply = floor
            .scene
            .rig
            .world
            .app
            .call(TerminalTicketCreate, &[&id])
            .token(&denied.secret)
            .send_bytes()
            .await;
        assert!(!reply.status.is_success());
        assert!(
            serde_json::from_slice::<Json>(&reply.raw)?
                .get("ticket")
                .is_none()
        );
    }
    let other_person = key(
        &floor.scene.rig.world,
        floor.scene.eve.id(),
        rules("TerminalTicketCreate", floor.environment, true),
    )
    .await?;
    let reply = floor
        .scene
        .rig
        .world
        .app
        .call(TerminalTicketCreate, &[&id])
        .token(&other_person.secret)
        .send_bytes()
        .await;
    assert_eq!(
        reply.status.as_u16(),
        404,
        "even EnvironmentManage does not lend the opener's ticket"
    );
    floor.stop().await;
    Ok(())
}

async fn terminal_current(revoked: bool) -> Proof {
    let floor = Floor::new().await;
    let made = floor.shell_of(&floor.scene.dan).await;
    let id = made.session.id;
    let selected =
        session_ticket_support::legacy(&floor.scene.rig.world, floor.scene.dan.id()).await?;
    let mint = || async {
        floor
            .scene
            .rig
            .world
            .app
            .call(TerminalTicketCreate, &[&id])
            .token(&selected.secret)
            .send_bytes()
            .await
    };
    let positive = ticket(&mint().await, "real legacy-key HTTP ticket positive")?;
    let address = floor.serve().await;
    let mut screen = Screen::connect(
        address,
        &format!("/openapi/v1/ws/terminals/{id}?ticket={positive}"),
    )
    .await;
    screen.ready().await;
    screen.type_("echo actual-ticket-positive\n").await;
    screen.until("actual-ticket-positive\r\n").await;
    screen.drop_it().await;
    let held = ticket(
        &mint().await,
        "actual retained legacy ticket before key change",
    )?;
    if revoked {
        let changed = sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
            .bind(selected.id)
            .bind(floor.scene.rig.world.now())
            .execute(floor.pool())
            .await?;
        assert_eq!(changed.rows_affected(), 1);
    } else {
        session_ticket_support::current_scope(&floor.scene.rig.world, selected.id, vec![]).await?;
    }
    let mut refused = Screen::connect(
        address,
        &format!("/openapi/v1/ws/terminals/{id}?ticket={held}"),
    )
    .await;
    let actual = refused.hear_within(PATIENCE).await;
    refused.drop_it().await;
    let row = orch_db::queries::terminals::get(&mut *floor.scene.rig.world.conn().await, id)
        .await?
        .ok_or("actual shell disappeared")?;
    let closed = floor
        .fake
        .closed
        .lock()
        .map_err(|_| "native shell fixture mutex is poisoned")?
        .clone();
    floor.stop().await;
    assert!(
        matches!(actual, Some(Heard::Text(ref frame)) if frame["op"] == "closed" && frame["reason"] == "revoked"),
        "key withdrawal must precede ready/replay bytes on actual socket: {actual:?}"
    );
    assert_eq!(
        row.state, "open",
        "withdrawn ticket ends the subscription without ending already opened shell"
    );
    assert!(
        !closed.contains(&id),
        "key-only loss cannot manufacture ordinary opener permission loss"
    );
    Ok(())
}
#[tokio::test(flavor = "multi_thread")]
async fn terminal_ticket_redeem_rechecks_actual_legacy_key_revocation() -> Proof {
    terminal_current(true).await
}
#[tokio::test(flavor = "multi_thread")]
async fn terminal_ticket_redeem_rechecks_same_key_operation_scope_narrowing() -> Proof {
    terminal_current(false).await
}

#[tokio::test]
async fn browser_ticket_headers_acknowledge_one_native_view_or_control_ticket_without_returning_its_secret()
-> Proof {
    use orch_api_contract::browsers::{BrowserTicketCreate, BrowserTicketCreateBody};
    use orch_types::new_id;
    use serde_json::json;
    for control in [false, true] {
        let scene = session_ticket_support::scene(Surface::Browser).await?;
        let browser = Surface::Browser.make(&scene).await?;
        let writer = key(
            &scene.rig.world,
            scene.cai.id(),
            rules(
                selection(Surface::Browser, control),
                scene.on.environment,
                control,
            ),
        )
        .await?;
        let act = new_id();
        let input = BrowserTicketCreateBody {
            control: Some(control),
        };
        let request = scene
            .rig
            .world
            .app
            .call(BrowserTicketCreate, &[&browser])
            .token(&writer.secret)
            .header("Idempotency-Key", &act.to_string())
            .body(&input);
        let first = request.send_bytes().await;
        let secret = ticket(&first, "one actual native browser ticket")?;
        let first_body: Json = serde_json::from_slice(&first.raw)?;
        assert!(first_body.get("projection").is_none());
        let before: (Json, String) = sqlx::query_as(
            "SELECT response,xmin::text FROM idempotency_keys WHERE user_id=$1 AND key=$2",
        )
        .bind(scene.cai.id())
        .bind(act.to_string())
        .fetch_one(scene.rig.db.pool())
        .await?;
        let native: orch_types::Id = serde_json::from_value(before.0["outcome"]["id"].clone())?;
        assert_eq!(before.0["outcome"]["kind"], "browser_ticket");
        assert_eq!(
            before.0["outcome"]["data"],
            json!({"ticket_expires_at":first_body["ticket_expires_at"]})
        );
        assert!(!before.0.to_string().contains(&secret));
        sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
            .bind(writer.id)
            .bind(scene.rig.world.now())
            .execute(scene.rig.db.pool())
            .await?;
        let reader = key(
            &scene.rig.world,
            scene.cai.id(),
            vec![rule(
                "EnvironmentBrowserList",
                "environment",
                scene.on.environment,
                "self",
                "read",
            )],
        )
        .await?;
        let request = scene
            .rig
            .world
            .app
            .call(BrowserTicketCreate, &[&browser])
            .token(&reader.secret)
            .header("Idempotency-Key", &act.to_string())
            .body(&input);
        let replay = request.send_bytes().await;
        assert_eq!(replay.status.as_u16(), 200);
        assert_eq!(
            serde_json::from_slice::<Json>(&replay.raw)?,
            json!({
                "projection":"receipt", "ticket_id":native, "ticket_expires_at":first_body["ticket_expires_at"]
            })
        );
        let request = scene
            .rig
            .world
            .app
            .call(BrowserTicketCreate, &[&browser])
            .token(&reader.secret)
            .header("Idempotency-Key", &new_id().to_string())
            .body(&input);
        assert_eq!(request.send_bytes().await.status.as_u16(), 403);
        let after: (Json, String) = sqlx::query_as(
            "SELECT response,xmin::text FROM idempotency_keys WHERE user_id=$1 AND key=$2",
        )
        .bind(scene.cai.id())
        .bind(act.to_string())
        .fetch_one(scene.rig.db.pool())
        .await?;
        assert_eq!(after, before);
    }
    Ok(())
}

#[tokio::test]
async fn display_ticket_headers_acknowledge_one_native_view_or_control_ticket_without_returning_its_secret()
-> Proof {
    use orch_api_contract::displays::{DisplayTicketCreate, DisplayTicketCreateBody};
    use orch_types::new_id;
    use serde_json::json;
    for control in [false, true] {
        let scene = session_ticket_support::scene(Surface::Display).await?;
        let display = Surface::Display.make(&scene).await?;
        let writer = key(
            &scene.rig.world,
            scene.cai.id(),
            rules(
                selection(Surface::Display, control),
                scene.on.environment,
                control,
            ),
        )
        .await?;
        let act = new_id();
        let input = DisplayTicketCreateBody {
            control: Some(control),
        };
        let request = scene
            .rig
            .world
            .app
            .call(DisplayTicketCreate, &[&display])
            .token(&writer.secret)
            .header("Idempotency-Key", &act.to_string())
            .body(&input);
        let first = request.send_bytes().await;
        let secret = ticket(&first, "one actual native display ticket")?;
        let first_body: Json = serde_json::from_slice(&first.raw)?;
        assert!(first_body.get("projection").is_none());
        let before: (Json, String) = sqlx::query_as(
            "SELECT response,xmin::text FROM idempotency_keys WHERE user_id=$1 AND key=$2",
        )
        .bind(scene.cai.id())
        .bind(act.to_string())
        .fetch_one(scene.rig.db.pool())
        .await?;
        let native: orch_types::Id = serde_json::from_value(before.0["outcome"]["id"].clone())?;
        assert_eq!(before.0["outcome"]["kind"], "display_ticket");
        assert_eq!(
            before.0["outcome"]["data"],
            json!({"ticket_expires_at":first_body["ticket_expires_at"]})
        );
        assert!(!before.0.to_string().contains(&secret));
        sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
            .bind(writer.id)
            .bind(scene.rig.world.now())
            .execute(scene.rig.db.pool())
            .await?;
        let reader = key(
            &scene.rig.world,
            scene.cai.id(),
            vec![rule(
                "EnvironmentDisplayList",
                "environment",
                scene.on.environment,
                "self",
                "read",
            )],
        )
        .await?;
        let request = scene
            .rig
            .world
            .app
            .call(DisplayTicketCreate, &[&display])
            .token(&reader.secret)
            .header("Idempotency-Key", &act.to_string())
            .body(&input);
        let replay = request.send_bytes().await;
        assert_eq!(replay.status.as_u16(), 200);
        assert_eq!(
            serde_json::from_slice::<Json>(&replay.raw)?,
            json!({
                "projection":"receipt", "ticket_id":native, "ticket_expires_at":first_body["ticket_expires_at"]
            })
        );
        let request = scene
            .rig
            .world
            .app
            .call(DisplayTicketCreate, &[&display])
            .token(&reader.secret)
            .header("Idempotency-Key", &new_id().to_string())
            .body(&input);
        assert_eq!(request.send_bytes().await.status.as_u16(), 403);
        let after: (Json, String) = sqlx::query_as(
            "SELECT response,xmin::text FROM idempotency_keys WHERE user_id=$1 AND key=$2",
        )
        .bind(scene.cai.id())
        .bind(act.to_string())
        .fetch_one(scene.rig.db.pool())
        .await?;
        assert_eq!(after, before);
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn shell_ticket_headers_return_only_the_actual_accepted_identity_after_the_first_key_and_ticket_end()
-> Proof {
    use orch_types::new_id;
    use serde_json::json;
    let floor = Floor::new().await;
    let shell = floor.shell_of(&floor.scene.dan).await.session.id;
    let writer = key(
        &floor.scene.rig.world,
        floor.scene.dan.id(),
        rules("TerminalTicketCreate", floor.environment, true),
    )
    .await?;
    let act = new_id();
    let request = floor
        .scene
        .rig
        .world
        .app
        .call(TerminalTicketCreate, &[&shell])
        .token(&writer.secret)
        .header("Idempotency-Key", &act.to_string());
    let first = request.send_bytes().await;
    let secret = ticket(&first, "one actual native shell ticket")?;
    let value: Json = serde_json::from_slice(&first.raw)?;
    let before: (Json, String) = sqlx::query_as(
        "SELECT response,xmin::text FROM idempotency_keys WHERE user_id=$1 AND key=$2",
    )
    .bind(floor.scene.dan.id())
    .bind(act.to_string())
    .fetch_one(floor.pool())
    .await?;
    let id: orch_types::Id = serde_json::from_value(before.0["outcome"]["id"].clone())?;
    assert_eq!(before.0["outcome"]["kind"], "terminal_ticket");
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
        floor.scene.dan.id(),
        vec![rule(
            "EnvironmentTerminalList",
            "environment",
            floor.environment,
            "self",
            "read",
        )],
    )
    .await?;
    let request = floor
        .scene
        .rig
        .world
        .app
        .call(TerminalTicketCreate, &[&shell])
        .token(&reader.secret)
        .header("Idempotency-Key", &act.to_string());
    let replay = request.send_bytes().await;
    assert_eq!(replay.status.as_u16(), 200);
    assert_eq!(
        serde_json::from_slice::<Json>(&replay.raw)?,
        json!({
            "projection":"receipt", "ticket_id":id,"ticket_expires_at":value["ticket_expires_at"]
        })
    );
    let request = floor
        .scene
        .rig
        .world
        .app
        .call(TerminalTicketCreate, &[&shell])
        .token(&reader.secret)
        .header("Idempotency-Key", &new_id().to_string());
    assert_eq!(request.send_bytes().await.status.as_u16(), 403);
    let after: (Json, String) = sqlx::query_as(
        "SELECT response,xmin::text FROM idempotency_keys WHERE user_id=$1 AND key=$2",
    )
    .bind(floor.scene.dan.id())
    .bind(act.to_string())
    .fetch_one(floor.pool())
    .await?;
    assert_eq!(after, before);
    floor.stop().await;
    Ok(())
}
