//! A token cannot give permissions outside its scopes, including the owner's rows and an admin's own override.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use orch_access_contract::permissions::Permission;
use orch_authz::pages::{ChangeError, Target};
use orch_authz::{Caller, Policy, TokenGrant};
use serde_json::json;
use support::{World, user_holder};

#[tokio::test]
async fn an_owners_view_only_token_cannot_give_control_of_the_page() {
    let world = World::new().await;
    let recipient = world.member("recipient").await;
    let scopes = TokenGrant::from_stored(&["thread.view".to_owned()], None).unwrap();
    let result = world.change_as(Caller::token(world.maker, scopes), Target::Live(world.thread),
        json!({ "rows": [{ "permission": "access.manage", "add": [{ "holder": user_holder(recipient) }] }] }),
        Policy { holders_share: true, ..Policy::default() }).await;
    assert!(
        matches!(result, Err(ChangeError::Denied(_))),
        "a scoped token cannot give page control: {result:?}"
    );
    assert!(
        !world
            .can(recipient, world.thread, Permission::AccessManage)
            .await
    );
}

#[tokio::test]
async fn an_admins_view_only_token_cannot_name_itself_outside_its_scopes() {
    let world = World::new().await;
    world
        .row(world.thread, Permission::ThreadManage, false)
        .await;
    let scopes = TokenGrant::from_stored(&["thread.view".to_owned()], None).unwrap();
    let result = world.change_as(Caller::token(world.boss, scopes), Target::Live(world.thread),
        json!({ "rows": [{ "permission": "thread.manage", "add": [{ "holder": user_holder(world.boss) }] }] }),
        Policy { holders_share: true, ..Policy::default() }).await;
    assert!(
        matches!(result, Err(ChangeError::Denied(_))),
        "an override cannot widen token scopes: {result:?}"
    );
    assert!(
        !world
            .can(world.boss, world.thread, Permission::ThreadManage)
            .await
    );
}

#[tokio::test]
async fn an_admins_token_cannot_use_an_override_without_the_page_management_scope() {
    let world = World::new().await;
    world
        .row(world.thread, Permission::ThreadManage, false)
        .await;
    let scopes = TokenGrant::from_stored(
        &["thread.view".to_owned(), "thread.manage".to_owned()],
        None,
    )
    .unwrap();
    let result = world.change_as(Caller::token(world.boss, scopes), Target::Live(world.thread),
        json!({ "rows": [{ "permission": "thread.manage", "add": [{ "holder": user_holder(world.boss) }] }] }),
        Policy { holders_share: true, ..Policy::default() }).await;
    assert!(
        matches!(result, Err(ChangeError::Denied(_))),
        "an override needs page management too: {result:?}"
    );
}
