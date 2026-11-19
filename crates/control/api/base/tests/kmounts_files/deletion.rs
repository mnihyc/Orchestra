//! Delete shares the cleanup plan without taking ownership of another person's environment or data.
use super::*;
use orch_api_contract::storage::{CleanupChoice, CleanupPlan, CleanupStart};
use orch_db::queries::resources::{access_fixtures as fixtures, mounts as rows};

pub(super) async fn plan(world: &World, person: &places::Who, mount: Id) -> CleanupPlan {
    world
        .app
        .call(MountDeletePreflight, &[&mount])
        .token(&person.token)
        .send()
        .await
        .ok()
        .into_full()
        .expect("the mount owner can read the native deletion plan")
}

pub(super) fn keep(plan: &CleanupPlan) -> CleanupStart {
    CleanupStart {
        plan_id: plan.id,
        choices: vec![],
        confirmation_name: None,
        notify: false,
    }
}

async fn location(world: &World, owner: &places::Who, home: Id, mount: Id) -> (Id, Id) {
    let mut conn = world.conn().await;
    let host =
        fixtures::insert_resource(&mut conn, "host", Some(owner.id()), Some(home), "Data node")
            .await
            .unwrap();
    let machine = sqlx::query_scalar("SELECT machine_id FROM hosts WHERE id=$1")
        .bind(host)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE hosts SET drivers=$2 WHERE id=$1").bind(host).bind(json!({"docker":{"data_storage":{"made_by_orch":"volume","existing_directory":true,"reason":null}}})).execute(&mut *conn).await.unwrap();
    rows::set_location(
        &mut conn,
        &rows::LocationRow {
            mount_id: mount,
            machine_id: machine,
            host_id: None,
            host_name: None,
            kind: "volume".into(),
            source: format!("orch-mount-{mount}"),
            storage: None,
            data_kind: None,
            created_by: owner.id(),
            created_at: world.now(),
        },
    )
    .await
    .unwrap();
    drop(conn);
    own_page(
        world,
        orch_access_contract::permissions::ResourceKind::Host,
        host,
        owner.id(),
    )
    .await;
    (host, machine)
}

#[tokio::test]
async fn deleting_a_mount_keeps_hidden_owned_data_and_running_attachments_until_the_next_start() {
    let world = World::new().await;
    let c = company(&world).await;
    let mount = named_mount(&world, &c.viewer, c.acme, "shared-datasets").await;
    let (host, machine) = location(&world, &c.admin, c.acme, mount.base.id).await;
    let environment = attached_on_host(
        &world,
        host,
        c.stranger.person.personal_org,
        c.stranger.id(),
        "Private running work",
        mount.base.id,
    )
    .await;
    let plan = plan(&world, &c.viewer, mount.base.id).await;
    assert_eq!(plan.items.len(), 1);
    assert_eq!(plan.items[0].selected, CleanupChoice::Keep);
    assert_eq!(plan.items[0].choices, [CleanupChoice::Keep]);
    let shown = serde_json::to_string(&plan).unwrap();
    for private in [
        "Private running work",
        "Data node",
        &machine.to_string(),
        &format!("orch-mount-{}", mount.base.id),
    ] {
        assert!(
            !shown.contains(private),
            "the mount owner does not learn hidden physical metadata"
        );
    }
    let body = keep(&plan);
    let accepted = world
        .app
        .call(MountDelete, &[&mount.base.id])
        .token(&c.viewer.token)
        .body(&body)
        .send()
        .await
        .ok();
    let repeated = world
        .app
        .call(MountDelete, &[&mount.base.id])
        .token(&c.viewer.token)
        .body(&body)
        .send()
        .await
        .ok();
    let id = |value: orch_api_contract::storage::CleanupJobResponse| match value {
        orch_api_contract::storage::CleanupJobResponse::Full(job) => job.id,
        orch_api_contract::storage::CleanupJobResponse::Receipt(receipt) => receipt.id,
    };
    let accepted = id(accepted);
    assert_eq!(
        accepted,
        id(repeated),
        "a lost response joins the same deletion"
    );
    let mut conn = world.conn().await;
    let kept = rows::kept(&mut conn, &[mount.base.id]).await.unwrap();
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].source, format!("orch-mount-{}", mount.base.id));
    assert!(
        rows::locations(&mut conn, &[mount.base.id])
            .await
            .unwrap()
            .is_empty()
    );
    let current: (String, serde_json::Value) =
        sqlx::query_as("SELECT status,facts FROM environments WHERE id=$1")
            .bind(environment)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!(current.0, "running");
    assert_eq!(
        current.1["mounts"][0]["state"], "attached",
        "a deletion cannot pretend a running mount was physically detached"
    );
    let commands: i64 =
        sqlx::query_scalar("SELECT count(*) FROM commands WHERE kind='node.volume.remove'")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!(
        commands, 0,
        "keeping data needs neither the other person's permission nor a node reply"
    );
    drop(conn);
    world.app.app.engine.start();
    world.app.app.engine.kick();
    let completed = closed(&world, accepted).await;
    world.app.app.engine.stop();
    assert_eq!(
        completed.result.as_ref().unwrap()["cleanup"][0]["outcome"],
        "kept",
        "an explicitly kept volume is kept even when its destructive fate was unavailable"
    );
    let history: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE object_id=$1 AND action IN ('mount.deleted','storage.cleaned')")
        .bind(mount.base.id).fetch_one(&world.db.pool).await.unwrap();
    assert_eq!(
        history, 1,
        "the shared data job does not add a second deletion history line"
    );
}

#[tokio::test]
async fn a_location_replaced_at_the_same_clock_cannot_be_destroyed_by_an_old_mount_deletion_plan() {
    for replaced_in_place in [false, true] {
        let world = World::new().await;
        let c = company(&world).await;
        let mount = named_mount(&world, &c.viewer, c.acme, "datasets").await;
        let (_, machine) = location(&world, &c.viewer, c.acme, mount.base.id).await;
        let preview = plan(&world, &c.viewer, mount.base.id).await;
        let mut conn = world.conn().await;
        let old = rows::locations(&mut conn, &[mount.base.id])
            .await
            .unwrap()
            .pop()
            .unwrap();
        if replaced_in_place {
            for source in ["replacement-volume", old.source.as_str()] {
                sqlx::query(
                    "UPDATE mount_locations SET source=$3 WHERE mount_id=$1 AND machine_id=$2",
                )
                .bind(mount.base.id)
                .bind(machine)
                .bind(source)
                .execute(&mut *conn)
                .await
                .unwrap();
            }
        } else {
            rows::remove_location(&mut conn, mount.base.id, machine)
                .await
                .unwrap();
            rows::set_location(&mut conn, &old).await.unwrap();
        }
        drop(conn);
        let refused = world
            .app
            .call(MountDelete, &[&mount.base.id])
            .token(&c.viewer.token)
            .body(&keep(&preview))
            .send()
            .await
            .problem();
        assert_eq!(refused.code, ErrorCode::PreflightExpired);
        assert_eq!(
            world
                .app
                .call(MountGet, &[&mount.base.id])
                .token(&c.viewer.token)
                .send()
                .await
                .ok()
                .revision,
            mount.revision
        );
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='storage_cleanup'")
                .fetch_one(&world.db.pool)
                .await
                .unwrap();
        assert_eq!(count, 0, "a stale physical membership changes nothing");
    }
}

#[tokio::test]
async fn deleting_owned_mount_data_requires_its_name_and_keeps_the_original_native_deletion_operation()
 {
    let world = World::new().await;
    let c = company(&world).await;
    let mount = named_mount(&world, &c.viewer, c.acme, "datasets").await;
    location(&world, &c.viewer, c.acme, mount.base.id).await;
    let preview = plan(&world, &c.viewer, mount.base.id).await;
    let item = preview.items.first().unwrap();
    assert_eq!(item.selected, CleanupChoice::Keep);
    assert!(item.choices.contains(&CleanupChoice::Purge));
    let mut body = keep(&preview);
    body.choices
        .push(orch_api_contract::storage::CleanupSelection {
            id: item.record.id.clone(),
            choice: CleanupChoice::Purge,
        });
    body.confirmation_name = Some("wrong".into());
    assert_eq!(
        world
            .app
            .call(MountDelete, &[&mount.base.id])
            .token(&c.viewer.token)
            .body(&body)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::RequestInvalid
    );
    assert_eq!(
        rows::locations(&mut *world.conn().await, &[mount.base.id])
            .await
            .unwrap()
            .len(),
        1
    );
    body.confirmation_name = Some(preview.confirmation_name);
    let accepted = world
        .app
        .call(MountDelete, &[&mount.base.id])
        .token(&c.viewer.token)
        .body(&body)
        .send()
        .await
        .ok();
    let job = match accepted {
        orch_api_contract::storage::CleanupJobResponse::Full(job) => job.id,
        orch_api_contract::storage::CleanupJobResponse::Receipt(receipt) => receipt.id,
    };
    let row = orch_db::queries::jobs::job(&mut *world.conn().await, job)
        .await
        .unwrap()
        .unwrap();
    let retained = row.plan["cleanup_operation"]["origin"]["operation"].clone();
    assert!(
        serde_json::to_string(&retained)
            .unwrap()
            .contains("MountDelete")
    );
    assert!(
        !serde_json::to_string(&retained)
            .unwrap()
            .contains("StorageCleanupCreate")
    );
    assert_eq!(row.plan["recipe"]["work"][0]["choice"], "purge");
    assert_eq!(
        rows::kept(&mut *world.conn().await, &[mount.base.id])
            .await
            .unwrap()
            .len(),
        1,
        "the data is retained before any node command can run and remains if native cleanup fails"
    );
}

#[tokio::test]
async fn a_mount_deletion_notifies_once_per_person_only_when_the_actor_selects_it() {
    for notify in [false, true] {
        let world = World::new().await;
        let c = company(&world).await;
        let mount = named_mount(&world, &c.viewer, c.acme, "shared-datasets").await;
        let (host, _) = location(&world, &c.admin, c.acme, mount.base.id).await;
        for name in ["Private first work", "Private second work"] {
            attached_on_host(
                &world,
                host,
                c.stranger.person.personal_org,
                c.stranger.id(),
                name,
                mount.base.id,
            )
            .await;
        }
        let preview = plan(&world, &c.viewer, mount.base.id).await;
        assert_eq!(
            preview.people.len() as i64 + preview.other_people,
            1,
            "two affected environments of the same person make one eligible recipient"
        );
        assert!(
            !preview.notify,
            "the person chooses whether to notify anyone"
        );
        let mut body = keep(&preview);
        body.notify = notify;
        world
            .app
            .call(MountDelete, &[&mount.base.id])
            .token(&c.viewer.token)
            .body(&body)
            .send()
            .await
            .ok();
        world
            .app
            .call(MountDelete, &[&mount.base.id])
            .token(&c.viewer.token)
            .body(&body)
            .send()
            .await
            .ok();
        let actual: i64 = sqlx::query_scalar("SELECT count(*) FROM notifications WHERE user_id=$1 AND data->>'operation_kind'='mount_delete'")
            .bind(c.stranger.id()).fetch_one(&world.db.pool).await.unwrap();
        assert_eq!(
            actual,
            i64::from(notify),
            "a retry cannot create another person's second Inbox item"
        );
    }
}

struct DataMachine;
#[async_trait::async_trait]
impl orch_api_core::testing::Machine for DataMachine {
    async fn run(
        &self,
        command: &orch_link_contract::commands::Cmd,
    ) -> Result<serde_json::Value, orch_link_contract::commands::CommandError> {
        if command.kind == "node.volume.remove" {
            return Ok(json!({}));
        }
        Err(orch_link_contract::commands::CommandError::new(
            orch_link_contract::commands::ErrorCode::Conflict,
            "the data fixture accepts only the confirmed volume removal",
        ))
    }
}

pub(super) async fn closed(world: &World, id: Id) -> orch_db::queries::jobs::JobRow {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let row = orch_db::queries::jobs::job(&mut *world.conn().await, id)
                .await
                .unwrap()
                .unwrap();
            if !row.is_open() {
                break row;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the native cleanup job must retain its factual outcome")
}

#[tokio::test]
async fn mount_data_is_removed_only_by_the_original_deletion_and_current_physical_management() {
    use orch_link_contract::commands::CommandKind;
    for withdraw in [false, true] {
        let world = World::new().await;
        let c = company(&world).await;
        let mount = named_mount(&world, &c.viewer, c.acme, "datasets").await;
        let (host, machine) = location(&world, &c.admin, c.acme, mount.base.id).await;
        sqlx::query("UPDATE machines SET online=true WHERE id=$1")
            .bind(machine)
            .execute(&world.db.pool)
            .await
            .unwrap();
        let grant = name_person(&world, "host", host, "host.manage", c.viewer.id()).await;
        let preview = plan(&world, &c.viewer, mount.base.id).await;
        let mut body = keep(&preview);
        body.choices
            .push(orch_api_contract::storage::CleanupSelection {
                id: preview.items[0].record.id.clone(),
                choice: CleanupChoice::Purge,
            });
        body.confirmation_name = Some(preview.confirmation_name);
        let accepted = world
            .app
            .call(MountDelete, &[&mount.base.id])
            .token(&c.viewer.token)
            .body(&body)
            .send()
            .await
            .ok();
        let id = match accepted {
            orch_api_contract::storage::CleanupJobResponse::Full(job) => job.id,
            orch_api_contract::storage::CleanupJobResponse::Receipt(receipt) => receipt.id,
        };
        if withdraw {
            sqlx::query("UPDATE permission_entries SET removed_at=$2 WHERE id=$1")
                .bind(grant)
                .bind(world.now())
                .execute(&world.db.pool)
                .await
                .unwrap();
        }
        let played = world.app.play(machine, std::sync::Arc::new(DataMachine));
        world.app.app.engine.start();
        world.app.app.engine.kick();
        let row = closed(&world, id).await;
        world.app.app.engine.stop();
        let actual = &row.result.as_ref().unwrap()["cleanup"][0]["outcome"];
        let kept = rows::kept(&mut *world.conn().await, &[mount.base.id])
            .await
            .unwrap();
        if withdraw {
            assert_eq!(actual, "failed");
            assert!(played.seen_of(CommandKind::NodeVolumeRemove).is_empty());
            assert_eq!(kept.len(), 1);
        } else {
            assert_eq!(actual, "removed");
            assert_eq!(played.seen_of(CommandKind::NodeVolumeRemove).len(), 1);
            assert!(kept.is_empty());
        }
    }
}

#[tokio::test]
async fn a_mount_deletion_plan_cannot_be_borrowed_by_storage_cleanup_or_another_mount() {
    let world = World::new().await;
    let c = company(&world).await;
    let mount = named_mount(&world, &c.viewer, c.acme, "first-datasets").await;
    let other = named_mount(&world, &c.viewer, c.acme, "second-datasets").await;
    let preview = plan(&world, &c.viewer, mount.base.id).await;
    let body = keep(&preview);
    assert_eq!(
        world
            .app
            .call(orch_api_contract::storage::StorageCleanupCreate, &[])
            .token(&c.viewer.token)
            .body(&body)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::NotFound
    );
    assert_eq!(
        world
            .app
            .call(MountDelete, &[&other.base.id])
            .token(&c.viewer.token)
            .body(&body)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::NotFound
    );
    assert_eq!(
        world
            .app
            .call(MountGet, &[&mount.base.id])
            .token(&c.viewer.token)
            .send()
            .await
            .ok()
            .revision,
        mount.revision
    );
    assert_eq!(
        world
            .app
            .call(MountGet, &[&other.base.id])
            .token(&c.viewer.token)
            .send()
            .await
            .ok()
            .revision,
        other.revision
    );
}

#[tokio::test]
async fn deleting_a_mount_whose_only_machine_is_lost_keeps_its_data_record_in_storage() {
    use orch_api_contract::storage::{StorageAvailability, StorageGet, StorageKind, StorageQuery};
    let world = World::new().await;
    let c = company(&world).await;
    let mount = named_mount(&world, &c.viewer, c.acme, "lost-datasets").await;
    let (_, machine) = location(&world, &c.admin, c.acme, mount.base.id).await;
    sqlx::query("UPDATE machines SET status='revoked',lost_at=$2 WHERE id=$1")
        .bind(machine)
        .bind(world.now())
        .execute(&world.db.pool)
        .await
        .unwrap();
    let preview = plan(&world, &c.viewer, mount.base.id).await;
    assert_eq!(preview.items.len(), 1);
    assert_eq!(preview.items[0].choices, [CleanupChoice::Keep]);
    assert_eq!(
        preview.items[0].record.availability,
        StorageAvailability::Unknown
    );
    world
        .app
        .call(MountDelete, &[&mount.base.id])
        .token(&c.viewer.token)
        .body(&keep(&preview))
        .send()
        .await
        .ok();
    let stored = world
        .app
        .call(StorageGet, &[])
        .token(&c.viewer.token)
        .query(&StorageQuery {
            resource_kind: Some(orch_access_contract::permissions::ObjectKind::Mount),
            resource_id: Some(mount.base.id),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert!(
        stored
            .items
            .iter()
            .any(|row| row.kind == StorageKind::MountData
                && row.availability == StorageAvailability::Unknown
                && !row.can_clean),
        "deleting the definition does not erase the last known custody record or invent a reachable cleanup"
    );
    let commands: i64 =
        sqlx::query_scalar("SELECT count(*) FROM commands WHERE kind='node.volume.remove'")
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(commands, 0);
}

#[tokio::test]
async fn a_copy_can_keep_its_data_while_mount_deletion_waits_to_read_that_location() {
    let world = World::new().await;
    let c = company(&world).await;
    let mount = named_mount(&world, &c.viewer, c.acme, "datasets").await;
    let (_, machine) = location(&world, &c.viewer, c.acme, mount.base.id).await;
    let mut copy = world.db.pool.begin().await.unwrap();
    let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *copy)
        .await
        .unwrap();
    sqlx::query(
        "SELECT mount_id FROM mount_locations WHERE mount_id=$1 AND machine_id=$2 FOR UPDATE",
    )
    .bind(mount.base.id)
    .bind(machine)
    .fetch_one(&mut *copy)
    .await
    .unwrap();
    let app = world.app.clone();
    let secret = c.viewer.token.clone();
    let id = mount.base.id;
    let pending = async move {
        app.call(MountDeletePreflight, &[&id])
            .token(&secret)
            .send_bytes()
            .await
    };
    tokio::pin!(pending);
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        tokio::select! {
            _ = &mut pending => panic!("the preview must reach the location held by the copy before answering"),
            () = async {
                loop {
                    let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%ORDER BY membership_id FOR UPDATE%')")
                        .bind(blocker).fetch_one(&world.db.pool).await.unwrap();
                    if waiting { break; }
                    tokio::task::yield_now().await;
                }
            } => {}
        }
    }).await.expect("the actual native location wait must be observed");
    rows::keep(
        &mut copy,
        &rows::KeptRow {
            id: orch_types::new_id(),
            mount_id: mount.base.id,
            machine_id: machine,
            kind: "volume".into(),
            source: format!("orch-mount-{}", mount.base.id),
            storage: None,
            data_kind: None,
            file_count: None,
            total_bytes: None,
            job_id: None,
            kept_by: c.viewer.id(),
            kept_at: world.now(),
        },
    )
    .await
    .expect("the mount preview must not block the copy's custody foreign key");
    copy.commit().await.unwrap();
    let reply = pending.await;
    assert!(
        reply.status.is_success(),
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
}

#[tokio::test]
async fn deleting_a_mount_requires_access_to_every_enabled_place_and_only_names_visible_refusals() {
    let world = World::new().await;
    let c = company(&world).await;
    let mount = named_mount(&world, &c.viewer, c.acme, "used-mount")
        .await
        .base
        .id;
    let hidden = c.stranger.person.personal_org;
    for node in [c.viewer.person.personal_org, c.acme, hidden] {
        world
            .set_at(
                orch_settings::Target::Node(node),
                "env.mounts",
                json!({ mount.to_string(): true }),
            )
            .await;
    }
    let preview = plan(&world, &c.viewer, mount).await;
    let blocked = preview
        .mount_switch_off_blocked
        .as_ref()
        .expect("review names the inaccessible settings");
    assert_eq!(
        blocked
            .visible
            .iter()
            .map(|row| row.name.as_str())
            .collect::<Vec<_>>(),
        ["Acme"]
    );
    assert_eq!((blocked.count, blocked.hidden), (2, 1));
    assert!(
        !serde_json::to_string(&preview)
            .unwrap()
            .contains(&hidden.to_string())
    );
    let problem = world
        .app
        .call(MountDelete, &[&mount])
        .token(&c.viewer.token)
        .body(&keep(&preview))
        .send()
        .await
        .problem();
    assert_eq!(problem.code, ErrorCode::AuthzDenied);
    assert!(problem.detail.contains("Acme"));
    assert!(
        !serde_json::to_string(&problem)
            .unwrap()
            .contains(&hidden.to_string())
    );
    let remaining: i64 =
        sqlx::query_scalar("SELECT count(*) FROM settings WHERE key='env.mounts' AND value ? $1")
            .bind(mount.to_string())
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(remaining, 3);
    world
        .app
        .call(MountGet, &[&mount])
        .token(&c.viewer.token)
        .send()
        .await
        .ok();
}

#[tokio::test]
async fn deleting_a_mount_requires_another_review_when_a_new_place_enables_it() {
    let world = World::new().await;
    let c = company(&world).await;
    let mount = named_mount(&world, &c.boss, c.acme, "growing-use")
        .await
        .base
        .id;
    let before = plan(&world, &c.boss, mount).await;
    world
        .set_at(
            orch_settings::Target::Node(c.acme),
            "env.mounts",
            json!({ mount.to_string(): "private" }),
        )
        .await;
    let refused = world
        .app
        .call(MountDelete, &[&mount])
        .token(&c.boss.token)
        .body(&keep(&before))
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::PreflightExpired);
    let fresh = plan(&world, &c.boss, mount).await;
    world
        .app
        .call(MountDelete, &[&mount])
        .token(&c.boss.token)
        .body(&keep(&fresh))
        .send()
        .await
        .ok();
}

#[tokio::test]
async fn a_delayed_setting_write_cannot_enable_a_mount_after_its_reviewed_deletion() {
    let world = World::new().await;
    let c = company(&world).await;
    let mount = named_mount(&world, &c.boss, c.acme, "retired-data")
        .await
        .base
        .id;
    let other = named_mount(&world, &c.boss, c.acme, "other-data")
        .await
        .base
        .id;
    world
        .set_at(
            orch_settings::Target::Node(c.acme),
            "env.mounts",
            json!({other.to_string():"private"}),
        )
        .await;
    let preview = plan(&world, &c.boss, mount).await;
    world
        .app
        .call(MountDelete, &[&mount])
        .token(&c.boss.token)
        .body(&keep(&preview))
        .send()
        .await
        .ok();
    let failure =
        sqlx::query("UPDATE settings SET value=value || $2 WHERE node_id=$1 AND key='env.mounts'")
            .bind(c.acme)
            .bind(json!({mount.to_string():true}))
            .execute(&world.db.pool)
            .await
            .unwrap_err();
    assert_eq!(
        failure.as_database_error().and_then(|e| e.constraint()),
        Some("mount_use_requires_live_mount")
    );
}

#[tokio::test]
async fn losing_permission_on_a_reviewed_topic_prevents_the_mount_and_other_uses_from_being_removed()
 {
    let world = World::new().await;
    let c = company(&world).await;
    let project = places::make_project(&world, &c.boss, c.acme, "Build lab").await;
    let topic = places::make_topic(&world, &c.boss, project.base.id, "Compiler")
        .await
        .base
        .id;
    let mount = named_mount(&world, &c.viewer, c.acme, "tool-cache")
        .await
        .base
        .id;
    name_person(&world, "node", topic, "topic.view", c.viewer.id()).await;
    let grant = name_person(&world, "node", topic, "topic.manage", c.viewer.id()).await;
    for id in [c.viewer.person.personal_org, topic] {
        world
            .set_at(
                orch_settings::Target::Node(id),
                "env.mounts",
                json!({mount.to_string(): true}),
            )
            .await;
    }
    let initial = plan(&world, &c.viewer, mount).await;
    assert!(initial.mount_switch_off_blocked.is_none());
    let mut tx = world.db.pool.begin().await.unwrap();
    sqlx::query("DELETE FROM permission_entries WHERE id=$1")
        .bind(grant)
        .execute(&mut *tx)
        .await
        .unwrap();
    orch_db::queries::access::bump_epoch(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    let denied = world
        .app
        .call(MountDelete, &[&mount])
        .token(&c.viewer.token)
        .body(&keep(&initial))
        .send()
        .await
        .problem();
    assert_eq!(denied.code, ErrorCode::AuthzDenied);
    assert!(denied.detail.contains("Compiler"));
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM settings WHERE key='env.mounts' AND value ? $1")
            .bind(mount.to_string())
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(count, 2);
}

#[tokio::test]
async fn a_topic_manager_receives_one_notice_only_if_the_mount_deleter_selects_notify() {
    for notify in [false, true] {
        let world = World::new().await;
        let c = company(&world).await;
        let mount = named_mount(&world, &c.boss, c.acme, "team-tools")
            .await
            .base
            .id;
        let project = places::make_project(&world, &c.boss, c.acme, "Build lab").await;
        for name in ["First lab", "Second lab"] {
            let topic = places::make_topic(&world, &c.boss, project.base.id, name)
                .await
                .base
                .id;
            name_person(&world, "node", topic, "topic.manage", c.viewer.id()).await;
            world
                .set_at(
                    orch_settings::Target::Node(topic),
                    "env.mounts",
                    json!({mount.to_string():true}),
                )
                .await;
        }
        let reviewed = plan(&world, &c.boss, mount).await;
        assert!(!reviewed.notify);
        assert!(
            reviewed
                .people
                .iter()
                .any(|person| person.id == c.viewer.id())
        );
        let mut selection = keep(&reviewed);
        selection.notify = notify;
        world
            .app
            .call(MountDelete, &[&mount])
            .token(&c.boss.token)
            .body(&selection)
            .send()
            .await
            .ok();
        let count:i64=sqlx::query_scalar("SELECT count(*) FROM notifications WHERE user_id=$1 AND data->>'operation_kind'='mount_delete'")
            .bind(c.viewer.id()).fetch_one(&world.db.pool).await.unwrap();
        assert_eq!(count, i64::from(notify));
    }
}
