//! The declared peer's file blobs, actual note bytes and production-format manifest.
//! This is a controlled blob supplier; it does not claim a real envhost copy.

use super::*;
use orch_db::queries::{environments::thread_engine, storage};
use orch_engine::manifest::{self, Entry, EntryKind, Header};

pub(super) async fn uploaded(
    environment_id: Id,
    selection: &orch_link_contract::commands::Selection,
    physical_root: Option<&str>,
    store: &dyn BlobStore,
    pool: &orch_db::Db,
    clock: &dyn orch_types::Clock,
) -> Result<orch_link_contract::commands::FsSnapshotResult, CommandError> {
    let environment = {
        let mut conn = pool
            .acquire()
            .await
            .map_err(|_| refusal("snapshot environment metadata is unavailable"))?;
        thread_engine::environment(&mut conn, environment_id)
            .await
            .map_err(|_| refusal("snapshot environment metadata could not be read"))?
            .filter(|environment| environment.deleted_at.is_none())
            .ok_or_else(|| refusal("the snapshot environment is unavailable"))?
    };
    let matcher = orch_engine::selection::Matcher::new(selection)
        .map_err(|_| refusal("the scripted snapshot selection is invalid"))?;
    // The declared workspace files stay below the first selection; notes may be a sibling selection.
    let prefixes: Vec<String> = selection
        .include
        .iter()
        .map(|pattern| {
            pattern
                .split('/')
                .take_while(|part| !part.contains(['*', '?']))
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join("/")
        })
        .collect();
    let primary = prefixes.first().map_or("", String::as_str);
    let mut common = primary
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    for prefix in prefixes.iter().skip(1) {
        let shared = common
            .iter()
            .copied()
            .zip(prefix.split('/').filter(|part| !part.is_empty()))
            .take_while(|(left, right)| left == right)
            .count();
        common.truncate(shared);
    }
    let prefix = common.join("/");
    let below = |base: &str, name: &str| {
        if base.is_empty() {
            name.to_owned()
        } else {
            format!("{base}/{name}")
        }
    };
    let is_notes = |base: &str| base == ".orch/memory" || base.ends_with("/.orch/memory");
    let now = clock.now();
    let mut entries = Vec::new();
    let mut registered = Vec::new();
    let mut uploaded_bytes = 0;
    let mut fixture_files = if is_notes(primary) {
        Vec::new()
    } else {
        vec![
            (below(primary, "a.txt"), b"source-a0\n".to_vec()),
            (below(primary, "b.txt"), b"source-b0\n".to_vec()),
            (below(primary, "c.txt"), b"source-c0\n".to_vec()),
        ]
    };
    let mut notes = prefixes
        .iter()
        .filter(|base| is_notes(base))
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    if notes.is_empty() {
        notes.insert(below(primary, ".orch/memory"));
    }
    if let Some(root) = physical_root {
        for directory in notes {
            let path = below(&directory, "MEMORY.md");
            if let Ok(bytes) = std::fs::read(std::path::Path::new(root).join(&path)) {
                fixture_files.push((path, bytes));
            }
        }
    }
    for (path, bytes) in fixture_files {
        if !matcher.matches(&path) {
            continue;
        }
        let name = if prefix.is_empty() {
            path.as_str()
        } else {
            path.strip_prefix(&format!("{prefix}/"))
                .ok_or_else(|| refusal("the scripted files leave their common source directory"))?
        };
        let hash = Hash::of(&bytes);
        let size =
            i64::try_from(bytes.len()).map_err(|_| refusal("the scripted file is too large"))?;
        let prior = store
            .size(&hash)
            .await
            .map_err(|_| refusal("the scripted file upload could not be checked"))?;
        if prior.is_some_and(|prior| prior != size) {
            return Err(refusal("the scripted file has conflicting stored metadata"));
        }
        store
            .put(
                &hash,
                Some(size),
                orch_blob_contract::store::stream_of(vec![bytes.into()]),
            )
            .await
            .map_err(|_| refusal("the scripted file upload failed"))?;
        if prior.is_none() {
            uploaded_bytes += size;
        }
        registered.push((hash, size));
        entries.push(Entry {
            path: name.to_owned(),
            kind: EntryKind::File,
            size,
            mtime_ns: 0,
            mode: manifest::DEFAULT_FILE_MODE,
            hash: Some(hash),
            link: None,
        });
    }
    // Epoch mtime is the fixture's immutable source-file metadata; sampling a
    // tree again must not pretend that its unchanged file contents changed.
    let written = manifest::build(
        &Header {
            v: manifest::MANIFEST_VERSION,
            root: orch_engine::sessions::absolute_directory(
                physical_root.unwrap_or(&environment.workspace_root),
                &prefix,
            ),
            generated_at: clock.time(),
            generation: i64::from(environment.generation),
            case_sensitive: true,
        },
        entries,
    )
    .map_err(|_| refusal("the scripted manifest could not be encoded"))?;
    let manifest_size = i64::try_from(written.bytes.len())
        .map_err(|_| refusal("the scripted manifest is too large"))?;
    store
        .put(
            &written.hash,
            Some(manifest_size),
            orch_blob_contract::store::stream_of(vec![written.bytes.into()]),
        )
        .await
        .map_err(|_| refusal("the scripted manifest upload failed"))?;
    registered.push((written.hash, manifest_size));
    // All blob I/O ends before borrowing this registration connection. The real
    // consumer will read/hash/parse the uploaded manifest and own its pins.
    {
        let mut conn = pool
            .acquire()
            .await
            .map_err(|_| refusal("snapshot blob metadata is unavailable"))?;
        for (hash, size) in registered {
            storage::register_blob(&mut conn, &hash, size, now)
                .await
                .map_err(|_| refusal("snapshot blob metadata could not be registered"))?;
        }
    }
    Ok(orch_link_contract::commands::FsSnapshotResult {
        manifest_blob: written.hash,
        file_count: written.file_count,
        total_bytes: written.total_bytes,
        uploaded_bytes,
        missing: 0,
    })
}
