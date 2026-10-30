//! Bounded selection measurements before a snapshot is made, without reading file content or collecting whole directories.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use orch_link_contract::commands::{
    DirectoryMeasure, FsMeasureResult, SNAPSHOT_MAX_ENTRIES, SNAPSHOT_SCAN_SECONDS, Selection,
    SelectionRefusal,
};

use crate::error::{HostError, Result};
use crate::files::Files;
use crate::ignore::Scope;
use crate::manifest::{self, Resolved};
use crate::paths::{Locate, RelPath, is_internal_entry};
use crate::platform::{Dir, Meta};
use crate::walk::WALK_DEPTH_MAX;

/// The ten largest directories fit on a preflight screen without returning a tree-sized answer.
const LARGEST_DIRECTORIES: usize = 10;

/// An empty answer, used when a live tree requires explicit paths.
pub fn refused(reason: SelectionRefusal) -> FsMeasureResult {
    FsMeasureResult {
        entry_count: 0,
        file_count: 0,
        directory_count: 0,
        total_bytes: 0,
        largest_directories: Vec::new(),
        complete: false,
        refusal: Some(reason),
    }
}

struct Scan {
    reporting: Option<std::sync::Arc<dyn crate::snapshot::Progress>>,
    last_report: Instant,
    selected: Resolved,
    until: Instant,
    max_entries: u64,
    result: FsMeasureResult,
    directories: BTreeMap<String, (i64, i64)>,
}

impl Scan {
    fn report(&mut self, force: bool) {
        if (force || self.last_report.elapsed() >= crate::snapshot::PROGRESS_INTERVAL)
            && let Some(progress) = &self.reporting
        {
            progress.counting(
                self.result.file_count,
                self.result.total_bytes,
                self.result.directory_count,
            );
            self.last_report = Instant::now();
        }
    }
    fn may_continue(&mut self) -> bool {
        if self.result.refusal.is_some() {
            return false;
        }
        if Instant::now() >= self.until {
            self.result.refusal = Some(SelectionRefusal::ScanTime);
            return false;
        }
        true
    }

    fn count(&mut self, path: &RelPath, meta: &Meta) -> bool {
        if !self.may_continue() {
            return false;
        }
        if !self.selected.included(path) {
            return true;
        }
        self.result.entry_count = self.result.entry_count.saturating_add(1);
        if meta.is_dir() {
            self.result.directory_count = self.result.directory_count.saturating_add(1);
            self.directories.entry(path.to_string()).or_default();
        } else if meta.is_file() {
            let bytes = i64::try_from(meta.size).unwrap_or(i64::MAX);
            self.result.file_count = self.result.file_count.saturating_add(1);
            self.result.total_bytes = self.result.total_bytes.saturating_add(bytes);
            let mut parent = path.parent();
            while let Some(directory) = parent {
                let totals = self.directories.entry(directory.to_string()).or_default();
                totals.0 = totals.0.saturating_add(1);
                totals.1 = totals.1.saturating_add(bytes);
                parent = directory.parent();
            }
        }
        self.report(false);
        if u64::try_from(self.result.entry_count).unwrap_or(u64::MAX) > self.max_entries {
            self.result.refusal = Some(SelectionRefusal::EntryLimit);
            return false;
        }
        true
    }

    fn directory(
        &mut self,
        dir: &Dir,
        path: &RelPath,
        scope: Option<&Scope>,
        depth: usize,
    ) -> Result<()> {
        if depth >= WALK_DEPTH_MAX {
            self.result.refusal = Some(SelectionRefusal::Unreadable);
            return Ok(());
        }
        let mut failure = None;
        dir.visit_names(&mut |name| {
            if let Err(error) = crate::error::check_cancelled() {
                failure = Some(error);
                return false;
            }
            // Check even ignored entries: a million ignored siblings must not evade the time bound.
            if !self.may_continue() {
                return false;
            }
            let Some(text) = name.to_str() else {
                self.result.refusal = Some(SelectionRefusal::Unreadable);
                return false;
            };
            if is_internal_entry(path, text) {
                return true;
            }
            let child = match path.join(text) {
                Ok(child) => child,
                Err(error) => {
                    failure = Some(error);
                    return false;
                }
            };
            if self.selected.excluded(&child) {
                return true;
            }
            let meta = match dir.lstat(name) {
                Ok(meta) => meta,
                Err(_) => {
                    self.result.refusal = Some(SelectionRefusal::Unreadable);
                    return false;
                }
            };
            if scope.is_some_and(|rules| rules.is_ignored(&child, meta.is_dir())) {
                return true;
            }
            if !self.count(&child, &meta) {
                return false;
            }
            if meta.is_dir() {
                let opened = match dir.open_child(name) {
                    Ok(dir) => dir,
                    Err(_) => {
                        self.result.refusal = Some(SelectionRefusal::Unreadable);
                        return false;
                    }
                };
                let child_scope = scope.map(|rules| rules.enter(&child, &opened));
                if let Err(error) = self.directory(&opened, &child, child_scope.as_ref(), depth + 1)
                {
                    failure = Some(error);
                    return false;
                }
            }
            self.result.refusal.is_none()
        })
        .map_err(|error| HostError::io(format!("measuring {path}"), error))?;
        match failure {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

/// Measures exactly the selection with snapshot ignore rules, stopping before hashes or uploads.
pub fn selection(files: &Files, selection: &Selection) -> Result<FsMeasureResult> {
    with_limits(
        files,
        selection,
        SNAPSHOT_MAX_ENTRIES,
        Duration::from_secs(SNAPSHOT_SCAN_SECONDS),
    )
}

fn with_limits(
    files: &Files,
    selection: &Selection,
    max_entries: u64,
    scan_time: Duration,
) -> Result<FsMeasureResult> {
    let until = Instant::now() + scan_time;
    let selected = manifest::resolve(files.root(), selection)?;
    let includes = selected.includes.clone();
    let mut scan = Scan {
        reporting: crate::snapshot::current_progress(),
        last_report: Instant::now(),
        selected,
        until,
        max_entries,
        result: FsMeasureResult {
            entry_count: 0,
            file_count: 0,
            directory_count: 0,
            total_bytes: 0,
            largest_directories: Vec::new(),
            complete: true,
            refusal: None,
        },
        directories: BTreeMap::new(),
    };
    for include in includes {
        if !scan.may_continue() {
            break;
        }
        if scan.selected.excluded(&include) {
            continue;
        }
        let located = files.root().locate(&include, Locate::ENTRY)?;
        let Some(meta) = located.meta else {
            continue;
        };
        if files
            .ignore()
            .is_some_and(|rules| rules.is_ignored(files.root(), &include, meta.is_dir()))
        {
            continue;
        }
        if !include.is_root() && !scan.count(&include, &meta) {
            break;
        }
        if meta.is_dir() {
            let dir = files.root().open_dir(&located)?;
            let scope = files
                .ignore()
                .and_then(|rules| rules.scope_at(files.root(), &include));
            scan.directory(&dir, &include, scope.as_ref(), 0)?;
        }
    }
    scan.may_continue();
    scan.result.complete = scan.result.refusal.is_none();
    scan.report(true);
    let mut directories: Vec<_> = scan
        .directories
        .into_iter()
        .filter(|(path, _)| !path.is_empty())
        .map(|(path, (file_count, total_bytes))| DirectoryMeasure {
            path,
            file_count,
            total_bytes,
        })
        .collect();
    directories.sort_by(|a, b| {
        b.total_bytes
            .cmp(&a.total_bytes)
            .then_with(|| a.path.cmp(&b.path))
    });
    directories.truncate(LARGEST_DIRECTORIES);
    scan.result.largest_directories = directories;
    Ok(scan.result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::environment::{GateState, Gates, Limits};
    use crate::ignore::IgnoreRules;
    use crate::index::HashCache;
    use crate::paths::{PathStyle, Workspace};
    use std::sync::Arc;

    #[test]
    fn generated_files_are_measured_before_copy_and_a_large_selection_is_refused_with_its_reason() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("chosen/small")).unwrap();
        std::fs::create_dir_all(root.path().join("chosen/big")).unwrap();
        for index in 0..4096 {
            std::fs::write(root.path().join(format!("chosen/big/{index}")), [0u8; 32]).unwrap();
        }
        std::fs::write(root.path().join("chosen/small/keep"), b"kept").unwrap();
        std::fs::write(root.path().join("chosen/secret.skip"), b"ignored").unwrap();
        let files = Files::workspace(
            Workspace::open(root.path(), &[], PathStyle::Posix, true).unwrap(),
            Arc::new(IgnoreRules::new(&["*.skip".into()], true)),
            Arc::new(HashCache::new()),
            Limits::default(),
            Arc::new(GateState::new(Gates::OPEN)),
        );
        let chosen = Selection {
            include: vec!["chosen".into()],
            exclude: Vec::new(),
        };
        let measured = selection(&files, &chosen).unwrap();
        assert!(measured.complete);
        assert_eq!(measured.file_count, 4097);
        assert_eq!(measured.total_bytes, 4096 * 32 + 4);
        assert_eq!(measured.largest_directories[0].path, "chosen");
        assert_eq!(measured.largest_directories[1].path, "chosen/big");
        let refused = with_limits(&files, &chosen, 100, Duration::from_secs(10)).unwrap();
        assert!(!refused.complete);
        assert_eq!(refused.refusal, Some(SelectionRefusal::EntryLimit));
        assert_eq!(refused.entry_count, 101);
        let narrower = selection(
            &files,
            &Selection {
                include: vec!["chosen/small".into()],
                exclude: Vec::new(),
            },
        )
        .unwrap();
        assert!(narrower.complete);
        assert_eq!(narrower.file_count, 1);
        assert_eq!(narrower.total_bytes, 4);
        assert_eq!(
            with_limits(&files, &chosen, 250_000, Duration::ZERO)
                .unwrap()
                .refusal,
            Some(SelectionRefusal::ScanTime)
        );
    }
}
