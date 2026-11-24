//! Metadata-only observations of the configured data directory, independent of credential readers.
use orch_api_contract::storage::{
    MachineDataInventory, MachineDataKind, MachineDataPart, MachineDataProblem,
};
use orch_types::Time;
use std::path::Path;

/// A component is normally one of the nine standard directories. Keep additional local entries
/// bounded within the existing facts frame and report every omission explicitly.
const PARTS_MAX: usize = 32;

fn kind(name: &std::ffi::OsStr) -> MachineDataKind {
    match name.to_str() {
        Some("environment-data") => MachineDataKind::EnvironmentData,
        Some("harness") => MachineDataKind::ProviderHomes,
        Some("mcp") => MachineDataKind::McpCopies,
        Some("storage-bindings") => MachineDataKind::StorageBindings,
        Some("logs") => MachineDataKind::Logs,
        // What is put aside for an earlier unit is its identity and what its key sealed.
        Some("secrets" | "identity" | "previous") => MachineDataKind::SealedStore,
        Some("spool" | "journal" | "cache" | "state") => MachineDataKind::State,
        _ => MachineDataKind::Other,
    }
}

fn size(
    root: &Path,
    path: &Path,
    stop: &tokio_util::sync::CancellationToken,
) -> Result<i64, MachineDataProblem> {
    let mut next = Some(path.to_owned());
    let mut total = 0_i64;
    let mut directories: Vec<(std::path::PathBuf, std::fs::ReadDir)> = Vec::new();
    loop {
        if stop.is_cancelled() {
            return Err(MachineDataProblem::Unreadable);
        }
        let path = if let Some(path) = next.take() {
            path
        } else {
            let Some((_, entries)) = directories.last_mut() else {
                return Ok(total);
            };
            match entries.next() {
                Some(entry) => entry.map_err(|_| MachineDataProblem::Unreadable)?.path(),
                None => {
                    directories.pop();
                    continue;
                }
            }
        };
        let metadata =
            std::fs::symlink_metadata(&path).map_err(|_| MachineDataProblem::Unreadable)?;
        if metadata.file_type().is_symlink() {
            return Err(MachineDataProblem::Link);
        }
        let canonical = std::fs::canonicalize(&path).map_err(|_| MachineDataProblem::Unreadable)?;
        if !canonical.starts_with(root) {
            return Err(MachineDataProblem::Link);
        }
        if metadata.is_dir() {
            if directories
                .iter()
                .any(|(ancestor, _)| *ancestor == canonical)
            {
                return Err(MachineDataProblem::Link);
            }
            let entries = std::fs::read_dir(&path).map_err(|_| MachineDataProblem::Unreadable)?;
            directories.push((canonical, entries));
        } else {
            let bytes =
                i64::try_from(metadata.len()).map_err(|_| MachineDataProblem::SizeOverflow)?;
            total = total
                .checked_add(bytes)
                .ok_or(MachineDataProblem::SizeOverflow)?;
        }
    }
}

pub(crate) fn measure_with_progress(
    directory: &Path,
    at: Time,
    stop: &tokio_util::sync::CancellationToken,
    mut progress: impl FnMut(&MachineDataInventory),
) -> MachineDataInventory {
    let mut report = MachineDataInventory {
        observed_at: at,
        complete: false,
        parts: Vec::new(),
        unlisted_parts: 0,
        problem: None,
    };
    let Ok(root) = std::fs::canonicalize(directory) else {
        report.problem = Some(MachineDataProblem::Unreadable);
        return report;
    };
    let Ok(entries) = std::fs::read_dir(&root) else {
        report.problem = Some(MachineDataProblem::Unreadable);
        return report;
    };
    // Keep only the entries that fit the existing facts frame. Descendant traversal likewise keeps
    // directory iterators for the active ancestry, not a queue of every sibling or visited directory.
    let mut names = std::collections::BTreeSet::new();
    let mut found = 0_u64;
    for entry in entries {
        if stop.is_cancelled() {
            report.problem = Some(MachineDataProblem::Unreadable);
            return report;
        }
        match entry {
            Ok(entry) => {
                let name = entry.file_name();
                found = found.saturating_add(1);
                names.insert((kind(&name), name));
                if names.len() > PARTS_MAX {
                    names.pop_last();
                }
            }
            Err(_) => report.problem = Some(MachineDataProblem::Unreadable),
        }
    }
    report.unlisted_parts = found.saturating_sub(u64::try_from(names.len()).unwrap_or(u64::MAX));
    let names: Vec<_> = names.into_iter().collect();
    report.parts = names
        .iter()
        .map(|(kind, name)| MachineDataPart {
            id: name
                .as_encoded_bytes()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
            name: name.to_string_lossy().into_owned(),
            kind: *kind,
            bytes: None,
            problem: None,
        })
        .collect();
    progress(&report);
    for (index, (_, name)) in names.iter().enumerate() {
        let measured = size(&root, &root.join(name), stop);
        let (bytes, problem) = match measured {
            Ok(bytes) => (Some(bytes), None),
            Err(problem) => (None, Some(problem)),
        };
        report.parts[index].bytes = bytes;
        report.parts[index].problem = problem;
        progress(&report);
    }
    report.complete = report.problem.is_none()
        && report.unlisted_parts == 0
        && report.parts.iter().all(|part| part.problem.is_none());
    report
}

#[cfg(test)]
fn measure(
    directory: &Path,
    at: Time,
    stop: &tokio_util::sync::CancellationToken,
) -> MachineDataInventory {
    measure_with_progress(directory, at, stop, |_| {})
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test assertions")]
mod tests {
    use super::*;

    #[test]
    fn progress_preserves_the_scan_time_and_does_not_claim_unmeasured_parts_are_empty() {
        let tmp = tempfile::tempdir().unwrap();
        for name in ["logs", "mcp"] {
            std::fs::create_dir(tmp.path().join(name)).unwrap();
            std::fs::write(tmp.path().join(name).join("entry"), b"data").unwrap();
        }
        let at = "2026-11-08T06:00:00Z".parse().unwrap();
        let mut snapshots = Vec::new();
        let report = measure_with_progress(
            tmp.path(),
            at,
            &tokio_util::sync::CancellationToken::new(),
            |partial| snapshots.push(partial.clone()),
        );
        assert_eq!(snapshots.len(), 3);
        assert!(snapshots.iter().all(|snapshot| snapshot.observed_at == at));
        assert!(snapshots.iter().all(|snapshot| !snapshot.complete));
        assert!(snapshots[0].parts.iter().all(|part| part.bytes.is_none()));
        assert_eq!(snapshots[1].parts[0].bytes, Some(4));
        assert_eq!(snapshots[1].parts[1].bytes, None);
        assert!(report.complete);
        assert!(report.parts.iter().all(|part| part.bytes == Some(4)));
    }

    #[test]
    fn the_machine_put_aside_for_an_earlier_unit_is_counted_with_the_sealed_store() {
        let tmp = tempfile::tempdir().unwrap();
        let aside = tmp.path().join("previous").join("unit-a").join("identity");
        std::fs::create_dir_all(&aside).unwrap();
        std::fs::write(aside.join("key"), b"private-key-value").unwrap();
        let at = "2026-11-08T06:00:00Z".parse().unwrap();
        let report = measure(tmp.path(), at, &tokio_util::sync::CancellationToken::new());
        let part = report
            .parts
            .iter()
            .find(|part| part.name == "previous")
            .unwrap();
        assert_eq!(part.kind, MachineDataKind::SealedStore);
        assert_eq!(part.bytes, Some(17));
        assert!(
            !serde_json::to_string(&report)
                .unwrap()
                .contains("private-key-value")
        );
    }

    #[test]
    fn a_regular_observation_lists_real_parts_and_never_reads_or_reports_secret_contents_or_names()
    {
        let tmp = tempfile::tempdir().unwrap();
        for name in [
            "secrets",
            "identity",
            "harness/provider",
            "environment-data/env",
            "mcp",
            "storage-bindings",
            "logs",
            "unrecorded",
        ] {
            std::fs::create_dir_all(tmp.path().join(name)).unwrap();
        }
        for (path, content) in [
            (
                "secrets/private-credential-name",
                "private-credential-value",
            ),
            ("identity/private-key-name", "private-key-value"),
            ("harness/provider/history", "history"),
            ("environment-data/env/file", "workspace"),
            ("mcp/server.json", "private-copy-value"),
            ("storage-bindings/binding.json", "binding"),
            ("logs/connector.log", "log"),
            ("unrecorded/unknown", "unknown-value"),
        ] {
            std::fs::write(tmp.path().join(path), content.as_bytes()).unwrap();
        }
        let at = "2026-11-08T06:00:00Z".parse().unwrap();
        let report = measure(tmp.path(), at, &tokio_util::sync::CancellationToken::new());
        assert!(report.complete);
        assert_eq!(
            report
                .parts
                .iter()
                .find(|part| part.name == "secrets")
                .unwrap()
                .bytes,
            Some(24)
        );
        assert_eq!(
            report
                .parts
                .iter()
                .find(|part| part.name == "unrecorded")
                .unwrap()
                .kind,
            MachineDataKind::Other
        );
        let wire = serde_json::to_string(&report).unwrap();
        for private in [
            "private-credential-name",
            "private-credential-value",
            "private-key-name",
            "private-key-value",
            "private-copy-value",
            "unknown-value",
        ] {
            assert!(!wire.contains(private));
        }
        assert!(!wire.contains(tmp.path().to_str().unwrap()));
    }
    #[cfg(unix)]
    #[test]
    fn a_link_outside_the_directory_has_no_claimed_size_or_target_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("never-listed"), b"never-read").unwrap();
        std::os::unix::fs::symlink(outside.path(), tmp.path().join("foreign")).unwrap();
        let report = measure(
            tmp.path(),
            "2026-11-08T06:00:00Z".parse().unwrap(),
            &tokio_util::sync::CancellationToken::new(),
        );
        assert!(!report.complete);
        assert_eq!(report.parts[0].bytes, None);
        assert_eq!(report.parts[0].problem, Some(MachineDataProblem::Link));
        let wire = serde_json::to_string(&report).unwrap();
        assert!(!wire.contains("never-listed"));
        assert!(!wire.contains(outside.path().to_str().unwrap()));
    }
    #[test]
    fn bounded_reports_keep_the_sealed_store_visible_and_report_unlisted_entries() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("secrets")).unwrap();
        for index in 0..PARTS_MAX + 3 {
            std::fs::write(tmp.path().join(format!("entry-{index}")), b"x").unwrap();
        }
        let report = measure(
            tmp.path(),
            "2026-11-08T06:00:00Z".parse().unwrap(),
            &tokio_util::sync::CancellationToken::new(),
        );
        assert!(!report.complete);
        assert_eq!(report.parts.len(), PARTS_MAX);
        assert_eq!(report.unlisted_parts, 4);
        assert!(report.parts.iter().any(|part| part.name == "secrets"));
    }
}
