//! Real reviewed loss effects retain distinct preview and confirmation authority.

use super::*;

const DECLARE_OP: &str = "LossDeclare";

fn effects(s: &Scene) -> Vec<Json> {
    vec![
        rule(DECLARE_OP, "host", s.host, "manage"),
        json!({"target":{"kind":"host","id":s.host,"reach":"self"},"resources":["machine"],"allow":[{"operation":DECLARE_OP,"actions":["manage"]}]}),
        rule(DECLARE_OP, "environment", s.own, "manage"),
        rule(DECLARE_OP, "environment", s.other, "manage"),
        rule(DECLARE_OP, "thread", s.thread, "manage"),
    ]
}
fn reads(s: &Scene, names: bool) -> Vec<Json> {
    let mut rules = vec![
        rule(DECLARE_OP, "host", s.host, "read"),
        rule(DECLARE_OP, "environment", s.own, "read"),
        rule(DECLARE_OP, "environment", s.other, "read"),
    ];
    if names {
        rules.push(rule(DECLARE_OP, "thread", s.thread, "read"));
    }
    rules
}
fn complete(s: &Scene, names: bool) -> Vec<Json> {
    let mut rules = effects(s);
    rules.extend(reads(s, names));
    rules
}
async fn preview(s: &Scene) -> LossPlan {
    full(
        &s.call(&s.people.viewer.token, LossTarget::Host { id: s.host })
            .await,
    )
}
fn body(plan: &LossPlan) -> LossDeclareBody {
    LossDeclareBody {
        preflight_id: plan.preflight_id,
        confirm: plan.name.clone(),
        notify: false,
    }
}
async fn declare(s: &Scene, token: &str, body: LossDeclareBody) -> RawReply {
    let call = s.world.app.call(LossDeclare, &[]);
    call.token(token).body(&body).send_bytes().await
}
fn start(s: &Scene, token: String, body: LossDeclareBody) -> JoinHandle<RawReply> {
    let app = s.world.app.clone();
    tokio::spawn(async move {
        let call = app.call(LossDeclare, &[]);
        call.token(&token).body(&body).send_bytes().await
    })
}
fn declared_full(reply: &RawReply) -> LossPlan {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice::<LossDeclareResponse>(&reply.raw)
        .unwrap()
        .into_full()
        .unwrap()
}
fn declared_receipt(reply: &RawReply) -> LossDeclareReceipt {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice::<LossDeclareResponse>(&reply.raw)
        .unwrap()
        .into_full()
        .unwrap_err()
}
async fn lost(s: &Scene) -> bool {
    sqlx::query_scalar("SELECT m.lost_at IS NOT NULL FROM hosts h JOIN machines m ON m.id=h.machine_id WHERE h.id=$1")
        .bind(s.host).fetch_one(&s.world.db.pool).await.unwrap()
}
async fn unchanged(s: &Scene, plan: &LossPlan) {
    assert!(!lost(s).await);
    assert!(
        forks::preflight(&mut *s.world.conn().await, plan.preflight_id)
            .await
            .unwrap()
            .is_some()
    );
    let accepted: i64 =
        sqlx::query_scalar("SELECT count(*) FROM notification_operations WHERE id=$1")
            .bind(plan.preflight_id)
            .fetch_one(&s.world.db.pool)
            .await
            .unwrap();
    assert_eq!(accepted, 0);
}

#[tokio::test]
async fn loss_confirmation_requires_every_real_effect_and_keeps_a_manage_only_receipt() {
    let s = Scene::new().await;
    let plan = preview(&s).await;
    let mut omissions = vec![
        reads(&s, true),
        vec![rule(DECLARE_OP, "host", plan.preflight_id, "manage")],
        s.manage(),
    ];
    for missing in 0..effects(&s).len() {
        let mut rules = effects(&s);
        rules.remove(missing);
        omissions.push(rules);
    }
    for rules in omissions {
        let actor = key(&s.world, s.people.viewer.id(), rules).await;
        assert_eq!(
            declare(&s, &actor.secret, body(&plan)).await.status,
            StatusCode::FORBIDDEN
        );
        unchanged(&s, &plan).await;
    }
    let actor = key(&s.world, s.people.viewer.id(), effects(&s)).await;
    let reply = declare(&s, &actor.secret, body(&plan)).await;
    let receipt = declared_receipt(&reply);
    assert_eq!(receipt.preflight_id, plan.preflight_id);
    assert_eq!(
        serde_json::from_slice::<Json>(&reply.raw).unwrap(),
        json!({"projection":"receipt","preflight_id":plan.preflight_id})
    );
    assert!(lost(&s).await);
    assert_eq!(s.previews().await, 0);
    let environments: Vec<String> =
        sqlx::query_scalar("SELECT status FROM environments WHERE id=ANY($1) ORDER BY id")
            .bind(vec![s.own, s.other])
            .fetch_all(&s.world.db.pool)
            .await
            .unwrap();
    assert_eq!(environments, vec!["lost", "lost"]);
    let state: String = sqlx::query_scalar("SELECT state FROM threads WHERE node_id=$1")
        .bind(s.thread)
        .fetch_one(&s.world.db.pool)
        .await
        .unwrap();
    assert_eq!(state, "blocked");
    let accepted: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notification_operations WHERE id=$1 AND data->>'kind'='lost'",
    )
    .bind(plan.preflight_id)
    .fetch_one(&s.world.db.pool)
    .await
    .unwrap();
    assert_eq!(accepted, 1);
    let commands: i64 = sqlx::query_scalar("SELECT count(*) FROM commands")
        .fetch_one(&s.world.db.pool)
        .await
        .unwrap();
    assert_eq!(commands, 0, "loss never contacts an unreachable machine");
}

#[tokio::test]
async fn loss_confirmation_preserves_the_ordinary_result_and_independent_native_names() {
    for names in [false, true] {
        let s = Scene::new().await;
        let plan = preview(&s).await;
        let actor = key(&s.world, s.people.viewer.id(), complete(&s, names)).await;
        let result = declared_full(&declare(&s, &actor.secret, body(&plan)).await);
        if names {
            assert_eq!(result, plan);
        } else {
            let mut expected = plan.clone();
            let environment = expected
                .environments
                .iter_mut()
                .find(|row| row.environment.id == s.own)
                .unwrap();
            environment.threads_hidden += environment.threads.len() as i64;
            environment.threads.clear();
            assert_eq!(result, expected);
        }
    }
    let s = Scene::new().await;
    let plan = preview(&s).await;
    let result = declared_full(&declare(&s, &s.people.viewer.token, body(&plan)).await);
    assert_eq!(
        result, plan,
        "the original ordinary native result stays unchanged"
    );
    let s = Scene::new().await;
    let plan = preview(&s).await;
    let actor = key(&s.world, s.people.viewer.id(), effects(&s)).await;
    let mut wrong = body(&plan);
    wrong.confirm = "not the reviewed exact name".into();
    assert_eq!(
        declare(&s, &actor.secret, wrong).await.status,
        StatusCode::BAD_REQUEST
    );
    unchanged(&s, &plan).await;
    sqlx::query("DELETE FROM permission_entries WHERE holder_id=$1 AND permission='host.manage'")
        .bind(s.people.viewer.id())
        .execute(&s.world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        declare(&s, &actor.secret, body(&plan)).await.status,
        StatusCode::FORBIDDEN
    );
    unchanged(&s, &plan).await;
}

#[tokio::test]
async fn loss_confirmation_keeps_the_original_preview_operation_separate_and_current() {
    for close in ["unchanged", "scope", "expired", "revoked"] {
        let s = Scene::new().await;
        let original = key(&s.world, s.people.viewer.id(), s.manage()).await;
        let preview = receipt(
            &s.call(&original.secret, LossTarget::Host { id: s.host })
                .await,
        );
        let plan = LossDeclareBody {
            preflight_id: preview.preflight_id,
            confirm: "Mixed host".into(),
            notify: false,
        };
        let actor = key(&s.world, s.people.viewer.id(), effects(&s)).await;
        match close {
            "scope" => {
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(original.id)
                    .bind(json!({"rules":[rule(DECLARE_OP,"host",s.host,"manage")]}))
                    .execute(&s.world.db.pool)
                    .await
                    .unwrap();
            }
            "expired" => {
                sqlx::query("UPDATE api_tokens SET expires_at=$2 WHERE id=$1")
                    .bind(original.id)
                    .bind(s.world.now())
                    .execute(&s.world.db.pool)
                    .await
                    .unwrap();
            }
            "revoked" => {
                sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
                    .bind(original.id)
                    .bind(s.world.now())
                    .execute(&s.world.db.pool)
                    .await
                    .unwrap();
            }
            _ => (),
        }
        let reply = declare(&s, &actor.secret, plan).await;
        if close == "unchanged" {
            assert_eq!(declared_receipt(&reply).preflight_id, preview.preflight_id);
            assert!(lost(&s).await);
        } else {
            assert_eq!(
                reply.status,
                StatusCode::FORBIDDEN,
                "a new declaration key cannot relabel or renew the original preview"
            );
            assert!(!lost(&s).await);
            assert_eq!(s.previews().await, 1);
            let repeat = declare(
                &s,
                &s.people.viewer.token,
                LossDeclareBody {
                    preflight_id: preview.preflight_id,
                    confirm: "Mixed host".into(),
                    notify: false,
                },
            )
            .await;
            assert_eq!(
                repeat.status,
                StatusCode::FORBIDDEN,
                "a browser cannot discard known original custody"
            );
        }
    }
}

#[tokio::test]
async fn loss_confirmation_checks_current_keys_across_native_plan_root_and_first_insert_waits() {
    for boundary in ["plan", "root", "insert"] {
        for expired in [false, true] {
            let s = Scene::new().await;
            let plan = preview(&s).await;
            let actor = key(&s.world, s.people.viewer.id(), effects(&s)).await;
            sqlx::query("UPDATE api_tokens SET expires_at=$2 WHERE id=$1")
                .bind(actor.id)
                .bind(s.world.now() + chrono::Duration::minutes(2))
                .execute(&s.world.db.pool)
                .await
                .unwrap();
            let mut blocker = s.world.db.pool.begin().await.unwrap();
            match boundary {
                "plan" => {
                    sqlx::query("SELECT id FROM preflights WHERE id=$1 FOR UPDATE")
                        .bind(plan.preflight_id)
                        .execute(&mut *blocker)
                        .await
                        .unwrap();
                }
                "root" => {
                    sqlx::query("SELECT id FROM hosts WHERE id=$1 FOR UPDATE")
                        .bind(s.host)
                        .execute(&mut *blocker)
                        .await
                        .unwrap();
                }
                _ => {
                    sqlx::query("LOCK TABLE notification_operations IN SHARE MODE")
                        .execute(&mut *blocker)
                        .await
                        .unwrap();
                }
            }
            let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *blocker)
                .await
                .unwrap();
            let request = start(&s, actor.secret, body(&plan));
            wait(
                &s,
                pid,
                match boundary {
                    "plan" => "%FROM preflights%FOR UPDATE%",
                    "root" => "%FROM hosts WHERE id = $1 FOR UPDATE%",
                    _ => "%INSERT INTO notification_operations%",
                },
                &request,
            )
            .await;
            assert!(!lost(&s).await);
            if expired {
                s.world.advance(chrono::Duration::minutes(3));
            }
            blocker.commit().await.unwrap();
            let reply = done(request).await;
            if expired {
                assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
                unchanged(&s, &plan).await;
            } else {
                declared_receipt(&reply);
                assert!(lost(&s).await);
            }
        }
    }
}

#[tokio::test]
async fn loss_confirmation_publication_retains_its_committed_effect_without_replaying_management() {
    for close in ["unchanged", "manage_closed", "read_closed", "expired"] {
        let s = Scene::new().await;
        let plan = preview(&s).await;
        let actor = key(&s.world, s.people.viewer.id(), complete(&s, true)).await;
        let mut blocker = s.world.db.pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let request = start(&s, actor.secret, body(&plan));
        wait(&s, pid, "%INSERT INTO api_token_activity%", &request).await;
        assert!(lost(&s).await);
        assert_eq!(s.previews().await, 0);
        let saved: (String, Json) =
            sqlx::query_as("SELECT xmin::text,data FROM notification_operations WHERE id=$1")
                .bind(plan.preflight_id)
                .fetch_one(&s.world.db.pool)
                .await
                .unwrap();
        match close {
            "manage_closed" => {
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(actor.id)
                    .bind(json!({"rules":reads(&s,true)}))
                    .execute(&s.world.db.pool)
                    .await
                    .unwrap();
            }
            "read_closed" => {
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(actor.id)
                    .bind(json!({"rules":effects(&s)}))
                    .execute(&s.world.db.pool)
                    .await
                    .unwrap();
            }
            "expired" => {
                s.world.advance(chrono::Duration::hours(2));
            }
            _ => (),
        }
        blocker.commit().await.unwrap();
        let reply = done(request).await;
        if matches!(close, "read_closed" | "expired") {
            assert_eq!(declared_receipt(&reply).preflight_id, plan.preflight_id);
        } else {
            assert_eq!(declared_full(&reply), plan);
        }
        let current: (String, Json) =
            sqlx::query_as("SELECT xmin::text,data FROM notification_operations WHERE id=$1")
                .bind(plan.preflight_id)
                .fetch_one(&s.world.db.pool)
                .await
                .unwrap();
        assert_eq!(
            current, saved,
            "final publication never repeats the native accepted cascade"
        );
    }
}
