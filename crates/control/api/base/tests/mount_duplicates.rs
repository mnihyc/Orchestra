//! Mount copies use actual source View, destination creation, fresh defaults and native retry identities.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]
mod places;
mod resources;
mod support;
mod thread_history_scope;
use orch_api_contract::{
    access::NewResourceAccess,
    duplicates::{DuplicateBody, DuplicateDestination, MountDuplicate},
    mounts::{MountCreate, MountCreateBody, MountFate},
};
use orch_api_core::testing::RawReply;
use orch_db::queries::resources::mounts as rows;
use orch_types::{Id, Json, new_id};
use serde_json::json;
use support::World;
use thread_history_scope::{Proof, key, ok, rule};

struct Scene {
    world: World,
    company: resources::Company,
    source: Id,
}
impl Scene {
    async fn new() -> Proof<Self> {
        let world = World::new().await;
        let company = resources::company(&world).await;
        let source = world
            .app
            .call(MountCreate, &[])
            .token(&company.boss.token)
            .body(&MountCreateBody {
                name: "original-data".into(),
                description: Some("Actual source description".into()),
                home_id: Some(company.acme),
                target: "/data/original".into(),
                read_only: Some(false),
                default_fate: Some(MountFate::Copy),
                access: Some(NewResourceAccess::OnlyMe),
            })
            .send()
            .await
            .ok()
            .into_full()
            .map_err(|_| "source must have its ordinary full result")?
            .base
            .id;
        Ok(Self {
            world,
            company,
            source,
        })
    }
    async fn copy(&self, token: &str, body: &DuplicateBody) -> RawReply {
        self.world
            .app
            .call(MountDuplicate, &[&self.source])
            .token(token)
            .body(body)
            .send_bytes()
            .await
    }
    async fn row(&self, id: Id) -> Proof<rows::MountRow> {
        rows::mounts(&mut *self.world.conn().await, &[id])
            .await?
            .pop()
            .ok_or_else(|| "created mount missing".into())
    }
    async fn count(&self) -> Proof<i64> {
        Ok(sqlx::query_scalar("SELECT count(*) FROM mounts")
            .fetch_one(&self.world.db.pool)
            .await?)
    }
}
fn identity(reply: &RawReply) -> Proof<Id> {
    let value = ok(reply)?;
    let id: Id = serde_json::from_value(value["id"].clone())?;
    assert_eq!(
        value,
        json!({"projection":"receipt","kind":"mount","id":id})
    );
    Ok(id)
}

#[tokio::test]
async fn same_org_mount_duplicates_copy_configuration_and_tags_but_start_with_fresh_destination_privileges()
-> Proof {
    let s = Scene::new().await?;
    let department = places::make_department(
        &s.world,
        &s.company.boss,
        s.company.acme,
        "Destination",
        "destination",
    )
    .await
    .base
    .id;

    resources::name_person(
        &s.world,
        "mount",
        s.source,
        "mount.view",
        s.company.viewer.id(),
    )
    .await;
    resources::name_person(
        &s.world,
        "mount",
        s.source,
        "mount.manage",
        s.company.stranger.id(),
    )
    .await;
    let tag = new_id();
    sqlx::query("INSERT INTO tags(id,node_id,name,color) VALUES($1,$2,'Copied tag',1)")
        .bind(tag)
        .bind(s.company.acme)
        .execute(&s.world.db.pool)
        .await?;
    sqlx::query(
        "INSERT INTO resource_tags(resource_kind,resource_id,tag_id) VALUES('mount',$1,$2)",
    )
    .bind(s.source)
    .bind(tag)
    .execute(&s.world.db.pool)
    .await?;
    let first = identity(
        &s.copy(&s.company.viewer.token, &DuplicateBody::default())
            .await,
    )?;
    let second = identity(
        &s.copy(
            &s.company.viewer.token,
            &DuplicateBody {
                destination: Some(DuplicateDestination::Node { id: department }),
                ..Default::default()
            },
        )
        .await,
    )?;
    for (id, name, home) in [
        (first, "original-data-copy", s.company.acme),
        (second, "original-data-copy-2", department),
    ] {
        assert_ne!(id, s.source);
        let made = s.row(id).await?;
        assert_eq!(
            (made.name.as_str(), made.home_id, made.org_id, made.owner_id),
            (
                name,
                Some(home),
                Some(s.company.acme),
                Some(s.company.viewer.id())
            )
        );
        assert_eq!(
            (
                made.target.as_str(),
                made.description.as_str(),
                made.read_only,
                made.default_fate.as_str()
            ),
            ("/data/original", "Actual source description", false, "copy")
        );
        let tags: Vec<Id> = sqlx::query_scalar(
            "SELECT tag_id FROM resource_tags WHERE resource_kind='mount' AND resource_id=$1",
        )
        .bind(id)
        .fetch_all(&s.world.db.pool)
        .await?;
        assert_eq!(tags, vec![tag]);
        let old_entries:i64=sqlx::query_scalar("SELECT count(*) FROM permission_entries WHERE object_kind='mount' AND object_id=$1 AND holder_id=$2 AND removed_at IS NULL").bind(id).bind(s.company.stranger.id()).fetch_one(&s.world.db.pool).await?;
        assert_eq!(
            old_entries, 0,
            "a manually named source manager is not copied"
        );
        let owners:Vec<Id>=sqlx::query_scalar("SELECT holder_id FROM permission_entries WHERE object_kind='mount' AND object_id=$1 AND permission='access.own' AND removed_at IS NULL ORDER BY holder_id").bind(id).fetch_all(&s.world.db.pool).await?;
        assert_eq!(owners, vec![s.company.viewer.id()]);
        let locations: i64 =
            sqlx::query_scalar("SELECT count(*) FROM mount_locations WHERE mount_id=$1")
                .bind(id)
                .fetch_one(&s.world.db.pool)
                .await?;
        assert_eq!(
            locations, 0,
            "configuration duplication creates no copy of a physical location"
        );
        let audits: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='mount.created'",
        )
        .bind(id)
        .fetch_one(&s.world.db.pool)
        .await?;
        assert_eq!(audits, 1);
    }
    assert_eq!(s.row(s.source).await?.name, "original-data");
    Ok(())
}

#[tokio::test]
async fn duplicate_admits_its_own_source_read_and_destination_write_and_every_native_gate() -> Proof
{
    let s = Scene::new().await?;
    let read = || rule("MountDuplicate", "mount", s.source, "self", "read");
    let write = || rule("MountDuplicate", "org", s.company.acme, "self", "write");
    for rules in [
        vec![read()],
        vec![write()],
        vec![rule("MountGet", "mount", s.source, "self", "read"), write()],
        vec![
            read(),
            rule("MountCreate", "org", s.company.acme, "self", "manage"),
        ],
        vec![
            rule("MountDuplicate", "mount", new_id(), "self", "read"),
            write(),
        ],
    ] {
        let token = key(&s.world, s.company.boss.id(), rules).await?;
        assert_eq!(
            s.copy(&token.secret, &DuplicateBody::default())
                .await
                .status
                .as_u16(),
            403
        );
        assert_eq!(s.count().await?, 1);
    }
    let token = key(&s.world, s.company.boss.id(), vec![read(), write()]).await?;
    let created = identity(&s.copy(&token.secret, &DuplicateBody::default()).await)?;
    assert_eq!(s.row(created).await?.name, "original-data-copy");
    assert_eq!(s.count().await?, 2);
    assert!(
        !s.copy(&s.company.viewer.token, &DuplicateBody::default())
            .await
            .status
            .is_success(),
        "keys never grant ordinary source visibility"
    );
    resources::name_person(
        &s.world,
        "mount",
        s.source,
        "mount.view",
        s.company.viewer.id(),
    )
    .await;
    s.world
        .set_at(
            orch_settings::Target::Node(s.company.acme),
            "resources.created_by_members",
            json!([]),
        )
        .await;
    assert_eq!(
        s.copy(&s.company.viewer.token, &DuplicateBody::default())
            .await
            .status
            .as_u16(),
        403
    );
    assert_eq!(s.count().await?, 2);
    let stale = s
        .copy(
            &s.company.boss.token,
            &DuplicateBody {
                expected_revision: Some(0),
                ..Default::default()
            },
        )
        .await;
    assert_eq!(
        stale.problem().code,
        orch_api_contract::common::ErrorCode::ConflictStaleRevision
    );
    assert_eq!(s.count().await?, 2);
    Ok(())
}

#[tokio::test]
async fn duplicate_default_names_respect_retained_names_and_retry_replays_only_one_new_identity()
-> Proof {
    let s = Scene::new().await?;
    let first = identity(
        &s.copy(&s.company.boss.token, &DuplicateBody::default())
            .await,
    )?;
    sqlx::query("UPDATE mounts SET deleted_at=$2 WHERE id=$1")
        .bind(first)
        .bind(s.world.now())
        .execute(&s.world.db.pool)
        .await?;
    let request_key = new_id().to_string();
    let body = DuplicateBody::default();
    let request = s
        .world
        .app
        .call(MountDuplicate, &[&s.source])
        .token(&s.company.boss.token)
        .header("Idempotency-Key", &request_key)
        .body(&body)
        .send_bytes()
        .await;
    let id = identity(&request)?;
    assert_eq!(s.row(id).await?.name, "original-data-copy-2");
    let reader = key(
        &s.world,
        s.company.boss.id(),
        vec![rule("MountGet", "mount", s.source, "self", "read")],
    )
    .await?;
    let replay = s
        .world
        .app
        .call(MountDuplicate, &[&s.source])
        .token(&reader.secret)
        .header("Idempotency-Key", &request_key)
        .body(&body)
        .send_bytes()
        .await;
    assert_eq!(
        identity(&replay)?,
        id,
        "accepted sparse identity does not replay source credentials or a spent Write"
    );
    assert_eq!(s.count().await?, 3);
    let fresh = s
        .world
        .app
        .call(MountDuplicate, &[&s.source])
        .token(&reader.secret)
        .header("Idempotency-Key", &new_id().to_string())
        .body(&body)
        .send_bytes()
        .await;
    assert_eq!(fresh.status.as_u16(), 403);
    let records: Vec<Json> =
        sqlx::query_scalar("SELECT response FROM idempotency_keys WHERE user_id=$1 AND key=$2")
            .bind(s.company.boss.id())
            .bind(&request_key)
            .fetch_all(&s.world.db.pool)
            .await?;
    assert_eq!(records.len(), 1);
    assert!(!json!(records).to_string().contains("/data/original"));
    Ok(())
}

#[tokio::test]
async fn a_duplicate_refuses_an_expired_credential_after_its_actual_native_name_wait() -> Proof {
    for expire in [false, true] {
        let s = Scene::new().await?;
        let token = key(
            &s.world,
            s.company.boss.id(),
            vec![
                rule("MountDuplicate", "mount", s.source, "self", "read"),
                rule("MountDuplicate", "org", s.company.acme, "self", "write"),
            ],
        )
        .await?;
        let mut blocker = s.world.db.pool.begin().await?;
        rows::lock_creation_name(&mut blocker, Some(s.company.acme), "waiting-copy").await?;
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await?;
        let app = s.world.app.clone();
        let source = s.source;
        let request = tokio::spawn(async move {
            let request = app
                .call(MountDuplicate, &[&source])
                .token(&token.secret)
                .body(&DuplicateBody {
                    name: Some("waiting-copy".into()),
                    ..Default::default()
                });
            request.send_bytes().await
        });
        tokio::time::timeout(std::time::Duration::from_secs(30),async {
            loop {
                assert!(!request.is_finished());
                let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)))").bind(pid).fetch_one(&s.world.db.pool).await.unwrap();
                if waiting {break;}
                tokio::task::yield_now().await;
            }
        }).await?;
        if expire {
            s.world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await?;
        let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request).await??;
        if expire {
            assert!(!reply.status.is_success());
            assert_eq!(s.count().await?, 1);
        } else {
            identity(&reply)?;
            assert_eq!(s.count().await?, 2);
        }
    }
    Ok(())
}

#[tokio::test]
async fn cross_org_mount_copy_uses_native_creation_and_records_the_original_source_namespace()
-> Proof {
    let s = Scene::new().await?;
    let destination = places::make_org(
        &s.world,
        &s.company.boss,
        "Cross-root recipient",
        "cross-root-recipient",
    )
    .await
    .base
    .id;
    let token = key(
        &s.world,
        s.company.boss.id(),
        vec![
            rule("MountDuplicate", "mount", s.source, "self", "read"),
            rule("MountDuplicate", "org", destination, "self", "write"),
        ],
    )
    .await?;
    let made = identity(
        &s.copy(
            &token.secret,
            &DuplicateBody {
                destination: Some(DuplicateDestination::Node { id: destination }),
                ..Default::default()
            },
        )
        .await,
    )?;
    assert_ne!(made, s.source);
    let row: serde_json::Value = sqlx::query_scalar("SELECT jsonb_build_object('home',home_id,'org',org_id,'owner',owner_id) FROM mounts WHERE id=$1")
        .bind(made).fetch_one(&s.world.db.pool).await?;
    assert_eq!(
        row,
        json!({"home":destination,"org":destination,"owner":s.company.boss.id()})
    );
    let audit: (Option<Id>, serde_json::Value) = sqlx::query_as(
        "SELECT org_id,details FROM audit_log WHERE object_id=$1 AND action='content.copied_out'",
    )
    .bind(s.source)
    .fetch_one(&s.world.db.pool)
    .await?;
    assert_eq!(audit.0, Some(s.company.acme));
    assert_eq!(audit.1["created_id"], json!(made));
    assert_eq!(audit.1["destination_org_id"], json!(destination));
    Ok(())
}

#[tokio::test]
async fn mount_move_commits_a_new_native_mount_and_ends_the_original_in_one_act() -> Proof {
    use orch_api_contract::duplicates::{
        MountMoveOut, MountMoveOutPreview, MoveOutBody, MoveOutConfirmation,
    };
    let s = Scene::new().await?;
    let destination = places::make_org(
        &s.world,
        &s.company.boss,
        "Move recipient",
        "move-recipient",
    )
    .await
    .base
    .id;
    let copy = DuplicateBody {
        destination: Some(DuplicateDestination::Node { id: destination }),
        ..Default::default()
    };
    let plan = s
        .world
        .app
        .call(MountMoveOutPreview, &[&s.source])
        .token(&s.company.boss.token)
        .body(&copy)
        .send()
        .await
        .ok();
    assert_eq!(s.count().await?, 1);
    let token = key(
        &s.world,
        s.company.boss.id(),
        vec![
            rule("MountMoveOut", "mount", s.source, "self", "manage"),
            rule("MountMoveOut", "org", destination, "self", "write"),
        ],
    )
    .await?;
    let reply = s
        .world
        .app
        .call(MountMoveOut, &[&s.source])
        .token(&token.secret)
        .body(&MoveOutBody {
            copy,
            delete_plan: MoveOutConfirmation {
                revision: plan.revision,
                stamp: plan.stamp,
            },
        })
        .send_bytes()
        .await;
    let moved = identity(&reply)?;
    let row = s.row(moved).await?;
    assert_eq!(
        (row.home_id, row.org_id, row.owner_id),
        (
            Some(destination),
            Some(destination),
            Some(s.company.boss.id())
        )
    );
    assert_eq!(row.target, "/data/original");
    let ended: bool = sqlx::query_scalar("SELECT deleted_at IS NOT NULL FROM mounts WHERE id=$1")
        .bind(s.source)
        .fetch_one(&s.world.db.pool)
        .await?;
    assert!(ended);
    let audits: Vec<String> = sqlx::query_scalar("SELECT action FROM audit_log WHERE (object_id=$1 AND action IN ('content.copied_out','mount.deleted')) OR (object_id=$2 AND action='mount.created') ORDER BY action")
        .bind(s.source).bind(moved).fetch_all(&s.world.db.pool).await?;
    assert_eq!(
        audits,
        vec!["content.copied_out", "mount.created", "mount.deleted"]
    );
    Ok(())
}

#[tokio::test]
async fn a_mount_volume_delete_refusal_keeps_the_entire_move_unapplied() -> Proof {
    use orch_api_contract::duplicates::{
        MountMoveOut, MountMoveOutPreview, MoveOutBody, MoveOutConfirmation,
    };
    let s = Scene::new().await?;
    let destination = places::make_org(
        &s.world,
        &s.company.boss,
        "Move recipient",
        "move-recipient",
    )
    .await
    .base
    .id;
    let copy = DuplicateBody {
        destination: Some(DuplicateDestination::Node { id: destination }),
        ..Default::default()
    };
    let plan = s
        .world
        .app
        .call(MountMoveOutPreview, &[&s.source])
        .token(&s.company.boss.token)
        .body(&copy)
        .send()
        .await
        .ok();
    let host = orch_db::queries::resources::access_fixtures::insert_resource(
        &mut *s.world.conn().await,
        "host",
        Some(s.company.boss.id()),
        Some(s.company.acme),
        "Actual volume custodian",
    )
    .await?;
    let machine: Id = sqlx::query_scalar("SELECT machine_id FROM hosts WHERE id=$1")
        .bind(host)
        .fetch_one(&s.world.db.pool)
        .await?;
    sqlx::query("INSERT INTO mount_locations(mount_id,machine_id,kind,source,created_by) VALUES($1,$2,'volume','controlled-source-volume',$3)")
        .bind(s.source).bind(machine).bind(s.company.boss.id()).execute(&s.world.db.pool).await?;
    let refused = s
        .world
        .app
        .call(MountMoveOut, &[&s.source])
        .token(&s.company.boss.token)
        .body(&MoveOutBody {
            copy,
            delete_plan: MoveOutConfirmation {
                revision: plan.revision,
                stamp: plan.stamp,
            },
        })
        .send_bytes()
        .await;
    assert_eq!(
        refused.problem().code,
        orch_api_contract::common::ErrorCode::ResourceInUse
    );
    assert_eq!(s.count().await?, 1);
    let kept: bool = sqlx::query_scalar("SELECT deleted_at IS NULL FROM mounts WHERE id=$1")
        .bind(s.source)
        .fetch_one(&s.world.db.pool)
        .await?;
    assert!(kept);
    Ok(())
}
