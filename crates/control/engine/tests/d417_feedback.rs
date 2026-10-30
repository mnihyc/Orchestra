//! Blocked turns expose actionable product explanations, with the same text in the catalogues.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
mod common;

use common::{World, queued};
use orch_authz::Caller;
use orch_engine::sessions::{APPROVAL_NOT_ALLOWED, APPROVAL_UNSUPPORTED, SANDBOX_UNSUPPORTED};
use orch_types::Json;
use serde_json::json;

#[test]
fn fixed_confinement_explanations_match_the_english_catalogue_and_name_the_setting_to_change() {
    let catalogue: Json = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../frontend/src/i18n/en.json"
    )))
    .unwrap();
    for (key, message, setting) in [
        (
            "approvalNotAllowed",
            APPROVAL_NOT_ALLOWED,
            "approval.allowed_modes",
        ),
        ("approvalUnsupported", APPROVAL_UNSUPPORTED, "approval.mode"),
        ("sandboxUnsupported", SANDBOX_UNSUPPORTED, "sandbox.mode"),
    ] {
        assert_eq!(catalogue["threadConfinement"][key].as_str(), Some(message));
        assert!(message.contains(setting));
        assert!(message.contains("Choose"));
    }
}

#[tokio::test]
async fn an_unsupported_sandbox_blocks_the_turn_with_the_actionable_setting_names() {
    let world = World::observed().await;
    let thread = world.thread("An enforceable sandbox").await;
    world
        .set(world.root, "sandbox.mode", json!("workspace"))
        .await;
    let mut descriptor = common::descriptor();
    descriptor["sandbox_modes"] = json!(["open"]);
    // The harness says so at every start too: the world's reader of the subscription's facts starts it in the
    // background, and what a start reports is what the profile keeps.
    world.declares(descriptor).await;
    world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("work within the selected sandbox"),
        )
        .await
        .unwrap();
    world.wait_blocked(thread, "binding_invalid").await;
    let view = world.sync(thread).await;
    let detail = view.thread.blocked.unwrap().detail.unwrap();
    assert!(detail.contains("This place asks for Workspace"), "{detail}");
    assert!(detail.contains("cautious default"), "{detail}");
    assert!(
        detail.contains("Allow Full access in this place's Sandbox setting"),
        "{detail}"
    );
    assert!(
        detail.contains("machine's policy") && detail.contains("environment template"),
        "{detail}"
    );
    assert!(
        world.turn_starts(thread).is_empty(),
        "an explanation never relaxes enforcement"
    );
}
