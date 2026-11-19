//! Automatic cleanup honors cache age, disabled policy and the artifacts retained data still needs.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

#[tokio::test]
async fn operation_notification_defaults_follow_the_place_override_and_a_system_lock() {
    let world = World::new().await;
    world.engine().stop();
    let mut conn = world.pool.acquire().await.unwrap();
    let place = orch_settings::Place::Node(world.topic);
    assert!(
        orch_engine::notifications::operation_default(&mut conn, place, true)
            .await
            .unwrap()
    );
    assert!(
        !orch_engine::notifications::operation_default(&mut conn, place, false)
            .await
            .unwrap()
    );
    for (scope, value, locked) in [
        (None, "never", false),
        (Some(world.topic), "always", false),
        (None, "never", true),
    ] {
        orch_db::queries::settings::put(
            &mut conn,
            scope,
            "notify.operations",
            &orch_db::queries::settings::Stored {
                value: &json!(value),
                locked,
                by: Some(world.ada),
                at: world.clock.now(),
            },
            None,
        )
        .await
        .unwrap();
        let expected = scope.is_some();
        assert_eq!(
            orch_engine::notifications::operation_default(&mut conn, place, true)
                .await
                .unwrap(),
            expected
        );
        assert_eq!(
            orch_engine::notifications::operation_default(&mut conn, place, false)
                .await
                .unwrap(),
            expected
        );
    }
    let own: serde_json::Value = sqlx::query_scalar(
        "SELECT value FROM settings WHERE node_id=$1 AND key='notify.operations'",
    )
    .bind(world.topic)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(
        own,
        json!("always"),
        "the system lock never erases the stored place choice"
    );
}
mod hosting;

use common::{World, eventually};
use hosting::ScriptedHost;
use orch_driver_contract::resources::{Resource, ResourceInventory, ResourceKind, ResourceRef};
use orch_link_contract::commands::{Command, CommandKind, CommandResult, Empty};
use orch_types::{Id, Time, new_id};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

#[test]
fn storage_counts_physical_content_once_and_keeps_unknown_sizes_distinct() {
    use orch_api_contract::storage::StorageKind;
    let at = Time::new(chrono::Utc::now());
    let mut one =
        orch_engine::storage::record("one".into(), StorageKind::Content, "one".into(), at);
    one.bytes = Some(5);
    one.accounting_key = "physical".into();
    let mut alias = one.clone();
    alias.id = "alias".into();
    let unknown = orch_engine::storage::record(
        "unknown".into(),
        StorageKind::ToolVersion,
        "unknown".into(),
        at,
    );
    let total = orch_engine::storage::totals(&[one.clone(), alias, unknown]);
    assert_eq!((total.items, total.bytes, total.unknown), (3, 5, 1));
    let mut parent = orch_engine::storage::record(
        "parent".into(),
        StorageKind::EnvironmentFiles,
        "parent".into(),
        at,
    );
    parent.bytes = Some(8);
    one.included_in = Some(parent.id.clone());
    assert_eq!(
        orch_engine::storage::totals(&[parent, one.clone()]).bytes,
        8
    );
    assert_eq!(
        orch_engine::storage::totals(&[one]).bytes,
        5,
        "a filtered subset without the parent counts its measured child"
    );
    let mut unknown_parent = orch_engine::storage::record(
        "parent".into(),
        StorageKind::EnvironmentFiles,
        "parent".into(),
        at,
    );
    unknown_parent.bytes = None;
    let mut measured_child =
        orch_engine::storage::record("child".into(), StorageKind::Volume, "child".into(), at);
    measured_child.bytes = Some(8);
    measured_child.included_in = Some("parent".into());
    let partial = orch_engine::storage::totals(&[unknown_parent, measured_child]);
    assert_eq!(
        (partial.bytes, partial.unknown),
        (8, 1),
        "an unmeasured aggregate cannot hide measured content"
    );
}

#[tokio::test]
async fn shared_facts_deduplicate_meter_rows_use_utc_days_and_keep_captured_people() {
    use orch_access_contract::decisions::ObjectRef;
    use orch_access_contract::permissions::ObjectKind;
    use orch_api_contract::resource_facts::{DependentRelation, FactSubject, UseCoverage};
    let world = World::new().await;
    let thread = world.thread("facts").await;
    let as_of = Time::new(world.clock.now());
    let from = (as_of.get().date_naive() - chrono::Duration::days(29))
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc();
    let first = orch_types::new_id();
    let second = orch_types::new_id();
    let old = orch_types::new_id();
    let other = orch_types::new_id();
    let mut conn = world.pool.acquire().await.unwrap();
    for (turn, user, at) in [
        (first, world.ada, from + chrono::Duration::milliseconds(1)),
        (first, world.ada, as_of.get()),
        (second, other, as_of.get()),
        (old, world.ada, from - chrono::Duration::milliseconds(1)),
    ] {
        sqlx::query("INSERT INTO usage_events(id,at,origin,scope,counted,org_id,project_id,topic_id,thread_id,turn_id,provider_id,user_id,agent_id,environment_id,model) VALUES($1,$2,'harness','turn',false,$3,$4,$5,$6,$7,$8,$9,$10,$11,'actual')")
            .bind(orch_types::new_id()).bind(at).bind(world.root).bind(world.project).bind(world.topic).bind(thread)
            .bind(turn).bind(world.provider).bind(user).bind(world.agent).bind(world.environment).execute(&mut *conn).await.unwrap();
    }
    // A summary is real consumption but has no turn identity, and must not become another turn.
    sqlx::query("INSERT INTO usage_events(id,at,origin,purpose,scope,counted,org_id,project_id,topic_id,provider_id,user_id,model) VALUES($1,$2,'orch','summary','request',true,$3,$4,$5,$6,$7,'summary')")
        .bind(orch_types::new_id()).bind(as_of.get()).bind(world.root).bind(world.project).bind(world.topic).bind(world.provider).bind(world.ada)
        .execute(&mut *conn).await.unwrap();
    // One explicit giving scope is one edge; inherited topic/thread receivers are not expanded.
    drop(conn);
    world
        .set(
            world.root,
            "agent.default_provider_id",
            json!(world.provider),
        )
        .await;
    let mut conn = world.pool.acquire().await.unwrap();
    let subjects = [
        ObjectKind::Provider,
        ObjectKind::Agent,
        ObjectKind::Environment,
    ]
    .into_iter()
    .zip([world.provider, world.agent, world.environment])
    .map(|(kind, id)| FactSubject::Resource {
        object: ObjectRef { kind, id },
    })
    .collect::<Vec<_>>();
    let facts = orch_engine::resource_facts::read(&mut conn, &subjects, as_of)
        .await
        .unwrap();
    for row in &facts {
        let uses = row.uses.as_ref().unwrap();
        assert_eq!(uses.coverage, UseCoverage::Complete);
        assert_eq!(uses.all_time, Some(3));
        assert_eq!(uses.last_30_days, Some(2));
        assert_eq!(uses.people, Some(2));
        assert_eq!(uses.people_30_days, Some(2));
        assert_eq!(uses.last_used_at, Some(as_of));
    }
    assert_eq!(
        facts[0]
            .dependents
            .iter()
            .filter(|edge| edge.relation == DependentRelation::ExplicitGiving)
            .count(),
        1
    );
    assert!(
        facts[0]
            .dependents
            .iter()
            .any(|edge| edge.object.is_some_and(|object| object.id == thread))
    );
    drop(conn);
    world.shutdown().await;
}

fn item(kind: ResourceKind, id: &str, used: Option<Time>, busy: bool) -> Resource {
    Resource {
        resource: ResourceRef {
            kind,
            id: id.into(),
        },
        name: id.into(),
        size_bytes: Some(4),
        labels: BTreeMap::new(),
        depends_on: Vec::new(),
        aliases: Vec::new(),
        last_used_at: used,
        busy,
    }
}

#[tokio::test]
async fn a_hosts_two_current_units_for_one_owner_are_counted_separately_and_keep_the_owner_name() {
    use orch_access_contract::{decisions::ObjectRef, permissions::ObjectKind};
    use orch_api_contract::resource_facts::{DependentRelation, FactSubject};
    use orch_authz::{
        Caller, Principal,
        ownership::{NewAccess, start_resource},
    };
    use orch_db::queries::environments::hosting as envs;
    let world = World::new().await;
    world.engine().stop();
    let host = ScriptedHost::on(&world, "Two current units", 16_384).await;
    let template = hosting::template(&world).await;
    let environment = new_id();
    let old = new_id();
    let next = new_id();
    let now = world.clock.now();
    let mut tx = world.pool.begin().await.unwrap();
    envs::insert_managed(
        &mut tx,
        &envs::NewManaged {
            id: environment,
            name: "One owner",
            owner_id: world.ada,
            home_node_id: world.topic,
            template_id: template,
            config: &json!({}),
            host_entry_id: None,
            template_entry_id: None,
        },
        now,
    )
    .await
    .unwrap();
    start_resource(
        &mut tx,
        ObjectRef {
            kind: ObjectKind::Environment,
            id: environment,
        },
        world.ada,
        NewAccess::OnlyMe,
        Time::new(now),
    )
    .await
    .unwrap();
    envs::insert_runtime(
        &mut tx,
        &envs::NewRuntime {
            id: old,
            host_id: host.id,
            unit_name: &format!("orch-env-{}", old.simple()),
            environment_id: environment,
            driver: "docker",
            template_id: template,
            spec_hash: &[0; 32],
            generation: 1,
            volumes: &json!({}),
        },
        now,
    )
    .await
    .unwrap();
    sqlx::query("UPDATE runtimes SET state='running' WHERE id=$1")
        .bind(old)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("INSERT INTO runtimes(id,kind,host_id,unit_name,successor_of,environment_id,driver,template_id,spec_hash,generation,state,volumes,created_at,updated_at) SELECT $2,kind,host_id,$3,id,environment_id,driver,template_id,spec_hash,generation+1,'stopped',volumes,$4,$4 FROM runtimes WHERE id=$1")
        .bind(old).bind(next).bind(format!("orch-env-{}",next.simple())).bind(now).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    let mut conn = world.pool.acquire().await.unwrap();
    let host_subject = FactSubject::Resource {
        object: ObjectRef {
            kind: ObjectKind::Host,
            id: host.id,
        },
    };
    let template_subject = FactSubject::Resource {
        object: ObjectRef {
            kind: ObjectKind::Template,
            id: template,
        },
    };
    let facts = orch_engine::resource_facts::dependents(
        &mut conn,
        &[host_subject, template_subject],
        Time::new(now),
    )
    .await
    .unwrap();
    let owner = ObjectRef {
        kind: ObjectKind::Environment,
        id: environment,
    };
    let units = facts[0]
        .dependents
        .iter()
        .filter(|edge| edge.relation == DependentRelation::Attachment && edge.object == Some(owner))
        .collect::<Vec<_>>();
    assert_eq!(
        units.len(),
        2,
        "both current unit generations depend on the host"
    );
    assert_ne!(units[0].key, units[1].key);
    assert!(
        units
            .iter()
            .all(|edge| edge.visibility == Some(owner) && edge.name == "One owner"),
        "unit identity never replaces the permitted owner chip"
    );
    assert_eq!(
        facts[1]
            .dependents
            .iter()
            .filter(
                |edge| edge.relation == DependentRelation::Attachment && edge.object == Some(owner)
            )
            .count(),
        1,
        "the template still counts its one environment"
    );
    let principal = Principal::load(&mut conn, Caller::session(world.ada), Time::new(now))
        .await
        .unwrap();
    let visible =
        orch_engine::resource_facts::project(&mut conn, &principal, Default::default(), &facts)
            .await
            .unwrap();
    let attached = visible[0]
        .dependents
        .iter()
        .find(|group| group.relation == DependentRelation::Attachment)
        .unwrap();
    assert_eq!(attached.count, 2);
    assert_eq!(attached.visible.len(), 2);
    assert!(
        attached
            .visible
            .iter()
            .all(|edge| edge.object == Some(owner) && edge.name == "One owner")
    );
    assert_eq!(attached.hidden, 0);
}

fn answer(host: &ScriptedHost, inventory: &Arc<Mutex<ResourceInventory>>) {
    let listing = inventory.clone();
    host.answer(CommandKind::NodeResourcesList, move |_| {
        Ok(CommandResult::NodeResourcesList(
            listing.lock().unwrap().clone(),
        ))
    });
    let changing = inventory.clone();
    host.answer(CommandKind::NodeResourcesRemove, move |command| {
        let Command::NodeResourcesRemove(body) = command else {
            panic!("not an inventory removal")
        };
        assert!(
            matches!(
                body.kind,
                ResourceKind::Image | ResourceKind::Build | ResourceKind::ToolVersion
            ),
            "owned data never enters automatic cleanup"
        );
        let mut inventory = changing.lock().unwrap();
        let selected = inventory
            .resources
            .iter()
            .find(|item| item.resource == body.reference())
            .unwrap();
        assert_eq!(body.expected_last_used_at, selected.last_used_at);
        inventory
            .resources
            .retain(|item| item.resource != body.reference());
        Ok(CommandResult::NodeResourcesRemove(Empty {}))
    });
}

#[tokio::test]
async fn a_mount_location_keeps_its_volume_without_a_unit_and_its_kept_record_keeps_the_same_hold()
{
    use orch_access_contract::{decisions::ObjectRef, permissions::ObjectKind};
    use orch_api_contract::resource_facts::{DependentRelation, FactSubject};
    use orch_authz::ownership::{NewAccess, start_resource};
    use orch_db::queries::resources::mounts::{self, KeptRow, LocationRow, MountRow};
    let world = World::new().await;
    hosting::serve(&world);
    let host = ScriptedHost::on(&world, "Registered mount data", 16_384).await;
    let mount = new_id();
    let native_name = format!("orch-mount-{mount}");
    let now = world.clock.now();
    let mut native = item(ResourceKind::Volume, "registered-data", None, false);
    native.name = native_name.clone();
    native
        .labels
        .insert("orch.mount_id".into(), mount.to_string());
    let inventory = ResourceInventory {
        observed_at: Time::new(now),
        resources: vec![native],
        unavailable: vec![],
    };
    let mut tx = world.pool.begin().await.unwrap();
    mounts::insert(
        &mut tx,
        &MountRow {
            id: mount,
            name: "unattached-data".into(),
            description: String::new(),
            org_id: Some(world.root),
            home_id: Some(world.root),
            owner_id: Some(world.ada),
            target: "/data/shared".into(),
            read_only: false,
            enabled: true,
            default_fate: "as_is".into(),
            revision: 1,
        },
        now,
    )
    .await
    .unwrap();
    start_resource(
        &mut tx,
        ObjectRef {
            kind: ObjectKind::Mount,
            id: mount,
        },
        world.ada,
        NewAccess::OnlyMe,
        Time::new(now),
    )
    .await
    .unwrap();
    mounts::set_location(
        &mut tx,
        &LocationRow {
            mount_id: mount,
            machine_id: host.machine,
            host_id: None,
            host_name: None,
            kind: "volume".into(),
            source: native_name.clone(),
            storage: None,
            data_kind: None,
            created_by: world.ada,
            created_at: now,
        },
    )
    .await
    .unwrap();
    sqlx::query("UPDATE hosts SET resource_inventory=$2 WHERE id=$1")
        .bind(host.id)
        .bind(serde_json::to_value(inventory).unwrap())
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let subject = FactSubject::Artifact {
        host_id: host.id,
        artifact_kind: "volume".into(),
        id: "registered-data".into(),
    };
    let mut conn = world.pool.acquire().await.unwrap();
    let facts = orch_engine::resource_facts::dependents(
        &mut conn,
        std::slice::from_ref(&subject),
        Time::new(now),
    )
    .await
    .unwrap();
    assert!(
        facts[0]
            .dependents
            .iter()
            .any(|edge| edge.relation == DependentRelation::Attachment
                && edge.object
                    == Some(ObjectRef {
                        kind: ObjectKind::Mount,
                        id: mount
                    })),
        "a registered location is not an orphan even without a unit"
    );
    assert!(
        facts[0].uses.is_none(),
        "a mount hold is not a popularity counter"
    );
    mounts::remove_location(&mut conn, mount, host.machine)
        .await
        .unwrap();
    let kept = new_id();
    mounts::keep(
        &mut conn,
        &KeptRow {
            id: kept,
            mount_id: mount,
            machine_id: host.machine,
            kind: "volume".into(),
            source: native_name,
            storage: None,
            data_kind: None,
            file_count: Some(0),
            total_bytes: Some(0),
            job_id: None,
            kept_by: world.ada,
            kept_at: now,
        },
    )
    .await
    .unwrap();
    let facts = orch_engine::resource_facts::dependents(
        &mut conn,
        std::slice::from_ref(&subject),
        Time::new(now),
    )
    .await
    .unwrap();
    assert!(
        facts[0]
            .dependents
            .iter()
            .any(|edge| edge.relation == DependentRelation::Restore
                && edge.object
                    == Some(ObjectRef {
                        kind: ObjectKind::Mount,
                        id: mount
                    }))
    );
    assert!(
        !facts[0]
            .dependents
            .iter()
            .any(|edge| edge.relation == DependentRelation::Attachment)
    );
    sqlx::query("UPDATE mount_kept_data SET lost_at=$2 WHERE id=$1")
        .bind(kept)
        .bind(now)
        .execute(&mut *conn)
        .await
        .unwrap();
    let facts = orch_engine::resource_facts::dependents(&mut conn, &[subject], Time::new(now))
        .await
        .unwrap();
    assert!(
        !facts[0].dependents.iter().any(|edge| matches!(
            edge.relation,
            DependentRelation::Attachment | DependentRelation::Restore
        )),
        "permanently lost metadata is not a current recovery hold"
    );
    world.engine().stop();
}

async fn sweep(world: &World, host: &ScriptedHost, epoch: u64, total: usize) {
    host.added
        .machine
        .publish(
            &orch_link_contract::streams::Stream::Node,
            &orch_link_contract::events::Event::NodeKeptImages(
                orch_link_contract::events::NodeKeptImages {
                    epoch,
                    images: Vec::new(),
                },
            ),
        )
        .await;
    eventually("the cache sweep ends", || async {
        let states: Vec<String> = sqlx::query_scalar(
            "SELECT state FROM jobs WHERE kind='kept_images_sweep' AND plan->>'host'=$1",
        )
        .bind(host.id.to_string())
        .fetch_all(world.pool.pool())
        .await
        .unwrap();
        states.len() == total
            && states
                .iter()
                .all(|state| !matches!(state.as_str(), "queued" | "running"))
    })
    .await;
    let states: Vec<(String, Option<serde_json::Value>)> = sqlx::query_as(
        "SELECT state,error FROM jobs WHERE kind='kept_images_sweep' AND plan->>'host'=$1",
    )
    .bind(host.id.to_string())
    .fetch_all(world.pool.pool())
    .await
    .unwrap();
    assert!(
        states.iter().all(|(state, _)| state == "succeeded"),
        "{states:?}"
    );
}

#[tokio::test]
async fn seven_days_removes_only_unused_caches_and_zero_keeps_even_an_old_cache() {
    let clock = Arc::new(orch_types::ManualClock::new(chrono::Utc::now()));
    let world = World::with_clock(clock).await;
    hosting::serve(&world);
    let host = ScriptedHost::on(&world, "cache-policy", 16_384).await;
    let now = Time::new(world.clock.now());
    let old = Time::new(now.get() - chrono::Duration::days(7));
    let inventory = Arc::new(Mutex::new(ResourceInventory {
        observed_at: now,
        unavailable: Vec::new(),
        resources: vec![
            item(ResourceKind::ToolVersion, "old", Some(old), false),
            item(
                ResourceKind::ToolVersion,
                "recent",
                Some(Time::new(old.get() + chrono::Duration::milliseconds(1))),
                false,
            ),
            item(ResourceKind::ToolVersion, "busy", Some(old), true),
            item(ResourceKind::ToolVersion, "unknown", None, false),
            item(ResourceKind::Volume, "data", Some(old), false),
            item(ResourceKind::KeptLayer, "retained-files", Some(old), false),
        ],
    }));
    answer(&host, &inventory);
    sweep(&world, &host, 1, 1).await;
    assert_eq!(host.count(CommandKind::NodeResourcesRemove), 1);
    let removed = host
        .calls()
        .into_iter()
        .find(|call| call.kind == CommandKind::NodeResourcesRemove)
        .unwrap();
    assert_eq!(removed.body["id"], "old");
    let snapshot: serde_json::Value =
        sqlx::query_scalar("SELECT resource_inventory FROM hosts WHERE id=$1")
            .bind(host.id)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert!(
        !snapshot["resources"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["resource"]["id"] == "old")
    );
    let mut conn = world.pool.acquire().await.unwrap();
    orch_db::queries::settings::put(
        &mut conn,
        None,
        "cleanup.unused_keep_days",
        &orch_db::queries::settings::Stored {
            value: &json!(0),
            locked: false,
            by: Some(world.ada),
            at: world.clock.now(),
        },
        None,
    )
    .await
    .unwrap();
    drop(conn);
    inventory.lock().unwrap().resources.push(item(
        ResourceKind::Build,
        "another-old",
        Some(old),
        false,
    ));
    sweep(&world, &host, 2, 2).await;
    assert_eq!(host.count(CommandKind::NodeResourcesRemove), 1);
    assert!(
        inventory
            .lock()
            .unwrap()
            .resources
            .iter()
            .any(|item| item.resource.id == "another-old")
    );
    let lines: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE object_id=$1 AND details->>'operation'='cleanup'",
    )
    .bind(host.id)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    assert_eq!(lines, 1);
    world.engine().stop();
}

#[tokio::test]
async fn retained_data_keeps_unknown_and_recorded_restore_dependencies_beyond_the_cache_window() {
    let world = World::new().await;
    hosting::serve(&world);
    let host = ScriptedHost::on(&world, "cache-restore", 16_384).await;
    let mut conn = world.pool.acquire().await.unwrap();
    let environment = orch_db::queries::resources::access_fixtures::insert_environment(
        &mut conn,
        world.ada,
        Some(world.root),
        "Kept workspace",
        "managed",
    )
    .await
    .unwrap();
    let runtime: Id = new_id();
    sqlx::query("INSERT INTO runtimes(id,kind,host_id,unit_name,environment_id,driver,template_id,spec_hash,state) SELECT $1,'environment',$2,$3,e.id,'test',e.template_id,'\\x00'::bytea,'removed' FROM environments e WHERE e.id=$4")
        .bind(runtime).bind(host.id).bind(format!("retained-{runtime}")).bind(environment).execute(&mut *conn).await.unwrap();
    sqlx::query("UPDATE environments SET status='removed',deleted_at=$2,removed_keep_days=14,volumes_purge_at=$3,removal_phase='kept',removal_revision=1 WHERE id=$1")
        .bind(environment).bind(world.clock.now()).bind(world.clock.now()+chrono::Duration::days(14)).execute(&mut *conn).await.unwrap();
    drop(conn);
    let now = Time::new(world.clock.now());
    let old = Time::new(now.get() - chrono::Duration::days(7));
    let inventory = Arc::new(Mutex::new(ResourceInventory {
        observed_at: now,
        unavailable: Vec::new(),
        resources: vec![
            item(ResourceKind::ToolVersion, "pinned", Some(old), false),
            item(ResourceKind::ToolVersion, "unused", Some(old), false),
        ],
    }));
    answer(&host, &inventory);
    sweep(&world, &host, 1, 1).await;
    assert_eq!(
        host.count(CommandKind::NodeResourcesRemove),
        0,
        "unknown restore references never prove a cache unused"
    );
    sqlx::query("UPDATE runtimes SET resources_used=$2 WHERE id=$1")
        .bind(runtime)
        .bind(json!([{"kind":"tool_version","id":"pinned"}]))
        .execute(world.pool.pool())
        .await
        .unwrap();
    sweep(&world, &host, 2, 2).await;
    let removed: Vec<_> = host
        .calls()
        .into_iter()
        .filter(|call| call.kind == CommandKind::NodeResourcesRemove)
        .map(|call| call.body["id"].clone())
        .collect();
    assert_eq!(removed, [json!("unused")]);
    assert!(
        inventory
            .lock()
            .unwrap()
            .resources
            .iter()
            .any(|item| item.resource.id == "pinned")
    );
    world.engine().stop();
}

#[tokio::test]
async fn cancellation_waits_for_the_active_irreversible_item_and_keeps_its_actual_result() {
    let world = World::new().await;
    hosting::serve(&world);
    let host = ScriptedHost::on(&world, "cancel-cache-cleanup", 16_384).await;
    let now = Time::new(world.clock.now());
    let used = Some(Time::new(now.get() - chrono::Duration::days(7)));
    let inventory = Arc::new(Mutex::new(ResourceInventory {
        observed_at: now,
        unavailable: Vec::new(),
        resources: vec![
            item(ResourceKind::ToolVersion, "one", used, false),
            item(ResourceKind::ToolVersion, "two", used, false),
        ],
    }));
    answer(&host, &inventory);
    host.added
        .machine
        .handler
        .hold(CommandKind::NodeResourcesRemove);
    host.added
        .machine
        .publish(
            &orch_link_contract::streams::Stream::Node,
            &orch_link_contract::events::Event::NodeKeptImages(
                orch_link_contract::events::NodeKeptImages {
                    epoch: 1,
                    images: Vec::new(),
                },
            ),
        )
        .await;
    host.wait_began(CommandKind::NodeResourcesRemove, 1).await;
    let id: Id = sqlx::query_scalar("SELECT id FROM jobs WHERE kind='kept_images_sweep' AND plan->>'host'=$1 ORDER BY created_at DESC LIMIT 1")
        .bind(host.id.to_string()).fetch_one(world.pool.pool()).await.unwrap();
    assert!(world.engine().cancel_job(id, world.ada).await.unwrap());
    let state: String = sqlx::query_scalar("SELECT state FROM jobs WHERE id=$1")
        .bind(id)
        .fetch_one(world.pool.pool())
        .await
        .unwrap();
    assert_eq!(
        state, "running",
        "the active native removal must settle before cancellation ends"
    );
    host.added.machine.handler.release();
    eventually("the cleanup cancellation settles", || async {
        sqlx::query_scalar::<_, String>("SELECT state FROM jobs WHERE id=$1")
            .bind(id)
            .fetch_one(world.pool.pool())
            .await
            .unwrap()
            == "cancelled"
    })
    .await;
    let (result, error): (serde_json::Value, serde_json::Value) =
        sqlx::query_as("SELECT result,error FROM jobs WHERE id=$1")
            .bind(id)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert_eq!(
        host.count(CommandKind::NodeResourcesRemove),
        1,
        "another irreversible removal never starts after cancel"
    );
    assert_eq!(result["cleanup"].as_array().unwrap().len(), 1);
    assert_eq!(result["cleanup"][0]["outcome"], "removed");
    assert_eq!(error["undone"], false, "deleted caches were not restored");
    assert_eq!(inventory.lock().unwrap().resources.len(), 1);
    let audit: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE object_id=$1 AND details->>'operation'='cleanup'",
    )
    .bind(host.id)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    assert_eq!(audit, 1);
}

#[tokio::test]
async fn resource_switch_off_interrupts_current_turns_without_holding_inputs_and_enable_can_hold_them()
 {
    use orch_access_contract::{decisions::ObjectRef, permissions::ObjectKind};
    use orch_api_contract::resource_facts::FactTarget;
    use orch_authz::Caller;
    use orch_link_contract::events::TurnOutcome;
    for kind in ["provider", "model", "agent", "environment"] {
        let world = World::new().await;
        let engine = world.engine();
        let thread = world.thread(kind).await;
        let (turn, _) = world.running_turn(thread, "A running request").await;
        let queued = engine
            .submit_input(
                &Caller::session(world.ada),
                thread,
                common::queued("Keep my place"),
            )
            .await
            .unwrap();
        let target = match kind {
            "agent" => FactTarget {
                object: ObjectRef {
                    kind: ObjectKind::Agent,
                    id: world.agent,
                },
                model_id: None,
            },
            "environment" => FactTarget {
                object: ObjectRef {
                    kind: ObjectKind::Environment,
                    id: world.environment,
                },
                model_id: None,
            },
            "model" => {
                let model: String = sqlx::query_scalar("SELECT model FROM turns WHERE id=$1")
                    .bind(turn.turn_id)
                    .fetch_one(world.pool.pool())
                    .await
                    .unwrap();
                let model_id: Id = sqlx::query_scalar(
                    "SELECT id FROM provider_models WHERE provider_id=$1 AND model_name=$2",
                )
                .bind(world.provider)
                .bind(model)
                .fetch_one(world.pool.pool())
                .await
                .unwrap();
                FactTarget {
                    object: ObjectRef {
                        kind: ObjectKind::Provider,
                        id: world.provider,
                    },
                    model_id: Some(model_id),
                }
            }
            _ => FactTarget {
                object: ObjectRef {
                    kind: ObjectKind::Provider,
                    id: world.provider,
                },
                model_id: None,
            },
        };
        let work = orch_engine::resource_facts::activity::read(
            &mut world.pool.acquire().await.unwrap(),
            &target,
        )
        .await
        .unwrap();
        assert_eq!(
            work.iter().filter(|row| row.kind == "running").count(),
            1,
            "{kind}"
        );
        assert_eq!(
            work.iter()
                .filter(|row| row.kind == "waiting")
                .map(|row| row.id)
                .collect::<Vec<_>>(),
            [queued.input.id]
        );
        let mut tx = world.pool.begin().await.unwrap();
        orch_db::queries::transaction_guard::lock_authority(&mut tx)
            .await
            .unwrap();
        let (stop, finished) = engine
            .stop_resource_in(
                &mut tx,
                orch_engine::resource_pause::ResourceStop {
                    target: target.clone(),
                    name: kind,
                    by: world.ada,
                    note: Some("Maintenance"),
                    deleted: false,
                },
                world.clock.now(),
            )
            .await
            .unwrap();
        match kind {
            "provider" => {
                sqlx::query("UPDATE providers SET status='disabled' WHERE id=$1")
                    .bind(world.provider)
                    .execute(&mut *tx)
                    .await
                    .unwrap();
            }
            "model" => {
                sqlx::query("UPDATE provider_models SET enabled=false WHERE id=$1")
                    .bind(target.model_id)
                    .execute(&mut *tx)
                    .await
                    .unwrap();
            }
            "agent" => {
                sqlx::query("UPDATE agents SET disabled=true WHERE id=$1")
                    .bind(world.agent)
                    .execute(&mut *tx)
                    .await
                    .unwrap();
            }
            _ => {
                sqlx::query("UPDATE environments SET status='disabled' WHERE id=$1")
                    .bind(world.environment)
                    .execute(&mut *tx)
                    .await
                    .unwrap();
            }
        }
        tx.commit().await.unwrap();
        for thread in finished.wakes {
            engine.wake(thread);
        }
        engine.stop_resource_now(stop.id).await.unwrap();
        world.wait_ran(CommandKind::TurnInterrupt, 1).await;
        let held: bool = sqlx::query_scalar("SELECT queue_held FROM thread_inputs WHERE id=$1")
            .bind(queued.input.id)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
        assert!(!held, "Disable must not hold the queue: {kind}");
        world
            .say(
                thread,
                common::turn_ended(turn.turn_id, TurnOutcome::Interrupted),
            )
            .await;
        world.wait_turn(turn.turn_id, "interrupted").await;
        assert_eq!(world.input_state(queued.input.id).await.0, "pending");
        let marker:serde_json::Value=sqlx::query_scalar("SELECT payload FROM items WHERE thread_id=$1 AND kind='marker' AND payload->>'marker'='resource_stopped'").bind(thread).fetch_one(world.pool.pool()).await.unwrap();
        assert_eq!(marker["data"]["by"]["id"], world.ada.to_string());
        assert_eq!(marker["data"]["note"], "Maintenance");
        assert_eq!(marker["data"]["name"], kind);
        let mut tx = world.pool.begin().await.unwrap();
        engine
            .hold_resource_waiting_in(&mut tx, &target, world.ada, world.clock.now())
            .await
            .unwrap();
        match kind {
            "provider" => {
                sqlx::query("UPDATE providers SET status='ready' WHERE id=$1")
                    .bind(world.provider)
                    .execute(&mut *tx)
                    .await
                    .unwrap();
            }
            "model" => {
                sqlx::query("UPDATE provider_models SET enabled=true WHERE id=$1")
                    .bind(target.model_id)
                    .execute(&mut *tx)
                    .await
                    .unwrap();
            }
            "agent" => {
                sqlx::query("UPDATE agents SET disabled=false WHERE id=$1")
                    .bind(world.agent)
                    .execute(&mut *tx)
                    .await
                    .unwrap();
            }
            _ => {
                sqlx::query("UPDATE environments SET status='running' WHERE id=$1")
                    .bind(world.environment)
                    .execute(&mut *tx)
                    .await
                    .unwrap();
            }
        }
        tx.commit().await.unwrap();
        let queue: bool = sqlx::query_scalar("SELECT queue_held FROM thread_inputs WHERE id=$1")
            .bind(queued.input.id)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
        assert!(
            queue,
            "Enable's off-default choice holds the original input: {kind}"
        );
        let revision: i64 =
            sqlx::query_scalar("SELECT queue_revision FROM threads WHERE node_id=$1")
                .bind(thread)
                .fetch_one(world.pool.pool())
                .await
                .unwrap();
        engine
            .resume_queue(&Caller::session(world.ada), thread, revision)
            .await
            .unwrap();
        let next = world.turn_start(thread, 1).await;
        assert_eq!(common::words_after_where(&next.text), "Keep my place");
        engine.stop();
    }
}

#[tokio::test]
async fn session_delivery_follows_native_acknowledgement_order_and_missing_legacy_ids_are_unknown()
{
    use orch_access_contract::{decisions::ObjectRef, permissions::ObjectKind};
    use orch_api_contract::resource_facts::FactSubject;
    use orch_db::queries::link::{Ending, NewCommand, finish_command, insert_command};
    let world = World::new().await;
    let thread = world.thread("Captured delivery").await;
    let (turn, _) = world.running_turn(thread, "Open a session").await;
    let segment = world.segment_of(thread).await;
    let old = new_id();
    let current = new_id();
    let first = new_id();
    let second = new_id();
    let at = world.clock.now();
    let first_body = json!({"thread_id":thread,"segment_id":segment,"mcp":[{"name":"same-name","server_id":old}],"skills":[]});
    let second_body = json!({"thread_id":thread,"segment_id":segment,"mcp":[{"name":"same-name","server_id":current}],"skills":[]});
    let mut conn = world.pool.acquire().await.unwrap();
    for (id, body) in [(first, &first_body), (second, &second_body)] {
        insert_command(
            &mut conn,
            &NewCommand {
                id,
                machine_id: world.machine_id,
                kind: "session.open",
                body,
                serial_key: None,
                deadline_at: at + chrono::Duration::minutes(2),
            },
            at,
        )
        .await
        .unwrap();
    }
    let result = json!({"native_session_ref":"captured"});
    finish_command(&mut conn, second, &Ending::Succeeded(&result), at)
        .await
        .unwrap();
    finish_command(&mut conn, first, &Ending::Succeeded(&result), at)
        .await
        .unwrap();
    let captured: serde_json::Value =
        sqlx::query_scalar("SELECT delivered_resources FROM segments WHERE id=$1")
            .bind(segment)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!(
        captured["mcp"],
        json!([current]),
        "an older delayed acknowledgement cannot rewrite accepted delivery at the same clock instant"
    );
    assert_eq!(captured["state"], "open");
    let close = new_id();
    let close_body = json!({"thread_id":thread,"segment_id":segment});
    insert_command(
        &mut conn,
        &NewCommand {
            id: close,
            machine_id: world.machine_id,
            kind: "session.close.fenced",
            body: &close_body,
            serial_key: None,
            deadline_at: at + chrono::Duration::minutes(2),
        },
        at,
    )
    .await
    .unwrap();
    finish_command(&mut conn, close, &Ending::Succeeded(&json!({})), at)
        .await
        .unwrap();
    assert!(
        !finish_command(&mut conn, second, &Ending::Succeeded(&result), at)
            .await
            .unwrap(),
        "replay is still one acknowledgement"
    );
    let closed: serde_json::Value =
        sqlx::query_scalar("SELECT delivered_resources FROM segments WHERE id=$1")
            .bind(segment)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!(closed["state"], "closed");
    let closed_facts = orch_engine::resource_facts::dependents(
        &mut conn,
        &[FactSubject::Resource {
            object: ObjectRef {
                kind: ObjectKind::McpServer,
                id: current,
            },
        }],
        Time::new(at),
    )
    .await
    .unwrap();
    assert!(closed_facts[0].dependents_complete);
    assert!(
        closed_facts[0].dependents.is_empty(),
        "a closed native session does not retain a current delivery attachment"
    );
    let legacy = new_id();
    let legacy_body =
        json!({"thread_id":thread,"segment_id":segment,"mcp":[{"name":"same-name"}],"skills":[]});
    insert_command(
        &mut conn,
        &NewCommand {
            id: legacy,
            machine_id: world.machine_id,
            kind: "session.open",
            body: &legacy_body,
            serial_key: None,
            deadline_at: at + chrono::Duration::minutes(2),
        },
        at,
    )
    .await
    .unwrap();
    finish_command(&mut conn, legacy, &Ending::Succeeded(&result), at)
        .await
        .unwrap();
    let unknown: serde_json::Value =
        sqlx::query_scalar("SELECT delivered_resources FROM segments WHERE id=$1")
            .bind(segment)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!(
        unknown["mcp"],
        serde_json::Value::Null,
        "a name cannot identify a historical server after replacement"
    );
    let facts = orch_engine::resource_facts::read(
        &mut conn,
        &[FactSubject::Resource {
            object: ObjectRef {
                kind: ObjectKind::McpServer,
                id: current,
            },
        }],
        Time::new(at),
    )
    .await
    .unwrap();
    assert!(
        !facts[0].dependents_complete,
        "unknown actual session delivery must not be reported as zero"
    );
    drop(conn);
    world
        .say(
            thread,
            common::turn_ended(
                turn.turn_id,
                orch_link_contract::events::TurnOutcome::Completed,
            ),
        )
        .await;
    world.engine().stop();
}

#[tokio::test]
async fn exact_skill_versions_and_the_aggregate_share_one_reported_source_without_invented_zeroes()
{
    use orch_access_contract::{decisions::ObjectRef, permissions::ObjectKind};
    use orch_api_contract::resource_facts::{ResourceUseKind, ResourceUses, UseCoverage};
    use orch_engine::resource_facts::{ReportedUses, UseRequest, reported_uses};
    struct Source {
        seen: Mutex<Vec<UseRequest>>,
        rows: Vec<(UseRequest, ResourceUses)>,
    }
    #[async_trait::async_trait]
    impl ReportedUses for Source {
        async fn read(
            &self,
            _conn: &mut orch_db::Conn,
            requested: &[UseRequest],
            _at: Time,
        ) -> Result<Vec<(UseRequest, ResourceUses)>, orch_engine::EngineError> {
            *self.seen.lock().unwrap() = requested.to_vec();
            Ok(self.rows.clone())
        }
    }
    let world = World::new().await;
    let object = ObjectRef {
        kind: ObjectKind::Skill,
        id: new_id(),
    };
    let aggregate = UseRequest {
        object,
        version_id: None,
        kind: ResourceUseKind::SkillInvocation,
    };
    let version = UseRequest {
        version_id: Some(new_id()),
        ..aggregate.clone()
    };
    let unknown = UseRequest {
        version_id: Some(new_id()),
        ..aggregate.clone()
    };
    let count = |all_time| ResourceUses {
        kind: ResourceUseKind::SkillInvocation,
        coverage: UseCoverage::Complete,
        last_used_at: Some(Time::new(world.clock.now())),
        all_time: Some(all_time),
        last_30_days: Some(all_time),
        people: Some(1),
        people_30_days: Some(1),
    };
    let source = Source {
        seen: Mutex::new(vec![]),
        rows: vec![
            (aggregate.clone(), count(2)),
            (version.clone(), count(1)),
            (
                UseRequest {
                    version_id: Some(new_id()),
                    ..aggregate.clone()
                },
                count(999),
            ),
        ],
    };
    let requested = vec![aggregate, version, unknown];
    let rows = reported_uses(
        &mut world.pool.acquire().await.unwrap(),
        &requested,
        Time::new(world.clock.now()),
        Some(&source),
    )
    .await
    .unwrap();
    assert_eq!(
        *source.seen.lock().unwrap(),
        requested,
        "one batched request carries every exact qualification"
    );
    assert_eq!(
        rows[0].1.all_time,
        Some(2),
        "the producer's distinct aggregate is not a client sum of versions"
    );
    assert_eq!(rows[1].1.all_time, Some(1));
    assert_eq!(rows[2].1.coverage, UseCoverage::Unrecorded);
    assert_eq!(
        rows[2].1.all_time, None,
        "another version cannot fill an unrecorded figure"
    );
    world.engine().stop();
}

#[tokio::test]
async fn an_acknowledged_harness_stop_closes_unknown_delivery_and_fences_a_late_open_acknowledgement()
 {
    use orch_access_contract::{decisions::ObjectRef, permissions::ObjectKind};
    use orch_api_contract::resource_facts::FactSubject;
    use orch_db::queries::link::{Ending, NewCommand, finish_command, insert_command};
    let world = World::new().await;
    let thread = world.thread("closed-delivery").await;
    let segment = world.segment_of(thread).await;
    let mut conn = world.pool.acquire().await.unwrap();
    sqlx::query(
        "UPDATE segments SET machine_id=$2,native_session_ref='legacy-session' WHERE id=$1",
    )
    .bind(segment)
    .bind(world.machine_id)
    .execute(&mut *conn)
    .await
    .unwrap();
    let at = world.clock.now();
    let command = new_id();
    let server = new_id();
    let body = json!({"thread_id":thread,"segment_id":segment,"mcp":[{"name":"known","server_id":server}],"skills":[]});
    insert_command(
        &mut conn,
        &NewCommand {
            id: command,
            machine_id: world.machine_id,
            kind: "session.open",
            body: &body,
            serial_key: None,
            deadline_at: at + chrono::Duration::minutes(2),
        },
        at,
    )
    .await
    .unwrap();
    orch_db::queries::session_dependencies::retire_harness(
        &mut conn,
        world.provider,
        world.agent,
        world.machine_id,
        at,
    )
    .await
    .unwrap();
    finish_command(
        &mut conn,
        command,
        &Ending::Succeeded(&json!({"native_session_ref":"late"})),
        at,
    )
    .await
    .unwrap();
    let captured: serde_json::Value =
        sqlx::query_scalar("SELECT delivered_resources FROM segments WHERE id=$1")
            .bind(segment)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!(
        captured["state"], "closed",
        "a stopped process cannot be resurrected by an older acknowledgement"
    );
    let facts = orch_engine::resource_facts::dependents(
        &mut conn,
        &[FactSubject::Resource {
            object: ObjectRef {
                kind: ObjectKind::McpServer,
                id: server,
            },
        }],
        Time::new(at),
    )
    .await
    .unwrap();
    assert!(
        facts[0].dependents_complete,
        "a confirmed closed session needs no guessed legacy server identity"
    );
    assert!(facts[0].dependents.is_empty());
    world.engine().stop();
}

#[tokio::test]
async fn operation_dependents_count_one_job_once_and_ignore_kept_cleanup_rows() {
    use orch_access_contract::{decisions::ObjectRef, permissions::ObjectKind};
    use orch_api_contract::resource_facts::{DependentRelation, FactSubject};
    use orch_db::queries::jobs::{self, NewJob};
    let world = World::new().await;
    let mut tx = world.pool.begin().await.unwrap();
    let now = world.clock.now();
    let runner = json!({"id":new_id(),"until":Time::new(now+chrono::Duration::hours(1))});
    let first = new_id();
    let target = json!({"kind":"environment","environment":world.environment,"revision":1});
    for (id, provider, environment, recipe) in [
        (
            first,
            Some(world.provider),
            Some(world.environment),
            json!({"runner":runner,"recipe":{"work":[{"choice":"purge","target":target},{"choice":"purge","target":target}]}}),
        ),
        (
            new_id(),
            None,
            None,
            json!({"runner":runner,"recipe":{"work":[{"choice":"keep","target":target}]}}),
        ),
    ] {
        let row = jobs::insert(
            &mut tx,
            &NewJob {
                id,
                kind: "storage_cleanup",
                provider_id: provider,
                environment_id: environment,
                node_id: None,
                created_by: Some(world.ada),
                plan: &recipe,
                progress: &json!({}),
            },
            now,
        )
        .await
        .unwrap();
        sqlx::query("UPDATE jobs SET state='running' WHERE id=$1")
            .bind(row.id)
            .execute(&mut *tx)
            .await
            .unwrap();
    }
    let profile = new_id();
    let plan = json!({"runner":runner,"quiesce":{"target":{"object":{"kind":"agent","id":world.agent},"model_id":null}}});
    jobs::insert(
        &mut tx,
        &NewJob {
            id: profile,
            kind: "resource_quiesce",
            provider_id: None,
            environment_id: None,
            node_id: None,
            created_by: Some(world.ada),
            plan: &plan,
            progress: &json!({}),
        },
        now,
    )
    .await
    .unwrap();
    sqlx::query("UPDATE jobs SET state='running' WHERE id=$1")
        .bind(profile)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let subjects = [
        (ObjectKind::Provider, world.provider),
        (ObjectKind::Environment, world.environment),
        (ObjectKind::Agent, world.agent),
    ]
    .map(|(kind, id)| FactSubject::Resource {
        object: ObjectRef { kind, id },
    });
    let facts = orch_engine::resource_facts::dependents(
        &mut world.pool.acquire().await.unwrap(),
        &subjects,
        Time::new(now),
    )
    .await
    .unwrap();
    for (fact, expected) in facts.iter().zip([first, first, profile]) {
        let operations: Vec<_> = fact
            .dependents
            .iter()
            .filter(|edge| edge.relation == DependentRelation::Operation)
            .collect();
        assert_eq!(
            operations.len(),
            1,
            "one accepted operation, not a count of recipe references"
        );
        assert_eq!(operations[0].key, format!("job:{expected}"));
    }
    world.engine().stop();
}

#[tokio::test]
async fn a_failed_common_purge_stays_purge_only_and_its_retry_keeps_the_original_requester() {
    use orch_access_contract::{decisions::ObjectRef, permissions::ObjectKind};
    use orch_api_contract::storage::{CleanupChoice, CleanupItem, CleanupPlan, StorageKind};
    use orch_db::queries::{environments::retained, jobs};
    use orch_engine::jobs::cleanup::{Recipe, Target, Work};
    let world = World::new().await;
    hosting::serve(&world);
    let host = ScriptedHost::on(&world, "purge-recovery", 16_384).await;
    host.added
        .machine
        .handler
        .on(CommandKind::NodeRuntimeRemove, |_| {
            Err(orch_link_contract::commands::CommandError::new(
                orch_link_contract::commands::ErrorCode::Conflict,
                "the fixture cannot confirm its volume removal",
            ))
        });
    let now = world.clock.now();
    let session = new_id();
    let mut tx = world.pool.begin().await.unwrap();
    sqlx::query("INSERT INTO sessions(id,user_id,refresh_hash,expires_at) VALUES($1,$2,$3,$4)")
        .bind(session)
        .bind(world.ada)
        .bind(session.as_bytes().as_slice())
        .bind(now + chrono::Duration::days(1))
        .execute(&mut *tx)
        .await
        .unwrap();
    let volumes = [retained::Volume {
        runtime_id: new_id(),
        environment_id: world.environment,
        host_id: host.id,
        driver: "test".into(),
        role: "workspace".into(),
        name: "kept-workspace".into(),
        storage: None,
    }];
    assert!(
        retained::freeze(
            &mut tx,
            world.environment,
            Some(world.ada),
            7,
            &volumes,
            now
        )
        .await
        .unwrap()
    );
    let revision = retained::removal(&mut tx, world.environment)
        .await
        .unwrap()
        .unwrap()
        .removal_revision;
    assert!(
        retained::begin_remove(&mut tx, world.environment, revision)
            .await
            .unwrap()
    );
    assert!(
        retained::keep_removed(&mut tx, world.environment, revision, now)
            .await
            .unwrap()
    );
    let before = retained::removal(&mut tx, world.environment)
        .await
        .unwrap()
        .unwrap();
    let id = format!("environment:{}", world.environment);
    let mut record = orch_engine::storage::record(
        id.clone(),
        StorageKind::EnvironmentFiles,
        "Kept workspace".into(),
        Time::new(now),
    );
    record.resource = Some(ObjectRef {
        kind: ObjectKind::Environment,
        id: world.environment,
    });
    record.until = before.volumes_purge_at.map(Time::new);
    let plan = CleanupPlan {
        id: new_id(),
        expires_at: Time::new(now + chrono::Duration::minutes(15)),
        confirmation_name: record.name.clone(),
        items: vec![CleanupItem {
            record,
            choices: vec![CleanupChoice::Keep, CleanupChoice::Purge],
            selected: CleanupChoice::Purge,
        }],
        people: vec![],
        other_people: 0,
        notify: false,
    };
    let recipe = Recipe {
        request: None,
        plan,
        work: vec![Work {
            id,
            choice: CleanupChoice::Purge,
            target: Some(Target::Environment {
                environment: world.environment,
                revision,
            }),
        }],
    };
    let job = jobs::insert(
        &mut tx,
        &jobs::NewJob {
            id: new_id(),
            kind: orch_engine::jobs::cleanup::KIND,
            environment_id: None,
            provider_id: None,
            node_id: None,
            created_by: Some(world.ada),
            plan: &json!({"recipe":recipe,"identity":{"session_id":session},"origin":orch_db::audit::Origin{as_admin:Some(world.ada),..Default::default()},"cleanup_results":[]}),
            progress: &serde_json::to_value(orch_engine::jobs::runner::initial_progress(
                &orch_engine::jobs::cleanup::PHASES,
            ))
            .unwrap(),
        },
        now,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    world.engine().kick();
    eventually(
        "the common cleanup records the failed native item",
        || async {
            jobs::job(&mut world.pool.acquire().await.unwrap(), job.id)
                .await
                .unwrap()
                .is_some_and(|job| !job.is_open())
        },
    )
    .await;
    let mut tx = world.pool.begin().await.unwrap();
    let completed = jobs::job(&mut tx, job.id).await.unwrap().unwrap();
    assert_eq!(
        completed.result.as_ref().unwrap()["cleanup"][0]["outcome"],
        "failed"
    );
    let kept = retained::removal(&mut tx, world.environment)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(kept.removal_phase.as_deref(), Some("purging"));
    assert_eq!(
        kept.purged_at, None,
        "a failed command never confirms removed data"
    );
    assert_eq!(
        kept.volumes_purge_at, before.volumes_purge_at,
        "failure never shortens the promised recovery date"
    );
    assert!(
        !retained::reserve(&mut tx, world.environment, revision, "restoring", now)
            .await
            .unwrap(),
        "possibly removed files cannot be offered as restorable"
    );
    let original = retained::started_purge(&mut tx, world.environment, revision)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(original.0, Some(world.ada));
    assert_eq!(original.1["identity"]["session_id"], session.to_string());
    let retry = orch_engine::jobs::retained::queue(
        &mut tx,
        world.environment,
        revision,
        true,
        None,
        serde_json::Value::Null,
        now,
    )
    .await
    .unwrap();
    assert_eq!(
        retry.created_by,
        Some(world.ada),
        "the existing reaper resumes the accepted person's purge"
    );
    assert_eq!(retry.plan["identity"]["session_id"], session.to_string());
    tx.rollback().await.unwrap();
    world.engine().stop();
}

#[tokio::test]
async fn recorded_turns_with_unknown_authors_never_report_zero_people_or_invent_todays_owner() {
    use orch_access_contract::{decisions::ObjectRef, permissions::ObjectKind};
    use orch_api_contract::resource_facts::FactSubject;
    let world = World::new().await;
    let at = Time::new(world.clock.now());
    let mut conn = world.pool.acquire().await.unwrap();
    for (author, when) in [
        (None, at.get() - chrono::Duration::days(40)),
        (Some(world.ada), at.get()),
    ] {
        sqlx::query("INSERT INTO usage_events(id,at,origin,scope,counted,org_id,project_id,topic_id,turn_id,provider_id,user_id,model) VALUES($1,$2,'harness','turn',false,$3,$4,$5,$6,$7,$8,'actual')")
            .bind(new_id()).bind(when).bind(world.root).bind(world.project).bind(world.topic).bind(new_id()).bind(world.provider).bind(author).execute(&mut *conn).await.unwrap();
    }
    let facts = orch_engine::resource_facts::read(
        &mut conn,
        &[FactSubject::Resource {
            object: ObjectRef {
                kind: ObjectKind::Provider,
                id: world.provider,
            },
        }],
        at,
    )
    .await
    .unwrap();
    let uses = facts[0].uses.as_ref().unwrap();
    assert_eq!(uses.all_time, Some(2));
    assert_eq!(uses.last_30_days, Some(1));
    assert_eq!(
        uses.people, None,
        "a missing historical author is not a known zero or today's resource owner"
    );
    assert_eq!(
        uses.people_30_days,
        Some(1),
        "complete captured identities in the current buckets remain countable"
    );
    assert_eq!(uses.last_used_at, Some(at));
    drop(conn);
    world.engine().stop();
}
