//! Native initial-snapshot boundaries without a timing-based interleaving.

use super::*;
use crate::blobs::MemoryBlobs;
use orch_link_contract::commands::IndexMode;
use orch_types::{SystemClock, new_id};
use tokio::sync::oneshot;

type Proof<T = ()> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
const PATIENCE: Duration = Duration::from_secs(15);

struct Boundary {
    entered: oneshot::Sender<()>,
    resume: oneshot::Receiver<()>,
}

tokio::task_local! {
    static BEFORE_SNAPSHOT: Mutex<Option<Boundary>>;
}

pub(super) async fn before_snapshot() {
    let boundary = BEFORE_SNAPSHOT
        .try_with(|slot| lock(slot).take())
        .ok()
        .flatten();
    if let Some(boundary) = boundary {
        let _ = boundary.entered.send(());
        let _ = boundary.resume.await;
    }
}

struct Rig {
    host: EnvHost,
    environment: Id,
    root: PathBuf,
    reports: mpsc::Receiver<Reported>,
    _directory: tempfile::TempDir,
}

impl Rig {
    async fn new() -> Proof<Self> {
        let directory = tempfile::tempdir()?;
        let root = directory.path().join("workspace");
        std::fs::create_dir(&root)?;
        let settings =
            HostSettings::connected(directory.path().join("connector"), vec![root.clone()]);
        let (reports, receiver) = Reports::channel(256);
        let host = EnvHost::new(
            settings,
            Arc::new(SystemClock),
            reports,
            Arc::new(MemoryBlobs::new()),
            Vec::new(),
        );
        let environment = new_id();
        host.env_attach(
            &EnvAttach {
                resources: None,
                data_generation: None,
                storage: None,
                data_sources: None,
                environment_id: environment,
                workspace_root: Some(root.to_string_lossy().into_owned()),
                ignore: Vec::new(),
                index: IndexMode::Live,
                mounts: None,
                variables: BTreeMap::new(),
                secrets: BTreeMap::new(),
            },
            &BTreeMap::new(),
        )
        .await?;
        Ok(Self {
            host,
            environment,
            root,
            reports: receiver,
            _directory: directory,
        })
    }

    fn request(&self, watch: Id) -> FsList {
        FsList {
            environment_id: self.environment,
            path: String::new(),
            mount: None,
            watch_id: Some(watch),
            watch_instance: None,
            page: None,
        }
    }

    fn paused(&self, watch: Id) -> Paused {
        let host = EnvHost {
            inner: Arc::clone(&self.host.inner),
        };
        let instance = new_id();
        let mut body = self.request(watch);
        body.watch_instance = Some(instance);
        let (entered, reached) = oneshot::channel();
        let (resume, blocked) = oneshot::channel();
        let task = tokio::spawn(BEFORE_SNAPSHOT.scope(
            Mutex::new(Some(Boundary {
                entered,
                resume: blocked,
            })),
            async move { host.fs_list(&body).await },
        ));
        Paused {
            task,
            reached,
            resume,
            instance,
        }
    }

    async fn changed(&mut self, watch: Id) -> Proof<FsChanged> {
        tokio::time::timeout(PATIENCE, async {
            loop {
                let report = self.reports.recv().await.ok_or("directory reports ended")?;
                if let Report::FsChanged(changed) = report.report
                    && changed.watch_id == watch
                {
                    return Ok(changed);
                }
            }
        })
        .await?
    }
}

struct Paused {
    task: tokio::task::JoinHandle<Result<FsListResult>>,
    reached: oneshot::Receiver<()>,
    resume: oneshot::Sender<()>,
    instance: Id,
}

#[tokio::test]
async fn native_watch_is_installed_before_snapshot_and_reset_precedes_later_changes() -> Proof {
    let mut rig = Rig::new().await?;
    let watch = new_id();
    let mut reading = rig.paused(watch);
    tokio::time::timeout(PATIENCE, &mut reading.reached).await??;
    let environment = rig.host.environment(rig.environment)?;
    assert!(
        lock(&environment.watches)
            .get(&watch)
            .is_some_and(|registered| registered.instance == reading.instance
                && registered.wire_instance == Some(reading.instance))
    );
    assert!(
        !lock(&environment.watcher)
            .as_ref()
            .ok_or("watcher absent")?
            .is_degraded()
    );
    // One atomic arrival gives the native admission a causal boundary before capture starts.
    // Creating and closing the file in place can otherwise deliver a later event during capture.
    let staged = rig.root.with_file_name("initial-entry");
    std::fs::write(&staged, b"first")?;
    let cache = &rig.host.inner.directory_pages;
    let before = cache.testing.invalidations.load(Ordering::Acquire);
    std::fs::rename(staged, rig.root.join("between.txt"))?;
    cache.testing.invalidated(before + 1).await?;
    reading
        .resume
        .send(())
        .map_err(|()| "the reader ended before its capture resumed")?;
    let first = tokio::time::timeout(PATIENCE, reading.task).await???;
    assert_eq!(first.watch_instance, Some(reading.instance));
    assert!(
        first
            .entries
            .iter()
            .any(|entry| entry.name == "between.txt")
    );
    let reset = rig.changed(watch).await?;
    assert!(reset.reset);
    assert_eq!(reset.watch_instance, Some(reading.instance));
    assert!(
        reset
            .changed
            .iter()
            .any(|entry| entry.name == "between.txt")
    );
    std::fs::write(rig.root.join("between.txt"), b"a later native write")?;
    let delta = rig.changed(watch).await?;
    assert!(!delta.reset);
    assert_eq!(delta.watch_instance, Some(reading.instance));
    assert!(
        delta
            .changed
            .iter()
            .any(|entry| entry.name == "between.txt" && entry.size == 20)
    );
    Ok(())
}

#[tokio::test]
async fn failed_native_watch_activation_is_not_a_successful_empty_directory() -> Proof {
    let rig = Rig::new().await?;
    let environment = rig.host.environment(rig.environment)?;
    lock(&environment.watcher)
        .as_ref()
        .ok_or("watcher absent")?
        .stop();
    let watch = new_id();
    let result = rig.host.fs_list(&rig.request(watch)).await;
    assert!(matches!(result, Err(HostError::Unsupported(_))));
    assert!(!lock(&environment.watches).contains_key(&watch));
    Ok(())
}

#[tokio::test]
async fn a_change_outside_a_partial_watch_window_invalidates_without_a_prefix_diff() -> Proof {
    let mut rig = Rig::new().await?;
    std::fs::write(rig.root.join("a.txt"), b"first")?;
    std::fs::write(rig.root.join("z.txt"), b"outside")?;
    let watch = new_id();
    let mut body = rig.request(watch);
    body.page = Some(orch_link_contract::commands::FsListPage {
        cursor: None,
        limit: Some(1),
        generation: None,
    });
    let first = rig.host.fs_list(&body).await?;
    assert!(first.truncated);
    assert_eq!(first.entries[0].name, "a.txt");
    let reset = rig.changed(watch).await?;
    assert!(reset.reset);
    assert!(
        reset
            .page
            .as_ref()
            .is_some_and(|page| page.next_cursor.is_some())
    );
    std::fs::write(rig.root.join("z.txt"), b"changed outside the prefix")?;
    let invalidation = rig.changed(watch).await?;
    assert!(!invalidation.reset);
    assert!(invalidation.invalidated);
    assert!(invalidation.truncated);
    assert!(invalidation.changed.is_empty());
    assert!(invalidation.removed.is_empty());
    assert!(invalidation.page.is_none());
    Ok(())
}

#[tokio::test]
async fn unwatch_during_first_capture_cannot_restore_the_abandoned_registration() -> Proof {
    let rig = Rig::new().await?;
    let watch = new_id();
    let mut reading = rig.paused(watch);
    tokio::time::timeout(PATIENCE, &mut reading.reached).await??;
    rig.host.fs_unwatch(&FsUnwatch { watch_id: watch })?;
    reading
        .resume
        .send(())
        .map_err(|()| "the reader ended before unwatch completed")?;
    let result = tokio::time::timeout(PATIENCE, reading.task).await??;
    assert!(matches!(result, Err(HostError::Changed(_))));
    assert!(!lock(&rig.host.environment(rig.environment)?.watches).contains_key(&watch));
    Ok(())
}

#[tokio::test]
async fn cancellation_releases_only_the_pending_installation() -> Proof {
    let rig = Rig::new().await?;
    let kept = new_id();
    rig.host.fs_list(&rig.request(kept)).await?;
    let abandoned = new_id();
    let mut reading = rig.paused(abandoned);
    tokio::time::timeout(PATIENCE, &mut reading.reached).await??;
    reading.task.abort();
    let result = tokio::time::timeout(PATIENCE, reading.task).await?;
    assert!(matches!(result, Err(error) if error.is_cancelled()));
    let environment = rig.host.environment(rig.environment)?;
    let watches = lock(&environment.watches);
    assert!(!watches.contains_key(&abandoned));
    assert!(watches.contains_key(&kept));
    Ok(())
}

#[tokio::test]
async fn removing_one_logical_reader_keeps_another_native_directory_watch_live() -> Proof {
    let mut rig = Rig::new().await?;
    let first = new_id();
    let second = new_id();
    rig.host.fs_list(&rig.request(first)).await?;
    rig.host.fs_list(&rig.request(second)).await?;
    assert!(rig.changed(first).await?.reset);
    assert!(rig.changed(second).await?.reset);
    rig.host.fs_unwatch(&FsUnwatch { watch_id: first })?;
    std::fs::write(rig.root.join("still-live.txt"), b"kept")?;
    let delta = rig.changed(second).await?;
    assert!(!delta.reset);
    assert!(
        delta
            .changed
            .iter()
            .any(|entry| entry.name == "still-live.txt")
    );
    Ok(())
}

#[tokio::test]
async fn a_plain_observation_does_not_claim_live_activation_when_native_watching_is_unavailable()
-> Proof {
    let rig = Rig::new().await?;
    std::fs::write(rig.root.join("actual.txt"), b"actual")?;
    let environment = rig.host.environment(rig.environment)?;
    lock(&environment.watcher)
        .as_ref()
        .ok_or("watcher absent")?
        .stop();
    let watch = new_id();
    let mut plain = rig.request(watch);
    plain.watch_id = None;
    let observed = rig.host.fs_list(&plain).await?;
    assert_eq!(
        observed
            .entries
            .iter()
            .map(|row| row.name.as_str())
            .collect::<Vec<_>>(),
        ["actual.txt"]
    );
    let metadata = observed
        .page
        .ok_or("a modern observation carries page metadata")?;
    assert!(
        metadata
            .observed_from
            .is_some_and(|start| start <= metadata.as_of)
    );
    assert!(matches!(
        rig.host.fs_list(&rig.request(watch)).await,
        Err(HostError::Unsupported(_))
    ));
    assert!(!lock(&environment.watches).contains_key(&watch));
    assert!(lock(&environment.directory_captures).is_empty());
    Ok(())
}

#[tokio::test]
async fn http_and_watch_first_reads_share_only_a_registration_covered_native_observation_in_either_order()
-> Proof {
    for watch_first in [false, true] {
        let mut rig = Rig::new().await?;
        for name in ["a", "b"] {
            std::fs::write(rig.root.join(name), name)?;
        }
        let cache = Arc::clone(&rig.host.inner.directory_pages);
        let mut held = cache.testing.hold_capture();
        let watch = new_id();
        let mut body = rig.request(watch);
        body.watch_instance = Some(new_id());
        body.page = Some(orch_link_contract::commands::FsListPage {
            cursor: None,
            limit: Some(1),
            generation: None,
        });
        let mut plain = body.clone();
        plain.watch_id = None;
        plain.watch_instance = None;
        plain.page.as_mut().ok_or("plain page")?.limit = Some(2);
        let first = if watch_first {
            body.clone()
        } else {
            plain.clone()
        };
        let host = EnvHost {
            inner: Arc::clone(&rig.host.inner),
        };
        let first = tokio::spawn(async move { host.fs_list(&first).await });
        held.entered().await?;
        let environment = rig.host.environment(rig.environment)?;
        assert!(
            !lock(&environment.directory_captures).is_empty(),
            "native coverage starts before the real capture"
        );
        let second = if watch_first { plain } else { body };
        let host = EnvHost {
            inner: Arc::clone(&rig.host.inner),
        };
        let second = tokio::spawn(async move { host.fs_list(&second).await });
        cache.testing.attached(2).await?;
        assert_eq!(cache.testing.started.load(Ordering::Acquire), 1);
        held.release()?;
        let first = tokio::time::timeout(PATIENCE, first).await???;
        let second = tokio::time::timeout(PATIENCE, second).await???;
        let (watched, plain) = if watch_first {
            (first, second)
        } else {
            (second, first)
        };
        assert_eq!(watched.entries.len(), 1);
        assert_eq!(plain.entries.len(), 2);
        let watched = watched.page.ok_or("watched metadata")?;
        let plain = plain.page.ok_or("plain metadata")?;
        assert_eq!(
            (watched.revision, watched.observed_from, watched.as_of),
            (plain.revision, plain.observed_from, plain.as_of)
        );
        let reset = rig.changed(watch).await?;
        assert!(reset.reset);
        assert_eq!(
            reset.page.as_ref().and_then(|page| page.observed_from),
            watched.observed_from
        );
        assert!(lock(&environment.directory_captures).is_empty());
        assert!(lock(&environment.watches).contains_key(&watch));
        rig.host.fs_unwatch(&FsUnwatch { watch_id: watch })?;
    }
    Ok(())
}

#[tokio::test]
async fn native_change_admission_invalidates_a_cursor_while_visible_directory_processing_is_held()
-> Proof {
    let mut rig = Rig::new().await?;
    for name in ["a", "b"] {
        std::fs::write(rig.root.join(name), name)?;
    }
    let watch = new_id();
    let mut body = rig.request(watch);
    body.page = Some(orch_link_contract::commands::FsListPage {
        cursor: None,
        limit: Some(1),
        generation: None,
    });
    let first = rig.host.fs_list(&body).await?;
    assert!(rig.changed(watch).await?.reset);
    let cursor = first
        .page
        .ok_or("first metadata")?
        .next_cursor
        .ok_or("continuation")?;
    let environment = rig.host.environment(rig.environment)?;
    let held = environment.directory_reads.lock().await;
    let cache = &rig.host.inner.directory_pages;
    let before = cache.testing.invalidations.load(Ordering::Acquire);
    std::fs::write(rig.root.join("b"), b"changed natively")?;
    cache.testing.invalidated(before + 1).await?;
    body.watch_id = None;
    body.page.as_mut().ok_or("page")?.cursor = Some(cursor);
    assert!(
        matches!(rig.host.fs_list(&body).await, Err(HostError::Stale { .. })),
        "native admission fences the selector before the blocked presentation path can run"
    );
    drop(held);
    rig.host.fs_unwatch(&FsUnwatch { watch_id: watch })?;
    Ok(())
}

#[tokio::test]
async fn admitted_loss_and_rule_changes_fence_current_cursors_without_any_stat_change() -> Proof {
    for evidence in [
        Change::Rescan,
        Change::Unavailable,
        Change::Paths(vec![RelPath::root().join(crate::ignore::IGNORE_FILE)?]),
    ] {
        let rig = Rig::new().await?;
        for name in ["a", "b"] {
            std::fs::write(rig.root.join(name), name)?;
        }
        let watch = new_id();
        let mut body = rig.request(watch);
        body.page = Some(orch_link_contract::commands::FsListPage {
            cursor: None,
            limit: Some(1),
            generation: None,
        });
        let first = rig.host.fs_list(&body).await?;
        let cursor = first
            .page
            .ok_or("first metadata")?
            .next_cursor
            .ok_or("continuation")?;
        let environment = rig.host.environment(rig.environment)?;
        let version = environment.files.directory_version(&RelPath::root())?;
        let held = environment.directory_reads.lock().await;
        // This is the same synchronous production hook called by native classification, not a
        // test wake or a delayed Git/index refresh. No native data or timestamps are modified.
        environment.admit_directory_change(&evidence);
        assert_eq!(
            environment.files.directory_version(&RelPath::root())?,
            version
        );
        body.watch_id = None;
        body.page.as_mut().ok_or("page")?.cursor = Some(cursor);
        assert!(matches!(
            rig.host.fs_list(&body).await,
            Err(HostError::Stale { .. })
        ));
        drop(held);
        rig.host.fs_unwatch(&FsUnwatch { watch_id: watch })?;
    }
    Ok(())
}

#[tokio::test]
async fn a_lost_complete_view_is_invalidated_without_invented_truncation_or_a_repair_scan() -> Proof
{
    let mut rig = Rig::new().await?;
    for name in ["a", "b"] {
        std::fs::write(rig.root.join(name), name)?;
    }
    let watch = new_id();
    let first = rig.host.fs_list(&rig.request(watch)).await?;
    assert!(!first.truncated);
    assert!(rig.changed(watch).await?.reset);
    let scans = rig
        .host
        .inner
        .directory_pages
        .testing
        .started
        .load(Ordering::Acquire);
    let environment = rig.host.environment(rig.environment)?;
    environment.admit_directory_change(&Change::Rescan);
    environment.changed(Change::Rescan).await;
    let lost = rig.changed(watch).await?;
    assert!(lost.invalidated);
    assert!(!lost.reset);
    assert!(
        !lost.truncated,
        "losing coverage is not evidence of exceeding the row bound"
    );
    assert!(lost.changed.is_empty() && lost.removed.is_empty());
    assert!(lost.page.is_none());
    assert_eq!(
        rig.host
            .inner
            .directory_pages
            .testing
            .started
            .load(Ordering::Acquire),
        scans
    );
    assert!(
        lock(&environment.watches)
            .get(&watch)
            .is_some_and(|watch| watch.invalidated)
    );
    rig.host.fs_unwatch(&FsUnwatch { watch_id: watch })?;
    Ok(())
}
