//! A page manager cannot give a permission they lack by reviving somebody else's expired entry.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use chrono::Duration;
use orch_access_contract::pages::Holder;
use orch_access_contract::permissions::Permission;
use orch_authz::pages::{ChangeError, Target};
use orch_authz::{Caller, Policy};
use serde_json::json;
use support::{World, user_holder};

#[tokio::test]
async fn a_manager_who_lacks_a_permission_cannot_revive_an_expired_entry_for_it() {
    let world = World::new().await;
    let manager = world.member("manager").await;
    let recipient = world.member("recipient").await;
    let target = Target::Live(world.thread);
    world
        .name(
            world.thread,
            Permission::AccessManage,
            Holder::User { id: manager },
        )
        .await;
    world
        .change(
            world.maker,
            target,
            json!({ "rows": [{
        "permission": "thread.manage",
        "add": [{ "holder": user_holder(recipient), "expires_at": world.later(Duration::hours(1)) }]
    }] }),
        )
        .await
        .unwrap();
    let page = world.read(world.maker, target).await;
    let entry = page
        .rows
        .iter()
        .find(|row| row.permission == Permission::ThreadManage)
        .unwrap()
        .entries[0]
        .id;
    let epoch = world.epoch().await;
    let result = world
        .change_at(
            Caller::session(manager),
            target,
            json!({ "rows": [{
        "permission": "thread.manage", "update": [{ "entry_id": entry, "expires_at": null }]
    }] }),
            Policy::default(),
            world.later(Duration::hours(2)),
        )
        .await;
    match result {
        Err(ChangeError::Denied(refusal)) => assert!(
            refusal
                .missing
                .iter()
                .any(|missing| missing.permissions.contains(&Permission::ThreadManage))
        ),
        other => panic!("a manager cannot give what they lack: {other:?}"),
    }
    assert_eq!(world.epoch().await, epoch, "a refused change gives nothing");
    assert!(
        !world
            .can_at(
                recipient,
                world.thread,
                Permission::ThreadManage,
                world.later(Duration::hours(2))
            )
            .await
    );
}
