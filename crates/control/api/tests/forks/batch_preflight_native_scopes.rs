//! Batch planning binds actual native governors before any supplier and keeps member disclosure separate.
use super::*;
use orch_api_core::testing::RawReply;
use thread_history_scope::{Proof, key, narrow, rule};
const OP: &str = "ForkBatchPreflight.switch";
fn manages(threads: &[Id]) -> Vec<Json> {
    threads
        .iter()
        .map(|id| rule(OP, "thread", *id, "self", "manage"))
        .collect()
}
fn reads(w: &Forks, threads: &[Id], namespace: &str) -> Vec<Json> {
    let mut result = threads
        .iter()
        .map(|id| rule(namespace, "thread", *id, "self", "read"))
        .collect::<Vec<_>>();
    result.extend(
        [
            ("org", w.s.tree.org.base.id),
            ("project", w.s.tree.project.base.id),
            ("topic", w.s.tree.topic.base.id),
            ("agent", w.s.on.agent),
            ("provider", w.s.on.provider),
            ("environment", w.s.on.environment),
            ("agent", w.agent),
            ("provider", w.provider),
        ]
        .into_iter()
        .map(|(kind, id)| rule(namespace, kind, id, "self", "read")),
    );
    result
}
async fn preview(w: &Forks, secret: &str, body: &ForkBatchPreflightBody) -> RawReply {
    w.s.rig
        .world
        .app
        .call(ForkBatchPreflight, &[])
        .token(secret)
        .body(body)
        .send_bytes()
        .await
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
fn receipt(reply: &RawReply) -> Proof<Json> {
    let value = body(reply)?;
    assert_eq!(value["projection"], "receipt");
    assert_eq!(value.as_object().ok_or("receipt object missing")?.len(), 6);
    for field in [
        "plans",
        "members",
        "to",
        "notification",
        "history",
        "files",
        "source",
    ] {
        assert!(
            value.get(field).is_none(),
            "batch receipt disclosed {field}"
        );
    }
    Ok(value)
}
async fn wait(
    w: &Forks,
    pid: i32,
    pending: &tokio::task::JoinHandle<RawReply>,
    activity: bool,
) -> Proof {
    tokio::time::timeout(rig::PATIENCE, async {
        loop {
            assert!(!pending.is_finished(), "the actual native wait must be reached");
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND (NOT $2 OR query ILIKE '%INSERT INTO api_token_activity%'))")
                .bind(pid).bind(activity).fetch_one(w.s.rig.db.pool()).await.unwrap();
            if blocked { break; }
            tokio::task::yield_now().await;
        }
    }).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn batch_preflight_binds_every_selected_thread_keeps_native_skips_and_needs_no_carried_read_or_destination_write()
-> Proof {
    let w = Forks::new().await;
    let a =
        w.s.thread_by(&w.s.ada, "First native switch source")
            .await
            .base
            .id;
    let b =
        w.s.thread_by(&w.s.ada, "Second native switch source")
            .await
            .base
            .id;
    let other =
        w.s.thread_by(&w.s.ada, "Not in this native batch")
            .await
            .base
            .id;
    let request = w.batch_of(vec![a, b, a, orch_types::new_id()]);
    for grants in [
        manages(&[a]),
        manages(&[other]),
        vec![
            rule(OP, "thread", a, "self", "read"),
            rule(OP, "thread", b, "self", "read"),
        ],
        vec![
            rule("ThreadForkPreflight.switch", "thread", a, "self", "manage"),
            rule("ThreadForkPreflight.switch", "thread", b, "self", "manage"),
        ],
    ] {
        let selected = key(&w.s.rig.world, w.s.ada.id(), grants).await?;
        let refused = preview(&w, &selected.secret, &request).await;
        assert_eq!(
            refused.status.as_u16(),
            403,
            "{}",
            String::from_utf8_lossy(&refused.raw)
        );
    }
    assert_eq!(w.count("SELECT count(*) FROM preflights").await, 0);
    let selected = key(&w.s.rig.world, w.s.ada.id(), manages(&[a, b])).await?;
    let made = receipt(&preview(&w, &selected.secret, &request).await)?;
    assert_eq!(made["planned"], 2);
    assert_eq!(made["executable"], 2);
    assert_eq!(made["blocked"], 0);
    assert_eq!(made["skipped"], 1);
    let id: Id = serde_json::from_value(made["preflight_id"].clone())?;
    let saved = orch_db::queries::jobs::forks::preflight(&mut *w.s.rig.world.conn().await, id)
        .await?
        .ok_or("native aggregate missing")?;
    let stored: orch_engine::fork_plan::StoredBatch = serde_json::from_value(saved.plan)?;
    assert_eq!(
        stored
            .members
            .iter()
            .map(|member| member.thread)
            .collect::<Vec<_>>(),
        vec![a, b]
    );
    assert_eq!(stored.plan.skipped.len(), 1);
    assert_eq!(
        w.count("SELECT count(*) FROM jobs WHERE kind='thread_fork'")
            .await,
        0
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn environment_batch_preflight_uses_the_native_environment_governor_including_hidden_and_empty_members()
-> Proof {
    let w = Forks::new().await;
    let hidden =
        w.s.thread_by(&w.s.ada, "Hidden whole-environment member")
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
    let to = another_environment(
        &w.s,
        "Whole environment destination",
        Some(w.s.rig.machine_id),
    )
    .await;
    let request = ForkBatchPreflightBody {
        all_of_environment: Some(w.s.on.environment),
        environment: Some(ForkEnvironment::Existing {
            environment_id: to,
            files: None,
        }),
        ..Default::default()
    };
    let ordinary = w.batch_plan(&w.s.eve, &request).await.ok();
    assert_eq!(ordinary.plans.len(), 1);
    assert!(ordinary.plans[0].source.redacted);
    assert!(ordinary.plans[0].source.name.is_none());
    for grants in [
        manages(&[hidden]),
        vec![rule(OP, "environment", to, "self", "manage")],
    ] {
        let selected = key(&w.s.rig.world, w.s.eve.id(), grants).await?;
        assert_eq!(
            preview(&w, &selected.secret, &request)
                .await
                .status
                .as_u16(),
            403
        );
    }
    let selected = key(
        &w.s.rig.world,
        w.s.eve.id(),
        vec![rule(
            OP,
            "environment",
            w.s.on.environment,
            "self",
            "manage",
        )],
    )
    .await?;
    let made = receipt(&preview(&w, &selected.secret, &request).await)?;
    assert_eq!(made["planned"], 1);
    assert_eq!(made["executable"], ordinary.executable);
    let empty = ForkBatchPreflightBody {
        all_of_environment: Some(to),
        environment: Some(ForkEnvironment::Existing {
            environment_id: w.s.on.environment,
            files: None,
        }),
        ..Default::default()
    };
    let selected = key(
        &w.s.rig.world,
        w.s.ada.id(),
        vec![rule(OP, "environment", to, "self", "manage")],
    )
    .await?;
    let made = receipt(&preview(&w, &selected.secret, &empty).await)?;
    assert_eq!(made["planned"], 0);
    assert_eq!(made["skipped"], 0);
    let outsider = key(
        &w.s.rig.world,
        w.s.cai.id(),
        vec![rule(
            OP,
            "environment",
            w.s.on.environment,
            "self",
            "manage",
        )],
    )
    .await?;
    assert!(
        !preview(&w, &outsider.secret, &request)
            .await
            .status
            .is_success()
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn batch_preflight_rechecks_lifetime_and_exact_members_after_the_real_source_row_wait()
-> Proof {
    for mode in ["live", "expired", "source"] {
        let w = Forks::new().await;
        let a =
            w.s.thread_by(&w.s.ada, "Waiting batch source")
                .await
                .base
                .id;
        let b =
            w.s.thread_by(&w.s.ada, "Other selected batch source")
                .await
                .base
                .id;
        let to = another_environment(
            &w.s,
            "Changed native batch source",
            Some(w.s.rig.machine_id),
        )
        .await;
        let selected = key(&w.s.rig.world, w.s.ada.id(), manages(&[a, b])).await?;
        let mut locked = w.s.rig.db.begin().await?;
        orch_db::queries::threads::lock(&mut locked, a)
            .await?
            .ok_or("native source missing")?;
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *locked)
            .await?;
        let app = w.s.rig.world.app.clone();
        let secret = selected.secret.clone();
        let request = w.batch_of(vec![a, b]);
        let pending = tokio::spawn(async move {
            let call = app
                .call(ForkBatchPreflight, &[])
                .token(&secret)
                .body(&request);
            call.send_bytes().await
        });
        wait(&w, pid, &pending, false).await?;
        if mode == "expired" {
            w.s.rig.world.advance(chrono::Duration::hours(2));
        }
        if mode == "source" {
            sqlx::query("UPDATE threads SET environment_id=$2 WHERE node_id=$1")
                .bind(a)
                .bind(to)
                .execute(&mut *locked)
                .await?;
        }
        locked.commit().await?;
        let reply = tokio::time::timeout(rig::PATIENCE, pending).await??;
        if mode == "live" {
            assert_eq!(receipt(&reply)?["planned"], 2);
        } else {
            assert!(
                matches!(reply.status.as_u16(), 401 | 403),
                "{}",
                String::from_utf8_lossy(&reply.raw)
            );
            assert_eq!(w.count("SELECT count(*) FROM preflights").await, 0);
        }
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn batch_preflight_supplier_rechecks_the_original_operation_before_the_next_selected_member()
-> Proof {
    for mode in ["live", "scope", "expired"] {
        let w = Forks::new().await;
        let a =
            w.s.thread_by(&w.s.ada, "First native file supplier")
                .await
                .base
                .id;
        let b =
            w.s.thread_by(&w.s.ada, "Second native file supplier")
                .await
                .base
                .id;
        let world = &w.s.rig.world;
        sqlx::query("UPDATE environments SET workspace_root='/workspace',facts=jsonb_set(jsonb_set(facts,'{index}','\"live\"'),'{workspace_root}','\"/workspace\"') WHERE id=$1")
            .bind(w.s.on.environment).execute(w.s.rig.db.pool()).await?;
        let env = orch_db::queries::environments::thread_engine::environment(
            &mut *world.conn().await,
            w.s.on.environment,
        )
        .await?
        .ok_or("source environment missing")?;
        let manifest = orch_engine::manifest::build(
            &orch_engine::manifest::Header {
                v: 1,
                root: "/workspace".into(),
                generated_at: orch_types::Time::new(world.now()),
                generation: i64::from(env.generation),
                case_sensitive: true,
            },
            vec![],
        )?;
        let stored = world
            .app
            .app
            .blobs
            .put_bytes(bytes::Bytes::from(manifest.bytes))
            .await?;
        orch_db::queries::storage::register_blob(
            &mut *world.conn().await,
            &stored.hash,
            stored.size,
            world.now(),
        )
        .await?;
        w.s.rig
            .machine
            .handler
            .on(CommandKind::FsManifest, move |_| {
                Ok(CommandResult::FsManifest(
                    orch_link_contract::commands::FsManifestResult {
                        manifest_blob: stored.hash,
                        file_count: 0,
                        total_bytes: 0,
                    },
                ))
            });
        w.s.rig
            .machine
            .handler
            .on(CommandKind::GitWorktreeCopyPlan, |_| {
                Ok(CommandResult::GitWorktreeCopyPlan(
                    orch_link_contract::source::GitWorktreeCopyPlanResult {
                        independent: false,
                        additional_files: 0,
                        additional_bytes: 0,
                    },
                ))
            });
        w.s.rig.machine.handler.hold(CommandKind::FsManifest);
        let to = another_environment(
            &w.s,
            "Native supplier destination",
            Some(w.s.rig.machine_id),
        )
        .await;
        let mut request = w.batch_of(vec![a, b]);
        request.environment = Some(ForkEnvironment::Existing {
            environment_id: to,
            files: Some(orch_api_contract::forks::FileSelection::Selection {
                include: vec!["src".into()],
                exclude: None,
            }),
        });
        let selected = key(world, w.s.ada.id(), manages(&[a, b])).await?;
        let app = world.app.clone();
        let secret = selected.secret.clone();
        let pending = tokio::spawn(async move {
            let call = app
                .call(ForkBatchPreflight, &[])
                .token(&secret)
                .body(&request);
            call.send_bytes().await
        });
        eventually("first native batch supplier", || async {
            assert!(!pending.is_finished());
            w.s.rig
                .machine
                .handler
                .executions()
                .iter()
                .any(|run| run.kind == CommandKind::FsManifest)
        })
        .await;
        let id =
            w.s.rig
                .machine
                .handler
                .executions()
                .iter()
                .find(|run| run.kind == CommandKind::FsManifest)
                .ok_or("native manifest absent")?
                .id;
        let authority =
            w.s.rig
                .commands
                .issued_authority(id)
                .ok_or("batch supplier lost its source")?;
        let private = serde_json::to_value(authority)?;
        assert_eq!(private["kind"], "fork_preparation");
        assert_eq!(private["authority"]["origin"]["origin"]["operation"], OP);
        if mode == "scope" {
            narrow(world, &selected, manages(&[a])).await?;
        }
        if mode == "expired" {
            world.advance(chrono::Duration::hours(2));
        }
        w.s.rig.machine.handler.release();
        let reply = tokio::time::timeout(rig::PATIENCE, pending).await??;
        if mode == "live" {
            assert_eq!(receipt(&reply)?["planned"], 2);
        } else {
            assert!(
                matches!(reply.status.as_u16(), 401 | 403),
                "{}",
                String::from_utf8_lossy(&reply.raw)
            );
            assert_eq!(w.count("SELECT count(*) FROM preflights").await, 0);
        }
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn batch_preflight_disclosure_and_completed_receipts_keep_their_own_read_without_replaying_manage()
-> Proof {
    for mode in ["live", "namespace", "manage", "read", "expired"] {
        let w = Forks::new().await;
        let a =
            w.s.thread_by(&w.s.ada, "First disclosed native member")
                .await
                .base
                .id;
        let b =
            w.s.thread_by(&w.s.ada, "Second disclosed native member")
                .await
                .base
                .id;
        let world = &w.s.rig.world;
        let read = reads(
            &w,
            &[a, b],
            if mode == "namespace" {
                "ForkBatchGet"
            } else {
                OP
            },
        );
        let mut grants = manages(&[a, b]);
        grants.extend(read.clone());
        let selected = key(world, w.s.ada.id(), grants).await?;
        let mut locked = w.s.rig.db.begin().await?;
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *locked)
            .await?;
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *locked)
            .await?;
        let app = world.app.clone();
        let secret = selected.secret.clone();
        let request = w.batch_of(vec![a, b]);
        let pending = tokio::spawn(async move {
            let call = app
                .call(ForkBatchPreflight, &[])
                .token(&secret)
                .body(&request);
            call.send_bytes().await
        });
        wait(&w, pid, &pending, true).await?;
        assert_eq!(w.count("SELECT count(*) FROM preflights").await, 1);
        match mode {
            "manage" => narrow(world, &selected, read).await?,
            "read" => narrow(world, &selected, manages(&[a, b])).await?,
            "expired" => world.advance(chrono::Duration::hours(2)),
            _ => {}
        }
        locked.commit().await?;
        let reply = tokio::time::timeout(rig::PATIENCE, pending).await??;
        let value = body(&reply)?;
        assert_eq!(
            value.get("projection").is_none(),
            matches!(mode, "live" | "manage")
        );
        if matches!(mode, "live" | "manage") {
            let plan: ForkBatchPlan = serde_json::from_value(value)?;
            assert_eq!(plan.plans.len(), 2);
        } else {
            assert_eq!(receipt(&reply)?["planned"], 2);
        }
        assert_eq!(w.count("SELECT count(*) FROM preflights").await, 1);
        assert_eq!(
            w.count("SELECT count(*) FROM jobs WHERE kind='thread_fork'")
                .await,
            0
        );
    }
    Ok(())
}
