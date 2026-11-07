//! Actual native confirmations, source waits, deferred startup and current full/receipt publication.
use super::*;
use orch_api_core::testing::RawReply;
use thread_history_scope::{Proof, key, narrow, rule};
const SINGLE: &str = "ThreadForkCreate.switch";
const BATCH: &str = "ForkBatchCreate.switch";
fn manages(op: &str, ids: &[Id]) -> Vec<Json> {
    ids.iter()
        .map(|&id| rule(op, "thread", id, "self", "manage"))
        .collect()
}
fn reads(w: &Forks, op: &str, ids: &[Id]) -> Vec<Json> {
    let mut rules = ids
        .iter()
        .map(|&id| rule(op, "thread", id, "self", "read"))
        .collect::<Vec<_>>();
    rules.extend(
        [
            ("org", w.s.tree.org.base.id),
            ("project", w.s.tree.project.base.id),
            ("topic", w.s.tree.topic.base.id),
            ("environment", w.s.on.environment),
            ("provider", w.s.on.provider),
            ("provider", w.provider),
        ]
        .into_iter()
        .map(|(kind, id)| rule(op, kind, id, "self", "read")),
    );
    rules
}
async fn single(w: &Forks, thread: Id, plan: Id, secret: &str) -> RawReply {
    w.s.rig
        .world
        .app
        .call(ThreadForkCreate, &[&thread])
        .token(secret)
        .body(&ForkCreateBody {
            preflight_id: plan,
            notify: None,
            options: None,
        })
        .send_bytes()
        .await
}
async fn batch(w: &Forks, plan: Id, secret: &str) -> RawReply {
    w.s.rig
        .world
        .app
        .call(ForkBatchCreate, &[])
        .token(secret)
        .body(&ForkCreateBody {
            preflight_id: plan,
            notify: None,
            options: None,
        })
        .send_bytes()
        .await
}
fn value(reply: &RawReply) -> Proof<Json> {
    assert_eq!(
        reply.status.as_u16(),
        202,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    Ok(serde_json::from_slice(&reply.raw)?)
}
fn single_receipt(reply: &RawReply) -> Proof<Id> {
    let body = value(reply)?;
    assert_eq!(body["projection"], "receipt");
    assert_eq!(body.as_object().ok_or("receipt missing")?.len(), 2);
    Ok(serde_json::from_value(body["job_id"].clone())?)
}
fn batch_receipt(reply: &RawReply, queued: usize) -> Proof<Id> {
    let body = value(reply)?;
    assert_eq!(body["projection"], "receipt");
    assert_eq!(body["queued"], queued);
    assert_eq!(body.as_object().ok_or("receipt missing")?.len(), 3);
    assert!(body.get("jobs").is_none() && body.get("members").is_none());
    Ok(serde_json::from_value(body["batch_id"].clone())?)
}
async fn wait(
    w: &Forks,
    pid: i32,
    pending: &tokio::task::JoinHandle<RawReply>,
    activity: bool,
) -> Proof {
    tokio::time::timeout(rig::PATIENCE,async {loop {
        assert!(!pending.is_finished(),"the actual native lock must be reached");
        let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND (NOT $2 OR query ILIKE '%INSERT INTO api_token_activity%'))")
            .bind(pid).bind(activity).fetch_one(w.s.rig.db.pool()).await.unwrap();
        if blocked {break;}tokio::task::yield_now().await;
    }}).await?;
    Ok(())
}
#[tokio::test(flavor = "multi_thread")]
async fn single_confirmation_binds_the_kept_native_variant_and_source_without_carried_read_or_component_write()
-> Proof {
    for new in [false, true] {
        let w = Forks::new().await;
        let thread = w.s.thread_by(&w.s.ada, "Confirmed source").await.base.id;
        let other =
            w.s.thread_by(&w.s.ada, "Different confirmed source")
                .await
                .base
                .id;
        let mut request = w.to_second();
        if new {
            request.result = ForkResult::New;
            request.name = Some("Confirmed child".into());
        }
        let plan = w.plan(&w.s.ada, thread, &request).await.ok();
        assert!(plan.blockers.is_empty());
        w.s.rig.world.app.app.engine.stop();
        let op = if new { "ThreadForkCreate.fork" } else { SINGLE };
        for grants in [
            vec![rule(op, "thread", thread, "self", "read")],
            manages(op, &[other]),
            manages(
                if new { SINGLE } else { "ThreadForkCreate.fork" },
                &[thread],
            ),
            manages("ThreadForkNow", &[thread]),
        ] {
            let actor = key(&w.s.rig.world, w.s.ada.id(), grants).await?;
            assert_eq!(
                single(&w, thread, plan.preflight_id, &actor.secret)
                    .await
                    .status
                    .as_u16(),
                403
            );
        }
        assert_eq!(
            w.count("SELECT count(*) FROM jobs WHERE kind='thread_fork'")
                .await,
            0
        );
        let actor = key(&w.s.rig.world, w.s.ada.id(), manages(op, &[thread])).await?;
        sqlx::query("UPDATE agents SET disabled=true WHERE id=$1")
            .bind(w.agent)
            .execute(w.s.rig.db.pool())
            .await?;
        assert!(
            !single(&w, thread, plan.preflight_id, &actor.secret)
                .await
                .status
                .is_success()
        );
        assert_eq!(
            w.count("SELECT count(*) FROM jobs WHERE kind='thread_fork'")
                .await,
            0
        );
        sqlx::query("UPDATE agents SET disabled=false WHERE id=$1")
            .bind(w.agent)
            .execute(w.s.rig.db.pool())
            .await?;
        let id = single_receipt(&single(&w, thread, plan.preflight_id, &actor.secret).await)?;
        let row = orch_db::queries::jobs::job(&mut *w.s.rig.world.conn().await, id)
            .await?
            .ok_or("native job missing")?;
        assert_eq!(row.node_id, Some(thread));
        assert_eq!(
            row.plan["confirmed_history_source"]["operation"]["origin"]["operation"],
            op
        );
        assert_eq!(
            row.plan["confirmed_history_source"]["operation"]["credential"]["kind"],
            "token"
        );
        assert!(row.plan.get("new_thread").is_some_and(|id| !id.is_null()) == new);
        assert!(
            orch_db::queries::jobs::forks::preflight(
                &mut *w.s.rig.world.conn().await,
                plan.preflight_id
            )
            .await?
            .is_none()
        );
    }
    Ok(())
}
#[tokio::test(flavor = "multi_thread")]
async fn batch_confirmation_binds_all_actual_sources_and_preserves_native_ordinary_counts() -> Proof
{
    let w = Forks::new().await;
    let a =
        w.s.thread_by(&w.s.ada, "First confirmation source")
            .await
            .base
            .id;
    let b =
        w.s.thread_by(&w.s.ada, "Second confirmation source")
            .await
            .base
            .id;
    let plan = w
        .batch_plan(&w.s.ada, &w.batch_of(vec![b, a, a, orch_types::new_id()]))
        .await
        .ok();
    assert_eq!(plan.plans.len(), 2);
    assert_eq!(plan.executable, 2);
    assert_eq!(plan.skipped.len(), 1);
    w.s.rig.world.app.app.engine.stop();
    for grants in [
        manages(BATCH, &[a]),
        vec![
            rule(BATCH, "thread", a, "self", "read"),
            rule(BATCH, "thread", b, "self", "read"),
        ],
        manages(SINGLE, &[a, b]),
    ] {
        let actor = key(&w.s.rig.world, w.s.ada.id(), grants).await?;
        assert_eq!(
            batch(&w, plan.preflight_id, &actor.secret)
                .await
                .status
                .as_u16(),
            403
        );
    }
    assert_eq!(
        w.count("SELECT count(*) FROM jobs WHERE kind='thread_fork'")
            .await,
        0
    );
    let actor = key(&w.s.rig.world, w.s.ada.id(), manages(BATCH, &[a, b])).await?;
    let id = batch_receipt(&batch(&w, plan.preflight_id, &actor.secret).await, 2)?;
    let rows =
        orch_db::queries::jobs::forks::of_batch(&mut *w.s.rig.world.conn().await, id).await?;
    assert_eq!(rows.len(), 2);
    for row in rows {
        assert_eq!(
            row.plan["confirmed_history_source"]["operation"]["origin"]["operation"],
            BATCH
        );
        assert!(orch_engine::jobs::fork::operation::Preparation::from_job(&row)?.is_some());
    }
    assert!(
        orch_db::queries::jobs::forks::preflight(
            &mut *w.s.rig.world.conn().await,
            plan.preflight_id
        )
        .await?
        .is_none()
    );
    Ok(())
}
#[tokio::test(flavor = "multi_thread")]
async fn confirmation_source_waits_cut_both_namespaces_before_the_first_native_job_insert() -> Proof
{
    for is_batch in [false, true] {
        for mode in ["live", "scope", "expired", "source"] {
            let w = Forks::new().await;
            let a =
                w.s.thread_by(&w.s.ada, "Waiting confirmed source")
                    .await
                    .base
                    .id;
            let b =
                w.s.thread_by(&w.s.ada, "Second waiting confirmed source")
                    .await
                    .base
                    .id;
            let other =
                another_environment(&w.s, "Changed confirmed source", Some(w.s.rig.machine_id))
                    .await;
            let preflight = if is_batch {
                w.batch_plan(&w.s.ada, &w.batch_of(vec![a, b]))
                    .await
                    .ok()
                    .preflight_id
            } else {
                w.plan(&w.s.ada, a, &w.to_second()).await.ok().preflight_id
            };
            w.s.rig.world.app.app.engine.stop();
            let op = if is_batch { BATCH } else { SINGLE };
            let ids = if is_batch { vec![a, b] } else { vec![a] };
            let actor = key(&w.s.rig.world, w.s.ada.id(), manages(op, &ids)).await?;
            let mut locked = w.s.rig.db.begin().await?;
            orch_db::queries::threads::lock(&mut locked, a)
                .await?
                .ok_or("source missing")?;
            let pid = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *locked)
                .await?;
            let app = w.s.rig.world.app.clone();
            let secret = actor.secret.clone();
            let pending = tokio::spawn(async move {
                let body = ForkCreateBody {
                    preflight_id: preflight,
                    notify: None,
                    options: None,
                };
                if is_batch {
                    let call = app.call(ForkBatchCreate, &[]).token(&secret).body(&body);
                    call.send_bytes().await
                } else {
                    let call = app.call(ThreadForkCreate, &[&a]).token(&secret).body(&body);
                    call.send_bytes().await
                }
            });
            wait(&w, pid, &pending, false).await?;
            if mode == "scope" {
                narrow(
                    &w.s.rig.world,
                    &actor,
                    ids.iter()
                        .map(|&id| rule(op, "thread", id, "self", "read"))
                        .collect(),
                )
                .await?;
            }
            if mode == "expired" {
                w.s.rig.world.advance(chrono::Duration::hours(2));
            }
            if mode == "source" {
                sqlx::query("UPDATE threads SET environment_id=$2 WHERE node_id=$1")
                    .bind(a)
                    .bind(other)
                    .execute(&mut *locked)
                    .await?;
            }
            locked.commit().await?;
            let reply = tokio::time::timeout(rig::PATIENCE, pending).await??;
            if is_batch && mode == "source" {
                let id = batch_receipt(&reply, 1)?;
                let rows =
                    orch_db::queries::jobs::forks::of_batch(&mut *w.s.rig.world.conn().await, id)
                        .await?;
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0].node_id, Some(b));
            } else if mode == "live" {
                if is_batch {
                    batch_receipt(&reply, 2)?;
                } else {
                    single_receipt(&reply)?;
                }
            } else {
                assert!(
                    !reply.status.is_success(),
                    "{}",
                    String::from_utf8_lossy(&reply.raw)
                );
                assert_eq!(
                    w.count("SELECT count(*) FROM jobs WHERE kind='thread_fork'")
                        .await,
                    0
                );
            }
        }
    }
    Ok(())
}
#[tokio::test(flavor = "multi_thread")]
async fn completed_confirmations_keep_current_own_read_after_manage_loss_and_receipts_after_disclosure_closes()
-> Proof {
    for is_batch in [false, true] {
        for mode in ["live", "namespace", "manage", "read", "expired"] {
            let w = Forks::new().await;
            let a =
                w.s.thread_by(&w.s.ada, "Published confirmation source")
                    .await
                    .base
                    .id;
            let b =
                w.s.thread_by(&w.s.ada, "Second published confirmation source")
                    .await
                    .base
                    .id;
            let preflight = if is_batch {
                w.batch_plan(&w.s.ada, &w.batch_of(vec![a, b]))
                    .await
                    .ok()
                    .preflight_id
            } else {
                w.plan(&w.s.ada, a, &w.to_second()).await.ok().preflight_id
            };
            w.s.rig.world.app.app.engine.stop();
            let ids = if is_batch { vec![a, b] } else { vec![a] };
            let op = if is_batch { BATCH } else { SINGLE };
            let read = reads(&w, if mode == "namespace" { "JobGet" } else { op }, &ids);
            let mut grants = manages(op, &ids);
            grants.extend(read.clone());
            let actor = key(&w.s.rig.world, w.s.ada.id(), grants).await?;
            let mut locked = w.s.rig.db.begin().await?;
            sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
                .execute(&mut *locked)
                .await?;
            let pid = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *locked)
                .await?;
            let app = w.s.rig.world.app.clone();
            let secret = actor.secret.clone();
            let pending = tokio::spawn(async move {
                let body = ForkCreateBody {
                    preflight_id: preflight,
                    notify: None,
                    options: None,
                };
                if is_batch {
                    let call = app.call(ForkBatchCreate, &[]).token(&secret).body(&body);
                    call.send_bytes().await
                } else {
                    let call = app.call(ThreadForkCreate, &[&a]).token(&secret).body(&body);
                    call.send_bytes().await
                }
            });
            wait(&w, pid, &pending, true).await?;
            assert_eq!(
                w.count("SELECT count(*) FROM jobs WHERE kind='thread_fork'")
                    .await,
                if is_batch { 2 } else { 1 }
            );
            match mode {
                "manage" => narrow(&w.s.rig.world, &actor, read).await?,
                "read" => narrow(&w.s.rig.world, &actor, manages(op, &ids)).await?,
                "expired" => w.s.rig.world.advance(chrono::Duration::hours(2)),
                _ => {}
            }
            locked.commit().await?;
            let reply = tokio::time::timeout(rig::PATIENCE, pending).await??;
            let body = value(&reply)?;
            assert_eq!(
                body.get("projection").is_none(),
                matches!(mode, "live" | "manage"),
                "{body}"
            );
            if matches!(mode, "live" | "manage") {
                if is_batch {
                    let full: ForkBatch = serde_json::from_value(body)?;
                    assert_eq!(full.jobs.len(), 2);
                } else {
                    let full: Job = serde_json::from_value(body)?;
                    assert_eq!(full.node_id, Some(a));
                }
            } else if is_batch {
                batch_receipt(&reply, 2)?;
            } else {
                single_receipt(&reply)?;
            }
        }
    }
    Ok(())
}
#[tokio::test(flavor = "multi_thread")]
async fn whole_environment_confirmation_carries_hidden_native_members_under_one_environment_manage()
-> Proof {
    let w = Forks::new().await;
    let hidden =
        w.s.thread_by(&w.s.ada, "Hidden confirmed member")
            .await
            .base
            .id;
    w.s.rig
        .world
        .app
        .call(
            orch_api_contract::access::PermissionPageUpdate,
            &[&"nodes", &hidden],
        )
        .token(&w.s.ada.token)
        .body(&orch_api_contract::access::PageChange {
            whole: Some(orch_api_contract::access::PageChangeWhole::Private),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert!(
        !w.s.rig
            .world
            .app
            .call(NodeGet, &[&hidden])
            .token(&w.s.eve.token)
            .send_bytes()
            .await
            .status
            .is_success()
    );
    let target = another_environment(
        &w.s,
        "Confirmed whole environment target",
        Some(w.s.rig.machine_id),
    )
    .await;
    let body = ForkBatchPreflightBody {
        all_of_environment: Some(w.s.on.environment),
        environment: Some(ForkEnvironment::Existing {
            environment_id: target,
            files: Some(orch_api_contract::forks::FileSelection::Nothing),
        }),
        ..Default::default()
    };
    let plan = w.batch_plan(&w.s.eve, &body).await.ok();
    assert_eq!(plan.plans.len(), 1);
    assert!(plan.plans[0].source.redacted);
    w.s.rig.world.app.app.engine.stop();
    let wrong = key(&w.s.rig.world, w.s.eve.id(), manages(BATCH, &[hidden])).await?;
    assert_eq!(
        batch(&w, plan.preflight_id, &wrong.secret)
            .await
            .status
            .as_u16(),
        403
    );
    let actor = key(
        &w.s.rig.world,
        w.s.eve.id(),
        vec![rule(
            BATCH,
            "environment",
            w.s.on.environment,
            "self",
            "manage",
        )],
    )
    .await?;
    let id = batch_receipt(&batch(&w, plan.preflight_id, &actor.secret).await, 1)?;
    let rows =
        orch_db::queries::jobs::forks::of_batch(&mut *w.s.rig.world.conn().await, id).await?;
    assert_eq!(rows[0].node_id, Some(hidden));
    assert_eq!(
        rows[0].plan["confirmed_history_source"]["operation"]["origin"]["operation"],
        BATCH
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn deferred_confirmations_keep_their_actual_namespace_and_stop_new_work_after_native_startup_scope_loss()
-> Proof {
    for is_batch in [false, true] {
        for revoked in [false, true] {
            let w = Forks::new().await;
            let a =
                w.s.thread_by(&w.s.ada, "Deferred confirmed source")
                    .await
                    .base
                    .id;
            let preflight = if is_batch {
                w.batch_plan(&w.s.ada, &w.batch_of(vec![a]))
                    .await
                    .ok()
                    .preflight_id
            } else {
                w.plan(&w.s.ada, a, &w.to_second()).await.ok().preflight_id
            };
            sqlx::query("UPDATE harnesses SET status='error',start_command_id=NULL WHERE provider_id=$1 AND agent_id=$2")
            .bind(w.provider).bind(w.agent).execute(w.s.rig.db.pool()).await?;
            let before =
                w.s.rig
                    .machine
                    .handler
                    .executions()
                    .iter()
                    .map(|run| run.id)
                    .collect();
            w.s.rig.machine.handler.hold(CommandKind::AgentStart);
            let op = if is_batch { BATCH } else { SINGLE };
            let actor = key(&w.s.rig.world, w.s.ada.id(), manages(op, &[a])).await?;
            let job = if is_batch {
                let id = batch_receipt(&batch(&w, preflight, &actor.secret).await, 1)?;
                orch_db::queries::jobs::forks::of_batch(&mut *w.s.rig.world.conn().await, id)
                    .await?
                    .first()
                    .ok_or("batch member missing")?
                    .id
            } else {
                single_receipt(&single(&w, a, preflight, &actor.secret).await)?
            };
            let command =
                super::immediate_native_scopes::new_native(&w, CommandKind::AgentStart, &before)
                    .await?;
            let authority =
                w.s.rig
                    .commands
                    .issued_authority(command)
                    .ok_or("native source missing")?;
            let private = serde_json::to_value(&authority)?;
            assert_eq!(private["kind"], "fork_native");
            assert_eq!(
                private["authority"]["source"]["origin"]["origin"]["operation"],
                op
            );
            let row = orch_db::queries::link::command(&mut *w.s.rig.world.conn().await, command)
                .await?
                .ok_or("native command missing")?;
            let clock = w.s.rig.world.app.app.clock.clone();
            let access = orch_api_core::thread_access::AuthzAccess::new(clock.clone());
            let mut tx = w.s.rig.db.begin().await?;
            assert!(
                authority
                    .current_callers(&mut tx, &row, clock.as_ref(), &access)
                    .await?
                    .is_some()
            );
            let mut changed = row.clone();
            changed.machine_id = orch_types::new_id();
            assert!(
                authority
                    .current_callers(&mut tx, &changed, clock.as_ref(), &access)
                    .await?
                    .is_none()
            );
            let mut changed = row.clone();
            changed.deadline_at += chrono::Duration::seconds(1);
            assert!(
                authority
                    .current_callers(&mut tx, &changed, clock.as_ref(), &access)
                    .await?
                    .is_none()
            );
            tx.rollback().await?;
            if revoked {
                narrow(
                    &w.s.rig.world,
                    &actor,
                    vec![rule(op, "thread", a, "self", "read")],
                )
                .await?;
            }
            w.s.rig.machine.handler.release();
            let done = w.ended(&w.s.ada, job).await;
            assert_eq!(done.state == JobState::Succeeded, !revoked, "{done:?}");
            assert_eq!(
                w.count("SELECT count(*) FROM jobs WHERE kind='thread_fork'")
                    .await,
                1
            );
        }
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn new_environment_confirmations_retain_their_real_make_child_before_a_target_thread_exists()
-> Proof {
    for is_batch in [false, true] {
        let w = Forks::new().await;
        let a =
            w.s.thread_by(&w.s.ada, "Making confirmed target")
                .await
                .base
                .id;
        let (host, template) = creator_support::managed(&w.s).await;
        let new = orch_api_contract::forks::ForkEnvironment::New {
            new: orch_api_contract::forks::ForkNewEnvironment {
                host_id: Some(Some(host)),
                template_id: Some(template),
                files: Some(orch_api_contract::forks::FileSelection::Nothing),
                ..Default::default()
            },
        };
        let preflight = if is_batch {
            let mut request = w.batch_of(vec![a]);
            request.environment = Some(new);
            let plan = w.batch_plan(&w.s.ada, &request).await.ok();
            assert_eq!(plan.executable, 1, "{plan:?}");
            plan.preflight_id
        } else {
            let mut request = w.fork();
            request.environment = Some(new);
            let plan = w.plan(&w.s.ada, a, &request).await.ok();
            assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
            plan.preflight_id
        };
        let op = if is_batch {
            BATCH
        } else {
            "ThreadForkCreate.fork"
        };
        let actor = key(&w.s.rig.world, w.s.ada.id(), manages(op, &[a])).await?;
        let job = if is_batch {
            let id = batch_receipt(&batch(&w, preflight, &actor.secret).await, 1)?;
            orch_db::queries::jobs::forks::of_batch(&mut *w.s.rig.world.conn().await, id)
                .await?
                .first()
                .ok_or("parent missing")?
                .id
        } else {
            single_receipt(&single(&w, a, preflight, &actor.secret).await)?
        };
        let (environment,child)=tokio::time::timeout(rig::PATIENCE,async {loop {
            let found:Option<(Id,Id)>=sqlx::query_as("SELECT e.id,c.id FROM jobs p JOIN environments e ON e.id=(p.plan->>'new_environment')::uuid JOIN jobs c ON c.environment_id=e.id AND c.kind='environment_provision' WHERE p.id=$1 AND c.plan ? 'creator_provision'")
                .bind(job).fetch_optional(w.s.rig.db.pool()).await.unwrap();
            if let Some(found)=found {break found;}
            tokio::task::yield_now().await;
        }}).await?;
        let child = orch_db::queries::jobs::job(&mut *w.s.rig.world.conn().await, child)
            .await?
            .ok_or("native make child missing")?;
        assert_eq!(child.plan["mode"], "make");
        assert_eq!(child.plan["creator_provision"]["origin"]["operation"], op);
        assert_eq!(child.plan["creator_provision"]["fork"]["job"], json!(job));
        assert_eq!(
            child.plan["creator_provision"]["environment"],
            json!(environment)
        );
        assert!(child.plan["creator_provision"].get("thread").is_none());
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM threads WHERE environment_id=$1")
            .bind(environment)
            .fetch_one(w.s.rig.db.pool())
            .await?;
        assert_eq!(
            count, 0,
            "the make source must precede the new target binding"
        );
        let clock = w.s.rig.world.app.app.clock.clone();
        let access = orch_api_core::thread_access::AuthzAccess::new(clock.clone());
        let mut tx = w.s.rig.db.begin().await?;
        let current = orch_engine::jobs::provision::creator_checkpoint(
            &mut tx,
            &child,
            clock.as_ref(),
            &access,
        )
        .await?
        .ok_or("native make source missing")?;
        current.check(clock.as_ref())?;
        tx.rollback().await?;
        narrow(
            &w.s.rig.world,
            &actor,
            vec![rule(op, "thread", a, "self", "read")],
        )
        .await?;
        let mut tx = w.s.rig.db.begin().await?;
        assert!(matches!(
            orch_engine::jobs::provision::creator_checkpoint(
                &mut tx,
                &child,
                clock.as_ref(),
                &access
            )
            .await,
            Err(orch_engine::EngineError::BeforeEffect(_))
        ));
        tx.rollback().await?;
        w.s.rig.world.app.app.engine.stop();
    }
    Ok(())
}
