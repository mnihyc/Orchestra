//! Actual archive open, once-only first-download bookkeeping and current byte delivery.

use super::*;
use futures::StreamExt as _;
use orch_api_contract::exports::ExportDownload;

const DOWNLOAD: &str = "ExportDownload";
fn rules(o: &Office, ids: &[Id], named: bool) -> Vec<Value> {
    let mut values = ids
        .iter()
        .map(|id| rule(DOWNLOAD, "export", *id, "self", &["export"], &["read"]))
        .collect::<Vec<_>>();
    if named {
        values.push(rule(
            DOWNLOAD,
            "org",
            o.org(),
            "subtree",
            &["org", "project", "topic", "thread"],
            &["read"],
        ));
    }
    values
}
async fn fetch(o: &Office, token: &str, id: Id) -> RawReply {
    let call = o.s.rig.world.app.call(ExportDownload, &[&id]);
    call.token(token).send_bytes().await
}
async fn marks(o: &Office, id: Id) -> (bool, i64) {
    sqlx::query_as("SELECT downloaded_at IS NOT NULL,(SELECT count(*) FROM audit_log WHERE action='export.downloaded' AND object_id=$1) FROM exports WHERE id=$1")
        .bind(id).fetch_one(o.s.rig.db.pool()).await.unwrap()
}
async fn wait(
    o: &Office,
    pid: i32,
    query: &str,
    export_lock: bool,
    future: &tokio::task::JoinHandle<RawReply>,
) {
    tokio::time::timeout(std::time::Duration::from_secs(30),async {
        loop {
            assert!(!future.is_finished(),"the actual native archive boundary must wait");
            let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity a WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE $2 AND (NOT $3 OR EXISTS(SELECT 1 FROM pg_locks l WHERE l.pid=a.pid AND l.relation='exports'::regclass AND l.mode='RowShareLock' AND l.granted)))")
                .bind(pid).bind(query).bind(export_lock).fetch_one(o.s.rig.db.pool()).await.unwrap();
            if blocked {break;} tokio::task::yield_now().await;
        }
    }).await.expect("native archive lock arrival");
}

#[tokio::test]
async fn archive_bytes_keep_native_zip_headers_once_only_audit_and_independent_filename_read() {
    let o = Office::new().await;
    let export = o.made(&o.s.ada, o.org(), &everything()).await;
    let full = key(&o, &o.s.ada, rules(&o, &[export.id], true)).await;
    assert_eq!(marks(&o, export.id).await, (false, 0));
    let scoped = fetch(&o, &full.secret, export.id).await;
    assert_eq!(
        scoped.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&scoped.raw)
    );
    let native = o.fetch(&o.s.ada, export.id).await;
    assert_eq!(scoped.raw, native.raw);
    for name in [
        "content-type",
        "content-length",
        "content-disposition",
        "cache-control",
        "x-content-type-options",
    ] {
        assert_eq!(scoped.headers[name], native.headers[name]);
    }
    assert_eq!(marks(&o, export.id).await, (true, 1));
    let only = key(&o, &o.s.ada, rules(&o, &[export.id], false)).await;
    let unnamed = fetch(&o, &only.secret, export.id).await;
    assert_eq!(unnamed.status, StatusCode::OK);
    assert_eq!(unnamed.raw, native.raw);
    let row = orch_db::queries::exports::get(&mut *o.s.rig.world.conn().await, export.id)
        .await
        .unwrap()
        .unwrap();
    let generic = orch_engine::exports::archive_file_name(&row, None);
    assert!(
        unnamed.headers["content-disposition"]
            .to_str()
            .unwrap()
            .contains(&generic)
    );
    assert_ne!(
        unnamed.headers["content-disposition"],
        native.headers["content-disposition"]
    );
    assert_eq!(marks(&o, export.id).await, (true, 1));
    let wrong = key(&o, &o.s.ada, vec![artifact(export.id)]).await;
    assert_eq!(
        fetch(&o, &wrong.secret, export.id).await.status,
        StatusCode::FORBIDDEN
    );
    let manage_only = key(
        &o,
        &o.s.ada,
        vec![rule(
            DOWNLOAD,
            "export",
            export.id,
            "self",
            &["export"],
            &["manage"],
        )],
    )
    .await;
    assert_eq!(
        fetch(&o, &manage_only.secret, export.id).await.status,
        StatusCode::FORBIDDEN
    );
    let other = key(&o, &o.s.cai, rules(&o, &[export.id], false)).await;
    assert_eq!(
        fetch(&o, &other.secret, export.id).await.status,
        StatusCode::NOT_FOUND
    );
    let linked = key(
        &o,
        &o.s.ada,
        vec![rule(
            DOWNLOAD,
            "org",
            o.org(),
            "subtree",
            &["export"],
            &["read"],
        )],
    )
    .await;
    sqlx::query("UPDATE exports SET node_id=NULL WHERE id=$1")
        .bind(export.id)
        .execute(o.s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(
        fetch(&o, &only.secret, export.id).await.status,
        StatusCode::OK
    );
    assert_eq!(
        fetch(&o, &linked.secret, export.id).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(marks(&o, export.id).await, (true, 1));
}

#[tokio::test]
async fn archive_open_checks_the_actual_key_after_row_and_audit_waits_before_committing_its_marker()
{
    for source in ["row", "audit"] {
        for expire in [false, true] {
            let o = Office::new().await;
            let export = o.made(&o.s.ada, o.org(), &everything()).await;
            let reader = key(&o, &o.s.ada, rules(&o, &[export.id], false)).await;
            let mut blocker = o.s.rig.db.pool().begin().await.unwrap();
            let pattern = if source == "row" {
                sqlx::query("SELECT id FROM exports WHERE id=$1 FOR UPDATE")
                    .bind(export.id)
                    .execute(&mut *blocker)
                    .await
                    .unwrap();
                "%FROM exports%FOR UPDATE%"
            } else {
                sqlx::query("LOCK TABLE audit_log IN SHARE MODE")
                    .execute(&mut *blocker)
                    .await
                    .unwrap();
                "%INSERT INTO audit_log%"
            };
            let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *blocker)
                .await
                .unwrap();
            let app = o.s.rig.world.app.clone();
            let id = export.id;
            let token = reader.secret.clone();
            let future = tokio::spawn(async move {
                let call = app.call(ExportDownload, &[&id]);
                call.token(&token).send_bytes().await
            });
            wait(&o, pid, pattern, source == "audit", &future).await;
            if expire {
                o.s.rig.world.advance(chrono::Duration::hours(2));
            }
            blocker.commit().await.unwrap();
            let reply = tokio::time::timeout(std::time::Duration::from_secs(30), future)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                reply.status,
                if expire {
                    StatusCode::UNAUTHORIZED
                } else {
                    StatusCode::OK
                }
            );
            assert_eq!(
                marks(&o, id).await,
                if expire { (false, 0) } else { (true, 1) }
            );
        }
    }
}

#[tokio::test]
async fn archive_publication_never_lends_a_changed_hash_or_filename_scope_to_opened_bytes() {
    for mode in ["unchanged", "hash_replaced", "name_scope_closed", "expired"] {
        let o = Office::new().await;
        let first = o.made(&o.s.ada, o.org(), &everything()).await;
        let second = o.made(&o.s.ada, o.topic(), &everything()).await;
        let reader = key(&o, &o.s.ada, rules(&o, &[first.id, second.id], true)).await;
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
        let id = first.id;
        let token = reader.secret.clone();
        let future = tokio::spawn(async move {
            let call = app.call(ExportDownload, &[&id]);
            call.token(&token).send_bytes().await
        });
        activity_wait(&o, pid, &future).await;
        assert_eq!(
            marks(&o, id).await,
            (true, 1),
            "the real native open/audit committed before bookkeeping"
        );
        match mode {
            "hash_replaced" => {
                sqlx::query(
                    "UPDATE exports SET blob=(SELECT blob FROM exports WHERE id=$2) WHERE id=$1",
                )
                .bind(id)
                .bind(second.id)
                .execute(o.s.rig.db.pool())
                .await
                .unwrap();
            }
            "name_scope_closed" => {
                let scope: KeyScope =
                    serde_json::from_value(json!({"rules":rules(&o,&[id,second.id],false)}))
                        .unwrap();
                assert!(scope.validate().is_empty());
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(reader.id)
                    .bind(json!(scope))
                    .execute(o.s.rig.db.pool())
                    .await
                    .unwrap();
            }
            "expired" => o.s.rig.world.advance(chrono::Duration::hours(2)),
            _ => {}
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(std::time::Duration::from_secs(30), future)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            reply.status,
            match mode {
                "unchanged" => StatusCode::OK,
                "expired" => StatusCode::UNAUTHORIZED,
                _ => StatusCode::FORBIDDEN,
            }
        );
        assert_eq!(
            marks(&o, id).await,
            (true, 1),
            "audit completion is not replayed or a byte-read lease"
        );
    }
}

#[tokio::test]
async fn archive_body_delivery_rechecks_the_actual_key_after_headers_without_reopening_or_reauditing()
 {
    for expire in [false, true] {
        let o = Office::new().await;
        let export = o.made(&o.s.ada, o.org(), &everything()).await;
        let reader = key(&o, &o.s.ada, rules(&o, &[export.id], false)).await;
        let call = o.s.rig.world.app.call(ExportDownload, &[&export.id]);
        let response = call.token(&reader.secret).send_response().await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(marks(&o, export.id).await, (true, 1));
        if expire {
            o.s.rig.world.advance(chrono::Duration::hours(2));
        }
        let mut body = response.into_body().into_data_stream();
        let piece = body
            .next()
            .await
            .expect("archive bytes or the actual current source refusal");
        assert_eq!(piece.is_err(), expire);
        assert_eq!(marks(&o, export.id).await, (true, 1));
    }
}
