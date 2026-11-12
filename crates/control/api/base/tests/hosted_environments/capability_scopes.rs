//! Native gateway revocation scopes, genuine no-job acknowledgements and accepted-effect cuts.

use super::*;
use orch_api_contract::environment::EnvironmentCapabilityRevoke;

const CAPABILITY: &str = "port_place";

async fn grant(s: &Scene) {
    sqlx::query("UPDATE environments SET granted_capabilities=ARRAY[$2::text] WHERE id=$1")
        .bind(s.first.base.id)
        .bind(CAPABILITY)
        .execute(&s.world.db.pool)
        .await
        .unwrap();
}

async fn grants(s: &Scene) -> Vec<String> {
    sqlx::query_scalar("SELECT granted_capabilities FROM environments WHERE id=$1")
        .bind(s.first.base.id)
        .fetch_one(&s.world.db.pool)
        .await
        .unwrap()
}

fn scope(s: &Scene, actions: &[&str]) -> Json {
    rule(
        "EnvironmentCapabilityRevoke",
        "environment",
        s.first.base.id,
        "self",
        &["environment"],
        actions,
    )
}

async fn revoke(s: &Scene, secret: &str) -> RawReply {
    s.world
        .app
        .call(
            EnvironmentCapabilityRevoke,
            &[&s.first.base.id, &CAPABILITY],
        )
        .token(secret)
        .send_bytes()
        .await
}

fn receipt(s: &Scene) -> Json {
    json!({"projection":"receipt", "environment_id":s.first.base.id,
        "key":CAPABILITY, "job_id":null})
}

async fn legacy_ports_key(s: &Scene) -> String {
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token(
        &mut *s.world.conn().await,
        &identity::NewApiToken {
            id: new_id(),
            user_id: s.team.c.admin.id(),
            name: "Native ports-only fixture",
            token_hash: &token_hash(&secret),
            scopes: &["ports.manage".to_owned()],
            node_id: None,
            at: s.world.now(),
            expires_at: Some(s.world.now() + chrono::Duration::hours(1)),
        },
    )
    .await
    .unwrap();
    secret
}

#[tokio::test]
async fn capability_revocation_uses_exact_manage_scope_keeps_native_ports_authority_and_distinguishes_receipts()
 {
    let s = Scene::new().await;
    grant(&s).await;
    for rules in [
        vec![scope(&s, &["read"])],
        vec![rule(
            "EnvironmentConfigTrustSet",
            "environment",
            s.first.base.id,
            "self",
            &["environment"],
            &["manage"],
        )],
        vec![rule(
            "EnvironmentCapabilityRevoke",
            "environment",
            s.second.base.id,
            "self",
            &["environment"],
            &["manage"],
        )],
        vec![rule(
            "EnvironmentCapabilityRevoke",
            "topic",
            s.team.topic,
            "self",
            &["environment"],
            &["manage"],
        )],
    ] {
        let denied = key(&s.world, s.team.c.admin.id(), rules).await;
        assert_eq!(
            revoke(&s, &denied.secret).await.status,
            StatusCode::FORBIDDEN
        );
        assert_eq!(grants(&s).await, vec![CAPABILITY]);
    }
    let viewer = key(&s.world, s.team.c.viewer.id(), vec![scope(&s, &["manage"])]).await;
    assert_eq!(
        revoke(&s, &viewer.secret).await.status,
        StatusCode::FORBIDDEN,
        "scope cannot lend the native PortsManage gate"
    );
    assert_eq!(grants(&s).await, vec![CAPABILITY]);

    let writer = key(
        &s.world,
        s.team.c.admin.id(),
        vec![
            scope(&s, &["manage"]),
            rule(
                "EnvironmentGet",
                "environment",
                s.first.base.id,
                "self",
                &["environment"],
                &["read"],
            ),
        ],
    )
    .await;
    let reply = revoke(&s, &writer.secret).await;
    assert_eq!(reply.status, StatusCode::ACCEPTED);
    assert_eq!(
        serde_json::from_slice::<Json>(&reply.raw).unwrap(),
        receipt(&s),
        "another operation cannot open the native job acknowledgement"
    );
    assert!(grants(&s).await.is_empty());
    let again = revoke(&s, &writer.secret).await;
    assert_eq!(
        again.status,
        StatusCode::NOT_FOUND,
        "taking back an absent grant keeps its native error"
    );

    grant(&s).await;
    let full = key(
        &s.world,
        s.team.c.admin.id(),
        vec![scope(&s, &["manage", "read"])],
    )
    .await;
    let reply = revoke(&s, &full.secret).await;
    assert_eq!(reply.status, StatusCode::ACCEPTED);
    assert_eq!(
        serde_json::from_slice::<Json>(&reply.raw).unwrap(),
        Json::Null,
        "same-operation Read preserves the genuine native gateway no-job result"
    );
    let legacy = legacy_ports_key(&s).await;
    for secret in [&s.team.c.admin.token, &legacy] {
        grant(&s).await;
        let reply = revoke(&s, secret).await;
        assert_eq!(reply.status, StatusCode::ACCEPTED);
        assert_eq!(
            serde_json::from_slice::<Json>(&reply.raw).unwrap(),
            Json::Null,
            "ordinary and ports-only legacy authority keep the original native wire result"
        );
        assert!(grants(&s).await.is_empty());
    }
}

#[tokio::test]
async fn capability_revocation_waits_for_the_actual_native_environment_then_refreshes_key_lifetime()
{
    for expire in [false, true] {
        let s = Scene::new().await;
        grant(&s).await;
        let writer = key(&s.world, s.team.c.admin.id(), vec![scope(&s, &["manage"])]).await;
        let control = revoke(&s, &writer.secret).await;
        assert_eq!(control.status, StatusCode::ACCEPTED);
        assert_eq!(
            serde_json::from_slice::<Json>(&control.raw).unwrap(),
            receipt(&s)
        );
        grant(&s).await;
        let mut blocker = s.world.db.pool.begin().await.unwrap();
        sqlx::query("SELECT id FROM environments WHERE id=$1 FOR UPDATE")
            .bind(s.first.base.id)
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = s.world.app.clone();
        let id = s.first.base.id;
        let secret = writer.secret.clone();
        let request = tokio::spawn(async move {
            let call = app.call(EnvironmentCapabilityRevoke, &[&id, &CAPABILITY]);
            call.token(&secret).send_bytes().await
        });
        let wait = std::time::Duration::from_secs(30);
        tokio::time::timeout(wait, async {
            loop {
                assert!(!request.is_finished(), "the revocation must reach its native lock");
                let blocked:bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%FROM environments WHERE id = $1 FOR UPDATE%')")
                    .bind(pid).fetch_one(&s.world.db.pool).await.unwrap();
                if blocked { break; }
                tokio::task::yield_now().await;
            }
        }).await.expect("the actual native Environment serialization wait");
        if expire {
            s.world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(wait, request).await.unwrap().unwrap();
        if expire {
            assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
            assert_eq!(
                grants(&s).await,
                vec![CAPABILITY],
                "an expired key takes nothing back"
            );
        } else {
            assert_eq!(reply.status, StatusCode::ACCEPTED);
            assert_eq!(
                serde_json::from_slice::<Json>(&reply.raw).unwrap(),
                receipt(&s)
            );
            assert!(grants(&s).await.is_empty());
        }
    }
}

#[tokio::test]
async fn completed_capability_revocation_keeps_its_receipt_after_activity_wait_expiry_without_replaying()
 {
    for expire in [false, true] {
        let s = Scene::new().await;
        grant(&s).await;
        let writer = key(
            &s.world,
            s.team.c.admin.id(),
            vec![scope(&s, &["manage", "read"])],
        )
        .await;
        let mut blocker = s.world.db.pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = s.world.app.clone();
        let id = s.first.base.id;
        let secret = writer.secret.clone();
        let request = tokio::spawn(async move {
            let call = app.call(EnvironmentCapabilityRevoke, &[&id, &CAPABILITY]);
            call.token(&secret).send_bytes().await
        });
        let wait = std::time::Duration::from_secs(30);
        tokio::time::timeout(wait, async {
            loop {
                assert!(!request.is_finished(), "the completed native act must reach activity append");
                let blocked:bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                    .bind(pid).fetch_one(&s.world.db.pool).await.unwrap();
                if blocked { break; }
                tokio::task::yield_now().await;
            }
        }).await.expect("the actual postcommit activity INSERT wait");
        assert!(
            grants(&s).await.is_empty(),
            "the native removal is already committed"
        );
        let xmin: String = sqlx::query_scalar("SELECT xmin::text FROM environments WHERE id=$1")
            .bind(s.first.base.id)
            .fetch_one(&s.world.db.pool)
            .await
            .unwrap();
        let audits:i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE environment_id=$1 AND action='capability.revoked'")
            .bind(s.first.base.id).fetch_one(&s.world.db.pool).await.unwrap();
        assert!(audits > 0);
        if expire {
            s.world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(wait, request).await.unwrap().unwrap();
        assert_eq!(
            reply.status,
            StatusCode::ACCEPTED,
            "publication cannot undo the accepted revocation"
        );
        let body: Json = serde_json::from_slice(&reply.raw).unwrap();
        assert_eq!(body, if expire { receipt(&s) } else { Json::Null });
        let after: String = sqlx::query_scalar("SELECT xmin::text FROM environments WHERE id=$1")
            .bind(s.first.base.id)
            .fetch_one(&s.world.db.pool)
            .await
            .unwrap();
        let after_audits:i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE environment_id=$1 AND action='capability.revoked'")
            .bind(s.first.base.id).fetch_one(&s.world.db.pool).await.unwrap();
        assert_eq!(after, xmin);
        assert_eq!(
            after_audits, audits,
            "the writer and accepted cleanup run once"
        );
    }
}
