//! Native archive removal, its rollbackable waits and completed empty acknowledgement.

use super::*;
use orch_api_contract::exports::ExportDelete;

const OPERATION: &str = "ExportDelete";

fn manage(id: Id) -> Value {
    rule(OPERATION, "export", id, "self", &["export"], &["manage"])
}

async fn remove(o: &Office, token: &str, id: Id) -> RawReply {
    let call = o.s.rig.world.app.call(ExportDelete, &[&id]);
    call.token(token).send_bytes().await
}

async fn stored(o: &Office, id: Id) -> (String, Option<String>) {
    sqlx::query_as("SELECT state,blob FROM exports WHERE id=$1")
        .bind(id)
        .fetch_one(o.s.rig.db.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn archive_manage_only_removes_native_content_without_metadata_read_and_keeps_an_empty_ack() {
    let o = Office::new().await;
    let export = o.made(&o.s.ada, o.org(), &everything()).await;
    let before = stored(&o, export.id).await;
    assert_eq!(before.0, "ready");
    assert!(before.1.is_some());
    for selected in [
        artifact(export.id),
        rule(
            OPERATION,
            "export",
            export.id,
            "self",
            &["export"],
            &["read"],
        ),
        rule(
            OPERATION,
            "import",
            export.id,
            "self",
            &["export"],
            &["manage"],
        ),
        rule(
            OPERATION,
            "account",
            o.s.ada.id(),
            "self",
            &["export"],
            &["manage"],
        ),
    ] {
        let denied = key(&o, &o.s.ada, vec![selected]).await;
        assert_eq!(
            remove(&o, &denied.secret, export.id).await.status,
            StatusCode::FORBIDDEN
        );
        assert_eq!(stored(&o, export.id).await, before);
    }
    let writer = key(&o, &o.s.ada, vec![manage(export.id)]).await;
    let reply = remove(&o, &writer.secret, export.id).await;
    assert_eq!(
        reply.status,
        StatusCode::NO_CONTENT,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    assert!(reply.raw.is_empty());
    assert_eq!(stored(&o, export.id).await, ("expired".into(), None));
    assert_eq!(o.fetch(&o.s.ada, export.id).await.status, StatusCode::GONE);
    let repeated = remove(&o, &writer.secret, export.id).await;
    assert_eq!(repeated.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn archive_removal_keeps_native_creator_custody_and_exact_identity_after_place_purge() {
    let o = Office::new().await;
    let export = o.made(&o.s.ada, o.org(), &everything()).await;
    let other = key(&o, &o.s.cai, vec![manage(export.id)]).await;
    assert_eq!(
        remove(&o, &other.secret, export.id).await.status,
        StatusCode::NOT_FOUND
    );
    let linked = key(
        &o,
        &o.s.ada,
        vec![rule(
            OPERATION,
            "org",
            o.org(),
            "subtree",
            &["export"],
            &["manage"],
        )],
    )
    .await;
    sqlx::query("UPDATE exports SET node_id=NULL WHERE id=$1")
        .bind(export.id)
        .execute(o.s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(
        remove(&o, &linked.secret, export.id).await.status,
        StatusCode::FORBIDDEN
    );
    let writer = key(&o, &o.s.ada, vec![manage(export.id)]).await;
    let reply = remove(&o, &writer.secret, export.id).await;
    assert_eq!(reply.status, StatusCode::NO_CONTENT);
    assert!(reply.raw.is_empty());
    assert_eq!(stored(&o, export.id).await, ("expired".into(), None));
}

async fn native_wait(
    o: &Office,
    blocker: i32,
    query: &str,
    request: &tokio::task::JoinHandle<RawReply>,
) {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            assert!(
                !request.is_finished(),
                "the native archive footprint wait must be reached"
            );
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database()
                 AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query ILIKE $2)",
            )
            .bind(blocker)
            .bind(query)
            .fetch_one(o.s.rig.db.pool())
            .await
            .unwrap();
            if waiting {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the native archive removal lock wait");
}

#[tokio::test]
async fn archive_removal_refreshes_the_actual_key_after_native_archive_job_and_pin_waits() {
    for (source, expected) in [
        ("archive", "%FROM exports%FOR UPDATE%"),
        ("job", "%FROM jobs%FOR UPDATE%"),
        ("pin", "%FROM blobs%FOR UPDATE%"),
    ] {
        for expire in [false, true] {
            let o = Office::new().await;
            let export = o.made(&o.s.ada, o.org(), &everything()).await;
            let before = stored(&o, export.id).await;
            let writer = key(&o, &o.s.ada, vec![manage(export.id)]).await;
            let mut blocker = o.s.rig.db.pool().begin().await.unwrap();
            match source {
                "archive" => {
                    sqlx::query("SELECT id FROM exports WHERE id=$1 FOR UPDATE")
                        .bind(export.id)
                        .execute(&mut *blocker)
                        .await
                        .unwrap();
                }
                "job" => {
                    sqlx::query("SELECT id FROM jobs WHERE id=$1 FOR UPDATE")
                        .bind(export.job_id.unwrap())
                        .execute(&mut *blocker)
                        .await
                        .unwrap();
                }
                _ => {
                    sqlx::query("SELECT hash FROM blobs WHERE hash=decode($1,'hex') FOR UPDATE")
                        .bind(before.1.as_ref().unwrap())
                        .execute(&mut *blocker)
                        .await
                        .unwrap();
                }
            }
            let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *blocker)
                .await
                .unwrap();
            let app = o.s.rig.world.app.clone();
            let id = export.id;
            let token = writer.secret.clone();
            let request = tokio::spawn(async move {
                let call = app.call(ExportDelete, &[&id]);
                call.token(&token).send_bytes().await
            });
            native_wait(&o, pid, expected, &request).await;
            if expire {
                o.s.rig.world.advance(chrono::Duration::hours(2));
            }
            blocker.commit().await.unwrap();
            let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                reply.status,
                if expire {
                    StatusCode::UNAUTHORIZED
                } else {
                    StatusCode::NO_CONTENT
                }
            );
            if expire {
                assert_eq!(
                    stored(&o, export.id).await,
                    before,
                    "no expiry or pin release before the current native first effect"
                );
            } else {
                assert!(reply.raw.is_empty());
                assert_eq!(stored(&o, export.id).await, ("expired".into(), None));
            }
        }
    }
}

#[tokio::test]
async fn archive_removal_keeps_its_committed_native_ack_when_key_expires_during_activity_publication()
 {
    let o = Office::new().await;
    let export = o.made(&o.s.ada, o.org(), &everything()).await;
    let writer = key(&o, &o.s.ada, vec![manage(export.id)]).await;
    let mut blocker = o.s.rig.db.pool().begin().await.unwrap();
    sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let app = o.s.rig.world.app.clone();
    let id = export.id;
    let token = writer.secret.clone();
    let request = tokio::spawn(async move {
        let call = app.call(ExportDelete, &[&id]);
        call.token(&token).send_bytes().await
    });
    activity_wait(&o, pid, &request).await;
    assert_eq!(
        stored(&o, id).await,
        ("expired".into(), None),
        "the native writer committed before bookkeeping"
    );
    o.s.rig.world.advance(chrono::Duration::hours(2));
    blocker.commit().await.unwrap();
    let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reply.status, StatusCode::NO_CONTENT);
    assert!(reply.raw.is_empty());
    assert_eq!(stored(&o, id).await, ("expired".into(), None));
}
