//! Binding and its native descriptor startup share the same source and current publication factory.
use super::*;
use orch_api_core::testing::RawReply;
use orch_link_contract::commands::CommandKind;
use thread_history_scope::{Proof, key, narrow, rule};
const OP: &str = "ThreadBindingSet";
fn manages(thread: Id) -> Vec<serde_json::Value> {
    vec![rule(OP, "thread", thread, "self", "manage")]
}
fn request(agent: Id, provider: Id) -> ThreadBindingSetBody {
    ThreadBindingSetBody {
        agent_id: agent,
        provider_id: provider,
        arrangement: None,
        model: Some(MODEL.into()),
        effort: None,
    }
}
fn reads(
    s: &Scene,
    thread: Id,
    agent: Id,
    provider: Id,
    namespace: &str,
) -> Vec<serde_json::Value> {
    [
        ("thread", thread),
        ("org", s.tree.org.base.id),
        ("project", s.tree.project.base.id),
        ("topic", s.tree.topic.base.id),
        ("agent", agent),
        ("provider", provider),
        ("environment", s.on.environment),
        ("machine", s.rig.machine_id),
    ]
    .into_iter()
    .map(|(kind, id)| rule(namespace, kind, id, "self", "read"))
    .collect()
}
async fn put(s: &Scene, thread: Id, secret: &str, body: &ThreadBindingSetBody) -> RawReply {
    s.rig
        .world
        .app
        .call(ThreadBindingSet, &[&thread])
        .token(secret)
        .body(body)
        .send_bytes()
        .await
}
fn decoded(reply: &RawReply) -> Proof<serde_json::Value> {
    assert_eq!(
        reply.status.as_u16(),
        200,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    Ok(serde_json::from_slice(&reply.raw)?)
}
fn receipt(reply: &RawReply) -> Proof<serde_json::Value> {
    let value = decoded(reply)?;
    assert_eq!(value["projection"], "receipt");
    assert_eq!(value.as_object().ok_or("binding receipt missing")?.len(), 3);
    assert!(
        value.get("binding").is_none()
            && value.get("environment").is_none()
            && value.get("model").is_none()
    );
    Ok(value)
}
async fn wait(
    s: &Scene,
    pid: i32,
    pending: &tokio::task::JoinHandle<RawReply>,
    activity: bool,
) -> Proof {
    tokio::time::timeout(rig::PATIENCE,async {
        loop {
            assert!(!pending.is_finished(),"the actual native wait must be reached");
            let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND (NOT $2 OR query ILIKE '%INSERT INTO api_token_activity%'))")
                .bind(pid).bind(activity).fetch_one(s.rig.db.pool()).await.unwrap();
            if blocked { break; }tokio::task::yield_now().await;
        }
    }).await?;
    Ok(())
}
async fn fresh(s: &Scene) -> (Id, Id) {
    let (agent, provider) = another_provider(s, "Native unreported binding", &["separated"]).await;
    sqlx::query("UPDATE agents SET descriptor='{}' WHERE id=$1")
        .bind(agent)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    (agent, provider)
}

#[tokio::test(flavor = "multi_thread")]
async fn rebind_binds_its_actual_managed_thread_and_keeps_native_profile_and_provider_use_without_component_writes()
-> Proof {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let other = s.thread_by(&s.ada, "Another binding source").await.base.id;
    let (agent, provider) = another_provider(&s, "Scoped target", &["separated"]).await;
    for grants in [
        vec![rule(OP, "thread", thread, "self", "read")],
        vec![rule(OP, "thread", thread, "self", "write")],
        manages(other),
        vec![rule(
            "ThreadEnvironmentSet",
            "thread",
            thread,
            "self",
            "manage",
        )],
    ] {
        let actor = key(&s.rig.world, s.ada.id(), grants).await?;
        assert_eq!(
            put(&s, thread, &actor.secret, &request(agent, provider))
                .await
                .status
                .as_u16(),
            403
        );
    }
    assert_eq!(bound_lines(&s, thread).await, 0);
    names(&s, "node", thread, "thread.manage", &s.dan, json!({})).await;
    let dan = key(&s.rig.world, s.dan.id(), manages(thread)).await?;
    assert_eq!(
        put(&s, thread, &dan.secret, &request(agent, provider))
            .await
            .status
            .as_u16(),
        403
    );
    names(&s, "agent", agent, "agent.use", &s.dan, json!({})).await;
    assert_eq!(
        put(&s, thread, &dan.secret, &request(agent, provider))
            .await
            .status
            .as_u16(),
        403
    );
    names(&s, "provider", provider, "provider.use", &s.dan, json!({})).await;
    let made = receipt(&put(&s, thread, &dan.secret, &request(agent, provider)).await)?;
    assert_eq!(made["thread_id"], json!(thread));
    let (actual_agent, actual_provider, _, _) = segment_of(&s, thread).await;
    assert_eq!((actual_agent, actual_provider), (agent, provider));
    assert_eq!(bound_lines(&s, thread).await, 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn rebind_checks_the_real_source_row_wait_before_its_original_first_stamp() -> Proof {
    for mode in ["live", "expired", "source"] {
        let s = Scene::new().await;
        let thread = s.thread().await.base.id;
        let (agent, provider) = another_provider(&s, "Waited binding", &["separated"]).await;
        let other =
            another_environment(&s, "Changed binding environment", Some(s.rig.machine_id)).await;
        let actor = key(&s.rig.world, s.ada.id(), manages(thread)).await?;
        let mut locked = s.rig.db.begin().await?;
        orch_db::queries::threads::lock(&mut locked, thread)
            .await?
            .ok_or("native source missing")?;
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *locked)
            .await?;
        let app = s.rig.world.app.clone();
        let secret = actor.secret.clone();
        let body = request(agent, provider);
        let pending = tokio::spawn(async move {
            let call = app
                .call(ThreadBindingSet, &[&thread])
                .token(&secret)
                .body(&body);
            call.send_bytes().await
        });
        wait(&s, pid, &pending, false).await?;
        if mode == "expired" {
            s.rig.world.advance(chrono::Duration::hours(2));
        }
        if mode == "source" {
            sqlx::query("UPDATE threads SET environment_id=$2 WHERE node_id=$1")
                .bind(thread)
                .bind(other)
                .execute(&mut *locked)
                .await?;
        }
        locked.commit().await?;
        let reply = tokio::time::timeout(rig::PATIENCE, pending).await??;
        if mode == "live" {
            receipt(&reply)?;
            assert_eq!(bound_lines(&s, thread).await, 1);
        } else {
            assert!(
                matches!(reply.status.as_u16(), 401 | 403),
                "{}",
                String::from_utf8_lossy(&reply.raw)
            );
            assert_eq!(bound_lines(&s, thread).await, 0);
        }
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn rebind_descriptor_rechecks_after_the_native_harness_claim_before_any_start_or_binding()
-> Proof {
    for expired in [false, true] {
        let s = Scene::new().await;
        let thread = s.thread().await.base.id;
        let (agent, provider) = fresh(&s).await;
        let actor = key(&s.rig.world, s.ada.id(), manages(thread)).await?;
        let mut locked = s.rig.db.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!(
                "harness.claim:{provider}:{agent}:{}",
                s.rig.machine_id
            ))
            .execute(&mut *locked)
            .await?;
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *locked)
            .await?;
        let before = s.rig.commands.issued_of("agent.start");
        let app = s.rig.world.app.clone();
        let secret = actor.secret.clone();
        let body = request(agent, provider);
        let pending = tokio::spawn(async move {
            let call = app
                .call(ThreadBindingSet, &[&thread])
                .token(&secret)
                .body(&body);
            call.send_bytes().await
        });
        wait(&s, pid, &pending, false).await?;
        assert_eq!(s.rig.commands.issued_of("agent.start"), before);
        assert_eq!(bound_lines(&s, thread).await, 0);
        if expired {
            s.rig.world.advance(chrono::Duration::hours(2));
        }
        locked.rollback().await?;
        let reply = tokio::time::timeout(rig::PATIENCE, pending).await??;
        if expired {
            assert!(
                matches!(reply.status.as_u16(), 401 | 403),
                "{}",
                String::from_utf8_lossy(&reply.raw)
            );
            assert_eq!(bound_lines(&s, thread).await, 0);
            assert_eq!(s.rig.commands.issued_of("agent.start"), before);
        } else {
            receipt(&reply)?;
            assert_eq!(bound_lines(&s, thread).await, 1);
        }
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn rebind_native_start_keeps_exact_original_operation_and_rechecks_after_real_io_without_a_login_or_adoption_alias()
-> Proof {
    for mode in ["live", "scope", "expired", "source"] {
        let s = Scene::new().await;
        let thread = s.thread().await.base.id;
        let (agent, provider) = fresh(&s).await;
        let other =
            another_environment(&s, "Other original binding source", Some(s.rig.machine_id)).await;
        let actor = key(&s.rig.world, s.ada.id(), manages(thread)).await?;
        s.rig.machine.handler.hold(CommandKind::AgentStart);
        let app = s.rig.world.app.clone();
        let secret = actor.secret.clone();
        let body = request(agent, provider);
        let pending = tokio::spawn(async move {
            let call = app
                .call(ThreadBindingSet, &[&thread])
                .token(&secret)
                .body(&body);
            call.send_bytes().await
        });
        let id = tokio::time::timeout(rig::PATIENCE, async {
            loop {
                assert!(
                    !pending.is_finished(),
                    "the original request must reach its actual native start"
                );
                for execution in s.rig.machine.handler.executions() {
                    if execution.kind == CommandKind::AgentStart
                        && let Some((command, _)) = s.rig.commands.issued_command(execution.id)
                        && command.body["agent_id"] == json!(agent)
                    {
                        return execution.id;
                    }
                }
                tokio::task::yield_now().await;
            }
        })
        .await?;
        let authority = s
            .rig
            .commands
            .issued_authority(id)
            .ok_or("native startup has no private source")?;
        let value = serde_json::to_value(&authority)?;
        assert_eq!(value["kind"], "rebind_preparation");
        assert_eq!(value["authority"]["origin"]["origin"]["operation"], OP);
        let mut tx = s.rig.db.begin().await?;
        let (row, deadline) = s
            .rig
            .commands
            .issued_command(id)
            .ok_or("actual native start missing")?;
        assert!(row.body.get("origin").is_none() && row.body.get("authority").is_none());
        let proposal = orch_db::queries::link::NewCommand {
            id,
            machine_id: row.machine,
            kind: &row.kind,
            body: &row.body,
            serial_key: row.key.as_deref(),
            deadline_at: deadline.get(),
        };
        let clock = s.rig.world.app.app.clock.clone();
        let access = orch_api_core::thread_access::AuthzAccess::new(clock.clone());
        assert!(
            authority
                .current_issue_callers(&mut tx, &proposal, clock.as_ref(), &access)
                .await?
                .is_some()
        );
        let mut changed = row.body.clone();
        changed["agent_id"] = json!(s.on.agent);
        assert!(
            authority
                .current_issue_callers(
                    &mut tx,
                    &orch_db::queries::link::NewCommand {
                        body: &changed,
                        ..proposal
                    },
                    clock.as_ref(),
                    &access,
                )
                .await?
                .is_none()
        );
        assert!(
            authority
                .current_issue_callers(
                    &mut tx,
                    &orch_db::queries::link::NewCommand {
                        deadline_at: deadline.get() + chrono::Duration::seconds(1),
                        ..proposal
                    },
                    clock.as_ref(),
                    &access,
                )
                .await?
                .is_none()
        );
        tx.rollback().await?;
        match mode {
            "scope" => {
                narrow(
                    &s.rig.world,
                    &actor,
                    vec![rule(OP, "thread", thread, "self", "read")],
                )
                .await?
            }
            "expired" => s.rig.world.advance(chrono::Duration::hours(2)),
            "source" => {
                sqlx::query("UPDATE threads SET environment_id=$2 WHERE node_id=$1")
                    .bind(thread)
                    .bind(other)
                    .execute(s.rig.db.pool())
                    .await?;
            }
            _ => {}
        }
        s.rig.machine.handler.release();
        let reply = tokio::time::timeout(rig::PATIENCE, pending).await??;
        if mode == "live" {
            receipt(&reply)?;
            assert_eq!(bound_lines(&s, thread).await, 1);
        } else {
            assert!(
                matches!(reply.status.as_u16(), 401 | 403),
                "{}",
                String::from_utf8_lossy(&reply.raw)
            );
            assert_eq!(bound_lines(&s, thread).await, 0);
        }
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn rebind_completed_publication_requires_its_own_current_read_and_never_replays_manage()
-> Proof {
    for mode in ["live", "namespace", "manage", "read", "expired"] {
        let s = Scene::new().await;
        let thread = s.thread().await.base.id;
        let (agent, provider) = another_provider(&s, "Published binding", &["separated"]).await;
        let read = reads(
            &s,
            thread,
            agent,
            provider,
            if mode == "namespace" { "ThreadGet" } else { OP },
        );
        let mut grants = manages(thread);
        grants.extend(read.clone());
        let actor = key(&s.rig.world, s.ada.id(), grants).await?;
        let mut locked = s.rig.db.begin().await?;
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *locked)
            .await?;
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *locked)
            .await?;
        let app = s.rig.world.app.clone();
        let secret = actor.secret.clone();
        let body = request(agent, provider);
        let pending = tokio::spawn(async move {
            let call = app
                .call(ThreadBindingSet, &[&thread])
                .token(&secret)
                .body(&body);
            call.send_bytes().await
        });
        wait(&s, pid, &pending, true).await?;
        assert_eq!(bound_lines(&s, thread).await, 1);
        match mode {
            "manage" => narrow(&s.rig.world, &actor, read).await?,
            "read" => narrow(&s.rig.world, &actor, manages(thread)).await?,
            "expired" => s.rig.world.advance(chrono::Duration::hours(2)),
            _ => {}
        }
        locked.commit().await?;
        let reply = tokio::time::timeout(rig::PATIENCE, pending).await??;
        let value = decoded(&reply)?;
        assert_eq!(
            value.get("projection").is_none(),
            matches!(mode, "live" | "manage")
        );
        if matches!(mode, "live" | "manage") {
            let header: Thread = serde_json::from_value(value)?;
            assert_eq!(
                header.binding.ok_or("native binding missing")?.provider.id,
                provider
            );
        } else {
            receipt(&reply)?;
        }
        assert_eq!(bound_lines(&s, thread).await, 1);
    }
    Ok(())
}
