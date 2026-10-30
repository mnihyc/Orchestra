//! Hosts over HTTP: listing what the caller may use or manage, reading, changing by the managers, and the token with which a host enrolls.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod resources;
mod support;

use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::ResourceKind;
use orch_api_contract::access::NewResourceAccess;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::environment::WhenFull;
use orch_api_contract::resources::{
    Host, HostEnrollmentTokenCreate, HostEnrollmentTokenCreateBody, HostGet, HostList,
    HostListQuery, HostUpdate, HostUpdateBody,
};
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::queries::resources::access_fixtures as fixtures;
use orch_types::{Id, Time};
use places::{Who, admin, make_project, missing_permissions};
use resources::{Company, company, name_person};
use serde_json::json;
use support::{Statements, World};

/// A host named `name` at home in `home` (none: the system's), first owned by `owner`, as the
/// node of a machine would have reported it.
async fn a_host(world: &World, owner: &Who, home: Option<Id>, name: &str) -> Id {
    let mut conn = world.conn().await;
    let id = fixtures::insert_resource(&mut conn, "host", Some(owner.id()), home, name)
        .await
        .unwrap();
    start_resource(
        &mut conn,
        ObjectRef {
            kind: ResourceKind::Host.into(),
            id,
        },
        owner.id(),
        NewAccess::Defaults,
        Time::new(world.now()),
    )
    .await
    .unwrap();
    sqlx::query(
        "UPDATE hosts SET labels = ARRAY['gpu'],
                drivers = '{\"docker\": {\"name\": \"docker\", \"title\": \"Docker\",
                            \"spec_schemas\": {\"environment\": {}, \"provider\": {}},
                            \"runtime_classes\": [\"runc\"]}}'::jsonb,
                capacity = '{\"cpus\": 8, \"memory_mb\": 16384, \"disk_gb\": 500}'::jsonb,
                storages = '[{\"name\": \"fast\", \"default\": true, \"capacity_gb\": 500, \"free_gb\": 300,
                              \"private_views\": false}]'::jsonb
          WHERE id = $1",
    )
    .bind(id)
    .execute(&mut *conn)
    .await
    .unwrap();
    id
}

async fn read(world: &World, by: &Who, id: Id) -> Host {
    world
        .app
        .call(HostGet, &[&id])
        .token(&by.token)
        .send()
        .await
        .ok()
}

#[tokio::test]
async fn storage_is_one_authorized_report_with_filtered_totals_unknowns_and_host_policy() {
    use orch_api_contract::storage::{StorageGet, StorageKind, StorageQuery};
    use orch_driver_contract::resources::{Resource, ResourceInventory, ResourceKind, ResourceRef};
    let world = World::new().await;
    let c = company(&world).await;
    let host = a_host(&world, &c.boss, Some(c.acme), "Storage host").await;
    name_person(&world, "host", host, "host.view", c.viewer.id()).await;
    let used = Time::new(world.now() - chrono::Duration::days(8));
    let inventory = ResourceInventory {
        observed_at: Time::new(world.now()),
        unavailable: Vec::new(),
        resources: [
            (ResourceKind::Image, "image", Some(4)),
            (ResourceKind::ToolVersion, "tool", None),
        ]
        .into_iter()
        .map(|(kind, id, bytes)| Resource {
            resource: ResourceRef {
                kind,
                id: id.into(),
            },
            name: id.into(),
            size_bytes: bytes,
            labels: Default::default(),
            depends_on: Vec::new(),
            aliases: Vec::new(),
            last_used_at: Some(used),
            busy: false,
        })
        .collect(),
    };
    sqlx::query("UPDATE hosts SET resource_inventory=$2 WHERE id=$1")
        .bind(host)
        .bind(json!(inventory))
        .execute(&world.db.pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE machines SET online=true WHERE id=(SELECT machine_id FROM hosts WHERE id=$1)",
    )
    .bind(host)
    .execute(&world.db.pool)
    .await
    .unwrap();
    let query = StorageQuery {
        scope: Some(format!("host:{host}")),
        ..Default::default()
    };
    let report = world
        .app
        .call(StorageGet, &[])
        .token(&c.boss.token)
        .query(&query)
        .send()
        .await
        .ok();
    assert_eq!(report.totals.items, 2);
    assert_eq!(report.totals.bytes, 4);
    assert_eq!(
        report.totals.unknown, 1,
        "an unmeasured tool never becomes zero bytes"
    );
    assert!(report.items.iter().all(|row| row.until.is_some()));
    assert!(
        report
            .items
            .iter()
            .all(|row| row.facts.as_ref().is_some_and(|facts| facts.uses.is_none())),
        "physical artifacts have no popularity figure"
    );
    let filtered = StorageQuery {
        kind: Some(StorageKind::Image),
        ..query.clone()
    };
    let report = world
        .app
        .call(StorageGet, &[])
        .token(&c.boss.token)
        .query(&filtered)
        .send()
        .await
        .ok();
    assert_eq!(
        (
            report.totals.items,
            report.totals.bytes,
            report.totals.unknown
        ),
        (1, 4, 0)
    );
    assert_eq!(report.items.len(), 1);
    sqlx::query("UPDATE hosts SET cleanup_keep_days=0 WHERE id=$1")
        .bind(host)
        .execute(&world.db.pool)
        .await
        .unwrap();
    let report = world
        .app
        .call(StorageGet, &[])
        .token(&c.boss.token)
        .query(&query)
        .send()
        .await
        .ok();
    assert!(
        report.items.iter().all(|row| row.until.is_none()),
        "zero follows the same Off policy as the host setting page"
    );
    assert_eq!(
        world
            .app
            .call(StorageGet, &[])
            .token(&c.viewer.token)
            .query(&query)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::NotFound
    );
    let empty = world
        .app
        .call(StorageGet, &[])
        .token(&c.stranger.token)
        .query(&StorageQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(empty.totals.items, 0);
    assert!(
        empty.unavailable.is_empty(),
        "another person's hosts are not disclosed as unavailable sources"
    );
}

#[tokio::test]
async fn host_cleanup_follows_the_system_accepts_zero_and_cannot_override_a_system_lock() {
    use orch_api_contract::settings::{
        HostSettingDelete, HostSettingList, HostSettingSet, NodeSettingDeleteQuery,
        NodeSettingSetBody, SettingScope,
    };
    use orch_db::queries::settings::{Stored, put};
    let world = World::new().await;
    let c = company(&world).await;
    let host = a_host(&world, &c.boss, Some(c.acme), "Cache host").await;
    name_person(&world, "host", host, "host.view", c.viewer.id()).await;
    let rows = world
        .app
        .call(HostSettingList, &[&host])
        .token(&c.viewer.token)
        .send()
        .await
        .ok();
    let initial = rows
        .iter()
        .find(|row| row.key == "cleanup.unused_keep_days")
        .unwrap();
    assert_eq!(initial.value, Some(json!(7)));
    assert!(!initial.writable);
    let value = NodeSettingSetBody {
        value: json!(0),
        locked: None,
        expected_revision: Some(0),
    };
    let denied = world
        .app
        .call(HostSettingSet, &[&host, &"cleanup.unused_keep_days"])
        .token(&c.viewer.token)
        .body(&value)
        .send()
        .await
        .problem();
    assert_eq!(denied.code, ErrorCode::AuthzDenied);
    let off = world
        .app
        .call(HostSettingSet, &[&host, &"cleanup.unused_keep_days"])
        .token(&c.boss.token)
        .body(&value)
        .send()
        .await
        .ok();
    assert_eq!(off.value, Some(json!(0)));
    assert_eq!(off.source.scope, SettingScope::HostOwn);
    let revision = off.own.as_ref().unwrap().revision;
    let stale = world
        .app
        .call(HostSettingSet, &[&host, &"cleanup.unused_keep_days"])
        .token(&c.boss.token)
        .body(&value)
        .send()
        .await
        .problem();
    assert_eq!(stale.code, ErrorCode::ConflictStaleRevision);
    let bad = NodeSettingSetBody {
        value: json!(-1),
        locked: None,
        expected_revision: Some(revision),
    };
    assert_eq!(
        world
            .app
            .call(HostSettingSet, &[&host, &"cleanup.unused_keep_days"])
            .token(&c.boss.token)
            .body(&bad)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::RequestInvalid
    );
    let root = admin(&world, "maintenance-admin").await;
    let system_value = json!(14);
    let mut conn = world.conn().await;
    put(
        &mut conn,
        None,
        "cleanup.unused_keep_days",
        &Stored {
            value: &system_value,
            locked: true,
            by: Some(root.id()),
            at: world.now(),
        },
        None,
    )
    .await
    .unwrap();
    drop(conn);
    let locked = world
        .app
        .call(HostSettingList, &[&host])
        .token(&c.boss.token)
        .send()
        .await
        .ok()
        .pop()
        .unwrap();
    assert_eq!(locked.value, Some(json!(14)));
    assert_eq!(locked.own.as_ref().unwrap().value, json!(0));
    assert!(!locked.writable);
    assert!(locked.locked_by.is_some());
    let clear = NodeSettingDeleteQuery {
        expected_revision: Some(revision),
    };
    assert_eq!(
        world
            .app
            .call(HostSettingDelete, &[&host, &"cleanup.unused_keep_days"])
            .token(&c.boss.token)
            .query(&clear)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::AccessLocked
    );
    let mut conn = world.conn().await;
    put(
        &mut conn,
        None,
        "cleanup.unused_keep_days",
        &Stored {
            value: &system_value,
            locked: false,
            by: Some(root.id()),
            at: world.now(),
        },
        None,
    )
    .await
    .unwrap();
    drop(conn);
    let restored = world
        .app
        .call(HostSettingDelete, &[&host, &"cleanup.unused_keep_days"])
        .token(&c.boss.token)
        .query(&clear)
        .send()
        .await
        .ok();
    assert_eq!(restored.value, Some(json!(14)));
    assert!(restored.own.is_none());
    assert_eq!(restored.source.scope, SettingScope::System);
    let lines: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE object_id=$1 AND details->>'key'='cleanup.unused_keep_days'").bind(host).fetch_one(&world.db.pool).await.unwrap();
    assert_eq!(
        lines, 2,
        "refusals and inherited changes add no host history line"
    );
}

#[tokio::test]
async fn host_choices_distinguish_measured_load_from_declared_memory() {
    let world = World::new().await;
    let c = company(&world).await;
    let id = a_host(&world, &c.boss, Some(c.acme), "Measured host").await;
    let before = read(&world, &c.boss, id).await;
    assert!(before.usage.is_none());
    assert_eq!(before.stopped_environments, Some(0));
    sqlx::query("UPDATE hosts SET load=$2,load_at=$3 WHERE id=$1")
        .bind(id)
        .bind(json!({"cpu_percent":12.5,"memory_used_mb":4096}))
        .bind(world.now())
        .execute(&world.db.pool)
        .await
        .unwrap();
    let after = read(&world, &c.boss, id).await;
    let measured = after.usage.unwrap();
    assert_eq!(measured.cpu_percent_milli, Some(12_500));
    assert_eq!(measured.memory_used_mb, Some(4096));
    assert_eq!(measured.observed_at, Time::new(world.now()));
    assert_eq!(after.declared.memory_mb, before.declared.memory_mb);
    assert_eq!(after.full, Some(false));
    let environment = {
        let mut conn = world.conn().await;
        fixtures::insert_environment(
            &mut conn,
            c.boss.id(),
            Some(c.acme),
            "Stopped workspace",
            "managed",
        )
        .await
        .unwrap()
    };
    sqlx::query("INSERT INTO runtimes(id,kind,host_id,unit_name,environment_id,driver,template_id,spec_hash,state)
        SELECT $1,'environment',$2,'stopped-workspace',e.id,'test',e.template_id,'\\x00'::bytea,'stopped'
        FROM environments e WHERE e.id=$3")
        .bind(orch_types::new_id()).bind(id).bind(environment).execute(&world.db.pool).await.unwrap();
    let with_stopped = read(&world, &c.boss, id).await;
    assert_eq!(with_stopped.stopped_environments, Some(1));
    assert_eq!(with_stopped.mine, 1);
    assert_eq!(with_stopped.running.environment, 0);
    assert_eq!(with_stopped.declared.memory_mb, before.declared.memory_mb);
}

async fn assert_list_measurements(world: &World, by: &Who, shown: &Host) {
    let listed = world
        .app
        .call(HostList, &[])
        .token(&by.token)
        .query(&HostListQuery::default())
        .send()
        .await
        .ok()
        .into_iter()
        .find_map(|entry| match entry {
            orch_api_contract::common::Listed::Visible(row)
                if row.content.base.id == shown.base.id =>
            {
                Some(row.content)
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(
        listed.capacity, shown.capacity,
        "the list and page use the same reported capacity"
    );
    assert_eq!(
        listed.declared, shown.declared,
        "both use the same native scheduler declarations"
    );
}

#[tokio::test]
async fn an_offline_host_keeps_its_reported_form_and_its_actual_disconnection_time() {
    use orch_api_contract::resources::{DriverInfo, DriverList};
    let world = World::new().await;
    let company = company(&world).await;
    // Owning counts inside an org: the host is at home in the company's.
    let host = a_host(
        &world,
        &company.boss,
        Some(company.acme),
        "Offline example host",
    )
    .await;
    let since = world.now();
    let schema = json!({"type":"object","properties":{"image":{"type":"string"}},
        "x-orch-examples":[{"id":"web","name":"Web","description":"A web workspace.","spec":{"image":"node:22-bookworm"}}]});
    let descriptor =
        json!({"name":"offline-example","version":"1","spec_schemas":{"environment":schema}});
    let mut conn = world.conn().await;
    sqlx::query(
        "UPDATE hosts SET drivers=jsonb_build_object('offline-example',$2::jsonb) WHERE id=$1",
    )
    .bind(host)
    .bind(&descriptor)
    .execute(&mut *conn)
    .await
    .unwrap();
    sqlx::query("UPDATE machines SET online=false,offline_since=$2 WHERE id=(SELECT machine_id FROM hosts WHERE id=$1)")
        .bind(host).bind(since).execute(&mut *conn).await.unwrap();
    drop(conn);
    let shown = read(&world, &company.boss, host).await;
    assert!(!shown.machine.online);
    assert_eq!(shown.offline_since, Some(Time::new(since)));
    let drivers: Vec<DriverInfo> = world
        .app
        .call(DriverList, &[])
        .token(&company.boss.token)
        .send()
        .await
        .ok();
    let cached = drivers
        .iter()
        .find(|driver| driver.name == "offline-example")
        .unwrap();
    assert_eq!(cached.descriptor, descriptor);
    sqlx::query("UPDATE machines SET offline_since=NULL WHERE id=(SELECT machine_id FROM hosts WHERE id=$1)")
        .bind(host).execute(&mut *world.conn().await).await.unwrap();
    assert_eq!(read(&world, &company.boss, host).await.offline_since, None);
}

#[tokio::test]
async fn a_host_reader_sees_owned_layer_bytes_even_when_the_image_has_no_running_unit() {
    let world = World::new().await;
    let company = company(&world).await;
    let owner = &company.boss;
    // Owning counts inside an org: the host is at home in the company's.
    let host = a_host(&world, owner, Some(company.acme), "Kept layers").await;
    let environment = orch_types::new_id();
    let capture = orch_types::new_id();
    sqlx::query("UPDATE hosts SET kept_images=$2 WHERE id=$1").bind(host).bind(json!({"epoch":7,"images":[{"system":"test","environment":environment,"generation":4,"capture":capture,"reference":format!("orch-keep-{environment}:4"),"bytes":768<<20,"total_bytes":900<<20,"layers":2}]})).execute(&mut *world.conn().await).await.unwrap();
    let shown = read(&world, owner, host).await;
    assert_eq!(shown.kept_layers.len(), 1);
    assert_eq!(shown.kept_layers[0].environment_id, environment);
    assert_eq!(shown.kept_layers[0].bytes, 768 << 20);
}

#[tokio::test]
async fn host_managers_see_safe_volume_only_retention_without_receiving_a_purge_right() {
    use orch_api_contract::environment::{
        EnvironmentPurge, EnvironmentPurgeQuery, EnvironmentRemovalPhase,
    };
    use orch_api_contract::nodes::JobState;
    let world = World::new().await;
    let c = company(&world).await;
    // Owning counts inside an org: the host is at home in the company's.
    let host = a_host(&world, &c.boss, Some(c.acme), "Retained volume host").await;
    let mut conn = world.conn().await;
    let environment = fixtures::insert_resource(
        &mut conn,
        "environment",
        Some(c.boss.id()),
        Some(c.acme),
        "Retained private environment",
    )
    .await
    .unwrap();
    start_resource(
        &mut conn,
        ObjectRef {
            kind: ResourceKind::Environment.into(),
            id: environment,
        },
        c.boss.id(),
        NewAccess::OnlyMe,
        Time::new(world.now()),
    )
    .await
    .unwrap();
    let purge_at = world.now() + chrono::Duration::days(7);
    let runtime = orch_types::new_id();
    let refs = json!([{"runtime_id":runtime,"environment_id":environment,"host_id":host,"driver":"test","role":"workspace","name":"private-volume-name-marker","storage":"fast"},
        {"runtime_id":runtime,"environment_id":environment,"host_id":host,"driver":"test","role":"home","name":"private-home-name-marker","storage":null}]);
    sqlx::query("UPDATE environments SET status='removed',deleted_at=$2,removed_keep_days=7,volumes_purge_at=$3,removal_phase='kept',removal_revision=17,retained_volumes=$4,retained_bytes=NULL WHERE id=$1")
        .bind(environment).bind(world.now()).bind(purge_at).bind(refs).execute(&mut *conn).await.unwrap();
    let job = orch_types::new_id();
    sqlx::query("INSERT INTO jobs(id,kind,environment_id,state,plan,progress,created_by) VALUES($1,'environment_restore',$2,'failed',$3,$4,$5)")
        .bind(job).bind(environment).bind(json!({"removal_revision":17,"private":"private-job-plan-marker"})).bind(json!({"phase":"create","percent":23})).bind(c.boss.id()).execute(&mut *conn).await.unwrap();
    drop(conn);
    name_person(&world, "host", host, "host.manage", c.stranger.id()).await;
    name_person(&world, "host", host, "host.view", c.viewer.id()).await;
    let shown = read(&world, &c.boss, host).await;
    assert!(
        shown.kept_layers.is_empty(),
        "volumes do not require a kept installed-file image"
    );
    let held = &shown.kept_data[0];
    assert_eq!(held.environment_id, environment);
    assert_eq!(held.name.as_deref(), Some("Retained private environment"));
    assert_eq!(held.phase, EnvironmentRemovalPhase::Kept);
    assert_eq!(held.bytes, None);
    assert_eq!(held.purge_at, Some(Time::new(purge_at)));
    assert!(held.storages.contains(&None) && held.storages.contains(&Some("fast".to_owned())));
    assert_eq!(held.job.as_ref().unwrap().id, job);
    assert_eq!(held.job.as_ref().unwrap().state, JobState::Failed);
    let encoded = serde_json::to_string(&shown.kept_data).unwrap();
    assert!(
        !encoded.contains("private-volume-name-marker")
            && !encoded.contains("private-home-name-marker")
            && !encoded.contains("private-job-plan-marker")
    );
    let manager = read(&world, &c.stranger, host).await;
    assert_eq!(manager.kept_data[0].name, None);
    assert!(
        manager.kept_data[0]
            .home
            .as_ref()
            .is_none_or(|home| home.redacted)
    );
    assert!(read(&world, &c.viewer, host).await.kept_data.is_empty());
    let refused = world
        .app
        .call(EnvironmentPurge, &[&environment])
        .token(&c.stranger.token)
        .query(&EnvironmentPurgeQuery {
            confirm: "Retained private environment".to_owned(),
            expected_revision: serde_json::from_value(json!(17)).unwrap(),
        })
        .send()
        .await;
    assert_eq!(
        refused.status, 403,
        "managing the host grants no early erasure of another home's files"
    );
}

async fn list(world: &World, by: &Who, query: &HostListQuery) -> Vec<String> {
    let mut names: Vec<String> = world
        .app
        .call(HostList, &[])
        .token(&by.token)
        .query(query)
        .send()
        .await
        .ok()
        .into_iter()
        .map(|host| match host {
            orch_api_contract::common::Listed::Hidden(row) => row.name,
            orch_api_contract::common::Listed::Visible(row) => row.content.base.name,
        })
        .collect();
    names.sort();
    names
}

fn change(revision: i64) -> HostUpdateBody {
    HostUpdateBody {
        name: None,
        labels: None,
        schedulable: None,
        when_full: None,
        ports: None,
        expected_revision: revision,
    }
}

#[tokio::test]
async fn a_host_is_read_with_what_it_reports_and_what_the_caller_holds_on_it() {
    let world = World::new().await;
    let c = company(&world).await;
    let id = a_host(&world, &c.boss, Some(c.acme), "Build box").await;
    let host = read(&world, &c.boss, id).await;
    assert_eq!(host.base.id, id);
    assert_eq!(host.base.name, "Build box");
    assert_eq!(host.base.address, "acme.boss.Build box");
    assert_eq!(host.owner.as_ref().unwrap().username, "boss");
    assert_eq!(host.org_id, Some(c.acme));
    assert_eq!(host.home.as_ref().unwrap().id, c.acme);
    assert_eq!(host.machine.name, "Build box");
    assert!(!host.machine.online);
    assert_eq!(host.labels, ["gpu"]);
    assert_eq!(host.drivers.len(), 1);
    assert_eq!(host.drivers[0].title, "Docker");
    assert_eq!(host.drivers[0].runtime_classes, ["runc"]);
    assert_eq!(
        (
            host.capacity.cpu_millis,
            host.capacity.memory_mb,
            host.capacity.disk_gb
        ),
        (8000, 16384, 500)
    );
    assert_eq!(host.storages.len(), 1);
    assert_eq!(host.storages[0].free_gb, 300);
    assert!(host.schedulable);
    assert_eq!(host.when_full.value, WhenFull::Warn);
    assert!(
        !host.when_full.own,
        "the system's answer, until the host has its own"
    );
    assert_eq!(
        (host.running.provider, host.running.environment, host.mine),
        (0, 0, 0)
    );
    assert!(host.revision > 0);
    assert!(host.access.permissions.iter().any(|p| p == "host.manage"));
    let nobody = world.app.call(HostGet, &[&id]).send().await.problem();
    assert_eq!(nobody.code, ErrorCode::AuthRequired);
}

#[tokio::test]
async fn a_host_is_read_by_whoever_holds_any_permission_on_it_and_by_nobody_else() {
    let world = World::new().await;
    let c = company(&world).await;
    let id = a_host(&world, &c.boss, Some(c.acme), "Build box").await;
    let refused = world
        .app
        .call(HostGet, &[&id])
        .token(&c.stranger.token)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(refused.extra["required"], "host.view");
    assert_eq!(missing_permissions(&refused), ["host.view"], "{refused:?}");
    name_person(&world, "host", id, "host.view", c.viewer.id()).await;
    let looked = read(&world, &c.viewer, id).await;
    assert_eq!(looked.access.permissions, ["host.view"]);
    // Only to look at: it is among the hosts they hold something on, with nothing to use.
    assert!(
        list(&world, &c.viewer, &HostListQuery::default())
            .await
            .contains(&"Build box".to_owned())
    );
    let unknown = world
        .app
        .call(HostGet, &[&orch_types::new_id()])
        .token(&c.boss.token)
        .send()
        .await
        .problem();
    assert_eq!(unknown.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn the_hosts_a_caller_may_use_or_manage_are_listed_and_narrow_to_an_org_with_the_systems() {
    let world = World::new().await;
    let c = company(&world).await;
    let root = admin(&world, "root").await;
    let own = a_host(&world, &c.boss, Some(c.acme), "Acme's").await;
    let systems = a_host(&world, &root, None, "Shared").await;
    let all = HostListQuery::default();
    assert_eq!(list(&world, &c.boss, &all).await, ["Acme's"]);
    // A system admin with As admin on holds View and Manage on every host natively.
    assert_eq!(list(&world, &root, &all).await, ["Acme's", "Shared"]);
    assert!(list(&world, &c.stranger, &all).await.is_empty());
    assert!(list(&world, &c.viewer, &all).await.is_empty());
    name_person(&world, "host", own, "host.use", c.viewer.id()).await;
    assert_eq!(list(&world, &c.viewer, &all).await, ["Acme's"]);
    // Whoever may use a host may read it: any permission on it is enough.
    assert_eq!(read(&world, &c.viewer, own).await.base.name, "Acme's");
    name_person(&world, "host", systems, "host.use", c.boss.id()).await;
    assert_eq!(list(&world, &c.boss, &all).await, ["Acme's", "Shared"]);
    let in_acme = HostListQuery {
        org_id: Some(c.acme),
    };
    assert_eq!(list(&world, &c.boss, &in_acme).await, ["Acme's", "Shared"]);
    let other = places::make_org(&world, &c.stranger, "Other", "other").await;
    let elsewhere = HostListQuery {
        org_id: Some(other.base.id),
    };
    assert_eq!(
        list(&world, &c.boss, &elsewhere).await,
        ["Shared"],
        "the system's are in every org, and nothing of Acme's is in another"
    );
    let nobody = world.app.call(HostList, &[]).send().await.problem();
    assert_eq!(nobody.code, ErrorCode::AuthRequired);
}

#[tokio::test]
async fn a_list_of_hosts_costs_the_same_statements_for_a_few_and_for_many() {
    let world = World::new().await;
    let c = company(&world).await;
    for index in 0..2 {
        a_host(&world, &c.boss, Some(c.acme), &format!("First {index}")).await;
    }
    async fn measure(world: &World, who: &Who) -> (usize, usize) {
        let call = world
            .app
            .call(HostList, &[])
            .token(&who.token)
            .query(&HostListQuery::default());
        let before = Statements::now();
        let listed = call.send().await.ok();
        (Statements::now() - before, listed.len())
    }
    let (few, few_count) = measure(&world, &c.boss).await;
    for index in 0..10 {
        a_host(&world, &c.boss, Some(c.acme), &format!("More {index}")).await;
    }
    let (many, many_count) = measure(&world, &c.boss).await;
    assert_eq!((few_count, many_count), (2, 12));
    assert_eq!(few, many, "{few} statements for 2 hosts, {many} for 12");
}

/// Puts a unit on a host: an environment of `owner`'s that declares `needs`, or the runtime of a
/// provider.
async fn run_on(
    world: &World,
    host: Id,
    owner: &Who,
    acme: Id,
    template: Id,
    needs: serde_json::Value,
) {
    let environment = orch_types::new_id();
    sqlx::query(
        "INSERT INTO environments (id, name, owner_id, org_id, home_node_id, placement, template_id, config)
         VALUES ($1, 'Unit', $2, $3, $3, 'managed', $4, $5)",
    )
    .bind(environment)
    .bind(owner.id())
    .bind(acme)
    .bind(template)
    .bind(json!({ "env.needs": needs }))
    .execute(&world.db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO runtimes (id, kind, host_id, unit_name, environment_id, driver, template_id, spec_hash, state)
         VALUES ($1, 'environment', $2, $3, $4, 'docker', $5, '\\x00'::bytea, 'running')",
    )
    .bind(orch_types::new_id())
    .bind(host)
    .bind(format!("unit-{environment}"))
    .bind(environment)
    .bind(template)
    .execute(&world.db.pool)
    .await
    .unwrap();
}

struct RuntimeBuilds;

struct NoReadBuilds(std::sync::Arc<std::sync::atomic::AtomicUsize>);

#[async_trait::async_trait]
impl orch_engine::hosting::HostServices for NoReadBuilds {
    async fn enrollment(
        &self,
        _: &mut orch_db::Conn,
        _: &orch_engine::hosting::EnrollmentAsk,
    ) -> Result<orch_engine::hosting::Enrollment, orch_engine::hosting::HostingError> {
        Err(orch_engine::hosting::HostingError::NotAttached)
    }
    async fn tools(
        &self,
        _: &mut orch_db::Conn,
        _: &str,
        _: &str,
        _: &[String],
    ) -> Result<Vec<orch_engine::hosting::ToolBuild>, orch_engine::hosting::HostingError> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Err(orch_engine::hosting::HostingError::Failed(
            "a read tried to refresh the build catalogue".into(),
        ))
    }
}

#[async_trait::async_trait]
impl orch_engine::hosting::HostServices for RuntimeBuilds {
    async fn enrollment(
        &self,
        _: &mut orch_db::Conn,
        _: &orch_engine::hosting::EnrollmentAsk,
    ) -> Result<orch_engine::hosting::Enrollment, orch_engine::hosting::HostingError> {
        Err(orch_engine::hosting::HostingError::NotAttached)
    }
    async fn tools(
        &self,
        _: &mut orch_db::Conn,
        _: &str,
        _: &str,
        harnesses: &[String],
    ) -> Result<Vec<orch_engine::hosting::ToolBuild>, orch_engine::hosting::HostingError> {
        Ok(std::iter::once("orch-connector".to_owned())
            .chain(harnesses.iter().cloned())
            .map(|name| orch_engine::hosting::ToolBuild {
                url: format!("https://builds.test/{name}"),
                name,
                version: "1.0.0".to_owned(),
                sha256: "0".repeat(64),
            })
            .collect())
    }
}

async fn a_current_unit(world: &World, c: &Company, host: Id, template: Id) -> (Id, Id) {
    run_on(world, host, &c.boss, c.acme, template, json!({})).await;
    let (runtime, environment): (Id, Id) = sqlx::query_as(
        "SELECT r.id,r.environment_id FROM runtimes r JOIN environments e ON e.id=r.environment_id
        WHERE r.host_id=$1 AND e.name='Unit'",
    )
    .bind(host)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    sqlx::query("UPDATE environments SET name=$2 WHERE id=$1")
        .bind(environment)
        .bind(format!("Unit {environment}"))
        .execute(&world.db.pool)
        .await
        .unwrap();
    let spec: serde_json::Value = sqlx::query_scalar("SELECT spec FROM env_templates WHERE id=$1")
        .bind(template)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    let resources = orch_engine::places::limits_of(&spec, None);
    let tools = vec![orch_engine::hosting::ToolBuild {
        name: "orch-connector".to_owned(),
        version: "1.0.0".to_owned(),
        url: "https://builds.test/orch-connector".to_owned(),
        sha256: "0".repeat(64),
    }];
    let hash = orch_engine::updates::definition_hash("docker", &spec, &resources, &tools);
    sqlx::query("UPDATE runtimes SET spec_hash=$2 WHERE id=$1")
        .bind(runtime)
        .bind(hash.as_bytes().as_slice())
        .execute(&world.db.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO jobs(id,kind,environment_id,state,plan) VALUES($1,'environment_provision',$2,'succeeded',$3)")
        .bind(orch_types::new_id()).bind(environment).bind(json!({"runtime":runtime,"tools":tools,
            "created_spec":{"spec":spec,"resources":resources,"mounts":[]}})).execute(&world.db.pool).await.unwrap();
    (environment, runtime)
}

#[tokio::test]
async fn host_fleet_environment_and_preflight_agree_on_updates_and_lists_read_together() {
    use orch_api_contract::admin::{AdminRuntimeList, AdminRuntimeListQuery};
    use orch_api_contract::environment::{
        EnvironmentGet, EnvironmentUpdatePreflight, UpdatePreflightBody,
    };
    use orch_api_contract::resources::{HostRuntimeList, HostRuntimeListQuery};
    let world = World::new().await;
    world
        .app
        .app
        .engine
        .attach_host_services(std::sync::Arc::new(RuntimeBuilds));
    let c = company(&world).await;
    let root = admin(&world, "runtime-reader").await;
    let host = a_host(&world, &c.boss, Some(c.acme), "Build").await;
    let template = fixtures::insert_resource(
        &mut *world.conn().await,
        "template",
        Some(c.boss.id()),
        Some(c.acme),
        "Tools",
    )
    .await
    .unwrap();
    sqlx::query(
        "UPDATE env_templates SET driver='docker',spec='{\"image\":\"image:old\"}' WHERE id=$1",
    )
    .bind(template)
    .execute(&world.db.pool)
    .await
    .unwrap();
    let (environment, runtime) = a_current_unit(&world, &c, host, template).await;
    a_current_unit(&world, &c, host, template).await;
    async fn count(
        world: &World,
        who: &Who,
        host: Id,
    ) -> (usize, Vec<orch_api_contract::admin::Runtime>) {
        let call = world
            .app
            .call(HostRuntimeList, &[&host])
            .token(&who.token)
            .query(&HostRuntimeListQuery::default());
        let before = Statements::now();
        let rows = call.send().await.ok();
        (Statements::now() - before, rows)
    }
    let (few, current) = count(&world, &c.boss, host).await;
    assert!(current.iter().all(|row| row.updates == 0));
    for _ in 0..10 {
        a_current_unit(&world, &c, host, template).await;
    }
    let (many, current) = count(&world, &c.boss, host).await;
    assert_eq!(current.len(), 12);
    assert_eq!(
        few, many,
        "runtime projection must not issue one read per row"
    );
    sqlx::query(
        "UPDATE env_templates SET spec='{\"image\":\"image:new\"}',revision=revision+1 WHERE id=$1",
    )
    .bind(template)
    .execute(&world.db.pool)
    .await
    .unwrap();
    let env = world
        .app
        .call(EnvironmentGet, &[&environment])
        .token(&c.boss.token)
        .send()
        .await
        .ok();
    let preflight = world
        .app
        .call(EnvironmentUpdatePreflight, &[&environment])
        .token(&c.boss.token)
        .body(&UpdatePreflightBody::default())
        .send()
        .await
        .ok();
    assert_eq!(env.updates, preflight.updates);
    assert!(!env.updates.is_empty());
    let expected = i64::try_from(env.updates.len()).unwrap();
    let (_, host_rows) = count(&world, &c.boss, host).await;
    let fleet = world
        .app
        .call(AdminRuntimeList, &[])
        .token(&root.token)
        .query(&AdminRuntimeListQuery {
            host_id: Some(host),
            kind: None,
        })
        .send()
        .await
        .ok();
    assert_eq!(
        host_rows
            .iter()
            .find(|row| row.id == runtime)
            .unwrap()
            .updates,
        expected
    );
    assert_eq!(
        fleet.iter().find(|row| row.id == runtime).unwrap().updates,
        expected
    );
    assert!(host_rows.iter().all(|row| row.updates == expected));
}

#[tokio::test]
async fn environment_and_host_runtime_pages_read_stored_state_without_inspection_or_build_refresh()
{
    use orch_api_contract::environment::EnvironmentGet;
    use orch_api_contract::resources::{HostRuntimeList, HostRuntimeListQuery};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let world = World::new().await;
    let c = company(&world).await;
    let host = a_host(&world, &c.boss, Some(c.acme), "Stored host").await;
    let mut conn = world.conn().await;
    let template = fixtures::insert_resource(
        &mut conn,
        "template",
        Some(c.boss.id()),
        Some(c.acme),
        "Stored template",
    )
    .await
    .unwrap();
    drop(conn);
    sqlx::query(
        "UPDATE env_templates SET driver='docker',spec='{\"image\":\"image:stored\"}' WHERE id=$1",
    )
    .bind(template)
    .execute(&world.db.pool)
    .await
    .unwrap();
    let (environment, runtime) = a_current_unit(&world, &c, host, template).await;
    sqlx::query(
        "UPDATE machines SET online=true WHERE id=(SELECT machine_id FROM hosts WHERE id=$1)",
    )
    .bind(host)
    .execute(&world.db.pool)
    .await
    .unwrap();
    let reads = Arc::new(AtomicUsize::new(0));
    world
        .app
        .app
        .engine
        .attach_host_services(Arc::new(NoReadBuilds(Arc::clone(&reads))));
    let inspections_before: i64 =
        sqlx::query_scalar("SELECT count(*) FROM commands WHERE kind='node.resources.inspect'")
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    let view = world
        .app
        .call(EnvironmentGet, &[&environment])
        .token(&c.boss.token)
        .query(&orch_api_contract::nodes::DeletedInspectionQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(view.base.id, environment);
    assert!(view.runtime_changes.is_some());
    assert_eq!(
        view.runtime_changes.unwrap().memory_used_bytes,
        None,
        "unknown usage is not guessed from a configured limit"
    );
    let runtimes = world
        .app
        .call(HostRuntimeList, &[&host])
        .token(&c.boss.token)
        .query(&HostRuntimeListQuery::default())
        .send()
        .await
        .ok();
    assert!(runtimes.iter().any(|row| row.id == runtime));
    assert_eq!(
        reads.load(Ordering::SeqCst),
        0,
        "GET must not refresh a catalogue, hash a binary or wait on a host"
    );
    let inspections_after: i64 =
        sqlx::query_scalar("SELECT count(*) FROM commands WHERE kind='node.resources.inspect'")
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(
        inspections_after, inspections_before,
        "GET never sends a native inspection"
    );
}

#[tokio::test]
async fn the_provider_page_and_host_and_fleet_count_the_same_runtime_changes() {
    use orch_api_contract::admin::{AdminRuntimeList, AdminRuntimeListQuery};
    use orch_api_contract::resources::{HostRuntimeList, HostRuntimeListQuery, ProviderGet};
    let world = World::new().await;
    world
        .app
        .app
        .engine
        .attach_host_services(std::sync::Arc::new(RuntimeBuilds));
    let c = company(&world).await;
    let root = admin(&world, "provider-runtime-reader").await;
    let host = a_host(&world, &c.boss, Some(c.acme), "Build").await;
    let mut conn = world.conn().await;
    let template = fixtures::insert_resource(
        &mut conn,
        "template",
        Some(c.boss.id()),
        Some(c.acme),
        "Login unit",
    )
    .await
    .unwrap();
    let provider = fixtures::insert_resource(
        &mut conn,
        "provider",
        Some(c.boss.id()),
        Some(c.acme),
        "Subscription",
    )
    .await
    .unwrap();
    // Its page is started as making it starts it: the boss owns it.
    start_resource(
        &mut conn,
        ObjectRef {
            kind: ResourceKind::Provider.into(),
            id: provider,
        },
        c.boss.id(),
        NewAccess::Defaults,
        Time::new(world.now()),
    )
    .await
    .unwrap();
    let spec = json!({"image":"login:old","env":{}});
    sqlx::query("UPDATE env_templates SET kind='provider',driver='docker',spec=$2 WHERE id=$1")
        .bind(template)
        .bind(&spec)
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE providers SET machine_id=NULL,harness_kind=NULL,agent_id=NULL,runtime_config=$2 WHERE id=$1")
        .bind(provider).bind(json!({"runtime.template_id":template})).execute(&mut *conn).await.unwrap();
    let runtime = orch_types::new_id();
    let kinds = orch_db::queries::resources::provider_runtime::system_harness_kinds(&mut conn)
        .await
        .unwrap();
    let tools = orch_engine::hosting::HostServices::tools(
        &RuntimeBuilds,
        &mut conn,
        "linux",
        "x86_64",
        &kinds,
    )
    .await
    .unwrap();
    let resources = orch_engine::places::limits_of(&spec, None);
    let hash = orch_engine::updates::definition_hash("docker", &spec, &resources, &tools);
    sqlx::query("INSERT INTO runtimes(id,kind,host_id,unit_name,provider_id,driver,template_id,spec_hash,state)
        VALUES($1,'provider',$2,$3,$4,'docker',$5,$6,'running')")
        .bind(runtime).bind(host).bind(format!("login-{runtime}")).bind(provider).bind(template).bind(hash.as_bytes().as_slice())
        .execute(&mut *conn).await.unwrap();
    sqlx::query("INSERT INTO jobs(id,kind,provider_id,state,plan) VALUES($1,'provider_provision',$2,'succeeded',$3)")
        .bind(orch_types::new_id()).bind(provider).bind(json!({"runtime":runtime,"tools":tools,"created_spec":{"spec":spec,"resources":resources}}))
        .execute(&mut *conn).await.unwrap();
    drop(conn);
    assert!(
        world
            .app
            .call(ProviderGet, &[&provider])
            .token(&c.boss.token)
            .send()
            .await
            .ok()
            .runtime
            .unwrap()
            .updates
            .is_empty()
    );
    sqlx::query("UPDATE env_templates SET spec=jsonb_set(spec,'{image}','\"login:new\"'),revision=revision+1 WHERE id=$1")
        .bind(template).execute(&world.db.pool).await.unwrap();
    let view = world
        .app
        .call(ProviderGet, &[&provider])
        .token(&c.boss.token)
        .send()
        .await
        .ok();
    let expected = i64::try_from(view.runtime.unwrap().updates.len()).unwrap();
    assert!(expected > 0);
    let host_rows = world
        .app
        .call(HostRuntimeList, &[&host])
        .token(&c.boss.token)
        .query(&HostRuntimeListQuery::default())
        .send()
        .await
        .ok();
    let fleet = world
        .app
        .call(AdminRuntimeList, &[])
        .token(&root.token)
        .query(&AdminRuntimeListQuery {
            host_id: Some(host),
            kind: None,
        })
        .send()
        .await
        .ok();
    assert_eq!(
        host_rows
            .iter()
            .find(|row| row.id == runtime)
            .unwrap()
            .updates,
        expected
    );
    assert_eq!(
        fleet.iter().find(|row| row.id == runtime).unwrap().updates,
        expected
    );
}

#[tokio::test]
async fn the_units_that_run_on_a_host_are_counted_and_what_they_declare_is_summed() {
    let world = World::new().await;
    let c = company(&world).await;
    let id = a_host(&world, &c.boss, Some(c.acme), "Build box").await;
    name_person(&world, "host", id, "host.use", c.viewer.id()).await;
    let (template, provider) = {
        let mut conn = world.conn().await;
        let template = fixtures::insert_resource(
            &mut conn,
            "template",
            Some(c.boss.id()),
            Some(c.acme),
            "Ubuntu",
        )
        .await
        .unwrap();
        let provider = fixtures::insert_resource(
            &mut conn,
            "provider",
            Some(c.boss.id()),
            Some(c.acme),
            "Sub",
        )
        .await
        .unwrap();
        (template, provider)
    };
    let nothing = read(&world, &c.boss, id).await;
    assert_eq!(
        (
            nothing.running.provider,
            nothing.running.environment,
            nothing.mine
        ),
        (0, 0, 0)
    );
    assert_eq!(
        nothing.declared.cpu_millis + nothing.declared.memory_mb + nothing.declared.disk_gb,
        0
    );
    run_on(
        &world,
        id,
        &c.boss,
        c.acme,
        template,
        json!({ "cpu_millis": 1500, "memory_mb": 2048 }),
    )
    .await;
    run_on(
        &world,
        id,
        &c.viewer,
        c.acme,
        template,
        json!({ "cpu_millis": 500, "disk_gb": 20 }),
    )
    .await;
    // A provider's runtime is a unit too.
    sqlx::query(
        "INSERT INTO runtimes (id, kind, host_id, unit_name, provider_id, driver, template_id, spec_hash, state)
         VALUES ($1, 'provider', $2, 'unit-provider', $3, 'docker', $4, '\\x00'::bytea, 'running')",
    )
    .bind(orch_types::new_id())
    .bind(id)
    .bind(provider)
    .bind(template)
    .execute(&world.db.pool)
    .await
    .unwrap();
    let boss = read(&world, &c.boss, id).await;
    assert_list_measurements(&world, &c.boss, &boss).await;
    assert_eq!(
        (boss.running.provider, boss.running.environment, boss.mine),
        (1, 2, 1)
    );
    assert_eq!(
        (
            boss.declared.cpu_millis,
            boss.declared.memory_mb,
            boss.declared.disk_gb
        ),
        (2000, 2048, 20)
    );
    // Each is told how many of the environments there are their own.
    assert_eq!(read(&world, &c.viewer, id).await.mine, 1);
    // A runtime that was replaced is no longer a unit that runs there.
    sqlx::query(
        "UPDATE runtimes SET state = 'retired'
          WHERE environment_id IN (SELECT id FROM environments WHERE owner_id = $1)",
    )
    .bind(c.boss.id())
    .execute(&world.db.pool)
    .await
    .unwrap();
    let then = read(&world, &c.boss, id).await;
    assert_list_measurements(&world, &c.boss, &then).await;
    assert_eq!(
        (then.running.provider, then.running.environment, then.mine),
        (1, 1, 0)
    );
    assert_eq!(
        (
            then.declared.cpu_millis,
            then.declared.memory_mb,
            then.declared.disk_gb
        ),
        (500, 0, 20)
    );
}

#[tokio::test]
async fn a_host_manager_removes_another_owners_provider_unit_with_the_requested_volume_choice() {
    use orch_api_contract::resources::{HostRuntimeDelete, HostRuntimeDeleteQuery};
    let world = World::new().await;
    let c = company(&world).await;
    let host = a_host(&world, &c.boss, Some(c.acme), "Build box").await;
    name_person(&world, "host", host, "host.use", c.viewer.id()).await;
    for volumes in [None, Some(true)] {
        let runtime = orch_types::new_id();
        let unit_name = format!("orch-provider-{runtime}");
        let (provider, machine) = {
            let mut conn = world.conn().await;
            let provider = fixtures::insert_resource(
                &mut conn,
                "provider",
                Some(c.viewer.id()),
                Some(c.acme),
                "Private provider",
            )
            .await
            .unwrap();
            start_resource(
                &mut conn,
                ObjectRef {
                    kind: ResourceKind::Provider.into(),
                    id: provider,
                },
                c.viewer.id(),
                NewAccess::OnlyMe,
                Time::new(world.now()),
            )
            .await
            .unwrap();
            let template = fixtures::insert_resource(
                &mut conn,
                "template",
                Some(c.boss.id()),
                Some(c.acme),
                "Provider unit",
            )
            .await
            .unwrap();
            let machine = orch_db::queries::resources::testing::insert_machine(
                &mut conn,
                "Provider connector",
                "connected",
            )
            .await
            .unwrap();
            sqlx::query("UPDATE providers SET machine_id=$2 WHERE id=$1")
                .bind(provider)
                .bind(machine)
                .execute(&mut *conn)
                .await
                .unwrap();
            sqlx::query("INSERT INTO runtimes(id,kind,host_id,machine_id,unit_name,provider_id,driver,template_id,spec_hash,state,volumes)
                VALUES($1,'provider',$2,$3,$4,$5,'docker',$6,'\\x00'::bytea,'running',$7)")
                .bind(runtime).bind(host).bind(machine).bind(&unit_name).bind(provider).bind(template)
                .bind(json!({"state":{"name":format!("orch-state-{provider}"),"storage":null}})).execute(&mut *conn).await.unwrap();
            (provider, machine)
        };
        let query = HostRuntimeDeleteQuery {
            confirm: unit_name.clone(),
            volumes,
        };
        let denied = world
            .app
            .call(HostRuntimeDelete, &[&host, &runtime])
            .token(&c.viewer.token)
            .query(&query)
            .send()
            .await
            .problem();
        assert_eq!(denied.code, ErrorCode::AuthzDenied);
        assert_eq!(missing_permissions(&denied), ["host.manage"]);
        let wrong = world
            .app
            .call(HostRuntimeDelete, &[&host, &runtime])
            .token(&c.boss.token)
            .query(&HostRuntimeDeleteQuery {
                confirm: "Private provider".to_owned(),
                volumes,
            })
            .send()
            .await
            .problem();
        assert_eq!(
            wrong.code,
            ErrorCode::RequestInvalid,
            "the API confirms the exact physical unit"
        );
        let before: (String, Option<Id>) = sqlx::query_as("SELECT r.state,p.machine_id FROM runtimes r JOIN providers p ON p.id=r.provider_id WHERE r.id=$1")
            .bind(runtime).fetch_one(&world.db.pool).await.unwrap();
        assert_eq!(before, ("running".to_owned(), Some(machine)));
        let response = world
            .app
            .call(HostRuntimeDelete, &[&host, &runtime])
            .token(&c.boss.token)
            .query(&query)
            .send()
            .await;
        assert!(response.status.is_success(), "{:?}", response.body);
        let after: (String, Option<Id>, Option<chrono::DateTime<chrono::Utc>>) = sqlx::query_as(
            "SELECT r.state,p.machine_id,p.deleted_at FROM runtimes r JOIN providers p ON p.id=r.provider_id WHERE r.id=$1")
            .bind(runtime).fetch_one(&world.db.pool).await.unwrap();
        assert_eq!(
            after,
            ("retired".to_owned(), None, None),
            "the provider survives and the next turn cannot restart the retired unit"
        );
        let plan: serde_json::Value = sqlx::query_scalar("SELECT plan FROM jobs WHERE provider_id=$1 AND kind='environment_remove' ORDER BY created_at DESC,id DESC LIMIT 1")
            .bind(provider).fetch_one(&world.db.pool).await.unwrap();
        assert_eq!(plan["runtime"], json!(runtime));
        assert_eq!(plan["volumes"], json!(volumes == Some(true)));
    }
}

#[tokio::test]
async fn a_host_is_changed_by_whoever_manages_it_and_not_by_one_who_only_uses_it() {
    let world = World::new().await;
    let c = company(&world).await;
    let id = a_host(&world, &c.boss, Some(c.acme), "Build box").await;
    let first = read(&world, &c.boss, id).await;
    let stored: i64 = sqlx::query_scalar("SELECT revision FROM hosts WHERE id=$1")
        .bind(id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(first.revision, stored);
    name_person(&world, "host", id, "host.use", c.viewer.id()).await;
    let refused = world
        .app
        .call(HostUpdate, &[&id])
        .token(&c.viewer.token)
        .body(&HostUpdateBody {
            name: Some("Mine".to_owned()),
            ..change(first.revision)
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(refused.extra["required"], "host.manage");
    assert_eq!(missing_permissions(&refused), ["host.manage"]);
    assert_eq!(read(&world, &c.boss, id).await.base.name, "Build box");
    // An admin of the home administers its page; the host is managed once the page says so.
    let admins = world
        .app
        .call(HostUpdate, &[&id])
        .token(&c.admin.token)
        .body(&HostUpdateBody {
            schedulable: Some(false),
            ..change(first.revision)
        })
        .send()
        .await
        .problem();
    assert_eq!(admins.code, ErrorCode::AuthzDenied);
    let changed = world
        .app
        .call(HostUpdate, &[&id])
        .token(&c.boss.token)
        .body(&HostUpdateBody {
            name: Some("  Big box ".to_owned()),
            labels: Some(vec![
                " gpu ".to_owned(),
                "fast".to_owned(),
                "gpu".to_owned(),
            ]),
            schedulable: Some(false),
            when_full: Some(Some(WhenFull::Block)),
            ..change(first.revision)
        })
        .send()
        .await
        .ok();
    assert_eq!(changed.base.name, "Big box");
    assert_eq!(changed.labels, ["gpu", "fast"]);
    assert!(!changed.schedulable);
    assert_eq!(changed.when_full.value, WhenFull::Block);
    assert!(changed.when_full.own);
    assert!(changed.revision > first.revision);
    // Following the system again.
    let system_again = world
        .app
        .call(HostUpdate, &[&id])
        .token(&c.boss.token)
        .body(&HostUpdateBody {
            when_full: Some(None),
            ..change(changed.revision)
        })
        .send()
        .await
        .ok();
    assert_eq!(system_again.when_full.value, WhenFull::Warn);
    assert!(!system_again.when_full.own);
    assert!(
        system_again.revision > changed.revision,
        "every change moves the revision on"
    );
    let audited: Vec<(String, Option<Id>)> = sqlx::query_as(
        "SELECT action, node_id FROM audit_log WHERE action LIKE 'host.%' ORDER BY at, id",
    )
    .fetch_all(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(
        audited,
        vec![
            ("host.updated".to_owned(), Some(c.acme)),
            ("host.updated".to_owned(), Some(c.acme))
        ]
    );
}

#[tokio::test]
async fn a_change_against_an_old_revision_is_refused_and_a_change_that_asks_for_nothing_new_moves_nothing()
 {
    let world = World::new().await;
    let c = company(&world).await;
    let id = a_host(&world, &c.boss, Some(c.acme), "Build box").await;
    let first = read(&world, &c.boss, id).await;
    let renamed = world
        .app
        .call(HostUpdate, &[&id])
        .token(&c.boss.token)
        .body(&HostUpdateBody {
            name: Some("Second".to_owned()),
            ..change(first.revision)
        })
        .send()
        .await
        .ok();
    // Two changes in the same instant of the clock still make two revisions.
    let again = world
        .app
        .call(HostUpdate, &[&id])
        .token(&c.boss.token)
        .body(&HostUpdateBody {
            name: Some("Third".to_owned()),
            ..change(renamed.revision)
        })
        .send()
        .await
        .ok();
    assert_eq!(renamed.revision, first.revision + 1);
    assert_eq!(again.revision, first.revision + 2);
    let stale = world
        .app
        .call(HostUpdate, &[&id])
        .token(&c.boss.token)
        .body(&HostUpdateBody {
            name: Some("Late".to_owned()),
            ..change(first.revision)
        })
        .send()
        .await
        .problem();
    assert_eq!(stale.code, ErrorCode::ConflictStaleRevision);
    assert_eq!(stale.extra["current"]["name"], "Third");
    assert_eq!(stale.extra["current"]["revision"], again.revision);
    // Asking for what is there changes nothing, and is stale just the same against an old revision.
    let same = world
        .app
        .call(HostUpdate, &[&id])
        .token(&c.boss.token)
        .body(&HostUpdateBody {
            name: Some("Third".to_owned()),
            schedulable: Some(true),
            ..change(again.revision)
        })
        .send()
        .await
        .ok();
    assert_eq!(same.revision, again.revision);
    let old = world
        .app
        .call(HostUpdate, &[&id])
        .token(&c.boss.token)
        .body(&change(first.revision))
        .send()
        .await
        .problem();
    assert_eq!(old.code, ErrorCode::ConflictStaleRevision);
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action = 'host.updated'")
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(count, 2, "only the two real changes are in the trail");
}

#[tokio::test]
async fn what_is_wrong_with_a_change_of_a_host_is_told_member_by_member() {
    let world = World::new().await;
    let c = company(&world).await;
    let id = a_host(&world, &c.boss, Some(c.acme), "Build box").await;
    let revision = read(&world, &c.boss, id).await.revision;
    for (body, path) in [
        (
            HostUpdateBody {
                name: Some("  ".to_owned()),
                ..change(revision)
            },
            "name",
        ),
        (
            HostUpdateBody {
                labels: Some(vec!["fine".to_owned(), " ".to_owned()]),
                ..change(revision)
            },
            "labels",
        ),
        (
            HostUpdateBody {
                labels: Some((0..33).map(|index| format!("label{index}")).collect()),
                ..change(revision)
            },
            "labels",
        ),
    ] {
        let refused = world
            .app
            .call(HostUpdate, &[&id])
            .token(&c.boss.token)
            .body(&body)
            .send()
            .await
            .problem();
        assert_eq!(refused.code, ErrorCode::RequestInvalid, "{path}");
        assert_eq!(refused.extra["errors"][0]["path"], path);
    }
    assert_eq!(read(&world, &c.boss, id).await.revision, revision);
}

async fn token_for(
    world: &World,
    by: &Who,
    home: Option<Id>,
) -> orch_api_contract::resources::EnrollmentToken {
    world
        .app
        .call(HostEnrollmentTokenCreate, &[])
        .token(&by.token)
        .body(&HostEnrollmentTokenCreateBody {
            delegate_policy: None,
            home_id: home,
            name_hint: Some("build-box".to_owned()),
            access: None,
        })
        .send()
        .await
        .ok()
}

#[tokio::test]
async fn the_node_command_binds_its_separate_role_and_local_handover_without_using_the_connector_installer()
 {
    let world = World::new().await;
    let c = company(&world).await;
    for handover in [false, true] {
        let made = world
            .app
            .call(HostEnrollmentTokenCreate, &[])
            .token(&c.boss.token)
            .body(&HostEnrollmentTokenCreateBody {
                home_id: Some(c.acme),
                name_hint: None,
                access: None,
                delegate_policy: Some(handover),
            })
            .send()
            .await
            .ok();
        let token: orch_link_contract::enrollment::EnrollmentToken = made.token.parse().unwrap();
        let choices = token.choices().unwrap().unwrap();
        assert_eq!(choices.roles, vec![orch_link_contract::session::Role::Node]);
        assert_eq!(
            choices.delegate,
            if handover {
                vec!["*".to_owned()]
            } else {
                Vec::<String>::new()
            }
        );
        assert!(made.commands.linux.contains("install-node.sh"));
        assert!(!made.commands.linux.contains("--roles"));
        assert_eq!(made.commands.linux.ends_with("--delegate-policy"), handover);
        assert!(
            !made.commands.windows.contains("install.ps1"),
            "Windows is told to use the node's Linux command"
        );
    }
}

#[tokio::test]
async fn a_member_asks_for_the_token_of_a_host_in_their_org_and_it_is_kept_as_a_hash_for_that_home()
{
    let world = World::new().await;
    let c = company(&world).await;
    let made = token_for(&world, &c.boss, Some(c.acme)).await;
    assert!(made.token.starts_with("oenr_"));
    assert!(made.commands.linux.contains(&made.token));
    // A host runs the node, which its own installer sets up, and which is built for Linux.
    assert!(
        made.commands
            .linux
            .contains("/api/v1/downloads/install-node.sh")
    );
    assert_eq!(made.commands.macos, made.commands.linux);
    assert!(!made.commands.windows.contains(&made.token));
    assert!(made.expires_at.get() > world.now());
    let stored: Vec<(String, Option<Id>, Id, Option<String>)> =
        sqlx::query_as("SELECT purpose, org_id, created_by, name_hint FROM enrollment_tokens")
            .fetch_all(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(
        stored,
        vec![(
            "host".to_owned(),
            Some(c.acme),
            c.boss.id(),
            Some("build-box".to_owned())
        )]
    );
    let hash_length: i32 = sqlx::query_scalar("SELECT length(token_hash) FROM enrollment_tokens")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        hash_length, 32,
        "a SHA-256 of the token is kept, never the token"
    );
}

#[tokio::test]
async fn who_may_add_a_host_where_is_the_engines_to_say() {
    let world = World::new().await;
    let c: Company = company(&world).await;
    let root = admin(&world, "root").await;
    let attempt = |who: &Who, home: Option<Id>| {
        world
            .app
            .call(HostEnrollmentTokenCreate, &[])
            .token(&who.token)
            .body(&HostEnrollmentTokenCreateBody {
                delegate_policy: None,
                home_id: home,
                name_hint: None,
                access: None,
            })
            .send()
    };
    // As shipped, any member adds one as their own; somebody who is no member does not.
    attempt(&c.viewer, Some(c.acme)).await.ok();
    assert_eq!(
        attempt(&c.stranger, Some(c.acme)).await.problem().code,
        ErrorCode::AuthzDenied
    );
    // An org that leaves the kind out for its members keeps it to whoever makes its rules.
    world
        .set_at(
            orch_settings::Target::Node(c.acme),
            "resources.created_by_members",
            json!(["provider"]),
        )
        .await;
    let kept = attempt(&c.viewer, Some(c.acme)).await.problem();
    assert_eq!(kept.code, ErrorCode::AuthzDenied);
    assert!(
        missing_permissions(&kept).contains(&"settings.manage".to_owned()),
        "{kept:?}"
    );
    attempt(&c.admin, Some(c.acme)).await.ok();
    // A host of the system is added by a system admin.
    let refused = attempt(&c.boss, None).await.problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(refused.extra["required"], "system.admin");
    attempt(&root, None).await.ok();
    let purposes: Vec<(String, Option<Id>)> =
        sqlx::query_as("SELECT purpose, org_id FROM enrollment_tokens")
            .fetch_all(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(purposes.len(), 3, "a refusal makes no token: {purposes:?}");
    assert_eq!(
        purposes.iter().filter(|(_, home)| home.is_none()).count(),
        1
    );
}

#[tokio::test]
async fn what_is_wrong_with_the_ask_for_a_host_token_is_told_member_by_member() {
    let world = World::new().await;
    let c = company(&world).await;
    let project = make_project(&world, &c.boss, c.acme, "Web").await;
    let ask = |home: Option<Id>, hint: Option<&str>, access: Option<NewResourceAccess>| {
        let body = HostEnrollmentTokenCreateBody {
            delegate_policy: None,
            home_id: home,
            name_hint: hint.map(str::to_owned),
            access,
        };
        let call = world
            .app
            .call(HostEnrollmentTokenCreate, &[])
            .token(&c.boss.token)
            .body(&body);
        async move { call.send().await.problem() }
    };
    let not_an_org = ask(Some(project.base.id), None, None).await;
    assert_eq!(not_an_org.code, ErrorCode::RequestInvalid);
    assert_eq!(not_an_org.extra["errors"][0]["path"], "home_id");
    let bad_hint = ask(Some(c.acme), Some("   "), None).await;
    assert_eq!(bad_hint.extra["errors"][0]["path"], "name_hint");
    // A resource of the system has no org whose members could be named by Home.
    let choice = ask(None, None, Some(NewResourceAccess::Home)).await;
    assert_eq!(choice.extra["errors"][0]["path"], "access");
    let unknown = ask(Some(orch_types::new_id()), None, None).await;
    assert_eq!(unknown.code, ErrorCode::NotFound);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM enrollment_tokens")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

/// A host's node enrolls with `token`, as a machine does, and the host it became is answered.
fn host_enrollment_request(
    token: &str,
    name: &str,
) -> orch_link_contract::enrollment::EnrollRequest {
    use orch_link_contract::enrollment::EnrollRequest;
    use orch_link_contract::session::{Arch, Os, Platform, Role};
    EnrollRequest {
        machine_id: None,
        token: token.to_owned(),
        csr_pem: orch_pki::enrollment::PendingEnrollment::start()
            .unwrap()
            .csr_pem()
            .to_owned(),
        name: name.to_owned(),
        platform: Platform {
            os: Os::Linux,
            arch: Arch::X86_64,
        },
        connector_version: "0.1.0".to_owned(),
        protocol_versions: vec![1],
        roles: vec![Role::Node],
    }
}

async fn enrolled_host(world: &World, token: &str, name: &str) -> Id {
    let request = host_enrollment_request(token, name);
    let reply = world
        .app
        .app
        .link
        .certificates()
        .enroll(&request, &orch_db::audit::Origin::default())
        .await
        .unwrap();
    sqlx::query_scalar("SELECT id FROM hosts WHERE machine_id = $1")
        .bind(reply.machine_id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn enrollment_keeps_the_token_makers_access_choice_and_tampering_or_replay_widens_nothing() {
    let world = World::new().await;
    let c = company(&world).await;
    let root = admin(&world, "root").await;
    for (name, maker, home, access, listed) in [
        (
            "Private",
            &c.boss,
            Some(c.acme),
            NewResourceAccess::OnlyMe,
            false,
        ),
        (
            "Org members",
            &c.boss,
            Some(c.acme),
            NewResourceAccess::Home,
            true,
        ),
        (
            "Defaults",
            &c.boss,
            Some(c.acme),
            NewResourceAccess::Defaults,
            false,
        ),
        (
            "System private",
            &root,
            None,
            NewResourceAccess::OnlyMe,
            false,
        ),
    ] {
        let made = world
            .app
            .call(HostEnrollmentTokenCreate, &[])
            .token(&maker.token)
            .body(&HostEnrollmentTokenCreateBody {
                delegate_policy: None,
                home_id: home,
                name_hint: Some(name.to_owned()),
                access: Some(access),
            })
            .send()
            .await
            .ok();
        let mut altered = made.token.clone().into_bytes();
        let last = altered.last_mut().unwrap();
        *last = if *last == b'A' { b'B' } else { b'A' };
        let altered = String::from_utf8(altered).unwrap();
        assert!(
            world
                .app
                .app
                .link
                .certificates()
                .enroll(
                    &host_enrollment_request(&altered, "altered"),
                    &orch_db::audit::Origin::default()
                )
                .await
                .is_err()
        );
        let id = enrolled_host(&world, &made.token, name).await;
        assert_eq!(read(&world, maker, id).await.base.name, name);
        let visible = list(&world, &c.viewer, &HostListQuery::default()).await;
        assert_eq!(visible.iter().any(|entry| entry == name), listed);
        if !listed {
            let refused = world
                .app
                .call(HostGet, &[&id])
                .token(&c.viewer.token)
                .send()
                .await
                .problem();
            assert_eq!(refused.code, ErrorCode::AuthzDenied);
        }
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM hosts")
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
        assert!(
            world
                .app
                .app
                .link
                .certificates()
                .enroll(
                    &host_enrollment_request(&made.token, "replayed"),
                    &orch_db::audit::Origin::default()
                )
                .await
                .is_err()
        );
        let after: i64 = sqlx::query_scalar("SELECT count(*) FROM hosts")
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
        assert_eq!(count, after);
    }
    assert_eq!(
        list(&world, &c.stranger, &HostListQuery::default()).await,
        Vec::<String>::new()
    );
}

#[tokio::test]
async fn a_host_that_has_just_enrolled_is_used_by_the_people_its_page_names_from_the_start() {
    let world = World::new().await;
    let c = company(&world).await;
    let root = admin(&world, "root").await;
    // The system's own host, as the bundled node enrolls: every user lists and uses it, and only a
    // system admin manages it.
    let token = token_for(&world, &root, None).await;
    let bundled = enrolled_host(&world, &token.token, "Bundled").await;
    for member in [&c.viewer, &c.stranger] {
        assert_eq!(
            list(&world, member, &HostListQuery::default()).await,
            ["Bundled"]
        );
        let seen = read(&world, member, bundled).await;
        assert_eq!(seen.base.name, "Bundled");
        let refused = world
            .app
            .call(HostUpdate, &[&bundled])
            .token(&member.token)
            .body(&HostUpdateBody {
                name: Some("Mine".to_owned()),
                ..change(seen.revision)
            })
            .send()
            .await
            .problem();
        assert_eq!(refused.code, ErrorCode::AuthzDenied);
    }
    let revision = read(&world, &root, bundled).await.revision;
    world
        .app
        .call(HostUpdate, &[&bundled])
        .token(&root.token)
        .body(&HostUpdateBody {
            labels: Some(vec!["shared".to_owned()]),
            ..change(revision)
        })
        .send()
        .await
        .ok();
    // A host an org's member enrolled for the org is the enroller's own until its page says more.
    let token = token_for(&world, &c.boss, Some(c.acme)).await;
    let mine = enrolled_host(&world, &token.token, "Build box").await;
    assert_eq!(
        list(&world, &c.boss, &HostListQuery::default()).await,
        ["Build box", "Bundled"]
    );
    assert_eq!(
        list(&world, &c.viewer, &HostListQuery::default()).await,
        ["Bundled"]
    );
    assert_eq!(read(&world, &c.boss, mine).await.base.name, "Build box");
}

#[tokio::test]
async fn one_node_serves_two_orgs_through_its_one_hosts_access_page_without_sharing_management() {
    use orch_api_contract::access::{
        HolderInput, PageChange, PageChangeAdd, PageChangeRow, PermissionPageUpdate,
    };
    let world = World::new().await;
    let owner = admin(&world, "node-owner").await;
    let alice = places::person(&world, "alice").await;
    let bea = places::person(&world, "bea").await;
    let outsider = places::person(&world, "outsider").await;
    let first = places::make_org(&world, &alice, "First", "first").await;
    let second = places::make_org(&world, &bea, "Second", "second").await;
    let host = a_host(&world, &owner, None, "Shared node").await;
    let add = |org| PageChangeAdd {
        holder: HolderInput::Members { id: org },
        options: None,
        expires_at: None,
        note: None,
    };
    let share = PageChange {
        rows: Some(vec![PageChangeRow {
            permission: "host.use".into(),
            profile: None,
            inherit: None,
            locked: None,
            add: Some(vec![add(first.base.id), add(second.base.id)]),
            update: None,
            remove: None,
        }]),
        ..Default::default()
    };
    let page = world
        .app
        .call(PermissionPageUpdate, &[&"hosts", &host])
        .token(&owner.token)
        .body(&share)
        .send()
        .await
        .ok();
    assert_eq!(
        page.rows
            .iter()
            .find(|row| row.permission == "host.use")
            .unwrap()
            .entries
            .len(),
        2
    );
    let mut settings = change(read(&world, &owner, host).await.revision);
    settings.labels = Some(vec!["shared-gpu".into()]);
    world
        .app
        .call(HostUpdate, &[&host])
        .token(&owner.token)
        .body(&settings)
        .send()
        .await
        .ok();
    for (member, org) in [(&alice, first.base.id), (&bea, second.base.id)] {
        let shown = read(&world, member, host).await;
        assert!(
            shown
                .access
                .permissions
                .iter()
                .any(|permission| permission == "host.use")
        );
        assert!(
            !shown
                .access
                .permissions
                .iter()
                .any(|permission| permission == "host.manage")
        );
        assert_eq!(
            shown.labels,
            ["shared-gpu"],
            "one host has one operator's settings for both orgs"
        );
        assert_eq!(
            list(&world, member, &HostListQuery { org_id: Some(org) }).await,
            ["Shared node"]
        );
        assert_eq!(
            world
                .app
                .call(HostUpdate, &[&host])
                .token(&member.token)
                .body(&change(shown.revision))
                .send()
                .await
                .problem()
                .code,
            ErrorCode::AuthzDenied
        );
    }
    assert!(
        list(&world, &outsider, &HostListQuery::default())
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn cleanup_previews_blocked_items_requires_the_typed_data_name_and_repeats_one_job() {
    use orch_api_contract::storage::{
        CleanupChoice, CleanupRequest, CleanupSelection, CleanupStart, StorageCleanupCreate,
        StorageCleanupPreflight, StorageGet, StorageQuery,
    };
    use orch_driver_contract::resources::{Resource, ResourceInventory, ResourceKind, ResourceRef};
    let world = World::new().await;
    let c = company(&world).await;
    let host = a_host(&world, &c.boss, Some(c.acme), "Kept volumes").await;
    name_person(&world, "host", host, "host.view", c.viewer.id()).await;
    let inventory = ResourceInventory {
        observed_at: Time::new(world.now()),
        unavailable: Vec::new(),
        resources: vec![
            Resource {
                resource: ResourceRef {
                    kind: ResourceKind::Volume,
                    id: "data".into(),
                },
                name: "Kept data".into(),
                size_bytes: Some(8),
                labels: Default::default(),
                depends_on: Vec::new(),
                aliases: Vec::new(),
                last_used_at: None,
                busy: false,
            },
            Resource {
                resource: ResourceRef {
                    kind: ResourceKind::Volume,
                    id: "busy".into(),
                },
                name: "Used data".into(),
                size_bytes: None,
                labels: Default::default(),
                depends_on: Vec::new(),
                aliases: Vec::new(),
                last_used_at: None,
                busy: true,
            },
        ],
    };
    sqlx::query("UPDATE hosts SET resource_inventory=$2 WHERE id=$1")
        .bind(host)
        .bind(json!(inventory))
        .execute(&world.db.pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE machines SET online=true WHERE id=(SELECT machine_id FROM hosts WHERE id=$1)",
    )
    .bind(host)
    .execute(&world.db.pool)
    .await
    .unwrap();
    let scope = format!("host:{host}");
    let report = world
        .app
        .call(StorageGet, &[])
        .token(&c.boss.token)
        .query(&StorageQuery {
            scope: Some(scope.clone()),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    let request = CleanupRequest {
        scope,
        resource: Some(ObjectRef {
            kind: orch_access_contract::permissions::ObjectKind::Host,
            id: host,
        }),
        items: report.items.iter().map(|row| row.id.clone()).collect(),
    };
    assert_eq!(
        world
            .app
            .call(StorageCleanupPreflight, &[])
            .token(&c.viewer.token)
            .body(&request)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::NotFound
    );
    let plan = world
        .app
        .call(StorageCleanupPreflight, &[])
        .token(&c.boss.token)
        .body(&request)
        .send()
        .await
        .ok();
    assert_eq!(
        plan.items.len(),
        2,
        "blocked things remain rows in the one plan"
    );
    let data = plan
        .items
        .iter()
        .find(|item| item.record.name == "Kept data")
        .unwrap();
    let busy = plan
        .items
        .iter()
        .find(|item| item.record.name == "Used data")
        .unwrap();
    assert_eq!(busy.choices, [CleanupChoice::Keep]);
    assert!(busy.record.blocker.is_some());
    assert_eq!(data.selected, CleanupChoice::Keep);
    assert_eq!(data.choices, [CleanupChoice::Keep, CleanupChoice::Purge]);
    let mut start = CleanupStart {
        plan_id: plan.id,
        choices: vec![CleanupSelection {
            id: data.record.id.clone(),
            choice: CleanupChoice::Purge,
        }],
        confirmation_name: Some("wrong".into()),
        notify: false,
    };
    assert_eq!(
        world
            .app
            .call(StorageCleanupCreate, &[])
            .token(&c.boss.token)
            .body(&start)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::RequestInvalid
    );
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='storage_cleanup'")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(before, 0, "a refused name admits no destructive work");
    start.confirmation_name = Some(plan.confirmation_name);
    let first = world
        .app
        .call(StorageCleanupCreate, &[])
        .token(&c.boss.token)
        .body(&start)
        .send()
        .await
        .ok();
    let repeated = world
        .app
        .call(StorageCleanupCreate, &[])
        .token(&c.boss.token)
        .body(&start)
        .send()
        .await
        .ok();
    assert_eq!(
        first.id, repeated.id,
        "a lost acceptance response cannot duplicate removal"
    );
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='storage_cleanup'")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(after, 1);
    assert!(
        orch_db::audit::emitters::registered().any(|writer| writer.action == "storage.cleaned")
    );
}

#[tokio::test]
async fn operator_storage_uses_admin_mode_shared_dependencies_and_commands_without_an_api_erase_fate()
 {
    use orch_api_contract::resource_facts::DependentRelation;
    use orch_api_contract::storage::{
        CleanupChoice, CleanupRequest, CleanupSelection, CleanupStart, KeptWhy,
        OperatorStorageDependency, OperatorStorageInventory, OperatorStorageItem,
        StorageAvailability, StorageCleanupCreate, StorageCleanupPreflight, StorageGet,
        StorageKind, StorageQuery,
    };
    let directory = tempfile::tempdir().unwrap();
    let mut config = orch_config::ServerConfig::default();
    config.installation.records_dir = directory.path().to_owned();
    let world = World::with(config).await;
    let by = admin(&world, "operator").await;
    let at = Time::new(world.now());
    let mut current = orch_engine::storage::record(
        "release:server:current".into(),
        StorageKind::Release,
        "server/current".into(),
        at,
    );
    current.bytes = Some(8);
    current.why = KeptWhy::InUse;
    current.availability = StorageAvailability::Ready;
    current.blocker = Some("release_active".into());
    let mut unused = orch_engine::storage::record(
        "release:server:unused".into(),
        StorageKind::Release,
        "server/unused".into(),
        at,
    );
    unused.bytes = Some(4);
    unused.why = KeptWhy::Cache;
    unused.availability = StorageAvailability::Ready;
    unused.can_clean = true;
    unused.operator_command = Some("'/opt/orch/deploy/update.sh' prune".into());
    let inventory = OperatorStorageInventory {
        format_version: 1,
        system_id: world
            .app
            .app
            .link
            .certificates()
            .authority_certificate()
            .fingerprint(),
        observed_at: at,
        issues: vec![],
        items: vec![
            OperatorStorageItem {
                record: current,
                dependencies: vec![OperatorStorageDependency {
                    key: "server/current".into(),
                    name: "server/current".into(),
                    relation: DependentRelation::Attachment,
                }],
            },
            OperatorStorageItem {
                record: unused,
                dependencies: vec![],
            },
        ],
    };
    let file = directory.path().join("storage.json");
    let original = serde_json::to_vec(&inventory).unwrap();
    std::fs::write(&file, &original).unwrap();
    let query = StorageQuery {
        scope: Some("system".into()),
        resource_kind: None,
        resource_id: None,
        host: None,
        place: None,
        owner: None,
        kind: Some(StorageKind::Release),
        why: None,
        age: None,
        page: Default::default(),
    };
    places::admin_mode(&world, &by, false).await;
    assert_eq!(
        world
            .app
            .call(StorageGet, &[])
            .token(&by.token)
            .query(&query)
            .send()
            .await
            .status,
        404
    );
    places::admin_mode(&world, &by, true).await;
    let report = world
        .app
        .call(StorageGet, &[])
        .token(&by.token)
        .query(&query)
        .send()
        .await
        .ok();
    assert_eq!((report.totals.items, report.totals.bytes), (2, 12));
    let held = report
        .items
        .iter()
        .find(|row| row.id == "release:server:current")
        .unwrap();
    let facts = held.facts.as_ref().unwrap();
    assert!(facts.uses.is_none());
    assert_eq!(facts.dependents.len(), 1);
    assert_eq!(facts.dependents[0].count, 1);
    assert_eq!(facts.dependents[0].visible[0].name, "server/current");
    let plan = world
        .app
        .call(StorageCleanupPreflight, &[])
        .token(&by.token)
        .body(&CleanupRequest {
            scope: "system".into(),
            resource: None,
            items: vec!["release:server:unused".into()],
        })
        .send()
        .await
        .ok();
    assert_eq!(plan.items.len(), 1);
    let item = &plan.items[0];
    assert_eq!(
        item.record.operator_command.as_deref(),
        Some("'/opt/orch/deploy/update.sh' prune")
    );
    assert_eq!(
        item.record.blocker, None,
        "a local command is not an unavailable API erase target"
    );
    assert_eq!(item.choices, vec![CleanupChoice::Keep]);
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='storage_cleanup'")
        .fetch_one(&mut *world.conn().await)
        .await
        .unwrap();
    let denied = world
        .app
        .call(StorageCleanupCreate, &[])
        .token(&by.token)
        .body(&CleanupStart {
            plan_id: plan.id,
            choices: vec![CleanupSelection {
                id: item.record.id.clone(),
                choice: CleanupChoice::Purge,
            }],
            confirmation_name: Some(plan.confirmation_name.clone()),
            notify: false,
        })
        .send()
        .await;
    assert_eq!(
        denied.status, 400,
        "the API cannot manufacture a file erase fate for operator-owned storage"
    );
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='storage_cleanup'")
        .fetch_one(&mut *world.conn().await)
        .await
        .unwrap();
    assert_eq!(before, after);
    assert_eq!(
        std::fs::read(file).unwrap(),
        original,
        "the API only reads the protected metadata"
    );
}

#[tokio::test]
async fn a_running_unrecorded_unit_needs_an_explicit_typed_stop_and_a_dependent_unit_is_not_stopped()
 {
    use orch_api_contract::storage::{
        CleanupChoice, CleanupRequest, CleanupSelection, CleanupStart, StorageCleanupCreate,
        StorageCleanupPreflight,
    };
    use orch_driver_contract::resources::{
        Resource, ResourceInventory, ResourceKind as NativeKind, ResourceRef,
    };
    let world = World::new().await;
    let c = company(&world).await;
    let host = a_host(&world, &c.boss, Some(c.acme), "Orphaned units").await;
    let base = Resource {
        resource: ResourceRef {
            kind: NativeKind::Unit,
            id: "unit".into(),
        },
        name: "Unrecorded unit".into(),
        size_bytes: Some(4),
        labels: Default::default(),
        depends_on: vec![],
        aliases: vec![],
        last_used_at: None,
        busy: true,
    };
    let inventory = ResourceInventory {
        observed_at: Time::new(world.now()),
        unavailable: vec![],
        resources: vec![base.clone()],
    };
    sqlx::query("UPDATE hosts SET resource_inventory=$2 WHERE id=$1")
        .bind(host)
        .bind(json!(inventory))
        .execute(&world.db.pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE machines SET online=true WHERE id=(SELECT machine_id FROM hosts WHERE id=$1)",
    )
    .bind(host)
    .execute(&world.db.pool)
    .await
    .unwrap();
    let request = CleanupRequest {
        scope: format!("host:{host}"),
        resource: Some(ObjectRef {
            kind: orch_access_contract::permissions::ObjectKind::Host,
            id: host,
        }),
        items: vec![],
    };
    let plan = world
        .app
        .call(StorageCleanupPreflight, &[])
        .token(&c.boss.token)
        .body(&request)
        .send()
        .await
        .ok();
    assert_eq!(plan.items.len(), 1);
    let item = &plan.items[0];
    assert_eq!(
        item.selected,
        CleanupChoice::Keep,
        "a running unit is never selected for removal by default"
    );
    assert_eq!(
        item.choices,
        [CleanupChoice::Keep, CleanupChoice::StopAndRemove]
    );
    let mut start = CleanupStart {
        plan_id: plan.id,
        choices: vec![CleanupSelection {
            id: item.record.id.clone(),
            choice: CleanupChoice::StopAndRemove,
        }],
        confirmation_name: None,
        notify: false,
    };
    assert_eq!(
        world
            .app
            .call(StorageCleanupCreate, &[])
            .token(&c.boss.token)
            .body(&start)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::RequestInvalid,
        "its writable root needs the same data-name confirmation"
    );
    start.confirmation_name = Some(plan.confirmation_name);
    let accepted = world
        .app
        .call(StorageCleanupCreate, &[])
        .token(&c.boss.token)
        .body(&start)
        .send()
        .await
        .ok();
    assert_eq!(accepted.kind, "storage_cleanup");
    let mut dependent = base.clone();
    dependent.resource.id = "dependent".into();
    dependent.name = "A unit that needs it".into();
    dependent.depends_on.push(base.resource.clone());
    let inventory = ResourceInventory {
        observed_at: Time::new(world.now()),
        unavailable: vec![],
        resources: vec![base, dependent],
    };
    sqlx::query("UPDATE hosts SET resource_inventory=$2 WHERE id=$1")
        .bind(host)
        .bind(json!(inventory))
        .execute(&world.db.pool)
        .await
        .unwrap();
    let plan = world
        .app
        .call(StorageCleanupPreflight, &[])
        .token(&c.boss.token)
        .body(&request)
        .send()
        .await
        .ok();
    let needed = plan
        .items
        .iter()
        .find(|item| item.record.name == "Unrecorded unit")
        .unwrap();
    assert_eq!(needed.choices, [CleanupChoice::Keep]);
    assert!(
        needed.record.blocker.is_some(),
        "another unit's dependency blocks stopping, before any native effect"
    );
}
