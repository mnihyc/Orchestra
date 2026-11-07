//! Real HTTP issuance followed by the same private redemption/current gates used by sockets.
//! These component observations assert gate behavior, not actual Browser/RFB frames or vendor UI.

#[path = "../../tests/session_ticket_support/mod.rs"]
mod fixture;

use fixture::{Proof, Surface};
use orch_types::Id;

async fn probe(
    scene: &crate::rig::Scene,
    surface: Surface,
    id: Id,
    token: &str,
) -> Proof<(bool, bool)> {
    let app = &scene.rig.world.app.app;
    let flags = match surface {
        Surface::Browser => super::browsers::ticket_scope_probe(app, id, token).await?,
        Surface::Display => super::displays::ticket_scope_probe(app, id, token).await?,
    };
    Ok(flags.ok_or("actual HTTP ticket could not be redeemed once on its original key surface")?)
}

async fn current(surface: Surface, control: bool) -> Proof {
    let scene = fixture::scene(surface).await?;
    let id = surface.make(&scene).await?;
    fixture::ticket(
        &fixture::request(&scene, surface, id, &scene.cai.token, control).await,
        "ordinary actual same-account session premise",
    )?;
    let held = fixture::legacy(&scene.rig.world, scene.cai.id()).await?;
    let positive = fixture::ticket(
        &fixture::request(&scene, surface, id, &held.secret, control).await,
        "actual sufficient legacy key HTTP premise",
    )?;
    assert_eq!(
        probe(&scene, surface, id, &positive).await?,
        (true, control),
        "actual same-path retained ticket/current ordinary gate positive"
    );
    let retained = fixture::ticket(
        &fixture::request(&scene, surface, id, &held.secret, control).await,
        "actual second HTTP ticket before same-key policy change",
    )?;
    let operation = fixture::selection(surface, control);
    let rules = if control {
        // Read of ORIGINAL .control remains valid. Another endpoint or .view is never substituted.
        fixture::rules(operation, scene.on.environment, false)
    } else {
        // Another selection's Read cannot keep this .view ticket authorized.
        fixture::rules(
            fixture::selection(surface, true),
            scene.on.environment,
            true,
        )
    };
    fixture::current_scope(&scene.rig.world, held.id, rules).await?;
    let actual = probe(&scene, surface, id, &retained).await?;
    assert_eq!(
        actual,
        if control {
            (true, false)
        } else {
            (false, false)
        },
        "restore accepted/current original selection: Read loss stops viewing; Manage loss disables driving only"
    );
    Ok(())
}

#[tokio::test]
async fn browser_view_ticket_redeem_rechecks_current_own_read() -> Proof {
    current(Surface::Browser, false).await
}
#[tokio::test]
async fn browser_control_ticket_keeps_own_read_but_drops_current_manage_after_narrowing() -> Proof {
    current(Surface::Browser, true).await
}
#[tokio::test]
async fn display_view_ticket_redeem_rechecks_current_own_read() -> Proof {
    current(Surface::Display, false).await
}
#[tokio::test]
async fn display_control_ticket_keeps_own_read_but_drops_current_manage_after_narrowing() -> Proof {
    current(Surface::Display, true).await
}

async fn no_escalation(surface: Surface) -> Proof {
    let scene = fixture::scene(surface).await?;
    let id = surface.make(&scene).await?;
    let held = fixture::legacy(&scene.rig.world, scene.cai.id()).await?;
    let positive = fixture::ticket(
        &fixture::request(&scene, surface, id, &held.secret, false).await,
        "actual view ticket positive before broader current policy",
    )?;
    assert_eq!(probe(&scene, surface, id, &positive).await?, (true, false));
    let retained = fixture::ticket(
        &fixture::request(&scene, surface, id, &held.secret, false).await,
        "actual retained view-only ticket",
    )?;
    let mut rules = fixture::rules(
        fixture::selection(surface, false),
        scene.on.environment,
        false,
    );
    rules.extend(fixture::rules(
        fixture::selection(surface, true),
        scene.on.environment,
        true,
    ));
    fixture::current_scope(&scene.rig.world, held.id, rules).await?;
    assert_eq!(
        probe(&scene, surface, id, &retained).await?,
        (true, false),
        "a native view-only ticket never gains input/driving from broader current key scope"
    );
    Ok(())
}
#[tokio::test]
async fn browser_view_only_ticket_never_escalates_into_input_after_scope_change() -> Proof {
    no_escalation(Surface::Browser).await
}
#[tokio::test]
async fn display_view_only_ticket_never_escalates_into_input_after_scope_change() -> Proof {
    no_escalation(Surface::Display).await
}

#[path = "terminal_write_socket_tests.rs"]
mod terminal_write_socket_tests;

#[path = "session_ticket_wire_tests.rs"]
mod session_ticket_wire_tests;
