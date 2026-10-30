//! The declared peer's three actual file blobs and production-format manifest.
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
    // These cases select a whole directory or src/**. A native snapshot may use
    // that narrower common directory; the ordinary rebase supplier handles it.
    let first = selection.include.first().map_or("", String::as_str);
    let prefix = first
        .split('/')
        .take_while(|part| !part.contains(['*', '?']))
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("/");
    let now = clock.now();
    let mut entries = Vec::new();
    let mut registered = Vec::new();
    let mut uploaded_bytes = 0;
    for (name, bytes) in [
        ("a.txt", b"source-a0\n".as_slice()),
        ("b.txt", b"source-b0\n".as_slice()),
        ("c.txt", b"source-c0\n".as_slice()),
    ] {
        let path = if prefix.is_empty() {
            name.to_owned()
        } else {
            format!("{prefix}/{name}")
        };
        if !matcher.matches(&path) {
            continue;
        }
        let hash = Hash::of(bytes);
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
                orch_blob_contract::store::stream_of(vec![bytes.to_vec().into()]),
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
