//! Native placement uses the source and destination currently stored after its real lock wait.

use super::*;
use orch_api_core::testing::RawReply;
use orch_types::Json;
use thread_history_scope::{Proof, key, narrow, rule};

const OP: &str = "ThreadEnvironmentSet";

fn manages(s: &Scene, thread: Id, to: Id) -> Vec<Json> {
    vec![
        rule(OP, "thread", thread, "self", "manage"),
        rule(OP, "environment", s.on.environment, "self", "manage"),
        rule(OP, "environment", to, "self", "manage"),
    ]
}
async fn reads(s: &Scene, thread: Id, to: Id, operation: &str) -> Proof<Vec<Json>> {
    let mut rules = vec![
        rule(operation, "thread", thread, "self", "read"),
        rule(operation, "environment", to, "self", "read"),
        rule(operation, "org", s.tree.org.base.id, "self", "read"),
        rule(operation, "project", s.tree.project.base.id, "self", "read"),
        rule(operation, "topic", s.tree.topic.base.id, "self", "read"),
        rule(operation, "agent", s.on.agent, "self", "read"),
        rule(operation, "provider", s.on.provider, "self", "read"),
    ];
    let host = orch_db::queries::resources::hosting::machine_hosts(
        &mut *s.rig.world.conn().await,
        &[s.rig.machine_id],
    )
    .await?
    .into_iter()
    .find(|(machine, _, _)| *machine == s.rig.machine_id)
    .map(|(_, host, _)| host);
    rules.push(rule(
        operation,
        if host.is_some() { "host" } else { "machine" },
        host.unwrap_or(s.rig.machine_id),
        "self",
        "read",
    ));
    Ok(rules)
}
fn request(to: Id) -> ThreadEnvironmentSetBody {
    ThreadEnvironmentSetBody {
        choice: EnvironmentChoice::Use(to),
        directory: Some("selected/working-directory".into()),
    }
}
async fn put(s: &Scene, thread: Id, to: Id, secret: &str) -> RawReply {
    let request = s
        .rig
        .world
        .app
        .call(ThreadEnvironmentSet, &[&thread])
        .token(secret)
        .body(&request(to));
    request.send_bytes().await
}
fn body(reply: &RawReply) -> Proof<Json> {
    assert_eq!(
        reply.status.as_u16(),
        200,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    Ok(serde_json::from_slice(&reply.raw)?)
}
async fn state(s: &Scene, thread: Id) -> Proof<(Option<Id>, String, i64)> {
    Ok(
        sqlx::query_as(
            "SELECT environment_id,workspace_dir,next_seq FROM threads WHERE node_id=$1",
        )
        .bind(thread)
        .fetch_one(s.rig.db.pool())
        .await?,
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn placement_requires_its_exact_thread_manage_both_actual_environments_and_native_authority_and_keeps_write_only_receipts()
-> Proof {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let to = another_environment(&s, "Scoped placement destination", Some(s.rig.machine_id)).await;
    let original = state(&s, thread).await?;
    for grants in [
        vec![
            rule(OP, "thread", thread, "self", "manage"),
            rule(OP, "environment", to, "self", "manage"),
        ],
        vec![
            rule(OP, "thread", thread, "self", "manage"),
            rule(OP, "environment", s.on.environment, "self", "manage"),
        ],
        vec![
            rule(OP, "thread", thread, "self", "read"),
            rule(OP, "environment", s.on.environment, "self", "manage"),
            rule(OP, "environment", to, "self", "manage"),
        ],
        vec![rule("ThreadBindingSet", "thread", thread, "self", "manage")],
    ] {
        let selected = key(&s.rig.world, s.ada.id(), grants).await?;
        assert!(
            !put(&s, thread, to, &selected.secret)
                .await
                .status
                .is_success()
        );
        assert_eq!(state(&s, thread).await?, original);
    }
    let outsider = key(&s.rig.world, s.cai.id(), manages(&s, thread, to)).await?;
    assert!(
        !put(&s, thread, to, &outsider.secret)
            .await
            .status
            .is_success()
    );
    assert_eq!(state(&s, thread).await?, original);
    let selected = key(&s.rig.world, s.ada.id(), manages(&s, thread, to)).await?;
    let value = body(&put(&s, thread, to, &selected.secret).await)?;
    assert_eq!(value["projection"], "receipt");
    assert_eq!(value["thread_id"], json!(thread));
    assert_eq!(value["environment_id"], json!(to));
    for field in ["base", "directory", "binding", "environment", "name"] {
        assert!(value.get(field).is_none());
    }
    let actual = state(&s, thread).await?;
    assert_eq!(
        (actual.0, actual.1.as_str()),
        (Some(to), "selected/working-directory")
    );
    let (_, _, segment_environment, _) = segment_of(&s, thread).await;
    assert_eq!(segment_environment, to);
    assert_eq!(bound_lines(&s, thread).await, 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn placement_readback_uses_only_its_own_thread_and_actual_native_content_sources() -> Proof {
    for mode in ["thread", "foreign", "missing_environment", "all"] {
        let s = Scene::new().await;
        let thread = s.thread().await.base.id;
        let to =
            another_environment(&s, "Native readback destination", Some(s.rig.machine_id)).await;
        let mut grants = manages(&s, thread, to);
        match mode {
            "thread" => grants.push(rule(OP, "thread", thread, "self", "read")),
            "foreign" => {
                grants.push(rule(OP, "thread", thread, "self", "read"));
                grants.extend(reads(&s, thread, to, "NodeGet.read").await?);
            }
            "missing_environment" => {
                grants.extend(
                    reads(&s, thread, to, OP)
                        .await?
                        .into_iter()
                        .filter(|rule| rule["target"]["id"] != json!(to)),
                );
            }
            _ => grants.extend(reads(&s, thread, to, OP).await?),
        }
        let selected = key(&s.rig.world, s.ada.id(), grants).await?;
        let value = body(&put(&s, thread, to, &selected.secret).await)?;
        assert_eq!(value.get("projection").is_none(), mode == "all");
        if mode == "all" {
            let thread: Thread = serde_json::from_value(value)?;
            assert_eq!(
                thread.environment.ok_or("placed environment missing")?.id,
                to
            );
            assert_eq!(thread.directory, "selected/working-directory");
        } else {
            assert_eq!(value["projection"], "receipt");
        }
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn placement_rechecks_real_key_lifetime_and_current_source_environment_after_the_actual_thread_row_wait()
-> Proof {
    for mode in ["unchanged", "expired", "source"] {
        let s = Scene::new().await;
        let thread = s.thread().await.base.id;
        let to =
            another_environment(&s, "Locked placement destination", Some(s.rig.machine_id)).await;
        let other =
            another_environment(&s, "Unadmitted current source", Some(s.rig.machine_id)).await;
        let selected = key(&s.rig.world, s.ada.id(), manages(&s, thread, to)).await?;
        let original = state(&s, thread).await?;
        let mut blocker = s.rig.db.begin().await?;
        orch_db::queries::threads::lock(&mut blocker, thread)
            .await?
            .ok_or("native thread missing")?;
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await?;
        let app = s.rig.world.app.clone();
        let secret = selected.secret.clone();
        let pending = tokio::spawn(async move {
            let request = app
                .call(ThreadEnvironmentSet, &[&thread])
                .token(&secret)
                .body(&request(to));
            request.send_bytes().await
        });
        tokio::time::timeout(rig::PATIENCE, async {
            loop {
                assert!(!pending.is_finished(), "the actual native thread lock must be reached");
                let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query ILIKE '%FROM threads%FOR UPDATE%')")
                    .bind(pid).fetch_one(s.rig.db.pool()).await.unwrap();
                if waiting { break; }
                tokio::task::yield_now().await;
            }
        }).await?;
        match mode {
            "expired" => s.rig.world.advance(chrono::Duration::hours(2)),
            "source" => {
                sqlx::query("UPDATE threads SET environment_id=$2 WHERE node_id=$1")
                    .bind(thread)
                    .bind(other)
                    .execute(&mut *blocker)
                    .await?;
            }
            _ => {}
        }
        blocker.commit().await?;
        let reply = tokio::time::timeout(rig::PATIENCE, pending).await??;
        let actual = state(&s, thread).await?;
        if mode == "unchanged" {
            body(&reply)?;
            assert_eq!(actual.0, Some(to));
        } else {
            assert!(!reply.status.is_success());
            assert_eq!(
                actual,
                (
                    if mode == "source" {
                        Some(other)
                    } else {
                        original.0
                    },
                    original.1,
                    original.2
                )
            );
            assert_eq!(bound_lines(&s, thread).await, 0);
        }
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn placement_completed_projection_survives_manage_loss_and_falls_back_after_read_or_source_loss_without_replaying()
-> Proof {
    for mode in ["unchanged", "read", "manage", "association"] {
        let s = Scene::new().await;
        let thread = s.thread().await.base.id;
        let to =
            another_environment(&s, "Retained placement destination", Some(s.rig.machine_id)).await;
        let other =
            another_environment(&s, "Another readable destination", Some(s.rig.machine_id)).await;
        let mut grants = manages(&s, thread, to);
        let mut read = reads(&s, thread, to, OP).await?;
        read.push(rule(OP, "environment", other, "self", "read"));
        grants.extend(read.clone());
        let selected = key(&s.rig.world, s.ada.id(), grants).await?;
        let mut blocker = s.rig.db.begin().await?;
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *blocker)
            .await?;
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await?;
        let app = s.rig.world.app.clone();
        let secret = selected.secret.clone();
        let pending = tokio::spawn(async move {
            let request = app
                .call(ThreadEnvironmentSet, &[&thread])
                .token(&secret)
                .body(&request(to));
            request.send_bytes().await
        });
        tokio::time::timeout(rig::PATIENCE, async {
            loop {
                assert!(!pending.is_finished(), "the actual final activity wait must be reached");
                let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query ILIKE '%INSERT INTO api_token_activity%')")
                    .bind(pid).fetch_one(s.rig.db.pool()).await.unwrap();
                if waiting { break; }
                tokio::task::yield_now().await;
            }
        }).await?;
        let committed = state(&s, thread).await?;
        assert_eq!(committed.0, Some(to));
        match mode {
            "read" => narrow(&s.rig.world, &selected, manages(&s, thread, to)).await?,
            "manage" => narrow(&s.rig.world, &selected, read).await?,
            "association" => {
                sqlx::query("UPDATE threads SET environment_id=$2 WHERE node_id=$1")
                    .bind(thread)
                    .bind(other)
                    .execute(s.rig.db.pool())
                    .await?;
            }
            _ => {}
        }
        blocker.commit().await?;
        let value = body(&tokio::time::timeout(rig::PATIENCE, pending).await??)?;
        assert_eq!(
            value.get("projection").is_none(),
            matches!(mode, "unchanged" | "manage")
        );
        if matches!(mode, "read" | "association") {
            assert_eq!(value["projection"], "receipt");
            assert!(value.get("directory").is_none());
        }
        assert_eq!(state(&s, thread).await?.2, committed.2);
        assert_eq!(bound_lines(&s, thread).await, 1);
    }
    Ok(())
}
