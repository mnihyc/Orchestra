//! Undo summaries bind actual native governors and keep their existing counts-only body.
use super::*;
use orch_api_contract::imports::ImportUndoPlan;

const OP: &str = "ImportUndoPreflight";
fn own(import: Id) -> Value {
    rule(OP, "import", import, "self", &["import"], &["manage"])
}
fn subject(id: Id) -> Value {
    rule(OP, "document", id, "self", &["document"], &["manage"])
}
async fn subjects(o: &Office, import: Id) -> Vec<Id> {
    sqlx::query_scalar("SELECT o.object_id FROM import_origins o JOIN documents d ON d.id=o.object_id WHERE o.created_by_import=$1 AND o.kind='document' AND d.deleted_at IS NULL ORDER BY o.object_id")
        .bind(import).fetch_all(o.s.rig.db.pool()).await.unwrap()
}
async fn call(o: &Office, token: &str, import: Id) -> RawReply {
    let call = o.s.rig.world.app.call(ImportUndoPreflight, &[&import]);
    call.token(token).send_bytes().await
}
fn read(reply: RawReply) -> ImportUndoPlan {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}

#[tokio::test]
async fn undo_summary_requires_its_own_manage_on_the_import_and_each_actual_document_without_read()
{
    let (o, import) = scene().await;
    let ids = subjects(&o, import.id).await;
    assert_eq!(ids.len(), 2);
    let native = read(call(&o, &o.s.ada.token, import.id).await);
    assert_eq!(native.counts.documents, 2);
    let mut all = vec![own(import.id)];
    all.extend(ids.iter().copied().map(subject));
    let manager = key(&o, &o.s.ada, all).await;
    assert_eq!(read(call(&o, &manager.secret, import.id).await), native);
    for rules in [
        vec![own(import.id)],
        vec![own(import.id), subject(ids[0])],
        vec![subject(ids[0]), subject(ids[1])],
        vec![
            rule(OP, "import", import.id, "self", &["import"], &["read"]),
            subject(ids[0]),
            subject(ids[1]),
        ],
        vec![
            own(import.id),
            rule(
                "ImportUndo",
                "topic",
                o.topic(),
                "subtree",
                &["document"],
                &["manage"],
            ),
        ],
        vec![
            own(import.id),
            rule(
                OP,
                "document_container",
                ids[0],
                "self",
                &["document_container"],
                &["manage"],
            ),
            subject(ids[1]),
        ],
    ] {
        let denied = key(&o, &o.s.ada, rules).await;
        let reply = call(&o, &denied.secret, import.id).await;
        assert!(reply.status.is_client_error());
        assert!(!String::from_utf8_lossy(&reply.raw).contains("first-source"));
    }
    let mut foreign = vec![own(import.id)];
    foreign.extend(ids.iter().copied().map(subject));
    let stranger = key(&o, &o.s.bea, foreign).await;
    assert_eq!(
        call(&o, &stranger.secret, import.id).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        subjects(&o, import.id).await,
        ids,
        "preflight changes nothing"
    );
}

#[tokio::test]
async fn a_genuinely_empty_undo_summary_keeps_its_real_import_identity_and_existing_body() {
    let (o, import) = scene().await;
    let job =
        o.s.rig
            .world
            .app
            .call(ImportUndo, &[&import.id])
            .token(&o.s.ada.token)
            .body(&ImportUndoBody::default())
            .send()
            .await
            .ok();
    o.ended(&o.s.ada, job.id).await;
    assert!(subjects(&o, import.id).await.is_empty());
    let native = read(call(&o, &o.s.ada.token, import.id).await);
    let manager = key(&o, &o.s.ada, vec![own(import.id)]).await;
    assert_eq!(read(call(&o, &manager.secret, import.id).await), native);
    assert_eq!(native.counts.documents, 0);
    assert!(!native.blockers.is_empty());
}

#[tokio::test]
async fn undo_summary_publication_rechecks_original_native_subjects_and_current_authority() {
    for mode in ["unchanged", "scope_closed", "moved", "expired"] {
        let (o, import) = scene().await;
        let ids = subjects(&o, import.id).await;
        let mut rules = vec![own(import.id)];
        rules.extend(ids.iter().copied().map(subject));
        let manager = key(&o, &o.s.ada, rules).await;
        let before = read(call(&o, &manager.secret, import.id).await);
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
        let id = import.id;
        let token = manager.secret.clone();
        let request = tokio::spawn(async move {
            let call = app.call(ImportUndoPreflight, &[&id]);
            call.token(&token).send_bytes().await
        });
        activity_wait(&o, pid, &request).await;
        match mode {
            "scope_closed" => {
                let scope: KeyScope =
                    serde_json::from_value(json!({"rules":[own(import.id),subject(ids[0])]}))
                        .unwrap();
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(manager.id)
                    .bind(json!(scope))
                    .execute(o.s.rig.db.pool())
                    .await
                    .unwrap();
            }
            "moved" => {
                sqlx::query("UPDATE documents SET node_id=$2 WHERE id=$1")
                    .bind(ids[0])
                    .bind(o.s.tree.project.base.id)
                    .execute(o.s.rig.db.pool())
                    .await
                    .unwrap();
            }
            "expired" => o.s.rig.world.advance(chrono::Duration::hours(2)),
            _ => {}
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request)
            .await
            .unwrap()
            .unwrap();
        match mode {
            "unchanged" => assert_eq!(read(reply), before),
            "expired" => assert_eq!(reply.status, StatusCode::UNAUTHORIZED),
            _ => assert!(reply.status.is_client_error()),
        }
    }
}
