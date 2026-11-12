//! Where a provider's Use entry lets work run, written over HTTP: the member is stored and read back as written, previewed and audited with the entry, refused for a place the editor cannot see or for a shape it does not have, and a name that is already on the entry stays when its place is gone.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod resources;
mod support;

use crate::support::FullPermissionPageReply;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::ResourceKind;
use orch_api_contract::access::{
    HolderInput, PageChange, PageChangeAdd, PageChangeRow, PageChangeUpdate, PagePreview,
    PermissionPage, PermissionPageGet, PermissionPagePreview, PermissionPageUpdate,
};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::resources::{CreateProvider, Provider, ProviderCreate, Upstream};
use orch_api_core::routes::resources::sealing::install_master_key;
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::queries::resources::access_fixtures as fixtures;
use orch_db::secrets::MasterKey;
use orch_types::{Id, Json, Time};
use places::{Who, make_org};
use resources::company;
use serde_json::json;
use support::World;

async fn world() -> World {
    install_master_key(MasterKey::new([7; 32]));
    World::new().await
}

async fn provider(world: &World, by: &Who, home: Id) -> Provider {
    world
        .app
        .call(ProviderCreate, &[])
        .token(&by.token)
        .body(&CreateProvider::Api {
            runtime_machine_id: None,
            name: "Team key".to_owned(),
            home_id: home,
            access: None,
            arrangements: None,
            runtime_config: None,
            upstreams: vec![Upstream {
                protocol: "openai-responses".to_owned(),
                base_url: "https://api.example.com/v1".to_owned(),
                options: json!({}),
            }],
            api_key: Some("sk-test".to_owned()),
            secrets: None,
            key_machine_id: None,
            sources: None,
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap()
        .provider
}

/// An environment of `owner` at home `home`, with a page of its own that its owner holds.
async fn environment(world: &World, owner: &Who, home: Id, name: &str) -> Id {
    let mut conn = world.conn().await;
    let id = fixtures::insert_environment(&mut conn, owner.id(), Some(home), name, "managed")
        .await
        .unwrap();
    start_resource(
        &mut conn,
        ObjectRef {
            kind: ResourceKind::Environment.into(),
            id,
        },
        owner.id(),
        NewAccess::Defaults,
        Time::new(world.now()),
    )
    .await
    .unwrap();
    id
}

fn place(kind: &str, id: Id) -> Json {
    json!({ "kind": kind, "id": id })
}

fn naming_with(person: Id, options: Json) -> PageChange {
    PageChange {
        rows: Some(vec![PageChangeRow {
            permission: "provider.use".to_owned(),
            profile: None,
            inherit: None,
            locked: None,
            add: Some(vec![PageChangeAdd {
                holder: HolderInput::User { id: person },
                options: Some(options),
                expires_at: None,
                note: None,
            }]),
            update: None,
            remove: None,
        }]),
        ..PageChange::default()
    }
}

async fn read(world: &World, by: &Who, id: Id) -> PermissionPage {
    world
        .app
        .call(PermissionPageGet, &[&"providers", &id])
        .token(&by.token)
        .send()
        .await
        .ok()
}

async fn change(
    world: &World,
    by: &Who,
    id: Id,
    change: &PageChange,
) -> orch_api_core::testing::Reply<PermissionPage> {
    world
        .app
        .call(PermissionPageUpdate, &[&"providers", &id])
        .token(&by.token)
        .body(change)
        .send()
        .await
        .full_permission_page()
}

fn stored_options(page: &PermissionPage) -> Vec<Json> {
    page.rows
        .iter()
        .filter(|row| row.permission == "provider.use")
        .flat_map(|row| row.entries.iter().map(|entry| entry.options.clone()))
        .collect()
}

#[tokio::test]
async fn where_is_written_on_an_entry_read_back_as_written_previewed_and_audited() {
    let world = world().await;
    let c = company(&world).await;
    let id = provider(&world, &c.boss, c.acme).await.base.id;
    let dev = environment(&world, &c.boss, c.acme, "dev").await;
    let options = json!({ "where": [place("environment", dev)], "models": ["gpt-x"] });
    let adding = naming_with(c.viewer.id(), options.clone());

    // Said before it is made: one page, the person who would gain, and nothing is changed.
    let before = read(&world, &c.boss, id).await;
    let preview: PagePreview = world
        .app
        .call(PermissionPagePreview, &[&"providers", &id])
        .token(&c.boss.token)
        .body(&adding)
        .send()
        .await
        .ok();
    assert_eq!(preview.pages, 1);
    assert!(
        preview
            .gaining
            .iter()
            .any(|person| person.user.id == c.viewer.id()),
        "{preview:?}"
    );
    assert_eq!(read(&world, &c.boss, id).await.revision, before.revision);

    let changed = change(&world, &c.boss, id, &adding).await.ok();
    assert!(
        stored_options(&changed).contains(&options),
        "the member comes back as written: {:?}",
        stored_options(&changed)
    );
    assert_eq!(
        stored_options(&read(&world, &c.boss, id).await),
        stored_options(&changed)
    );

    let audited: Vec<(String, Json)> = sqlx::query_as(
        "SELECT action, details FROM audit_log WHERE action LIKE 'permission.%' ORDER BY at, id",
    )
    .fetch_all(&world.db.pool)
    .await
    .unwrap();
    let line = audited
        .iter()
        .find(|(action, details)| {
            action == "permission.added" && details["options"]["where"].is_array()
        })
        .unwrap_or_else(|| panic!("the entry is audited with its options: {audited:?}"));
    assert_eq!(
        line.1["options"]["where"],
        json!([place("environment", dev)]),
        "the audit row carries the place by kind and identifier"
    );
}

#[tokio::test]
async fn a_place_the_editor_cannot_see_or_that_is_not_there_is_refused_naming_the_member() {
    let world = world().await;
    let c = company(&world).await;
    let id = provider(&world, &c.boss, c.acme).await.base.id;
    // An environment of another org, which the editor sees nothing of.
    let outside = places::person(&world, "outsider").await;
    let other = make_org(&world, &outside, "Other", "other").await.base.id;
    let hidden = environment(&world, &outside, other, "private").await;
    for options in [
        json!({ "where": [place("environment", hidden)] }),
        json!({ "where": [place("environment", orch_types::new_id())] }),
        json!({ "where": [place("host", hidden)] }),
    ] {
        let refused = change(&world, &c.boss, id, &naming_with(c.viewer.id(), options))
            .await
            .problem();
        assert_eq!(refused.code, ErrorCode::RequestInvalid);
        let faults = refused.extra["errors"].as_array().unwrap();
        assert!(
            faults.iter().any(|fault| fault["path"]
                .as_str()
                .is_some_and(|path| path.contains("options.where[0]"))),
            "{faults:?}"
        );
    }
    // A shape it does not have is refused at the name that is wrong, before anything is looked up.
    let refused = change(
        &world,
        &c.boss,
        id,
        &naming_with(c.viewer.id(), json!({ "where": ["not-a-reference"] })),
    )
    .await
    .problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
    assert!(
        refused.extra["errors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|fault| fault["path"]
                .as_str()
                .is_some_and(|path| path.ends_with("where[0]")))
    );
    let page = read(&world, &c.boss, id).await;
    assert!(stored_options(&page).is_empty(), "nothing was written");
}

#[tokio::test]
async fn a_name_that_is_already_on_the_entry_stays_when_its_place_is_gone() {
    let world = world().await;
    let c = company(&world).await;
    let id = provider(&world, &c.boss, c.acme).await.base.id;
    let dev = environment(&world, &c.boss, c.acme, "dev").await;
    let options = json!({ "where": [place("environment", dev)] });
    let page = change(
        &world,
        &c.boss,
        id,
        &naming_with(c.viewer.id(), options.clone()),
    )
    .await
    .ok();
    let entry = page
        .rows
        .iter()
        .find(|row| row.permission == "provider.use")
        .and_then(|row| row.entries.first())
        .unwrap()
        .id;
    sqlx::query("UPDATE environments SET deleted_at = now() WHERE id = $1")
        .bind(dev)
        .execute(&world.db.pool)
        .await
        .unwrap();
    // The entry is edited for something else, with the name it has: that is not an error.
    let kept = json!({ "where": [place("environment", dev)], "models": ["gpt-x"] });
    let updating = PageChange {
        rows: Some(vec![PageChangeRow {
            permission: "provider.use".to_owned(),
            profile: None,
            inherit: None,
            locked: None,
            add: None,
            update: Some(vec![PageChangeUpdate {
                entry_id: entry,
                options: Some(kept.clone()),
                expires_at: None,
                note: None,
            }]),
            remove: None,
        }]),
        ..PageChange::default()
    };
    let changed = change(&world, &c.boss, id, &updating).await.ok();
    assert!(stored_options(&changed).contains(&kept));
    // A name that is gone is not one it can be given anew.
    let again = change(
        &world,
        &c.boss,
        id,
        &naming_with(
            c.admin.id(),
            json!({ "where": [place("environment", dev)] }),
        ),
    )
    .await
    .problem();
    assert_eq!(again.code, ErrorCode::RequestInvalid);
}
