//! Native trust answers, independent response Read, and real database wait boundaries.

use super::*;
use orch_api_contract::trust::{
    ConfigTrustAnswer, ConfigTrustBody, ConfigTrustResponse, EnvironmentConfigTrustSet,
};

const HARNESS: &str = "fixture-harness";
const PRIVATE_PATH: &str = "fixture-config/private-settings.json";

async fn ask(s: &Scene, files: &str) {
    assert!(
        orch_db::queries::trust::ask(
            &mut *s.world.conn().await,
            s.first.base.id,
            HARNESS,
            files,
            &[PRIVATE_PATH.to_owned()],
            s.world.now(),
        )
        .await
        .unwrap()
    );
}

fn grant(s: &Scene, actions: &[&str]) -> Json {
    rule(
        "EnvironmentConfigTrustSet",
        "environment",
        s.first.base.id,
        "self",
        &["environment"],
        actions,
    )
}

async fn answer(s: &Scene, secret: &str, answer: ConfigTrustAnswer) -> RawReply {
    s.world
        .app
        .call(EnvironmentConfigTrustSet, &[&s.first.base.id, &HARNESS])
        .token(secret)
        .body(&ConfigTrustBody { answer })
        .send_bytes()
        .await
}

async fn stored(s: &Scene) -> orch_db::queries::trust::Trust {
    orch_db::queries::trust::of_harness(&mut *s.world.conn().await, s.first.base.id, HARNESS)
        .await
        .unwrap()
        .unwrap()
}

fn receipt(s: &Scene, answer: &str) -> Json {
    json!({"projection":"receipt", "environment_id":s.first.base.id,
        "harness":HARNESS, "answer":answer})
}

#[tokio::test]
async fn scoped_trust_keeps_its_exact_manage_scope_native_person_gate_and_safe_receipt() {
    let s = Scene::new().await;
    ask(&s, "private-question-files-marker").await;
    for rules in [
        vec![rule(
            "EnvironmentGet",
            "environment",
            s.first.base.id,
            "self",
            &["environment"],
            &["read"],
        )],
        vec![grant(&s, &["read"])],
        vec![rule(
            "EnvironmentConfigTrustSet",
            "environment",
            s.second.base.id,
            "self",
            &["environment"],
            &["manage"],
        )],
        vec![rule(
            "EnvironmentConfigTrustSet",
            "topic",
            s.team.topic,
            "self",
            &["environment"],
            &["manage"],
        )],
    ] {
        let key = key(&s.world, s.team.c.admin.id(), rules).await;
        let reply = answer(&s, &key.secret, ConfigTrustAnswer::Trust).await;
        assert_eq!(reply.status, StatusCode::FORBIDDEN);
        assert!(!String::from_utf8_lossy(&reply.raw).contains(PRIVATE_PATH));
        assert_eq!(
            stored(&s).await.answer,
            None,
            "a refused operation makes no answer"
        );
    }
    let stranger = key(
        &s.world,
        s.team.c.stranger.id(),
        vec![grant(&s, &["manage"])],
    )
    .await;
    let refused = answer(&s, &stranger.secret, ConfigTrustAnswer::Trust).await;
    assert_eq!(
        refused.status,
        StatusCode::FORBIDDEN,
        "scope cannot lend native Manage"
    );
    assert!(!String::from_utf8_lossy(&refused.raw).contains(&s.first.base.name));
    assert_eq!(stored(&s).await.answer, None);

    let service = new_id();
    identity::create_service_account(
        &mut *s.world.conn().await,
        service,
        s.team.c.acme,
        "trust-service-fixture",
        "Trust service fixture",
        s.team.c.admin.id(),
        s.world.now(),
    )
    .await
    .unwrap();
    resources::name_person(
        &s.world,
        "environment",
        s.first.base.id,
        "environment.manage",
        service,
    )
    .await;
    let service = key(&s.world, service, vec![grant(&s, &["manage"])]).await;
    let refused = answer(&s, &service.secret, ConfigTrustAnswer::Trust).await;
    assert_eq!(
        refused.status,
        StatusCode::FORBIDDEN,
        "a service with a native Manage entry and exact scope still cannot answer a person's question"
    );
    assert_eq!(stored(&s).await.answer, None);

    let writer = key(
        &s.world,
        s.team.c.admin.id(),
        vec![
            grant(&s, &["manage"]),
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
    let reply = answer(&s, &writer.secret, ConfigTrustAnswer::KeepOut).await;
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "exact Manage admits the native act"
    );
    assert_eq!(
        serde_json::from_slice::<Json>(&reply.raw).unwrap(),
        receipt(&s, "keep_out"),
        "another operation's Read cannot disclose the stored question or Environment"
    );
    let kept = stored(&s).await;
    assert_eq!(kept.answer.as_deref(), Some("keep_out"));
    assert_eq!(kept.by, Some(s.team.c.admin.id()));
    assert_eq!(kept.files, "private-question-files-marker");
    assert_eq!(kept.found, vec![PRIVATE_PATH.to_owned()]);
}

#[tokio::test]
async fn scoped_trust_full_readback_uses_its_own_read_and_the_unchanged_native_environment() {
    let s = Scene::new().await;
    ask(&s, "full-question-files-marker").await;
    let reader = key(
        &s.world,
        s.team.c.admin.id(),
        vec![
            grant(&s, &["manage", "read"]),
            s.sources("EnvironmentConfigTrustSet"),
        ],
    )
    .await;
    let reply = answer(&s, &reader.secret, ConfigTrustAnswer::Trust).await;
    assert_eq!(reply.status, StatusCode::OK);
    let ConfigTrustResponse::Full(shown) = serde_json::from_slice(&reply.raw).unwrap() else {
        panic!("own-operation Read must retain the ordinary native projection");
    };
    let ordinary = s
        .world
        .app
        .call(EnvironmentGet, &[&s.first.base.id])
        .token(&s.team.c.admin.token)
        .send()
        .await
        .ok();
    assert_eq!(*shown, ordinary);
    assert!(
        shown
            .config_trust
            .iter()
            .any(|question| question.harness == HARNESS
                && question.answer == Some(ConfigTrustAnswer::Trust)
                && question.path == PRIVATE_PATH)
    );

    let no_foreign = key(
        &s.world,
        s.team.c.admin.id(),
        vec![grant(&s, &["manage", "read"])],
    )
    .await;
    let reply = answer(&s, &no_foreign.secret, ConfigTrustAnswer::KeepOut).await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(
        serde_json::from_slice::<Json>(&reply.raw).unwrap(),
        receipt(&s, "keep_out"),
        "Template and Host content need response scope without blocking the act"
    );
    assert_eq!(stored(&s).await.answer.as_deref(), Some("keep_out"));
}

#[tokio::test]
async fn scoped_trust_refreshes_the_actual_key_after_the_native_environment_lock_wait() {
    for expire in [false, true] {
        let s = Scene::new().await;
        ask(&s, "before-lock-control").await;
        let writer = key(&s.world, s.team.c.admin.id(), vec![grant(&s, &["manage"])]).await;
        assert_eq!(
            answer(&s, &writer.secret, ConfigTrustAnswer::Trust)
                .await
                .status,
            StatusCode::OK,
            "the exact key is accepted before the wait premise"
        );
        ask(&s, "waiting-question-files-marker").await;
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
            let call = app.call(EnvironmentConfigTrustSet, &[&id, &HARNESS]);
            call.token(&secret)
                .body(&ConfigTrustBody {
                    answer: ConfigTrustAnswer::KeepOut,
                })
                .send_bytes()
                .await
        });
        let wait = std::time::Duration::from_secs(30);
        tokio::time::timeout(wait, async {
            loop {
                assert!(!request.is_finished(), "the operation must reach its native row lock");
                let blocked:bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%FROM environments WHERE id = $1 FOR UPDATE%')")
                    .bind(pid).fetch_one(&s.world.db.pool).await.unwrap();
                if blocked { break; }
                tokio::task::yield_now().await;
            }
        }).await.expect("the actual native Environment lock wait");
        if expire {
            s.world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(wait, request).await.unwrap().unwrap();
        if expire {
            assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
            assert_eq!(
                stored(&s).await.answer,
                None,
                "no answer follows an expired checkpoint"
            );
            assert!(!String::from_utf8_lossy(&reply.raw).contains(PRIVATE_PATH));
        } else {
            assert_eq!(reply.status, StatusCode::OK);
            assert_eq!(
                serde_json::from_slice::<Json>(&reply.raw).unwrap(),
                receipt(&s, "keep_out")
            );
            assert_eq!(stored(&s).await.answer.as_deref(), Some("keep_out"));
        }
    }
}

#[tokio::test]
async fn completed_trust_returns_its_receipt_after_publication_expiry_without_replaying_the_writer()
{
    for expire in [false, true] {
        let s = Scene::new().await;
        ask(&s, "publication-question-files-marker").await;
        let writer = key(
            &s.world,
            s.team.c.admin.id(),
            vec![
                grant(&s, &["manage", "read"]),
                s.sources("EnvironmentConfigTrustSet"),
            ],
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
            let call = app.call(EnvironmentConfigTrustSet, &[&id, &HARNESS]);
            call.token(&secret)
                .body(&ConfigTrustBody {
                    answer: ConfigTrustAnswer::Trust,
                })
                .send_bytes()
                .await
        });
        let wait = std::time::Duration::from_secs(30);
        tokio::time::timeout(wait, async {
            loop {
                assert!(!request.is_finished(), "the completed answer must reach final publication");
                let blocked:bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                    .bind(pid).fetch_one(&s.world.db.pool).await.unwrap();
                if blocked { break; }
                tokio::task::yield_now().await;
            }
        }).await.expect("the actual activity append after the answer commits");
        assert_eq!(
            stored(&s).await.answer.as_deref(),
            Some("trust"),
            "the native effect really committed"
        );
        let xmin: String = sqlx::query_scalar("SELECT xmin::text FROM environments WHERE id=$1")
            .bind(s.first.base.id)
            .fetch_one(&s.world.db.pool)
            .await
            .unwrap();
        let audit: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM audit_log WHERE environment_id=$1 AND action='approval.decided'",
        )
        .bind(s.first.base.id)
        .fetch_one(&s.world.db.pool)
        .await
        .unwrap();
        assert!(audit > 0, "the native writer records the accepted answer");
        if expire {
            s.world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(wait, request).await.unwrap().unwrap();
        assert_eq!(
            reply.status,
            StatusCode::OK,
            "the committed answer remains successful"
        );
        if expire {
            assert_eq!(
                serde_json::from_slice::<Json>(&reply.raw).unwrap(),
                receipt(&s, "trust")
            );
            assert!(!String::from_utf8_lossy(&reply.raw).contains(PRIVATE_PATH));
        } else {
            assert!(
                matches!(
                    serde_json::from_slice::<ConfigTrustResponse>(&reply.raw).unwrap(),
                    ConfigTrustResponse::Full(_)
                ),
                "unchanged current authority preserves the native rich response"
            );
        }
        let after: String = sqlx::query_scalar("SELECT xmin::text FROM environments WHERE id=$1")
            .bind(s.first.base.id)
            .fetch_one(&s.world.db.pool)
            .await
            .unwrap();
        let audits_after: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM audit_log WHERE environment_id=$1 AND action='approval.decided'",
        )
        .bind(s.first.base.id)
        .fetch_one(&s.world.db.pool)
        .await
        .unwrap();
        assert_eq!(after, xmin, "publication cannot run the writer again");
        assert_eq!(audits_after, audit);
    }
}
