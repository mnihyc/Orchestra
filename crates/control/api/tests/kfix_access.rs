//! Generic access pages describe service placement without creating membership.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

#[path = "../../authz/tests/support/mod.rs"]
mod access_world;

use access_world::World;
use orch_access_contract::pages::Holder;
use orch_access_contract::permissions::Permission as P;
use orch_api_contract::access::Holder as ApiHolder;
use orch_api_core::routes::access::page::permission_page;
use orch_authz::Caller;
use orch_authz::pages::Target;
use orch_db::queries::hierarchy::testing::insert_node;
use orch_db::queries::identity;
use orch_types::new_id;

#[tokio::test]
async fn permission_pages_mark_only_service_identities_of_another_root_as_outside() {
    let w = World::new().await;
    let department = w.node("org", w.root, "Department").await;
    let outside_root = new_id();
    let mut conn = w.conn().await;
    insert_node(&mut conn, outside_root, "org", None, "Other", w.boss)
        .await
        .unwrap();
    let mut services = Vec::new();
    for (org, username, outside) in [
        (w.root, "root-bot", false),
        (department, "dept-bot", false),
        (outside_root, "other-bot", true),
    ] {
        let service = new_id();
        identity::create_service_account(
            &mut conn,
            service,
            org,
            username,
            username,
            w.boss,
            w.now.get(),
        )
        .await
        .unwrap();
        w.name(w.thread, P::ThreadInteract, Holder::User { id: service })
            .await;
        services.push((service, outside));
    }
    let principal = w.principal(Caller::session(w.maker)).await;
    let page = w.read(w.maker, Target::Live(w.thread)).await;
    let wire = permission_page(&mut conn, &principal, &page, Some(w.root), w.now)
        .await
        .unwrap();
    for (service, outside) in services {
        let entry = wire
            .rows
            .iter()
            .flat_map(|row| &row.entries)
            .find(|entry| matches!(&entry.holder, ApiHolder::User { user } if user.id == service))
            .unwrap();
        assert_eq!(entry.outside, outside);
    }
}
