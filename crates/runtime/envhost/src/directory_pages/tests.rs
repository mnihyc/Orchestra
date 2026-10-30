//! Native metadata paging, lifetime and aggregate resource admission.

use super::*;
use crate::environment::{GateState, Gates, Limits};
use crate::index::HashCache;
use crate::paths::{PathStyle, Workspace};
use orch_types::ManualClock;

type Proof<T = ()> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

struct Rig {
    _temporary: tempfile::TempDir,
    root: PathBuf,
    files: Files,
    clock: ManualClock,
    cache: Arc<DirectoryPages>,
    binding: Binding,
}

impl Rig {
    fn new() -> Proof<Self> {
        let temporary = tempfile::tempdir()?;
        let root = temporary.path().join("workspace");
        std::fs::create_dir(&root)?;
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
            files,
            clock,
            cache,
            binding,
        })
    }

    async fn read(&self, binding: Binding, cursor: Option<&str>, limit: u32) -> Result<Page> {
        self.cache
            .read(binding, self.files.clone(), RelPath::root(), cursor, limit)
            .await
    }
}

#[tokio::test]
async fn every_sibling_is_discoverable_in_sorted_bounded_pages_of_one_revision() -> Proof {
    let rig = Rig::new()?;
    std::fs::create_dir(rig.root.join("Z-dir"))?;
    std::fs::create_dir(rig.root.join("a-dir"))?;
    for n in (0..5_003).rev() {
        std::fs::write(rig.root.join(format!("file-{n:05}")), b"")?;
    }
    let mut cursor = None;
    let mut names = Vec::new();
    let mut revision = None;
    let mut as_of = None;
    loop {
        let page = rig
            .read(rig.binding.clone(), cursor.as_deref(), 777)
            .await?;
        assert!(!page.entries.is_empty());
        assert!(page.entries.len() <= 777);
        assert!(serde_json::to_vec(&page.entries)?.len() < PAGE_BYTES);
        if let Some(revision) = revision {
            assert_eq!(page.revision, revision);
        } else {
            revision = Some(page.revision);
            as_of = Some(page.as_of);
        }
        assert_eq!(Some(page.as_of), as_of);
        names.extend(page.entries.into_iter().map(|entry| entry.name));
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
        assert!(names.len() <= 5_005, "a cursor must make progress");
    }
    let expected: Vec<_> = ["a-dir".to_owned(), "Z-dir".to_owned()]
        .into_iter()
        .chain((0..5_003).map(|n| format!("file-{n:05}")))
        .collect();
    assert_eq!(names, expected);
    let preview = rig.files.list(&RelPath::root())?;
    assert!(preview.truncated);
    assert_eq!(preview.entries.len(), FS_LIST_MAX_ENTRIES as usize);
    Ok(())
}

#[tokio::test]
async fn real_long_paths_hit_the_serialized_bound_before_the_row_bound_without_losing_siblings()
-> Proof {
    let rig = Rig::new()?;
    // Five legal 200-byte components plus 200-byte basenames make 2,000 rows
    // exceed two MiB while remaining well below the 5,000-row ceiling.
    let prefix = (0..5)
        .map(|n| format!("{n}{}", "p".repeat(199)))
        .collect::<Vec<_>>()
        .join("/");
    let directory = rig.root.join(&prefix);
    std::fs::create_dir_all(&directory)?;
    let names: Vec<_> = (0..2_000)
        .map(|n| format!("{}-{n:04}", "f".repeat(195)))
        .collect();
    for name in &names {
        std::fs::write(directory.join(name), b"x")?;
    }
    let path = rig.files.parse(&prefix)?;
    let binding = Binding {
        path: prefix,
        ..rig.binding.clone()
    };
    let first = rig
        .cache
        .read(
            binding.clone(),
            rig.files.clone(),
            path.clone(),
            None,
            FS_LIST_MAX_ENTRIES,
        )
        .await?;
    assert!(first.entries.len() < 2_000);
    assert!(first.next_cursor.is_some());
    assert!(serde_json::to_vec(&first.entries)?.len() <= ENTRY_BYTES);
    let mut found: Vec<_> = first.entries.into_iter().map(|entry| entry.name).collect();
    let mut cursor = first.next_cursor;
    while let Some(text) = cursor {
        let page = rig
            .cache
            .read(
                binding.clone(),
                rig.files.clone(),
                path.clone(),
                Some(&text),
                FS_LIST_MAX_ENTRIES,
            )
            .await?;
        assert!(!page.entries.is_empty());
        assert!(serde_json::to_vec(&page.entries)?.len() <= ENTRY_BYTES);
        found.extend(page.entries.into_iter().map(|entry| entry.name));
        assert!(found.len() <= 2_000);
        cursor = page.next_cursor;
    }
    assert_eq!(found, names);
    Ok(())
}

#[tokio::test]
async fn continuation_is_attachment_generation_mount_path_bound_and_actual_use_refreshes_five_minutes()
-> Proof {
    let rig = Rig::new()?;
    std::fs::write(rig.root.join("a"), b"a")?;
    std::fs::write(rig.root.join("b"), b"b")?;
    let first = rig.read(rig.binding.clone(), None, 1).await?;
    let cursor = first
        .next_cursor
        .as_deref()
        .ok_or("expected another sibling")?;
    for binding in [
        Binding {
            environment: new_id(),
            ..rig.binding.clone()
        },
        Binding {
            attachment: new_id(),
            ..rig.binding.clone()
        },
        Binding {
            generation: Some(5),
            ..rig.binding.clone()
        },
        Binding {
            mount: Some("other".to_owned()),
            ..rig.binding.clone()
        },
        Binding {
            path: "other".to_owned(),
            ..rig.binding.clone()
        },
    ] {
        assert!(matches!(
            rig.read(binding, Some(cursor), 1).await,
            Err(HostError::Stale { .. })
        ));
    }
    rig.clock.advance(chrono::Duration::minutes(4));
    let used = rig.read(rig.binding.clone(), Some(cursor), 1).await?;
    assert_eq!(used.entries[0].name, "b");
    assert_eq!(
        used.expires_at.get(),
        rig.clock.now() + chrono::Duration::minutes(5)
    );
    rig.clock.advance(chrono::Duration::minutes(4));
    assert_eq!(
        rig.read(rig.binding.clone(), Some(cursor), 1)
            .await?
            .entries[0]
            .name,
        "b"
    );
    rig.clock.advance(chrono::Duration::minutes(5));
    assert!(matches!(
        rig.read(rig.binding.clone(), Some(cursor), 1).await,
        Err(HostError::Stale { .. })
    ));
    assert_eq!(rig.cache.ledger.used.load(Ordering::Acquire), 0);
    assert_eq!(
        rig.read(rig.binding.clone(), None, 1).await?.entries[0].name,
        "a"
    );
    Ok(())
}

#[tokio::test]
async fn old_pages_keep_observed_membership_fresh_reads_observe_again_and_known_changes_or_eviction_restart()
-> Proof {
    let rig = Rig::new()?;
    std::fs::write(rig.root.join("a"), b"a")?;
    std::fs::write(rig.root.join("b"), b"b")?;
    let first = rig.read(rig.binding.clone(), None, 1).await?;
    let cursor = first
        .next_cursor
        .as_deref()
        .ok_or("expected continuation")?;
    std::fs::write(rig.root.join("c"), b"c")?;
    // Keep the actual insertion -> continuation interval unchanged. Inspect evidence only after
    // that read: a diagnostic sleep/stat/list must not make a timestamp gap disappear.
    let observed = rig.read(rig.binding.clone(), Some(cursor), 1).await;
    let kind = page_outcome(&observed);
    let code = observed.as_ref().err().map(HostError::code);
    let page = observed.as_ref().ok().map(|page| {
        (
            page.entries.len(),
            page.next_cursor.is_some(),
            page.revision == first.revision,
        )
    });
    eprintln!("directory membership result: outcome={kind} code={code:?} page_shape={page:?}");
    let token: Cursor = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(cursor)?)?;
    let captured = lock(&rig.cache.snapshots)
        .get(&token.snapshot)
        .map(|snapshot| snapshot.version.clone());
    let current = rig.files.directory_version(&RelPath::root())?;
    let mut names = std::fs::read_dir(&rig.root)?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<std::io::Result<Vec<_>>>()?;
    names.sort();
    assert_eq!(
        names,
        vec!["a", "b", "c"]
            .into_iter()
            .map(std::ffi::OsString::from)
            .collect::<Vec<_>>(),
        "the native directory really gained exactly the third sibling"
    );
    eprintln!(
        "directory membership version: captured={captured:?} current={current:?} native_names={}",
        names.len()
    );
    let old = observed?;
    assert_eq!(
        old.entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        ["b"],
        "an unobserved mutation must not append new native membership to an old observation"
    );
    assert!(old.next_cursor.is_none());
    assert_eq!(
        (old.revision, old.observed_from, old.as_of),
        (first.revision, first.observed_from, first.as_of)
    );
    let all = rig
        .read(rig.binding.clone(), None, FS_LIST_MAX_ENTRIES)
        .await?;
    assert_eq!(
        all.entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        ["a", "b", "c"],
        "a fresh first read must observe all three actual siblings"
    );
    rig.cache.invalidate(
        rig.binding.environment,
        rig.binding.attachment,
        &[rig.files.parse("c")?],
        false,
    );
    assert!(
        matches!(
            rig.read(rig.binding.clone(), Some(cursor), 1).await,
            Err(HostError::Stale { .. })
        ),
        "known change admission must invalidate independently of native timestamp equality"
    );
    let fresh = rig.read(rig.binding.clone(), None, 1).await?;
    let cursor = fresh
        .next_cursor
        .as_deref()
        .ok_or("the fresh three-sibling page must continue")?;
    rig.cache.forget(rig.binding.environment);
    assert_eq!(rig.cache.ledger.used.load(Ordering::Acquire), 0);
    let observed = rig.read(rig.binding.clone(), Some(cursor), 1).await;
    let kind = page_outcome(&observed);
    eprintln!(
        "directory eviction: outcome={kind} code={:?}",
        observed.as_ref().err().map(HostError::code)
    );
    assert!(
        matches!(observed, Err(HostError::Stale { .. })),
        "forgotten snapshot must be Stale, observed {kind}"
    );
    Ok(())
}

/// Variant names only: do not print error sentences, native paths, cursor bytes or content.
fn page_outcome(value: &Result<Page>) -> &'static str {
    match value {
        Ok(_) => "Ok(Page)",
        Err(error) => match error {
            HostError::Cancelled => "Cancelled",
            HostError::NotFound(_) => "NotFound",
            HostError::Ignored(_) => "Ignored",
            HostError::Exists(_) => "Exists",
            HostError::Stale { .. } => "Stale",
            HostError::Changed(_) => "Changed",
            HostError::RestoreRecoveryRequired(_) => "RestoreRecoveryRequired",
            HostError::TooLarge { .. } => "TooLarge",
            HostError::Unsupported(_) => "Unsupported",
            HostError::RemoteWriteDisabled => "RemoteWriteDisabled",
            HostError::DeliverFilesDisabled => "DeliverFilesDisabled",
            HostError::RootNotAllowed(_) => "RootNotAllowed",
            HostError::ExecNotOffered(_) => "ExecNotOffered",
            HostError::McpImportDisabled => "McpImportDisabled",
            HostError::McpAddDisabled => "McpAddDisabled",
            HostError::Timeout(_) => "Timeout",
            HostError::Busy(_) => "Busy",
            HostError::Content(_) => "Content",
            HostError::Io { .. } => "Io",
            HostError::Internal(_) => "Internal",
        },
    }
}

fn entry(name: &str, payload: usize, at: Time) -> DirEntry {
    DirEntry {
        name: name.to_owned(),
        path: name.to_owned(),
        kind: DirEntryKind::Symlink,
        size: 1,
        mtime: at,
        mode: 0o777,
        link_target: Some("x".repeat(payload)),
        cached: false,
    }
}

#[test]
fn serialized_bytes_split_before_row_limit_and_merge_quota_includes_all_files() -> Proof {
    let rig = Rig::new()?;
    std::fs::create_dir_all(&rig.cache.directory)?;
    assert_eq!(rig.cache.ledger.maximum, 1024 * 1024 * 1024);
    let cancel = CancellationToken::new();
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut builder = Builder {
        cache: &rig.cache,
        rows: Vec::new(),
        bytes: 0,
        levels: Vec::new(),
    };
    for name in ["c", "a", "b"] {
        builder
            .rows
            .push(Record::new(entry(name, 800_000, rig.clock.time()))?);
        builder.flush(&cancel, deadline)?;
    }
    let metadata = builder.finish(&cancel, deadline)?;
    assert_eq!(
        rig.cache.ledger.used.load(Ordering::Acquire),
        metadata.bytes,
        "merge inputs must close/release their reservations"
    );
    let snapshot = Snapshot {
        id: new_id(),
        binding: rig.binding.clone(),
        version: rig.files.directory_version(&RelPath::root())?,
        metadata: Mutex::new(metadata),
        revision: Hash::of(b"synthetic immutable metadata"),
        as_of: rig.clock.time(),
        observed_from: rig.clock.time(),
        last_use: AtomicI64::new(rig.clock.now().timestamp_millis()),
        observation: Arc::default(),
    };
    let first = rig
        .cache
        .page(&snapshot, 0, FS_LIST_MAX_ENTRIES, &cancel, deadline)?;
    assert_eq!(
        first
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        ["a", "b"]
    );
    assert!(serde_json::to_vec(&first.entries)?.len() <= ENTRY_BYTES);
    let encoded = first.next_cursor.ok_or("byte bound needs a continuation")?;
    let cursor: Cursor = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(encoded)?)?;
    let last = rig.cache.page(
        &snapshot,
        cursor.offset,
        FS_LIST_MAX_ENTRIES,
        &cancel,
        deadline,
    )?;
    assert_eq!(last.entries[0].name, "c");
    assert!(last.next_cursor.is_none());
    drop(snapshot);
    assert_eq!(rig.cache.ledger.used.load(Ordering::Acquire), 0);
    Ok(())
}

#[test]
fn pinned_and_intermediate_work_is_busy_instead_of_exceeding_one_host_quota() -> Proof {
    let mut rig = Rig::new()?;
    let cache = Arc::get_mut(&mut rig.cache).ok_or("cache unexpectedly shared")?;
    cache.ledger = Arc::new(Ledger {
        used: AtomicU64::new(0),
        maximum: 128,
    });
    std::fs::create_dir_all(&cache.directory)?;
    let mut kept = Stored::new(cache)?;
    kept.write(cache, &[1; 80])?;
    let mut merge_output = Stored::new(cache)?;
    assert!(matches!(
        merge_output.write(cache, &[2; 49]),
        Err(HostError::Busy(_))
    ));
    assert_eq!(cache.ledger.used.load(Ordering::Acquire), 80);
    merge_output.write(cache, &[2; 48])?;
    assert_eq!(cache.ledger.used.load(Ordering::Acquire), 128);
    drop(kept);
    assert_eq!(cache.ledger.used.load(Ordering::Acquire), 48);
    drop(merge_output);
    assert_eq!(cache.ledger.used.load(Ordering::Acquire), 0);
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(matches!(
        check(&cancel, Instant::now() + Duration::from_secs(15)),
        Err(HostError::Cancelled)
    ));
    Ok(())
}

#[tokio::test]
async fn cancelling_a_builder_releases_its_intermediate_files_without_publishing_a_snapshot()
-> Proof {
    let rig = Rig::new()?;
    std::fs::create_dir_all(&rig.cache.directory)?;
    let cancel = CancellationToken::new();
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut builder = Builder {
        cache: &rig.cache,
        rows: vec![Record::new(entry("a", 100, rig.clock.time()))?],
        bytes: 100,
        levels: Vec::new(),
    };
    builder.flush(&cancel, deadline)?;
    assert!(rig.cache.ledger.used.load(Ordering::Acquire) > 0);
    builder
        .rows
        .push(Record::new(entry("b", 100, rig.clock.time()))?);
    cancel.cancel();
    assert!(matches!(
        builder.flush(&cancel, deadline),
        Err(HostError::Cancelled)
    ));
    drop(builder);
    assert_eq!(rig.cache.ledger.used.load(Ordering::Acquire), 0);
    assert!(lock(&rig.cache.snapshots).is_empty());
    Ok(())
}

#[tokio::test]
async fn lru_evicts_only_unpinned_metadata_and_forget_keeps_active_quota_charged() -> Proof {
    let rig = Rig::new()?;
    std::fs::write(rig.root.join("a"), b"a")?;
    let first = rig.read(rig.binding.clone(), None, 1).await?;
    let pinned = lock(&rig.cache.snapshots)
        .values()
        .next()
        .cloned()
        .ok_or("first snapshot")?;
    rig.clock.advance(chrono::Duration::seconds(1));
    rig.read(rig.binding.clone(), None, 1).await?;
    assert!(rig.cache.evict_one());
    assert_eq!(lock(&rig.cache.snapshots).len(), 1);
    assert!(lock(&rig.cache.snapshots).contains_key(&pinned.id));
    assert!(!rig.cache.evict_one());
    let bytes = lock(&pinned.metadata).bytes;
    rig.cache.forget(rig.binding.environment);
    assert_eq!(rig.cache.ledger.used.load(Ordering::Acquire), bytes);
    assert_eq!(first.revision, pinned.revision);
    drop(pinned);
    assert_eq!(rig.cache.ledger.used.load(Ordering::Acquire), 0);
    Ok(())
}
