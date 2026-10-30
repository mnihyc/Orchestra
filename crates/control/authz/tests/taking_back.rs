//! A sharer takes back their own entry only while they could give its permission again.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use orch_access_contract::pages::Holder;
use orch_access_contract::permissions::Permission;
use orch_authz::pages::{ChangeError, Target};
use orch_authz::{Caller, Policy};
use serde_json::json;
use support::{World, user_holder};

#[tokio::test]
async fn a_sharer_who_lost_the_permission_cannot_take_their_old_entry_back() {
    let world = World::new().await;
    let sharer = world.member("sharer").await;
    let recipient = world.member("recipient").await;
    let target = Target::Live(world.thread);
    world
        .name(
            world.thread,
            Permission::ThreadView,
            Holder::User { id: sharer },
        )
        .await;
    let own = world
        .name(
            world.thread,
            Permission::ThreadInteract,
            Holder::User { id: sharer },
        )
        .await;
    let policy = Policy {
        holders_share: true,
        ..Policy::default()
    };
    world
        .change_as(
            Caller::session(sharer),
            target,
            json!({ "rows": [{
        "permission": "thread.interact", "add": [{ "holder": user_holder(recipient) }]
    }] }),
            policy,
        )
        .await
        .unwrap();
    let page = world.read(world.maker, target).await;
    let given = page
        .rows
        .iter()
        .find(|row| row.permission == Permission::ThreadInteract)
        .unwrap()
        .entries
        .iter()
        .find(|entry| entry.holder == (Holder::User { id: recipient }))
        .unwrap()
        .id;
    world
        .change(
            world.maker,
            target,
            json!({ "rows": [{
        "permission": "thread.interact", "remove": [own]
    }] }),
        )
        .await
        .unwrap();
    assert!(
        !world
            .can(sharer, world.thread, Permission::ThreadInteract)
            .await
    );
    let epoch = world.epoch().await;
    let result = world
        .change_as(
            Caller::session(sharer),
            target,
            json!({ "rows": [{
        "permission": "thread.interact", "remove": [given]
    }] }),
            policy,
        )
        .await;
    match result {
        Err(ChangeError::Denied(refusal)) => assert!(
            refusal
                .missing
                .iter()
                .any(|missing| missing.permissions.contains(&Permission::ThreadInteract))
        ),
        other => panic!("a former holder cannot take their old entry back: {other:?}"),
    }
    assert_eq!(world.epoch().await, epoch);
    assert!(
        world
            .can(recipient, world.thread, Permission::ThreadInteract)
            .await
    );
}
