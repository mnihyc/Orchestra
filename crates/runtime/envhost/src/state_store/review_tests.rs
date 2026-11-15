use super::*;
use crate::blobs::MemoryBlobs;
use crate::environment::{GateState, Gates, Limits};
use crate::file_states::{FileStates, RecordOperation};
use crate::files::Files;
use crate::ignore::IgnoreRules;
use crate::index::{HashCache, Index};
use crate::paths::Workspace;
use orch_link_contract::commands::{CommandKind, IndexMode};
use orch_link_contract::source::FsRecord;
use orch_types::new_id;

type TestResult<T = ()> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
const PATIENCE: std::time::Duration = std::time::Duration::from_secs(30);

struct Capture {
    files: Files,
    index: Index,
    states: FileStates,
    control: Arc<DirectoryControl>,
    now: Time,
}

impl Capture {
    fn at(root: &Path) -> TestResult<Self> {
        let workspace = root.join("workspace");
        std::fs::create_dir(&workspace)?;
        let control = Arc::new(DirectoryControl::default());
        let store = ObjectStore::open_with(
            &root.join("kept"),
            &workspace,
            DirectoryDurability {
                control: Some(Arc::clone(&control)),
            },
        )?;
        let files = Files::workspace(
            Workspace::open(&workspace, &[], PathStyle::Posix, true)?,
            Arc::new(IgnoreRules::new(&[], true)),
            Arc::new(HashCache::new()),
            Limits::default(),
            Arc::new(GateState::new(Gates::OPEN)),
        );
        Ok(Self {
            files,
            index: Index::new(
                IndexMode::Full,
                orch_link_contract::commands::WorkspaceLimits::default().index_entries,
            ),
            states: FileStates::new(store),
            control,
            now: "2026-10-16T00:00:00Z".parse()?,
        })
    }
    fn prepare(&self, bytes: &[u8]) -> TestResult {
        std::fs::write(self.files.root().root_path().join("code.bin"), bytes)?;
        self.index.scan(
            self.files.root(),
            self.files.ignore().ok_or("ignore rules absent")?,
        )?;
        self.states.refresh(&self.files, &self.index, None, true)?;
        self.states.prepare_recordings(&self.files, 1, self.now)?;
        Ok(())
    }
    fn request(&self) -> FsRecord {
        FsRecord {
            environment_id: new_id(),
            snapshot_id: new_id(),
            directory: String::new(),
            wait_for_indexing: true,
            kind: RecordKind::Turn,
            record_days: 1,
            max_bytes: 1024 * 1024,
        }
    }
    fn operation(&self) -> RecordOperation {
        RecordOperation {
            id: new_id(),
            until: Time::new(
                self.now.get()
                    + chrono::Duration::seconds(i64::from(CommandKind::FsRecord.deadline_secs())),
            ),
            cancel: tokio_util::sync::CancellationToken::new(),
        }
    }
    fn events(&self) -> Vec<DirectoryEvent> {
        self.control
            .events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
    fn clear(&self) {
        self.control
            .events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }
    fn refuse(&self, path: &Path) {
        *self
            .control
            .refuse
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(path.to_owned());
    }
}

fn at(events: &[DirectoryEvent], event: &DirectoryEvent) -> TestResult<usize> {
    events
        .iter()
        .position(|actual| actual == event)
        .ok_or_else(|| format!("missing directory barrier event: {event:?}").into())
}

#[test]
fn fresh_store_ancestry_is_flushed_before_descendants_and_a_failed_edge_is_repaired_on_retry()
-> TestResult {
    let dir = tempfile::tempdir()?;
    let base = std::fs::canonicalize(dir.path())?;
    let workspace = base.join("workspace");
    std::fs::create_dir(&workspace)?;
    let root = base.join("native/data/kept");
    let control = Arc::new(DirectoryControl::default());
    *control
        .refuse
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = Some(base.clone());
    let durability = DirectoryDurability {
        control: Some(Arc::clone(&control)),
    };
    assert!(ObjectStore::open_with(&root, &workspace, durability.clone()).is_err());
    assert!(base.join("native").is_dir());
    assert!(!base.join("native/data").exists());
    let failed = control
        .events
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    assert!(
        at(&failed, &DirectoryEvent::Created(base.join("native")))?
            < at(&failed, &DirectoryEvent::Sync(base.clone()))?
    );
    assert!(!failed.contains(&DirectoryEvent::Synced(base.clone())));
    let store = ObjectStore::open_with(&root, &workspace, durability)?;
    let events = control
        .events
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    assert!(
        at(&events, &DirectoryEvent::Synced(base.clone()))?
            < at(&events, &DirectoryEvent::Created(base.join("native/data")))?
    );
    assert!(
        at(&events, &DirectoryEvent::Synced(base.join("native")))?
            < at(&events, &DirectoryEvent::Created(root.clone()))?
    );
    assert!(
        at(&events, &DirectoryEvent::Synced(base.join("native/data")))?
            < at(&events, &DirectoryEvent::Created(root.join("git")))?
    );
    assert!(store.root().join("git/HEAD").is_file() && store.root().join("git/config").is_file());
    Ok(())
}

#[test]
fn fresh_object_fanout_requires_its_parent_barrier_before_publishing_an_object() -> TestResult {
    let dir = tempfile::tempdir()?;
    let capture = Capture::at(dir.path())?;
    let store = capture.states.store();
    let raw = b"immutable raw\0\xffbytes";
    let mut hash = Sha1::new();
    hash.update(format!("blob {}\0", raw.len()).as_bytes());
    hash.update(raw);
    let oid = hex::encode(hash.finalize());
    let path = store.object_path(&oid)?;
    let fanout = path.parent().ok_or("fanout absent")?.to_owned();
    let objects = store.root().join("git/objects");
    capture.clear();
    capture.refuse(&objects);
    assert!(store.write_object("blob", raw).is_err());
    assert!(fanout.is_dir());
    assert!(!path.exists());
    let events = capture.events();
    assert!(
        at(&events, &DirectoryEvent::Created(fanout.clone()))?
            < at(&events, &DirectoryEvent::Sync(objects.clone()))?
    );
    assert!(!events.contains(&DirectoryEvent::Published(path.clone())));
    capture.clear();
    assert_eq!(store.write_object("blob", raw)?, oid);
    let events = capture.events();
    assert!(
        at(&events, &DirectoryEvent::Synced(objects))?
            < at(&events, &DirectoryEvent::Published(path.clone()))?
    );
    assert!(
        at(&events, &DirectoryEvent::Published(path))?
            < at(&events, &DirectoryEvent::Synced(fanout))?
    );
    let reopened = ObjectStore::open(store.root(), capture.files.root().root_path())?;
    let verified = reopened.verify_object(
        &oid,
        "blob",
        Some(i64::try_from(raw.len())?),
        Some(Hash::of(raw)),
    )?;
    assert_eq!(std::fs::read(verified.path())?, raw);
    Ok(())
}

#[tokio::test]
async fn first_snapshot_pin_parent_refusal_prevents_capture_handoff_and_retry_keeps_original_bytes()
-> TestResult {
    let dir = tempfile::tempdir()?;
    let capture = Capture::at(dir.path())?;
    capture.prepare(b"original\0bytes")?;
    let request = capture.request();
    let operation = capture.operation();
    let store = capture.states.store();
    let pins = store.root().join("pins");
    let snapshot = pins.join(request.snapshot_id.to_string());
    capture.clear();
    capture.refuse(&pins);
    assert!(
        capture
            .states
            .capture(&capture.files, &request, 1, capture.now, &operation)
            .is_err()
    );
    assert!(snapshot.is_dir());
    assert!(!snapshot.join(operation.id.to_string()).exists());
    assert!(store.require_idle().is_ok());
    let frozen = store
        .recorded_reply(&request)?
        .ok_or("frozen capture reply absent")?;
    assert_eq!(frozen.state, RecordState::Complete);
    let failed = capture.events();
    assert!(
        at(&failed, &DirectoryEvent::Created(snapshot.clone()))?
            < at(&failed, &DirectoryEvent::Sync(pins.clone()))?
    );
    capture.prepare(b"different live workspace after failed handoff")?;
    capture.clear();
    let retry = capture.operation();
    let captured = capture
        .states
        .capture(&capture.files, &request, 1, capture.now, &retry)?;
    assert_eq!(captured.result, frozen);
    let events = capture.events();
    let pin = snapshot.join(retry.id.to_string());
    assert!(
        at(&events, &DirectoryEvent::Synced(pins))?
            < at(&events, &DirectoryEvent::Published(pin.clone()))?
    );
    assert!(
        at(&events, &DirectoryEvent::Published(pin))?
            < at(&events, &DirectoryEvent::Synced(snapshot))?
    );
    let blobs = MemoryBlobs::new();
    assert_eq!(captured.publish(&blobs).await?, frozen);
    let hash = frozen.manifest_blob.ok_or("captured hash absent")?;
    let parsed = manifest::parse(&blobs.get_bytes(&hash).ok_or("published manifest absent")?)?;
    assert_eq!(
        parsed
            .entries
            .iter()
            .find(|entry| entry.p == "code.bin")
            .and_then(|entry| entry.h),
        Some(Hash::of(b"original\0bytes"))
    );
    Ok(())
}

#[test]
fn a_root_visible_after_failed_directory_flush_is_republished_durably_before_recovered_handoff()
-> TestResult {
    let dir = tempfile::tempdir()?;
    let capture = Capture::at(dir.path())?;
    capture.prepare(b"original before root flush")?;
    let request = capture.request();
    let operation = capture.operation();
    let store = capture.states.store();
    capture.refuse(&store.root().join("roots"));
    assert!(
        capture
            .states
            .capture(&capture.files, &request, 1, capture.now, &operation)
            .is_err()
    );
    assert!(store.has_root(request.snapshot_id));
    assert!(store.recorded_reply(&request)?.is_none());
    let (root, _) = store.load(request.snapshot_id, capture.now)?;
    let frozen = root.result();
    capture.prepare(b"newer workspace must not replace the visible root")?;
    capture.clear();
    let captured = capture.states.capture(
        &capture.files,
        &request,
        1,
        capture.now,
        &capture.operation(),
    )?;
    assert_eq!(captured.result, frozen);
    let events = capture.events();
    let root_path = store
        .root()
        .join("roots")
        .join(format!("{}.json", request.snapshot_id));
    let reply_path = store
        .root()
        .join("results")
        .join(format!("{}.json", request.snapshot_id));
    assert!(
        at(&events, &DirectoryEvent::Published(root_path))?
            < at(&events, &DirectoryEvent::Synced(store.root().join("roots")))?
    );
    assert!(
        at(&events, &DirectoryEvent::Synced(store.root().join("roots")))?
            < at(&events, &DirectoryEvent::Published(reply_path))?
    );
    drop(captured);
    let reopened = ObjectStore::open(store.root(), capture.files.root().root_path())?;
    assert_eq!(
        reopened.load(request.snapshot_id, capture.now)?.0.result(),
        frozen
    );
    Ok(())
}

#[test]
fn a_cached_reply_does_not_bypass_a_failed_directory_barrier() -> TestResult {
    let dir = tempfile::tempdir()?;
    let capture = Capture::at(dir.path())?;
    capture.prepare(b"original reply bytes")?;
    let request = capture.request();
    let operation = capture.operation();
    let store = capture.states.store();
    let captured = capture
        .states
        .capture(&capture.files, &request, 1, capture.now, &operation)?;
    let frozen = captured.result.clone();
    drop(captured);
    let results = store.root().join("results");
    capture.refuse(&results);
    assert!(
        store.keep_reply(&request, &frozen).is_err(),
        "a visible rename is not a successful durable write"
    );
    capture.refuse(&results);
    assert!(
        capture
            .states
            .capture(
                &capture.files,
                &request,
                1,
                capture.now,
                &capture.operation()
            )
            .is_err(),
        "the cached name must not bypass the refused durability barrier"
    );
    assert!(store.require_idle().is_ok());
    let retry = capture.states.capture(
        &capture.files,
        &request,
        1,
        capture.now,
        &capture.operation(),
    )?;
    assert_eq!(retry.result, frozen);
    drop(retry);
    Ok(())
}

#[test]
fn destructive_admission_excludes_pins_and_retirement_never_revives_old_store_clones() -> TestResult
{
    let dir = tempfile::tempdir()?;
    let capture = Capture::at(dir.path())?;
    let store = capture.states.store();
    let old = store.clone();
    let reopened = ObjectStore::open(store.root(), capture.files.root().root_path())?;
    let request = capture.request();
    let operation = capture.operation();
    let pin = old.pin(request.snapshot_id, operation.id, operation.until)?;
    assert!(matches!(
        store.reserve_destructive(),
        Err(HostError::Busy(_))
    ));
    drop(pin);
    let exclusive = store.reserve_destructive()?;
    assert!(matches!(
        old.pin(request.snapshot_id, operation.id, operation.until),
        Err(HostError::Busy(_))
    ));
    assert!(matches!(
        reopened.pin(request.snapshot_id, operation.id, operation.until),
        Err(HostError::Busy(_))
    ));
    assert!(matches!(
        reopened.reserve_destructive(),
        Err(HostError::Busy(_))
    ));
    exclusive.retire();
    drop(exclusive);
    assert!(matches!(
        old.pin(request.snapshot_id, operation.id, operation.until),
        Err(HostError::NotFound(_))
    ));
    assert!(matches!(
        store.reserve_destructive(),
        Err(HostError::NotFound(_))
    ));
    let pin = reopened.pin(request.snapshot_id, operation.id, operation.until)?;
    drop(pin);
    Ok(())
}

struct ReleaseMutation(Option<std::sync::mpsc::Sender<()>>);
impl Drop for ReleaseMutation {
    fn drop(&mut self) {
        if let Some(sender) = self.0.take() {
            let _ = sender.send(());
        }
    }
}

#[tokio::test]
async fn replacing_an_attachment_keeps_an_admitted_publication_usable_and_its_pin_bounds_the_new_handle()
-> TestResult {
    let dir = tempfile::tempdir()?;
    let capture = Capture::at(dir.path())?;
    capture.prepare(b"admitted original bytes")?;
    let request = capture.request();
    let store = capture.states.store();
    let captured = capture.states.capture(
        &capture.files,
        &request,
        1,
        capture.now,
        &capture.operation(),
    )?;
    let result = captured.result.clone();
    let next = ObjectStore::open(store.root(), capture.files.root().root_path())?;
    store.retire_attachment();
    assert!(matches!(
        store.pin(request.snapshot_id, new_id(), capture.operation().until),
        Err(HostError::NotFound(_))
    ));
    assert!(
        matches!(next.reserve_destructive(), Err(HostError::Busy(_))),
        "the old admitted pin still protects the physical store"
    );
    let blobs = MemoryBlobs::new();
    assert_eq!(captured.publish(&blobs).await?, result);
    let hash = result.manifest_blob.ok_or("manifest absent")?;
    assert!(
        blobs.get_bytes(&hash).is_some(),
        "retirement does not cancel an admitted immutable publication"
    );
    let reservation = next.reserve_destructive()?;
    drop(reservation);
    Ok(())
}

#[tokio::test]
async fn blocking_materialization_files_keep_the_reservation_after_the_callers_files_are_dropped()
-> TestResult {
    let dir = tempfile::tempdir()?;
    let capture = Capture::at(dir.path())?;
    let store = capture.states.store();
    let files = crate::data_transfer::files(store.root(), &capture.files)?
        .with_recorded_operation(Arc::new(store.reserve_destructive()?));
    let worker_files = files.clone();
    let request = capture.request();
    let operation = capture.operation();
    let (release, held) = std::sync::mpsc::channel();
    let (entered, began) = tokio::sync::oneshot::channel();
    let worker = tokio::task::spawn_blocking(move || -> std::io::Result<()> {
        let _ = entered.send(());
        held.recv().map_err(std::io::Error::other)?;
        std::fs::write(
            worker_files.root().root_path().join("results/mutation"),
            b"finished",
        )
    });
    let mut release = ReleaseMutation(Some(release));
    tokio::time::timeout(PATIENCE, began).await??;
    drop(files); // The async caller ended; its outstanding native closure still owns a clone.
    assert!(matches!(
        store.pin(request.snapshot_id, operation.id, operation.until),
        Err(HostError::Busy(_))
    ));
    release.0.take().ok_or("release sender absent")?.send(())?;
    tokio::time::timeout(PATIENCE, worker).await???;
    assert_eq!(
        std::fs::read(store.root().join("results/mutation"))?,
        b"finished"
    );
    let pin = store.pin(request.snapshot_id, operation.id, operation.until)?;
    drop(pin);
    Ok(())
}

#[derive(Serialize, Deserialize)]
struct CrashReceipt {
    request: FsRecord,
    result: FsRecordResult,
    until: Time,
}

#[test]
fn captured_state_and_crash_pin_reopen_after_abrupt_exit() -> TestResult {
    const CHILD_ROOT: &str = "ORCH_RECORDING_CRASH_PROOF_ROOT";
    if let Some(root) = std::env::var_os(CHILD_ROOT) {
        let root = PathBuf::from(root);
        let capture = Capture::at(&root)?;
        capture.prepare(b"crash-safe original\0\xff")?;
        let request = capture.request();
        let operation = capture.operation();
        let captured =
            capture
                .states
                .capture(&capture.files, &request, 1, capture.now, &operation)?;
        let receipt = CrashReceipt {
            request,
            result: captured.result.clone(),
            until: operation.until,
        };
        atomic_write(&root.join("handoff.json"), &serde_json::to_vec(&receipt)?)?;
        // Skip StatePin/fixture destructors: parent must observe the durable-only pin and raw state.
        std::process::exit(0);
    }
    let dir = tempfile::tempdir()?;
    let status = std::process::Command::new(std::env::current_exe()?)
        .args([
            "--exact",
            "state_store::review_tests::captured_state_and_crash_pin_reopen_after_abrupt_exit",
            "--test-threads=1",
        ])
        .env(CHILD_ROOT, dir.path())
        .status()?;
    assert!(status.success(), "native capture child failed: {status}");
    let receipt: CrashReceipt =
        serde_json::from_slice(&std::fs::read(dir.path().join("handoff.json"))?)?;
    let store = ObjectStore::open(&dir.path().join("kept"), &dir.path().join("workspace"))?;
    let before = Time::new(receipt.until.get() - chrono::Duration::seconds(1));
    let after = Time::new(receipt.until.get() + chrono::Duration::seconds(1));
    assert!(
        store.require_idle().is_ok(),
        "the crashed process has no live reservation"
    );
    assert!(store.pinned(receipt.request.snapshot_id, before)?);
    assert!(!store.pinned(receipt.request.snapshot_id, after)?);
    let (root, parsed) = store.load(receipt.request.snapshot_id, before)?;
    assert_eq!(root.result(), receipt.result);
    let entry = parsed
        .entries
        .iter()
        .find(|entry| entry.p == "code.bin")
        .ok_or("captured file absent")?;
    assert_eq!(
        std::fs::read(store.read_entry(&root, entry)?.path())?,
        b"crash-safe original\0\xff"
    );
    // This is abrupt process recovery; injected barrier-order tests establish the required fsync
    // calls. It does not simulate loss of the kernel cache or certify non-Unix power loss.
    Ok(())
}

#[test]
fn a_baseline_flushes_its_objects_and_shared_directories_before_a_root_names_them() -> TestResult {
    let dir = tempfile::tempdir()?;
    let capture = Capture::at(dir.path())?;
    let workspace = capture.files.root().root_path().to_owned();
    for number in 0..40 {
        std::fs::write(
            workspace.join(format!("file-{number}.txt")),
            format!("the bytes of file {number}"),
        )?;
    }
    capture.index.scan(
        capture.files.root(),
        capture.files.ignore().ok_or("ignore rules absent")?,
    )?;
    capture.clear();
    capture
        .states
        .refresh(&capture.files, &capture.index, None, true)?;
    let flushes = capture
        .events()
        .iter()
        .filter(|event| {
            **event == DirectoryEvent::Sync(capture.states.store().root().join("git/objects"))
        })
        .count();
    assert_eq!(
        flushes, 1,
        "the shared object directory is flushed once per batch"
    );
    assert_eq!(
        capture
            .events()
            .iter()
            .filter(|event| matches!(event, DirectoryEvent::FileSynced(_)))
            .count(),
        40,
        "each retained file is flushed without a filesystem-wide barrier"
    );
    let store = capture.states.store();
    assert!(
        store.deferred_written() >= 40,
        "every file was read and kept"
    );
    assert_eq!(
        (store.flushes(), store.unsynced_objects()),
        (1, 0),
        "one flush for the whole pass"
    );
    capture
        .states
        .prepare_recordings(&capture.files, 1, capture.now)?;
    let request = capture.request();
    let captured = capture.states.capture(
        &capture.files,
        &request,
        1,
        capture.now,
        &capture.operation(),
    )?;
    assert_eq!(captured.result.files, 40);
    assert_eq!(
        (store.flushes(), store.unsynced_objects()),
        (2, 0),
        "the trees of the state were flushed once, before its root was kept"
    );
    let (root, parsed) = store.load(request.snapshot_id, capture.now)?;
    assert_eq!(parsed.entries.len(), 40);
    for entry in parsed
        .entries
        .iter()
        .filter(|entry| entry.t != EntryKind::Dir)
    {
        assert_eq!(
            std::fs::read(store.read_entry(&root, entry)?.path())?,
            std::fs::read(workspace.join(&entry.p))?
        );
    }
    Ok(())
}

#[test]
fn a_heavy_writer_beside_the_store_does_not_join_a_recordings_flush() -> TestResult {
    use std::sync::atomic::AtomicUsize;
    use std::time::Instant;
    let dir = tempfile::tempdir()?;
    let capture = Capture::at(dir.path())?;
    let store = capture.states.store();
    let mut kept = Vec::new();
    for number in 0..40 {
        let oid = store.write_object_deferred("blob", format!("saved {number}").as_bytes())?;
        kept.push(store.object_path(&oid)?);
    }
    capture.clear();
    let stop = AtomicBool::new(false);
    let writes = AtomicUsize::new(0);
    let unrelated = dir.path().join("building-beside-the-store");
    let (ready, writing) = std::sync::mpsc::channel();
    std::thread::scope(|scope| -> TestResult {
        let writer = scope.spawn(|| -> TestResult {
            use std::io::{Seek, SeekFrom};
            let mut file = File::create(&unrelated)?;
            let bytes = vec![1u8; 1024 * 1024];
            file.write_all(&bytes)?;
            ready.send(())?;
            let began = Instant::now();
            while !stop.load(Ordering::SeqCst) && began.elapsed() < PATIENCE {
                file.seek(SeekFrom::Start(0))?;
                file.write_all(&bytes)?;
                writes.fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        });
        let result = (|| -> TestResult {
            writing.recv_timeout(PATIENCE)?;
            let began = Instant::now();
            store.make_durable()?;
            assert!(
                began.elapsed() < PATIENCE,
                "recording waited for the unrelated writer"
            );
            let events = capture.events();
            for path in &kept {
                assert!(events.contains(&DirectoryEvent::FileSynced(path.clone())));
            }
            assert!(
                events.iter().all(|event| match event {
                    DirectoryEvent::Sync(path) | DirectoryEvent::FileSynced(path) =>
                        path.starts_with(store.root()),
                    _ => true,
                }),
                "only the store's own files and directory handles belong to the barrier"
            );
            assert_eq!(store.unsynced_objects(), 0);
            Ok(())
        })();
        stop.store(true, Ordering::SeqCst);
        writer.join().map_err(|_| "the beside writer panicked")??;
        result
    })?;
    Ok(())
}

#[test]
fn failed_and_concurrent_flushes_keep_the_same_pending_objects_until_they_are_durable() -> TestResult
{
    let dir = tempfile::tempdir()?;
    let capture = Capture::at(dir.path())?;
    let store = capture.states.store();
    let oid = store.write_object_deferred("blob", b"durable after retry")?;
    let path = store.object_path(&oid)?;
    capture.refuse(&path);
    assert!(store.make_durable().is_err());
    assert_eq!(
        store.unsynced_objects(),
        1,
        "a failed batch never loses its debt"
    );
    std::thread::scope(|scope| -> TestResult {
        let other = scope.spawn(|| store.make_durable());
        store.make_durable()?;
        other.join().map_err(|_| "the second flush panicked")??;
        Ok(())
    })?;
    assert_eq!((store.flushes(), store.unsynced_objects()), (1, 0));
    store.verify_object(
        &oid,
        "blob",
        Some(i64::try_from(b"durable after retry".len())?),
        Some(Hash::of(b"durable after retry")),
    )?;
    Ok(())
}

#[test]
fn a_verified_object_is_never_replaced_under_an_existing_recording() -> TestResult {
    let dir = tempfile::tempdir()?;
    let capture = Capture::at(dir.path())?;
    let store = capture.states.store();
    let oid = store.write_object_deferred("blob", b"the same retained bytes")?;
    store.make_durable()?;
    let path = store.object_path(&oid)?;
    let before = platform::file_meta(&File::open(&path)?)?;
    store.write_object_deferred("blob", b"the same retained bytes")?;
    assert_eq!(
        platform::file_meta(&File::open(&path)?)?.identity(),
        before.identity(),
        "a later baseline cannot replace a durable inode with unflushed identical bytes"
    );
    store.make_durable()?;
    Ok(())
}

#[test]
fn a_recording_barrier_flushes_pending_directories_even_when_its_files_are_already_durable()
-> TestResult {
    let dir = tempfile::tempdir()?;
    let capture = Capture::at(dir.path())?;
    let store = capture.states.store();
    store.write_object_deferred("blob", b"kept by another capture")?;
    store.write_object("blob", b"kept by another capture")?;
    assert_eq!(store.unsynced_objects(), 0);
    capture.clear();
    store.make_durable()?;
    assert!(
        capture
            .events()
            .contains(&DirectoryEvent::Synced(store.root().join("git/objects")))
    );
    Ok(())
}

#[test]
fn a_full_refresh_keeps_the_preparation_owed_to_a_requested_restore_point() -> TestResult {
    use crate::file_states::Work;
    let dir = tempfile::tempdir()?;
    let capture = Capture::at(dir.path())?;
    capture.prepare(b"unchanged file bytes")?;
    let request = capture.request();
    capture.states.request(&RelPath::root(), request.max_bytes);
    assert!(matches!(capture.states.take_work(), Work::RecordMetadata));
    capture
        .states
        .prepare_recordings(&capture.files, 1, capture.now)?;
    capture.states.want_everything();
    assert!(matches!(capture.states.take_work(), Work::Everything));
    capture
        .states
        .reconcile(&capture.files, &capture.index, true)?;
    if matches!(capture.states.take_work(), Work::RecordMetadata) {
        capture
            .states
            .prepare_recordings(&capture.files, 1, capture.now)?;
    }
    let captured = capture.states.capture(
        &capture.files,
        &request,
        1,
        capture.now,
        &capture.operation(),
    )?;
    assert_eq!(
        captured.result.state,
        RecordState::Complete,
        "{:?}",
        captured.result
    );
    Ok(())
}

#[test]
fn a_ready_turn_capture_reuses_prepared_metadata_without_touching_the_object_tree() -> TestResult {
    let dir = tempfile::tempdir()?;
    let capture = Capture::at(dir.path())?;
    capture.prepare(b"ready file bytes")?;
    capture
        .states
        .prepare_recordings(&capture.files, 1, capture.now)?;
    let store = capture.states.store();
    let written = store.deferred_written();
    capture.clear();
    capture.refuse(&store.root().join("git/objects"));
    let request = capture.request();
    let captured = capture.states.capture(
        &capture.files,
        &request,
        1,
        capture.now,
        &capture.operation(),
    )?;
    assert_eq!(captured.result.state, RecordState::Complete);
    assert_eq!(
        store.deferred_written(),
        written,
        "the turn builds no trees"
    );
    assert!(capture.events().iter().all(|event| match event {
        DirectoryEvent::Sync(path) => !path.starts_with(store.root().join("git/objects")),
        _ => true,
    }));
    drop(captured);
    let (root, _) = store.load(request.snapshot_id, capture.now)?;
    assert!(
        root.objects.contains_key("code.bin"),
        "explicit reads still get the complete saved namespace"
    );
    Ok(())
}

#[test]
fn writes_excluded_by_gitignore_keep_the_prepared_turn_recording_ready() -> TestResult {
    let dir = tempfile::tempdir()?;
    let capture = Capture::at(dir.path())?;
    std::fs::write(
        capture.files.root().root_path().join(".gitignore"),
        "build/\n",
    )?;
    capture.prepare(b"included source")?;
    let path = capture.files.parse("build/output.txt")?;
    capture.states.changed(&[path]);
    let result = capture.states.capture(
        &capture.files,
        &capture.request(),
        1,
        capture.now,
        &capture.operation(),
    )?;
    assert_eq!(
        result.result.state,
        RecordState::Complete,
        "an excluded build output cannot invalidate the included files or force their tree to be rebuilt"
    );
    Ok(())
}
