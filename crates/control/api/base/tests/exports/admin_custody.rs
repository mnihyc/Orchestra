//! Real export artifacts keep creator custody while an explicit administrative opening is current and audited.
use super::*;
use orch_api_contract::storage::{StorageGet, StorageKind, StorageQuery};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_administrator_opens_an_actual_export_only_in_mode_and_the_opening_is_audited() {
    let o = Office::new().await;
    let export = o.made(&o.s.ada, o.org(), &everything()).await;
    let admin = places::admin(&o.s.rig.world, "artifact-reviewer").await;
    places::admin_mode(&o.s.rig.world, &admin, false).await;
    assert_eq!(o.read(&admin, export.id).await.status, 404);
    assert_eq!(o.fetch(&admin, export.id).await.status, 404);
    let before: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action='admin.opened' AND object_id=$1",
    )
    .bind(export.id)
    .fetch_one(o.s.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(before, 0);
    places::admin_mode(&o.s.rig.world, &admin, true).await;
    let read = o.read(&admin, export.id).await;
    assert_eq!(read.status, 200, "{:?}", read.body);
    let inspected = read.ok();
    assert!(
        inspected
            .admin_access
            .as_ref()
            .is_some_and(|reach| reach.private)
    );
    assert_eq!(inspected.id, export.id);
    assert_eq!(inspected.counts, export.counts);
    let manifest = inspected
        .manifest
        .as_ref()
        .expect("the actual native writer recorded its safe header");
    assert_eq!(manifest["of"]["id"], json!(o.org()));
    assert!(manifest.get("objects").is_none());
    assert!(manifest.get("sealed").is_none());
    assert!(
        !serde_json::to_string(manifest)
            .unwrap()
            .contains("Be kind.")
    );
    assert!(
        o.list(&admin).await.is_empty(),
        "personal lists keep their own namespace"
    );
    let fetched = o.fetch(&admin, export.id).await;
    assert_eq!(
        fetched.status,
        200,
        "{}",
        String::from_utf8_lossy(&fetched.raw)
    );
    let expected = o.fetch(&o.s.ada, export.id).await;
    assert_eq!(fetched.raw, expected.raw);
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(fetched.raw.to_vec())).unwrap();
    let mut header = String::new();
    use std::io::Read;
    zip.by_name("export.json")
        .unwrap()
        .read_to_string(&mut header)
        .unwrap();
    assert_eq!(
        &serde_json::from_str::<Value>(&header).unwrap(),
        manifest,
        "metadata is the exact actual header, not a fabricated package view"
    );
    let audited:i64=sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action='admin.opened' AND object_id=$1 AND actor_id=$2")
        .bind(export.id).bind(admin.id()).fetch_one(o.s.rig.db.pool()).await.unwrap();
    assert_eq!(
        audited, 2,
        "the metadata and package were separate actual openings"
    );
    let inventory =
        o.s.rig
            .world
            .app
            .call(StorageGet, &[])
            .token(&admin.token)
            .query(&StorageQuery {
                scope: Some("system".into()),
                kind: Some(StorageKind::Export),
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
                .is_some_and(|artifact| artifact.id == export.id)
        })
        .unwrap();
    assert_eq!(
        retained.owner.as_ref().map(|owner| owner.id),
        Some(o.s.ada.id())
    );
    assert_eq!(
        retained.bytes,
        Some(i64::try_from(fetched.raw.len()).unwrap())
    );
    assert_eq!(retained.since, Some(export.created_at));
    assert_eq!(inventory.totals.bytes, retained.bytes.unwrap());
    assert_eq!(retained.physical_parts.len(), 1);
    assert_eq!(retained.accounting_key, retained.physical_parts[0].key);
    places::admin_mode(&o.s.rig.world, &admin, false).await;
    assert_eq!(o.fetch(&admin, export.id).await.status, 404);
    assert_eq!(o.read(&admin, export.id).await.status, 404);
    assert!(
        o.read(&o.s.ada, export.id)
            .await
            .ok()
            .admin_access
            .is_none()
    );
}
