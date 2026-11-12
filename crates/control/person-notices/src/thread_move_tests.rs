//! Actual Inbox and native delivery-source controls for the completed thread receipt.
use super::*;
use orch_access_contract::key_scope::{KeyAction, KeyFacet};
use orch_authz::ResolvedFacetRole;
use orch_db::queries::{access, hierarchy, identity, threads};
use orch_testkit::{clock, database::TestDb};
use orch_types::{ManualClock, new_id};

type Proof<T = ()> = Result<T, Box<dyn std::error::Error>>;
struct Site {
    _test: TestDb,
    db: orch_db::Db,
    clock: ManualClock,
    by: Id,
    receiver: Id,
    other: Id,
    source: Id,
    created: Id,
    topic: Id,
    job: Id,
}
impl Site {
    async fn new() -> Proof<Self> {
        let test = TestDb::fresh().await;
        let db = orch_db::Db::from_pool(test.pool.clone());
        let clock = clock::manual();
        let (by, receiver, other, source, created, topic, job) = (
            new_id(),
            new_id(),
            new_id(),
            new_id(),
            new_id(),
            new_id(),
            new_id(),
        );
        let mut conn = db.acquire().await?;
        for (id, name) in [
            (by, "move-by"),
            (receiver, "move-reader"),
            (other, "move-other"),
        ] {
            identity::testing::insert_user(&mut conn, id, name).await?;
        }
        for (thread, topic) in [(source, new_id()), (created, topic)] {
            let (org, project) = (new_id(), new_id());
            for (id, kind, parent) in [
                (org, "org", None),
                (project, "project", Some(org)),
                (topic, "topic", Some(project)),
            ] {
                hierarchy::testing::insert_node(&mut conn, id, kind, parent, kind, by).await?;
            }
            assert!(
                threads::insert_thread(&mut conn, thread, topic, "Accepted move", by, clock.now())
                    .await?
            );
        }
        access::testing::add_entry(
            &mut conn,
            &access::PageKey::new("node", source, "live"),
            "any",
            "thread.view",
            ("user", Some(receiver), None),
            None,
            clock.now(),
        )
        .await?;
        drop(conn);
        Ok(Self {
            _test: test,
            db,
            clock,
            by,
            receiver,
            other,
            source,
            created,
            topic,
            job,
        })
    }
    async fn origin(&self, conn: &mut Conn) -> Proof<CapturedOperation> {
        let facets = [
            KeyFacet {
                operation: "ThreadMoveOut".into(),
                action: KeyAction::Manage,
                resource_kind: KeyTargetKind::Object(ObjectKind::Thread),
                target: key_targets::node(conn, self.source).await?,
            },
            KeyFacet {
                operation: "ThreadMoveOut".into(),
                action: KeyAction::Write,
                resource_kind: KeyTargetKind::Object(ObjectKind::Thread),
                target: key_targets::node(conn, self.topic).await?,
            },
        ];
        let roles = facets
            .iter()
            .map(|facet| ResolvedFacetRole {
                action: facet.action,
                resources: vec![facet.resource_kind],
                facets: vec![facet.clone()],
            })
            .collect::<Vec<_>>();
        let caller = Caller::session(self.by)
            .admit_resolved_operation("ThreadMoveOut", &roles, &facets)?
            .into_caller();
        Ok(CapturedOperation::capture(&caller)?)
    }
    async fn file(&self, conn: &mut Conn) -> Proof<Snapshot> {
        transaction_guard::lock_authority(conn).await?;
        let receipt = ThreadMove {
            source: self.source,
            created: self.created,
            job: self.job,
        };
        let mut capture = Capture {
            id: self.job,
            origin: self.origin(conn).await?,
            subject: Subject::Object(ObjectRef {
                kind: ObjectKind::Thread,
                id: self.source,
            }),
            expected: None,
            receivers: Vec::new(),
            raw: None,
            completed_chat_move: None,
            completed_thread_move: Some(receipt),
            eligible: vec![self.receiver],
        };
        let data = serde_json::json!({"operation_id":self.job,"by":self.by,"subject":"Accepted move","path":format!("/thread/{}",self.created),"completed_thread_move":receipt});
        let key = format!("person.notify:thread_move:{}", self.job);
        let notice = NewNotification {
            user_id: self.receiver,
            kind: "person.notify",
            node_id: None,
            thread_id: None,
            chat_id: None,
            approval_id: None,
            title: "Accepted move",
            body: "",
            data: &data,
            dedupe_key: &key,
            at: self.clock.now(),
        };
        let actual = orch_db::queries::notifications::people::announce(conn, &notice)
            .await?
            .ok_or("actual Inbox receipt")?;
        capture.record(&notice, &actual)?;
        capture.save(conn).await?;
        Ok(Snapshot::person_notice(Some(actual.id), &notice))
    }
}

#[tokio::test]
async fn thread_receipt_survives_its_native_soft_and_hard_ending_and_sender_loss() -> Proof {
    let site = Site::new().await?;
    let mut tx = site.db.begin().await?;
    let reader =
        Principal::load(&mut tx, Caller::session(site.receiver), site.clock.time()).await?;
    assert!(
        reads(
            &mut tx,
            &reader,
            Subject::Object(ObjectRef {
                kind: ObjectKind::Thread,
                id: site.source
            })
        )
        .await?
    );
    let snapshot = site.file(&mut tx).await?;
    assert!(
        !operations::ending::move_readable(&mut tx, site.receiver, &snapshot.kind, &snapshot.data)
            .await?
    );
    sqlx::query("UPDATE nodes SET deleted_at=$2,deleted_by=$3 WHERE id=$1")
        .bind(site.source)
        .bind(site.clock.now())
        .bind(site.by)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    let mut tx = site.db.begin().await?;
    transaction_guard::lock_authority(&mut tx).await?;
    sqlx::query("UPDATE users SET status='suspended' WHERE id=$1")
        .bind(site.by)
        .execute(&mut *tx)
        .await?;
    let Loaded::Source(opened) = load(&mut tx, &site.clock, site.receiver, &snapshot).await? else {
        return Err("accepted moved receipt closed with its sender/source".into());
    };
    assert!(opened.check(&mut tx, &site.clock).await?.is_some());
    let reader =
        Principal::load(&mut tx, Caller::session(site.receiver), site.clock.time()).await?;
    assert!(
        !reads(
            &mut tx,
            &reader,
            Subject::Object(ObjectRef {
                kind: ObjectKind::Thread,
                id: site.created
            })
        )
        .await?,
        "receipt gives no destination access"
    );
    sqlx::query("DELETE FROM nodes WHERE id=$1")
        .bind(site.source)
        .execute(&mut *tx)
        .await?;
    assert!(opened.check(&mut tx, &site.clock).await?.is_some());
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notifications WHERE user_id=$1 AND data->>'operation_id'=$2",
    )
    .bind(site.receiver)
    .bind(site.job.to_string())
    .fetch_one(&mut *tx)
    .await?;
    assert_eq!(count, 1);
    tx.commit().await?;
    Ok(())
}

#[tokio::test]
async fn thread_receipt_rejects_another_person_changed_data_and_rolled_back_success() -> Proof {
    let site = Site::new().await?;
    let mut tx = site.db.begin().await?;
    let snapshot = site.file(&mut tx).await?;
    sqlx::query("UPDATE nodes SET deleted_at=$2 WHERE id=$1")
        .bind(site.source)
        .bind(site.clock.now())
        .execute(&mut *tx)
        .await?;
    assert!(
        !operations::ending::move_readable(&mut tx, site.other, &snapshot.kind, &snapshot.data)
            .await?
    );
    let mut changed = snapshot.data.clone();
    changed["path"] = serde_json::json!(format!("/thread/{}", site.other));
    assert!(
        !operations::ending::move_readable(&mut tx, site.receiver, &snapshot.kind, &changed)
            .await?
    );
    tx.rollback().await?;
    let mut conn = site.db.acquire().await?;
    assert!(operations::get(&mut conn, site.job).await?.is_none());
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM notifications WHERE user_id=$1")
        .bind(site.receiver)
        .fetch_one(&mut *conn)
        .await?;
    assert_eq!(count, 0);
    Ok(())
}
