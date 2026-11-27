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

#[tokio::test]
async fn picker_choices_are_private_newest_first_and_preserve_other_kinds_and_preferences() {
    use orch_api_contract::personal::{
        MePickerRecentChoice, MePickerRecentCreate, MePickerRecentsGet,
    };
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
    let save = |kind: &str, key: &str| MePickerRecentChoice {
        kind: kind.to_owned(),
        key: key.to_owned(),
    };
    for index in 0..8 {
        world
            .app
            .call(MePickerRecentCreate, &[])
            .token(&ada.token)
            .body(&save("agent", &format!("agent-{index}")))
            .send()
            .await
            .ok();
    }
    // Different devices selecting different kinds cannot overwrite one another's preferences.
    let agent = save("agent", "agent-5");
    let model = save("model", "provider-id/model-name");
    let (one, two) = tokio::join!(
        world
            .app
            .call(MePickerRecentCreate, &[])
            .token(&ada.token)
            .body(&agent)
            .send(),
        world
            .app
            .call(MePickerRecentCreate, &[])
            .token(&ada.token)
            .body(&model)
            .send(),
    );
    one.ok();
    two.ok();
    let answer = world
        .app
        .call(MePickerRecentsGet, &[])
        .token(&ada.token)
        .send()
        .await
        .ok();
    assert_eq!(answer.count, 5);
    assert_eq!(
        answer.choices["agent"],
        ["agent-5", "agent-7", "agent-6", "agent-4", "agent-3"]
    );
    assert_eq!(answer.choices["model"], ["provider-id/model-name"]);
    assert!(
        world
            .app
            .call(MePickerRecentsGet, &[])
            .token(&bea.token)
            .send()
            .await
            .ok()
            .choices
            .is_empty()
    );
    let state = world
        .app
        .call(MeStateGet, &[])
        .token(&ada.token)
        .send()
        .await
        .ok();
    assert_eq!(state["sidebar"], json!({"folded":["topics"]}));
    assert_eq!(
        state["picker_recents"]["model"],
        json!(["provider-id/model-name"])
    );
    let refused = world
        .app
        .call(MePickerRecentCreate, &[])
        .token(&ada.token)
        .body(&save("agent", ""))
        .send()
        .await
        .problem();
    assert_eq!(refused.status, 400);
    assert_eq!(
        world
            .app
            .call(MePickerRecentsGet, &[])
            .token(&ada.token)
            .send()
            .await
            .ok(),
        answer
    );
}

#[tokio::test]
async fn a_recent_group_can_be_longer_than_ten_or_switched_off_without_changing_any_picker_source()
{
    use orch_api_contract::personal::{
        MePickerRecentChoice, MePickerRecentCreate, MePickerRecentsGet,
    };
    let world = support::World::new().await;
    let ada = places::person(&world, "ada").await;
    sqlx::query("INSERT INTO settings (key,value) VALUES ('ui.picker_recent_count','13'::jsonb)")
        .execute(&mut *world.conn().await)
        .await
        .unwrap();
    for index in 0..13 {
        world
            .app
            .call(MePickerRecentCreate, &[])
            .token(&ada.token)
            .body(&MePickerRecentChoice {
                kind: "document".to_owned(),
                key: format!("document-{index}"),
            })
            .send()
            .await
            .ok();
    }
    let answer = world
        .app
        .call(MePickerRecentsGet, &[])
        .token(&ada.token)
        .send()
        .await
        .ok();
    assert_eq!(answer.count, 13);
    assert_eq!(answer.choices["document"].len(), 13);
    assert_eq!(answer.choices["document"][0], "document-12");
    let before = world
        .app
        .call(MeStateGet, &[])
        .token(&ada.token)
        .send()
        .await
        .ok();
    sqlx::query("UPDATE settings SET value='0'::jsonb WHERE key='ui.picker_recent_count' AND node_id IS NULL")
        .execute(&mut *world.conn().await).await.unwrap();
    let answer = world
        .app
        .call(MePickerRecentCreate, &[])
        .token(&ada.token)
        .body(&MePickerRecentChoice {
            kind: "document".to_owned(),
            key: "another-document".to_owned(),
        })
        .send()
        .await
        .ok();
    assert_eq!(
        serde_json::to_value(answer).unwrap(),
        json!({"count":0,"remembered":false})
    );
    assert_eq!(
        world
            .app
            .call(MeStateGet, &[])
            .token(&ada.token)
            .send()
            .await
            .ok(),
        before,
        "switching remembering off neither records a choice nor erases an unrelated earlier preference"
    );
    assert!(
        world
            .app
            .call(MePickerRecentsGet, &[])
            .token(&ada.token)
            .send()
            .await
            .ok()
            .choices
            .is_empty()
    );
}
