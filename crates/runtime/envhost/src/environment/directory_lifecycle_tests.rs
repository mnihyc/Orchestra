//! Native no-store request cancellation, sealed attachment cleanup and reset delivery.

use super::*;
use crate::blobs::MemoryBlobs;
use orch_link_contract::commands::{FsListPage, IndexMode};
use orch_types::{SystemClock, new_id};
use std::task::Poll;
use tokio::sync::oneshot;

type Proof<T = ()> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
const PATIENCE: Duration = Duration::from_secs(15);

struct Boundary {
    entered: oneshot::Sender<()>,
    resume: oneshot::Receiver<()>,
}
tokio::task_local! { static BEFORE_DELIVERY: Mutex<Option<Boundary>>; }

pub(super) async fn before_delivery() {
    let boundary = BEFORE_DELIVERY
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
        for name in ["a", "b"] {
            std::fs::write(root.join(name), name)?;
        }
        let connector = directory.path().join("connector");
        std::fs::create_dir(&connector)?;
        // A real failed store open, not a recording lease/test substitution. Ordinary live
        // reads must own their native work even when no recording facility exists.
        std::fs::write(connector.join("file-states"), b"no recording store")?;
        let settings = HostSettings::connected(connector, vec![root.clone()]);
        let (reports, receiver) = Reports::channel(256);
        let host = EnvHost::new(
            settings,
            Arc::new(SystemClock),
            reports,
            Arc::new(MemoryBlobs::new()),
            Vec::new(),
        );
        let rig = Self {
            host,
            environment: new_id(),
            root,
            reports: receiver,
            _directory: directory,
        };
        rig.attach().await?;
        assert!(rig.host.environment(rig.environment)?.file_states.is_none());
        Ok(rig)
    }

    async fn attach(&self) -> Proof {
        self.host
            .env_attach(
                &EnvAttach {
                    resources: None,
                    data_generation: None,
                    storage: None,
                    data_sources: None,
                    environment_id: self.environment,
                    workspace_root: Some(self.root.to_string_lossy().into_owned()),
                    ignore: Vec::new(),
                    index: IndexMode::Live,
                    mounts: None,
                    variables: BTreeMap::new(),
                    secrets: BTreeMap::new(),
                },
                &BTreeMap::new(),
            )
            .await?;
        Ok(())
    }

    fn request(&self, cursor: Option<String>) -> FsList {
        FsList {
            environment_id: self.environment,
            path: String::new(),
            mount: None,
            watch_id: None,
            watch_instance: None,
            page: Some(FsListPage {
                cursor,
                limit: Some(1),
                generation: None,
            }),
        }
    }

    fn spawn(&self, body: FsList) -> tokio::task::JoinHandle<Result<FsListResult>> {
        let host = EnvHost {
            inner: Arc::clone(&self.host.inner),
        };
        tokio::spawn(async move { host.fs_list(&body).await })
    }

    async fn fresh_after_detach(&self, old: &Environment, cursor: String) -> Proof {
        assert!(
            matches!(
                old.directory_operations
                    .begin(&self.host.inner.directory_pages, None)
                    .await,
                Err(HostError::Changed(_))
            ),
            "a sealed old attachment must never admit another phase or command"
        );
        self.attach().await?;
        let fresh = self.host.environment(self.environment)?;
        assert_ne!(fresh.directory_attachment, old.directory_attachment);
        assert!(fresh.file_states.is_none());
        assert!(matches!(
            self.host.fs_list(&self.request(Some(cursor))).await,
            Err(HostError::Stale { .. })
        ));
        let mut request = self.request(None);
        request.page.as_mut().ok_or("page")?.limit = Some(2);
        let page = self.host.fs_list(&request).await?;
        assert_eq!(
            page.entries
                .iter()
                .map(|row| row.name.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
        self.host
            .env_detach(&EnvDetach {
                environment_id: self.environment,
            })
            .await?;
        Ok(())
    }
}

#[tokio::test]
async fn detach_joins_the_actual_cancelled_continuation_worker_without_a_capture_or_recording_store()
-> Proof {
    let rig = Rig::new().await?;
    let first = rig.host.fs_list(&rig.request(None)).await?;
    let cursor = first
        .page
        .ok_or("page metadata")?
        .next_cursor
        .ok_or("continuation")?;
    let old = rig.host.environment(rig.environment)?;
    let cache = &rig.host.inner.directory_pages;
    let finished = cache.testing.pages_exited.load(Ordering::Acquire);
    let captures = cache.testing.started.load(Ordering::Acquire);
    let mut hold = cache.testing.hold_page();
    let request = rig.spawn(rig.request(Some(cursor.clone())));
    hold.entered().await?;
    request.abort();
    assert!(
        tokio::time::timeout(PATIENCE, request)
            .await?
            .is_err_and(|error| error.is_cancelled())
    );
    assert_eq!(cache.testing.pages_exited.load(Ordering::Acquire), finished);
    let body = EnvDetach {
        environment_id: rig.environment,
    };
    let detach = rig.host.env_detach(&body);
    tokio::pin!(detach);
    assert!(
        matches!(futures::poll!(&mut detach), Poll::Pending),
        "empty native teardown must wait for the held actual page worker"
    );
    tokio::time::timeout(PATIENCE, old.directory_operations.retired()).await?;
    assert_eq!(cache.testing.pages_exited.load(Ordering::Acquire), finished);
    hold.release()?;
    tokio::time::timeout(PATIENCE, &mut detach).await??;
    assert_eq!(
        cache.testing.pages_exited.load(Ordering::Acquire),
        finished + 1
    );
    assert_eq!(
        cache.testing.started.load(Ordering::Acquire),
        captures,
        "continuation must not create a producer"
    );
    rig.fresh_after_detach(&old, cursor).await?;
    Ok(())
}

#[tokio::test]
async fn detach_seals_the_first_location_read_and_never_admits_an_old_attachment_capture_after_join()
-> Proof {
    let rig = Rig::new().await?;
    let old = rig.host.environment(rig.environment)?;
    let cache = &rig.host.inner.directory_pages;
    let finished = cache.testing.locations_exited.load(Ordering::Acquire);
    let mut hold = cache.testing.hold_location();
    let request = rig.spawn(rig.request(None));
    hold.entered().await?;
    let body = EnvDetach {
        environment_id: rig.environment,
    };
    let detach = rig.host.env_detach(&body);
    tokio::pin!(detach);
    assert!(matches!(futures::poll!(&mut detach), Poll::Pending));
    tokio::time::timeout(PATIENCE, old.directory_operations.retired()).await?;
    assert_eq!(
        cache.testing.locations_exited.load(Ordering::Acquire),
        finished
    );
    assert_eq!(cache.testing.started.load(Ordering::Acquire), 0);
    hold.release()?;
    tokio::time::timeout(PATIENCE, &mut detach).await??;
    assert!(matches!(
        tokio::time::timeout(PATIENCE, request).await??,
        Err(HostError::Changed(_))
    ));
    assert_eq!(
        cache.testing.locations_exited.load(Ordering::Acquire),
        finished + 1
    );
    assert_eq!(
        cache.testing.started.load(Ordering::Acquire),
        0,
        "no producer may start after old admission was sealed and joined"
    );
    assert!(matches!(
        old.directory_operations.begin(cache, None).await,
        Err(HostError::Changed(_))
    ));
    rig.attach().await?;
    let fresh = rig.host.fs_list(&rig.request(None)).await?;
    assert_eq!(fresh.entries[0].name, "a");
    assert_ne!(
        rig.host.environment(rig.environment)?.directory_attachment,
        old.directory_attachment
    );
    rig.host.env_detach(&body).await?;
    Ok(())
}

#[tokio::test]
async fn reset_delivery_refuses_an_evicted_observation_after_a_real_native_change() -> Proof {
    let mut rig = Rig::new().await?;
    let environment = rig.host.environment(rig.environment)?;
    let watch = new_id();
    let mut body = rig.request(None);
    body.watch_id = Some(watch);
    body.watch_instance = Some(new_id());
    let (entered, reached) = oneshot::channel();
    let (resume, blocked) = oneshot::channel();
    let host = EnvHost {
        inner: Arc::clone(&rig.host.inner),
    };
    let reading = tokio::spawn(BEFORE_DELIVERY.scope(
        Mutex::new(Some(Boundary {
            entered,
            resume: blocked,
        })),
        async move { host.fs_list(&body).await },
    ));
    tokio::time::timeout(PATIENCE, reached).await??;
    let cache = &rig.host.inner.directory_pages;
    assert!(
        cache.evict_observation_for_test(),
        "the in-hand reset has released its completed metadata pin"
    );
    let before = cache.testing.invalidations.load(Ordering::Acquire);
    std::fs::write(
        rig.root.join("b"),
        b"a genuine native change while reset delivery is held",
    )?;
    cache.testing.invalidated(before + 1).await?;
    resume
        .send(())
        .map_err(|()| "reset delivery ended before its event boundary")?;
    assert!(matches!(
        tokio::time::timeout(PATIENCE, reading).await??,
        Err(HostError::Stale { .. })
    ));
    assert!(
        !lock(&environment.watches).contains_key(&watch),
        "an invalid initial reset must not commit its installation"
    );
    while let Ok(report) = rig.reports.try_recv() {
        assert!(
            !matches!(report.report, Report::FsChanged(ref changed) if changed.watch_id == watch && changed.reset),
            "no successful old reset may be queued after the known event"
        );
    }
    rig.host
        .env_detach(&EnvDetach {
            environment_id: rig.environment,
        })
        .await?;
    Ok(())
}
