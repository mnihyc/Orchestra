//! Effect-free privileged body reads keep their native Manage and original source/value.

use super::*;
use orch_api_contract::loss::{LossPreflightBody, LossRecoveryList, LossTarget, RecoverySources};

const OPERATION: &str = "LossRecoveryList";

fn host(s: &Scene) -> Json {
    rule(
        OPERATION,
        "host",
        s.team.host,
        "self",
        &["host"],
        &["manage"],
    )
}
fn environment(_s: &Scene, id: Id) -> Json {
    rule(
        OPERATION,
        "environment",
        id,
        "self",
        &["environment"],
        &["manage"],
    )
}

async fn call(s: &Scene, token: &str, target: LossTarget) -> RawReply {
    s.world
        .app
        .call(LossRecoveryList, &[])
        .token(token)
        .body(&LossPreflightBody { target })
        .send_bytes()
        .await
}

async fn shown(s: &Scene, token: &str, target: LossTarget) -> RecoverySources {
    let reply = call(s, token, target).await;
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}

async fn activity_wait(s: &Scene, pid: i32, request: &tokio::task::JoinHandle<RawReply>) {
    tokio::time::timeout(std::time::Duration::from_secs(30),async{
        loop{
            assert!(!request.is_finished(),"the native body preview must reach activity publication");
            let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                .bind(pid).fetch_one(&s.world.db.pool).await.unwrap();
            if blocked{break;}
            tokio::task::yield_now().await;
        }
    }).await.expect("the actual activity INSERT after the effect-free native preview");
}

#[tokio::test]
async fn loss_recovery_body_read_keeps_native_manage_metadata_and_scopes_members_before_names() {
    let s = Scene::new().await;
    let target = LossTarget::Host { id: s.team.host };
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM preflights")
        .fetch_one(&s.world.db.pool)
        .await
        .unwrap();
    let native = shown(&s, &s.team.c.admin.token, target).await;
    assert_eq!(native.environments.len(), 2);
    let full = key(
        &s.world,
        s.team.c.admin.id(),
        vec![
            host(&s),
            environment(&s, s.first.base.id),
            environment(&s, s.second.base.id),
        ],
    )
    .await;
    assert_eq!(
        shown(&s, &full.secret, target).await,
        native,
        "Manage remains the genuine inspection action, without adding Read"
    );
    let narrow = key(
        &s.world,
        s.team.c.admin.id(),
        vec![host(&s), environment(&s, s.first.base.id)],
    )
    .await;
    let selected = shown(&s, &narrow.secret, target).await;
    assert_eq!(selected.environments.len(), 1);
    assert_eq!(selected.environments[0].id, s.first.base.id);
    assert_eq!(
        selected.others,
        native.others + 1,
        "the managed Host licenses its native redacted aggregate count"
    );
    assert!(
        !serde_json::to_string(&selected)
            .unwrap()
            .contains(&s.second.base.name)
    );
    let summary = key(&s.world, s.team.c.admin.id(), vec![host(&s)]).await;
    let selected = shown(&s, &summary.secret, target).await;
    assert!(selected.environments.is_empty());
    assert_eq!(selected.others, native.others + 2);
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM preflights")
        .fetch_one(&s.world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        after, before,
        "this POST is a native reader, not a saved or applied loss plan"
    );
}

#[tokio::test]
async fn loss_recovery_requires_its_exact_target_action_and_native_host_machine_or_environment_gate()
 {
    let s = Scene::new().await;
    let target = LossTarget::Host { id: s.team.host };
    let good = key(&s.world, s.team.c.admin.id(), vec![host(&s)]).await;
    assert_eq!(call(&s, &good.secret, target).await.status, StatusCode::OK);
    for policy in [
        vec![rule(
            OPERATION,
            "host",
            s.team.host,
            "self",
            &["host"],
            &["read"],
        )],
        vec![rule(
            "LossPreflight",
            "host",
            s.team.host,
            "self",
            &["host"],
            &["manage"],
        )],
        vec![rule(
            OPERATION,
            "environment",
            s.team.host,
            "self",
            &["host"],
            &["manage"],
        )],
        vec![environment(&s, s.first.base.id)],
    ] {
        let denied = key(&s.world, s.team.c.admin.id(), policy).await;
        let reply = call(&s, &denied.secret, target).await;
        assert_eq!(reply.status, StatusCode::FORBIDDEN);
        assert!(!String::from_utf8_lossy(&reply.raw).contains(&s.first.base.name));
    }
    let viewer = key(
        &s.world,
        s.team.c.viewer.id(),
        vec![host(&s), environment(&s, s.first.base.id)],
    )
    .await;
    let denied = call(&s, &viewer.secret, target).await;
    assert_eq!(
        denied.status,
        StatusCode::FORBIDDEN,
        "scope cannot lend native HostManage"
    );
    assert!(
        !String::from_utf8_lossy(&denied.raw).contains("Build box"),
        "an unlicensed native refusal carries no stored Host name"
    );
    let one = key(
        &s.world,
        s.team.c.admin.id(),
        vec![environment(&s, s.first.base.id)],
    )
    .await;
    let scoped = shown(
        &s,
        &one.secret,
        LossTarget::Environment {
            id: s.first.base.id,
        },
    )
    .await;
    let native = shown(
        &s,
        &s.team.c.admin.token,
        LossTarget::Environment {
            id: s.first.base.id,
        },
    )
    .await;
    assert_eq!(scoped, native);
    let machine = fixtures::insert_persons_machine(
        &mut *s.world.conn().await,
        s.team.c.admin.id(),
        "Recovery body physical source",
    )
    .await
    .unwrap();
    let key = key(
        &s.world,
        s.team.c.admin.id(),
        vec![rule(
            OPERATION,
            "machine",
            machine,
            "self",
            &["machine"],
            &["manage"],
        )],
    )
    .await;
    assert_eq!(
        shown(&s, &key.secret, LossTarget::Machine { id: machine }).await,
        shown(
            &s,
            &s.team.c.admin.token,
            LossTarget::Machine { id: machine }
        )
        .await,
        "the actual native machine-owner branch is preserved"
    );
}

#[tokio::test]
async fn loss_recovery_body_publication_does_not_borrow_root_manage_after_member_manage_is_lost() {
    for withdraw in [false, true] {
        let s = Scene::new().await;
        resources::name_person(
            &s.world,
            "host",
            s.team.host,
            "host.manage",
            s.team.c.viewer.id(),
        )
        .await;
        let grant = resources::name_person(
            &s.world,
            "environment",
            s.first.base.id,
            "environment.manage",
            s.team.c.viewer.id(),
        )
        .await;
        let target = LossTarget::Host { id: s.team.host };
        let reader = key(
            &s.world,
            s.team.c.viewer.id(),
            vec![host(&s), environment(&s, s.first.base.id)],
        )
        .await;
        let allowed = shown(&s, &reader.secret, target).await;
        assert_eq!(allowed.environments.len(), 1);
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
        let token = reader.secret.clone();
        let request = tokio::spawn(async move {
            let call = app.call(LossRecoveryList, &[]);
            call.token(&token)
                .body(&LossPreflightBody { target })
                .send_bytes()
                .await
        });
        activity_wait(&s, pid, &request).await;
        if withdraw {
            assert!(
                orch_db::queries::access::remove_entry(
                    &mut *s.world.conn().await,
                    grant,
                    s.team.c.admin.id(),
                    s.world.now()
                )
                .await
                .unwrap()
            );
            let current = shown(&s, &s.team.c.viewer.token, target).await;
            assert!(
                current.environments.is_empty(),
                "native target management remains, while member management is gone"
            );
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            reply.status,
            if withdraw {
                StatusCode::FORBIDDEN
            } else {
                StatusCode::OK
            }
        );
        if withdraw {
            assert!(!String::from_utf8_lossy(&reply.raw).contains(&s.first.base.name));
        }
    }
}

#[tokio::test]
async fn loss_recovery_body_post_remains_a_refused_read_after_final_publication_key_expiry() {
    for expire in [false, true] {
        let s = Scene::new().await;
        let target = LossTarget::Host { id: s.team.host };
        let reader = key(
            &s.world,
            s.team.c.admin.id(),
            vec![host(&s), environment(&s, s.first.base.id)],
        )
        .await;
        assert_eq!(
            shown(&s, &reader.secret, target).await.environments.len(),
            1
        );
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
        let token = reader.secret.clone();
        let request = tokio::spawn(async move {
            let call = app.call(LossRecoveryList, &[]);
            call.token(&token)
                .body(&LossPreflightBody { target })
                .send_bytes()
                .await
        });
        activity_wait(&s, pid, &request).await;
        if expire {
            s.world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            reply.status,
            if expire {
                StatusCode::UNAUTHORIZED
            } else {
                StatusCode::OK
            },
            "a pure POST read has no committed writer receipt"
        );
        if expire {
            assert!(!String::from_utf8_lossy(&reply.raw).contains(&s.first.base.name));
        }
    }
}
