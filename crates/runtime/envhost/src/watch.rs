//! Recursive watching and fallback scans.
//!
//! The host learns that files changed from the operating system's own notices, gathered for a
//! moment so that a build that touches thousands of files is one batch and not thousands of
//! calls, and turned into paths of the workspace. The notices are not trusted to be complete:
//! when the system says it lost some, [`Change::Rescan`] repairs an indexed tree. Exhausted or
//! unavailable watches give the index up through [`Change::Unavailable`]; periodic notices then
//! refresh only the directories somebody has open. A tree that is too large to be indexed is not watched as a
//! whole; the directories somebody has open are watched one by one.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};
use tokio::sync::mpsc;

use crate::paths::{PathStyle, RelPath, is_internal_name};

/// How a watch is made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchMode {
    /// With the operating system's notices, and rescans where they fall short.
    Native,
    /// Without them: the tree is looked at again every `rescan`.
    Polling,
}

/// How long and how often.
#[derive(Debug, Clone, Copy)]
pub struct WatchConfig {
    /// How the watch is made.
    pub mode: WatchMode,
    /// How long after the last notice a batch is sent: a burst of changes is one batch.
    pub quiet: Duration,
    /// The longest a batch is held back while changes keep coming.
    pub longest: Duration,
    /// How often the whole tree is looked at again while the watch falls short.
    pub rescan: Duration,
}

impl WatchConfig {
    /// A batch is sent a fifth of a second after the last notice: quick enough for a panel to
    /// follow an edit, long enough to hold a save that is several writes.
    pub const QUIET: Duration = Duration::from_millis(200);

    /// ... and never held back for more than a second, so that a build that never pauses still shows.
    pub const LONGEST: Duration = Duration::from_secs(1);

    /// The configuration for the usual watch, rescanning at the interval given.
    #[must_use]
    pub fn new(rescan: Duration) -> Self {
        Self {
            mode: WatchMode::Native,
            quiet: Self::QUIET,
            longest: Self::LONGEST,
            rescan,
        }
    }
}

/// How often a watch with nothing to send looks at whether it has fallen short, which only a
/// notice or a timer would otherwise make it do.
const FLAG_LOOK: Duration = Duration::from_secs(1);

/// What the watch has to say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// These paths changed (were made, written, renamed or removed).
    Paths(Vec<RelPath>),
    /// Look at the whole tree again: notices were lost, or there are none.
    Rescan,
    /// The platform cannot watch the tree; give up whole-tree indexing.
    Unavailable,
}

/// Records native evidence before debounce or any asynchronous index/presentation work.
pub(crate) type Admission = Arc<dyn Fn(Change) + Send + Sync>;

enum Raw {
    Paths(Vec<PathBuf>),
    Rescan,
    Degraded,
    #[cfg(test)]
    Ready(tokio::sync::oneshot::Sender<()>),
}

/// A watch of one workspace.
pub struct Watcher {
    root: PathBuf,
    recursive: AtomicBool,
    degraded: Arc<AtomicBool>,
    native: Mutex<Option<RecommendedWatcher>>,
    admission: Option<Admission>,
    task: tokio::task::JoinHandle<()>,
    /// Keeps the channel of notices open for as long as the watch lasts, whether or not the
    /// operating system sends any.
    _notices: mpsc::UnboundedSender<Raw>,
}

impl std::fmt::Debug for Watcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Watcher")
            .field("root", &self.root)
            .field("degraded", &self.is_degraded())
            .finish_non_exhaustive()
    }
}

/// Turns what the operating system said into something the collector understands.
fn classify(event: notify::Result<notify::Event>) -> Option<Raw> {
    match event {
        Ok(event) => {
            if event.need_rescan() {
                return Some(Raw::Rescan);
            }
            match event.kind {
                // Reading a file is not a change.
                EventKind::Access(_) => None,
                _ => Some(Raw::Paths(event.paths)),
            }
        }
        Err(error) => match error.kind {
            notify::ErrorKind::MaxFilesWatch => Some(Raw::Degraded),
            _ => Some(Raw::Rescan),
        },
    }
}

impl Watcher {
    /// Watches the directory `root` (a real path). With `recursive` the whole tree is watched;
    /// without, only the directories added with [`Watcher::watch_dir`]. Changes arrive on the
    /// receiver. Must be called inside a runtime.
    pub fn start(
        root: &Path,
        recursive: bool,
        config: WatchConfig,
    ) -> (Self, mpsc::UnboundedReceiver<Change>) {
        Self::start_observed(root, recursive, config, None)
    }

    /// The same native watcher with an immediate bounded metadata-invalidation hook.
    pub(crate) fn start_observed(
        root: &Path,
        recursive: bool,
        config: WatchConfig,
        admission: Option<Admission>,
    ) -> (Self, mpsc::UnboundedReceiver<Change>) {
        let (raw_tx, raw_rx) = mpsc::unbounded_channel::<Raw>();
        let (out_tx, out_rx) = mpsc::unbounded_channel::<Change>();
        let degraded = Arc::new(AtomicBool::new(config.mode == WatchMode::Polling));
        let mut native = None;
        if config.mode == WatchMode::Native {
            let sender = raw_tx.clone();
            let evidence = admission.clone();
            let watched_root = root.to_path_buf();
            let availability = Arc::clone(&degraded);
            let made = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                if let Some(raw) = classify(event) {
                    if matches!(&raw, Raw::Degraded) {
                        availability.store(true, Ordering::SeqCst);
                    }
                    admit(&watched_root, evidence.as_ref(), &raw);
                    let _ = sender.send(raw);
                }
            });
            match made {
                Ok(mut watcher) => {
                    let _ = watcher.configure(Config::default());
                    if recursive && watcher.watch(root, RecursiveMode::Recursive).is_err() {
                        degraded.store(true, Ordering::SeqCst);
                    }
                    native = Some(watcher);
                }
                Err(_) => degraded.store(true, Ordering::SeqCst),
            }
        }
        let task = tokio::spawn(collect(
            root.to_path_buf(),
            config,
            raw_rx,
            out_tx,
            Arc::clone(&degraded),
            admission.clone(),
        ));
        (
            Self {
                root: root.to_path_buf(),
                recursive: AtomicBool::new(recursive),
                degraded,
                native: Mutex::new(native),
                admission,
                task,
                _notices: raw_tx,
            },
            out_rx,
        )
    }

    /// Whether notices cannot be relied on, so that the tree is looked at again at intervals.
    #[must_use]
    pub fn is_degraded(&self) -> bool {
        self.degraded.load(Ordering::SeqCst)
    }

    /// Starts a recursive watch only after the bounded first scan finished successfully.
    pub fn enable_recursive(&self) {
        if self.recursive.load(Ordering::SeqCst) || self.is_degraded() {
            return;
        }
        // Changing native coverage is not a proved uninterrupted generation/fence.
        if let Some(admission) = &self.admission {
            admission(Change::Rescan);
        }
        let mut native = self.native.lock().unwrap_or_else(PoisonError::into_inner);
        if native
            .as_mut()
            .is_some_and(|watcher| watcher.watch(&self.root, RecursiveMode::Recursive).is_ok())
        {
            self.recursive.store(true, Ordering::SeqCst);
        } else {
            self.degraded.store(true, Ordering::SeqCst);
            if let Some(admission) = &self.admission {
                admission(Change::Unavailable);
            }
            let _ = self._notices.send(Raw::Degraded);
        }
    }

    /// Stops the whole-tree watch when the index is given up; individual open directories can remain watched.
    pub fn disable_recursive(&self) {
        if !self.recursive.swap(false, Ordering::SeqCst) {
            return;
        }
        if let Some(admission) = &self.admission {
            admission(Change::Rescan);
        }
        if let Some(watcher) = self
            .native
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_mut()
        {
            let _ = watcher.unwatch(&self.root);
        }
    }

    /// Watches the directory `dir` itself, for its entries coming and going, when the whole tree
    /// is not watched already.
    pub(crate) fn try_watch_dir(&self, dir: &Path) -> crate::error::Result<()> {
        self.watch_directory(dir, false)
    }

    /// Rebinds a reused path to its current identity without retaining its old native descriptor.
    pub(crate) fn replace_watch_dir(&self, dir: &Path) -> crate::error::Result<()> {
        self.watch_directory(dir, true)
    }

    fn watch_directory(&self, dir: &Path, replace: bool) -> crate::error::Result<()> {
        let mut native = self.native.lock().unwrap_or_else(PoisonError::into_inner);
        let unavailable = || {
            crate::error::HostError::Unsupported(
                "native directory watching is unavailable".to_owned(),
            )
        };
        let Some(watcher) = native.as_mut() else {
            return Err(unavailable());
        };
        if self.degraded.load(Ordering::SeqCst) {
            return Err(unavailable());
        }
        if self.recursive.load(Ordering::SeqCst) {
            return Ok(());
        }
        if replace {
            if let Some(admission) = &self.admission {
                admission(Change::Rescan);
            }
            if let Err(error) = watcher.unwatch(dir)
                && !matches!(&error.kind, notify::ErrorKind::WatchNotFound)
            {
                self.degraded.store(true, Ordering::SeqCst);
                if let Some(admission) = &self.admission {
                    admission(Change::Unavailable);
                }
                return Err(crate::error::HostError::Unsupported(format!(
                    "native directory watching could not be rebound: {error}"
                )));
            }
        }
        watcher
            .watch(dir, RecursiveMode::NonRecursive)
            .map_err(|error| {
                self.degraded.store(true, Ordering::SeqCst);
                if let Some(admission) = &self.admission {
                    admission(Change::Unavailable);
                }
                crate::error::HostError::Unsupported(format!(
                    "native directory watching could not start: {error}"
                ))
            })
    }

    /// Best-effort watching for the existing index/fallback owner. Live activation uses the
    /// fallible variant so it never claims a polling fallback is an installed native watch.
    pub fn watch_dir(&self, dir: &Path) {
        if self.recursive.load(Ordering::SeqCst) {
            return;
        }
        let mut native = self.native.lock().unwrap_or_else(PoisonError::into_inner);
        match native.as_mut() {
            Some(watcher) => {
                if watcher.watch(dir, RecursiveMode::NonRecursive).is_err() {
                    self.degraded.store(true, Ordering::SeqCst);
                }
            }
            None => self.degraded.store(true, Ordering::SeqCst),
        }
        if self.is_degraded()
            && let Some(admission) = &self.admission
        {
            admission(Change::Unavailable);
        }
    }

    /// Stops watching `dir`.
    pub fn unwatch_dir(&self, dir: &Path) {
        if self.recursive.load(Ordering::SeqCst) {
            return;
        }
        if let Some(watcher) = self
            .native
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_mut()
        {
            let _ = watcher.unwatch(dir);
        }
    }

    /// Stops the watch.
    pub fn stop(&self) {
        self.degraded.store(true, Ordering::SeqCst);
        if let Some(admission) = &self.admission {
            admission(Change::Unavailable);
        }
        self.task.abort();
        *self.native.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        if let Some(admission) = &self.admission {
            admission(Change::Unavailable);
        }
        self.task.abort();
    }
}

fn admit(root: &Path, admission: Option<&Admission>, raw: &Raw) {
    let Some(admission) = admission else { return };
    match raw {
        Raw::Paths(paths) => {
            if paths.iter().any(|path| path == root) {
                admission(Change::Rescan);
                return;
            }
            let paths: Vec<_> = paths
                .iter()
                .filter_map(|path| relative(root, path))
                .collect();
            if !paths.is_empty() {
                admission(Change::Paths(paths));
            }
        }
        Raw::Rescan => admission(Change::Rescan),
        Raw::Degraded => admission(Change::Unavailable),
        #[cfg(test)]
        Raw::Ready(_) => {}
    }
}

fn relative(root: &Path, path: &Path) -> Option<RelPath> {
    let below = path.strip_prefix(root).ok()?;
    let text = below.to_str()?.replace(std::path::MAIN_SEPARATOR, "/");
    if text.is_empty() {
        return None;
    }
    let parsed = RelPath::parse(&text, PathStyle::Posix).ok()?;
    let internal = parsed.components().any(is_internal_name);
    (!internal).then_some(parsed)
}

async fn collect(
    root: PathBuf,
    config: WatchConfig,
    mut raw: mpsc::UnboundedReceiver<Raw>,
    out: mpsc::UnboundedSender<Change>,
    degraded: Arc<AtomicBool>,
    admission: Option<Admission>,
) {
    let mut batch: BTreeSet<RelPath> = BTreeSet::new();
    let mut first_at: Option<tokio::time::Instant> = None;
    let mut last_at = tokio::time::Instant::now();
    let mut rescan_at = tokio::time::Instant::now() + config.rescan;
    let mut was_degraded = degraded.load(Ordering::SeqCst);
    if was_degraded {
        if let Some(admission) = &admission {
            admission(Change::Unavailable);
        }
        let _ = out.send(Change::Unavailable);
    }
    loop {
        let flush_at = first_at.map(|first| (last_at + config.quiet).min(first + config.longest));
        let wait_until = match (flush_at, degraded.load(Ordering::SeqCst)) {
            (Some(flush), true) => flush.min(rescan_at),
            (Some(flush), false) => flush,
            (None, true) => rescan_at,
            // Nothing to flush and nothing to rescan: wait for the next notice, with a look at the flag now and then.
            (None, false) => tokio::time::Instant::now() + FLAG_LOOK,
        };
        tokio::select! {
            incoming = raw.recv() => {
                match incoming {
                    None => break,
                    Some(Raw::Paths(paths)) => {
                        for path in paths {
                            if let Some(rel) = relative(&root, &path) {
                                batch.insert(rel);
                            }
                        }
                        if !batch.is_empty() {
                            let now = tokio::time::Instant::now();
                            first_at.get_or_insert(now);
                            last_at = now;
                        }
                    }
                    Some(Raw::Rescan) => {
                        batch.clear();
                        first_at = None;
                        if out.send(Change::Rescan).is_err() { break }
                    }
                    Some(Raw::Degraded) => {
                        degraded.store(true, Ordering::SeqCst);
                        was_degraded = true;
                        if out.send(Change::Unavailable).is_err() { break }
                    }
                    #[cfg(test)]
                    Some(Raw::Ready(ready)) => { let _ = ready.send(()); }
                }
            }
            () = tokio::time::sleep_until(wait_until) => {
                let now = tokio::time::Instant::now();
                if flush_at.is_some_and(|flush| now >= flush) && !batch.is_empty() {
                    let paths: Vec<RelPath> = std::mem::take(&mut batch).into_iter().collect();
                    first_at = None;
                    if out.send(Change::Paths(paths)).is_err() { break }
                }
                let is_degraded = degraded.load(Ordering::SeqCst);
                if is_degraded && !was_degraded && out.send(Change::Unavailable).is_err() {
                    break;
                }
                if is_degraded && now >= rescan_at {
                    rescan_at = now + config.rescan;
                    // The existing fallback tick is not evidence of another native mutation.
                    // Immutable HTTP observations have no timer-driven invalidation/upkeep.
                    if out.send(Change::Rescan).is_err() { break }
                }
                was_degraded = is_degraded;
            }
        }
    }
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;

    fn quick(mode: WatchMode, rescan: Duration) -> WatchConfig {
        WatchConfig {
            mode,
            quiet: Duration::from_millis(50),
            longest: Duration::from_millis(300),
            rescan,
        }
    }

    async fn next(rx: &mut mpsc::UnboundedReceiver<Change>) -> Change {
        tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("a change within ten seconds")
            .expect("the watch is on")
    }

    /// Collects the paths of batches until all of `wanted` have been seen.
    async fn paths_until(
        rx: &mut mpsc::UnboundedReceiver<Change>,
        wanted: &[&str],
    ) -> BTreeSet<String> {
        let mut seen = BTreeSet::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while !wanted.iter().all(|w| seen.contains(*w)) {
            let change = tokio::time::timeout_at(deadline, rx.recv())
                .await
                .unwrap_or_else(|_| panic!("saw only {seen:?}, wanted {wanted:?}"))
                .unwrap();
            if let Change::Paths(paths) = change {
                seen.extend(paths.into_iter().map(|p| p.to_string()));
            }
        }
        seen
    }

    #[tokio::test]
    async fn changes_in_the_tree_arrive_as_paths_of_the_workspace_in_a_batch() {
        let root = tempfile::tempdir().unwrap();
        let real = root.path().canonicalize().unwrap();
        std::fs::create_dir_all(real.join("src/deep")).unwrap();
        let (watcher, mut rx) = Watcher::start(
            &real,
            true,
            quick(WatchMode::Native, Duration::from_secs(60)),
        );
        assert!(!watcher.is_degraded());
        tokio::time::sleep(Duration::from_millis(100)).await;
        std::fs::write(real.join("src/deep/a.txt"), "a").unwrap();
        std::fs::write(real.join("top.txt"), "t").unwrap();
        std::fs::write(real.join(".orch-tmp-half"), "x").unwrap();
        let seen = paths_until(&mut rx, &["src/deep/a.txt", "top.txt"]).await;
        assert!(
            !seen.iter().any(|p| p.contains(".orch-tmp-")),
            "the host's own half-written files are not changes: {seen:?}"
        );

        std::fs::remove_file(real.join("top.txt")).unwrap();
        std::fs::rename(real.join("src/deep/a.txt"), real.join("src/b.txt")).unwrap();
        let seen = paths_until(&mut rx, &["top.txt", "src/b.txt", "src/deep/a.txt"]).await;
        assert!(
            seen.contains("src/b.txt"),
            "a rename names both ends: {seen:?}"
        );
        watcher.stop();
    }

    #[tokio::test(start_paused = true)]
    async fn a_burst_of_changes_is_one_batch() {
        let root = tempfile::tempdir().unwrap();
        let real = root.path().canonicalize().unwrap();
        let config = quick(WatchMode::Native, Duration::from_secs(60));
        let (raw_tx, raw_rx) = mpsc::unbounded_channel();
        let (out_tx, mut rx) = mpsc::unbounded_channel();
        let task = tokio::spawn(collect(
            real.clone(),
            config,
            raw_rx,
            out_tx,
            Arc::new(AtomicBool::new(false)),
            None,
        ));
        // Real native delivery is covered separately. This test controls when the native
        // notices arrive and when their quiet period ends, independent of filesystem speed.
        for n in 0..40 {
            let path = real.join(format!("f{n}"));
            raw_tx.send(Raw::Paths(vec![path.clone(), path])).unwrap();
        }
        raw_tx
            .send(Raw::Paths(vec![real.join(".orch-tmp-half")]))
            .unwrap();
        let (ready, gathered) = tokio::sync::oneshot::channel();
        raw_tx.send(Raw::Ready(ready)).unwrap();
        gathered.await.unwrap();
        assert!(
            rx.try_recv().is_err(),
            "nothing is published before the quiet period"
        );
        tokio::time::advance(config.quiet).await;
        let first = next(&mut rx).await;
        let Change::Paths(paths) = first else {
            panic!("{first:?}")
        };
        let paths = paths
            .into_iter()
            .map(|path| path.to_string())
            .collect::<BTreeSet<_>>();
        let expected = (0..40).map(|n| format!("f{n}")).collect::<BTreeSet<_>>();
        assert_eq!(
            paths, expected,
            "one complete batch, no duplicates or private temporary path"
        );
        task.abort();
    }

    #[tokio::test]
    async fn without_notices_the_index_is_given_up_and_open_directories_can_be_refreshed() {
        let root = tempfile::tempdir().unwrap();
        let real = root.path().canonicalize().unwrap();
        let (watcher, mut rx) = Watcher::start(
            &real,
            true,
            quick(WatchMode::Polling, Duration::from_millis(100)),
        );
        assert!(watcher.is_degraded());
        assert_eq!(
            next(&mut rx).await,
            Change::Unavailable,
            "the tree is not watchable"
        );
        assert_eq!(
            next(&mut rx).await,
            Change::Rescan,
            "and again after the interval"
        );
        assert_eq!(next(&mut rx).await, Change::Rescan);
        watcher.stop();
    }

    #[tokio::test]
    async fn exhausted_watches_give_up_the_index_while_lost_events_request_one_repair() {
        // What the operating system's backend reports, fed to the classifier as it would be.
        let limit = Err(notify::Error::new(notify::ErrorKind::MaxFilesWatch));
        assert!(matches!(classify(limit), Some(Raw::Degraded)));
        let lost = notify::Event::new(EventKind::Other).set_flag(notify::event::Flag::Rescan);
        assert!(matches!(classify(Ok(lost)), Some(Raw::Rescan)));
        let read = notify::Event::new(EventKind::Access(notify::event::AccessKind::Read))
            .add_path(PathBuf::from("/x"));
        assert!(classify(Ok(read)).is_none(), "reading is not changing");
        assert!(matches!(
            classify(Err(notify::Error::generic("other"))),
            Some(Raw::Rescan)
        ));

        // And a collector that was told the limit was reached rescans from then on.
        let root = tempfile::tempdir().unwrap();
        let (raw_tx, raw_rx) = mpsc::unbounded_channel();
        let (out_tx, mut out_rx) = mpsc::unbounded_channel();
        let degraded = Arc::new(AtomicBool::new(false));
        let task = tokio::spawn(collect(
            root.path().to_path_buf(),
            quick(WatchMode::Native, Duration::from_millis(100)),
            raw_rx,
            out_tx,
            Arc::clone(&degraded),
            None,
        ));
        raw_tx.send(Raw::Degraded).unwrap();
        assert_eq!(next(&mut out_rx).await, Change::Unavailable);
        assert!(degraded.load(Ordering::SeqCst));
        assert_eq!(
            next(&mut out_rx).await,
            Change::Rescan,
            "and again, for as long as it stays so"
        );
        task.abort();
    }

    #[tokio::test]
    async fn a_directory_can_be_watched_by_itself_when_the_tree_is_not() {
        let root = tempfile::tempdir().unwrap();
        let real = root.path().canonicalize().unwrap();
        std::fs::create_dir_all(real.join("open")).unwrap();
        std::fs::create_dir_all(real.join("closed")).unwrap();
        let (watcher, mut rx) = Watcher::start(
            &real,
            false,
            quick(WatchMode::Native, Duration::from_secs(60)),
        );
        watcher.watch_dir(&real.join("open"));
        tokio::time::sleep(Duration::from_millis(100)).await;
        std::fs::write(real.join("closed/nobody-looks.txt"), "x").unwrap();
        std::fs::write(real.join("open/seen.txt"), "x").unwrap();
        let seen = paths_until(&mut rx, &["open/seen.txt"]).await;
        assert!(!seen.contains("closed/nobody-looks.txt"));
        watcher.unwatch_dir(&real.join("open"));
        watcher.stop();
    }
}
