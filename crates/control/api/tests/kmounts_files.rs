//! Mount resources and machine locations are governed by their pages, conceal local paths and explain uses before deletion.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod resources;
mod support;

use orch_api_contract::access::NewResourceAccess;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::mounts::*;
use orch_types::Id;
use resources::{company, name_person};
use serde_json::json;
use support::World;

fn create_body(home: Id) -> MountCreateBody {
    MountCreateBody {
        name: "datasets".into(),
        description: Some("Team data".into()),
        home_id: Some(home),
        target: "/mnt/datasets".into(),
        read_only: Some(true),
        default_fate: None,
        access: Some(NewResourceAccess::OnlyMe),
    }
}

async fn named_mount(world: &World, person: &places::Who, home: Id, name: &str) -> Mount {
    let mut body = create_body(home);
    body.name = name.to_owned();
    world
        .app
        .call(MountCreate, &[])
        .token(&person.token)
        .body(&body)
        .send()
        .await
        .ok()
}

async fn own_page(
    world: &World,
    kind: orch_access_contract::permissions::ResourceKind,
    id: Id,
    owner: Id,
) {
    let mut conn = world.conn().await;
    orch_authz::ownership::start_resource(
        &mut conn,
        orch_access_contract::decisions::ObjectRef {
            kind: kind.into(),
            id,
        },
        owner,
        orch_authz::ownership::NewAccess::OnlyMe,
        orch_types::Time::new(world.now()),
    )
    .await
    .unwrap();
}

async fn attached_on_host(
    world: &World,
    host: Id,
    home: Id,
    owner: Id,
    name: &str,
    mount: Id,
) -> Id {
    use orch_db::queries::environments::hosting::{NewRuntime, insert_runtime};
    use orch_db::queries::resources::access_fixtures as fixtures;
    let mut conn = world.conn().await;
    let environment = fixtures::insert_environment(&mut conn, owner, Some(home), name, "managed")
        .await
        .unwrap();
    let template: Id = sqlx::query_scalar("SELECT template_id FROM environments WHERE id=$1")
        .bind(environment)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    let connector = fixtures::insert_persons_machine(&mut conn, owner, name)
        .await
        .unwrap();
    let runtime = orch_types::new_id();
    insert_runtime(
        &mut conn,
        &NewRuntime {
            id: runtime,
            host_id: host,
            unit_name: &format!("mount-test-{runtime}"),
            environment_id: environment,
            driver: "docker",
            template_id: template,
            spec_hash: &[0; 32],
            generation: 1,
            volumes: &json!({}),
        },
        world.now(),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE machines SET placement='managed',owner_id=NULL,host_id=$2,runtime_id=$3 WHERE id=$1")
        .bind(connector).bind(host).bind(runtime).execute(&mut *conn).await.unwrap();
    sqlx::query("UPDATE runtimes SET machine_id=$2,state='running' WHERE id=$1")
        .bind(runtime)
        .bind(connector)
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE environments SET machine_id=$2,status='running',facts=$3 WHERE id=$1")
        .bind(environment)
        .bind(connector)
        .bind(json!({"mounts":[{"mount_id":mount,"state":"attached"}]}))
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    own_page(
        world,
        orch_access_contract::permissions::ResourceKind::Environment,
        environment,
        owner,
    )
    .await;
    environment
}

#[tokio::test]
async fn directory_choices_on_a_machine_without_a_host_return_its_roots_and_unreported_facts() {
    use orch_db::queries::resources::access_fixtures;
    let world = World::new().await;
    let c = company(&world).await;
    let mut conn = world.conn().await;
    let machine =
        access_fixtures::insert_persons_machine(&mut conn, c.viewer.id(), "Directory machine")
            .await
            .unwrap();
    let policy = orch_link_contract::policy::LocalPolicy {
        environment_roots: vec!["/srv/notes".into()],
        ..Default::default()
    };
    sqlx::query("UPDATE machines SET facts=$2 WHERE id=$1")
        .bind(machine)
        .bind(json!({"policy":policy}))
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    let page = world
        .app
        .call(MachineEnvironmentDataGet, &[&machine])
        .token(&c.viewer.token)
        .send()
        .await
        .ok();
    assert_eq!(page.machine_id, machine);
    assert_eq!(page.roots.len(), 1);
    assert_eq!(page.roots[0].path, "/srv/notes");
    assert!(page.roots[0].uses.is_empty());
    assert!(page.direct.is_none());
    assert!(page.drivers.is_empty());
    let refused = world
        .app
        .call(MachineEnvironmentDataGet, &[&machine])
        .token(&c.stranger.token)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::NotFound);
    sqlx::query("UPDATE machines SET facts='{}'::jsonb WHERE id=$1")
        .bind(machine)
        .execute(&world.db.pool)
        .await
        .unwrap();
    let unreported = world
        .app
        .call(MachineEnvironmentDataGet, &[&machine])
        .token(&c.viewer.token)
        .send()
        .await
        .ok();
    assert!(unreported.roots.is_empty());
    assert!(unreported.direct.is_none());
    assert!(unreported.drivers.is_empty());
}

#[tokio::test]
async fn a_machines_mounts_are_scoped_to_that_machine_and_its_owner_and_only_offer_manageable_additions()
 {
    use orch_db::queries::resources::{access_fixtures as fixtures, mounts as rows};
    let world = World::new().await;
    let c = company(&world).await;
    let visible = named_mount(&world, &c.viewer, c.acme, "visible-data").await;
    let _available = named_mount(&world, &c.viewer, c.acme, "available-data").await;
    let hidden = named_mount(
        &world,
        &c.stranger,
        c.stranger.person.personal_org,
        "hidden-data",
    )
    .await;
    let mut conn = world.conn().await;
    let machine = fixtures::insert_persons_machine(&mut conn, c.viewer.id(), "Laptop")
        .await
        .unwrap();
    let other = fixtures::insert_persons_machine(&mut conn, c.viewer.id(), "Other laptop")
        .await
        .unwrap();
    for (mount, machine, source) in [
        (visible.base.id, machine, "/srv/visible"),
        (hidden.base.id, machine, "/srv/private"),
        (visible.base.id, other, "/srv/other"),
    ] {
        rows::set_location(
            &mut conn,
            &rows::LocationRow {
                mount_id: mount,
                machine_id: machine,
                host_id: None,
                host_name: None,
                kind: "path".into(),
                source: source.into(),
                storage: None,
                data_kind: None,
                created_by: c.viewer.id(),
                created_at: world.now(),
            },
        )
        .await
        .unwrap();
    }
    drop(conn);
    let page = world
        .app
        .call(MachineMountList, &[&machine])
        .token(&c.viewer.token)
        .send()
        .await
        .ok();
    assert_eq!(page.locations.len(), 1);
    assert_eq!(page.locations[0].mount.id, visible.base.id);
    assert_eq!(page.locations[0].source.as_deref(), Some("/srv/visible"));
    assert!(page.locations[0].can_remove);
    assert_eq!(page.locations[0].size_bytes, None);
    assert_eq!(
        page.available
            .iter()
            .map(|choice| choice.mount.name.as_str())
            .collect::<Vec<_>>(),
        vec!["available-data"]
    );
    assert!(
        page.available[0].path
            && !page.available[0].default_storage
            && page.available[0].storages.is_empty()
    );
    assert!(
        !serde_json::to_string(&page)
            .unwrap()
            .contains("hidden-data")
    );
    assert_eq!(
        world
            .app
            .call(MachineMountList, &[&machine])
            .token(&c.admin.token)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::NotFound
    );
    let other = world
        .app
        .call(MachineMountList, &[&other])
        .token(&c.viewer.token)
        .send()
        .await
        .ok();
    assert_eq!(other.locations[0].source.as_deref(), Some("/srv/other"));
}

#[tokio::test]
async fn machine_directory_roots_and_driver_choices_name_only_readable_environment_uses() {
    use orch_db::queries::resources::access_fixtures as fixtures;
    let world = World::new().await;
    let c = company(&world).await;
    let mut conn = world.conn().await;
    // The boss's machine, which the viewer manages: its owner manages every environment registered
    // on it, whoever's it is, and whoever else manages it reads only the environments they may.
    let machine = fixtures::insert_persons_machine(&mut conn, c.boss.id(), "Directory machine")
        .await
        .unwrap();
    orch_db::queries::access::testing::add_entry(
        &mut conn,
        &orch_db::queries::access::PageKey::new("machine", machine, "live"),
        "any",
        "machine.manage",
        ("user", Some(c.viewer.id()), None),
        None,
        world.now(),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE machines SET facts=$2 WHERE id=$1").bind(machine).bind(json!({
        "policy":{"environment.roots":["/srv/work"]},
        "environment_descriptor":{"lifecycle":"attach","data_storage":{"made_by_orch":"directory","existing_directory":true,"reason":null}}
    })).execute(&mut *conn).await.unwrap();
    let shown = fixtures::insert_environment(
        &mut conn,
        c.viewer.id(),
        Some(c.acme),
        "Visible directory",
        "connected",
    )
    .await
    .unwrap();
    let hidden = fixtures::insert_environment(
        &mut conn,
        c.stranger.id(),
        Some(c.stranger.person.personal_org),
        "Secret directory",
        "connected",
    )
    .await
    .unwrap();
    for (id, path) in [(shown, "/srv/work/visible"), (hidden, "/srv/work/secret")] {
        sqlx::query("UPDATE environments SET machine_id=$2,workspace_root=$3 WHERE id=$1")
            .bind(id)
            .bind(machine)
            .bind(path)
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    drop(conn);
    own_page(
        &world,
        orch_access_contract::permissions::ResourceKind::Environment,
        shown,
        c.viewer.id(),
    )
    .await;
    own_page(
        &world,
        orch_access_contract::permissions::ResourceKind::Environment,
        hidden,
        c.stranger.id(),
    )
    .await;
    let page = world
        .app
        .call(MachineEnvironmentDataGet, &[&machine])
        .token(&c.viewer.token)
        .send()
        .await
        .ok();
    assert_eq!(page.roots.len(), 1, "{:?}", page.roots);
    assert_eq!(page.roots[0].path, "/srv/work");
    assert_eq!(page.roots[0].uses.len(), 1);
    assert_eq!(page.roots[0].uses[0].environment.id, shown);
    assert_eq!(
        page.direct.unwrap().made_by_orch,
        Some(orch_driver_contract::descriptor::MadeDataKind::Directory)
    );
    assert!(
        !serde_json::to_string(&page.roots)
            .unwrap()
            .contains("secret")
    );
    let owners = world
        .app
        .call(MachineEnvironmentDataGet, &[&machine])
        .token(&c.boss.token)
        .send()
        .await
        .ok();
    assert_eq!(
        owners.roots[0].uses.len(),
        2,
        "the owner of the machine manages both: {:?}",
        owners.roots
    );
    assert_eq!(
        world
            .app
            .call(MachineEnvironmentDataGet, &[&machine])
            .token(&c.stranger.token)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::NotFound
    );
}

#[tokio::test]
async fn a_directory_machine_offers_owned_mount_data_from_facts_and_bounds_paths_by_the_same_roots()
{
    use orch_db::queries::resources::{access_fixtures as fixtures, mounts as rows};
    use orch_driver_contract::descriptor::{
        DriverDescriptor, EnvironmentDataStorage, MadeDataKind,
    };
    let world = World::new().await;
    let c = company(&world).await;
    let mount = named_mount(&world, &c.viewer, c.acme, "machine-data").await;
    let mut conn = world.conn().await;
    let machine = fixtures::insert_persons_machine(&mut conn, c.viewer.id(), "Directory maker")
        .await
        .unwrap();
    let policy = orch_link_contract::policy::LocalPolicy {
        environment_roots: vec!["/srv/work".into()],
        ..Default::default()
    };
    let descriptor = DriverDescriptor::registered_directories(
        "1",
        EnvironmentDataStorage {
            made_by_orch: Some(MadeDataKind::Directory),
            existing_directory: true,
            reason: None,
        },
    );
    sqlx::query("UPDATE machines SET facts=$2 WHERE id=$1").bind(machine).bind(json!({"policy":policy,"environment_descriptor":descriptor,"storages":[{"name":"default","default":true,"capacity_gb":100,"free_gb":80,"private_views":false}]}))
        .execute(&mut *conn).await.unwrap();
    let principal = orch_authz::Principal::load(
        &mut conn,
        orch_authz::Caller::session(c.viewer.id()),
        orch_types::Time::new(world.now()),
    )
    .await
    .unwrap();
    let volume = orch_authz::resources::may_add_location(
        &mut conn,
        &principal,
        &orch_authz::resources::LocationAsk {
            mount: mount.base.id,
            machine,
            kind: orch_authz::resources::LocationKind::Volume,
            storage: None,
        },
        &orch_authz::usable::Policies::default(),
    )
    .await
    .unwrap();
    assert!(matches!(volume, orch_authz::resources::Locating::Allowed));
    drop(conn);
    let choices = world
        .app
        .call(MachineMountList, &[&machine])
        .token(&c.viewer.token)
        .send()
        .await
        .ok();
    let choice = choices
        .available
        .iter()
        .find(|choice| choice.mount.id == mount.base.id)
        .unwrap();
    assert!(
        choice.path && choice.default_storage,
        "both choices follow facts on the very same machine"
    );
    let refused = world
        .app
        .call(MountLocationSet, &[&mount.base.id, &machine])
        .token(&c.viewer.token)
        .body(&MountLocationSetBody::Path {
            source: "/outside/data".into(),
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
    assert!(refused.extra["errors"].to_string().contains("source"));
    let mut conn = world.conn().await;
    assert!(
        rows::locations(&mut conn, &[mount.base.id])
            .await
            .unwrap()
            .is_empty()
    );
    drop(conn);
    let located = world
        .app
        .call(MountLocationSet, &[&mount.base.id, &machine])
        .token(&c.viewer.token)
        .body(&MountLocationSetBody::Path {
            source: "/srv/work/data".into(),
        })
        .send()
        .await
        .ok();
    assert_eq!(
        located.locations[0].source.as_deref(),
        Some("/srv/work/data")
    );
    let denied = world
        .app
        .call(MountLocationSet, &[&mount.base.id, &machine])
        .token(&c.stranger.token)
        .body(&MountLocationSetBody::Path {
            source: "/srv/work/other".into(),
        })
        .send()
        .await
        .problem();
    assert_eq!(denied.code, ErrorCode::AuthzDenied);
}

#[tokio::test]
async fn a_hosts_mount_projection_masks_sources_and_hidden_environments_and_matches_reported_sizes_to_the_exact_location()
 {
    use orch_access_contract::permissions::ResourceKind;
    use orch_db::queries::resources::{access_fixtures as fixtures, mounts as rows};
    let world = World::new().await;
    let c = company(&world).await;
    let visible = named_mount(&world, &c.viewer, c.acme, "host-data").await;
    let _available = named_mount(&world, &c.viewer, c.acme, "host-addition").await;
    let mut conn = world.conn().await;
    let host =
        fixtures::insert_resource(&mut conn, "host", Some(c.admin.id()), Some(c.acme), "Node")
            .await
            .unwrap();
    let machine: Id = sqlx::query_scalar("SELECT machine_id FROM hosts WHERE id=$1")
        .bind(host)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE hosts SET storages=$2 WHERE id=$1").bind(host)
        .bind(json!([{"name":"fast","default":true,"capacity_gb":100,"free_gb":90,"private_views":true}])).execute(&mut *conn).await.unwrap();
    sqlx::query("UPDATE hosts SET drivers=$2 WHERE id=$1").bind(host).bind(json!({"docker":{"data_storage":{"made_by_orch":"volume","existing_directory":true,"reason":null}}})).execute(&mut *conn).await.unwrap();
    rows::set_location(
        &mut conn,
        &rows::LocationRow {
            mount_id: visible.base.id,
            machine_id: machine,
            host_id: None,
            host_name: None,
            kind: "volume".into(),
            source: "owned-volume".into(),
            storage: Some("fast".into()),
            data_kind: None,
            created_by: c.admin.id(),
            created_at: world.now(),
        },
    )
    .await
    .unwrap();
    rows::record_sizes(&mut conn, host, &json!([{"mount_id":visible.base.id,"kind":"volume","source":"owned-volume","storage":"fast","bytes":0}])).await.unwrap();
    drop(conn);
    own_page(&world, ResourceKind::Host, host, c.admin.id()).await;
    name_person(&world, "host", host, "host.view", c.viewer.id()).await;
    let attached = attached_on_host(
        &world,
        host,
        c.acme,
        c.viewer.id(),
        "Visible environment",
        visible.base.id,
    )
    .await;
    let _hidden = attached_on_host(
        &world,
        host,
        c.stranger.person.personal_org,
        c.stranger.id(),
        "Private environment",
        visible.base.id,
    )
    .await;
    let page = world
        .app
        .call(MachineMountList, &[&machine])
        .token(&c.viewer.token)
        .send()
        .await
        .ok();
    assert_eq!(page.locations.len(), 1);
    let location = &page.locations[0];
    let card = world
        .app
        .call(MountGet, &[&visible.base.id])
        .token(&c.viewer.token)
        .send()
        .await
        .ok();
    assert_eq!(card.attached_environments, Some(2));
    assert_eq!(card.locations[0].measured_bytes, Some(0));
    assert_eq!(location.storage.as_deref(), Some("fast"));
    assert_eq!(
        location.size_bytes,
        Some(0),
        "reported zero is an empty volume, not unknown"
    );
    assert!(!location.can_remove && location.source.is_none());
    assert_eq!(
        location
            .environments
            .iter()
            .map(|environment| environment.id)
            .collect::<Vec<_>>(),
        vec![attached]
    );
    assert!(
        page.available.is_empty(),
        "host.view alone never offers a new location"
    );
    assert!(
        !serde_json::to_string(&page)
            .unwrap()
            .contains("Private environment")
    );
    assert_eq!(
        world
            .app
            .call(MachineMountList, &[&machine])
            .token(&c.stranger.token)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::NotFound
    );
    name_person(&world, "host", host, "host.use", c.viewer.id()).await;
    let still_read = world
        .app
        .call(MachineMountList, &[&machine])
        .token(&c.viewer.token)
        .send()
        .await
        .ok();
    assert!(
        !still_read.locations[0].can_remove && still_read.available.is_empty(),
        "Use does not grant management of the machine"
    );
    name_person(&world, "host", host, "host.manage", c.viewer.id()).await;
    let page = world
        .app
        .call(MachineMountList, &[&machine])
        .token(&c.viewer.token)
        .send()
        .await
        .ok();
    assert!(page.locations[0].can_remove);
    assert_eq!(page.locations[0].source.as_deref(), Some("owned-volume"));
    assert_eq!(page.available[0].storages, vec!["fast"]);
    assert!(page.available[0].path && page.available[0].default_storage);
    let mut conn = world.conn().await;
    sqlx::query("UPDATE mount_locations SET source='replacement-volume' WHERE mount_id=$1 AND machine_id=$2")
        .bind(visible.base.id).bind(machine).execute(&mut *conn).await.unwrap();
    drop(conn);
    let page = world
        .app
        .call(MachineMountList, &[&machine])
        .token(&c.viewer.token)
        .send()
        .await
        .ok();
    assert_eq!(
        page.locations[0].size_bytes, None,
        "an old source's size must not describe a new location"
    );
}

#[tokio::test]
async fn a_readable_offline_hosts_location_is_named_and_is_not_reported_as_absent() {
    use orch_access_contract::permissions::ResourceKind;
    use orch_db::queries::resources::{access_fixtures as fixtures, mounts as rows};
    let world = World::new().await;
    let c = company(&world).await;
    let mount = named_mount(&world, &c.viewer, c.acme, "offline-notes").await;
    assert!(mount.locations.is_empty());
    assert_eq!(mount.locations_hidden, Some(0));
    let mut conn = world.conn().await;
    let host = fixtures::insert_resource(
        &mut conn,
        "host",
        Some(c.viewer.id()),
        Some(c.acme),
        "Offline storage host",
    )
    .await
    .unwrap();
    let machine: Id = sqlx::query_scalar("SELECT machine_id FROM hosts WHERE id=$1")
        .bind(host)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE machines SET online=false WHERE id=$1")
        .bind(machine)
        .execute(&mut *conn)
        .await
        .unwrap();
    rows::set_location(
        &mut conn,
        &rows::LocationRow {
            mount_id: mount.base.id,
            machine_id: machine,
            host_id: None,
            host_name: None,
            kind: "path".into(),
            source: "/srv/notes".into(),
            storage: None,
            data_kind: None,
            created_by: c.viewer.id(),
            created_at: world.now(),
        },
    )
    .await
    .unwrap();
    drop(conn);
    own_page(&world, ResourceKind::Host, host, c.viewer.id()).await;
    let read = world
        .app
        .call(MountGet, &[&mount.base.id])
        .token(&c.viewer.token)
        .send()
        .await
        .ok();
    assert_eq!(read.locations.len(), 1);
    assert_eq!(read.locations_hidden, Some(0));
    assert!(!read.locations[0].machine.online);
    assert_eq!(
        read.locations[0].host.as_ref().unwrap().name,
        "Offline storage host"
    );
}

#[tokio::test]
async fn a_seed_read_only_mount_creates_its_owners_page_and_advances_authority() {
    let world = World::new().await;
    let c = company(&world).await;
    let before = {
        let mut conn = world.conn().await;
        orch_db::queries::access::epoch(&mut conn).await.unwrap()
    };
    // The development seed leaves access out: the new page follows its home's defaults.
    let body = MountCreateBody {
        name: "shared-notes".into(),
        description: Some("world2: a host directory, read-only".into()),
        home_id: Some(c.acme),
        target: "/mnt/notes".into(),
        read_only: Some(true),
        default_fate: None,
        access: None,
    };
    let reply = world
        .app
        .call(MountCreate, &[])
        .token(&c.admin.token)
        .body(&body)
        .send()
        .await;
    assert_eq!(reply.status, http::StatusCode::OK, "{:?}", reply.body);
    let made = reply.ok();
    assert_eq!(made.base.name, "shared-notes");
    assert_eq!(made.target, "/mnt/notes");
    assert!(made.read_only && made.locations.is_empty());
    assert_eq!(
        made.owner.as_ref().map(|owner| owner.id),
        Some(c.admin.id())
    );
    assert!(
        made.access
            .permissions
            .iter()
            .any(|permission| permission == "mount.manage")
    );
    let after = {
        let mut conn = world.conn().await;
        orch_db::queries::access::epoch(&mut conn).await.unwrap()
    };
    assert!(
        after > before,
        "creating its ownership page must still invalidate old authority decisions"
    );
    let read = world
        .app
        .call(MountGet, &[&made.base.id])
        .token(&c.admin.token)
        .send()
        .await
        .ok();
    assert_eq!(
        read, made,
        "the committed mount and its access are readable by its maker"
    );
}

#[tokio::test]
async fn a_mount_is_created_listed_changed_and_removed_with_its_current_revision() {
    let world = World::new().await;
    let c = company(&world).await;
    let made = world
        .app
        .call(MountCreate, &[])
        .token(&c.viewer.token)
        .body(&create_body(c.acme))
        .send()
        .await
        .ok();
    assert_eq!(made.owner.as_ref().unwrap().id, c.viewer.id());
    assert!(made.read_only && made.locations.is_empty());
    let read = world
        .app
        .call(MountGet, &[&made.base.id])
        .token(&c.viewer.token)
        .send()
        .await
        .ok();
    assert_eq!(read, made);
    let listed = world
        .app
        .call(MountList, &[])
        .token(&c.viewer.token)
        .query(&MountListQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(
        listed
            .into_iter()
            .filter_map(orch_api_contract::common::Listed::into_visible)
            .collect::<Vec<_>>(),
        vec![made.clone()]
    );
    let stranger = world
        .app
        .call(MountGet, &[&made.base.id])
        .token(&c.stranger.token)
        .send()
        .await
        .problem();
    assert_eq!(stranger.code, ErrorCode::AuthzDenied);
    let change = MountUpdateBody {
        name: None,
        description: None,
        target: Some("/mnt/data".into()),
        read_only: None,
        enabled: None,
        default_fate: None,
        expected_revision: made.revision,
    };
    let denied = world
        .app
        .call(MountUpdate, &[&made.base.id])
        .token(&c.stranger.token)
        .body(&change)
        .send()
        .await
        .problem();
    assert_eq!(denied.code, ErrorCode::AuthzDenied);
    let changed = world
        .app
        .call(MountUpdate, &[&made.base.id])
        .token(&c.viewer.token)
        .body(&change)
        .send()
        .await
        .ok();
    assert_eq!(changed.target, "/mnt/data");
    assert_eq!(changed.revision, made.revision + 1);
    let stale = world
        .app
        .call(MountUpdate, &[&made.base.id])
        .token(&c.viewer.token)
        .body(&change)
        .send()
        .await
        .problem();
    assert_eq!(stale.code, ErrorCode::ConflictStaleRevision);
    assert_eq!(stale.extra["current"]["target"], "/mnt/data");
    world
        .app
        .call(MountDelete, &[&made.base.id])
        .token(&c.viewer.token)
        .send()
        .await
        .ok();
    assert_eq!(
        world
            .app
            .call(MountGet, &[&made.base.id])
            .token(&c.viewer.token)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::NotFound
    );
}

#[tokio::test]
async fn a_person_adds_only_their_own_machine_location_and_other_readers_do_not_see_its_path() {
    let world = World::new().await;
    let c = company(&world).await;
    let made = world
        .app
        .call(MountCreate, &[])
        .token(&c.viewer.token)
        .body(&create_body(c.acme))
        .send()
        .await
        .ok();
    let machine = orch_db::queries::resources::access_fixtures::insert_persons_machine(
        &mut *world.conn().await,
        c.viewer.id(),
        "laptop",
    )
    .await
    .unwrap();
    name_person(&world, "mount", made.base.id, "mount.use", c.stranger.id()).await;
    let policy = orch_link_contract::policy::LocalPolicy {
        environment_roots: vec!["/srv".into()],
        ..Default::default()
    };
    sqlx::query("UPDATE machines SET facts=jsonb_build_object('policy',$2::jsonb) WHERE id=$1")
        .bind(machine)
        .bind(json!(policy))
        .execute(&mut *world.conn().await)
        .await
        .unwrap();
    let body = MountLocationSetBody::Path {
        source: "/srv/data".into(),
    };
    let refused = world
        .app
        .call(MountLocationSet, &[&made.base.id, &machine])
        .token(&c.stranger.token)
        .body(&body)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    let located = world
        .app
        .call(MountLocationSet, &[&made.base.id, &machine])
        .token(&c.viewer.token)
        .body(&body)
        .send()
        .await
        .ok();
    assert_eq!(located.locations[0].source.as_deref(), Some("/srv/data"));
    assert!(located.locations[0].can_remove);
    let other = world
        .app
        .call(MountGet, &[&made.base.id])
        .token(&c.stranger.token)
        .send()
        .await
        .ok();
    assert!(
        other.locations.is_empty(),
        "a mount's reader does not learn a private machine or path"
    );
    assert_eq!(
        other.locations_hidden,
        Some(1),
        "a concealed location is not described as unconfigured"
    );
    let hidden = serde_json::to_string(&other).unwrap();
    assert!(!hidden.contains("/srv/data") && !hidden.contains("laptop"));
    name_person(&world, "machine", machine, "machine.view", c.stranger.id()).await;
    let shared = world
        .app
        .call(MountGet, &[&made.base.id])
        .token(&c.stranger.token)
        .send()
        .await
        .ok();
    assert_eq!(shared.locations.len(), 1);
    assert_eq!(shared.locations_hidden, Some(0));
    assert_eq!(shared.locations[0].machine.name, "laptop");
    assert_eq!(
        shared.locations[0].source, None,
        "machine View reveals its name, not its private source path"
    );
    assert!(!shared.locations[0].can_remove);
    let escaped = MountLocationSetBody::Path {
        source: "/srv/../etc".into(),
    };
    assert_eq!(
        world
            .app
            .call(MountLocationSet, &[&made.base.id, &machine])
            .token(&c.viewer.token)
            .body(&escaped)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::RequestInvalid
    );
    world
        .app
        .call(MountLocationDelete, &[&made.base.id, &machine])
        .token(&c.viewer.token)
        .query(&MountLocationDeleteQuery::default())
        .send()
        .await
        .ok();
    let read = world
        .app
        .call(MountGet, &[&made.base.id])
        .token(&c.viewer.token)
        .send()
        .await
        .ok();
    assert!(read.locations.is_empty());
}

#[tokio::test]
async fn an_enabled_mount_says_where_it_is_used_and_deletion_changes_nothing() {
    let world = World::new().await;
    let c = company(&world).await;
    let made = world
        .app
        .call(MountCreate, &[])
        .token(&c.viewer.token)
        .body(&create_body(c.acme))
        .send()
        .await
        .ok();
    world
        .set_at(
            orch_settings::Target::Node(c.acme),
            "env.mounts",
            json!({made.base.id.to_string(): "private"}),
        )
        .await;
    let used = world
        .app
        .call(MountUsesGet, &[&made.base.id])
        .token(&c.viewer.token)
        .send()
        .await
        .ok();
    assert_eq!(used.scopes.len(), 1);
    assert_eq!(used.scopes[0].node.id, c.acme);
    assert_eq!(
        used.scopes[0].value,
        MountScopeValue::Mode(MountScopeMode::Private)
    );
    let refused = world
        .app
        .call(MountDelete, &[&made.base.id])
        .token(&c.viewer.token)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::ResourceInUse);
    assert_eq!(
        refused.extra["uses"]["scopes"][0]["node"]["id"],
        json!(c.acme)
    );
    assert_eq!(
        world
            .app
            .call(MountGet, &[&made.base.id])
            .token(&c.viewer.token)
            .send()
            .await
            .ok()
            .revision,
        made.revision
    );
    assert_eq!(
        world
            .app
            .call(MountCreate, &[])
            .token(&c.stranger.token)
            .body(&create_body(c.acme))
            .send()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
}

#[tokio::test]
async fn a_mounts_manager_sets_what_plans_preselect_and_says_what_a_locations_data_is() {
    let world = World::new().await;
    let c = company(&world).await;
    let made = world
        .app
        .call(MountCreate, &[])
        .token(&c.viewer.token)
        .body(&create_body(c.acme))
        .send()
        .await
        .ok();
    assert_eq!(
        made.default_fate,
        MountFate::AsIs,
        "as shipped, a plan mounts it as it is"
    );
    let copy = MountUpdateBody {
        name: None,
        description: None,
        target: None,
        read_only: None,
        enabled: None,
        default_fate: Some(MountFate::Copy),
        expected_revision: made.revision,
    };
    let changed = world
        .app
        .call(MountUpdate, &[&made.base.id])
        .token(&c.viewer.token)
        .body(&copy)
        .send()
        .await
        .ok();
    assert_eq!(changed.default_fate, MountFate::Copy);
    assert_eq!(changed.revision, made.revision + 1);
    let machine = orch_db::queries::resources::access_fixtures::insert_persons_machine(
        &mut *world.conn().await,
        c.viewer.id(),
        "laptop",
    )
    .await
    .unwrap();
    let policy = orch_link_contract::policy::LocalPolicy {
        environment_roots: vec!["/srv".into()],
        ..Default::default()
    };
    sqlx::query("UPDATE machines SET facts=jsonb_build_object('policy',$2::jsonb) WHERE id=$1")
        .bind(machine)
        .bind(json!(policy))
        .execute(&mut *world.conn().await)
        .await
        .unwrap();
    let located = world
        .app
        .call(MountLocationSet, &[&made.base.id, &machine])
        .token(&c.viewer.token)
        .body(&MountLocationSetBody::Path {
            source: "/srv/data".into(),
        })
        .send()
        .await
        .ok();
    assert_eq!(located.locations[0].data_kind, None, "nobody said yet");
    name_person(&world, "mount", made.base.id, "mount.use", c.stranger.id()).await;
    let synced = MountLocationUpdateBody {
        data_kind: Some(MountDataKind::Synced),
    };
    let refused = world
        .app
        .call(MountLocationUpdate, &[&made.base.id, &machine])
        .token(&c.stranger.token)
        .body(&synced)
        .send()
        .await
        .problem();
    assert_eq!(
        refused.code,
        ErrorCode::AuthzDenied,
        "using a mount is not managing it"
    );
    let described = world
        .app
        .call(MountLocationUpdate, &[&made.base.id, &machine])
        .token(&c.viewer.token)
        .body(&synced)
        .send()
        .await
        .ok();
    assert_eq!(
        described.locations[0].data_kind,
        Some(MountDataKind::Synced)
    );
    assert_eq!(
        described.locations[0].source.as_deref(),
        Some("/srv/data"),
        "the hint changes nothing about where the data is"
    );
    let read = world
        .app
        .call(MountGet, &[&made.base.id])
        .token(&c.viewer.token)
        .send()
        .await
        .ok();
    assert_eq!(read.locations[0].data_kind, Some(MountDataKind::Synced));
    let cleared = world
        .app
        .call(MountLocationUpdate, &[&made.base.id, &machine])
        .token(&c.viewer.token)
        .body(&MountLocationUpdateBody { data_kind: None })
        .send()
        .await
        .ok();
    assert_eq!(cleared.locations[0].data_kind, None);
    let elsewhere = orch_types::new_id();
    assert_eq!(
        world
            .app
            .call(MountLocationUpdate, &[&made.base.id, &elsewhere])
            .token(&c.viewer.token)
            .body(&synced)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::NotFound,
        "a machine without a location has nothing to describe"
    );
}

#[tokio::test]
async fn data_a_copy_displaced_is_listed_until_whoever_may_remove_that_location_purges_it() {
    use orch_db::queries::resources::mounts as rows;
    let world = World::new().await;
    let c = company(&world).await;
    let made = world
        .app
        .call(MountCreate, &[])
        .token(&c.viewer.token)
        .body(&create_body(c.acme))
        .send()
        .await
        .ok();
    let machine = orch_db::queries::resources::access_fixtures::insert_persons_machine(
        &mut *world.conn().await,
        c.viewer.id(),
        "laptop",
    )
    .await
    .unwrap();
    let policy = orch_link_contract::policy::LocalPolicy {
        environment_roots: vec!["/srv".into()],
        ..Default::default()
    };
    sqlx::query("UPDATE machines SET facts=jsonb_build_object('policy',$2::jsonb) WHERE id=$1")
        .bind(machine)
        .bind(json!(policy))
        .execute(&mut *world.conn().await)
        .await
        .unwrap();
    let kept = orch_types::new_id();
    rows::keep(
        &mut *world.conn().await,
        &rows::KeptRow {
            id: kept,
            mount_id: made.base.id,
            machine_id: machine,
            kind: "path".into(),
            source: "/srv/old-data".into(),
            storage: None,
            data_kind: Some("synced".into()),
            file_count: Some(12),
            total_bytes: Some(4096),
            job_id: None,
            kept_by: c.viewer.id(),
            kept_at: world.now(),
        },
    )
    .await
    .unwrap();
    let read = world
        .app
        .call(MountGet, &[&made.base.id])
        .token(&c.viewer.token)
        .send()
        .await
        .ok();
    assert_eq!(read.kept.len(), 1);
    assert_eq!(read.kept[0].source.as_deref(), Some("/srv/old-data"));
    assert_eq!(read.kept[0].data_kind, Some(MountDataKind::Synced));
    assert_eq!(
        (read.kept[0].file_count, read.kept[0].total_bytes),
        (Some(12), Some(4096))
    );
    assert!(read.kept[0].can_purge);
    name_person(&world, "mount", made.base.id, "mount.view", c.stranger.id()).await;
    let refused = world
        .app
        .call(MountKeptPurge, &[&made.base.id, &kept])
        .token(&c.stranger.token)
        .query(&MountLocationDeleteQuery::default())
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    let other = world
        .app
        .call(MountGet, &[&made.base.id])
        .token(&c.stranger.token)
        .send()
        .await
        .ok();
    assert!(
        other.kept.is_empty(),
        "kept data on a machine the reader may not see is not shown"
    );
    world
        .app
        .call(MountKeptPurge, &[&made.base.id, &kept])
        .token(&c.viewer.token)
        .query(&MountLocationDeleteQuery::default())
        .send()
        .await
        .ok();
    let after = world
        .app
        .call(MountGet, &[&made.base.id])
        .token(&c.viewer.token)
        .send()
        .await
        .ok();
    assert!(
        after.kept.is_empty(),
        "a kept directory is forgotten, and only that"
    );
}
