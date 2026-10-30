//! Exact transcript ownership, measurements and removal in managed Claude Code homes.

use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use orch_agent_contract::adapter::AdapterError;
use orch_agent_contract::history::{NativeHistoryFact, NativeHomeCleanupItem, NativePrefixSource};
use orch_types::{Id, Time};

/// Only vendor-documented disposable leftovers; transcripts, memory, skills and sign-in stay outside this list.
const CACHE_ROOTS: &[&str] = &[
    "debug",
    "paste-cache",
    "shell-snapshots",
    "session-env",
    "usage-data",
    "backups",
    "feedback-bundles",
    "feedback/drafts",
    "statsig",
    "logs",
];

fn failure(code: &str) -> AdapterError {
    AdapterError::Other {
        code: code.into(),
        message: "The exact native history could not be managed safely.".into(),
        retryable: false,
    }
}
fn io_error(error: io::Error) -> AdapterError {
    AdapterError::HarnessError(format!("Native history storage: {error}"))
}

fn checked(home: &Path, path: &Path) -> Result<PathBuf, AdapterError> {
    if !home.is_absolute()
        || std::fs::symlink_metadata(home)
            .map_err(io_error)?
            .file_type()
            .is_symlink()
    {
        return Err(failure("history.unmanaged_home"));
    }
    let relative = path
        .strip_prefix(home)
        .map_err(|_| failure("history.outside_home"))?;
    if relative
        .components()
        .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(failure("history.outside_home"));
    }
    let mut current = home.to_path_buf();
    for part in relative.components() {
        current.push(part);
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(failure("history.symlink"));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error(error)),
        }
    }
    Ok(path.to_path_buf())
}

/// The vendor's alternate transcripts, indexed on disk once per home batch.
#[derive(Default)]
pub(crate) struct Inventory(Mutex<Option<AlternatesIndex>>);

#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
struct Extent {
    offset: u64,
    length: u64,
}
#[derive(serde::Serialize, serde::Deserialize)]
struct Alternate {
    path: PathBuf,
    next: Option<Extent>,
}
struct AlternatesIndex {
    heads: crate::history::spool::Map<Extent>,
    paths: std::fs::File,
    end: u64,
}

impl Inventory {
    fn add(&mut self, native: &str, path: PathBuf) -> Result<(), AdapterError> {
        let inner = self.0.get_mut().unwrap_or_else(PoisonError::into_inner);
        if inner.is_none() {
            *inner = Some(AlternatesIndex {
                heads: crate::history::spool::Map::with_cancel(Arc::default()).map_err(io_error)?,
                paths: tempfile::tempfile().map_err(io_error)?,
                end: 0,
            });
        }
        let Some(index) = inner else {
            return Err(failure("history.inventory_unavailable"));
        };
        let next = index.heads.get(native).map_err(io_error)?;
        let offset = index.end;
        index
            .paths
            .seek(SeekFrom::Start(offset))
            .map_err(io_error)?;
        {
            // Native path tokens and the disk cursor are the only retained inventory values.
            const PAGE_BYTES: usize = 64 * 1024;
            let mut writer = BufWriter::with_capacity(PAGE_BYTES, &mut index.paths);
            serde_json::to_writer(&mut writer, &Alternate { path, next })
                .map_err(|error| io_error(io::Error::other(error)))?;
            writer.flush().map_err(io_error)?;
        }
        index.end = index.paths.stream_position().map_err(io_error)?;
        index
            .heads
            .insert(
                native,
                &Extent {
                    offset,
                    length: index.end - offset,
                },
            )
            .map_err(io_error)
    }

    fn alternates(&self, native: &str) -> Result<Alternates<'_>, AdapterError> {
        let mut index = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let next = match &mut *index {
            Some(index) => index.heads.get(native).map_err(io_error)?,
            None => None,
        };
        Ok(Alternates {
            inventory: self,
            next,
        })
    }
}

struct Alternates<'a> {
    inventory: &'a Inventory,
    next: Option<Extent>,
}
impl Iterator for Alternates<'_> {
    type Item = Result<PathBuf, AdapterError>;
    fn next(&mut self) -> Option<Self::Item> {
        let next = self.next.take()?;
        let result = (|| {
            let mut index = self
                .inventory
                .0
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let index = index
                .as_mut()
                .ok_or_else(|| failure("history.inventory_unavailable"))?;
            index
                .paths
                .seek(SeekFrom::Start(next.offset))
                .map_err(io_error)?;
            const PAGE_BYTES: usize = 64 * 1024;
            let path: Alternate = serde_json::from_reader(BufReader::with_capacity(
                PAGE_BYTES,
                (&mut index.paths).take(next.length),
            ))
            .map_err(|error| io_error(io::Error::other(error)))?;
            self.next = path.next;
            Ok(path.path)
        })();
        Some(result)
    }
}

/// Finds exact UUID-owned alternate names in the one managed project directory.
pub(crate) fn inventory(home: &Path) -> Result<Inventory, AdapterError> {
    let directory = checked(
        home,
        &home
            .join("projects")
            .join(crate::config::MANAGED_PROJECT_DIR),
    )?;
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Inventory::default()),
        Err(error) => return Err(io_error(error)),
    };
    let mut inventory = Inventory::default();
    for entry in entries {
        let entry = entry.map_err(io_error)?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        // UUID's canonical width is part of the native identity format, not a history limit.
        const UUID_CHARS: usize = 36;
        let Some(native) = name
            .get(..UUID_CHARS)
            .filter(|native| Id::parse_str(native).is_ok())
        else {
            continue;
        };
        let suffix = &name[UUID_CHARS..];
        if suffix.starts_with(".jsonl.superseded-")
            || suffix.starts_with(".orphaned-") && suffix.ends_with(".jsonl")
        {
            inventory.add(native, entry.path())?;
        }
    }
    Ok(inventory)
}

fn roots(home: &Path, native: &str) -> Result<Vec<PathBuf>, AdapterError> {
    Id::parse_str(native).map_err(|_| failure("history.invalid_identity"))?;
    let transcript = crate::sessions::managed_transcript(home, native);
    let roots = vec![
        transcript.clone(),
        transcript.with_extension(""),
        home.join("file-history").join(native),
        home.join("image-cache").join(native),
        home.join("uploads").join(native),
        home.join("dev-mods").join(native),
        home.join("tasks").join(native),
        home.join("debug").join(format!("{native}.txt")),
        home.join("session-env").join(native),
        home.join("session-env").join(format!("{native}.json")),
    ];
    roots.into_iter().map(|path| checked(home, &path)).collect()
}

#[derive(Default)]
struct Walked {
    bytes: i64,
    files: u64,
    peak_paths: usize,
}

fn visit(
    home: &Path,
    path: &Path,
    remove: bool,
    depth: usize,
    walked: &mut Walked,
) -> Result<(), AdapterError> {
    let path = checked(home, path)?;
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(io_error(error)),
    };
    walked.peak_paths = walked.peak_paths.max(depth);
    if metadata.is_dir() {
        // Keep one directory cursor per ancestor; directory width does not retain a path list.
        for entry in std::fs::read_dir(&path).map_err(io_error)? {
            visit(
                home,
                &entry.map_err(io_error)?.path(),
                remove,
                depth + 1,
                walked,
            )?;
        }
        if remove {
            checked(home, &path)?;
            match std::fs::remove_dir(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(io_error(error)),
            }
        }
    } else if metadata.is_file() {
        walked.bytes = walked
            .bytes
            .checked_add(i64::try_from(metadata.len()).map_err(|_| failure("history.too_large"))?)
            .ok_or_else(|| failure("history.too_large"))?;
        walked.files = walked.files.saturating_add(1);
        if remove {
            checked(home, &path)?;
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(io_error(error)),
            }
        }
    } else {
        return Err(failure("history.special_file"));
    }
    Ok(())
}

fn entries(home: &Path, roots: &[PathBuf], remove: bool) -> Result<Walked, AdapterError> {
    let mut walked = Walked::default();
    for path in roots {
        visit(home, path, remove, 1, &mut walked)?;
    }
    Ok(walked)
}

/// Removes exactly this server-owned session's documented native file set.
pub(crate) fn manage(
    home: &Path,
    inventory: &Inventory,
    source: NativePrefixSource,
    remove: bool,
) -> Result<NativeHistoryFact, AdapterError> {
    let roots = roots(home, &source.native_session_ref)?;
    // Validate the complete exact file set before the first irreversible act. Both passes stream alternates.
    let mut measured = entries(home, &roots, false)?;
    for path in inventory.alternates(&source.native_session_ref)? {
        visit(home, &path?, false, 1, &mut measured)?;
    }
    if remove {
        entries(home, &roots, true)?;
        let mut removed = Walked::default();
        for path in inventory.alternates(&source.native_session_ref)? {
            visit(home, &path?, true, 1, &mut removed)?;
        }
    }
    Ok(NativeHistoryFact {
        source,
        bytes: Some(measured.bytes),
        removed: remove || measured.bytes == 0,
        error: None,
    })
}

fn cleanup_path(
    home: &Path,
    file: &Path,
    before: Time,
    item: &mut NativeHomeCleanupItem,
) -> Result<(), AdapterError> {
    checked(
        home,
        file.parent()
            .ok_or_else(|| failure("history.outside_home"))?,
    )?;
    let metadata = match std::fs::symlink_metadata(file) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(io_error(error)),
    };
    if metadata.file_type().is_symlink() {
        item.error.get_or_insert_with(|| "history.symlink".into());
        return Ok(());
    }
    let file = checked(home, file)?;
    if metadata.is_dir() {
        for entry in std::fs::read_dir(file).map_err(io_error)? {
            cleanup_path(home, &entry.map_err(io_error)?.path(), before, item)?;
        }
        return Ok(());
    }
    if !metadata.is_file() {
        return Err(failure("history.special_file"));
    }
    let changed: chrono::DateTime<chrono::Utc> = metadata.modified().map_err(io_error)?.into();
    if changed < before.get() {
        let bytes = i64::try_from(metadata.len()).map_err(|_| failure("history.too_large"))?;
        std::fs::remove_file(file).map_err(io_error)?;
        item.files = item.files.saturating_add(1);
        item.bytes = item.bytes.saturating_add(bytes);
    }
    Ok(())
}

/// Walks only documented cache roots using modification age, with confirmed partial counts.
pub(crate) fn cleanup_home(home: &Path, before: Time) -> Vec<NativeHomeCleanupItem> {
    CACHE_ROOTS
        .iter()
        .map(|root| {
            let mut item = NativeHomeCleanupItem {
                directory: (*root).into(),
                files: 0,
                bytes: 0,
                error: None,
            };
            if let Err(error) = cleanup_path(home, &home.join(root), before, &mut item) {
                item.error = Some(error.code().into());
            }
            item
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn source() -> NativePrefixSource {
        NativePrefixSource {
            thread_id: orch_types::new_id(),
            segment_id: orch_types::new_id(),
            native_session_ref: "11111111-1111-4111-8111-111111111111".into(),
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_hundred_thousand_alternate_transcripts_use_a_disk_index_and_streamed_file_sets() {
        const CHILD: &str = "ORCH_NATIVE_ALTERNATE_SCALE_CHILD";
        if std::env::var_os(CHILD).is_none() {
            // An isolated process makes the retained-memory assertion independent of parallel test cases.
            let output = std::process::Command::new("timeout")
                .arg("10m").arg(std::env::current_exe().unwrap())
                .args(["--exact", "native_history::tests::a_hundred_thousand_alternate_transcripts_use_a_disk_index_and_streamed_file_sets", "--test-threads=1"])
                .env(CHILD, "1").output().unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        fn peak_bytes() -> u64 {
            std::fs::read_to_string("/proc/self/status")
                .unwrap()
                .lines()
                .find_map(|line| {
                    line.strip_prefix("VmHWM:")
                        .and_then(|value| value.split_whitespace().next())
                        .and_then(|value| value.parse::<u64>().ok())
                })
                .unwrap()
                * 1024
        }
        const FILES: u64 = 100_000;
        // Eight MiB permits allocator/I/O overhead while rejecting a retained 100,000-path vector.
        const RETAINED_BUDGET: u64 = 8 * 1024 * 1024;
        let home = tempfile::tempdir().unwrap();
        let source = source();
        let project = home.path().join("projects/orch");
        std::fs::create_dir_all(&project).unwrap();
        let canonical = project.join(format!("{}.jsonl", source.native_session_ref));
        std::fs::write(&canonical, "root").unwrap();
        for entry in 0..FILES {
            std::fs::write(
                project.join(format!(
                    "{}.jsonl.superseded-{entry:08}",
                    source.native_session_ref
                )),
                "a",
            )
            .unwrap();
        }
        let other = project.join("22222222-2222-4222-8222-222222222222.jsonl.superseded-kept");
        std::fs::write(&other, "another native session").unwrap();
        let before = peak_bytes();
        let index = inventory(home.path()).unwrap();
        let mut count = 0;
        for path in index.alternates(&source.native_session_ref).unwrap() {
            assert!(
                path.unwrap()
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .starts_with(&source.native_session_ref)
            );
            count += 1;
        }
        assert_eq!(count, FILES);
        assert!(
            peak_bytes().saturating_sub(before) < RETAINED_BUDGET,
            "native alternate metadata must stay on disk"
        );
        let fact = manage(home.path(), &index, source.clone(), true).unwrap();
        assert_eq!(fact.bytes, Some(i64::try_from(FILES + 4).unwrap()));
        assert!(fact.removed);
        assert!(!canonical.exists());
        assert_eq!(
            std::fs::read_to_string(other).unwrap(),
            "another native session"
        );
        assert_eq!(std::fs::read_dir(project).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn an_unsafe_alternate_refuses_the_whole_removal_before_any_native_file_is_deleted() {
        let home = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let source = source();
        let canonical =
            crate::sessions::managed_transcript(home.path(), &source.native_session_ref);
        std::fs::create_dir_all(canonical.parent().unwrap()).unwrap();
        std::fs::write(&canonical, "keep the native conversation").unwrap();
        let target = outside.path().join("keep");
        std::fs::write(&target, "outside").unwrap();
        let alternate = canonical.with_file_name(format!(
            "{}.jsonl.superseded-unsafe",
            source.native_session_ref
        ));
        std::os::unix::fs::symlink(&target, alternate).unwrap();
        let index = inventory(home.path()).unwrap();
        let result = manage(home.path(), &index, source, true).unwrap_err();
        assert_eq!(result.code(), "history.symlink");
        assert_eq!(
            std::fs::read_to_string(canonical).unwrap(),
            "keep the native conversation"
        );
        assert_eq!(std::fs::read_to_string(target).unwrap(), "outside");
    }

    #[test]
    fn old_unloaded_plan_and_task_state_is_not_a_disposable_home_cache() {
        let home = tempfile::tempdir().unwrap();
        let old: std::time::SystemTime =
            chrono::DateTime::parse_from_rfc3339("2026-10-01T00:00:00Z")
                .unwrap()
                .into();
        for relative in ["plans/needed.md", "todos/needed.json", "debug/old.log"] {
            let file = home.path().join(relative);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(&file, "kept state").unwrap();
            std::fs::File::options()
                .write(true)
                .open(file)
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_modified(old))
                .unwrap();
        }
        let result = cleanup_home(home.path(), "2026-10-02T00:00:00Z".parse().unwrap());
        assert!(result.iter().all(|item| item.error.is_none()));
        assert!(!home.path().join("debug/old.log").exists());
        assert!(home.path().join("plans/needed.md").exists());
        assert!(home.path().join("todos/needed.json").exists());
    }

    #[test]
    fn the_vendor_file_set_is_freed_and_every_other_session_and_home_file_survives() {
        let home = tempfile::tempdir().unwrap();
        let source = source();
        let native = &source.native_session_ref;
        let project = home.path().join("projects/orch");
        let sources = [
            format!("projects/orch/{native}.jsonl"),
            format!("projects/orch/{native}.orphaned-20261001-backup.jsonl"),
            format!("projects/orch/{native}.jsonl.superseded-20261001"),
            format!("projects/orch/{native}/subagents/agent.jsonl"),
            format!("projects/orch/{native}/tool-results/result.txt"),
            format!("projects/orch/{native}/mcp-tasks/task.json"),
            format!("file-history/{native}/before"),
            format!("image-cache/{native}/image.png"),
            format!("uploads/{native}/uploaded.txt"),
            format!("dev-mods/{native}/mod.txt"),
            format!("tasks/{native}/task.json"),
            format!("session-env/{native}.json"),
            format!("debug/{native}.txt"),
        ];
        let expected_bytes = i64::try_from(sources.len()).unwrap() * 5;
        for file in sources {
            let path = home.path().join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "12345").unwrap();
        }
        std::fs::write(project.join("other.jsonl"), "keep").unwrap();
        std::fs::write(home.path().join(".credentials.json"), "keep sign-in").unwrap();
        let inventory = inventory(home.path()).unwrap();
        assert_eq!(
            manage(home.path(), &inventory, source.clone(), false)
                .unwrap()
                .bytes,
            Some(expected_bytes)
        );
        let result = manage(home.path(), &inventory, source.clone(), true).unwrap();
        assert_eq!(result.bytes, Some(expected_bytes));
        assert!(result.removed);
        assert!(!crate::sessions::managed_transcript(home.path(), native).exists());
        assert!(!project.join(native).exists());
        assert!(!home.path().join("file-history").join(native).exists());
        assert_eq!(
            std::fs::read_to_string(project.join("other.jsonl")).unwrap(),
            "keep"
        );
        assert_eq!(
            std::fs::read_to_string(home.path().join(".credentials.json")).unwrap(),
            "keep sign-in"
        );
        assert!(
            manage(home.path(), &inventory, source, true)
                .unwrap()
                .removed
        );
    }

    #[test]
    fn home_cleanup_frees_only_old_documented_caches_and_preserves_recent_data_and_credentials() {
        use std::fs::FileTimes;
        let home = tempfile::tempdir().unwrap();
        let old = home.path().join("debug/old.txt");
        std::fs::create_dir_all(old.parent().unwrap()).unwrap();
        std::fs::write(&old, "old bytes").unwrap();
        let old_time: std::time::SystemTime =
            chrono::DateTime::parse_from_rfc3339("2026-10-01T00:00:00Z")
                .unwrap()
                .into();
        std::fs::File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_times(FileTimes::new().set_modified(old_time))
            .unwrap();
        let recent = home.path().join("debug/recent.txt");
        std::fs::write(&recent, "recent").unwrap();
        std::fs::write(home.path().join(".credentials.json"), "sign in").unwrap();
        let results = cleanup_home(home.path(), "2026-10-02T00:00:00Z".parse().unwrap());
        let debug = results
            .iter()
            .find(|item| item.directory == "debug")
            .unwrap();
        assert_eq!((debug.files, debug.bytes), (1, 9));
        assert!(!old.exists());
        assert!(recent.exists());
        assert!(home.path().join(".credentials.json").exists());
        assert!(results.iter().all(|item| item.error.is_none()));
    }

    #[test]
    fn a_link_never_authorizes_removing_anything_outside_the_home() {
        let home = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(home.path(), home.path().join("projects")).unwrap();
            assert_eq!(
                manage(home.path(), &Inventory::default(), source(), true)
                    .unwrap_err()
                    .code(),
                "history.symlink"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn cache_cleanup_preserves_links_and_their_targets_while_reclaiming_old_regular_files() {
        use std::fs::FileTimes;
        let home = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let old = home.path().join("debug/old.txt");
        std::fs::create_dir_all(old.parent().unwrap()).unwrap();
        let target = outside.path().join("keep.txt");
        std::fs::write(&old, "old bytes").unwrap();
        std::fs::write(&target, "keep these bytes").unwrap();
        let old_time: std::time::SystemTime =
            chrono::DateTime::parse_from_rfc3339("2026-10-01T00:00:00Z")
                .unwrap()
                .into();
        for file in [&old, &target] {
            std::fs::File::options()
                .write(true)
                .open(file)
                .unwrap()
                .set_times(FileTimes::new().set_modified(old_time))
                .unwrap();
        }
        let pointer = home.path().join("debug/latest");
        std::os::unix::fs::symlink(&target, &pointer).unwrap();
        std::os::unix::fs::symlink(outside.path(), home.path().join("paste-cache")).unwrap();
        let results = cleanup_home(home.path(), "2026-10-02T00:00:00Z".parse().unwrap());
        let debug = results
            .iter()
            .find(|item| item.directory == "debug")
            .unwrap();
        assert_eq!((debug.files, debug.bytes), (1, 9));
        assert_eq!(debug.error.as_deref(), Some("history.symlink"));
        assert!(!old.exists());
        assert!(
            std::fs::symlink_metadata(pointer)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read_to_string(target).unwrap(), "keep these bytes");
        let refused = results
            .iter()
            .find(|item| item.directory == "paste-cache")
            .unwrap();
        assert_eq!(refused.error.as_deref(), Some("history.symlink"));
        assert_eq!((refused.files, refused.bytes), (0, 0));
    }
    #[test]
    fn a_hundred_thousand_sparse_tool_results_are_measured_and_removed_without_a_file_list() {
        // More than 10 billion logical bytes need only metadata reads, with one live path per depth.
        const FILES: u64 = 100_000;
        const FILE_BYTES: u64 = 1024 * 1024;
        let home = tempfile::tempdir().unwrap();
        let source = source();
        let session = crate::sessions::managed_transcript(home.path(), &source.native_session_ref)
            .with_extension("");
        let tool_results = session.join("tool-results");
        std::fs::create_dir_all(&tool_results).unwrap();
        for index in 0..FILES {
            std::fs::File::create(tool_results.join(format!("result-{index}")))
                .unwrap()
                .set_len(FILE_BYTES)
                .unwrap();
        }
        let unrelated = home.path().join("keep-this-file");
        std::fs::write(&unrelated, "another session's retained bytes").unwrap();
        let inventory = Inventory::default();
        let roots = roots(home.path(), &source.native_session_ref).unwrap();
        let measured = entries(home.path(), &roots, false).unwrap();
        assert_eq!(measured.files, FILES);
        assert_eq!(measured.bytes, i64::try_from(FILES * FILE_BYTES).unwrap());
        assert_eq!(
            measured.peak_paths, 3,
            "directory width cannot retain a whole file list"
        );
        let removed = manage(home.path(), &inventory, source, true).unwrap();
        assert_eq!(removed.bytes, Some(measured.bytes));
        assert!(removed.removed);
        assert!(!session.exists());
        assert_eq!(
            std::fs::read_to_string(unrelated).unwrap(),
            "another session's retained bytes"
        );
    }
}
