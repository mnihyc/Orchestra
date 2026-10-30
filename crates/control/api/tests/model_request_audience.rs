//! The only current user of a private subscription is told about a held message, even outside all Owners rows.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
mod places;
mod rig;
mod support;
use orch_api_contract::threads::{Delivery, ThreadInputRun};
use orch_types::Id;
use rig::Scene;

#[tokio::test]
async fn a_subscription_kept_to_one_driver_tells_that_driver_and_runs_on_their_access_with_the_author_named()
 {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    // Eve drives the thread, but is neither an owner of it nor an org/system admin.
    // Its subscription's first owner is now Eve and the org keeps that subscription kind to its owner.
    let mut conn = s.rig.world.conn().await;
    let held = orch_db::queries::access::roles_in(&mut conn, s.tree.org.base.id, s.rig.world.now())
        .await
        .unwrap()
        .into_iter()
        .filter(|row| row.subject_kind == "user" && row.subject_id == s.cai.id())
        .collect::<Vec<_>>();
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].role, "viewer");
    orch_db::queries::access::set_role(&mut conn, held[0].id, "admin")
        .await
        .unwrap();
    assert_eq!(
        orch_db::queries::access::role_at(&mut conn, s.tree.org.base.id, s.cai.id())
            .await
            .unwrap()
            .as_deref(),
        Some("admin")
    );
    sqlx::query("UPDATE providers SET owner_id=$2 WHERE id=$1")
        .bind(s.on.provider)
        .bind(s.eve.id())
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE permission_entries SET holder_id=$2 WHERE object_kind='provider' AND object_id=$1 AND permission='access.own' AND removed_at IS NULL").bind(s.on.provider).bind(s.eve.id()).execute(&mut *conn).await.unwrap();
    sqlx::query(
        "INSERT INTO settings(node_id,key,value) VALUES($1,'providers.subscription_sharing','[]')",
    )
    .bind(s.tree.org.base.id)
    .execute(&mut *conn)
    .await
    .unwrap();
    let epoch = orch_authz::epoch::bump(&mut conn).await.unwrap();
    orch_db::sequences::notify_epoch(&mut conn, epoch)
        .await
        .unwrap();
    drop(conn);
    let input = s
        .say(
            &s.dan,
            thread,
            "Please run these words",
            Delivery::Queue,
            "held-private-subscription",
        )
        .await
        .ok()
        .input;
    assert_eq!(input.state_detail.as_deref(), Some("model_use_required"));
    let approval = input.approval_id.unwrap();
    let notified:Vec<Id>=sqlx::query_scalar("SELECT user_id FROM notifications WHERE approval_id=$1 AND kind='approval.requested' ORDER BY user_id").bind(approval).fetch_all(s.rig.db.pool()).await.unwrap();
    assert_eq!(
        notified,
        [s.eve.id()],
        "the owner and admins cannot use that subscription and receive no attention item"
    );
    let incapable:Vec<Id>=sqlx::query_scalar("SELECT user_id FROM notifications WHERE thread_id=$1 AND user_id=ANY($2) AND kind IN ('approval.requested','thread.blocked')").bind(thread).bind(vec![s.ada.id(),s.cai.id()]).fetch_all(s.rig.db.pool()).await.unwrap();
    assert!(
        incapable.is_empty(),
        "an incapable owner/admin gets no request or attention notice"
    );
    let ran = s
        .rig
        .world
        .app
        .call(ThreadInputRun, &[&thread, &input.id])
        .token(&s.eve.token)
        .send()
        .await
        .ok();
    assert_eq!(ran.author.as_ref().unwrap().id, s.dan.id());
    assert_eq!(ran.run_as.as_ref().unwrap().id, s.eve.id());
    let start = s.rig.turn_start(thread, 0).await;
    let responsibility: Option<Id> =
        sqlx::query_scalar("SELECT audit_for_user_id FROM turns WHERE id=$1")
            .bind(start.turn_id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(responsibility, Some(s.eve.id()));
    let row =
        orch_db::queries::approvals::approval(&mut s.rig.db.acquire().await.unwrap(), approval)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(row.state, "approved");
    assert_eq!(row.decided_by, Some(s.eve.id()));
}
