//! Exact existing lock boundaries, current deadlines and private-mode separation.

use super::*;
use orch_db::queries::access;
use std::time::Duration as WaitDuration;

/// Same ten-second/ten-millisecond lock-witness budget as the existing native
/// invitation_roles controls. A real pg_blocking_pids edge is required.
async fn blocked(world: &World, locker: i32) -> Result<(), tokio::time::error::Elapsed> {
    tokio::time::timeout(WaitDuration::from_secs(10), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND $1=ANY(pg_blocking_pids(pid)))",
            )
            .bind(locker)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
            if waiting {
                break;
            }
            tokio::time::sleep(WaitDuration::from_millis(10)).await;
        }
    })
    .await
}

async fn expires_after_wait(recipient_key: bool) {
    let s = Scene::new().await;
    let (offer, issuer) = if recipient_key {
        (browser_offer(&s, Destination::Node).await, None)
    } else {
        let issuer = legacy(s.world(), s.room.ada.id(), s.room.project).await;
        sqlx::query("UPDATE api_tokens SET expires_at=$2 WHERE id=$1")
            .bind(issuer.id)
            .bind(s.world().now() + chrono::Duration::seconds(1))
            .execute(&s.world().db.pool)
            .await
            .unwrap();
        let created = checked(
            create(s.world(), &issuer.secret, &s.body(Destination::Node)).await,
            StatusCode::OK,
        );
        (full_created(&created), Some(issuer))
    };
    let recipient = if recipient_key {
        let caller = key(
            s.world(),
            s.room.eve.id(),
            s.rules(Destination::Node, ACCEPT, "manage", false),
        )
        .await;
        sqlx::query("UPDATE api_tokens SET expires_at=$2 WHERE id=$1")
            .bind(caller.id)
            .bind(s.world().now() + chrono::Duration::seconds(1))
            .execute(&s.world().db.pool)
            .await
            .unwrap();
        caller.secret
    } else {
        s.room.eve.token.clone()
    };
    let mut held = s.world().db.pool.begin().await.unwrap();
    let locker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *held)
        .await
        .unwrap();
    sqlx::query("SELECT id FROM invitations WHERE id=$1 FOR UPDATE")
        .bind(offer.id)
        .execute(&mut *held)
        .await
        .unwrap();
    let app = s.world().app.clone();
    let token = offer.token.clone();
    let mut pending = tokio::spawn(async move {
        let request =
            app.call(InvitationAccept, &[])
                .token(&recipient)
                .body(&InvitationAcceptBody {
                    accepted_terms_hash: None,
                    token,
                });
        request.send_bytes().await
    });
    tokio::select! {
        finished = &mut pending => {
            held.rollback().await.unwrap();
            let reply = finished.unwrap();
            let data = if reply.raw.is_empty() { Json::Null } else { json_reply(&reply) };
            panic!("acceptance completed before the real invitation lock: status={}, code={}; expiry-after-wait premise was not reached", reply.status, data["code"]);
        }
        witness = blocked(s.world(), locker) => {
            if witness.is_err() {
                pending.abort();
                let _ = pending.await;
                held.rollback().await.unwrap();
                panic!("acceptance did not establish the exact held invitation lock edge");
            }
        }
    }
    // Cross only the one-second key deadline. The one-hour invitation remains
    // valid, and no user role, invitation row or other credential changes.
    s.world().advance(chrono::Duration::seconds(2));
    held.commit().await.unwrap();
    let reply = pending.await.unwrap();
    if recipient_key {
        checked(reply, StatusCode::UNAUTHORIZED);
    } else {
        let gone = checked(reply, StatusCode::GONE);
        assert_eq!(gone["reason"], "invitation_issuer_authority_lost");
        assert!(issuer.is_some());
    }
    assert_eq!(uses(s.world(), offer.id).await, 0);
    let grants: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM node_roles WHERE node_id=$1 AND subject_id=$2 AND removed_at IS NULL",
    )
    .bind(s.room.project)
    .bind(s.room.eve.id())
    .fetch_one(&s.world().db.pool)
    .await
    .unwrap();
    assert_eq!(grants, 0);
    assert_eq!(
        memberships(s.world(), s.room.root, s.room.eve.id()).await,
        0
    );
}

#[tokio::test]
async fn the_recipient_key_deadline_is_checked_after_the_actual_invitation_lock_wait() {
    expires_after_wait(true).await;
}

#[tokio::test]
async fn the_legacy_issuer_key_deadline_is_checked_after_the_actual_invitation_lock_wait() {
    expires_after_wait(false).await;
}

#[tokio::test]
async fn ordinary_system_administration_does_not_give_a_key_private_node_inviter_authority() {
    let s = Scene::new().await;
    // Give only the advanced outsider row. The browser's actual native mode
    // supplies its management reach; a key must not inherit that private reach.
    access::testing::add_entry(
        &mut *s.world().conn().await,
        &access::PageKey::new("node", s.room.project, "live"),
        "any",
        "access.external",
        ("user", Some(s.admin.id()), None),
        None,
        s.world().now(),
    )
    .await
    .unwrap();
    let native = checked(
        create(s.world(), &s.admin.token, &s.body(Destination::Node)).await,
        StatusCode::OK,
    );
    let native = full_created(&native);
    let scoped = key(
        s.world(),
        s.admin.id(),
        s.rules(Destination::Node, CREATE, "manage", true),
    )
    .await;
    let refused = checked(
        create(s.world(), &scoped.secret, &s.body(Destination::Node)).await,
        StatusCode::FORBIDDEN,
    );
    assert_eq!(refused["required"], "access.manage");
    let offers: i64 = sqlx::query_scalar("SELECT count(*) FROM invitations WHERE created_by=$1")
        .bind(s.admin.id())
        .fetch_one(&s.world().db.pool)
        .await
        .unwrap();
    assert_eq!(offers, 1, "only the actual browser-mode offer was accepted");
    assert_eq!(uses(s.world(), native.id).await, 0);
}
