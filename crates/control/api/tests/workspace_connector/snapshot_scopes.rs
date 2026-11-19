//! Actual native recording and FsRecordRead storage on the already-enrolled connector fixture.

use super::*;
use crate::snapshot_scope_helpers as keys;
use orch_api_contract::workspace::{
    SnapshotArchiveFormat, SnapshotArchiveGet, SnapshotArchiveGetQuery,
};
use orch_db::queries::{source, storage};
use orch_link_contract::commands::SerialKey;
use orch_link_contract::source::{FsRecord, RecordKind, RecordState};
use orch_settings_contract::registry::{self, keys as settings};

const FILE_PATH: &str = "src/main.rs";
const CONTENT: &[u8] = b"native retained bytes\n";

#[derive(Clone, Copy, Debug)]
enum Reader {
    Get,
    List,
    Content,
    Archive,
}

impl Reader {
    fn operation(self) -> &'static str {
        match self {
            Self::Get => "SnapshotGet",
            Self::List => "SnapshotFileList",
            Self::Content => "SnapshotFileContentGet",
            Self::Archive => "SnapshotArchiveGet",
        }
    }

    async fn request(self, site: &Site, id: Id, token: &str) -> orch_api_core::testing::RawReply {
        match self {
            Self::Get => {
                site.app()
                    .call(SnapshotGet, &[&id])
                    .token(token)
                    .send_bytes()
                    .await
            }
            Self::List => {
                site.app()
                    .call(SnapshotFileList, &[&id])
                    .token(token)
                    .query(&SnapshotPathQuery { path: "src".into() })
                    .send_bytes()
                    .await
            }
            Self::Content => {
                site.app()
                    .call(SnapshotFileContentGet, &[&id])
                    .token(token)
                    .query(&SnapshotPathQuery {
                        path: FILE_PATH.into(),
                    })
                    .send_bytes()
                    .await
            }
            Self::Archive => {
                site.app()
                    .call(SnapshotArchiveGet, &[&id])
                    .token(token)
                    .query(&SnapshotArchiveGetQuery {
                        format: Some(SnapshotArchiveFormat::Zip),
                    })
                    .send_bytes()
                    .await
            }
        }
    }

    fn assert_bytes(self, reply: orch_api_core::testing::RawReply, snapshot: Id, environment: Id) {
        assert_eq!(
            reply.status,
            StatusCode::OK,
            "{}: {}",
            self.operation(),
            String::from_utf8_lossy(&reply.raw)
        );
        match self {
            Self::Get => {
                let value: orch_api_contract::workspace::Snapshot =
                    serde_json::from_slice(&reply.raw).unwrap();
                assert_eq!(value.id, snapshot);
                assert_eq!(value.environment.id, environment);
                assert_eq!(
                    value.storage,
                    orch_api_contract::workspace::SnapshotStorage::Environment
                );
            }
            Self::List => {
                let value: orch_api_contract::workspace::DirListing =
                    serde_json::from_slice(&reply.raw).unwrap();
                assert_eq!(value.entries.len(), 1);
                assert_eq!(value.entries[0].name, "main.rs");
                assert!(
                    !value.entries[0].cached,
                    "native store remains its owning producer"
                );
            }
            Self::Content => assert_eq!(reply.raw.as_ref(), CONTENT),
            Self::Archive => {
                use std::io::Read;
                let mut zip = zip::ZipArchive::new(std::io::Cursor::new(reply.raw)).unwrap();
                let mut bytes = Vec::new();
                zip.by_name("Native retained state/src/main.rs")
                    .unwrap()
                    .read_to_end(&mut bytes)
                    .unwrap();
                assert_eq!(bytes, CONTENT);
            }
        }
    }
}

async fn native_recorded() -> (Cluster, Id) {
    // This target's existing helper can otherwise invoke Cargo itself. Root owns the one build
    // lane, so these controls require that lane's binary instead of starting a nested builder.
    let executable = std::env::current_exe().unwrap();
    let profile = executable
        .parent()
        .and_then(std::path::Path::parent)
        .unwrap();
    assert!(
        profile.join("orch-connector").is_file(),
        "Root must provide the existing native fixture binary before this target runs"
    );
    let cluster = Cluster::with_store(true, true, &[("web/src/main.rs", CONTENT)], true).await;
    let site = &cluster.site;
    let id = new_id();
    let dispatcher = site.app().app.link.dispatcher();
    let command = dispatcher
        .issue(
            site.machine,
            &SerialKey::Environment(site.environment),
            &FsRecord {
                environment_id: site.environment,
                snapshot_id: id,
                directory: "web".into(),
                kind: RecordKind::Turn,
                record_days: registry::get(settings::WORKSPACE_RECORD_DAYS)
                    .unwrap()
                    .default_value()
                    .unwrap()
                    .as_i64()
                    .unwrap(),
                max_bytes: registry::get(settings::WORKSPACE_RECORD_MAX_BYTES)
                    .unwrap()
                    .default_value()
                    .unwrap()
                    .as_i64()
                    .unwrap(),
            },
        )
        .await
        .unwrap();
    let result = dispatcher.wait_for::<FsRecord>(command.id).await.unwrap();
    assert_eq!(
        result.state,
        RecordState::Complete,
        "actual native recording prerequisite"
    );
    let mut conn = site.world.conn().await;
    let environment =
        orch_db::queries::environments::workspace::environment(&mut conn, site.environment)
            .await
            .unwrap()
            .unwrap();
    source::begin_recorded(
        &mut conn,
        &storage::NewSnapshot {
            id,
            thread_id: Some(site.thread),
            environment_id: site.environment,
            env_generation: environment.generation,
            label: "Native retained state",
            selection: &serde_json::json!({"include":["web"],"exclude":[]}),
            created_by: Some(site.ada.id()),
            at: site.world.now(),
        },
        "web",
        "turn",
    )
    .await
    .unwrap();
    source::finish_recorded(&mut conn, id, &result, site.world.now())
        .await
        .unwrap();
    assert_eq!(result.files, 1);
    assert_eq!(result.bytes, CONTENT.len() as i64);
    let manifest = result.manifest_blob.unwrap();
    storage::pin_snapshot(&mut conn, id, &[manifest])
        .await
        .unwrap();
    drop(conn);
    (cluster, id)
}

async fn exact_native(reader: Reader) {
    let (cluster, id) = native_recorded().await;
    let site = &cluster.site;
    let ordinary = reader.request(site, id, &site.vic.token).await;
    assert_eq!(
        ordinary.status,
        StatusCode::OK,
        "ordinary native {} fixture: {}",
        reader.operation(),
        String::from_utf8_lossy(&ordinary.raw)
    );
    reader.assert_bytes(ordinary, id, site.environment);
    let key = keys::scoped(
        site,
        site.vic.id(),
        vec![keys::read(reader.operation(), site.environment)],
    )
    .await;
    reader.assert_bytes(
        reader.request(site, id, &key.secret).await,
        id,
        site.environment,
    );
    let rows: Vec<(String, Option<String>, serde_json::Value)> = sqlx::query_as(
        "SELECT kind,serial_key,body FROM commands WHERE kind='fs.record.read' AND body->>'snapshot_id'=$1")
        .bind(id.to_string()).fetch_all(&site.world.db.pool).await.unwrap();
    assert_eq!(
        rows.len(),
        if matches!(reader, Reader::Get) { 0 } else { 2 },
        "ordinary and scoped reads use actual native custody"
    );
    for (kind, key, body) in rows {
        assert_eq!(kind, "fs.record.read");
        assert!(key.is_none());
        assert_eq!(body["environment_id"], serde_json::json!(site.environment));
        assert_eq!(body["snapshot_id"], serde_json::json!(id));
        assert_eq!(
            body["path"],
            if matches!(reader, Reader::Content) {
                serde_json::json!(FILE_PATH)
            } else {
                serde_json::Value::Null
            }
        );
        assert_eq!(body["export"], matches!(reader, Reader::Archive));
    }
    drop(cluster.system);
}

#[tokio::test]
async fn scoped_snapshot_get_reads_the_real_recorded_metadata() {
    exact_native(Reader::Get).await;
}
#[tokio::test]
async fn scoped_snapshot_file_list_reads_the_real_recorded_manifest() {
    exact_native(Reader::List).await;
}
#[tokio::test]
async fn scoped_snapshot_file_content_reads_the_real_retained_native_file() {
    exact_native(Reader::Content).await;
}
#[tokio::test]
async fn scoped_snapshot_archive_exports_the_real_retained_native_files() {
    exact_native(Reader::Archive).await;
}
