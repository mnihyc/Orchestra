//! One native fork and original Delete, with independent real destination and current source proof.
use super::*;
use orch_api_contract::duplicates::{
    ThreadMoveOut, ThreadMoveOutPreview, ThreadMoveOutPreviewResponse,
};
use orch_api_core::testing::RawReply;
use thread_history_scope::{Proof, key, ok, rule};

#[tokio::test(flavor = "multi_thread")]
async fn a_main_move_names_every_open_side_and_waits_for_native_close_or_keep() -> Proof {
    let w = Forks::new().await;
    let source = w.thread_with_history("Main with a side").await;
    let topic = destination(&w).await;
    let side = start_side(&w, source, "move-side-1").await?;
    let response: ThreadMoveOutPreviewResponse =
        serde_json::from_value(ok(&preview(&w, source, topic, &w.s.ada.token).await)?)?;
    let sides = response.open_side_threads.ok_or("side review absent")?;
    assert_eq!(sides.len(), 1);
    assert_eq!(sides[0].id, side);
    assert_eq!(sides[0].name.as_deref(), Some("Main with a side (side)"));
    let plan = response.plan.into_full().map_err(|_| "plan absent")?;
    assert!(plan.blockers.iter().any(|blocker| {
        blocker.code == "open_side_thread" && blocker.message.contains("Main with a side (side)")
    }));
    let refused = confirm(&w, source, plan.preflight_id, &w.s.ada.token, false).await;
    assert!(!refused.status.is_success());
    assert_eq!(
        w.count("SELECT count(*) FROM jobs WHERE kind='thread_fork'")
            .await,
        0
    );
    let closed =
        w.s.rig
            .world
            .app
            .call(orch_api_contract::ownership::NodeDelete, &[&side])
            .token(&w.s.ada.token)
            .body(&orch_api_contract::ownership::NodeDeleteBody::default())
            .send_bytes()
            .await;
    assert_eq!(
        closed.status,
        200,
        "{}",
        String::from_utf8_lossy(&closed.raw)
    );
    let response: ThreadMoveOutPreviewResponse =
        serde_json::from_value(ok(&preview(&w, source, topic, &w.s.ada.token).await)?)?;
    assert!(
        response
            .open_side_threads
            .ok_or("refreshed sides absent")?
            .is_empty()
    );
    let plan = response
        .plan
        .into_full()
        .map_err(|_| "refreshed plan absent")?;
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    let accepted = confirm(&w, source, plan.preflight_id, &w.s.ada.token, false).await;
    assert_eq!(
        accepted.status,
        202,
        "{}",
        String::from_utf8_lossy(&accepted.raw)
    );
    let job: Id =
        serde_json::from_value(serde_json::from_slice::<Json>(&accepted.raw)?["job_id"].clone())?;
    let finished = over(&w, job).await;
    assert_eq!(finished.state, "succeeded", "{:?}", finished.error);
    let target: Id = serde_json::from_value(finished.plan["new_thread"].clone())?;
    let copied_sides: i64 = sqlx::query_scalar("SELECT count(*) FROM threads WHERE side_of=$1")
        .bind(target)
        .fetch_one(w.s.rig.db.pool())
        .await?;
    assert_eq!(
        copied_sides, 0,
        "the native fork does not recreate side conversations"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn opening_a_side_after_the_main_move_review_cannot_erase_that_side() -> Proof {
    let w = Forks::new().await;
    let source = w.thread_with_history("A newly opened side").await;
    let topic = destination(&w).await;
    let response: ThreadMoveOutPreviewResponse =
        serde_json::from_value(ok(&preview(&w, source, topic, &w.s.ada.token).await)?)?;
    let plan = response.plan.into_full().map_err(|_| "plan absent")?;
    let side = start_side(&w, source, "move-side-after-review").await?;
    let refused = confirm(&w, source, plan.preflight_id, &w.s.ada.token, false).await;
    assert!(!refused.status.is_success());
    let alive: i64 =
        sqlx::query_scalar("SELECT count(*) FROM nodes WHERE id=ANY($1) AND deleted_at IS NULL")
            .bind(vec![source, side])
            .fetch_one(w.s.rig.db.pool())
            .await?;
    assert_eq!(alive, 2);
    assert_eq!(
        w.count("SELECT count(*) FROM jobs WHERE kind='thread_fork'")
            .await,
        0
    );
    Ok(())
}

async fn start_side(w: &Forks, source: Id, client: &str) -> Proof<Id> {
    let response =
        w.s.rig
            .world
            .app
            .call(orch_api_contract::threads::ThreadSideCreate, &[&source])
            .token(&w.s.ada.token)
            .body(&orch_api_contract::threads::ThreadSideCreateBody {
                content: vec![orch_api_contract::threads::ContentPart::Text {
                    text: "Read this aside".into(),
                }],
                client_id: client.into(),
                to_turn: None,
            })
            .send()
            .await;
    assert_eq!(
        response.status,
        201,
        "{}",
        String::from_utf8_lossy(&response.raw)
    );
    let full = response
        .body
        .map_err(|error| format!("{error:?}"))?
        .into_full()
        .map_err(|_| "side response was only a receipt")?;
    Ok(full.thread.base.id)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_manage_only_move_preview_returns_its_wrapper_receipt_without_source_or_audience() -> Proof
{
    let w = Forks::new().await;
    let source = w
        .thread_with_history("Private reviewed moving history")
        .await;
    let topic = destination(&w).await;
    let selected = key(
        &w.s.rig.world,
        w.s.ada.id(),
        vec![
            rule("ThreadMoveOutPreview", "thread", source, "self", "manage"),
            json!({"target":{"kind":"topic","id":topic,"reach":"self"},"resources":["thread"],"allow":[{"operation":"ThreadMoveOutPreview","actions":["write"]}]}),
        ],
    )
    .await?;
    let reply = preview(&w, source, topic, &selected.secret).await;
    let response: ThreadMoveOutPreviewResponse = serde_json::from_value(ok(&reply)?)?;
    assert!(response.delete_plan.is_none());
    assert!(response.notification.is_none());
    assert!(response.open_side_threads.is_none());
    let receipt = match response.plan {
        orch_api_contract::forks::ForkPlanResponse::Receipt(receipt) => receipt,
        _ => return Err("a manage-only key disclosed a full plan".into()),
    };
    assert_ne!(receipt.preflight_id, Id::nil());
    assert!(!String::from_utf8_lossy(&reply.raw).contains("Private reviewed moving history"));
    assert_eq!(
        confirm(&w, source, receipt.preflight_id, &selected.secret, false)
            .await
            .status,
        403,
        "the preview does not grant the separately named Move act"
    );
    assert_eq!(
        w.count("SELECT count(*) FROM jobs WHERE kind='thread_fork'")
            .await,
        0
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn native_management_and_destination_creation_do_not_substitute_for_thread_ownership() -> Proof
{
    let w = Forks::new().await;
    let source = w.thread_with_history("Only actual owners can move").await;
    for permission in ["thread.view", "thread.manage"] {
        w.s.rig
            .lets_use("node", source, permission, &w.s.eve, json!({}))
            .await;
    }
    let world = &w.s.rig.world;
    let org = places::make_org(world, &w.s.eve, "Manager recipient", "manager-recipient")
        .await
        .base
        .id;
    let project = places::make_project(world, &w.s.eve, org, "Project")
        .await
        .base
        .id;
    let topic = places::make_topic(world, &w.s.eve, project, "Topic")
        .await
        .base
        .id;
    let reply = preview(&w, source, topic, &w.s.eve.token).await;
    assert_eq!(reply.status, 403, "{}", String::from_utf8_lossy(&reply.raw));
    assert_eq!(
        w.count("SELECT count(*) FROM jobs WHERE kind='thread_fork'")
            .await,
        0
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn raw_closure_after_native_history_open_keeps_the_original_thread() -> Proof {
    let w = Forks::new().await;
    let source = w.thread_with_history("A current source key").await;
    let topic = destination(&w).await;
    let response: ThreadMoveOutPreviewResponse =
        serde_json::from_value(ok(&preview(&w, source, topic, &w.s.ada.token).await)?)?;
    let plan = response.plan.into_full().map_err(|_| "plan absent")?;
    let selected = key(&w.s.rig.world, w.s.ada.id(), vec![
        rule("ThreadMoveOut", "thread", source, "self", "manage"),
        json!({"target":{"kind":"topic","id":topic,"reach":"self"},"resources":["thread"],"allow":[{"operation":"ThreadMoveOut","actions":["write"]}]}),
    ]).await?;
    let opening = CommandKind::SessionOpenFenced;
    let before = w.s.rig.commands.issued_of(opening.as_str());
    w.s.rig.machine.handler.hold(opening);
    let reply = confirm(&w, source, plan.preflight_id, &selected.secret, false).await;
    assert_eq!(reply.status, 202, "{}", String::from_utf8_lossy(&reply.raw));
    let id: Id =
        serde_json::from_value(serde_json::from_slice::<Json>(&reply.raw)?["job_id"].clone())?;
    eventually("the native history open is held", || async {
        w.s.rig.commands.issued_of(opening.as_str()) > before
    })
    .await;
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
        .bind(selected.id)
        .bind(w.s.rig.world.now())
        .execute(w.s.rig.db.pool())
        .await?;
    w.s.rig.machine.handler.release();
    assert_eq!(over(&w, id).await.state, "failed");
    let alive: bool = sqlx::query_scalar("SELECT deleted_at IS NULL FROM nodes WHERE id=$1")
        .bind(source)
        .fetch_one(w.s.rig.db.pool())
        .await?;
    assert!(alive);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn thread_move_notify_choice_is_reviewed_and_survives_source_ending() -> Proof {
    for (notify, late_failure) in [(false, false), (true, false), (true, true)] {
        let w = Forks::new().await;
        let source = w.thread_with_history("Another person's conversation").await;
        // Eve has real native profile/provider/environment Use; Ada authored the source.
        w.s.rig
            .lets_use("node", source, "access.own", &w.s.eve, json!({}))
            .await;
        let world = &w.s.rig.world;
        let org = places::make_org(world, &w.s.eve, "Eve recipient", "eve-recipient")
            .await
            .base
            .id;
        let project = places::make_project(world, &w.s.eve, org, "Project")
            .await
            .base
            .id;
        let topic = places::make_topic(world, &w.s.eve, project, "Topic")
            .await
            .base
            .id;
        let response: ThreadMoveOutPreviewResponse =
            serde_json::from_value(ok(&preview(&w, source, topic, &w.s.eve.token).await)?)?;
        assert_eq!(
            response
                .notification
                .ok_or("reviewed audience absent")?
                .people,
            1
        );
        let plan = response
            .plan
            .into_full()
            .map_err(|_| "source plan absent")?;
        assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
        if late_failure {
            let mut conn = w.s.rig.db.acquire().await?;
            let ddl = format!(
                "CREATE FUNCTION refuse_notifying_thread_move() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.id='{}'::uuid AND NEW.deleted_at IS NOT NULL THEN RAISE EXCEPTION 'synthetic late notifying source failure'; END IF; RETURN NEW; END $$",
                source
            );
            sqlx::query(sqlx::AssertSqlSafe(ddl))
                .execute(&mut *conn)
                .await?;
            sqlx::query("CREATE TRIGGER refuse_notifying_thread_move BEFORE UPDATE ON nodes FOR EACH ROW EXECUTE FUNCTION refuse_notifying_thread_move()").execute(&mut *conn).await?;
        }
        let reply = confirm(&w, source, plan.preflight_id, &w.s.eve.token, notify).await;
        assert_eq!(reply.status, 202, "{}", String::from_utf8_lossy(&reply.raw));
        let id: Id =
            serde_json::from_value(serde_json::from_slice::<Json>(&reply.raw)?["job_id"].clone())?;
        let row = over(&w, id).await;
        if late_failure {
            assert_eq!(row.state, "failed", "{:?}", row.error);
            let notices: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM notifications WHERE data->>'operation_id'=$1",
            )
            .bind(id.to_string())
            .fetch_one(w.s.rig.db.pool())
            .await?;
            assert_eq!(notices, 0);
            assert!(
                sqlx::query_scalar::<_, bool>("SELECT deleted_at IS NULL FROM nodes WHERE id=$1")
                    .bind(source)
                    .fetch_one(w.s.rig.db.pool())
                    .await?
            );
            continue;
        }
        assert_eq!(row.state, "succeeded", "{:?}", row.error);
        let target: Id = serde_json::from_value(row.plan["new_thread"].clone())?;
        let notices: Vec<NoticeRow> = sqlx::query_as("SELECT node_id,thread_id,chat_id,data FROM notifications WHERE user_id=$1 AND kind='person.notify' AND data->>'operation_id'=$2")
            .bind(w.s.ada.id()).bind(id.to_string()).fetch_all(w.s.rig.db.pool()).await?;
        assert_eq!(notices.len(), usize::from(notify));
        if let Some((node, thread, chat, data)) = notices.first() {
            assert_eq!((*node, *thread, *chat), (None, None, None));
            assert_eq!(data["path"], format!("/thread/{target}"));
        }
        assert_eq!(
            world
                .app
                .call(NodeGet, &[&target])
                .token(&w.s.ada.token)
                .send_bytes()
                .await
                .status,
            403,
            "a notice does not enroll its recipient in the new conversation"
        );
    }
    Ok(())
}

async fn destination(w: &Forks) -> Id {
    let world = &w.s.rig.world;
    let org = places::make_org(world, &w.s.ada, "Moving recipient", "moving-recipient")
        .await
        .base
        .id;
    let project = places::make_project(world, &w.s.ada, org, "New project")
        .await
        .base
        .id;
    places::make_topic(world, &w.s.ada, project, "New topic")
        .await
        .base
        .id
}
async fn preview(w: &Forks, source: Id, topic: Id, token: &str) -> RawReply {
    w.s.rig
        .world
        .app
        .call(ThreadMoveOutPreview, &[&source])
        .token(token)
        .body(&ForkRequest {
            topic_id: Some(topic),
            ..w.fork()
        })
        .send_bytes()
        .await
}
async fn confirm(w: &Forks, source: Id, preflight: Id, token: &str, notify: bool) -> RawReply {
    w.s.rig
        .world
        .app
        .call(ThreadMoveOut, &[&source])
        .token(token)
        .body(&ForkCreateBody {
            preflight_id: preflight,
            options: None,
            notify: Some(notify),
        })
        .send_bytes()
        .await
}
/// A person notice row: its node, thread, chat and data.
type NoticeRow = (Option<Id>, Option<Id>, Option<Id>, Json);

async fn over(w: &Forks, id: Id) -> orch_db::queries::jobs::JobRow {
    eventually("native Thread Move finishes", || async {
        orch_db::queries::jobs::job(&mut w.s.rig.db.acquire().await.unwrap(), id)
            .await
            .unwrap()
            .is_some_and(|row| !row.is_open())
    })
    .await;
    orch_db::queries::jobs::job(&mut w.s.rig.db.acquire().await.unwrap(), id)
        .await
        .unwrap()
        .unwrap()
}
#[tokio::test(flavor = "multi_thread")]
async fn an_owner_thread_move_uses_native_history_and_fresh_defaults_then_ends_the_original_once()
-> Proof {
    let w = Forks::new().await;
    let source = w.thread_with_history("Move the conversation").await;
    let topic = destination(&w).await;
    sqlx::query("INSERT INTO settings(node_id,key,value) VALUES($1,'thread.switch_history',$2) ON CONFLICT(node_id,key) DO UPDATE SET value=excluded.value")
        .bind(w.s.tree.topic.base.id)
        .bind(json!("summary"))
        .execute(w.s.rig.db.pool())
        .await?;
    let original: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM items WHERE thread_id=$1 AND kind NOT IN ('marker','announcement')",
    )
    .bind(source)
    .fetch_one(w.s.rig.db.pool())
    .await?;
    let response: ThreadMoveOutPreviewResponse =
        serde_json::from_value(ok(&preview(&w, source, topic, &w.s.ada.token).await)?)?;
    let plan = response
        .plan
        .into_full()
        .map_err(|_| "owner preview needs its exact source Read")?;
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    assert_eq!(
        plan.history.mode,
        orch_api_contract::forks::HistoryMode::All
    );
    assert_eq!(
        response
            .delete_plan
            .as_ref()
            .ok_or("original deletion review absent")?
            .destination,
        orch_api_contract::duplicates::DuplicateDestination::Node { id: topic }
    );
    let reply = confirm(&w, source, plan.preflight_id, &w.s.ada.token, false).await;
    assert_eq!(reply.status, 202, "{}", String::from_utf8_lossy(&reply.raw));
    let receipt: Json = serde_json::from_slice(&reply.raw)?;
    assert_eq!(receipt["projection"], "receipt");
    let job: Id = serde_json::from_value(receipt["job_id"].clone())?;
    let row = over(&w, job).await;
    assert_eq!(row.state, "succeeded", "{:?}", row.error);
    let target: Id = serde_json::from_value(row.plan["new_thread"].clone())?;
    assert_ne!(target, source);
    let source_deleted: bool =
        sqlx::query_scalar("SELECT deleted_at IS NOT NULL FROM nodes WHERE id=$1")
            .bind(source)
            .fetch_one(w.s.rig.db.pool())
            .await?;
    assert!(source_deleted);
    let new: (Id, Id) = sqlx::query_as(
        "SELECT topic_id,owner_id FROM threads t JOIN nodes n ON n.id=t.node_id WHERE t.node_id=$1",
    )
    .bind(target)
    .fetch_one(w.s.rig.db.pool())
    .await?;
    assert_eq!(new, (topic, w.s.ada.id()));
    let copied: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM items WHERE thread_id=$1 AND kind NOT IN ('marker','announcement')",
    )
    .bind(target)
    .fetch_one(w.s.rig.db.pool())
    .await?;
    assert_eq!(copied, original);
    assert_eq!(
        row.plan["confirmed_history_source"]["operation"]["origin"]["operation"],
        "ThreadMoveOut"
    );
    let audits:Vec<String>=sqlx::query_scalar("SELECT action FROM audit_log WHERE object_id=$1 AND action IN ('content.copied_out','node.deleted') ORDER BY action").bind(source).fetch_all(w.s.rig.db.pool()).await?;
    assert_eq!(audits, vec!["content.copied_out", "node.deleted"]);
    Ok(())
}
#[tokio::test(flavor = "multi_thread")]
async fn a_scoped_thread_move_keeps_its_actual_destination_and_original_operation_before_native_send()
-> Proof {
    let w = Forks::new().await;
    let source = w.thread_with_history("Scoped moving conversation").await;
    let topic = destination(&w).await;
    let preview = preview(&w, source, topic, &w.s.ada.token).await;
    let response: ThreadMoveOutPreviewResponse = serde_json::from_value(ok(&preview)?)?;
    let plan = response
        .plan
        .into_full()
        .map_err(|_| "source plan absent")?;
    w.s.rig.world.app.app.engine.stop();
    let grants = vec![
        rule("ThreadMoveOut", "thread", source, "self", "manage"),
        json!({"target":{"kind":"topic","id":topic,"reach":"self"},"resources":["thread"],"allow":[{"operation":"ThreadMoveOut","actions":["write"]}]}),
    ];
    let token = key(&w.s.rig.world, w.s.ada.id(), grants).await?;
    let reply = confirm(&w, source, plan.preflight_id, &token.secret, false).await;
    assert_eq!(reply.status, 202, "{}", String::from_utf8_lossy(&reply.raw));
    let receipt: Json = serde_json::from_slice(&reply.raw)?;
    let id: Id = serde_json::from_value(receipt["job_id"].clone())?;
    let row = orch_db::queries::jobs::job(&mut *w.s.rig.db.acquire().await?, id)
        .await?
        .ok_or("job missing")?;
    let clock = w.s.rig.world.app.app.clock.clone();
    let access = orch_api_core::thread_access::AuthzAccess::new(clock.clone());
    let preparation = orch_engine::jobs::fork::operation::Preparation::from_job(&row)?
        .ok_or("actual Move preparation absent")?;
    let mut tx = w.s.rig.db.begin().await?;
    let current = preparation
        .current(&mut tx, clock.as_ref(), &access)
        .await?;
    assert_eq!(current.caller().admitted_operation(), Some("ThreadMoveOut"));
    current.check(clock.as_ref())?;
    tx.rollback().await?;
    thread_history_scope::narrow(
        &w.s.rig.world,
        &token,
        vec![rule("ThreadMoveOut", "thread", source, "self", "manage")],
    )
    .await?;
    let mut tx = w.s.rig.db.begin().await?;
    assert!(
        preparation
            .current(&mut tx, clock.as_ref(), &access)
            .await
            .is_err()
    );
    tx.rollback().await?;
    let alive: bool = sqlx::query_scalar("SELECT deleted_at IS NULL FROM nodes WHERE id=$1")
        .bind(source)
        .fetch_one(w.s.rig.db.pool())
        .await?;
    assert!(alive);
    Ok(())
}
#[tokio::test(flavor = "multi_thread")]
async fn a_thread_move_rejects_changed_original_content_and_never_accepts_an_ordinary_fork_preflight()
-> Proof {
    let w = Forks::new().await;
    let source = w.thread_with_history("Reviewed moving history").await;
    let topic = destination(&w).await;
    let request = ForkRequest {
        topic_id: Some(topic),
        ..w.fork()
    };
    let ordinary = w.plan(&w.s.ada, source, &request).await.ok();
    assert_eq!(
        confirm(&w, source, ordinary.preflight_id, &w.s.ada.token, false)
            .await
            .status,
        403
    );
    let response: ThreadMoveOutPreviewResponse =
        serde_json::from_value(ok(&preview(&w, source, topic, &w.s.ada.token).await)?)?;
    let plan = response.plan.into_full().map_err(|_| "plan absent")?;
    let mut conn = w.s.rig.db.acquire().await?;
    let seq = orch_db::sequences::next(&mut conn, source)
        .await?
        .ok_or("source sequence absent")?;
    sqlx::query("UPDATE items SET seq_updated=$2 WHERE thread_id=$1 AND kind='user_message'")
        .bind(source)
        .bind(seq)
        .execute(&mut *conn)
        .await?;
    drop(conn);
    let reply = confirm(&w, source, plan.preflight_id, &w.s.ada.token, false).await;
    assert!(
        !reply.status.is_success(),
        "a changed deletion review cannot queue its old copy"
    );
    let alive: bool = sqlx::query_scalar("SELECT deleted_at IS NULL FROM nodes WHERE id=$1")
        .bind(source)
        .fetch_one(w.s.rig.db.pool())
        .await?;
    assert!(alive);
    Ok(())
}
#[tokio::test(flavor = "multi_thread")]
async fn a_late_native_thread_delete_failure_rolls_back_the_copy_ready_and_export() -> Proof {
    let w = Forks::new().await;
    let source = w.thread_with_history("Late failing move").await;
    let topic = destination(&w).await;
    let response: ThreadMoveOutPreviewResponse =
        serde_json::from_value(ok(&preview(&w, source, topic, &w.s.ada.token).await)?)?;
    let plan = response.plan.into_full().map_err(|_| "plan absent")?;
    let mut conn = w.s.rig.db.acquire().await?;
    let ddl = format!(
        "CREATE FUNCTION refuse_thread_move() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.id='{}'::uuid AND NEW.deleted_at IS NOT NULL THEN RAISE EXCEPTION 'synthetic late source ending failure'; END IF; RETURN NEW; END $$",
        source
    );
    sqlx::query(sqlx::AssertSqlSafe(ddl))
        .execute(&mut *conn)
        .await?;
    sqlx::query("CREATE TRIGGER refuse_thread_move BEFORE UPDATE ON nodes FOR EACH ROW EXECUTE FUNCTION refuse_thread_move()").execute(&mut *conn).await?;
    drop(conn);
    let reply = confirm(&w, source, plan.preflight_id, &w.s.ada.token, false).await;
    assert_eq!(reply.status, 202);
    let id: Id =
        serde_json::from_value(serde_json::from_slice::<Json>(&reply.raw)?["job_id"].clone())?;
    assert_eq!(over(&w, id).await.state, "failed");
    let alive: bool = sqlx::query_scalar("SELECT deleted_at IS NULL FROM nodes WHERE id=$1")
        .bind(source)
        .fetch_one(w.s.rig.db.pool())
        .await?;
    assert!(alive);
    let exports: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='content.copied_out'",
    )
    .bind(source)
    .fetch_one(w.s.rig.db.pool())
    .await?;
    assert_eq!(exports, 0);
    Ok(())
}
