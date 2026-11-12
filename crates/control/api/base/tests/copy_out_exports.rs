//! Destination inspection keeps the native Export door separate from source View and destination creation.
mod archive_world;
mod places;
mod rig;
mod support;

use orch_api_contract::{
    access::{AccessChoice, HolderInput, PageChange, PageChangeName, PermissionPageUpdate},
    duplicates::{
        CopyDestinationCause, CopyDestinationChoice, CopyDestinationOutcome, CopyDestinationsJudge,
        CopyDestinationsRequest, CopyIntent, CopySubject, CopySubjectKind, DuplicateDestination,
    },
};
use orch_types::Time;

type Proof = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test]
async fn source_thread_and_document_export_rows_decide_cross_org_copy_without_a_mutation_probe()
-> Proof {
    let o = archive_world::Office::new().await;
    let world = &o.s.rig.world;
    let root = places::make_org(world, &o.s.bea, "Copy recipient", "copy-recipient")
        .await
        .base
        .id;
    let project = places::make_project(world, &o.s.bea, root, "Recipient work")
        .await
        .base
        .id;
    let home = places::make_topic(world, &o.s.bea, project, "Recipient topic")
        .await
        .base
        .id;
    for (kind, namespace, id, permission) in [
        (
            CopySubjectKind::Thread,
            "nodes",
            o.thread.base.id,
            "thread.export",
        ),
        (
            CopySubjectKind::Document,
            "documents",
            o.document.base.id,
            "doc.export",
        ),
    ] {
        for export in [false, true] {
            world
                .app
                .call(PermissionPageUpdate, &[&namespace, &id])
                .token(&o.s.ada.token)
                .body(&PageChange {
                    name: Some(vec![PageChangeName {
                        holders: vec![HolderInput::User { id: o.s.bea.id() }],
                        choice: AccessChoice::FullView,
                        export: Some(export),
                        options: None,
                        expires_at: Some(Time::new(world.now() + chrono::Duration::days(1))),
                        note: None,
                    }]),
                    ..Default::default()
                })
                .send()
                .await
                .ok();
            let response = world
                .app
                .call(CopyDestinationsJudge, &[])
                .token(&o.s.bea.token)
                .body(&CopyDestinationsRequest {
                    subject: CopySubject { kind, id },
                    intent: CopyIntent::Duplicate,
                    destinations: vec![DuplicateDestination::Node { id: home }],
                })
                .send()
                .await
                .ok();
            let row = response
                .destinations
                .into_iter()
                .next()
                .ok_or("missing explicit destination")?;
            assert!(row.cross_root);
            if !export {
                match row.outcome {
                    CopyDestinationOutcome::Blocked {
                        cause: CopyDestinationCause::ExportPermission,
                        problem,
                    } => {
                        assert_eq!(places::missing_permissions(&problem), [permission]);
                    }
                    other => {
                        return Err(
                            format!("source View must not replace Export: {other:?}").into()
                        );
                    }
                }
            } else if kind == CopySubjectKind::Thread {
                assert!(matches!(
                    row.outcome,
                    CopyDestinationOutcome::NeedsChoice {
                        choice: CopyDestinationChoice::ForkPlan
                    }
                ));
            } else {
                assert!(matches!(row.outcome, CopyDestinationOutcome::Allowed));
            }
        }
    }
    let created: i64 = sqlx::query_scalar("SELECT count(*) FROM documents WHERE node_id=$1")
        .bind(home)
        .fetch_one(&world.db.pool)
        .await?;
    assert_eq!(created, 0);
    let exported: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action='content.copied_out'")
            .fetch_one(&world.db.pool)
            .await?;
    assert_eq!(
        exported, 0,
        "a read-only eligibility check is not an accepted export or copy"
    );
    Ok(())
}

#[tokio::test]
async fn a_native_cross_org_fork_starts_with_destination_privileges_and_records_the_source_copy()
-> Proof {
    use orch_api_contract::{
        forks::{ForkCreateBody, ForkRequest, ThreadForkCreate, ThreadForkPreflight},
        nodes::JobState,
        workspace::JobGet,
    };
    let o = archive_world::Office::new().await;
    let world = &o.s.rig.world;
    let root = places::make_org(world, &o.s.ada, "Fork recipient", "fork-recipient")
        .await
        .base
        .id;
    let project = places::make_project(world, &o.s.ada, root, "Copied work")
        .await
        .base
        .id;
    let home = places::make_topic(world, &o.s.ada, project, "Copied threads")
        .await
        .base
        .id;
    world
        .app
        .call(PermissionPageUpdate, &[&"nodes", &o.thread.base.id])
        .token(&o.s.ada.token)
        .body(&PageChange {
            name: Some(vec![PageChangeName {
                holders: vec![HolderInput::User { id: o.s.bea.id() }],
                choice: AccessChoice::FullView,
                export: Some(false),
                options: None,
                expires_at: Some(Time::new(world.now() + chrono::Duration::days(1))),
                note: None,
            }]),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    let request: ForkRequest = serde_json::from_value(serde_json::json!({
        "result":"new","topic_id":home,"history":{"mode":"none"},"name":"Independent cross-root fork"
    }))?;
    let plan = world
        .app
        .call(ThreadForkPreflight, &[&o.thread.base.id])
        .token(&o.s.ada.token)
        .body(&request)
        .send()
        .await
        .ok()
        .into_full()
        .map_err(|_| "native fork planning returned no readable plan")?;
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    let job = world
        .app
        .call(ThreadForkCreate, &[&o.thread.base.id])
        .token(&o.s.ada.token)
        .body(&ForkCreateBody {
            preflight_id: plan.preflight_id,
            notify: Some(false),
            options: None,
        })
        .send()
        .await
        .ok()
        .into_full()
        .map_err(|_| "native fork returned no readable job")?;
    let done = tokio::time::timeout(rig::PATIENCE, async {
        loop {
            let current = world
                .app
                .call(JobGet, &[&job.id])
                .token(&o.s.ada.token)
                .send()
                .await
                .ok();
            if !matches!(current.state, JobState::Queued | JobState::Running) {
                break current;
            }
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        }
    })
    .await?;
    assert_eq!(done.state, JobState::Succeeded, "{:?}", done.error);
    let result = done.result.ok_or("native fork result missing")?;
    let thread: orch_types::Id = serde_json::from_value(result["thread_id"].clone())?;
    let namespace: (orch_types::Id, orch_types::Id) =
        sqlx::query_as("SELECT org_id,owner_id FROM nodes WHERE id=$1")
            .bind(thread)
            .fetch_one(&world.db.pool)
            .await?;
    assert_eq!(namespace, (root, o.s.ada.id()));
    let inherited: i64 = sqlx::query_scalar("SELECT count(*) FROM permission_entries WHERE object_id=$1 AND holder_id=$2 AND removed_at IS NULL")
        .bind(thread).bind(o.s.bea.id()).fetch_one(&world.db.pool).await?;
    assert_eq!(
        inherited, 0,
        "the source's changed page cannot follow a copy into another org"
    );
    let audits: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action='content.copied_out' AND object_id=$1 AND details->>'created_id'=$2")
        .bind(o.thread.base.id).bind(thread.to_string()).fetch_one(&world.db.pool).await?;
    assert_eq!(audits, 1);
    Ok(())
}
