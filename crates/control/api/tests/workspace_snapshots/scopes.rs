//! Four actual Snapshot readers keep their existing parent/custody and Full source boundaries.

use super::*;
use crate::snapshot_scope_helpers as keys;
use orch_api_core::testing::RawReply;
use orch_blob_contract::store::stream_of;
use orch_types::Time;

const FILE: &str = "src/main.rs";
const CONTENT: &[u8] = b"fn main() {}\n";

#[derive(Clone, Copy, Debug)]
enum Reader {
    Get,
    List,
    Content,
    Archive,
}

impl Reader {
    const ALL: [Self; 4] = [Self::Get, Self::List, Self::Content, Self::Archive];
    const BYTES: [Self; 3] = [Self::List, Self::Content, Self::Archive];

    fn operation(self) -> &'static str {
        match self {
            Self::Get => "SnapshotGet",
            Self::List => "SnapshotFileList",
            Self::Content => "SnapshotFileContentGet",
            Self::Archive => "SnapshotArchiveGet",
        }
    }

    async fn request(self, site: &Site, id: Id, token: &str) -> RawReply {
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
                    .query(&SnapshotPathQuery { path: FILE.into() })
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

    fn assert_content(self, reply: RawReply, id: Id, origin: Id) {
        assert_eq!(
            reply.status,
            StatusCode::OK,
            "{}: {}",
            self.operation(),
            String::from_utf8_lossy(&reply.raw)
        );
        match self {
            Self::Get => {
                let snapshot: Snapshot = serde_json::from_slice(&reply.raw).unwrap();
                assert_eq!(snapshot.id, id);
                assert_eq!(snapshot.environment.id, origin);
                assert_eq!(snapshot.file_count, 1);
                assert_eq!(snapshot.label, "Private copied state");
            }
            Self::List => {
                let listing: orch_api_contract::workspace::DirListing =
                    serde_json::from_slice(&reply.raw).unwrap();
                assert_eq!(listing.entries.len(), 1);
                assert_eq!(listing.entries[0].name, "main.rs");
            }
            Self::Content => assert_eq!(reply.raw.as_ref(), CONTENT),
            Self::Archive => {
                let mut archive = zip::ZipArchive::new(std::io::Cursor::new(reply.raw)).unwrap();
                let mut bytes = Vec::new();
                archive
                    .by_name("Private copied state/src/main.rs")
                    .unwrap()
                    .read_to_end(&mut bytes)
                    .unwrap();
                assert_eq!(bytes, CONTENT);
            }
        }
    }
}

/// Pure stored facts: these readers must work with no machine or creator job running.
async fn copied(site: &Site, thread: Option<Id>, files: Vec<(String, Vec<u8>)>) -> Id {
    seeded(site, thread, files, true).await
}

async fn seeded(site: &Site, thread: Option<Id>, files: Vec<(String, Vec<u8>)>, held: bool) -> Id {
    use orch_engine::manifest::{Entry, EntryKind, Header, MANIFEST_VERSION};
    let store = site.app().app.blobs.store();
    let mut entries = Vec::new();
    let mut hashes = Vec::new();
    let mut total = 0;
    for (path, bytes) in files {
        let hash = Hash::of(&bytes);
        if held {
            store
                .put(
                    &hash,
                    Some(bytes.len() as i64),
                    stream_of(vec![bytes::Bytes::from(bytes.clone())]),
                )
                .await
                .unwrap();
            hashes.push(hash);
        }
        total += bytes.len() as i64;
        entries.push(Entry {
            path,
            kind: EntryKind::File,
            size: bytes.len() as i64,
            mtime_ns: 0,
            mode: 0o644,
            hash: Some(hash),
            link: None,
        });
    }
    let count = entries.len() as i64;
    let manifest = orch_engine::manifest::build(
        &Header {
            v: MANIFEST_VERSION,
            root: "/workspace/web".into(),
            generated_at: Time::new(site.world.now()),
            generation: 1,
            case_sensitive: true,
        },
        entries,
    )
    .unwrap();
    store
        .put(
            &manifest.hash,
            Some(manifest.bytes.len() as i64),
            stream_of(vec![bytes::Bytes::copy_from_slice(&manifest.bytes)]),
        )
        .await
        .unwrap();
    let id = new_id();
    let mut conn = site.world.conn().await;
    hashes.push(manifest.hash);
    for hash in &hashes {
        storage::register_blob(
            &mut conn,
            hash,
            store.size(hash).await.unwrap().unwrap(),
            site.world.now(),
        )
        .await
        .unwrap();
    }
    storage::begin_snapshot(
        &mut conn,
        &NewSnapshot {
            id,
            thread_id: thread,
            environment_id: site.environment,
            env_generation: 1,
            label: "Private copied state",
            selection: &json!({"include":["**"],"exclude":[]}),
            created_by: Some(site.ada.id()),
            at: site.world.now(),
        },
    )
    .await
    .unwrap();
    assert!(
        storage::end_snapshot(
            &mut conn,
            id,
            &SnapshotEnd {
                state: snapshot_state::COMPLETE,
                file_count: count,
                total_bytes: total,
                missing_count: 0,
                manifest_blob: Some(manifest.hash),
                at: site.world.now(),
            }
        )
        .await
        .unwrap()
    );
    storage::pin_snapshot(&mut conn, id, &hashes).await.unwrap();
    id
}

async fn fixture() -> (Site, Id) {
    let site = Site::new().await;
    let id = copied(
        &site,
        Some(site.thread),
        vec![(FILE.into(), CONTENT.into())],
    )
    .await;
    site.power_off().await;
    (site, id)
}

async fn exact_positive(reader: Reader) {
    let (site, id) = fixture().await;
    let ordinary = reader.request(&site, id, &site.vic.token).await;
    let full = matches!(reader, Reader::Get | Reader::List)
        .then(|| serde_json::from_slice::<serde_json::Value>(&ordinary.raw).unwrap());
    reader.assert_content(ordinary, id, site.environment);
    let issued = keys::scoped(
        &site,
        site.vic.id(),
        vec![keys::read(reader.operation(), site.environment)],
    )
    .await;
    let scoped = reader.request(&site, id, &issued.secret).await;
    assert_eq!(
        scoped.status,
        StatusCode::OK,
        "{}: {}",
        reader.operation(),
        String::from_utf8_lossy(&scoped.raw)
    );
    if let Some(full) = full {
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&scoped.raw).unwrap(),
            full,
            "the existing complete projection is preserved under same-operation read"
        );
    }
    reader.assert_content(scoped, id, site.environment);
    assert_eq!(
        site.sent(CommandKind::FsRecordRead),
        0,
        "copied content does not borrow native authority"
    );
}

/// This component peer preserves the actual HTTP/native upload custody path. It is not a
/// real-connector or journal proof; those have a separate Root execution target.
struct RecordedPeer {
    disk: Arc<workspace::Disk>,
    content: workspace::Content,
    snapshot: Id,
    environment: Id,
    machine: Id,
    manifest: Vec<u8>,
    commands: std::sync::Mutex<Vec<orch_link_contract::commands::Cmd>>,
    hold: AtomicBool,
    reached: Notify,
    release: Notify,
}

#[async_trait::async_trait]
impl orch_api_core::testing::Machine for RecordedPeer {
    async fn run(
        &self,
        command: &orch_link_contract::commands::Cmd,
    ) -> Result<serde_json::Value, orch_link_contract::commands::CommandError> {
        if command.kind != CommandKind::FsRecordRead.as_str() {
            return self.disk.run(command).await;
        }
        let body: orch_link_contract::source::FsRecordRead =
            serde_json::from_value(command.body.clone()).unwrap();
        assert_eq!(body.environment_id, self.environment);
        assert_eq!(body.snapshot_id, self.snapshot);
        assert!(command.key.is_none(), "recorded reads use SerialKey::None");
        assert!(!(body.export && body.path.is_some()));
        self.commands.lock().unwrap().push(command.clone());
        if self.hold.swap(false, Ordering::SeqCst) {
            self.reached.notify_one();
            self.release.notified().await;
        }
        if body.export || body.path.is_some() {
            assert!(body.path.as_deref().is_none_or(|path| path == FILE));
            self.content
                .upload(self.machine, command.id, CONTENT, workspace::PIECE)
                .await;
        }
        let bytes = if body.path.is_some() {
            CONTENT
        } else {
            &self.manifest
        };
        let hash = self
            .content
            .upload(self.machine, command.id, bytes, workspace::PIECE)
            .await;
        Ok(json!({"blob":hash,"size":bytes.len()}))
    }
}

async fn recorded() -> (Site, Id, Arc<RecordedPeer>) {
    let mut site = Site::new().await;
    let id = seeded(
        &site,
        Some(site.thread),
        vec![(FILE.into(), CONTENT.into())],
        false,
    )
    .await;
    sqlx::query("UPDATE snapshots SET storage='environment',kind='turn',directory='web',tree_ref='captured-root',expires_at=$2 WHERE id=$1")
        .bind(id).bind(site.world.now() + chrono::Duration::days(7)).execute(&site.world.db.pool).await.unwrap();
    let row = storage::snapshot(&mut *site.world.conn().await, id)
        .await
        .unwrap()
        .unwrap();
    let manifest = site
        .app()
        .app
        .blobs
        .read_whole(&row.manifest_blob.unwrap(), 1024 * 1024)
        .await
        .unwrap()
        .to_vec();
    let peer = Arc::new(RecordedPeer {
        disk: site.disk.clone(),
        content: site.content.clone(),
        snapshot: id,
        environment: site.environment,
        machine: site.machine,
        manifest,
        commands: std::sync::Mutex::new(Vec::new()),
        hold: AtomicBool::new(false),
        reached: Notify::new(),
        release: Notify::new(),
    });
    site.power_on_with(peer.clone());
    (site, id, peer)
}

async fn recorded_positive(reader: Reader) {
    let (site, id, peer) = recorded().await;
    reader.assert_content(
        reader.request(&site, id, &site.vic.token).await,
        id,
        site.environment,
    );
    let count = peer.commands.lock().unwrap().len();
    assert_eq!(
        count,
        usize::from(!matches!(reader, Reader::Get)),
        "ordinary fixture reached the actual native branch"
    );
    let key = keys::scoped(
        &site,
        site.vic.id(),
        vec![keys::read(reader.operation(), site.environment)],
    )
    .await;
    reader.assert_content(
        reader.request(&site, id, &key.secret).await,
        id,
        site.environment,
    );
    assert_eq!(
        peer.commands.lock().unwrap().len(),
        count * 2,
        "scoped read uses the same recorded-store producer, rather than a cached authority bypass"
    );
}

#[tokio::test]
async fn snapshot_recorded_get_exact_read_keeps_the_existing_full_projection() {
    recorded_positive(Reader::Get).await;
}
#[tokio::test]
async fn snapshot_recorded_file_list_exact_read_uses_native_manifest_custody() {
    recorded_positive(Reader::List).await;
}
#[tokio::test]
async fn snapshot_recorded_file_content_exact_read_uses_native_file_custody() {
    recorded_positive(Reader::Content).await;
}
#[tokio::test]
async fn snapshot_recorded_archive_exact_read_uses_native_export_custody() {
    recorded_positive(Reader::Archive).await;
}

#[tokio::test]
async fn recorded_snapshot_keeps_its_origin_environment_after_the_thread_moves() {
    let (site, id, _) = recorded().await;
    let destination = elsewhere(&site, &site.eve, site.eve.person.personal_org).await;
    move_thread(&site, destination).await;
    for reader in Reader::ALL {
        reader.assert_content(
            reader.request(&site, id, &site.vic.token).await,
            id,
            site.environment,
        );
        assert!(
            !reader
                .request(&site, id, &site.eve.token)
                .await
                .status
                .is_success(),
            "current Thread Environment gives no recorded-store right"
        );
        let wrong = keys::scoped(
            &site,
            site.vic.id(),
            vec![keys::read(reader.operation(), destination)],
        )
        .await;
        assert_eq!(
            reader.request(&site, id, &wrong.secret).await.status,
            StatusCode::FORBIDDEN
        );
        let actual = keys::scoped(
            &site,
            site.vic.id(),
            vec![keys::read(reader.operation(), site.environment)],
        )
        .await;
        reader.assert_content(
            reader.request(&site, id, &actual.secret).await,
            id,
            site.environment,
        );
    }
}

#[tokio::test]
async fn historical_snapshot_generation_is_distinct_from_the_current_custodian_generation() {
    let (site, id, _) = recorded().await;
    sqlx::query("UPDATE environments SET generation=generation+1 WHERE id=$1")
        .bind(site.environment)
        .execute(&site.world.db.pool)
        .await
        .unwrap();
    let row = storage::snapshot(&mut *site.world.conn().await, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.env_generation, 1,
        "recording provenance stays historical content"
    );
    for reader in Reader::ALL {
        reader.assert_content(
            reader.request(&site, id, &site.vic.token).await,
            id,
            site.environment,
        );
        let key = keys::scoped(
            &site,
            site.vic.id(),
            vec![keys::read(reader.operation(), site.environment)],
        )
        .await;
        reader.assert_content(
            reader.request(&site, id, &key.secret).await,
            id,
            site.environment,
        );
    }
}

#[tokio::test]
async fn accepted_recorded_upload_finishes_its_facts_after_revocation_without_public_readback() {
    let (site, id, peer) = recorded().await;
    assert!(
        !site
            .app()
            .app
            .blobs
            .store()
            .exists(&Hash::of(CONTENT))
            .await
            .unwrap(),
        "native file has not yet been copied"
    );
    let key = keys::scoped(
        &site,
        site.vic.id(),
        vec![keys::read("SnapshotFileContentGet", site.environment)],
    )
    .await;
    peer.hold.store(true, Ordering::SeqCst);
    let (app, token) = (site.app().clone(), key.secret.clone());
    let mut pending = tokio::spawn(async move {
        let request = app
            .call(SnapshotFileContentGet, &[&id])
            .token(&token)
            .query(&SnapshotPathQuery { path: FILE.into() });
        request.send_bytes().await
    });
    tokio::select! {
        arrived = tokio::time::timeout(RENDEZVOUS, peer.reached.notified()) => { arrived.unwrap(); }
        reply = &mut pending => {
            let reply = reply.unwrap();
            panic!("reader returned before actual accepted peer custody: {} {}", reply.status, String::from_utf8_lossy(&reply.raw));
        }
    }
    let command = peer.commands.lock().unwrap()[0].id;
    eventually(
        "the actual read command is Accepted before key withdrawal",
        || async {
            orch_db::queries::link::command(&mut *site.world.conn().await, command)
                .await
                .unwrap()
                .is_some_and(|row| row.state == "accepted")
        },
    )
    .await;
    keys::revoke(&site, &key).await;
    peer.release.notify_one();
    let denied = tokio::time::timeout(RENDEZVOUS, pending)
        .await
        .unwrap()
        .unwrap();
    assert!(
        !denied.status.is_success(),
        "accepted native custody is not public readback authority"
    );
    assert!(
        site.app()
            .app
            .blobs
            .store()
            .exists(&Hash::of(CONTENT))
            .await
            .unwrap(),
        "accepted exact upload reconciled"
    );
    let ended = orch_db::queries::link::command(&mut *site.world.conn().await, command)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        ended.state, "succeeded",
        "revocation does not rewrite an accepted native fact"
    );
    assert_eq!(ended.result.unwrap()["blob"], json!(Hash::of(CONTENT)));
    assert_eq!(
        peer.commands.lock().unwrap().len(),
        1,
        "no replacement read borrows the old Accepted command"
    );
}

#[tokio::test]
async fn snapshot_get_exact_read_returns_its_actual_full_source() {
    exact_positive(Reader::Get).await;
}
#[tokio::test]
async fn snapshot_file_list_exact_read_uses_its_saved_manifest() {
    exact_positive(Reader::List).await;
}
#[tokio::test]
async fn snapshot_file_content_exact_read_needs_no_live_machine() {
    exact_positive(Reader::Content).await;
}
#[tokio::test]
async fn snapshot_archive_exact_read_copies_only_its_saved_files() {
    exact_positive(Reader::Archive).await;
}

#[tokio::test]
async fn snapshot_scope_limits_browser_legacy_and_ordinary_rights_stay_independent() {
    let (site, id) = fixture().await;
    let legacy = keys::legacy(&site, site.vic.id(), None).await;
    let node_bound_legacy = keys::legacy(&site, site.vic.id(), Some(site.topic)).await;
    for reader in Reader::ALL {
        reader.assert_content(
            reader.request(&site, id, &site.vic.token).await,
            id,
            site.environment,
        );
        reader.assert_content(
            reader.request(&site, id, &legacy.secret).await,
            id,
            site.environment,
        );
        assert_eq!(
            reader
                .request(&site, id, &node_bound_legacy.secret)
                .await
                .status,
            StatusCode::FORBIDDEN,
            "ordinary resource Site.path=None retains the legacy node ceiling"
        );
        for rules in [
            vec![keys::rule(
                reader.operation(),
                "environment",
                site.environment,
                "self",
                &["write"],
            )],
            vec![keys::read(reader.operation(), new_id())],
            vec![keys::rule(
                reader.operation(),
                "thread",
                site.environment,
                "self",
                &["read"],
            )],
            vec![keys::read(
                match reader {
                    Reader::Get => "SnapshotFileList",
                    _ => "SnapshotGet",
                },
                site.environment,
            )],
        ] {
            let issued = keys::scoped(&site, site.vic.id(), rules).await;
            let denied = reader.request(&site, id, &issued.secret).await;
            assert_eq!(
                denied.status,
                StatusCode::FORBIDDEN,
                "{}: {}",
                reader.operation(),
                String::from_utf8_lossy(&denied.raw)
            );
            assert_eq!(denied.problem().code, ErrorCode::AuthzDenied);
        }
        let ordinary = reader.request(&site, id, &site.eve.token).await;
        assert!(!ordinary.status.is_success(), "ordinary outsider premise");
        let outsider = keys::scoped(
            &site,
            site.eve.id(),
            vec![keys::read(reader.operation(), site.environment)],
        )
        .await;
        assert!(
            !reader
                .request(&site, id, &outsider.secret)
                .await
                .status
                .is_success(),
            "scope grants no ordinary FilesView"
        );
    }
}

use orch_blob_contract::range::ByteRange;
use orch_blob_contract::store::{BlobError, BlobRead, BlobStore, ByteStream, Stored};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::sync::Notify;

// This is the existing reader rendezvous watchdog, not an authority cache or product timeout.
const RENDEZVOUS: std::time::Duration = std::time::Duration::from_secs(30);

struct OpenGate {
    inner: Arc<dyn BlobStore>,
    hash: Hash,
    used: AtomicBool,
    reads: AtomicUsize,
    reached: Notify,
    release: Notify,
}

#[async_trait::async_trait]
impl BlobStore for OpenGate {
    async fn put(
        &self,
        hash: &Hash,
        size: Option<i64>,
        body: ByteStream,
    ) -> Result<Stored, BlobError> {
        self.inner.put(hash, size, body).await
    }
    async fn get(&self, hash: &Hash, range: Option<ByteRange>) -> Result<BlobRead, BlobError> {
        if *hash == self.hash {
            self.reads.fetch_add(1, Ordering::SeqCst);
        }
        let opened = self.inner.get(hash, range).await;
        if *hash == self.hash && !self.used.swap(true, Ordering::SeqCst) {
            self.reached.notify_one();
            self.release.notified().await;
        }
        opened
    }
    async fn exists(&self, hash: &Hash) -> Result<bool, BlobError> {
        self.inner.exists(hash).await
    }
    async fn size(&self, hash: &Hash) -> Result<Option<i64>, BlobError> {
        self.inner.size(hash).await
    }
    async fn delete(&self, hash: &Hash) -> Result<bool, BlobError> {
        self.inner.delete(hash).await
    }
}

fn gate_blob_open(site: &mut Site, hash: Hash) -> Arc<OpenGate> {
    gate_store(site, hash, false)
}

fn gate_store(site: &mut Site, hash: Hash, recording_recovery: bool) -> Arc<OpenGate> {
    use orch_api_core::{registry::Registry, state::AppState, testing::TestApp};
    let old = site.app().app.clone();
    let gate = Arc::new(OpenGate {
        inner: old.blobs.store(),
        hash,
        used: AtomicBool::new(false),
        reads: AtomicUsize::new(0),
        reached: Notify::new(),
        release: Notify::new(),
    });
    let engine = if recording_recovery {
        let engine =
            orch_api_core::thread_engine::detached(&old.db, &old.clock, &old.config, &old.link);
        engine.attach_blobs(gate.clone());
        engine
    } else {
        old.engine.clone()
    };
    let state = Arc::new(AppState {
        db: old.db.clone(),
        config: old.config.clone(),
        clock: old.clock.clone(),
        link: old.link.clone(),
        engine,
        realtime: old.realtime.clone(),
        build: old.build,
        blobs: orch_api_core::blobs::Blobs::new(
            gate.clone(),
            std::path::Path::new(&old.config.storage.root),
        ),
        cache: orch_api_core::cache::Cache::new(&old.config),
        host_ports: old.host_ports.clone(),
        builtin_sources: old.builtin_sources.clone(),
    });
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    site.world.app = TestApp::over(state, site.world.app.clock.clone(), registry);
    gate
}

async fn unavailable_original(site: &Site, id: Id) -> Hash {
    use orch_db::queries::link as commands;
    use orch_link_contract::source::{FsRecord, FsRecordResult, RecordKind, RecordState};
    let mut conn = site.world.conn().await;
    let hash = storage::snapshot(&mut conn, id)
        .await
        .unwrap()
        .unwrap()
        .manifest_blob
        .unwrap();
    let command = orch_engine::ids::derived("fs.record.before-turn", &[id.as_bytes()]);
    let body = serde_json::to_value(FsRecord {
        environment_id: site.environment,
        snapshot_id: id,
        directory: "web".into(),
        kind: RecordKind::Turn,
        record_days: 7,
        max_bytes: CONTENT.len() as i64,
    })
    .unwrap();
    let serial = site.environment.to_string();
    commands::insert_command(
        &mut conn,
        &commands::NewCommand {
            id: command,
            machine_id: site.machine,
            kind: CommandKind::FsRecord.as_str(),
            body: &body,
            serial_key: Some(&serial),
            deadline_at: site.world.now() + chrono::Duration::minutes(1),
        },
        site.world.now(),
    )
    .await
    .unwrap();
    let result = serde_json::to_value(FsRecordResult {
        state: RecordState::Complete,
        tree_ref: Some("original-captured-root".into()),
        repository_head: None,
        manifest_blob: Some(hash),
        files: 1,
        bytes: CONTENT.len() as i64,
        expires_at: Some(Time::new(site.world.now() + chrono::Duration::days(7))),
        reason: None,
    })
    .unwrap();
    commands::finish_command(
        &mut conn,
        command,
        &commands::Ending::Succeeded(&result),
        site.world.now(),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE snapshots SET kind='turn',storage='environment',directory='web',state='unavailable',unavailable_reason='store_unavailable',tree_ref=NULL,manifest_blob=NULL WHERE id=$1")
        .bind(id).execute(&mut *conn).await.unwrap();
    hash
}

#[tokio::test]
async fn snapshot_get_recovery_keeps_exact_original_facts_but_rechecks_current_public_read() {
    let (mut site, id) = fixture().await;
    let original = unavailable_original(&site, id).await;
    let ordinary = site
        .app()
        .call(SnapshotGet, &[&id])
        .token(&site.vic.token)
        .send()
        .await
        .ok();
    assert_eq!(
        ordinary.state,
        SnapshotState::Complete,
        "ordinary original-result repair fixture"
    );
    assert_eq!(
        ordinary.storage,
        orch_api_contract::workspace::SnapshotStorage::Environment
    );
    // Replay the old validation fallback, never a new record command or a different file source.
    sqlx::query("UPDATE snapshots SET state='unavailable',unavailable_reason='store_unavailable',tree_ref=NULL,manifest_blob=NULL WHERE id=$1")
        .bind(id).execute(&site.world.db.pool).await.unwrap();
    let key = keys::scoped(
        &site,
        site.vic.id(),
        vec![keys::read("SnapshotGet", site.environment)],
    )
    .await;
    let gate = gate_store(&mut site, original, true);
    let (app, token) = (site.app().clone(), key.secret.clone());
    let mut pending = tokio::spawn(async move {
        let request = app.call(SnapshotGet, &[&id]).token(&token);
        request.send_bytes().await
    });
    tokio::select! {
        arrived = tokio::time::timeout(RENDEZVOUS, gate.reached.notified()) => { arrived.unwrap(); }
        reply = &mut pending => {
            let reply = reply.unwrap();
            panic!("reader returned before exact original manifest validation: {} {}", reply.status, String::from_utf8_lossy(&reply.raw));
        }
    }
    keys::revoke(&site, &key).await;
    gate.release.notify_one();
    let denied = tokio::time::timeout(RENDEZVOUS, pending)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        denied.status,
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
    ));
    let restored = storage::snapshot(&mut *site.world.conn().await, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        restored.state,
        snapshot_state::COMPLETE,
        "system factual reconciliation survives revoke"
    );
    assert_eq!(restored.manifest_blob, Some(original));
    assert_eq!(
        site.sent(CommandKind::FsRecord),
        0,
        "Get never recaptures today's files"
    );
    assert_eq!(
        site.sent(CommandKind::FsRecordRead),
        0,
        "Get recovery issues no native read"
    );
}

#[derive(Clone, Copy, Debug)]
enum Change {
    Revoke,
    Narrow,
    Account,
    Move,
    Manifest,
    Removed,
}

async fn after_file_open(change: Change, header: Option<(&'static str, String)>) {
    let (mut site, id) = fixture().await;
    Reader::Content.assert_content(
        Reader::Content.request(&site, id, &site.vic.token).await,
        id,
        site.environment,
    );
    let spare_target = new_id();
    let mut rules = vec![keys::read("SnapshotFileContentGet", site.environment)];
    if matches!(change, Change::Narrow) {
        rules.push(keys::read("SnapshotFileContentGet", spare_target));
    }
    let key = keys::scoped(&site, site.vic.id(), rules).await;
    let destination = if matches!(change, Change::Move) {
        Some(elsewhere(&site, &site.ada, site.topic).await)
    } else {
        None
    };
    if destination.is_some() {
        // The move test isolates scope from ordinary rights on the new Environment.
        orch_db::queries::access::testing::add_entry(
            &mut *site.world.conn().await,
            &orch_db::queries::access::PageKey::new("environment", destination.unwrap(), "live"),
            "any",
            "files.view",
            ("user", Some(site.vic.id()), None),
            None,
            site.world.now(),
        )
        .await
        .unwrap();
    }
    let replacement = if matches!(change, Change::Manifest) {
        let other = copied(
            &site,
            Some(site.thread),
            vec![(FILE.into(), b"different saved source\n".to_vec())],
        )
        .await;
        Some(
            storage::snapshot(&mut *site.world.conn().await, other)
                .await
                .unwrap()
                .unwrap()
                .manifest_blob
                .unwrap(),
        )
    } else {
        None
    };
    let gate = gate_blob_open(&mut site, Hash::of(CONTENT));
    let (app, token) = (site.app().clone(), key.secret.clone());
    let mut pending = tokio::spawn(async move {
        let call = app
            .call(SnapshotFileContentGet, &[&id])
            .token(&token)
            .query(&SnapshotPathQuery { path: FILE.into() });
        let call = if let Some((name, value)) = header {
            call.header(name, &value)
        } else {
            call
        };
        call.send_bytes().await
    });
    tokio::select! {
        arrived = tokio::time::timeout(RENDEZVOUS, gate.reached.notified()) => {
            arrived.expect("the already-admitted reader opens its actual selected file");
        }
        reply = &mut pending => {
            let reply = reply.unwrap();
            panic!("Snapshot reader returned before its real blob-open checkpoint: {} {}", reply.status,
                String::from_utf8_lossy(&reply.raw));
        }
    }
    match change {
        Change::Revoke => keys::revoke(&site, &key).await,
        Change::Narrow => {
            let scope: orch_access_contract::key_scope::KeyScope =
                serde_json::from_value(json!({"rules":[
                keys::read("SnapshotFileContentGet", spare_target)
                ]}))
                .unwrap();
            assert!(scope.validate().is_empty());
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                .bind(key.id)
                .bind(json!(scope))
                .execute(&site.world.db.pool)
                .await
                .unwrap();
            orch_db::sequences::notify_identity(&mut *site.world.conn().await, key.user)
                .await
                .unwrap();
        }
        Change::Account => {
            let removed = site
                .app()
                .call(
                    orch_api_contract::ownership::OrgMemberRemove,
                    &[&site.org, &site.vic.id()],
                )
                .token(&site.ada.token)
                .body(&orch_api_contract::ownership::OrgMemberRemoveBody::default())
                .send_bytes()
                .await;
            assert_eq!(
                removed.status,
                StatusCode::NO_CONTENT,
                "ordinary membership ending removes roles/entries and advances its epoch: {}",
                String::from_utf8_lossy(&removed.raw)
            );
            let ordinary = Reader::Get.request(&site, id, &site.vic.token).await;
            assert!(
                matches!(
                    ordinary.status,
                    StatusCode::FORBIDDEN | StatusCode::NOT_FOUND
                ),
                "the fresh same browser reader really lost ordinary FilesView before release: {} {}",
                ordinary.status,
                String::from_utf8_lossy(&ordinary.raw)
            );
        }
        Change::Move => move_thread(&site, destination.unwrap()).await,
        Change::Manifest => {
            sqlx::query("UPDATE snapshots SET manifest_blob=$2 WHERE id=$1")
                .bind(id)
                .bind(replacement.unwrap().as_bytes().to_vec())
                .execute(&site.world.db.pool)
                .await
                .unwrap();
        }
        Change::Removed => {
            let removed = site
                .app()
                .call(SnapshotDelete, &[&id])
                .token(&site.ada.token)
                .send_bytes()
                .await;
            assert_eq!(
                removed.status,
                StatusCode::NO_CONTENT,
                "ordinary maker deletion fixture"
            );
        }
    }
    gate.release.notify_one();
    let reply = tokio::time::timeout(RENDEZVOUS, pending)
        .await
        .unwrap()
        .unwrap();
    if matches!(change, Change::Manifest | Change::Removed) {
        assert_eq!(
            reply.status,
            StatusCode::OK,
            "{change:?}: an authenticated opened tuple is not revoked by row mutation"
        );
        assert_eq!(
            reply.raw.as_ref(),
            CONTENT,
            "the open read never retargets to the replacement manifest"
        );
        let fresh = Reader::Content.request(&site, id, &key.secret).await;
        if matches!(change, Change::Removed) {
            assert_eq!(
                fresh.status,
                StatusCode::NOT_FOUND,
                "fresh requests resolve the current row"
            );
        } else {
            assert_eq!(fresh.status, StatusCode::OK);
            assert_eq!(fresh.raw.as_ref(), b"different saved source\n");
        }
        return;
    }
    assert!(
        reply.status.is_client_error() && reply.status != StatusCode::RANGE_NOT_SATISFIABLE,
        "{change:?} allowed existing file bytes or conditional metadata after source/authority changed: {}",
        reply.status
    );
    if matches!(change, Change::Revoke | Change::Narrow) {
        assert!(
            matches!(
                reply.status,
                StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
            ),
            "credential loss must be an actual authentication/authorization refusal"
        );
    }
    if matches!(change, Change::Move) {
        Reader::Content.assert_content(
            Reader::Content.request(&site, id, &site.vic.token).await,
            id,
            site.environment,
        );
    }
}

#[tokio::test]
async fn snapshot_file_content_rechecks_key_after_actual_blob_open() {
    after_file_open(Change::Revoke, None).await;
}

#[tokio::test]
async fn snapshot_file_content_rechecks_current_scope_after_actual_blob_open() {
    after_file_open(Change::Narrow, None).await;
}

#[tokio::test]
async fn snapshot_file_content_rechecks_ordinary_membership_after_actual_blob_open() {
    after_file_open(Change::Account, None).await;
}

#[tokio::test]
async fn snapshot_file_content_rechecks_current_thread_environment_after_actual_blob_open() {
    after_file_open(Change::Move, None).await;
}

#[tokio::test]
async fn snapshot_open_file_keeps_its_authenticated_manifest_across_replacement_and_deletion() {
    after_file_open(Change::Manifest, None).await;
    after_file_open(Change::Removed, None).await;
}

#[tokio::test]
async fn snapshot_conditional_and_range_metadata_do_not_bypass_the_current_read() {
    after_file_open(
        Change::Revoke,
        Some(("if-none-match", format!("\"{}\"", Hash::of(CONTENT)))),
    )
    .await;
    after_file_open(Change::Revoke, Some(("range", "bytes=999999-".into()))).await;
}

#[tokio::test]
async fn snapshot_archive_rechecks_key_before_delivering_already_queued_future_pieces() {
    use http_body_util::BodyExt;
    let site = Site::new().await;
    // Two MiB of deterministic nonrepeating input exceeds the existing eight64-KiB queue pieces,
    // so the test observes continuation rather than a one-piece archive.
    let mut state = 0x243f_6a88_85a3_08d3_u64;
    let bytes = (0..2 * 1024 * 1024)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect::<Vec<_>>();
    let id = copied(&site, Some(site.thread), vec![("large.bin".into(), bytes)]).await;
    let key = keys::scoped(
        &site,
        site.vic.id(),
        vec![keys::read("SnapshotArchiveGet", site.environment)],
    )
    .await;
    let response = site
        .app()
        .call(SnapshotArchiveGet, &[&id])
        .token(&key.secret)
        .query(&SnapshotArchiveGetQuery {
            format: Some(SnapshotArchiveFormat::Zip),
        })
        .send_response()
        .await;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "the exact modern reader must be admitted first"
    );
    let mut body = response.into_body();
    let first = tokio::time::timeout(RENDEZVOUS, body.frame())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        first.data_ref().is_some_and(|bytes| !bytes.is_empty()),
        "actual first archive data"
    );
    keys::revoke(&site, &key).await;
    let next = tokio::time::timeout(RENDEZVOUS, body.frame())
        .await
        .unwrap();
    assert!(
        next.is_none() || next.is_some_and(|frame| frame.is_err()),
        "buffered future archive bytes must not borrow the initial key admission"
    );
    drop(body); // Existing receiver-loss cleanup stops the blocking writer.
}

async fn elsewhere(site: &Site, owner: &Who, home: Id) -> Id {
    let id = new_id();
    let mut conn = site.world.conn().await;
    let org: Id = sqlx::query_scalar("SELECT org_id FROM nodes WHERE id=$1")
        .bind(home)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    sqlx::query("INSERT INTO environments (id,name,owner_id,org_id,placement,home_node_id,workspace_root,machine_id,generation,status)
        VALUES ($1,'Other private source',$2,$3,'connected',$4,'/elsewhere',$5,1,'running')")
        .bind(id).bind(owner.id()).bind(org).bind(home).bind(site.machine)
        .execute(&mut *conn).await.unwrap();
    orch_db::queries::access::testing::add_entry(
        &mut conn,
        &orch_db::queries::access::PageKey::new("environment", id, "live"),
        "any",
        "access.own",
        ("user", Some(owner.id()), None),
        None,
        site.world.now(),
    )
    .await
    .unwrap();
    id
}

async fn move_thread(site: &Site, environment: Id) {
    sqlx::query("UPDATE threads SET environment_id=$2 WHERE node_id=$1")
        .bind(site.thread)
        .bind(environment)
        .execute(&site.world.db.pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn manual_snapshot_follows_current_thread_environment_and_full_keeps_its_own_origin_read() {
    let (site, id) = fixture().await;
    let destination = elsewhere(&site, &site.eve, site.eve.person.personal_org).await;
    move_thread(&site, destination).await;
    // Eve's normal current-place access intentionally returns a live origin ref without FilesView there.
    Reader::Get.assert_content(
        Reader::Get.request(&site, id, &site.eve.token).await,
        id,
        site.environment,
    );
    assert!(
        !Reader::Get
            .request(&site, id, &site.vic.token)
            .await
            .status
            .is_success()
    );
    for reader in Reader::BYTES {
        let old = keys::scoped(
            &site,
            site.eve.id(),
            vec![keys::read(reader.operation(), site.environment)],
        )
        .await;
        assert_eq!(
            reader.request(&site, id, &old.secret).await.status,
            StatusCode::FORBIDDEN
        );
        let current = keys::scoped(
            &site,
            site.eve.id(),
            vec![keys::read(reader.operation(), destination)],
        )
        .await;
        reader.assert_content(
            reader.request(&site, id, &current.secret).await,
            id,
            site.environment,
        );
    }
    let current_only = keys::scoped(
        &site,
        site.eve.id(),
        vec![keys::read("SnapshotGet", destination)],
    )
    .await;
    assert_eq!(
        Reader::Get
            .request(&site, id, &current_only.secret)
            .await
            .status,
        StatusCode::FORBIDDEN,
        "Full contains live origin EnvironmentRef content"
    );
    let different_operation = keys::scoped(
        &site,
        site.eve.id(),
        vec![
            keys::read("SnapshotGet", destination),
            keys::read("SnapshotFileList", site.environment),
        ],
    )
    .await;
    assert_eq!(
        Reader::Get
            .request(&site, id, &different_operation.secret)
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    let both = keys::scoped(
        &site,
        site.eve.id(),
        vec![
            keys::read("SnapshotGet", destination),
            keys::read("SnapshotGet", site.environment),
        ],
    )
    .await;
    Reader::Get.assert_content(
        Reader::Get.request(&site, id, &both.secret).await,
        id,
        site.environment,
    );
}

#[tokio::test]
async fn private_export_stays_maker_only_and_reads_retained_environment_identity_after_removal() {
    let site = Site::new().await;
    let id = copied(&site, None, vec![(FILE.into(), CONTENT.into())]).await;
    for reader in Reader::ALL {
        assert_eq!(
            reader.request(&site, id, &site.vic.token).await.status,
            StatusCode::NOT_FOUND
        );
    }
    let mut issued = Vec::new();
    for reader in Reader::BYTES {
        reader.assert_content(
            reader.request(&site, id, &site.ada.token).await,
            id,
            site.environment,
        );
        let exact = keys::scoped(
            &site,
            site.ada.id(),
            vec![keys::read(reader.operation(), site.environment)],
        )
        .await;
        let subtree = keys::scoped(
            &site,
            site.ada.id(),
            vec![keys::rule(
                reader.operation(),
                "org",
                site.org,
                "subtree",
                &["read"],
            )],
        )
        .await;
        issued.push((reader, exact, subtree));
    }
    sqlx::query("UPDATE environments SET deleted_at=$2,status='removed' WHERE id=$1")
        .bind(site.environment)
        .bind(site.world.now())
        .execute(&site.world.db.pool)
        .await
        .unwrap();
    for (reader, exact, subtree) in issued {
        reader.assert_content(
            reader.request(&site, id, &site.ada.token).await,
            id,
            site.environment,
        );
        reader.assert_content(
            reader.request(&site, id, &exact.secret).await,
            id,
            site.environment,
        );
        reader.assert_content(
            reader.request(&site, id, &subtree.secret).await,
            id,
            site.environment,
        );
        let colleague = keys::scoped(
            &site,
            site.vic.id(),
            vec![keys::read(reader.operation(), site.environment)],
        )
        .await;
        assert_eq!(
            reader.request(&site, id, &colleague.secret).await.status,
            StatusCode::NOT_FOUND,
            "same key target cannot borrow the maker's private custody"
        );
    }
}

#[path = "delete_scopes.rs"]
mod deletion;

#[path = "update_scopes.rs"]
mod update;

#[path = "list_scopes.rs"]
mod list;
