//! The real adopted history is retried under its own source, through native waits and receipts.
use super::*;
use orch_api_contract::threads::ThreadAdoptRetryHistory;
use orch_api_core::testing::RawReply;
use thread_history_scope::{Proof, key, narrow, rule};
const OP: &str = "ThreadAdoptRetryHistory";
async fn ready() -> (Scene, Thread) {
    let scene = Scene::new().await;
    the_harness_lists_its_sessions(&scene).await;
    registered(&scene, "/work").await;
    a_history_read_fails(&scene);
    let made = adopt(&scene, &scene.ada, "native-a", None, None).await;
    assert!(made.status.is_success());
    let thread: Thread = serde_json::from_slice(&made.raw).unwrap();
    the_harness_recorded(&scene, Some(MODEL), &two_turns()).await;
    (scene, thread)
}
fn grants(scene: &Scene, thread: Id) -> Vec<serde_json::Value> {
    vec![
        rule(OP, "thread", thread, "self", "manage"),
        rule(OP, "provider", scene.on.provider, "self", "read"),
    ]
}
async fn send(scene: &Scene, thread: Id, token: &str) -> RawReply {
    scene
        .rig
        .world
        .app
        .call(ThreadAdoptRetryHistory, &[&thread])
        .token(token)
        .send_bytes()
        .await
}
fn receipt(reply: &RawReply, thread: Id, changed: bool) -> Proof {
    assert_eq!(
        reply.status.as_u16(),
        200,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    let value: serde_json::Value = serde_json::from_slice(&reply.raw)?;
    assert_eq!(
        value,
        json!({"projection":"receipt", "thread_id":thread,"changed":changed})
    );
    for text in [
        "Fix the login",
        "Fixed the handler",
        "history_error",
        "native_session_ref",
        "approval_mode",
    ] {
        assert!(!String::from_utf8_lossy(&reply.raw).contains(text));
    }
    Ok(())
}
async fn before_count(scene: &Scene, thread: Id) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM items WHERE thread_id=$1 AND before_orch")
        .bind(thread)
        .fetch_one(scene.rig.db.pool())
        .await
        .unwrap()
}
#[tokio::test(flavor = "multi_thread")]
async fn retry_binds_the_original_thread_and_provider_without_lending_another_read_or_requiring_thread_content()
-> Proof {
    let (scene, original) = ready().await;
    let thread = original.base.id;
    let world = &scene.rig.world;
    let reads = scene.rig.commands.issued_of("agent.read_session");
    let discoveries = scene.rig.commands.issued_of("agent.discover_sessions");
    for rules in [
        vec![rule(OP, "thread", thread, "self", "manage")],
        vec![rule(OP, "provider", scene.on.provider, "self", "read")],
        vec![
            rule(OP, "thread", thread, "self", "manage"),
            rule(OP, "provider", new_id(), "self", "read"),
        ],
        vec![
            rule(OP, "thread", new_id(), "self", "manage"),
            rule(OP, "provider", scene.on.provider, "self", "read"),
        ],
        vec![
            rule(OP, "thread", thread, "self", "manage"),
            rule(
                "ThreadAdoptPreview",
                "provider",
                scene.on.provider,
                "self",
                "read",
            ),
        ],
    ] {
        let selected = key(world, scene.ada.id(), rules).await?;
        assert_eq!(
            send(&scene, thread, &selected.secret).await.status.as_u16(),
            403
        );
    }
    let viewer = scene.rig.person("History viewer").await;
    scene
        .rig
        .lets_use("node", thread, "thread.view", &viewer, json!({}))
        .await;
    let no_native_manage = key(world, viewer.id(), grants(&scene, thread)).await?;
    assert_eq!(
        send(&scene, thread, &no_native_manage.secret)
            .await
            .status
            .as_u16(),
        403
    );
    assert_eq!(scene.rig.commands.issued_of("agent.read_session"), reads);
    assert_eq!(
        scene.rig.commands.issued_of("agent.discover_sessions"),
        discoveries
    );
    let selected = key(world, scene.ada.id(), grants(&scene, thread)).await?;
    receipt(&send(&scene, thread, &selected.secret).await, thread, true)?;
    assert_eq!(before_count(&scene, thread).await, 4);
    assert_eq!(
        adopted_line(&items_of(&scene, thread).await).history_missing,
        None
    );
    let count = scene.rig.commands.issued_of("agent.read_session");
    receipt(&send(&scene, thread, &selected.secret).await, thread, false)?;
    assert_eq!(scene.rig.commands.issued_of("agent.read_session"), count);
    let ordinary = send(&scene, thread, &scene.ada.token).await;
    let ordinary: Thread = serde_json::from_slice(&ordinary.raw)?;
    assert_eq!(ordinary.model, original.model);
    assert_eq!(scene.rig.commands.issued_of("turn.start"), 0);
    assert_eq!(scene.rig.commands.issued_of("session.open"), 0);
    Ok(())
}
async fn held_read(scene: &Scene, earlier: usize) -> Proof<Id> {
    tokio::time::timeout(rig::PATIENCE, async {
        loop {
            let runs = scene
                .rig
                .machine
                .handler
                .executions()
                .into_iter()
                .filter(|run| run.kind == CommandKind::AgentReadSession)
                .collect::<Vec<_>>();
            if runs.len() > earlier {
                return runs.last().map(|run| run.id).ok_or("native read missing");
            }
            tokio::task::yield_now().await;
        }
    })
    .await?
    .map_err(Into::into)
}
#[tokio::test(flavor = "multi_thread")]
async fn retry_rechecks_its_exact_original_operation_after_native_read_wait_before_any_history_write()
-> Proof {
    for mode in ["live", "scope", "expired", "association"] {
        let (scene, original) = ready().await;
        let thread = original.base.id;
        let selected = key(&scene.rig.world, scene.ada.id(), grants(&scene, thread)).await?;
        let earlier = scene
            .rig
            .machine
            .handler
            .executions()
            .iter()
            .filter(|run| run.kind == CommandKind::AgentReadSession)
            .count();
        scene
            .rig
            .machine
            .handler
            .hold(CommandKind::AgentReadSession);
        let app = scene.rig.world.app.clone();
        let secret = selected.secret.clone();
        let pending = tokio::spawn(async move {
            let request = app.call(ThreadAdoptRetryHistory, &[&thread]).token(&secret);
            request.send_bytes().await
        });
        let id = held_read(&scene, earlier).await?;
        let authority = scene
            .rig
            .commands
            .issued_authority(id)
            .ok_or("the native history read has no retained operation")?;
        let value = serde_json::to_value(authority)?;
        assert_eq!(value["kind"], "adoption_preparation");
        assert_eq!(value["authority"]["request"]["purpose"], "RetryHistory");
        assert_eq!(
            value["authority"]["request"]["retained"]["thread"],
            thread.to_string()
        );
        assert_eq!(before_count(&scene, thread).await, 0);
        match mode {
            "scope" => {
                narrow(
                    &scene.rig.world,
                    &selected,
                    vec![rule(OP, "thread", thread, "self", "manage")],
                )
                .await?
            }
            "expired" => scene.rig.world.advance(chrono::Duration::hours(2)),
            "association" => {
                sqlx::query("UPDATE items SET payload=jsonb_set(payload,'{data,native_session_ref}','\"native-c\"'::jsonb) WHERE thread_id=$1 AND native_ref='adopted'").bind(thread).execute(scene.rig.db.pool()).await?;
            }
            _ => {}
        }
        scene.rig.machine.handler.release();
        let reply = tokio::time::timeout(rig::PATIENCE, pending).await??;
        if mode == "live" {
            receipt(&reply, thread, true)?;
            assert_eq!(before_count(&scene, thread).await, 4);
        } else {
            assert_eq!(
                reply.status.as_u16(),
                if mode == "expired" { 401 } else { 403 }
            );
            assert_eq!(before_count(&scene, thread).await, 0);
            let missing: bool = sqlx::query_scalar("SELECT payload->'data'->>'history_missing'='unreadable' FROM items WHERE thread_id=$1 AND native_ref='adopted'").bind(thread).fetch_one(scene.rig.db.pool()).await?;
            assert!(missing);
        }
    }
    Ok(())
}
#[tokio::test(flavor = "multi_thread")]
async fn retry_rechecks_after_its_actual_native_session_lock_before_import_and_keeps_the_live_control()
-> Proof {
    for expired in [false, true] {
        let (scene, original) = ready().await;
        let thread = original.base.id;
        let selected = key(&scene.rig.world, scene.ada.id(), grants(&scene, thread)).await?;
        let mut locked = scene.rig.world.db.pool.begin().await?;
        orch_db::queries::adoption::lock_session(&mut locked, "native-a").await?;
        let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *locked)
            .await?;
        let app = scene.rig.world.app.clone();
        let secret = selected.secret.clone();
        let pending = tokio::spawn(async move {
            let request = app.call(ThreadAdoptRetryHistory, &[&thread]).token(&secret);
            request.send_bytes().await
        });
        tokio::time::timeout(rig::PATIENCE, async { loop {
            assert!(!pending.is_finished());
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)))").bind(blocker).fetch_one(scene.rig.db.pool()).await.unwrap();
            if waiting { break; }
            tokio::task::yield_now().await;
        } }).await?;
        assert_eq!(before_count(&scene, thread).await, 0);
        if expired {
            scene.rig.world.advance(chrono::Duration::hours(2));
        }
        locked.commit().await?;
        let reply = tokio::time::timeout(rig::PATIENCE, pending).await??;
        if expired {
            assert_eq!(reply.status.as_u16(), 401);
            assert_eq!(before_count(&scene, thread).await, 0);
        } else {
            receipt(&reply, thread, true)?;
            assert_eq!(before_count(&scene, thread).await, 4);
        }
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn completed_history_retry_keeps_its_receipt_after_the_actual_activity_wait_and_key_expiry()
-> Proof {
    let (scene, original) = ready().await;
    let thread = original.base.id;
    let selected = key(&scene.rig.world, scene.ada.id(), grants(&scene, thread)).await?;
    let mut blocked = scene.rig.world.db.pool.begin().await?;
    sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
        .execute(&mut *blocked)
        .await?;
    let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocked)
        .await?;
    let app = scene.rig.world.app.clone();
    let secret = selected.secret.clone();
    let pending = tokio::spawn(async move {
        let request = app.call(ThreadAdoptRetryHistory, &[&thread]).token(&secret);
        request.send_bytes().await
    });
    tokio::time::timeout(rig::PATIENCE, async {
        loop {
            assert!(!pending.is_finished());
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query ILIKE '%INSERT INTO api_token_activity%')").bind(blocker).fetch_one(scene.rig.db.pool()).await.unwrap();
            if waiting && before_count(&scene, thread).await == 4 { break; }
            tokio::task::yield_now().await;
        }
    }).await?;
    scene.rig.world.advance(chrono::Duration::hours(2));
    blocked.commit().await?;
    let reply = tokio::time::timeout(rig::PATIENCE, pending).await??;
    receipt(&reply, thread, true)?;
    assert_eq!(before_count(&scene, thread).await, 4);
    Ok(())
}
