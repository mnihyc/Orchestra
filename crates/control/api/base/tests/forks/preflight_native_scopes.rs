//! The one managed act carries hidden members; only the same operation's Read opens its plan.
use super::*;
use orch_api_core::testing::RawReply;
use thread_history_scope::{Proof, key, narrow, rule};
use tracing::instrument::WithSubscriber as _;

fn operation(result: ForkResult) -> &'static str {
    if result == ForkResult::New {
        "ThreadForkPreflight.fork"
    } else {
        "ThreadForkPreflight.switch"
    }
}
fn manages(thread: Id, result: ForkResult) -> Vec<Json> {
    vec![rule(operation(result), "thread", thread, "self", "manage")]
}
fn reads(w: &Forks, thread: Id, _result: ForkResult, namespace: &str) -> Vec<Json> {
    [
        ("org", w.s.tree.org.base.id),
        ("project", w.s.tree.project.base.id),
        ("topic", w.s.tree.topic.base.id),
        ("thread", thread),
        ("agent", w.s.on.agent),
        ("provider", w.s.on.provider),
        ("environment", w.s.on.environment),
        ("agent", w.agent),
        ("provider", w.provider),
    ]
    .into_iter()
    .map(|(kind, id)| rule(namespace, kind, id, "self", "read"))
    .collect()
}
async fn preview(w: &Forks, thread: Id, secret: &str, body: &ForkRequest) -> RawReply {
    w.s.rig
        .world
        .app
        .call(ThreadForkPreflight, &[&thread])
        .token(secret)
        .body(body)
        .send_bytes()
        .await
}
fn value(reply: &RawReply) -> Proof<Json> {
    assert_eq!(
        reply.status.as_u16(),
        200,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    Ok(serde_json::from_slice(&reply.raw)?)
}
fn receipt(reply: &RawReply) -> Proof<Json> {
    let body = value(reply)?;
    assert_eq!(body["projection"], "receipt");
    assert_eq!(body["planned"], 1);
    assert_eq!(body.as_object().ok_or("receipt object missing")?.len(), 5);
    for field in [
        "source",
        "from",
        "to",
        "history",
        "files",
        "access",
        "blockers",
        "agent_access",
    ] {
        assert!(body.get(field).is_none(), "receipt disclosed {field}");
    }
    Ok(body)
}
async fn wait_for_lock(
    w: &Forks,
    pid: i32,
    pending: &tokio::task::JoinHandle<RawReply>,
    activity: bool,
) -> Proof {
    let statement = if activity {
        "%INSERT INTO api_token_activity%"
    } else {
        "SELECT t.node_id, t.topic_id, t.environment_id%"
    };
    tokio::time::timeout(rig::PATIENCE, async {
        loop {
            assert!(!pending.is_finished(), "the real native wait must be reached");
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query ILIKE $2)")
                .bind(pid).bind(statement).fetch_one(w.s.rig.db.pool()).await.unwrap();
            if waiting { break; }
            tokio::task::yield_now().await;
        }
    }).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn fork_and_switch_preflight_bind_only_their_actual_managed_source_and_keep_hidden_members_inside_the_act()
-> Proof {
    let w = Forks::new().await;
    let thread = w.thread_with_history("Private carried conversation").await;
    let other = w.s.thread_by(&w.s.ada, "Another act source").await.base.id;
    for request in [w.fork(), w.to_second()] {
        let op = operation(request.result);
        let before = w.count("SELECT count(*) FROM preflights").await;
        for grants in [
            vec![rule(op, "thread", thread, "self", "read")],
            vec![rule(op, "thread", thread, "self", "write")],
            manages(other, request.result),
            vec![rule("ThreadGet", "thread", thread, "self", "manage")],
        ] {
            let selected = key(&w.s.rig.world, w.s.ada.id(), grants).await?;
            let refused = preview(&w, thread, &selected.secret, &request).await;
            assert_eq!(
                refused.status.as_u16(),
                403,
                "{}",
                String::from_utf8_lossy(&refused.raw)
            );
        }
        assert_eq!(w.count("SELECT count(*) FROM preflights").await, before);
        // Neither source Read, a ledger/history Read, nor a destination component Write is supplied.
        let selected = key(
            &w.s.rig.world,
            w.s.ada.id(),
            manages(thread, request.result),
        )
        .await?;
        let accepted = receipt(&preview(&w, thread, &selected.secret, &request).await)?;
        let id: Id = serde_json::from_value(accepted["preflight_id"].clone())?;
        let saved = orch_db::queries::jobs::forks::preflight(&mut *w.s.rig.world.conn().await, id)
            .await?
            .ok_or("accepted native preflight missing")?;
        assert_eq!(saved.created_by, w.s.ada.id());
        assert_eq!(saved.node_id, Some(thread));
        assert_eq!(saved.plan["plan"]["source"]["id"], json!(thread));
        assert_eq!(
            saved.plan["request"]["result"],
            serde_json::to_value(request.result)?
        );
        assert_eq!(accepted["executable"], 1);
        assert_eq!(accepted["blocked"], 0);
    }
    assert_eq!(
        w.count("SELECT count(*) FROM jobs WHERE kind='thread_fork'")
            .await,
        0
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn preflight_keeps_native_destination_prerequisites_and_discloses_only_under_its_own_read()
-> Proof {
    let w = Forks::new().await;
    let thread = w
        .thread_with_history("Source kept private by the key")
        .await;
    let request = w.fork();
    let ordinary = w.plan(&w.s.cai, thread, &request).await.ok();
    assert!(
        !ordinary.blockers.is_empty(),
        "the human destination requires creation authority"
    );
    let selected = key(
        &w.s.rig.world,
        w.s.cai.id(),
        manages(thread, request.result),
    )
    .await?;
    let accepted = receipt(&preview(&w, thread, &selected.secret, &request).await)?;
    assert_eq!(accepted["blocked"], 1);
    assert_eq!(accepted["executable"], 0);
    let id: Id = serde_json::from_value(accepted["preflight_id"].clone())?;
    let row = orch_db::queries::jobs::forks::preflight(&mut *w.s.rig.world.conn().await, id)
        .await?
        .ok_or("blocked native plan missing")?;
    let kept: ForkPlan = serde_json::from_value(row.plan["plan"].clone())?;
    assert_eq!(blocker_codes(&kept), blocker_codes(&ordinary));
    for (namespace, full) in [("ThreadGet", false), (operation(request.result), true)] {
        let mut grants = manages(thread, request.result);
        grants.extend(reads(&w, thread, request.result, namespace));
        let selected = key(&w.s.rig.world, w.s.ada.id(), grants).await?;
        let body = value(&preview(&w, thread, &selected.secret, &request).await)?;
        assert_eq!(body.get("projection").is_none(), full);
        if full {
            let plan: ForkPlan = serde_json::from_value(body)?;
            assert_eq!(plan.source.id, thread);
            assert_eq!(plan.history.method, HistoryMethod::NativeFork);
        } else {
            assert_eq!(body["projection"], "receipt");
        }
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn preflight_rechecks_after_the_actual_source_row_wait_before_the_first_private_plan_insert()
-> Proof {
    for mode in ["live", "expired", "source"] {
        let w = Forks::new().await;
        let thread = w.thread_with_history("Waiting native source").await;
        w.s.rig.world.app.app.engine.stop();
        let other = another_environment(
            &w.s,
            "Changed original environment",
            Some(w.s.rig.machine_id),
        )
        .await;
        let selected = key(
            &w.s.rig.world,
            w.s.ada.id(),
            manages(thread, ForkResult::New),
        )
        .await?;
        let mut locked = w.s.rig.db.begin().await?;
        orch_db::queries::threads::lock(&mut locked, thread)
            .await?
            .ok_or("native source missing")?;
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *locked)
            .await?;
        let app = w.s.rig.world.app.clone();
        let secret = selected.secret.clone();
        let body = w.fork();
        // The change below is to land between the request's first check and its first write, so the case waits for
        // the request itself, not for any backend that waits on this transaction.
        let mut waited = source_wait::SourceWait::new("ThreadForkPreflight", mode);
        let recorder = waited.recorder();
        let pending = tokio::spawn(async move {
            let request = app
                .call(ThreadForkPreflight, &[&thread])
                .token(&secret)
                .body(&body);
            request.send_bytes().with_subscriber(recorder).await
        });
        waited.reached(&w, pid, &pending).await?;
        if mode == "expired" {
            w.s.rig.world.advance(chrono::Duration::hours(2));
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
        } else {
            assert!(
                matches!(reply.status.as_u16(), 401 | 403),
                "{}",
                waited.explain(&reply)
            );
            assert_eq!(
                w.count("SELECT count(*) FROM preflights").await,
                0,
                "{}",
                waited.explain(&reply)
            );
        }
        waited.finish();
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn preflight_native_manifest_retains_exact_command_and_source_through_the_real_supplier_wait()
-> Proof {
    for mode in ["live", "scope", "expired", "source"] {
        let w = Forks::new().await;
        let thread = w.thread_with_history("Carried native files").await;
        let world = &w.s.rig.world;
        let to = another_environment(&w.s, "Native target", Some(w.s.rig.machine_id)).await;
        let other =
            another_environment(&w.s, "Another native source", Some(w.s.rig.machine_id)).await;
        sqlx::query(
            "UPDATE environments SET workspace_root='/workspace',facts=jsonb_set(jsonb_set(facts,'{index}','\"live\"'),'{workspace_root}','\"/workspace\"') WHERE id=$1",
        )
        .bind(w.s.on.environment)
        .execute(w.s.rig.db.pool())
        .await?;
        let env = orch_db::queries::environments::thread_engine::environment(
            &mut *world.conn().await,
            w.s.on.environment,
        )
        .await?
        .ok_or("actual source environment missing")?;
        let root = "/workspace";
        let manifest = orch_engine::manifest::build(
            &orch_engine::manifest::Header {
                v: 1,
                root: root.to_owned(),
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
        let selected = key(world, w.s.ada.id(), manages(thread, ForkResult::Replace)).await?;
        let mut body = w.to_second();
        body.environment = Some(ForkEnvironment::Existing {
            environment_id: to,
            files: Some(orch_api_contract::forks::FileSelection::Selection {
                include: vec!["src".into()],
                exclude: None,
            }),
        });
        let app = world.app.clone();
        let secret = selected.secret.clone();
        let pending = tokio::spawn(async move {
            let request = app
                .call(ThreadForkPreflight, &[&thread])
                .token(&secret)
                .body(&body);
            request.send_bytes().await
        });
        eventually("the actual native manifest read", || async {
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
                .ok_or("native manifest command absent")?
                .id;
        let evidence =
            w.s.rig
                .commands
                .issued_authority(id)
                .ok_or("manifest has no private source")?;
        assert_eq!(serde_json::to_value(&evidence)?["kind"], "fork_preparation");
        let mut conn = world.db.pool.begin().await?;
        let canonical = orch_db::queries::link::command(&mut conn, id)
            .await?
            .ok_or("manifest journal missing")?;
        assert!(
            canonical.body.get("origin").is_none() && canonical.body.get("authority").is_none()
        );
        let clock = world.app.app.clock.clone();
        let access = orch_api_core::thread_access::AuthzAccess::new(clock.clone());
        assert!(
            evidence
                .current_callers(&mut conn, &canonical, clock.as_ref(), &access)
                .await?
                .is_some()
        );
        let mut changed = canonical.clone();
        changed.body["environment_id"] = json!(other);
        assert!(
            evidence
                .current_callers(&mut conn, &changed, clock.as_ref(), &access)
                .await?
                .is_none()
        );
        conn.rollback().await?;
        match mode {
            "scope" => {
                narrow(
                    world,
                    &selected,
                    vec![rule(
                        "ThreadForkPreflight.switch",
                        "thread",
                        thread,
                        "self",
                        "read",
                    )],
                )
                .await?
            }
            "expired" => world.advance(chrono::Duration::hours(2)),
            "source" => {
                sqlx::query("UPDATE threads SET environment_id=$2 WHERE node_id=$1")
                    .bind(thread)
                    .bind(other)
                    .execute(w.s.rig.db.pool())
                    .await?;
            }
            _ => {}
        }
        w.s.rig.machine.handler.release();
        let reply = tokio::time::timeout(rig::PATIENCE, pending).await??;
        if mode == "live" {
            receipt(&reply)?;
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
async fn accepted_preflight_publication_keeps_read_after_manage_loss_and_keeps_receipt_after_read_loss_or_expiry()
-> Proof {
    for mode in ["live", "manage", "read", "expired"] {
        let w = Forks::new().await;
        let thread = w.thread_with_history("Completed private preflight").await;
        let world = &w.s.rig.world;
        let read = reads(&w, thread, ForkResult::New, "ThreadForkPreflight.fork");
        let mut grants = manages(thread, ForkResult::New);
        grants.extend(read.clone());
        let selected = key(world, w.s.ada.id(), grants).await?;
        let mut blocked = w.s.rig.db.begin().await?;
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *blocked)
            .await?;
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocked)
            .await?;
        let app = world.app.clone();
        let secret = selected.secret.clone();
        let body = w.fork();
        let pending = tokio::spawn(async move {
            let request = app
                .call(ThreadForkPreflight, &[&thread])
                .token(&secret)
                .body(&body);
            request.send_bytes().await
        });
        wait_for_lock(&w, pid, &pending, true).await?;
        let stored = w.count("SELECT count(*) FROM preflights").await;
        if stored != 1 && std::env::var_os("ORCH_TEST_NATIVE_OPERATIONS").is_some() {
            blocked.commit().await?;
            let reply = tokio::time::timeout(rig::PATIENCE, pending).await??;
            eprintln!(
                "Native preflight publication stored_before={stored} status={} body={} stored_after={}",
                reply.status,
                String::from_utf8_lossy(&reply.raw),
                w.count("SELECT count(*) FROM preflights").await
            );
            panic!("the native preflight was not stored before its publication wait");
        }
        assert_eq!(stored, 1);
        match mode {
            "manage" => narrow(world, &selected, read).await?,
            "read" => narrow(world, &selected, manages(thread, ForkResult::New)).await?,
            "expired" => world.advance(chrono::Duration::hours(2)),
            _ => {}
        }
        blocked.commit().await?;
        let reply = tokio::time::timeout(rig::PATIENCE, pending).await??;
        let body = value(&reply)?;
        assert_eq!(
            body.get("projection").is_none(),
            matches!(mode, "live" | "manage")
        );
        if matches!(mode, "read" | "expired") {
            receipt(&reply)?;
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
