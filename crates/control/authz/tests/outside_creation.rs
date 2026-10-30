//! Resource creation by an outside administrator selects an active owner and retains the creator.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use orch_access_contract::pages::{Holder, StoredKind};
use orch_access_contract::permissions::{Permission as P, ResourceKind};
use orch_authz::ownership::{self, Basis, Making, NewAccess};
use orch_authz::{Caller, Policy, TokenGrant};
use orch_db::queries::access::{self, PageKey};
use orch_db::queries::resources::access_fixtures;
use support::World;
use support::things::object;

#[tokio::test]
async fn an_outside_administrator_can_make_a_resource_but_an_admin_token_cannot_bypass_its_page() {
    let w = World::new().await;
    let outsider = w.user("outside-admin").await;
    let mut conn = w.conn().await;
    access::testing::make_system_admin(&mut conn, outsider)
        .await
        .unwrap();
    let browser = w
        .principal(Caller::session(outsider).with_admin_mode(true))
        .await;
    assert!(matches!(
        ownership::may_make(
            &mut conn,
            &browser,
            ResourceKind::Provider,
            Some(w.root),
            &[],
            Policy::default()
        )
        .await
        .unwrap(),
        Making::Allowed(Basis::Rules(_))
    ));
    let token = w
        .principal(Caller::token(outsider, TokenGrant::everything()))
        .await;
    assert!(matches!(
        ownership::may_make(
            &mut conn,
            &token,
            ResourceKind::Provider,
            Some(w.root),
            &[],
            Policy::default()
        )
        .await
        .unwrap(),
        Making::Refused(_)
    ));
}

#[tokio::test]
async fn every_managed_resource_of_an_outside_creator_is_owned_by_a_member_and_keeps_the_creators_manage_entry()
 {
    let w = World::new().await;
    let maker = w.user("outside-creator").await;
    w.name(w.root, P::OrgView, Holder::User { id: maker }).await;
    let mut conn = w.conn().await;
    for (kind, manage) in [
        ("provider", P::ProviderManage),
        ("agent", P::AgentManage),
        ("environment", P::EnvironmentManage),
        ("host", P::HostManage),
        ("template", P::TemplateManage),
        ("mcp_server", P::McpManage),
        ("mount", P::MountManage),
    ] {
        let id = access_fixtures::insert_resource(&mut conn, kind, Some(maker), Some(w.root), kind)
            .await
            .unwrap();
        ownership::start_resource(
            &mut conn,
            object(kind, id),
            maker,
            NewAccess::Defaults,
            w.now,
        )
        .await
        .unwrap();
        let page = PageKey::new(kind, id, "live");
        let entries = access::entries_of(&mut conn, &page).await.unwrap();
        assert!(
            entries.iter().any(|entry| entry.permission == "access.own"
                && entry.holder_id == Some(w.boss)
                && entry.created_by == Some(maker)),
            "{kind}"
        );
        assert!(
            entries
                .iter()
                .any(|entry| entry.permission == manage.as_str()
                    && entry.holder_id == Some(maker)
                    && entry.source == "creation"),
            "{kind}"
        );
        assert!(
            w.can_on(maker, kind, id, manage).await,
            "the creator may manage {kind}"
        );
        assert!(
            !w.can_on(maker, kind, id, P::AccessOwn).await,
            "the creator does not own {kind}"
        );
        let stored = orch_db::queries::resources::resource_facts(&mut conn, &[id])
            .await
            .unwrap();
        assert_eq!(stored[0].owner_id, Some(w.boss), "{kind}");
    }
    assert!(
        access::members_among(&mut conn, w.root, &[maker])
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn an_outside_creators_manage_entry_ends_when_their_way_into_its_place_ends() {
    let w = World::new().await;
    let maker = w.user("outside-creator").await;
    let way = w
        .name(w.root, P::OrgInteract, Holder::User { id: maker })
        .await;
    let mut conn = w.conn().await;
    let project = orch_types::new_id();
    orch_db::queries::hierarchy::testing::insert_node(
        &mut conn,
        project,
        "project",
        Some(w.root),
        "Their project",
        maker,
    )
    .await
    .unwrap();
    orch_authz::pages::start(&mut conn, StoredKind::Node, project, maker, w.now)
        .await
        .unwrap();
    assert!(w.can(maker, project, P::ProjectManage).await);
    sqlx::query("DELETE FROM permission_entries WHERE id = $1")
        .bind(way)
        .execute(&mut *conn)
        .await
        .unwrap();
    assert!(!w.can(maker, project, P::ProjectManage).await);
    assert!(w.can(w.boss, project, P::AccessOwn).await);
}

#[tokio::test]
async fn an_inactive_nearer_owner_is_skipped_without_choosing_an_outside_creator() {
    let w = World::new().await;
    let outsider = w.user("former-owner").await;
    let department = w.node_owned_by("org", w.root, "Department", outsider).await;
    let mut conn = w.conn().await;
    assert_eq!(
        ownership::owner_of_new(&mut conn, outsider, Some(department))
            .await
            .unwrap(),
        w.boss
    );
}
