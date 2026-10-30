//! An environment's short address is the same locator on every projection and in the finder.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod resources;
mod support;

use orch_api_contract::environment::{
    EnvironmentGet, EnvironmentList, EnvironmentListQuery, EnvironmentUpdate, EnvironmentUpdateBody,
};
use orch_api_contract::search::{NameSearchList, NameSearchListQuery};
use orch_db::queries::resources::access_fixtures;
use orch_db::secrets::MasterKey;
use orch_types::{Id, Time};
use places::{Who, make_department};
use serde_json::json;
use support::World;

async fn world() -> World {
    orch_api_core::routes::resources::sealing::install_master_key(MasterKey::new([7; 32]));
    orch_api_core::routes::settings::install_master_key(MasterKey::new([7; 32]));
    World::new().await
}

async fn projections(world: &World, who: &Who, id: Id, label: &str, address: &str) -> i64 {
    let read = world
        .app
        .call(EnvironmentGet, &[&id])
        .token(&who.token)
        .send()
        .await
        .ok();
    assert_eq!(
        read.base.name, label,
        "the display label is not the address segment"
    );
    assert_eq!(read.base.address, address);
    let listed = world
        .app
        .call(EnvironmentList, &[])
        .token(&who.token)
        .query(&EnvironmentListQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(
        listed
            .iter()
            .filter_map(|row| row.as_visible())
            .find(|row| row.base.id == id)
            .expect("the owner reads its environment in the list")
            .base
            .address,
        address
    );
    let mut conn = world.conn().await;
    let row = orch_db::queries::environments::thread_engine::environment(&mut conn, id)
        .await
        .unwrap()
        .unwrap();
    let reference =
        orch_engine::views::environment_ref_json(&mut conn, &row, Time::new(world.now()))
            .await
            .unwrap();
    assert_eq!(reference["name"], label);
    assert_eq!(
        reference["address"], address,
        "thread/binding references agree with the page and list"
    );
    drop(conn);
    let found = world
        .app
        .call(NameSearchList, &[])
        .token(&who.token)
        .query(&NameSearchListQuery {
            q: Some(address.to_uppercase()),
            kinds: Some("environment".into()),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    let hit = found
        .items
        .iter()
        .find(|hit| hit.id == id)
        .expect("the displayed short address resolves in the finder");
    assert_eq!(hit.name, label);
    assert_eq!(hit.address.as_deref(), Some(address));
    read.revision
}

#[tokio::test]
async fn a_short_environment_address_agrees_across_surfaces_finds_the_same_identity_and_follows_a_rename()
 {
    for placement in ["connected", "managed"] {
        let world = world().await;
        let company = resources::company(&world).await;
        let home = make_department(
            &world,
            &company.admin,
            company.acme,
            "Human laboratory name",
            "lab",
        )
        .await
        .base
        .id;
        let id = {
            let mut conn = world.conn().await;
            access_fixtures::insert_environment(
                &mut conn,
                company.admin.id(),
                Some(home),
                "Acme workspace",
                placement,
            )
            .await
            .unwrap()
        };
        resources::name_on_page(
            &world,
            "environment",
            id,
            "access.own",
            ("user", Some(company.admin.id()), None),
        )
        .await;
        let revision = projections(
            &world,
            &company.admin,
            id,
            "Acme workspace",
            "acme.lab.admin.acme-workspace",
        )
        .await;
        let body: EnvironmentUpdateBody =
            serde_json::from_value(json!({"name":"New Workspace","expected_revision":revision}))
                .unwrap();
        let changed = world
            .app
            .call(EnvironmentUpdate, &[&id])
            .token(&company.admin.token)
            .body(&body)
            .send()
            .await
            .ok();
        assert_eq!(changed.base.id, id);
        projections(
            &world,
            &company.admin,
            id,
            "New Workspace",
            "acme.lab.admin.new-workspace",
        )
        .await;
        let hidden = world
            .app
            .call(NameSearchList, &[])
            .token(&company.stranger.token)
            .query(&NameSearchListQuery {
                q: Some("acme.lab.admin.new-workspace".into()),
                kinds: Some("environment".into()),
                ..Default::default()
            })
            .send()
            .await
            .ok();
        assert!(
            hidden.items.iter().all(|hit| hit.id != id),
            "an address never grants visibility"
        );
    }
}

#[tokio::test]
async fn punctuation_in_an_environment_label_never_becomes_dotted_address_syntax() {
    let world = world().await;
    let company = resources::company(&world).await;
    for (label, short) in [
        ("Acme / workspace.v2", Some("acme-workspace-v2")),
        ("... / ...", None),
    ] {
        let id = {
            let mut conn = world.conn().await;
            access_fixtures::insert_environment(
                &mut conn,
                company.admin.id(),
                Some(company.acme),
                label,
                "connected",
            )
            .await
            .unwrap()
        };
        resources::name_on_page(
            &world,
            "environment",
            id,
            "access.own",
            ("user", Some(company.admin.id()), None),
        )
        .await;
        let address = format!(
            "acme.admin.{}",
            short
                .map(str::to_owned)
                .unwrap_or_else(|| format!("environment-{id}"))
        );
        projections(&world, &company.admin, id, label, &address).await;
    }
}
