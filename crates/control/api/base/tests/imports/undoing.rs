//! A native undo retains its own credential through every actual deletion, with write-only receipts.
use super::*;
use orch_api_contract::imports::{ImportUndoReceipt, ImportUndoResponse};

const OP: &str = "ImportUndo";
async fn subjects(o: &Office, import: Id) -> Vec<Id> {
    sqlx::query_scalar("SELECT o.object_id FROM import_origins o JOIN documents d ON d.id=o.object_id WHERE o.created_by_import=$1 AND o.kind='document' AND d.deleted_at IS NULL ORDER BY o.object_id")
        .bind(import).fetch_all(o.s.rig.db.pool()).await.unwrap()
}
fn rules(import: Id, ids: &[Id]) -> Vec<Value> {
    std::iter::once(rule(OP, "import", import, "self", &["import"], &["manage"]))
        .chain(
            ids.iter()
                .map(|id| rule(OP, "document", *id, "self", &["document"], &["manage"])),
        )
        .collect()
}
async fn call(o: &Office, token: &str, import: Id) -> RawReply {
    let call = o.s.rig.world.app.call(ImportUndo, &[&import]);
    call.token(token)
        .body(&ImportUndoBody::default())
        .send_bytes()
        .await
}
fn receipt(reply: RawReply) -> ImportUndoReceipt {
    assert_eq!(
        reply.status,
        StatusCode::ACCEPTED,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    let value: Value = serde_json::from_slice(&reply.raw).unwrap();
    assert_eq!(value.as_object().unwrap().len(), 2);
    match serde_json::from_value::<ImportUndoResponse>(value).unwrap() {
        ImportUndoResponse::Receipt(value) => value,
        ImportUndoResponse::Full(_) => {
            panic!("Manage without Read must return the actual native receipt")
        }
    }
}
async fn wait(o: &Office, pid: i32, pattern: &str) {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE $2)")
                .bind(pid).bind(pattern).fetch_one(o.s.rig.db.pool()).await.unwrap();
            if waiting { break; }
            tokio::task::yield_now().await;
        }
    }).await.expect("the actual native undo writer wait");
}

#[tokio::test]
async fn undo_admits_exact_manage_sources_and_returns_a_truthful_job_receipt_without_read() {
    let (o, import) = scene().await;
    let ids = subjects(&o, import.id).await;
    let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs")
        .fetch_one(o.s.rig.db.pool())
        .await
        .unwrap();
    for wrong in [
        rules(import.id, &ids[..1]),
        rules(new_id(), &ids),
        vec![
            rule(OP, "import", import.id, "self", &["import"], &["read"]),
            rule(
                OP,
                "topic",
                o.topic(),
                "subtree",
                &["document"],
                &["manage"],
            ),
        ],
        vec![
            rule(
                "ImportUndoPreflight",
                "import",
                import.id,
                "self",
                &["import"],
                &["manage"],
            ),
            rule(
                OP,
                "topic",
                o.topic(),
                "subtree",
                &["document"],
                &["manage"],
            ),
        ],
    ] {
        let denied = key(&o, &o.s.ada, wrong).await;
        assert!(
            call(&o, &denied.secret, import.id)
                .await
                .status
                .is_client_error()
        );
        assert_eq!(subjects(&o, import.id).await, ids);
    }
    let foreign = key(&o, &o.s.bea, rules(import.id, &ids)).await;
    assert_eq!(
        call(&o, &foreign.secret, import.id).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM jobs")
            .fetch_one(o.s.rig.db.pool())
            .await
            .unwrap(),
        jobs
    );
    let manager = key(&o, &o.s.ada, rules(import.id, &ids)).await;
    let accepted = receipt(call(&o, &manager.secret, import.id).await);
    let plan: Value = sqlx::query_scalar("SELECT plan FROM jobs WHERE id=$1")
        .bind(accepted.id)
        .fetch_one(o.s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(plan["undo_operation"]["credential"]["kind"], "token");
    assert_eq!(
        plan["undo_operation"]["credential"]["id"],
        json!(manager.id)
    );
    assert_eq!(plan["undo_operation"]["user"], json!(o.s.ada.id()));
    assert_eq!(plan["undo_operation_required"], true);
    let ended = o.ended(&o.s.ada, accepted.id).await;
    assert_eq!(ended.state, JobState::Succeeded, "{:?}", ended.error);
    assert!(subjects(&o, import.id).await.is_empty());
    let original: bool = sqlx::query_scalar("SELECT deleted_at IS NULL FROM documents WHERE id=$1")
        .bind(o.document.base.id)
        .fetch_one(o.s.rig.db.pool())
        .await
        .unwrap();
    assert!(original, "an unrelated preexisting document stays");
}

#[tokio::test]
async fn undo_full_job_metadata_uses_only_its_own_import_and_actual_creator_reads() {
    for creator in [false, true] {
        let (o, import) = scene().await;
        let ids = subjects(&o, import.id).await;
        let mut grants = rules(import.id, &ids);
        grants.push(rule(
            OP,
            "import",
            import.id,
            "self",
            &["import"],
            &["read"],
        ));
        if creator {
            grants.push(rule(
                OP,
                "account",
                o.s.ada.id(),
                "self",
                &["account"],
                &["read"],
            ));
        } else {
            grants.push(rule(
                "UserGet",
                "account",
                o.s.ada.id(),
                "self",
                &["account"],
                &["read"],
            ));
        }
        let manager = key(&o, &o.s.ada, grants).await;
        let reply = call(&o, &manager.secret, import.id).await;
        let id = if creator {
            assert_eq!(reply.status, StatusCode::ACCEPTED);
            let full = serde_json::from_slice::<ImportUndoResponse>(&reply.raw)
                .unwrap()
                .into_full()
                .unwrap();
            assert_eq!(full.kind, "import");
            assert_eq!(full.created_by.unwrap().id, o.s.ada.id());
            full.id
        } else {
            receipt(reply).id
        };
        assert_eq!(o.ended(&o.s.ada, id).await.state, JobState::Succeeded);
    }
}

#[tokio::test]
async fn queued_undo_rechecks_the_original_key_and_native_sources_after_the_real_document_lock_wait()
 {
    for mode in [
        "unchanged",
        "org_refused",
        "expired",
        "moved",
        "missing",
        "invalid",
    ] {
        let (o, import) = scene().await;
        let ids = subjects(&o, import.id).await;
        let manager = key(&o, &o.s.ada, rules(import.id, &ids)).await;
        let mut blocker = o.s.rig.db.pool().begin().await.unwrap();
        sqlx::query("SELECT id FROM documents WHERE id=$1 FOR UPDATE")
            .bind(ids[0])
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let accepted = receipt(call(&o, &manager.secret, import.id).await);
        wait(&o, pid, "%documents%").await;
        match mode {
            "org_refused" => {
                // The worker already shares the key row. A refusal in this org
                // narrows the same live key without rewriting the locked row.
                let mut policy = o.s.rig.db.begin().await.unwrap();
                assert!(
                    orch_db::queries::tokens::refuse(
                        &mut policy,
                        manager.id,
                        o.s.tree.org.base.id,
                        o.s.ada.id(),
                        o.s.rig.world.now(),
                    )
                    .await
                    .unwrap()
                );
                orch_db::queries::access::bump_epoch(&mut policy)
                    .await
                    .unwrap();
                policy.commit().await.unwrap();
            }
            "expired" => o.s.rig.world.advance(chrono::Duration::hours(2)),
            "moved" => {
                sqlx::query("UPDATE documents SET node_id=$2 WHERE id=$1")
                    .bind(ids[0])
                    .bind(o.s.tree.project.base.id)
                    .execute(&mut *blocker)
                    .await
                    .unwrap();
            }
            "missing" => {
                sqlx::query("UPDATE jobs SET plan=plan-'undo_operation' WHERE id=$1")
                    .bind(accepted.id)
                    .execute(o.s.rig.db.pool())
                    .await
                    .unwrap();
            }
            "invalid" => {
                sqlx::query("UPDATE jobs SET plan=jsonb_set(plan,'{undo_operation}','null'::jsonb) WHERE id=$1").bind(accepted.id)
                .execute(o.s.rig.db.pool()).await.unwrap();
            }
            _ => {}
        }
        blocker.commit().await.unwrap();
        let ended = o.ended(&o.s.ada, accepted.id).await;
        if mode == "unchanged" {
            assert_eq!(ended.state, JobState::Succeeded, "{:?}", ended.error);
            assert!(subjects(&o, import.id).await.is_empty());
        } else {
            assert_eq!(ended.state, JobState::Failed, "{mode}: {:?}", ended.error);
            assert_eq!(
                subjects(&o, import.id).await,
                ids,
                "{mode}: no deletion follows the lost original source"
            );
        }
    }
}

#[tokio::test]
async fn a_committed_undo_queue_keeps_its_receipt_after_activity_wait_expiry() {
    let (o, import) = scene().await;
    let ids = subjects(&o, import.id).await;
    let manager = key(&o, &o.s.ada, rules(import.id, &ids)).await;
    let mut blocker = o.s.rig.db.pool().begin().await.unwrap();
    sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
        .execute(&mut *blocker)
        .await
        .unwrap();
    sqlx::query("SELECT id FROM documents WHERE id=$1 FOR UPDATE")
        .bind(ids[0])
        .fetch_one(&mut *blocker)
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
        let call = app.call(ImportUndo, &[&id]);
        call.token(&token)
            .body(&ImportUndoBody::default())
            .send_bytes()
            .await
    });
    activity_wait(&o, pid, &request).await;
    let accepted: Id = sqlx::query_scalar("SELECT undo_job_id FROM imports WHERE id=$1")
        .bind(import.id)
        .fetch_one(o.s.rig.db.pool())
        .await
        .unwrap();
    o.s.rig.world.advance(chrono::Duration::hours(2));
    blocker.commit().await.unwrap();
    let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(receipt(reply).id, accepted);
    assert_eq!(o.ended(&o.s.ada, accepted).await.state, JobState::Failed);
    assert_eq!(subjects(&o, import.id).await, ids);
}

#[tokio::test]
async fn undoing_merged_history_and_messages_uses_their_real_thread_and_chat_without_deleting_the_home()
 {
    use orch_api_contract::imports::{ImportObjectChoice, ImportRoot};
    let o = Office::new().await;
    let before_items: i64 = sqlx::query_scalar("SELECT count(*) FROM items WHERE thread_id=$1")
        .bind(o.thread.base.id)
        .fetch_one(o.s.rig.db.pool())
        .await
        .unwrap();
    let before_messages: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM chat_messages WHERE chat_id=ANY($1) AND deleted_at IS NULL",
    )
    .bind(vec![o.topic_chat, o.thread_chat])
    .fetch_one(o.s.rig.db.pool())
    .await
    .unwrap();
    let package = package_of(&o, o.topic()).await;
    let file = upload(&o, &o.s.ada, "merge-history.zip", &package).await;
    let mut request = asked("scoped-undo-merged-history", &file);
    request.destination.root = Some(ImportRoot::Here);
    request.choices.objects = vec![
        ImportObjectChoice {
            source: o.thread.base.id,
            action: ImportAction::Replace,
        },
        ImportObjectChoice {
            source: o.document.base.id,
            action: ImportAction::Skip,
        },
    ];
    let import = imported(&o, &o.s.ada, o.topic(), &request).await;
    let added_items: i64 = sqlx::query_scalar("SELECT count(*) FROM items WHERE thread_id=$1")
        .bind(o.thread.base.id)
        .fetch_one(o.s.rig.db.pool())
        .await
        .unwrap();
    let added_messages: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM chat_messages WHERE chat_id=ANY($1) AND deleted_at IS NULL",
    )
    .bind(vec![o.topic_chat, o.thread_chat])
    .fetch_one(o.s.rig.db.pool())
    .await
    .unwrap();
    assert!(added_items > before_items);
    assert!(added_messages > before_messages);
    let manager = key(
        &o,
        &o.s.ada,
        vec![
            rule(OP, "import", import.id, "self", &["import"], &["manage"]),
            rule(
                OP,
                "thread",
                o.thread.base.id,
                "self",
                &["thread"],
                &["manage"],
            ),
            rule(OP, "chat", o.topic_chat, "self", &["chat"], &["manage"]),
            rule(OP, "chat", o.thread_chat, "self", &["chat"], &["manage"]),
        ],
    )
    .await;
    let accepted = receipt(call(&o, &manager.secret, import.id).await);
    let ended = o.ended(&o.s.ada, accepted.id).await;
    assert_eq!(ended.state, JobState::Succeeded, "{:?}", ended.error);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM items WHERE thread_id=$1")
            .bind(o.thread.base.id)
            .fetch_one(o.s.rig.db.pool())
            .await
            .unwrap(),
        before_items
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM chat_messages WHERE chat_id=ANY($1) AND deleted_at IS NULL"
        )
        .bind(vec![o.topic_chat, o.thread_chat])
        .fetch_one(o.s.rig.db.pool())
        .await
        .unwrap(),
        before_messages
    );
    assert!(
        sqlx::query_scalar::<_, bool>("SELECT deleted_at IS NULL FROM nodes WHERE id=$1")
            .bind(o.thread.base.id)
            .fetch_one(o.s.rig.db.pool())
            .await
            .unwrap()
    );
}

async fn retry(o: &Office, token: &str, act: &str, import: Id) -> RawReply {
    let call = o.s.rig.world.app.call(ImportUndo, &[&import]);
    call.token(token)
        .header("Idempotency-Key", act)
        .body(&ImportUndoBody::default())
        .send_bytes()
        .await
}
#[tokio::test]
async fn a_completed_undo_creation_replays_its_current_job_without_admitting_deleted_subjects_again()
 {
    let (o, import) = scene().await;
    let ids = subjects(&o, import.id).await;
    let manager = key(&o, &o.s.ada, rules(import.id, &ids)).await;
    let act = new_id().to_string();
    let accepted = receipt(retry(&o, &manager.secret, &act, import.id).await);
    assert_eq!(
        o.ended(&o.s.ada, accepted.id).await.state,
        JobState::Succeeded
    );
    assert!(subjects(&o, import.id).await.is_empty());
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs")
        .fetch_one(o.s.rig.db.pool())
        .await
        .unwrap();
    let original: Value = sqlx::query_scalar("SELECT plan->'undo_operation' FROM jobs WHERE id=$1")
        .bind(accepted.id)
        .fetch_one(o.s.rig.db.pool())
        .await
        .unwrap();
    let reader = key(
        &o,
        &o.s.ada,
        vec![
            rule(OP, "import", import.id, "self", &["import"], &["read"]),
            rule(OP, "account", o.s.ada.id(), "self", &["account"], &["read"]),
        ],
    )
    .await;
    let reply = retry(&o, &reader.secret, &act, import.id).await;
    assert_eq!(
        reply.status,
        StatusCode::ACCEPTED,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    let full = serde_json::from_slice::<ImportUndoResponse>(&reply.raw)
        .unwrap()
        .into_full()
        .unwrap();
    assert_eq!(full.id, accepted.id);
    assert_eq!(full.state, JobState::Succeeded);
    let wrong = key(
        &o,
        &o.s.ada,
        vec![
            rule(
                "ImportGet",
                "import",
                import.id,
                "self",
                &["import"],
                &["read"],
            ),
            rule(OP, "account", o.s.ada.id(), "self", &["account"], &["read"]),
        ],
    )
    .await;
    assert_eq!(
        receipt(retry(&o, &wrong.secret, &act, import.id).await),
        accepted
    );
    assert!(
        retry(&o, &reader.secret, &new_id().to_string(), import.id)
            .await
            .status
            .is_client_error()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM jobs")
            .fetch_one(o.s.rig.db.pool())
            .await
            .unwrap(),
        before
    );
    assert_eq!(
        sqlx::query_scalar::<_, Value>("SELECT plan->'undo_operation' FROM jobs WHERE id=$1")
            .bind(accepted.id)
            .fetch_one(o.s.rig.db.pool())
            .await
            .unwrap(),
        original
    );
}
