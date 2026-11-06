//! Actual ticket issuance uses each native door's own operation and current account bounds.

mod places;
mod rig;
mod session_ticket_support;
mod support;
mod terminals_support;
mod thread_history_scope;

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
    let closed = floor.fake.closed.lock().unwrap().clone();
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
