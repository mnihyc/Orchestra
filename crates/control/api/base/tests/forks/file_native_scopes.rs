//! Real copy commands use the actual retained fork act, not a second file-read scope.
use super::*;
use orch_engine::manifest::{Entry, EntryKind, Header, build};
use orch_link_contract::commands::{FsMaterializeResult, FsSnapshotResult};
use thread_history_scope::{Proof, key, narrow, rule};
const OP: &str = "ThreadForkCreate.switch";

async fn held(w: &Forks, kind: CommandKind, before: &std::collections::BTreeSet<Id>) -> Proof<Id> {
    Ok(tokio::time::timeout(rig::PATIENCE, async {
        loop {
            if let Some(run) =
                w.s.rig
                    .machine
                    .handler
                    .executions()
                    .iter()
                    .find(|run| run.kind == kind && !before.contains(&run.id))
            {
                break run.id;
            }
            tokio::task::yield_now().await;
        }
    })
    .await?)
}
async fn copy_scene(w: &Forks, plan: &ForkPlan) -> Proof<Arc<std::sync::atomic::AtomicUsize>> {
    let content =
        w.s.rig
            .world
            .app
            .app
            .blobs
            .put_bytes(bytes::Bytes::from_static(b"kept file"))
            .await?;
    let manifest = build(
        &Header {
            v: 1,
            root: plan
                .files
                .as_ref()
                .ok_or("native file plan missing")?
                .path_map
                .from
                .clone(),
            generated_at: orch_types::Time::new(w.s.rig.world.now()),
            generation: 1,
            case_sensitive: true,
        },
        vec![Entry {
            path: "keep.txt".into(),
            kind: EntryKind::File,
            size: content.size,
            mtime_ns: 0,
            mode: 0o644,
            hash: Some(content.hash),
            link: None,
        }],
    )?;
    let archive =
        w.s.rig
            .world
            .app
            .app
            .blobs
            .put_bytes(bytes::Bytes::from(manifest.bytes))
            .await?;
    for blob in [content, archive] {
        orch_db::queries::storage::register_blob(
            &mut *w.s.rig.world.conn().await,
            &blob.hash,
            blob.size,
            w.s.rig.world.now(),
        )
        .await?;
    }
    w.s.rig
        .machine
        .handler
        .on(CommandKind::FsSnapshot, move |_| {
            Ok(CommandResult::FsSnapshot(FsSnapshotResult {
                manifest_blob: archive.hash,
                file_count: 1,
                total_bytes: 9,
                uploaded_bytes: 9,
                missing: 0,
            }))
        });
    let copied = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = copied.clone();
    w.s.rig
        .machine
        .handler
        .on(CommandKind::FsMaterialize, move |_| {
            count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(CommandResult::FsMaterialize(FsMaterializeResult {
                files: 1,
                bytes: 9,
                skipped: 0,
            }))
        });
    Ok(copied)
}
#[tokio::test(flavor = "multi_thread")]
async fn retained_native_copy_commands_keep_one_act_scope_exact_recipients_and_earliest_budget()
-> Proof {
    for mode in ["live", "scope", "expired", "association", "minimum"] {
        let w = Forks::new().await;
        let thread = w.s.thread_by(&w.s.ada, "Native copy source").await.base.id;
        let target =
            another_environment(&w.s, "Native copy target", Some(w.s.rig.machine_id)).await;
        let mut request = w.to_second();
        request.history = Some(ForkHistory {
            mode: HistoryMode::Nothing,
            from_turn: None,
            to_turn: None,
        });
        request.environment = Some(ForkEnvironment::Existing {
            environment_id: target,
            files: None,
        });
        let plan = w.plan(&w.s.ada, thread, &request).await.ok();
        assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
        let copied = copy_scene(&w, &plan).await?;
        let before =
            w.s.rig
                .machine
                .handler
                .executions()
                .iter()
                .map(|run| run.id)
                .collect();
        w.s.rig.machine.handler.hold(CommandKind::FsSnapshot);
        let actor = key(
            &w.s.rig.world,
            w.s.ada.id(),
            vec![rule(OP, "thread", thread, "self", "manage")],
        )
        .await?;
        let reply =
            w.s.rig
                .world
                .app
                .call(ThreadForkCreate, &[&thread])
                .token(&actor.secret)
                .body(&ForkCreateBody {
                    preflight_id: plan.preflight_id,
                    notify: None,
                    options: None,
                })
                .send_bytes()
                .await;
        assert_eq!(
            reply.status.as_u16(),
            202,
            "{}",
            String::from_utf8_lossy(&reply.raw)
        );
        let receipt: Json = serde_json::from_slice(&reply.raw)?;
        assert_eq!(receipt["projection"], "receipt");
        let job_id = serde_json::from_value(receipt["job_id"].clone())?;
        let job = orch_db::queries::jobs::job(&mut *w.s.rig.world.conn().await, job_id)
            .await?
            .ok_or("native confirmed job missing")?;
        let id = held(&w, CommandKind::FsSnapshot, &before).await?;
        let authority =
            w.s.rig
                .commands
                .issued_authority(id)
                .ok_or("native file command lost its source")?;
        let private = serde_json::to_value(&authority)?;
        assert_eq!(private["kind"], "fork_native");
        assert_eq!(
            private["authority"]["source"]["origin"]["origin"]["operation"],
            OP
        );
        let clock = w.s.rig.world.app.app.clock.clone();
        let access = orch_api_core::thread_access::AuthzAccess::new(clock.clone());
        let row = orch_db::queries::link::command(&mut *w.s.rig.world.conn().await, id)
            .await?
            .ok_or("native command row missing")?;
        assert!(row.body.get("authority").is_none() && row.body.get("origin").is_none());
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
        let mut changed = row.clone();
        changed.body["environment_id"] = json!(target);
        assert!(
            authority
                .current_callers(&mut tx, &changed, clock.as_ref(), &access)
                .await?
                .is_none()
        );
        tx.rollback().await?;
        match mode {
            "scope" => {
                narrow(
                    &w.s.rig.world,
                    &actor,
                    vec![rule(OP, "thread", thread, "self", "read")],
                )
                .await?
            }
            "expired" => w.s.rig.world.advance(chrono::Duration::hours(2)),
            "association" => {
                sqlx::query("UPDATE environments SET generation=generation+1 WHERE id=$1")
                    .bind(w.s.on.environment)
                    .execute(w.s.rig.db.pool())
                    .await?;
            }
            "minimum" => {
                sqlx::query("UPDATE api_tokens SET expires_at=$2 WHERE id=$1")
                    .bind(actor.id)
                    .bind(w.s.rig.world.now() + chrono::Duration::minutes(1))
                    .execute(w.s.rig.db.pool())
                    .await?;
                let mut tx = w.s.rig.db.begin().await?;
                assert!(
                    authority
                        .current_callers(&mut tx, &row, clock.as_ref(), &access)
                        .await?
                        .is_some()
                );
                tx.commit().await?;
                sqlx::query("UPDATE api_tokens SET expires_at=$2 WHERE id=$1")
                    .bind(actor.id)
                    .bind(w.s.rig.world.now() + chrono::Duration::hours(3))
                    .execute(w.s.rig.db.pool())
                    .await?;
                w.s.rig.world.advance(chrono::Duration::minutes(2));
            }
            _ => {}
        }
        if mode != "live" {
            let mut tx = w.s.rig.db.begin().await?;
            assert!(
                authority
                    .current_callers(&mut tx, &row, clock.as_ref(), &access)
                    .await?
                    .is_none(),
                "native copy did not refuse {mode}"
            );
            tx.rollback().await?;
        }
        w.s.rig.machine.handler.release();
        let done = w.ended(&w.s.ada, job.id).await;
        if mode == "live" {
            assert_eq!(done.state, JobState::Succeeded, "{done:?}");
            assert_eq!(copied.load(std::sync::atomic::Ordering::SeqCst), 2);
        } else {
            assert_ne!(done.state, JobState::Succeeded, "{mode}");
            assert_eq!(
                copied.load(std::sync::atomic::Ordering::SeqCst),
                0,
                "a retained source refusal stops new destination work"
            );
        }
    }
    Ok(())
}
