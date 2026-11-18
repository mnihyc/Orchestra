//! Cancellation of the public lifecycle future must not acknowledge unfinished native cleanup.

use super::*;
use crate::blobs::MemoryBlobs;
use orch_link_contract::commands::{FsListPage, IndexMode};
use orch_types::{SystemClock, new_id};
use std::task::Poll;

type Proof<T = ()> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
// The existing public detach budget; the blocking gates keep their original15-second bound.
const PATIENCE: Duration = Duration::from_secs(30);

struct Rig {
    host: EnvHost,
    attach: EnvAttach,
    _reports: mpsc::Receiver<Reported>,
    _directory: tempfile::TempDir,
}

impl Rig {
    async fn new() -> Proof<Self> {
        Self::build(false).await
    }

    async fn build(mounted: bool) -> Proof<Self> {
        let directory = tempfile::tempdir()?;
        let root = directory.path().join("workspace");
        std::fs::create_dir(&root)?;
        for name in ["a", "b"] {
            std::fs::write(root.join(name), name)?;
        }
        let connector = directory.path().join("connector");
        std::fs::create_dir(&connector)?;
        std::fs::write(connector.join("file-states"), b"no recording store")?;
        let mount = directory.path().join("mount");
        let mounts = if mounted {
            std::fs::create_dir(&mount)?;
            Some(vec![orch_link_contract::commands::EnvAttachMount {
                mount_id: new_id(),
                name: "data".into(),
                source: mount.to_str().ok_or("mount path")?.into(),
                volume: None,
                mode: orch_link_contract::commands::AttachMode::Shared,
            }])
        } else {
            None
        };
        let (reports, receiver) = Reports::channel(256);
        let host = EnvHost::new(
            HostSettings::connected(
                connector,
                vec![if mounted {
                    directory.path().to_path_buf()
                } else {
                    root.clone()
                }],
            ),
            Arc::new(SystemClock),
            reports,
            Arc::new(MemoryBlobs::new()),
            Vec::new(),
        );
        let attach = EnvAttach {
            resources: None,
            data_generation: None,
            storage: None,
            data_sources: None,
            environment_id: new_id(),
            workspace_root: Some(root.to_string_lossy().into_owned()),
            ignore: Vec::new(),
            index: IndexMode::Live,
            mounts,
            variables: BTreeMap::new(),
            secrets: BTreeMap::new(),
        };
        host.env_attach(&attach, &BTreeMap::new()).await?;
        assert!(
            host.environment(attach.environment_id)?
                .file_states
                .is_none()
        );
        Ok(Self {
            host,
            attach,
            _reports: receiver,
            _directory: directory,
        })
    }

    fn request(&self, cursor: Option<String>) -> FsList {
        FsList {
            environment_id: self.attach.environment_id,
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

    fn spawn_read(&self, cursor: String) -> tokio::task::JoinHandle<Result<FsListResult>> {
        let host = self.host.clone();
        let request = self.request(Some(cursor));
        tokio::spawn(async move { host.fs_list(&request).await })
    }

    async fn owners(&self, available: usize) -> Proof {
        // Probe the real host semaphore without a test-only shortcut or another native read.
        let scope = crate::directory_pages::Attachment::new();
        let mut owners = Vec::new();
        for _ in 0..available {
            owners.push(scope.begin(&self.host.inner.directory_pages, None).await?);
        }
        assert!(matches!(
            scope.begin(&self.host.inner.directory_pages, None).await,
            Err(HostError::Busy(_))
        ));
        drop(owners);
        Ok(())
    }
}

fn stopped(environment: &Environment) {
    assert!(lock(&environment.watcher).is_none());
    assert!(lock(&environment.tasks).is_empty());
    assert!(lock(&environment.reporters).is_empty());
}

#[tokio::test]
async fn a_cancelled_public_detach_retries_the_host_owned_old_finalizer_instead_of_empty_map_success()
-> Proof {
    let rig = Rig::new().await?;
    let first = rig.host.fs_list(&rig.request(None)).await?;
    let cursor = first
        .page
        .ok_or("page metadata")?
        .next_cursor
        .ok_or("continuation")?;
    let old = rig.host.environment(rig.attach.environment_id)?;
    let cache = &rig.host.inner.directory_pages;
    let finished = cache.testing.pages_exited.load(Ordering::Acquire);
    let mut hold = cache.testing.hold_page();
    let read = rig.spawn_read(cursor.clone());
    hold.entered().await?;
    read.abort();
    assert!(
        tokio::time::timeout(PATIENCE, read)
            .await?
            .is_err_and(|error| error.is_cancelled())
    );
    let body = EnvDetach {
        environment_id: rig.attach.environment_id,
    };
    {
        let detach = rig.host.env_detach(&body);
        tokio::pin!(detach);
        assert!(matches!(futures::poll!(&mut detach), Poll::Pending));
        tokio::time::timeout(PATIENCE, old.directory_operations.retired()).await?;
        assert!(rig.host.environment(body.environment_id).is_err());
        let retiring = lock(&rig.host.inner.retiring);
        assert_eq!(
            retiring
                .get(&body.environment_id)
                .ok_or("old finalizer absent")?
                .attachment,
            old.directory_attachment
        );
    } // Drop the exact public future after it removed the live entry, with the OS worker held.
    assert_eq!(cache.testing.pages_exited.load(Ordering::Acquire), finished);
    rig.owners(DIR_WATCHES_MAX - 1).await?;
    let retry = rig.host.env_detach(&body);
    tokio::pin!(retry);
    assert!(
        matches!(futures::poll!(&mut retry), Poll::Pending),
        "public retry cannot acknowledge the absent map entry while old native work remains"
    );
    assert_eq!(cache.testing.pages_exited.load(Ordering::Acquire), finished);
    assert!(
        matches!(
            rig.host.env_attach(&rig.attach, &BTreeMap::new()).await,
            Err(HostError::Busy(_))
        ),
        "a new attachment cannot overtake the still-retiring removed attachment"
    );
    hold.release()?;
    tokio::time::timeout(PATIENCE, &mut retry).await??;
    assert_eq!(
        cache.testing.pages_exited.load(Ordering::Acquire),
        finished + 1
    );
    assert!(!lock(&rig.host.inner.retiring).contains_key(&body.environment_id));
    stopped(&old);
    rig.owners(DIR_WATCHES_MAX).await?;
    rig.host.env_detach(&body).await?; // A genuinely completed repeated detach is still harmless.
    rig.host.env_attach(&rig.attach, &BTreeMap::new()).await?;
    assert_ne!(
        rig.host
            .environment(body.environment_id)?
            .directory_attachment,
        old.directory_attachment
    );
    assert!(matches!(
        rig.host.fs_list(&rig.request(Some(cursor))).await,
        Err(HostError::Stale { .. })
    ));
    assert_eq!(
        rig.host.fs_list(&rig.request(None)).await?.entries[0].name,
        "a"
    );
    rig.host.env_detach(&body).await?;
    Ok(())
}

#[tokio::test]
async fn cancelling_public_replacement_retains_old_cleanup_without_closing_the_usable_new_attachment()
-> Proof {
    let rig = Rig::new().await?;
    let first = rig.host.fs_list(&rig.request(None)).await?;
    let cursor = first
        .page
        .ok_or("page metadata")?
        .next_cursor
        .ok_or("continuation")?;
    let old = rig.host.environment(rig.attach.environment_id)?;
    let old_reservation = rig.host.reserve_detach(rig.attach.environment_id)?;
    let cache = &rig.host.inner.directory_pages;
    let mut hold = cache.testing.hold_page();
    let read = rig.spawn_read(cursor);
    hold.entered().await?;
    read.abort();
    assert!(
        tokio::time::timeout(PATIENCE, read)
            .await?
            .is_err_and(|error| error.is_cancelled())
    );
    let host = rig.host.clone();
    let body = rig.attach.clone();
    let replacing = tokio::spawn(async move { host.env_attach(&body, &BTreeMap::new()).await });
    tokio::time::timeout(PATIENCE, old.directory_operations.retired()).await?;
    replacing.abort();
    assert!(
        tokio::time::timeout(PATIENCE, replacing)
            .await?
            .is_err_and(|error| error.is_cancelled())
    );
    let current = rig.host.environment(rig.attach.environment_id)?;
    assert_ne!(current.directory_attachment, old.directory_attachment);
    let cleanup = lock(&rig.host.inner.retiring)
        .get(&rig.attach.environment_id)
        .cloned()
        .ok_or("old replacement finalizer absent")?;
    assert_eq!(cleanup.attachment, old.directory_attachment);
    assert!(matches!(
        rig.host.env_attach(&rig.attach, &BTreeMap::new()).await,
        Err(HostError::Busy(_))
    ));
    {
        // A reservation made against the old attachment also joins only that old finalizer.
        let retry = rig.host.detach_reserved(&old_reservation);
        tokio::pin!(retry);
        assert!(matches!(futures::poll!(&mut retry), Poll::Pending));
    }
    assert!(Arc::ptr_eq(
        &current,
        &rig.host.environment(rig.attach.environment_id)?
    ));
    assert!(lock(&current.watcher).is_some());
    let first_current = rig.host.fs_list(&rig.request(None)).await?;
    assert_eq!(first_current.entries[0].name, "a");
    let metadata = first_current.page.ok_or("new page metadata")?;
    let continuation = metadata.next_cursor.ok_or("new continuation")?;
    let before_release = cache.testing.pages_exited.load(Ordering::Acquire);
    rig.owners(DIR_WATCHES_MAX - 1).await?;
    hold.release()?;
    tokio::time::timeout(PATIENCE, cleanup.wait()).await??;
    assert_eq!(
        cache.testing.pages_exited.load(Ordering::Acquire),
        before_release + 1
    );
    stopped(&old);
    assert!(!lock(&rig.host.inner.retiring).contains_key(&rig.attach.environment_id));
    rig.owners(DIR_WATCHES_MAX).await?;
    assert!(Arc::ptr_eq(
        &current,
        &rig.host.environment(rig.attach.environment_id)?
    ));
    assert!(
        lock(&current.watcher).is_some(),
        "old cleanup must not stop the new watcher"
    );
    let next = rig.host.fs_list(&rig.request(Some(continuation))).await?;
    assert_eq!(next.entries[0].name, "b");
    assert_eq!(
        next.page.ok_or("new continuation metadata")?.revision,
        metadata.revision
    );
    rig.host
        .env_detach(&EnvDetach {
            environment_id: rig.attach.environment_id,
        })
        .await?;
    Ok(())
}

#[tokio::test]
async fn mount_data_stays_in_use_until_the_original_detached_attachment_has_actually_finished()
-> Proof {
    let rig = Rig::build(true).await?;
    let old = rig.host.environment(rig.attach.environment_id)?;
    let identity = old.mounts().files("data")?.root().root_meta()?;
    assert!(rig.host.mount_directory_in_use(identity.dev, identity.ino));
    let unrelated = rig._directory.path().join("unrelated");
    std::fs::create_dir(&unrelated)?;
    let other = crate::platform::Dir::open_path(&unrelated)?.stat_self()?;
    assert!(!rig.host.mount_directory_in_use(other.dev, other.ino));
    let first = rig.host.fs_list(&rig.request(None)).await?;
    let cursor = first
        .page
        .ok_or("page metadata")?
        .next_cursor
        .ok_or("continuation")?;
    let mut hold = rig.host.inner.directory_pages.testing.hold_page();
    let read = rig.spawn_read(cursor);
    hold.entered().await?;
    read.abort();
    assert!(
        tokio::time::timeout(PATIENCE, read)
            .await?
            .is_err_and(|error| error.is_cancelled())
    );
    let body = EnvDetach {
        environment_id: rig.attach.environment_id,
    };
    {
        let detach = rig.host.env_detach(&body);
        tokio::pin!(detach);
        assert!(matches!(futures::poll!(&mut detach), Poll::Pending));
        tokio::time::timeout(PATIENCE, old.directory_operations.retired()).await?;
        assert!(rig.host.environment(body.environment_id).is_err());
        assert!(
            rig.host.mount_directory_in_use(identity.dev, identity.ino),
            "absence from the current map must not permit overwriting a retiring consumer's data"
        );
    }
    assert!(rig.host.mount_directory_in_use(identity.dev, identity.ino));
    hold.release()?;
    tokio::time::timeout(PATIENCE, rig.host.env_detach(&body)).await??;
    assert!(!rig.host.mount_directory_in_use(identity.dev, identity.ino));
    Ok(())
}

#[tokio::test]
async fn attaching_a_directory_waits_for_data_work_on_its_actual_opened_mount_root() -> Proof {
    let rig = Rig::build(true).await?;
    let identity = rig
        .host
        .environment(rig.attach.environment_id)?
        .mounts()
        .files("data")?
        .root()
        .root_meta()?;
    rig.host
        .env_detach(&EnvDetach {
            environment_id: rig.attach.environment_id,
        })
        .await?;
    let locks = rig.host.storage_locks();
    let writing = locks.hold_directory(identity.dev, identity.ino).await;
    let secrets = BTreeMap::new();
    let attach = rig.host.env_attach(&rig.attach, &secrets);
    tokio::pin!(attach);
    tokio::time::timeout(PATIENCE, async {
        loop {
            if locks.directory_contended(identity.dev, identity.ino) { break; }
            tokio::select! {
                result = &mut attach => panic!("attachment completed before its native data writer: {result:?}"),
                () = tokio::task::yield_now() => {},
            }
        }
    }).await?;
    assert!(rig.host.environment(rig.attach.environment_id).is_err());
    drop(writing);
    tokio::time::timeout(PATIENCE, &mut attach).await??;
    assert!(rig.host.mount_directory_in_use(identity.dev, identity.ino));
    rig.host
        .env_detach(&EnvDetach {
            environment_id: rig.attach.environment_id,
        })
        .await?;
    Ok(())
}
