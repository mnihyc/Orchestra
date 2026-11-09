//! Saved import metadata and report use the same current browser administration as other private artifacts.
use super::*;
use orch_api_contract::imports::{ImportReportDownload, ImportReportDownloadQuery};
use orch_api_contract::storage::{StorageGet, StorageKind, StorageQuery};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_administrator_reads_the_actual_import_report_without_borrowing_its_maker() {
    let o = Office::new().await;
    let file = upload(
        &o,
        &o.s.ada,
        "reviewed.md",
        b"# Reviewed\n\nImported content.\n",
    )
    .await;
    let imported = imported(
        &o,
        &o.s.ada,
        o.topic(),
        &asked("administrative-report", &file),
    )
    .await;
    let admin = places::admin(&o.s.rig.world, "import-reviewer").await;
    places::admin_mode(&o.s.rig.world, &admin, false).await;
    let app = &o.s.rig.world.app;
    assert_eq!(
        app.call(ImportGet, &[&imported.id])
            .token(&admin.token)
            .send()
            .await
            .status,
        404
    );
    places::admin_mode(&o.s.rig.world, &admin, true).await;
    let viewed = read(&o, &admin, imported.id).await;
    assert!(
        viewed
            .admin_access
            .as_ref()
            .is_some_and(|reach| reach.private)
    );
    assert_eq!(viewed.id, imported.id);
    assert_eq!(viewed.files, imported.files);
    let ordinary = report(&o, &o.s.ada, imported.id).await;
    assert_eq!(report(&o, &admin, imported.id).await, ordinary);
    let downloaded = app
        .call(ImportReportDownload, &[&imported.id])
        .token(&admin.token)
        .query(&ImportReportDownloadQuery::default())
        .send_bytes()
        .await;
    assert_eq!(
        downloaded.status,
        200,
        "{}",
        String::from_utf8_lossy(&downloaded.raw)
    );
    let reviewed:i64=sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action='admin.opened' AND object_id=$1 AND actor_id=$2")
        .bind(imported.id).bind(admin.id()).fetch_one(o.s.rig.db.pool()).await.unwrap();
    assert_eq!(
        reviewed, 3,
        "metadata, report page and report download are actual openings"
    );
    let inventory = app
        .call(StorageGet, &[])
        .token(&admin.token)
        .query(&StorageQuery {
            scope: Some("system".into()),
            kind: Some(StorageKind::Import),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    let retained = inventory
        .items
        .iter()
        .find(|row| {
            row.artifact
                .as_ref()
                .is_some_and(|artifact| artifact.id == imported.id)
        })
        .unwrap();
    assert_eq!(
        retained.owner.as_ref().map(|owner| owner.id),
        Some(o.s.ada.id())
    );
    assert_eq!(
        retained.bytes,
        Some(0),
        "the completed native import released its input pins; its report stays"
    );
    assert!(retained.physical_parts.is_empty());
    places::admin_mode(&o.s.rig.world, &admin, false).await;
    let denied = app
        .call(ImportReportDownload, &[&imported.id])
        .token(&admin.token)
        .query(&ImportReportDownloadQuery::default())
        .send_bytes()
        .await;
    assert_eq!(denied.status, 404);
    assert!(read(&o, &o.s.ada, imported.id).await.admin_access.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pending_native_imports_list_their_maker_and_shared_retained_sources_only_once() {
    let o = Office::new().await;
    // Keep the real requests queued so their original source custody remains live.
    o.s.rig.engine.stop();
    let content = b"# Queued source\n\nA retained immutable input.\n";
    let file = upload(&o, &o.s.ada, "queued.md", content).await;
    let mut ids = Vec::new();
    for client in ["kept-first", "kept-second"] {
        let response =
            o.s.rig
                .world
                .app
                .call(NodeImportCreate, &[&o.topic()])
                .token(&o.s.ada.token)
                .body(&asked(client, &file))
                .send()
                .await;
        assert_eq!(response.status, 202, "{:?}", response.body);
        let id: Id = sqlx::query_scalar("SELECT id FROM imports WHERE client_id=$1 AND user_id=$2")
            .bind(client)
            .bind(o.s.ada.id())
            .fetch_one(o.s.rig.db.pool())
            .await
            .unwrap();
        ids.push(id);
    }
    let admin = places::admin(&o.s.rig.world, "queued-reviewer").await;
    let report =
        o.s.rig
            .world
            .app
            .call(StorageGet, &[])
            .token(&admin.token)
            .query(&StorageQuery {
                scope: Some("system".into()),
                kind: Some(StorageKind::Import),
                ..Default::default()
            })
            .send()
            .await
            .ok();
    let retained = report
        .items
        .iter()
        .filter(|row| {
            row.artifact
                .as_ref()
                .is_some_and(|artifact| ids.contains(&artifact.id))
        })
        .collect::<Vec<_>>();
    assert_eq!(retained.len(), 2);
    for row in retained {
        assert_eq!(row.owner.as_ref().map(|owner| owner.id), Some(o.s.ada.id()));
        assert_eq!(row.bytes, Some(i64::try_from(content.len()).unwrap()));
        assert_eq!(row.physical_parts.len(), 1);
        assert_eq!(row.artifact.as_ref().unwrap().state, "pending");
    }
    assert_eq!(report.totals.bytes, i64::try_from(content.len()).unwrap());
    let attached = orch_db::queries::uploads::inventory::attachments(
        &mut *o.s.rig.world.conn().await,
        &file.blob,
        Some(o.s.ada.id()),
        10,
    )
    .await
    .unwrap();
    assert_eq!(attached.0, 2);
    assert!(
        attached
            .1
            .iter()
            .all(|(kind, id)| kind == "import" && ids.contains(id))
    );
    // Nullable native custody must never fall back to the job creator or the inspecting admin.
    sqlx::query("UPDATE imports SET user_id=NULL WHERE id=$1")
        .bind(ids[0])
        .execute(o.s.rig.db.pool())
        .await
        .unwrap();
    let unknown =
        o.s.rig
            .world
            .app
            .call(StorageGet, &[])
            .token(&admin.token)
            .query(&StorageQuery {
                scope: Some("system".into()),
                kind: Some(StorageKind::Import),
                ..Default::default()
            })
            .send()
            .await
            .ok();
    assert!(
        unknown
            .items
            .iter()
            .find(|row| row
                .artifact
                .as_ref()
                .is_some_and(|artifact| artifact.id == ids[0]))
            .unwrap()
            .owner
            .is_none()
    );
}
