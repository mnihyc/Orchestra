//! Native immutable observation intervals, early invalidation and shared capture ownership.

use super::*;
use crate::environment::{GateState, Gates, Limits};
use crate::index::HashCache;
use crate::paths::{PathStyle, Workspace};
use orch_types::ManualClock;
use std::sync::atomic::AtomicUsize;
use tokio::sync::{Notify, oneshot};

type Proof<T = ()> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
// Reuse the existing native directory-boundary proof budget; a dropped sender also unblocks a gate.
const PATIENCE: Duration = Duration::from_secs(15);

struct Boundary {
    reached: oneshot::Sender<()>,
    resume: std::sync::mpsc::Receiver<()>,
}

#[derive(Default)]
pub(crate) struct Hooks {
    capture: Mutex<Option<Boundary>>,
    publication: Mutex<Option<Boundary>>,
    publication_lock: Mutex<Option<Boundary>>,
    reservation: Mutex<Option<Boundary>>,
    location: Mutex<Option<Boundary>>,
    page: Mutex<Option<Boundary>>,
    pub(crate) started: AtomicUsize,
    pub(crate) exited: AtomicUsize,
    pub(crate) locations_exited: AtomicUsize,
    pub(crate) pages_exited: AtomicUsize,
    attached: AtomicUsize,
    readers: Notify,
    pub(crate) invalidations: AtomicUsize,
    evidence: Notify,
}

pub(crate) struct Hold {
    reached: oneshot::Receiver<()>,
    resume: std::sync::mpsc::Sender<()>,
}

impl Hold {
    pub(crate) async fn entered(&mut self) -> Proof {
        tokio::time::timeout(PATIENCE, &mut self.reached).await??;
        Ok(())
    }

    pub(crate) fn release(self) -> Proof {
        self.resume.send(()).map_err(Into::into)
    }
}

impl Hooks {
    pub(crate) fn hold_capture(&self) -> Hold {
        install(&self.capture)
    }
    pub(crate) fn hold_publication(&self) -> Hold {
        install(&self.publication)
    }
    pub(crate) fn hold_publication_lock(&self) -> Hold {
        install(&self.publication_lock)
    }
    pub(crate) fn hold_reservation(&self) -> Hold {
        install(&self.reservation)
    }
    pub(crate) fn hold_location(&self) -> Hold {
        install(&self.location)
    }
    pub(crate) fn hold_page(&self) -> Hold {
        install(&self.page)
    }

    pub(super) fn before_capture(&self) -> Result<()> {
        self.started.fetch_add(1, Ordering::AcqRel);
        pause(&self.capture)
    }

    pub(super) fn before_publication(&self) -> Result<()> {
        pause(&self.publication)
    }
    pub(super) fn before_publication_lock(&self) -> Result<()> {
        pause(&self.publication_lock)
    }
    pub(super) fn before_reservation(&self) -> Result<()> {
        pause(&self.reservation)
    }

    pub(crate) fn before_location(self: &Arc<Self>) -> Result<ReadExit> {
        let exit = ReadExit {
            hooks: Arc::clone(self),
            page: false,
        };
        pause(&self.location)?;
        Ok(exit)
    }

    pub(super) fn before_page(self: &Arc<Self>) -> Result<ReadExit> {
        let exit = ReadExit {
            hooks: Arc::clone(self),
            page: true,
        };
        pause(&self.page)?;
        Ok(exit)
    }

    pub(super) fn reader(&self) {
        self.attached.fetch_add(1, Ordering::AcqRel);
        self.readers.notify_waiters();
    }

    pub(super) fn invalidate(&self) {
        self.invalidations.fetch_add(1, Ordering::AcqRel);
        self.evidence.notify_waiters();
    }

    pub(crate) async fn invalidated(&self, count: usize) -> Proof {
        tokio::time::timeout(PATIENCE, async {
            loop {
                let waiting = self.evidence.notified();
                tokio::pin!(waiting);
                waiting.as_mut().enable();
                if self.invalidations.load(Ordering::Acquire) >= count {
                    break;
                }
                waiting.await;
            }
        })
        .await?;
        Ok(())
    }

    pub(crate) async fn attached(&self, count: usize) -> Proof {
        tokio::time::timeout(PATIENCE, async {
            loop {
                let waiting = self.readers.notified();
                tokio::pin!(waiting);
                waiting.as_mut().enable();
                if self.attached.load(Ordering::Acquire) >= count {
                    break;
                }
                waiting.await;
            }
        })
        .await?;
        Ok(())
    }

    pub(super) fn exit(self: &Arc<Self>) -> Exit {
        Exit(Arc::clone(self))
    }
}

pub(super) struct Exit(Arc<Hooks>);
impl Drop for Exit {
    fn drop(&mut self) {
        self.0.exited.fetch_add(1, Ordering::AcqRel);
    }
}

pub(crate) struct ReadExit {
    hooks: Arc<Hooks>,
    page: bool,
}
impl Drop for ReadExit {
    fn drop(&mut self) {
        let counter = if self.page {
            &self.hooks.pages_exited
        } else {
            &self.hooks.locations_exited
        };
        counter.fetch_add(1, Ordering::AcqRel);
    }
}

fn install(slot: &Mutex<Option<Boundary>>) -> Hold {
    let (reached, receiver) = oneshot::channel();
    let (resume, blocking) = std::sync::mpsc::channel();
    *lock(slot) = Some(Boundary {
        reached,
        resume: blocking,
    });
    Hold {
        reached: receiver,
        resume,
    }
}

fn pause(slot: &Mutex<Option<Boundary>>) -> Result<()> {
    let boundary = lock(slot).take();
    if let Some(boundary) = boundary {
        let _ = boundary.reached.send(());
        boundary
            .resume
            .recv_timeout(PATIENCE)
            .map_err(|error| match error {
                std::sync::mpsc::RecvTimeoutError::Disconnected => HostError::Cancelled,
                std::sync::mpsc::RecvTimeoutError::Timeout => HostError::Timeout(
                    "the controlled directory boundary exceeded its original proof budget"
                        .to_owned(),
                ),
            })?;
    }
    Ok(())
}

pub(super) struct Rig {
    _temporary: tempfile::TempDir,
    pub(super) root: PathBuf,
    pub(super) cache: Arc<DirectoryPages>,
    pub(super) files: Files,
    pub(super) clock: ManualClock,
    pub(super) binding: Binding,
}

impl Rig {
    pub(super) fn new() -> Proof<Self> {
        let temporary = tempfile::tempdir()?;
        let root = temporary.path().join("workspace");
        std::fs::create_dir(&root)?;
        for name in ["a", "b"] {
            std::fs::write(root.join(name), name)?;
        }
        let files = Files::mount(
            Workspace::open(&root, &[], PathStyle::Posix, true)?,
            Arc::new(HashCache::new()),
            Limits::default(),
            Arc::new(GateState::new(Gates::OPEN)),
        );
        let clock =
            ManualClock::new(chrono::DateTime::from_timestamp(1_760_000_000, 0).ok_or("clock")?);
        let cache = Arc::new(DirectoryPages::new(
            temporary.path().join("metadata"),
            Arc::new(clock.clone()),
        ));
        let binding = Binding {
            environment: new_id(),
            attachment: new_id(),
            generation: Some(4),
            mount: None,
            path: String::new(),
        };
        Ok(Self {
            _temporary: temporary,
            root,
            cache,
            files,
            clock,
            binding,
        })
    }

    pub(super) fn spawn(
        &self,
        limit: u32,
        coverage: Option<Coverage>,
    ) -> tokio::task::JoinHandle<Result<Page>> {
        let (cache, binding, files) = (
            Arc::clone(&self.cache),
            self.binding.clone(),
            self.files.clone(),
        );
        tokio::spawn(async move {
            cache
                .read_covered(binding, files, RelPath::root(), None, limit, coverage)
                .await
        })
    }

    pub(super) async fn read(&self, cursor: Option<&str>, limit: u32) -> Result<Page> {
        self.cache
            .read(
                self.binding.clone(),
                self.files.clone(),
                RelPath::root(),
                cursor,
                limit,
            )
            .await
    }
}

#[tokio::test]
async fn an_observation_keeps_its_actual_interval_on_every_page_and_a_first_read_observes_again()
-> Proof {
    let rig = Rig::new()?;
    let mut hold = rig.cache.testing.hold_capture();
    let task = rig.spawn(1, None);
    hold.entered().await?;
    let start = rig.clock.time();
    // Only the supplied logical clock changes; native stat times are never forced.
    rig.clock.advance(chrono::Duration::seconds(2));
    hold.release()?;
    let first = tokio::time::timeout(PATIENCE, task).await???;
    assert_eq!(first.observed_from, start);
    assert_eq!(first.as_of, rig.clock.time());
    assert!(first.observed_from < first.as_of);
    let cursor = first
        .next_cursor
        .as_deref()
        .ok_or("expected the second observed sibling")?;
    rig.clock.advance(chrono::Duration::seconds(1));
    let last = rig.read(Some(cursor), 1).await?;
    assert_eq!(
        (last.revision, last.observed_from, last.as_of),
        (first.revision, first.observed_from, first.as_of)
    );
    assert_eq!(last.entries[0].name, "b");
    assert!(last.next_cursor.is_none());
    let fresh = rig.read(None, FS_LIST_MAX_ENTRIES).await?;
    assert_eq!(
        fresh
            .entries
            .iter()
            .map(|row| row.name.as_str())
            .collect::<Vec<_>>(),
        ["a", "b"]
    );
    assert!(fresh.observed_from > first.as_of);
    assert_eq!(
        rig.cache.testing.started.load(Ordering::Acquire),
        2,
        "completed metadata is not a fresh first-page cache"
    );
    Ok(())
}

#[tokio::test]
async fn a_known_change_invalidates_even_when_the_native_stat_tuple_did_not_change() -> Proof {
    let rig = Rig::new()?;
    let first = rig.read(None, 1).await?;
    let cursor = first
        .next_cursor
        .as_deref()
        .ok_or("expected continuation")?;
    let version = rig.files.directory_version(&RelPath::root())?;
    rig.cache.invalidate(
        rig.binding.environment,
        rig.binding.attachment,
        &[rig.files.parse("b")?],
        false,
    );
    assert_eq!(rig.files.directory_version(&RelPath::root())?, version);
    assert!(matches!(
        rig.read(Some(cursor), 1).await,
        Err(HostError::Stale { .. })
    ));
    assert_eq!(
        rig.cache.testing.started.load(Ordering::Acquire),
        1,
        "continuation invalidation must not rescan children"
    );
    Ok(())
}

#[tokio::test]
async fn replacing_a_directory_refuses_the_old_cursor_even_when_its_path_is_reused() -> Proof {
    let rig = Rig::new()?;
    std::fs::create_dir(rig.root.join("nested"))?;
    for name in ["a", "b"] {
        std::fs::write(rig.root.join("nested").join(name), name)?;
    }
    let path = rig.files.parse("nested")?;
    let binding = Binding {
        path: "nested".to_owned(),
        ..rig.binding.clone()
    };
    let first = rig
        .cache
        .read(binding.clone(), rig.files.clone(), path.clone(), None, 1)
        .await?;
    let cursor = first
        .next_cursor
        .as_deref()
        .ok_or("old directory continuation")?;
    std::fs::rename(rig.root.join("nested"), rig.root.join("retired"))?;
    std::fs::create_dir(rig.root.join("nested"))?;
    std::fs::write(rig.root.join("nested/new"), b"new")?;
    assert!(matches!(
        rig.cache
            .read(
                binding.clone(),
                rig.files.clone(),
                path.clone(),
                Some(cursor),
                1
            )
            .await,
        Err(HostError::Stale { .. })
    ));
    assert_eq!(rig.cache.testing.started.load(Ordering::Acquire), 1);
    let fresh = rig
        .cache
        .read(binding, rig.files.clone(), path, None, FS_LIST_MAX_ENTRIES)
        .await?;
    assert_eq!(
        fresh
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        ["new"]
    );
    Ok(())
}

#[tokio::test]
async fn an_old_attachments_native_events_and_detach_cannot_invalidate_the_new_attachments_cursor()
-> Proof {
    let rig = Rig::new()?;
    let old = rig.read(None, 1).await?;
    let newer = Binding {
        attachment: new_id(),
        ..rig.binding.clone()
    };
    let fresh = rig
        .cache
        .read(newer.clone(), rig.files.clone(), RelPath::root(), None, 1)
        .await?;
    let cursor = fresh
        .next_cursor
        .as_deref()
        .ok_or("new attachment continuation")?;
    rig.cache
        .invalidate(rig.binding.environment, rig.binding.attachment, &[], true);
    rig.cache
        .forget_attachment(rig.binding.environment, rig.binding.attachment);
    rig.cache
        .join_attachment(rig.binding.environment, rig.binding.attachment)
        .await?;
    assert!(matches!(
        rig.read(old.next_cursor.as_deref(), 1).await,
        Err(HostError::Stale { .. })
    ));
    let last = rig
        .cache
        .read(newer, rig.files.clone(), RelPath::root(), Some(cursor), 1)
        .await?;
    assert_eq!(
        (last.revision, last.observed_from, last.as_of),
        (fresh.revision, fresh.observed_from, fresh.as_of)
    );
    assert_eq!(last.entries[0].name, "b");
    Ok(())
}

#[tokio::test]
async fn a_known_event_before_publication_refuses_the_capture_independently_of_stat_equality()
-> Proof {
    let rig = Rig::new()?;
    let version = rig.files.directory_version(&RelPath::root())?;
    let mut hold = rig.cache.testing.hold_publication();
    let task = rig.spawn(1, None);
    hold.entered().await?;
    rig.cache.invalidate(
        rig.binding.environment,
        rig.binding.attachment,
        &[rig.files.parse("b")?],
        false,
    );
    assert_eq!(rig.files.directory_version(&RelPath::root())?, version);
    hold.release()?;
    assert!(matches!(
        tokio::time::timeout(PATIENCE, task).await??,
        Err(HostError::Changed(_))
    ));
    rig.cache
        .join_attachment(rig.binding.environment, rig.binding.attachment)
        .await?;
    assert!(lock(&rig.cache.snapshots).is_empty());
    assert_eq!(rig.cache.ledger.used.load(Ordering::Acquire), 0);
    assert_eq!(rig.cache.testing.exited.load(Ordering::Acquire), 1);
    Ok(())
}

#[tokio::test]
async fn event_loss_invalidates_all_the_environments_old_observations_before_fresh_restart() -> Proof
{
    let rig = Rig::new()?;
    std::fs::create_dir(rig.root.join("nested"))?;
    std::fs::write(rig.root.join("nested/a"), b"a")?;
    std::fs::write(rig.root.join("nested/b"), b"b")?;
    let first = rig.read(None, 1).await?;
    let nested = rig.files.parse("nested")?;
    let binding = Binding {
        path: "nested".to_owned(),
        ..rig.binding.clone()
    };
    let child = rig
        .cache
        .read(binding.clone(), rig.files.clone(), nested.clone(), None, 1)
        .await?;
    let cursor = first.next_cursor.as_deref().ok_or("root continuation")?;
    let child_cursor = child.next_cursor.as_deref().ok_or("nested continuation")?;
    rig.cache
        .invalidate(rig.binding.environment, rig.binding.attachment, &[], true);
    assert!(matches!(
        rig.read(Some(cursor), 1).await,
        Err(HostError::Stale { .. })
    ));
    assert!(matches!(
        rig.cache
            .read(binding, rig.files.clone(), nested, Some(child_cursor), 1)
            .await,
        Err(HostError::Stale { .. })
    ));
    let fresh = rig.read(None, FS_LIST_MAX_ENTRIES).await?;
    assert_eq!(
        fresh
            .entries
            .iter()
            .map(|row| row.name.as_str())
            .collect::<Vec<_>>(),
        ["nested", "a", "b"]
    );
    Ok(())
}

#[tokio::test]
async fn in_flight_readers_share_one_producer_with_independent_limits_and_cancellation() -> Proof {
    let rig = Rig::new()?;
    let mut hold = rig.cache.testing.hold_capture();
    let first = rig.spawn(1, None);
    hold.entered().await?;
    let second = rig.spawn(2, None);
    rig.cache.testing.attached(2).await?;
    assert_eq!(rig.cache.testing.started.load(Ordering::Acquire), 1);
    assert_eq!(lock(&rig.cache.captures).len(), 1);
    first.abort();
    assert!(
        tokio::time::timeout(PATIENCE, first)
            .await?
            .is_err_and(|error| error.is_cancelled())
    );
    hold.release()?;
    let page = tokio::time::timeout(PATIENCE, second).await???;
    assert_eq!(
        page.entries
            .iter()
            .map(|row| row.name.as_str())
            .collect::<Vec<_>>(),
        ["a", "b"]
    );
    assert!(page.next_cursor.is_none());
    assert_eq!(rig.cache.testing.started.load(Ordering::Acquire), 1);
    assert_eq!(rig.cache.testing.exited.load(Ordering::Acquire), 1);
    assert!(lock(&rig.cache.captures).is_empty());
    Ok(())
}

struct Owner(Arc<AtomicBool>);
impl Drop for Owner {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

#[tokio::test]
async fn final_reader_cancellation_retains_the_native_owner_and_permit_until_joined_producer_exit()
-> Proof {
    for before_publication in [false, true] {
        let rig = Rig::new()?;
        let gone = Arc::new(AtomicBool::new(false));
        let owner: Coverage = Arc::new(Owner(Arc::clone(&gone)));
        let mut hold = if before_publication {
            rig.cache.testing.hold_publication()
        } else {
            rig.cache.testing.hold_capture()
        };
        let task = rig.spawn(1, Some(owner));
        hold.entered().await?;
        task.abort();
        assert!(
            tokio::time::timeout(PATIENCE, task)
                .await?
                .is_err_and(|error| error.is_cancelled())
        );
        assert!(!gone.load(Ordering::Acquire));
        assert_eq!(rig.cache.build.available_permits(), 0);
        let builds = lock(&rig.cache.captures).clone();
        hold.release()?;
        // Join without issuing a second cancellation/invalidation: the request's actual final
        // reader drop must itself stop publication, including after all sorting has finished.
        for build in builds {
            tokio::time::timeout(PATIENCE, build.join()).await??;
        }
        rig.cache.reap().await?;
        assert!(gone.load(Ordering::Acquire));
        assert_eq!(rig.cache.build.available_permits(), 1);
        assert_eq!(rig.cache.testing.exited.load(Ordering::Acquire), 1);
        assert!(lock(&rig.cache.captures).is_empty());
        assert!(lock(&rig.cache.snapshots).is_empty());
        assert_eq!(rig.cache.ledger.used.load(Ordering::Acquire), 0);
    }
    Ok(())
}

#[tokio::test]
async fn a_protected_reader_never_joins_an_earlier_unprotected_capture() -> Proof {
    let rig = Rig::new()?;
    let mut hold = rig.cache.testing.hold_capture();
    let plain = rig.spawn(1, None);
    hold.entered().await?;
    let watched = rig.spawn(2, Some(Arc::new(())));
    rig.cache.testing.attached(2).await?;
    assert_eq!(lock(&rig.cache.captures).len(), 2);
    assert_eq!(
        rig.cache.testing.started.load(Ordering::Acquire),
        1,
        "the second producer waits for the host's one permit"
    );
    hold.release()?;
    let first = tokio::time::timeout(PATIENCE, plain).await???;
    let second = tokio::time::timeout(PATIENCE, watched).await???;
    assert_eq!(first.entries.len(), 1);
    assert_eq!(second.entries.len(), 2);
    assert_eq!(rig.cache.testing.started.load(Ordering::Acquire), 2);
    Ok(())
}

#[tokio::test]
async fn a_plain_reader_can_share_a_capture_protected_from_its_beginning() -> Proof {
    let rig = Rig::new()?;
    let mut hold = rig.cache.testing.hold_capture();
    let watched = rig.spawn(1, Some(Arc::new(())));
    hold.entered().await?;
    let plain = rig.spawn(2, None);
    rig.cache.testing.attached(2).await?;
    assert_eq!(lock(&rig.cache.captures).len(), 1);
    hold.release()?;
    let first = tokio::time::timeout(PATIENCE, watched).await???;
    let second = tokio::time::timeout(PATIENCE, plain).await???;
    assert_eq!(
        (first.revision, first.observed_from, first.as_of),
        (second.revision, second.observed_from, second.as_of)
    );
    assert_eq!(first.entries.len(), 1);
    assert_eq!(second.entries.len(), 2);
    assert_eq!(rig.cache.testing.started.load(Ordering::Acquire), 1);
    Ok(())
}

#[test]
fn duplicate_exact_names_are_refused_across_bounded_runs_even_when_their_kinds_conflict() -> Proof {
    let rig = Rig::new()?;
    std::fs::create_dir_all(&rig.cache.directory)?;
    for kind in [DirEntryKind::File, DirEntryKind::Dir] {
        let cancel = CancellationToken::new();
        let deadline = Instant::now() + PATIENCE;
        let mut builder = Builder {
            cache: &rig.cache,
            rows: Vec::new(),
            bytes: 0,
            levels: Vec::new(),
        };
        let row = |kind| DirEntry {
            name: "same".to_owned(),
            path: "same".to_owned(),
            kind,
            size: 0,
            mtime: rig.clock.time(),
            mode: 0o644,
            link_target: None,
            cached: false,
        };
        builder.rows.push(Record::new(row(DirEntryKind::File))?);
        builder.flush_order(Order::Name, &cancel, deadline)?;
        builder.rows.push(Record::new(row(kind))?);
        assert!(matches!(
            builder.flush_order(Order::Name, &cancel, deadline),
            Err(HostError::Changed(_))
        ));
        drop(builder);
        assert_eq!(rig.cache.ledger.used.load(Ordering::Acquire), 0);
    }
    Ok(())
}
