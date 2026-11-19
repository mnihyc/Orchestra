//! Original native attachment association survives the completed reply's actual activity wait.

use super::*;
use orch_api_contract::access::NewResourceAccess;
use orch_api_contract::mounts::{MountCreate, MountCreateBody};
use orch_api_contract::trust::{ConfigTrustAnswer, ConfigTrustBody, EnvironmentConfigTrustSet};

#[tokio::test]
async fn trust_publication_keeps_the_original_native_mount_association_even_while_both_ids_stay_readable()
 {
    for unlink in [false, true] {
        let s = Scene::new().await;
        let mount = s
            .world
            .app
            .call(MountCreate, &[])
            .token(&s.team.c.admin.token)
            .body(&MountCreateBody {
                name: "original-trust-attachment-marker".to_owned(),
                description: None,
                home_id: Some(s.team.c.acme),
                target: "/mnt/fixture-data".to_owned(),
                read_only: Some(true),
                default_fate: None,
                access: Some(NewResourceAccess::OnlyMe),
            })
            .send()
            .await
            .ok();
        sqlx::query(
            "UPDATE environments SET config=jsonb_set(config,'{env.mounts}',$2,true) WHERE id=$1",
        )
        .bind(s.first.base.id)
        .bind(json!({(mount.base.id.to_string()):true}))
        .execute(&s.world.db.pool)
        .await
        .unwrap();
        let native = s
            .world
            .app
            .call(EnvironmentGet, &[&s.first.base.id])
            .token(&s.team.c.admin.token)
            .send()
            .await
            .ok();
        assert_eq!(
            native.mounts.len(),
            1,
            "the actual native response has the attachment"
        );
        assert_eq!(native.mounts[0].mount.id, mount.base.id);
        assert_eq!(
            native.mounts[0].mount.name,
            "original-trust-attachment-marker"
        );
        orch_db::queries::trust::ask(
            &mut *s.world.conn().await,
            s.first.base.id,
            "fixture-harness",
            "original-association-files-v1",
            &["fixture-config/settings.json".to_owned()],
            s.world.now(),
        )
        .await
        .unwrap();
        let writer = key(
            &s.world,
            s.team.c.admin.id(),
            vec![
                rule(
                    "EnvironmentConfigTrustSet",
                    "environment",
                    s.first.base.id,
                    "self",
                    &["environment"],
                    &["manage", "read"],
                ),
                s.sources("EnvironmentConfigTrustSet"),
            ],
        )
        .await;
        let before_scope: Json =
            sqlx::query_scalar("SELECT operation_scope FROM api_tokens WHERE id=$1")
                .bind(writer.id)
                .fetch_one(&s.world.db.pool)
                .await
                .unwrap();
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
            let call = app.call(EnvironmentConfigTrustSet, &[&id, &"fixture-harness"]);
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
                assert!(!request.is_finished(), "the completed native body must reach activity append");
                let blocked:bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                    .bind(pid).fetch_one(&s.world.db.pool).await.unwrap();
                if blocked { break; }
                tokio::task::yield_now().await;
            }
        }).await.expect("the actual final activity INSERT holds the already committed answer");
        let answered = orch_db::queries::trust::of_harness(
            &mut *s.world.conn().await,
            s.first.base.id,
            "fixture-harness",
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(answered.answer.as_deref(), Some("trust"));
        if unlink {
            sqlx::query("UPDATE environments SET config=jsonb_set(config,'{env.mounts}',$2,true) WHERE id=$1")
                .bind(s.first.base.id).bind(json!({})).execute(&s.world.db.pool).await.unwrap();
            let current = s
                .world
                .app
                .call(EnvironmentGet, &[&s.first.base.id])
                .token(&s.team.c.admin.token)
                .send()
                .await
                .ok();
            assert!(
                current.mounts.is_empty(),
                "the current native factory really lost the association"
            );
            let present: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM mounts WHERE id=$1 AND deleted_at IS NULL)",
            )
            .bind(mount.base.id)
            .fetch_one(&s.world.db.pool)
            .await
            .unwrap();
            assert!(present, "the original Mount itself remains live");
        }
        let current_scope: Json =
            sqlx::query_scalar("SELECT operation_scope FROM api_tokens WHERE id=$1")
                .bind(writer.id)
                .fetch_one(&s.world.db.pool)
                .await
                .unwrap();
        assert_eq!(
            current_scope, before_scope,
            "both resource scopes remain unchanged"
        );
        let xmin: String = sqlx::query_scalar("SELECT xmin::text FROM environments WHERE id=$1")
            .bind(s.first.base.id)
            .fetch_one(&s.world.db.pool)
            .await
            .unwrap();
        let audits: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM audit_log WHERE environment_id=$1 AND action='approval.decided'",
        )
        .bind(s.first.base.id)
        .fetch_one(&s.world.db.pool)
        .await
        .unwrap();
        assert!(audits > 0);
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(wait, request).await.unwrap().unwrap();
        assert_eq!(
            reply.status,
            StatusCode::OK,
            "the accepted answer remains successful"
        );
        let body: Json = serde_json::from_slice(&reply.raw).unwrap();
        if unlink {
            assert_eq!(
                body,
                json!({"projection":"receipt", "environment_id":s.first.base.id,
                "harness":"fixture-harness", "answer":"trust"})
            );
            assert!(
                !String::from_utf8_lossy(&reply.raw).contains("original-trust-attachment-marker")
            );
        } else {
            assert_eq!(body["mounts"][0]["mount"]["id"], json!(mount.base.id));
            assert_eq!(
                body["mounts"][0]["mount"]["name"],
                "original-trust-attachment-marker"
            );
        }
        let after: String = sqlx::query_scalar("SELECT xmin::text FROM environments WHERE id=$1")
            .bind(s.first.base.id)
            .fetch_one(&s.world.db.pool)
            .await
            .unwrap();
        let after_audits: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM audit_log WHERE environment_id=$1 AND action='approval.decided'",
        )
        .bind(s.first.base.id)
        .fetch_one(&s.world.db.pool)
        .await
        .unwrap();
        assert_eq!(
            after, xmin,
            "publication cannot replay the completed writer"
        );
        assert_eq!(after_audits, audits);
    }
}
