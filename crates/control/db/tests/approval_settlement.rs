//! One winning decision settles every recipient copy and publishes both readers' changed Inbox sequences.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_db::queries::{approvals, identity, notifications::people};
use orch_testkit::{clock, database::TestDb};
use orch_types::new_id;
use serde_json::json;

#[tokio::test]
async fn one_decision_removes_every_kinds_waiting_and_unread_copies_without_another_notice() {
    let db = TestDb::fresh().await;
    let mut conn = db.pool.acquire().await.unwrap();
    let ada = new_id();
    let bo = new_id();
    identity::testing::insert_user(&mut conn, ada, "ada")
        .await
        .unwrap();
    identity::testing::insert_user(&mut conn, bo, "bob")
        .await
        .unwrap();
    let asked = clock::start();
    let at = asked + chrono::Duration::minutes(1);
    for kind in ["harness", "capability", "port", "access", "plan", "input"] {
        for state in ["approved", "denied"] {
            let id = new_id();
            sqlx::query("INSERT INTO approvals(id,kind,title,request,required_permission,created_at,updated_at) VALUES($1,$2,'Review request','{}','',$3,$3)").bind(id).bind(kind).bind(asked).execute(&mut *conn).await.unwrap();
            let notification_kind = match kind {
                "access" => "access.requested",
                "capability" => "capability.requested",
                _ => "approval.requested",
            };
            for user in [ada, bo] {
                people::announce(
                    &mut conn,
                    &people::NewNotification {
                        user_id: user,
                        kind: notification_kind,
                        node_id: None,
                        thread_id: None,
                        approval_id: Some(id),
                        chat_id: None,
                        title: "Review request",
                        body: "",
                        data: &json!({"approval_id":id}),
                        dedupe_key: &format!("{id}:{user}"),
                        at: asked,
                    },
                )
                .await
                .unwrap();
            }
            // An already-read request still needs the changed outcome in its live history.
            sqlx::query("UPDATE notifications SET read_at=$3 WHERE approval_id=$1 AND user_id=$2")
                .bind(id)
                .bind(bo)
                .bind(asked)
                .execute(&mut *conn)
                .await
                .unwrap();
            let before: Vec<(orch_types::Id, i64)> =
                sqlx::query_as("SELECT id,inbox_seq FROM users WHERE id=ANY($1) ORDER BY id")
                    .bind(vec![ada, bo])
                    .fetch_all(&mut *conn)
                    .await
                    .unwrap();
            let decision = json!({"approve":state=="approved","expires_at":at+chrono::Duration::days(7),"note":"Reviewed together"});
            assert!(
                approvals::decide(&mut conn, id, state, Some(bo), &decision, at)
                    .await
                    .unwrap()
            );
            let row = approvals::approval(&mut conn, id).await.unwrap().unwrap();
            assert_eq!(
                (row.state.as_str(), row.decided_by, row.decided_at),
                (state, Some(bo), Some(at))
            );
            assert_eq!(row.decision, Some(decision));
            let copies:Vec<(orch_types::Id,bool,bool)>=sqlx::query_as("SELECT user_id,read_at IS NOT NULL,data->>'_orch_approval_settled'='true' FROM notifications WHERE approval_id=$1 ORDER BY user_id").bind(id).fetch_all(&mut *conn).await.unwrap();
            assert_eq!(copies.len(), 2, "{kind}: settlement creates no notice");
            assert!(
                copies.iter().all(|(_, read, settled)| *read && *settled),
                "{kind}: every copy is settled and read"
            );
            let pending: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM approvals WHERE id=$1 AND state='pending'",
            )
            .bind(id)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
            assert_eq!(pending, 0);
            let after: Vec<(orch_types::Id, i64)> =
                sqlx::query_as("SELECT id,inbox_seq FROM users WHERE id=ANY($1) ORDER BY id")
                    .bind(vec![ada, bo])
                    .fetch_all(&mut *conn)
                    .await
                    .unwrap();
            assert!(
                after
                    .iter()
                    .zip(before)
                    .all(|((id, seq), (old_id, old_seq))| *id == old_id && *seq == old_seq + 1),
                "{kind}: both live readers have a new sequence"
            );
            assert!(
                !approvals::decide(
                    &mut conn,
                    id,
                    "denied",
                    Some(ada),
                    &json!({"note":"Too late"}),
                    at
                )
                .await
                .unwrap()
            );
            let unchanged: Vec<(orch_types::Id, i64)> =
                sqlx::query_as("SELECT id,inbox_seq FROM users WHERE id=ANY($1) ORDER BY id")
                    .bind(vec![ada, bo])
                    .fetch_all(&mut *conn)
                    .await
                    .unwrap();
            assert_eq!(unchanged, after, "a losing decision has no effect");
        }
    }
}
