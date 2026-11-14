//! Actual native writer waits must not keep an expired authenticated key alive.
use super::*;
use std::time::Duration as WaitDuration;

/// Reuse the native invitation lock-witness budget: prove a real edge, never sleep to line up a race.
async fn blocked(s: &Scene, locker: i32) -> Result<(), tokio::time::error::Elapsed> {
    tokio::time::timeout(WaitDuration::from_secs(10),async {
        loop {
            let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND $1=ANY(pg_blocking_pids(pid)))")
                .bind(locker).fetch_one(s.rig.db.pool()).await.unwrap();
            if waiting {break;}
            tokio::time::sleep(WaitDuration::from_millis(10)).await;
        }
    }).await
}
async fn expires_after_wait(decision: bool) {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let (id, actor) = if decision {
        (
            asking(&s, thread, &[("expired-answer", command())]).await[0],
            s.ada.id(),
        )
    } else {
        (access_request(&s, thread).await, s.cai.id())
    };
    let caller = legacy(&s, actor).await;
    checked(get(&s, &caller.secret, id).await, StatusCode::OK);
    sqlx::query("UPDATE api_tokens SET expires_at=$2 WHERE id=$1")
        .bind(caller.id)
        .bind(s.rig.world.now() + chrono::Duration::seconds(1))
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let mut held = s.rig.db.pool().begin().await.unwrap();
    let locker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *held)
        .await
        .unwrap();
    sqlx::query("SELECT id FROM approvals WHERE id=$1 FOR UPDATE")
        .bind(id)
        .execute(&mut *held)
        .await
        .unwrap();
    let app = s.rig.world.app.clone();
    let mut pending = tokio::spawn(async move {
        if decision {
            let request = app
                .call(ApprovalDecide, &[&id])
                .token(&caller.secret)
                .raw_body(&json!({"option_id":"allow_once"}));
            request.send_bytes().await
        } else {
            let request = app.call(ApprovalCancel, &[&id]).token(&caller.secret);
            request.send_bytes().await
        }
    });
    tokio::select! {
        finished=&mut pending=>{
            held.rollback().await.unwrap();
            let reply=finished.unwrap();
            let body:Json=serde_json::from_slice(&reply.raw).unwrap();
            panic!("operation completed before its real native writer wait: status={}, code={}; no expiry-after-wait premise",reply.status,body["code"]);
        }
        witness=blocked(&s,locker)=>{
            if witness.is_err(){pending.abort();let _=pending.await;held.rollback().await.unwrap();panic!("no exact native writer lock edge was observed");}
        }
    }
    s.rig.world.advance(chrono::Duration::seconds(2));
    held.commit().await.unwrap();
    checked(pending.await.unwrap(), StatusCode::UNAUTHORIZED);
    assert_eq!(state(&s, id).await, "pending");
    assert!(s.rig.seen.of_kind(CommandKind::ApprovalResolve).is_empty());
    assert!(!has(&s, &s.cai, thread, Permission::ThreadInteract).await);
}
#[tokio::test]
async fn legacy_harness_decider_deadline_is_rechecked_after_the_real_approval_lock() {
    expires_after_wait(true).await;
}
#[tokio::test]
async fn legacy_canceller_deadline_is_rechecked_after_the_real_approval_lock() {
    expires_after_wait(false).await;
}
