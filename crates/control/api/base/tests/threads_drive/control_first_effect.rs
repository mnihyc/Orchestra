//! The actual native writers preserve a transport refusal and never veto their accepted tail.
use super::*;
use orch_api_core::middleware::requests::ApiError;
use orch_engine::inputs::InputError;
use std::sync::atomic::{AtomicUsize, Ordering};

async fn principal(s: &Scene, tx: &mut orch_db::Conn, actor: Id) -> Principal {
    Principal::load(tx, Caller::session(actor), Time::new(s.rig.world.now()))
        .await
        .unwrap()
}

async fn rejected(which: &str) {
    let s = Scene::new().await;
    let expected = ApiError::new(
        ErrorCode::AuthInvalidToken,
        "the original transport deadline",
    );
    let calls = AtomicUsize::new(0);
    let check = || {
        calls.fetch_add(1, Ordering::SeqCst);
        Err(orch_engine::EngineError::BeforeEffect(Box::new(
            expected.clone(),
        )))
    };
    let (thread, result, before) = match which {
        "plan" => {
            let p = plan(&s, "First effect plan refusal").await;
            let before = snapshot(&s, p.thread).await;
            let mut tx = s.rig.db.begin().await.unwrap();
            let principal = principal(&s, &mut tx, s.ada.id()).await;
            let result = s
                .rig
                .engine
                .decide_plan_projected_guarded(
                    tx,
                    principal,
                    p.thread,
                    p.item,
                    &ThreadPlanDecideBody {
                        proceed: true,
                        text: None,
                    },
                    true,
                    check,
                )
                .await
                .map(|_| ());
            (p.thread, result, before)
        }
        "run" => {
            let i = held_input(&s, "First effect run refusal").await;
            let before = snapshot(&s, i.thread).await;
            let mut tx = s.rig.db.begin().await.unwrap();
            let principal = principal(&s, &mut tx, s.eve.id()).await;
            let result = s
                .rig
                .engine
                .run_input_projected_guarded(tx, principal, i.thread, i.input, true, check)
                .await
                .map(|_| ());
            (i.thread, result, before)
        }
        "bypass" => {
            let i = bypass_input(&s, "First effect bypass refusal", true).await;
            let before = snapshot(&s, i.thread).await;
            let mut tx = s.rig.db.begin().await.unwrap();
            let principal = principal(&s, &mut tx, s.ada.id()).await;
            let row = orch_db::queries::threads::thread(&mut tx, i.thread)
                .await
                .unwrap()
                .unwrap();
            let asked = LimitBypassRequest {
                caps: Some(i.caps),
                ..Default::default()
            };
            let homes = s
                .rig
                .engine
                .input_bypass_subjects(&mut tx, &row, s.eve.id(), &asked, s.rig.world.now())
                .await
                .unwrap();
            let result = s
                .rig
                .engine
                .bypass_input_projected_guarded(
                    tx, principal, i.thread, i.input, &asked, true, &homes, check,
                )
                .await
                .map(|_| ());
            (i.thread, result, before)
        }
        _ => unreachable!(),
    };
    let error = match result {
        Err(InputError::Internal(error)) => ApiError::engine(error),
        other => panic!("the original first-effect refusal must survive: {other:?}"),
    };
    assert_eq!(error, expected);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(snapshot(&s, thread).await, before);
}

#[tokio::test]
async fn plan_preserves_a_transport_refusal_before_native_effects() {
    rejected("plan").await;
}

#[tokio::test]
async fn run_preserves_a_transport_refusal_before_native_effects() {
    rejected("run").await;
}

#[tokio::test]
async fn bypass_preserves_a_transport_refusal_before_native_effects() {
    rejected("bypass").await;
}

#[tokio::test]
async fn accepted_plan_runs_the_transport_boundary_once_through_all_native_writes() {
    let s = Scene::new().await;
    let p = plan(&s, "First effect accepted plan").await;
    let mut tx = s.rig.db.begin().await.unwrap();
    let principal = principal(&s, &mut tx, s.ada.id()).await;
    let calls = AtomicUsize::new(0);
    let (submitted, _) = s
        .rig
        .engine
        .decide_plan_projected_guarded(
            tx,
            principal,
            p.thread,
            p.item,
            &ThreadPlanDecideBody {
                proceed: true,
                text: None,
            },
            true,
            || {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
        )
        .await
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(submitted.thread_id, p.thread);
    let state: String = sqlx::query_scalar("SELECT state FROM approvals WHERE id=$1")
        .bind(p.approval)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(state, "approved");
}

#[tokio::test]
async fn duplicate_inbox_rows_leave_the_first_effect_boundary_for_the_next_real_write() {
    use orch_db::queries::notifications::people::{NewNotification, announce, announce_with};
    let s = Scene::new().await;
    let mut tx = s.rig.db.begin().await.unwrap();
    let data = json!({});
    let row = NewNotification {
        user_id: s.ada.id(),
        kind: "milestone",
        node_id: None,
        thread_id: None,
        approval_id: None,
        chat_id: None,
        title: "First effect notification",
        body: "",
        data: &data,
        dedupe_key: "first-effect-existing",
        at: s.rig.world.now(),
    };
    assert!(announce(&mut tx, &row).await.unwrap().is_some());
    let before: i64 = sqlx::query_scalar("SELECT inbox_seq FROM users WHERE id=$1")
        .bind(s.ada.id())
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let calls = AtomicUsize::new(0);
    let check = || -> Result<(), orch_engine::EngineError> {
        calls.fetch_add(1, Ordering::SeqCst);
        Err(orch_engine::EngineError::BeforeEffect(Box::new(
            ApiError::new(
                ErrorCode::AuthInvalidToken,
                "no new notification may be accepted",
            ),
        )))
    };
    assert!(
        announce_with(&mut tx, &row, Some(check))
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let fresh = NewNotification {
        dedupe_key: "first-effect-new",
        ..row
    };
    assert!(announce_with(&mut tx, &fresh, Some(check)).await.is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let after: i64 = sqlx::query_scalar("SELECT inbox_seq FROM users WHERE id=$1")
        .bind(s.ada.id())
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(after, before);
    tx.rollback().await.unwrap();
}
