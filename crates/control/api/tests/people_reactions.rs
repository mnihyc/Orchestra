//! Eight recent reaction glyphs belong to the person and replace only their own guarded state key.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;

use orch_api_contract::personal::{MeStateGet, MeStateSet};
use serde_json::json;

#[tokio::test]
async fn recent_choices_are_distinct_bounded_and_keep_unrelated_personal_state_and_people_apart() {
    let world = support::World::new().await;
    let ada = places::person(&world, "ada").await;
    let bea = places::person(&world, "bea").await;
    world
        .app
        .call(MeStateSet, &[&"sidebar"])
        .token(&ada.token)
        .raw_body(&json!({"folded":["topics"]}))
        .send()
        .await
        .ok();
    world
        .app
        .call(MeStateSet, &[&"reactions"])
        .token(&ada.token)
        .raw_body(&json!([
            "👍", "✅", "👍", "🎉", "❤️", "👀", "🚀", "🤔", "🙌", "💡"
        ]))
        .send()
        .await
        .ok();
    let state = world
        .app
        .call(MeStateGet, &[])
        .token(&ada.token)
        .send()
        .await
        .ok();
    assert_eq!(
        state["reactions"],
        json!(["👍", "✅", "🎉", "❤️", "👀", "🚀", "🤔", "🙌"])
    );
    assert_eq!(state["sidebar"], json!({"folded":["topics"]}));
    assert!(
        world
            .app
            .call(MeStateGet, &[])
            .token(&bea.token)
            .send()
            .await
            .ok()
            .is_empty()
    );
    let refused = world
        .app
        .call(MeStateSet, &[&"reactions"])
        .token(&ada.token)
        .raw_body(&json!([":custom:"]))
        .send()
        .await
        .problem();
    assert_eq!(refused.status, 400);
    assert_eq!(
        world
            .app
            .call(MeStateGet, &[])
            .token(&ada.token)
            .send()
            .await
            .ok()["reactions"],
        state["reactions"]
    );
}
