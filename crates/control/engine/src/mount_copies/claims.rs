//! Durable destination claims shared by every producer of mount copies.

use super::{CopyDecision, of_plan};
use crate::EngineError;
use orch_db::{
    Conn, DbError,
    queries::jobs::{self, JobRow},
};
use orch_types::{Id, Json};

/// A claim records identity only. Every native producer still checks its own current authority.
pub(crate) async fn acquire(
    conn: &mut Conn,
    parent: &JobRow,
    copies: &[&CopyDecision],
) -> Result<bool, EngineError> {
    if !orch_db::queries::transaction_guard::in_transaction(conn) {
        let mut tx = sqlx::Connection::begin(conn).await.map_err(DbError::from)?;
        let acquired = acquire_on(&mut tx, parent, copies).await?;
        tx.commit().await.map_err(DbError::from)?;
        return Ok(acquired);
    }
    acquire_on(conn, parent, copies).await
}

async fn acquire_on(
    conn: &mut Conn,
    parent: &JobRow,
    copies: &[&CopyDecision],
) -> Result<bool, EngineError> {
    if copies.is_empty() {
        return Ok(true);
    }
    let mut mounts: Vec<_> = copies.iter().map(|copy| copy.mount_id).collect();
    mounts.sort_unstable();
    mounts.dedup();
    let present: Vec<Id> =
        sqlx::query_scalar("SELECT id FROM mounts WHERE id=ANY($1) ORDER BY id FOR NO KEY UPDATE")
            .bind(&mounts)
            .fetch_all(&mut *conn)
            .await
            .map_err(DbError::from)?;
    if present != mounts {
        return Err(EngineError::unreadable(
            "The copy's original mount was removed.",
        ));
    }
    let current = jobs::lock(conn, parent.id)
        .await?
        .ok_or_else(|| EngineError::unreadable("The copy's original job was removed."))?;
    if !current.is_open()
        || current.plan["committed"] == true
        || current.plan["mount_decisions"] != parent.plan["mount_decisions"]
    {
        return Err(EngineError::unreadable("The copy's accepted job changed."));
    }
    for copy in copies {
        if !memberships_current(conn, copy).await? {
            return Err(EngineError::unreadable(
                "The reviewed source or destination location changed.",
            ));
        }
    }
    let accepted = of_plan(&current.plan)?;
    if copies.iter().any(|copy| !accepted.copies.contains(copy)) {
        return Err(EngineError::unreadable(
            "The native copy differs from its accepted destination.",
        ));
    }
    let mut fresh = Vec::new();
    // Inspect the whole set before changing one claim. A busy destination must
    // never leave a partial reservation that another waiting copy needs.
    for copy in copies {
        let owner: Option<(Id, Json)> = sqlx::query_as(
            "SELECT job_id,decision FROM mount_copy_claims WHERE mount_id=$1 AND machine_id=$2",
        )
        .bind(copy.mount_id)
        .bind(copy.target_machine)
        .fetch_optional(&mut *conn)
        .await
        .map_err(DbError::from)?;
        let decision = serde_json::to_value(copy)?;
        match owner {
            Some((id, old)) if id == current.id && old == decision => {}
            Some((id, _)) if id != current.id => {
                let was_closed: bool = sqlx::query_scalar(
                    "SELECT NOT(state IN ('queued','running')) FROM jobs WHERE id=$1",
                )
                .bind(id)
                .fetch_one(&mut *conn)
                .await
                .map_err(DbError::from)?;
                if !was_closed {
                    return Ok(false);
                }
                let old = jobs::lock(conn, id).await?.ok_or_else(|| {
                    EngineError::unreadable("The prior copy's retained job was removed.")
                })?;
                if old.is_open() {
                    return Ok(false);
                }
                fresh.push((*copy, decision, Some(id)));
            }
            Some(_) => {
                return Err(EngineError::unreadable(
                    "The copy's retained destination claim changed.",
                ));
            }
            None => fresh.push((*copy, decision, None)),
        }
    }
    for (copy, decision, previous) in fresh {
        if let Some(previous) = previous {
            sqlx::query(
                "DELETE FROM mount_copy_claims WHERE mount_id=$1 AND machine_id=$2 AND job_id=$3",
            )
            .bind(copy.mount_id)
            .bind(copy.target_machine)
            .bind(previous)
            .execute(&mut *conn)
            .await
            .map_err(DbError::from)?;
        }
        insert(conn, current.id, copy, &decision).await?;
    }
    Ok(true)
}
async fn insert(
    conn: &mut Conn,
    job: Id,
    copy: &CopyDecision,
    decision: &Json,
) -> Result<(), EngineError> {
    sqlx::query(
        "INSERT INTO mount_copy_claims(mount_id,machine_id,job_id,decision) VALUES($1,$2,$3,$4)",
    )
    .bind(copy.mount_id)
    .bind(copy.target_machine)
    .bind(job)
    .bind(decision)
    .execute(conn)
    .await
    .map_err(DbError::from)?;
    Ok(())
}
pub(crate) async fn memberships_current(
    conn: &mut Conn,
    copy: &CopyDecision,
) -> Result<bool, EngineError> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM mount_locations WHERE mount_id=$1 AND machine_id=$2 AND membership_id=$3) AND (SELECT membership_id FROM mount_locations WHERE mount_id=$1 AND machine_id=$4) IS NOT DISTINCT FROM $5::uuid")
        .bind(copy.mount_id).bind(copy.source_machine).bind(copy.source_membership_id)
        .bind(copy.target_machine).bind(copy.target_membership_id)
        .fetch_one(conn).await.map_err(DbError::from)?)
}

pub(crate) async fn held(
    conn: &mut Conn,
    job: Id,
    copy: &CopyDecision,
) -> Result<bool, EngineError> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM mount_copy_claims WHERE mount_id=$1 AND machine_id=$2 AND job_id=$3 AND decision=$4)")
        .bind(copy.mount_id).bind(copy.target_machine).bind(job).bind(serde_json::to_value(copy)?)
        .fetch_one(conn).await.map_err(DbError::from)?)
}
/// The caller retains its owned publication/compensation transaction or a locked terminal job.
pub(crate) async fn release(conn: &mut Conn, job: Id) -> Result<(), EngineError> {
    sqlx::query("DELETE FROM mount_copy_claims WHERE job_id=$1")
        .bind(job)
        .execute(conn)
        .await
        .map_err(DbError::from)?;
    Ok(())
}

/// The same terminal-job fence recovers a completion hook interrupted by a process exit.
pub(crate) async fn reap_finished(conn: &mut Conn, limit: i64) -> Result<(), EngineError> {
    sqlx::query("WITH closed AS (SELECT j.id FROM jobs j WHERE j.state NOT IN ('queued','running') AND EXISTS(SELECT 1 FROM mount_copy_claims c WHERE c.job_id=j.id) ORDER BY j.finished_at,j.id LIMIT $1 FOR UPDATE OF j SKIP LOCKED) DELETE FROM mount_copy_claims c USING closed j WHERE c.job_id=j.id")
        .bind(limit).execute(conn).await.map_err(DbError::from)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mount_copies::{CopyDecision, DataRef, MountDecisions};
    use orch_db::queries::{identity::testing, jobs, resources::access_fixtures};
    use orch_testkit::{clock, database::TestDb};
    use orch_types::new_id;
    use serde_json::json;

    async fn source_membership(
        conn: &mut orch_db::Conn,
        mount: orch_types::Id,
        machine: orch_types::Id,
        actor: orch_types::Id,
        path: &str,
    ) -> orch_types::Id {
        orch_db::queries::resources::mounts::set_location(
            conn,
            &orch_db::queries::resources::mounts::LocationRow {
                mount_id: mount,
                machine_id: machine,
                host_id: None,
                host_name: None,
                kind: "path".into(),
                source: path.into(),
                storage: None,
                data_kind: None,
                created_by: actor,
                created_at: clock::start(),
            },
        )
        .await
        .unwrap();
        sqlx::query_scalar(
            "SELECT membership_id FROM mount_locations WHERE mount_id=$1 AND machine_id=$2",
        )
        .bind(mount)
        .bind(machine)
        .fetch_one(conn)
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn a_copy_destination_survives_its_waiter_and_a_late_release_cannot_take_another_jobs_claim()
     {
        let db = TestDb::fresh().await;
        let mut conn = db.pool.acquire().await.unwrap();
        let actor = new_id();
        testing::insert_user(&mut conn, actor, "copy-owner")
            .await
            .unwrap();
        let reader = access_fixtures::insert_persons_machine(&mut conn, actor, "Source")
            .await
            .unwrap();
        let writer = access_fixtures::insert_persons_machine(&mut conn, actor, "Destination")
            .await
            .unwrap();
        let mount =
            access_fixtures::insert_resource(&mut conn, "mount", Some(actor), None, "copy-data")
                .await
                .unwrap();
        let copy = CopyDecision {
            source_membership_id: source_membership(&mut conn, mount, reader, actor, "/srv/data")
                .await,
            target_membership_id: None,
            mount_id: mount,
            source_machine: reader,
            source_driver: "directory".into(),
            from: DataRef {
                kind: "path".into(),
                source: "/srv/data".into(),
                storage: None,
            },
            target_machine: writer,
            target_driver: "directory".into(),
            to: DataRef {
                kind: "volume".into(),
                source: format!("orch-mount-{mount}"),
                storage: None,
            },
            made: true,
            kept: None,
            file_count: 2,
            total_bytes: 4096,
            data_kind: None,
        };
        let decisions = MountDecisions {
            detached: vec![],
            copies: vec![copy.clone()],
        };
        let mut parents = Vec::new();
        for kind in ["snapshot", "export"] {
            parents.push(
                jobs::insert(
                    &mut conn,
                    &jobs::NewJob {
                        id: new_id(),
                        kind,
                        node_id: None,
                        environment_id: None,
                        provider_id: None,
                        plan: &json!({"mount_decisions":decisions}),
                        progress: &json!({}),
                        created_by: Some(actor),
                    },
                    clock::start(),
                )
                .await
                .unwrap(),
            );
        }
        assert!(acquire(&mut conn, &parents[0], &[&copy]).await.unwrap());
        assert!(acquire(&mut conn, &parents[0], &[&copy]).await.unwrap());
        drop(conn);
        let mut conn = db.pool.acquire().await.unwrap();
        assert!(
            !acquire(&mut conn, &parents[1], &[&copy]).await.unwrap(),
            "a second multi-command plan cannot displace the first native writer"
        );
        release(&mut conn, parents[0].id).await.unwrap();
        assert!(acquire(&mut conn, &parents[1], &[&copy]).await.unwrap());
        release(&mut conn, parents[0].id).await.unwrap();
        assert!(held(&mut conn, parents[1].id, &copy).await.unwrap());
        let mut changed = copy.clone();
        changed.to.source.push_str("-another");
        assert!(
            !held(&mut conn, parents[1].id, &changed).await.unwrap(),
            "a job's claim cannot be reused for a different native proposal"
        );
    }
    #[tokio::test]
    async fn a_busy_copy_destination_never_leaves_part_of_another_plans_claims_behind() {
        let db = TestDb::fresh().await;
        let mut conn = db.pool.acquire().await.unwrap();
        let actor = new_id();
        testing::insert_user(&mut conn, actor, "copy-owner")
            .await
            .unwrap();
        let reader = access_fixtures::insert_persons_machine(&mut conn, actor, "Source")
            .await
            .unwrap();
        let writer = access_fixtures::insert_persons_machine(&mut conn, actor, "Destination")
            .await
            .unwrap();
        let mut copies = Vec::new();
        for name in ["first-copy", "second-copy"] {
            let mount =
                access_fixtures::insert_resource(&mut conn, "mount", Some(actor), None, name)
                    .await
                    .unwrap();
            copies.push(CopyDecision {
                source_membership_id: source_membership(
                    &mut conn,
                    mount,
                    reader,
                    actor,
                    &format!("/srv/{name}"),
                )
                .await,
                target_membership_id: None,
                mount_id: mount,
                source_machine: reader,
                source_driver: "directory".into(),
                from: DataRef {
                    kind: "path".into(),
                    source: format!("/srv/{name}"),
                    storage: None,
                },
                target_machine: writer,
                target_driver: "directory".into(),
                to: DataRef {
                    kind: "volume".into(),
                    source: format!("orch-mount-{mount}"),
                    storage: None,
                },
                made: true,
                kept: None,
                file_count: 2,
                total_bytes: 4096,
                data_kind: None,
            });
        }
        let mut parents = Vec::new();
        for planned in [copies.clone(), vec![copies[1].clone()]] {
            parents.push(jobs::insert(&mut conn,&jobs::NewJob {
                id:new_id(),kind:"snapshot",node_id:None,environment_id:None,provider_id:None,
                plan:&json!({"mount_decisions":MountDecisions {detached:vec![],copies:planned}}),
                progress:&json!({}),created_by:Some(actor),
            },clock::start()).await.unwrap());
        }
        assert!(
            acquire(&mut conn, &parents[1], &[&copies[1]])
                .await
                .unwrap()
        );
        assert!(
            !acquire(&mut conn, &parents[0], &[&copies[0], &copies[1]])
                .await
                .unwrap()
        );
        assert!(
            !held(&mut conn, parents[0].id, &copies[0]).await.unwrap(),
            "waiting for a later destination must not reserve an earlier one"
        );
        release(&mut conn, parents[1].id).await.unwrap();
        assert!(
            acquire(&mut conn, &parents[0], &[&copies[0], &copies[1]])
                .await
                .unwrap()
        );
        for copy in &copies {
            assert!(held(&mut conn, parents[0].id, copy).await.unwrap());
        }
    }
    #[tokio::test]
    async fn a_waiting_copy_refuses_a_destination_published_after_its_review() {
        let db = TestDb::fresh().await;
        let mut conn = db.pool.acquire().await.unwrap();
        let actor = new_id();
        testing::insert_user(&mut conn, actor, "copy-owner")
            .await
            .unwrap();
        let reader = access_fixtures::insert_persons_machine(&mut conn, actor, "Source")
            .await
            .unwrap();
        let writer = access_fixtures::insert_persons_machine(&mut conn, actor, "Destination")
            .await
            .unwrap();
        let mount = access_fixtures::insert_resource(
            &mut conn,
            "mount",
            Some(actor),
            None,
            "reviewed-copy",
        )
        .await
        .unwrap();
        let copy = CopyDecision {
            source_membership_id: source_membership(&mut conn, mount, reader, actor, "/srv/data")
                .await,
            target_membership_id: None,
            mount_id: mount,
            source_machine: reader,
            source_driver: "directory".into(),
            from: DataRef {
                kind: "path".into(),
                source: "/srv/data".into(),
                storage: None,
            },
            target_machine: writer,
            target_driver: "directory".into(),
            to: DataRef {
                kind: "volume".into(),
                source: format!("orch-mount-{mount}"),
                storage: None,
            },
            made: true,
            kept: None,
            file_count: 2,
            total_bytes: 4096,
            data_kind: None,
        };
        let parent=jobs::insert(&mut conn,&jobs::NewJob {
            id:new_id(),kind:"snapshot",node_id:None,environment_id:None,provider_id:None,
            plan:&json!({"mount_decisions":MountDecisions {detached:vec![],copies:vec![copy.clone()]}}),
            progress:&json!({}),created_by:Some(actor),
        },clock::start()).await.unwrap();
        source_membership(&mut conn, mount, writer, actor, "/srv/another-publication").await;
        assert!(
            acquire(&mut conn, &parent, &[&copy]).await.is_err(),
            "an old plan must not reinterpret another completed publication as its own destination"
        );
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM mount_copy_claims WHERE job_id=$1")
                .bind(parent.id)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        assert_eq!(count, 0);
    }
    #[tokio::test]
    async fn publishing_replaced_data_invalidates_old_reviews_even_when_its_name_and_time_match() {
        let db = TestDb::fresh().await;
        let mut conn = db.pool.acquire().await.unwrap();
        let actor = new_id();
        testing::insert_user(&mut conn, actor, "copy-owner")
            .await
            .unwrap();
        let reader = access_fixtures::insert_persons_machine(&mut conn, actor, "Source")
            .await
            .unwrap();
        let writer = access_fixtures::insert_persons_machine(&mut conn, actor, "Destination")
            .await
            .unwrap();
        let mount = access_fixtures::insert_resource(
            &mut conn,
            "mount",
            Some(actor),
            None,
            "replaced-copy",
        )
        .await
        .unwrap();
        let source = source_membership(&mut conn, mount, reader, actor, "/srv/data").await;
        let volume = format!("orch-mount-{mount}");
        orch_db::queries::resources::mounts::set_location(
            &mut conn,
            &orch_db::queries::resources::mounts::LocationRow {
                mount_id: mount,
                machine_id: writer,
                host_id: None,
                host_name: None,
                kind: "volume".into(),
                source: volume.clone(),
                storage: None,
                data_kind: None,
                created_by: actor,
                created_at: clock::start(),
            },
        )
        .await
        .unwrap();
        let target = sqlx::query_scalar(
            "SELECT membership_id FROM mount_locations WHERE mount_id=$1 AND machine_id=$2",
        )
        .bind(mount)
        .bind(writer)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        let copy = CopyDecision {
            source_membership_id: source,
            target_membership_id: Some(target),
            mount_id: mount,
            source_machine: reader,
            source_driver: "directory".into(),
            from: DataRef {
                kind: "path".into(),
                source: "/srv/data".into(),
                storage: None,
            },
            target_machine: writer,
            target_driver: "directory".into(),
            to: DataRef {
                kind: "volume".into(),
                source: volume,
                storage: None,
            },
            made: false,
            kept: None,
            file_count: 2,
            total_bytes: 4096,
            data_kind: None,
        };
        let decisions = MountDecisions {
            detached: vec![],
            copies: vec![copy.clone()],
        };
        let parent = jobs::insert(
            &mut conn,
            &jobs::NewJob {
                id: new_id(),
                kind: "snapshot",
                node_id: None,
                environment_id: None,
                provider_id: None,
                plan: &json!({"mount_decisions":decisions}),
                progress: &json!({}),
                created_by: Some(actor),
            },
            clock::start(),
        )
        .await
        .unwrap();
        assert!(acquire(&mut conn, &parent, &[&copy]).await.unwrap());
        crate::mount_copies::commit_locations(
            &mut conn,
            &decisions,
            None,
            parent.id,
            actor,
            clock::start(),
        )
        .await
        .unwrap();
        assert!(
            !memberships_current(&mut conn, &copy).await.unwrap(),
            "new data at the same name is a new reviewed location membership"
        );
        assert!(!held(&mut conn, parent.id, &copy).await.unwrap());
    }
}
