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
            preview.notify,
            "the blocking-operation default is shown before confirmation"
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
