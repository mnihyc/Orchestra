//! The shell bootstrap agrees with its existing reads and never crosses the signed-in person's scope.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod chat_world;
mod places;
mod support;

use chat_world::Room;
use orch_api_contract::auth::{
    AdminModeSetBody, MeAdminModeGet, MeAdminModeSet, MeAuthorityGet, MeBootstrapGet, MeGet,
    MetaGet,
};
use orch_api_contract::chats::{ChatLevel, ChatStateSet, ChatStateSetBody};
use orch_api_contract::nodes::{DeletedInspectionQuery, NodeGet, OrgList};
use orch_api_contract::notifications::{NotificationSummaryGet, NotificationSummaryGetQuery};
use orch_api_contract::personal::{
    MeChatStateList, MeFavoriteList, MeFavoriteSet, MeFavoriteSetBody, MeReadStateList,
    MeReadStateListQuery, MeStateGet, MeStateSet, MeSubscriptionList, MeSubscriptionListQuery,
};
use serde_json::json;

#[tokio::test]
async fn one_shell_answer_contains_the_same_personal_projections_as_their_reads() {
    let room = Room::new().await;
    let app = &room.world.app;
    app.call(MeStateSet, &[&"sidebar"])
        .token(&room.ada.token)
        .body(&json!({"open": false}))
        .send()
        .await
        .ok();
    app.call(MeFavoriteSet, &[&room.project])
        .token(&room.ada.token)
        .body(&MeFavoriteSetBody {
            after_node_id: None,
            view: None,
        })
        .send()
        .await
        .ok();
    app.call(ChatStateSet, &[&room.chat])
        .token(&room.ada.token)
        // A node's chat has a bell and no place on a list of one's own chats to be taken off.
        .body(&ChatStateSetBody {
            level: Some(Some(ChatLevel::All)),
            hidden: None,
        })
        .send()
        .await
        .ok();
    let reply = app
        .call(MeBootstrapGet, &[])
        .token(&room.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(
        reply.person,
        app.call(MeGet, &[])
            .token(&room.ada.token)
            .send()
            .await
            .ok()
    );
    assert_eq!(
        reply.meta,
        app.call(MetaGet, &[])
            .token(&room.ada.token)
            .send()
            .await
            .ok()
    );
    let authority = app
        .call(MeAuthorityGet, &[])
        .token(&room.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(authority.person, reply.person);
    assert_eq!(authority.admin_mode, reply.admin_mode);
    assert_eq!(authority.orgs, reply.orgs);
    assert_eq!(authority.meta, reply.meta);
    assert_eq!(
        reply.state,
        app.call(MeStateGet, &[])
            .token(&room.ada.token)
            .send()
            .await
            .ok()
    );
    assert_eq!(
        reply.orgs,
        app.call(OrgList, &[])
            .token(&room.ada.token)
            .send()
            .await
            .ok()
    );
    assert_eq!(
        reply.admin_mode,
        app.call(MeAdminModeGet, &[])
            .token(&room.ada.token)
            .send()
            .await
            .ok()
    );
    assert_eq!(
        reply.favorites,
        app.call(MeFavoriteList, &[])
            .token(&room.ada.token)
            .send()
            .await
            .ok()
    );
    assert_eq!(
        reply.chat_states,
        app.call(MeChatStateList, &[])
            .token(&room.ada.token)
            .send()
            .await
            .ok()
    );
    assert_eq!(
        reply.read_states,
        app.call(MeReadStateList, &[])
            .token(&room.ada.token)
            .query(&MeReadStateListQuery::default())
            .send()
            .await
            .ok()
    );
    assert_eq!(
        reply.subscriptions,
        app.call(MeSubscriptionList, &[])
            .token(&room.ada.token)
            .query(&MeSubscriptionListQuery::default())
            .send()
            .await
            .ok()
    );
    assert_eq!(
        reply.notification_summary,
        app.call(NotificationSummaryGet, &[])
            .token(&room.ada.token)
            .query(&NotificationSummaryGetQuery::default())
            .send()
            .await
            .ok()
    );
    assert_eq!(reply.state.get("sidebar"), Some(&json!({"open":false})));
    assert_eq!(reply.favorites[0].node.id, room.project);
    assert!(!reply.admin_mode.as_admin);
    assert!(reply.admin_mode.available);

    app.call(MeAdminModeSet, &[])
        .token(&room.ada.token)
        .body(&AdminModeSetBody { as_admin: true })
        .send()
        .await
        .ok();
    assert!(
        app.call(MeBootstrapGet, &[])
            .token(&room.ada.token)
            .send()
            .await
            .ok()
            .admin_mode
            .as_admin
    );
}

#[tokio::test]
async fn a_served_page_keeps_its_portable_location_without_replacing_sidebar_choices() {
    let room = Room::new().await;
    let app = &room.world.app;
    app.call(MeStateSet, &[&"sidebar"])
        .token(&room.ada.token)
        .body(&json!({"folded":["recent"],"show":{"threads":false}}))
        .send()
        .await
        .ok();
    let route = format!("/project/{}?view=board", room.project);
    app.call(NodeGet, &[&room.project])
        .token(&room.ada.token)
        .query(&DeletedInspectionQuery::default())
        .header("x-orch-view", &route)
        .send()
        .await
        .ok();
    let state = app
        .call(MeStateGet, &[])
        .token(&room.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(state["sidebar"]["last_place"]["route"], route);
    assert_eq!(
        state["sidebar"]["last_place"]["node_id"],
        room.project.to_string()
    );
    assert_eq!(state["sidebar"]["folded"], json!(["recent"]));
    assert_eq!(state["sidebar"]["show"], json!({"threads":false}));
}

#[tokio::test]
async fn the_bootstrap_refuses_anonymous_and_has_none_of_another_persons_stars_or_state() {
    let room = Room::new().await;
    room.world
        .app
        .call(MeStateSet, &[&"sidebar"])
        .token(&room.ada.token)
        .body(&json!({"private": "Ada's layout"}))
        .send()
        .await
        .ok();
    room.world
        .app
        .call(MeFavoriteSet, &[&room.project])
        .token(&room.ada.token)
        .body(&MeFavoriteSetBody {
            after_node_id: None,
            view: None,
        })
        .send()
        .await
        .ok();
    let other = room
        .world
        .app
        .call(MeBootstrapGet, &[])
        .token(&room.eve.token)
        .send()
        .await
        .ok();
    assert_eq!(other.person.user.id, room.eve.id());
    assert!(other.favorites.is_empty());
    assert!(other.state.is_empty());
    assert!(other.orgs.iter().all(|org| org.base.id != room.root));
    assert!(!other.admin_mode.available);
    assert_eq!(
        room.world.app.call(MeBootstrapGet, &[]).send().await.status,
        401
    );
}

#[tokio::test]
async fn administrator_mode_does_not_fill_the_shell_with_other_peoples_organizations() {
    let room = Room::new().await;
    let foreign = places::make_org(&room.world, &room.eve, "Another team", "another-team").await;
    room.world.make_admin(&room.ada.person).await;
    room.world
        .app
        .call(MeAdminModeSet, &[])
        .token(&room.ada.token)
        .body(&AdminModeSetBody { as_admin: true })
        .send()
        .await
        .ok();
    let shell = room
        .world
        .app
        .call(MeBootstrapGet, &[])
        .token(&room.ada.token)
        .send()
        .await
        .ok();
    assert!(shell.admin_mode.as_admin);
    assert!(shell.orgs.iter().any(|org| org.base.id == room.root));
    assert!(shell.orgs.iter().all(|org| org.base.id != foreign.base.id));
}
