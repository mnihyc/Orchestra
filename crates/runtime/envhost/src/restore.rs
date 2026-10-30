//! Complete namespace restore plans and durable guarded file restoration.
//!
//! Plans read actual files only on an explicit restore request. Their fingerprint covers every
//! included name, type, content or raw link and executable bit, while a private version record
//! also catches an identical-looking path recreated after the plan. Project Git metadata and
//! ignored data stay outside coverage. Confirmation must recheck the same generation, directory,
//! namespace and ignores, then keep a complete undo state before the first workspace write.

use std::collections::BTreeMap;

use orch_api_contract::source::{ExpectedAbsent, RestoreAction, RestoreExpected, RestorePath};
use orch_link_contract::source::{FsRestorePlan, FsRestorePlanResult};
use orch_types::{Hash, Id, Time};
use serde::{Deserialize, Serialize};

use crate::error::{HostError, Result};
use crate::file_states::{FileStates, GitRules};
use crate::files::Files;
use crate::index::IndexEntry;
use crate::manifest::{EntryKind, ManifestEntry};
use crate::paths::{Locate, RelPath};
use crate::platform;
use crate::state_store::{EXECUTABLE_BITS, RecordPolicy, atomic_write, read_bounded};
use crate::walk::{Flow, walk};

/// Ordinary file creation bits, before the process's umask, matching workspace writes.
const NEW_FILE_BITS: u32 = 0o666;
/// Ordinary directory creation bits, before the process's umask, matching workspace mkdir.
const NEW_DIRECTORY_BITS: u32 = 0o777;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum JournalPhase {
    Prepared,
    Applying,
    RollingBack,
    Complete,
    RolledBack,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Step {
    path: String,
    before: Option<ManifestEntry>,
    after: Option<ManifestEntry>,
}

struct Temporary {
    directory: platform::Dir,
    name: std::ffi::OsString,
    owned: bool,
}

impl Drop for Temporary {
    fn drop(&mut self) {
        if self.owned {
            let _ = self.directory.unlink(&self.name);
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    job_id: Id,
    plan_blob: Hash,
    snapshot_id: Id,
    undo_snapshot_id: Id,
    generation: i64,
    directory: String,
    phase: JournalPhase,
    next: usize,
    rollback_next: usize,
    steps: Vec<Step>,
    result: Option<orch_link_contract::source::FsRestoreResult>,
}

fn journal_path(states: &FileStates, job: Id) -> std::path::PathBuf {
    states
        .store()
        .root()
        .join("journals")
        .join(format!("{job}.json"))
}

fn keep_journal(states: &FileStates, journal: &Journal) -> Result<()> {
    let bytes =
        serde_json::to_vec(journal).map_err(|error| HostError::Internal(error.to_string()))?;
    atomic_write(&journal_path(states, journal.job_id), &bytes)
}

/// A failed effect is still held until this journal has completed or returned every applied path.
pub fn recovery_required(states: &FileStates, job: Id) -> Result<bool> {
    let path = journal_path(states, job);
    match std::fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(HostError::io("reading restore recovery ownership", error)),
        Ok(_) => {
            let journal: Journal = serde_json::from_slice(&read_bounded(
                &path,
                crate::manifest::MANIFEST_MAX_EXPANDED_BYTES,
            )?)
            .map_err(|_| {
                HostError::Unsupported("the restore recovery journal is unreadable".to_owned())
            })?;
            Ok(matches!(
                journal.phase,
                JournalPhase::Applying | JournalPhase::RollingBack
            ))
        }
    }
}

fn steps(plan: &StoredPlan) -> Vec<Step> {
    let current: BTreeMap<&str, &ManifestEntry> = plan
        .current
        .iter()
        .map(|entry| (entry.p.as_str(), entry))
        .collect();
    let target: BTreeMap<&str, &ManifestEntry> = plan
        .target
        .iter()
        .map(|entry| (entry.p.as_str(), entry))
        .collect();
    let mut removals: Vec<Step> = current
        .iter()
        .filter(|(path, old)| target.get(**path).is_none_or(|new| old.t != new.t))
        .map(|(path, old)| Step {
            path: (*path).to_owned(),
            before: Some((*old).clone()),
            after: None,
        })
        .collect();
    removals.sort_by_key(|step| std::cmp::Reverse(step.path.split('/').count()));
    let mut puts: Vec<Step> = target
        .iter()
        .filter(|(path, new)| {
            current
                .get(**path)
                .is_none_or(|old| semantic(old) != semantic(new))
        })
        .map(|(path, new)| Step {
            path: (*path).to_owned(),
            before: current
                .get(*path)
                .filter(|old| old.t == new.t)
                .map(|old| (*old).clone()),
            after: Some((*new).clone()),
        })
        .collect();
    puts.sort_by_key(|step| step.path.split('/').count());
    removals.extend(puts);
    removals
}

fn at_path(files: &Files, directory: &RelPath, path: &str) -> Result<Option<ManifestEntry>> {
    let relative = files.parse(path)?;
    let full = crate::file_states::join(directory, &relative, files)?;
    let located = match files.root().locate(&full, Locate::ENTRY) {
        Ok(located) => located,
        Err(HostError::NotFound(_)) => return Ok(None),
        Err(error) => return Err(error),
    };
    let Some(meta) = located.meta else {
        return Ok(None);
    };
    files.check_visible(&full, &located, meta.is_dir())?;
    let (hash, link) = if meta.is_file() {
        let file = located
            .parent
            .open_read(located.name()?)
            .map_err(|error| HostError::io("checking a restore effect", error))?;
        (
            Some(
                crate::blobs::hash_file(&file)
                    .map_err(|error| HostError::io("hashing a restore effect", error))?,
            ),
            None,
        )
    } else if meta.is_symlink() {
        (
            None,
            Some(
                located
                    .parent
                    .read_link(located.name()?)
                    .map_err(|error| HostError::io("checking restored link text", error))?
                    .into_string()
                    .map_err(|_| {
                        HostError::Unsupported("restored link text is not UTF-8".to_owned())
                    })?,
            ),
        )
    } else {
        (None, None)
    };
    Ok(Some(ManifestEntry {
        p: path.to_owned(),
        t: meta.kind.into(),
        s: if meta.is_file() { meta.size as i64 } else { 0 },
        m: meta.mtime_ns,
        x: meta.mode,
        h: hash,
        l: link,
    }))
}

fn same(left: Option<&ManifestEntry>, right: Option<&ManifestEntry>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => semantic(left) == semantic(right),
        _ => false,
    }
}

fn apply_step(
    files: &Files,
    states: &FileStates,
    directory: &RelPath,
    step: &Step,
    root: &crate::state_store::StateRoot,
    job: Id,
    position: usize,
) -> Result<()> {
    let current = at_path(files, directory, &step.path)?;
    if same(current.as_ref(), step.after.as_ref()) {
        return Ok(());
    }
    if !same(current.as_ref(), step.before.as_ref()) {
        return Err(HostError::Changed(
            "a restore effect encountered a later writer; recovery holds the directory".to_owned(),
        ));
    }
    let full = crate::file_states::join(directory, &files.parse(&step.path)?, files)?;
    let located = files.root().locate(&full, Locate::ENTRY)?;
    let name = located.name()?;
    match &step.after {
        None => {
            if located.meta.is_some_and(|meta| meta.is_dir()) {
                located.parent.rmdir(name)
            } else {
                located.parent.unlink(name)
            }
            .map_err(|error| HostError::io("removing an included restore path", error))?;
        }
        Some(entry) if entry.t == EntryKind::Dir => {
            if located.meta.is_none() {
                located
                    .parent
                    .mkdir(name, NEW_DIRECTORY_BITS)
                    .map_err(|error| HostError::io("making a restored directory", error))?;
            }
            let existing = located
                .parent
                .lstat(name)
                .map_err(|error| HostError::io("checking restored directory permissions", error))?;
            located
                .parent
                .set_mode(
                    name,
                    (existing.mode & !EXECUTABLE_BITS) | (entry.x & EXECUTABLE_BITS),
                )
                .map_err(|error| HostError::io("restoring directory executable bits", error))?;
        }
        Some(entry) => {
            let temporary = std::ffi::OsString::from(format!(
                ".orch-tmp-restore-{job}-{position}-{}",
                orch_types::new_id()
            ));
            let mut cleanup = Temporary {
                directory: located.parent.try_clone().map_err(|error| {
                    HostError::io("holding the restore's temporary directory", error)
                })?,
                name: temporary.clone(),
                owned: false,
            };
            match entry.t {
                EntryKind::File => {
                    let verified = states.store().read_entry(root, entry)?;
                    let mut source = std::fs::File::open(verified.path())
                        .map_err(|error| HostError::io("opening verified restore bytes", error))?;
                    let mut target = located
                        .parent
                        .create_new(&temporary, NEW_FILE_BITS)
                        .map_err(|error| HostError::io("staging restored bytes", error))?;
                    cleanup.owned = true;
                    crate::snapshot::copy_counted(&mut source, &mut target)?;
                    let base = located
                        .meta
                        .map_or_else(
                            || platform::file_meta(&target).map(|meta| meta.mode),
                            |meta| Ok(meta.mode),
                        )
                        .map_err(|error| {
                            HostError::io("checking restored file permissions", error)
                        })?;
                    platform::set_file_mode(
                        &target,
                        (base & !EXECUTABLE_BITS) | (entry.x & EXECUTABLE_BITS),
                    )
                    .map_err(|error| HostError::io("restoring file executable bits", error))?;
                    target
                        .sync_all()
                        .map_err(|error| HostError::io("syncing restored bytes", error))?;
                }
                EntryKind::Link => {
                    // Verification precedes creating the raw link. No read follows its target.
                    states.store().read_entry(root, entry)?;
                    let target = entry.l.as_deref().ok_or_else(|| {
                        HostError::Unsupported("recorded link text is missing".to_owned())
                    })?;
                    located
                        .parent
                        .symlink(std::ffi::OsStr::new(target), &temporary)
                        .map_err(|error| HostError::io("staging restored link text", error))?;
                    cleanup.owned = true;
                }
                _ => {
                    return Err(HostError::Unsupported(
                        "unsupported restore entry".to_owned(),
                    ));
                }
            }
            located
                .parent
                .rename_to(&temporary, &located.parent, name, located.meta.is_some())
                .map_err(|error| HostError::io("publishing restored bytes", error))?;
            cleanup.owned = false;
        }
    }
    located
        .parent
        .sync()
        .map_err(|error| HostError::io("syncing a restore effect", error))
}

/// Executes or resumes one journaled restore; changed plans write no workspace file.
pub fn apply(
    files: &Files,
    states: &FileStates,
    request: &orch_link_contract::source::FsRestore,
    generation: i64,
    now: Time,
) -> Result<orch_link_contract::source::FsRestoreResult> {
    let _change = states.change_gate();
    apply_with_checkpoints(files, states, request, generation, now, &mut |_, _| Ok(()))
}

fn apply_with_checkpoints(
    files: &Files,
    states: &FileStates,
    request: &orch_link_contract::source::FsRestore,
    generation: i64,
    now: Time,
    checkpoint: &mut dyn FnMut(&str, usize) -> Result<()>,
) -> Result<orch_link_contract::source::FsRestoreResult> {
    files.gates().require_remote_write()?;
    let plan = read_plan(states, request.plan_blob)?;
    if plan.snapshot_id != request.snapshot_id
        || generation != request.generation
        || generation != plan.generation
    {
        return Err(HostError::Changed(
            "the restore identity or generation no longer matches the plan".to_owned(),
        ));
    }
    let path = journal_path(states, request.job_id);
    let mut journal = if path.exists() {
        let bytes = read_bounded(&path, crate::manifest::MANIFEST_MAX_EXPANDED_BYTES)?;
        let journal: Journal = serde_json::from_slice(&bytes)
            .map_err(|_| HostError::Unsupported("the restore journal needs recovery".to_owned()))?;
        if journal.job_id != request.job_id
            || journal.plan_blob != request.plan_blob
            || journal.snapshot_id != request.snapshot_id
            || journal.undo_snapshot_id != request.undo_snapshot_id
        {
            return Err(HostError::Changed(
                "the restore job was reused with another plan".to_owned(),
            ));
        }
        journal
    } else {
        recheck(files, states, &plan, generation, now)?;
        let observed = scan(files, &files.parse(&plan.directory)?, plan.record_policy)?;
        let undo = states.capture_explicit(
            files,
            &observed.indexed,
            &orch_link_contract::source::FsRecord {
                environment_id: request.environment_id,
                snapshot_id: request.undo_snapshot_id,
                directory: plan.directory.clone(),
                kind: orch_link_contract::source::RecordKind::Restore,
                record_days: plan.record_policy.record_days,
                max_bytes: plan.record_policy.max_bytes,
            },
            generation,
            now,
        )?;
        let undo_entries = states.store().load(undo.snapshot_id, now)?.1.entries;
        if namespace_fingerprint(&undo_entries)? != plan.namespace_fingerprint {
            return Err(HostError::Changed(
                "the current files changed while complete undo was recorded".to_owned(),
            ));
        }
        recheck(files, states, &plan, generation, now)?;
        let journal = Journal {
            job_id: request.job_id,
            plan_blob: request.plan_blob,
            snapshot_id: request.snapshot_id,
            undo_snapshot_id: request.undo_snapshot_id,
            generation,
            directory: plan.directory.clone(),
            phase: JournalPhase::Prepared,
            next: 0,
            rollback_next: 0,
            steps: steps(&plan),
            result: None,
        };
        keep_journal(states, &journal)?;
        checkpoint("prepared", 0)?;
        journal
    };
    if let Some(result) = journal.result {
        return Ok(result);
    }
    let directory = files.parse(&journal.directory)?;
    let (target, target_entries) = states.store().load_for_recovery(journal.snapshot_id)?;
    let undo = states
        .store()
        .load_for_recovery(journal.undo_snapshot_id)?
        .0;
    if target.manifest_blob != plan.target_manifest || target_entries.entries != plan.target {
        return Err(HostError::Changed(
            "the held recorded target changed during recovery".to_owned(),
        ));
    }
    let until = Time::new(
        now.get()
            .checked_add_signed(chrono::Duration::seconds(i64::from(
                orch_link_contract::commands::CommandKind::FsRestore.deadline_secs(),
            )))
            .ok_or_else(|| {
                HostError::Unsupported("the restore operation deadline is invalid".to_owned())
            })?,
    );
    let _target_pin = states
        .store()
        .pin(target.snapshot_id, request.job_id, until)?;
    let _undo_pin = states
        .store()
        .pin(undo.snapshot_id, request.job_id, until)?;
    if journal.phase == JournalPhase::Prepared {
        // This journal already held both roots while they were unexpired. A restart may cross
        // their retention deadline, but must still finish or return this same operation safely.
        recheck_namespace(files, &plan, generation)?;
        // Verify all target and undo bytes before the first workspace effect, not halfway
        // through applying a state whose objects became unreadable since preflight.
        for entry in plan
            .target
            .iter()
            .filter(|entry| matches!(entry.t, EntryKind::File | EntryKind::Link))
        {
            states.store().read_entry(&target, entry)?;
        }
        let undo_entries = states.store().load_for_recovery(undo.snapshot_id)?.1;
        for entry in undo_entries
            .entries
            .iter()
            .filter(|entry| matches!(entry.t, EntryKind::File | EntryKind::Link))
        {
            states.store().read_entry(&undo, entry)?;
        }
        journal.phase = JournalPhase::Applying;
        keep_journal(states, &journal)?;
        checkpoint("applying", 0)?;
    }
    let reporting = crate::snapshot::current_progress();
    let mut last_report = std::time::Instant::now();
    let mut files_done = journal
        .steps
        .iter()
        .take(journal.next)
        .filter(|step| {
            step.after
                .as_ref()
                .is_some_and(|entry| entry.t == EntryKind::File)
        })
        .count() as i64;
    let files_total = journal
        .steps
        .iter()
        .filter(|step| {
            step.after
                .as_ref()
                .is_some_and(|entry| entry.t == EntryKind::File)
        })
        .count() as i64;
    let bytes_total = journal
        .steps
        .iter()
        .filter_map(|step| step.after.as_ref())
        .filter(|entry| entry.t == EntryKind::File)
        .fold(0i64, |bytes, entry| bytes.saturating_add(entry.s));
    let phase = request
        .work
        .as_ref()
        .map_or("apply", |work| work.phase.as_str());
    let mut report = |done: i64, force: bool| {
        if (force || last_report.elapsed() >= crate::snapshot::PROGRESS_INTERVAL)
            && let Some(progress) = &reporting
        {
            progress.report(&orch_api_contract::workspace::JobProgress {
                phase: phase.to_owned(),
                phases: vec![orch_api_contract::workspace::JobPhase {
                    name: phase.to_owned(),
                    weight: 100,
                    state: orch_api_contract::workspace::PhaseState::Running,
                }],
                percent: if files_total > 0 {
                    ((done as i128 * 100 / files_total as i128).clamp(0, 100)) as u32
                } else {
                    0
                },
                files_done: done,
                files_total: Some(files_total),
                bytes_done: 0,
                bytes_total: Some(bytes_total),
                rate_bps: None,
                eta_seconds: None,
                message: None,
                activity: None,
            });
            last_report = std::time::Instant::now();
        }
    };
    report(files_done, true);
    if journal.phase == JournalPhase::Applying {
        while journal.next < journal.steps.len() {
            let step = &journal.steps[journal.next];
            checkpoint("before_effect", journal.next)?;
            if let Err(error) = apply_step(
                files,
                states,
                &directory,
                step,
                &target,
                request.job_id,
                journal.next,
            ) {
                journal.phase = JournalPhase::RollingBack;
                journal.rollback_next = (journal.next + 1).min(journal.steps.len());
                keep_journal(states, &journal)?;
                if matches!(error, HostError::Changed(_)) {
                    return Err(error);
                }
                break;
            }
            if step
                .after
                .as_ref()
                .is_some_and(|entry| entry.t == EntryKind::File)
            {
                files_done = files_done.saturating_add(1);
            }
            report(files_done, false);
            checkpoint("after_effect", journal.next)?;
            journal.next += 1;
            keep_journal(states, &journal)?;
            checkpoint("after_advance", journal.next)?;
        }
    }
    report(files_done, true);
    if journal.phase == JournalPhase::RollingBack {
        while journal.rollback_next > 0 {
            let position = journal.rollback_next - 1;
            let original = &journal.steps[position];
            let inverse = Step {
                path: original.path.clone(),
                before: original.after.clone(),
                after: original.before.clone(),
            };
            apply_step(
                files,
                states,
                &directory,
                &inverse,
                &undo,
                request.job_id,
                position,
            )?;
            checkpoint("after_rollback_effect", position)?;
            journal.rollback_next = position;
            keep_journal(states, &journal)?;
        }
        journal.phase = JournalPhase::RolledBack;
    } else {
        let target_bytes = plan
            .target
            .iter()
            .map(|entry| entry.s + entry.l.as_ref().map_or(0, |link| link.len() as i64))
            .sum::<i64>();
        let verify = RecordPolicy {
            max_bytes: plan.record_policy.max_bytes.max(target_bytes),
            ..plan.record_policy
        };
        let verified = scan(files, &directory, verify).and_then(|applied| {
            if namespace_fingerprint(&plan.target)? == applied.fingerprint {
                Ok(())
            } else {
                Err(HostError::Changed(
                    "the restored namespace changed before verification".to_owned(),
                ))
            }
        });
        if verified.is_err() {
            journal.phase = JournalPhase::RollingBack;
            journal.rollback_next = journal.next;
            keep_journal(states, &journal)?;
            return Err(HostError::Changed(
                "the restored namespace changed before verification; the same job must recover"
                    .to_owned(),
            ));
        }
        journal.phase = JournalPhase::Complete;
    }
    let files_written = journal
        .steps
        .iter()
        .filter(|step| {
            step.after
                .as_ref()
                .is_some_and(|entry| entry.t == EntryKind::File)
        })
        .count() as i64;
    let result = orch_link_contract::source::FsRestoreResult {
        files: files_written,
        undo_snapshot_id: journal.undo_snapshot_id,
        undone: journal.phase == JournalPhase::RolledBack,
    };
    journal.result = Some(result);
    keep_journal(states, &journal)?;
    checkpoint("complete", journal.next)?;
    Ok(result)
}

/// A private identity check alongside the public content namespace fingerprint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PathVersion {
    /// The device on which the path was observed.
    pub device: u64,
    /// Its filesystem identity, where the platform has one.
    pub inode: u64,
    /// The last metadata/content change time, where the platform has one.
    pub changed_at: i64,
}

impl PathVersion {
    fn of(meta: &platform::Meta) -> Self {
        Self {
            device: meta.dev,
            inode: meta.ino,
            changed_at: meta.ctime_ns,
        }
    }
}

/// The full private plan uploaded under one content hash and frozen by the control plane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredPlan {
    /// The recorded target, never a selection of newer live files.
    pub snapshot_id: Id,
    /// Its original immutable manifest commitment.
    pub target_manifest: Hash,
    /// The same workspace-relative directory.
    pub directory: String,
    /// The runtime generation confirmed by this plan.
    pub generation: i64,
    /// Every included current name, type, executable bit, content and link.
    pub namespace_fingerprint: Hash,
    /// The ignore coverage observed while planning.
    pub ignore_fingerprint: Hash,
    /// The current directory's identity, preventing an alias or recreated root.
    pub directory_version: PathVersion,
    /// Actual identities distinguish recreation even when bytes are identical.
    pub versions: BTreeMap<String, PathVersion>,
    /// All current entries in the common snapshot encoding.
    pub current: Vec<ManifestEntry>,
    /// All target entries in the common snapshot encoding.
    pub target: Vec<ManifestEntry>,
    /// The exact path effects presented to the person.
    pub paths: Vec<RestorePath>,
    /// Recording settings frozen by the trusted preparation command.
    pub record_policy: RecordPolicy,
}

struct Observed {
    entries: Vec<ManifestEntry>,
    indexed: Vec<(RelPath, IndexEntry)>,
    versions: BTreeMap<String, PathVersion>,
    directory_version: PathVersion,
    fingerprint: Hash,
    ignore_fingerprint: Hash,
}

fn semantic(entry: &ManifestEntry) -> serde_json::Value {
    serde_json::json!({ "p": entry.p, "t": entry.t, "s": entry.s, "x": entry.x & EXECUTABLE_BITS, "h": entry.h, "l": entry.l })
}

/// Canonical complete namespace digest, independent of index stat-cache refreshes and scan time.
pub fn namespace_fingerprint(entries: &[ManifestEntry]) -> Result<Hash> {
    let mut entries: Vec<serde_json::Value> = entries.iter().map(semantic).collect();
    entries.sort_by(|left, right| left["p"].as_str().cmp(&right["p"].as_str()));
    let bytes =
        serde_json::to_vec(&entries).map_err(|error| HostError::Internal(error.to_string()))?;
    Ok(Hash::of(&bytes))
}

fn scan(files: &Files, directory: &RelPath, policy: RecordPolicy) -> Result<Observed> {
    let located = files.root().locate(directory, Locate::ENTRY)?;
    let meta = located
        .meta
        .ok_or_else(|| HostError::NotFound("the recorded directory is absent".to_owned()))?;
    if !meta.is_dir() || located.real != *directory {
        return Err(HostError::Changed(
            "the recorded directory became another place".to_owned(),
        ));
    }
    files.check_visible(directory, &located, true)?;
    let mut indexed = Vec::new();
    let outcome = walk(files.root(), files.ignore(), directory, &mut |entry| {
        if entry.path.has_component(".git") {
            return Flow::SkipDir;
        }
        indexed.push((
            entry.path.clone(),
            IndexEntry {
                kind: entry.meta.kind,
                size: if entry.meta.is_file() {
                    entry.meta.size
                } else {
                    0
                },
                mtime_ns: entry.meta.mtime_ns,
                mode: entry.meta.mode,
                link_target: entry.link_target.clone(),
            },
        ));
        Flow::Continue
    })?;
    if !outcome.unreadable.is_empty() || outcome.stopped {
        return Err(HostError::Unsupported(
            "the current directory cannot supply complete undo coverage".to_owned(),
        ));
    }
    let mut ancestor = Some(directory.clone());
    while let Some(path) = ancestor {
        indexed.push((
            path.clone(),
            IndexEntry {
                kind: platform::Kind::Dir,
                size: 0,
                mtime_ns: 0,
                mode: 0,
                link_target: None,
            },
        ));
        ancestor = path.parent();
    }
    let rules = GitRules::read(files, &indexed, directory)?;
    let mut entries = Vec::new();
    let mut versions = BTreeMap::new();
    let mut bytes = 0i64;
    for (path, index) in &indexed {
        let Some(relative) = path.strip_prefix(directory).filter(|path| !path.is_root()) else {
            continue;
        };
        if rules.ignored(path, index.kind == platform::Kind::Dir) {
            continue;
        }
        let located = files.root().locate(path, Locate::ENTRY)?;
        let meta = located.meta.ok_or_else(|| {
            HostError::Changed("an included restore path disappeared while planning".to_owned())
        })?;
        if meta.kind != index.kind {
            return Err(HostError::Changed(
                "an included restore path changed type while planning".to_owned(),
            ));
        }
        let (hash, link, size) = match meta.kind {
            platform::Kind::File => {
                let size = i64::try_from(meta.size)
                    .map_err(|_| HostError::Unsupported("complete undo is too large".to_owned()))?;
                bytes = bytes.checked_add(size).ok_or_else(|| {
                    HostError::Unsupported("undo byte count overflowed".to_owned())
                })?;
                if bytes > policy.max_bytes {
                    return Err(HostError::TooLarge {
                        what: "complete undo".to_owned(),
                        size: bytes,
                        limit: policy.max_bytes,
                    });
                }
                let file = located
                    .parent
                    .open_read(located.name()?)
                    .map_err(|error| HostError::io("opening current restore bytes", error))?;
                let before = platform::file_meta(&file)
                    .map_err(|error| HostError::io("checking current restore bytes", error))?;
                let hash = crate::blobs::hash_file(&file)
                    .map_err(|error| HostError::io("hashing current restore bytes", error))?;
                let after = platform::file_meta(&file)
                    .map_err(|error| HostError::io("rechecking current restore bytes", error))?;
                if before.identity() != after.identity()
                    || before.mode != after.mode
                    || before.identity() != meta.identity()
                {
                    return Err(HostError::Changed(
                        "current restore bytes changed during the scan".to_owned(),
                    ));
                }
                (Some(hash), None, size)
            }
            platform::Kind::Symlink => {
                let link = located
                    .parent
                    .read_link(located.name()?)
                    .map_err(|error| HostError::io("reading current restore link text", error))?
                    .into_string()
                    .map_err(|_| {
                        HostError::Unsupported("current link text is not UTF-8".to_owned())
                    })?;
                bytes = bytes.checked_add(link.len() as i64).ok_or_else(|| {
                    HostError::Unsupported("undo byte count overflowed".to_owned())
                })?;
                if bytes > policy.max_bytes {
                    return Err(HostError::TooLarge {
                        what: "complete undo".to_owned(),
                        size: bytes,
                        limit: policy.max_bytes,
                    });
                }
                (None, Some(link), 0)
            }
            platform::Kind::Dir => (None, None, 0),
            platform::Kind::Other => {
                return Err(HostError::Unsupported(
                    "a special file prevents complete undo".to_owned(),
                ));
            }
        };
        versions.insert(relative.to_string(), PathVersion::of(&meta));
        entries.push(ManifestEntry {
            p: relative.to_string(),
            t: meta.kind.into(),
            s: size,
            m: meta.mtime_ns,
            x: meta.mode,
            h: hash,
            l: link,
        });
    }
    entries.sort_by(|left, right| left.p.cmp(&right.p));
    entries.dedup_by(|left, right| left.p == right.p);
    Ok(Observed {
        fingerprint: namespace_fingerprint(&entries)?,
        entries,
        indexed,
        versions,
        directory_version: PathVersion::of(&meta),
        ignore_fingerprint: rules.fingerprint,
    })
}

fn effects(current: &[ManifestEntry], target: &[ManifestEntry]) -> Result<Vec<RestorePath>> {
    let current: BTreeMap<&str, &ManifestEntry> = current
        .iter()
        .map(|entry| (entry.p.as_str(), entry))
        .collect();
    let target: BTreeMap<&str, &ManifestEntry> = target
        .iter()
        .map(|entry| (entry.p.as_str(), entry))
        .collect();
    let paths: std::collections::BTreeSet<&str> =
        current.keys().chain(target.keys()).copied().collect();
    let mut changes = Vec::new();
    for path in paths {
        let action = match (current.get(path), target.get(path)) {
            (None, Some(_)) => RestoreAction::Add,
            (Some(_), None) => RestoreAction::Remove,
            (Some(left), Some(right)) if semantic(left) != semantic(right) => RestoreAction::Change,
            _ => continue,
        };
        let expected = match current.get(path) {
            None => RestoreExpected::Absent(ExpectedAbsent::None),
            Some(entry) if entry.t == EntryKind::File => {
                RestoreExpected::Hash(entry.h.ok_or_else(|| {
                    HostError::Unsupported("current file hash is missing".to_owned())
                })?)
            }
            Some(entry) => RestoreExpected::Hash(Hash::of(
                &serde_json::to_vec(&semantic(entry))
                    .map_err(|error| HostError::Internal(error.to_string()))?,
            )),
        };
        changes.push(RestorePath {
            path: path.to_owned(),
            action,
            expected,
            later_change: None,
            later_writers: Vec::new(),
        });
    }
    Ok(changes)
}

/// Makes the complete plan without altering the workspace or any project's Git state.
pub fn plan(
    files: &Files,
    states: &FileStates,
    request: &FsRestorePlan,
    generation: i64,
    now: Time,
) -> Result<(StoredPlan, FsRestorePlanResult)> {
    files.gates().require_remote_write()?;
    let directory = files.parse(&request.directory)?;
    let policy = match (request.record_days, request.max_bytes) {
        (Some(record_days), Some(max_bytes)) => Some(RecordPolicy {
            record_days,
            max_bytes,
        }),
        (None, None) => states.store().policy(&directory)?,
        _ => {
            return Err(HostError::Unsupported(
                "a restore needs both frozen recording limits".to_owned(),
            ));
        }
    }
    .filter(|policy| policy.record_days > 0 && policy.max_bytes > 0)
    .ok_or_else(|| {
        HostError::Unsupported("complete undo recording is disabled or not configured".to_owned())
    })?;
    let read = states.read_recorded(
        files,
        &crate::index::Index::new(orch_link_contract::commands::IndexMode::Live, 0),
        &orch_link_contract::source::FsRecordRead {
            environment_id: request.environment_id,
            snapshot_id: request.snapshot_id,
            path: None,
            export: false,
        },
        orch_types::new_id(),
        now,
    )?;
    if read.root.directory != request.directory {
        return Err(HostError::Changed(
            "a recorded state is restored only into its own directory".to_owned(),
        ));
    }
    let current = scan(files, &directory, policy)?;
    // Validate every target object before presenting a whole-state plan.
    for entry in read
        .entries
        .iter()
        .filter(|entry| matches!(entry.t, EntryKind::File | EntryKind::Link))
    {
        states.store().read_entry(&read.root, entry)?;
    }
    let paths = effects(&current.entries, &read.entries)?;
    let plan = StoredPlan {
        snapshot_id: request.snapshot_id,
        target_manifest: read.root.manifest_blob,
        directory: request.directory.clone(),
        generation,
        namespace_fingerprint: current.fingerprint,
        ignore_fingerprint: current.ignore_fingerprint,
        directory_version: current.directory_version,
        versions: current.versions,
        current: current.entries,
        target: read.entries,
        paths: paths.clone(),
        record_policy: policy,
    };
    let bytes =
        serde_json::to_vec(&plan).map_err(|error| HostError::Internal(error.to_string()))?;
    let hash = Hash::of(&bytes);
    atomic_write(
        &states.store().root().join("plans").join(hash.to_string()),
        &bytes,
    )?;
    Ok((
        plan,
        FsRestorePlanResult {
            plan_blob: hash,
            paths,
            writers_active: false,
            undo_available: true,
        },
    ))
}

/// Reads the exact content-addressed plan; a changed blob never becomes a different restore.
pub fn read_plan(states: &FileStates, hash: Hash) -> Result<StoredPlan> {
    let bytes = read_bounded(
        &states.store().root().join("plans").join(hash.to_string()),
        crate::manifest::MANIFEST_MAX_EXPANDED_BYTES,
    )?;
    if Hash::of(&bytes) != hash {
        return Err(HostError::Changed(
            "the stored restore plan changed".to_owned(),
        ));
    }
    let plan: StoredPlan = serde_json::from_slice(&bytes)
        .map_err(|_| HostError::Unsupported("the stored restore plan is invalid".to_owned()))?;
    if namespace_fingerprint(&plan.current)? != plan.namespace_fingerprint
        || effects(&plan.current, &plan.target)? != plan.paths
    {
        return Err(HostError::Changed(
            "the stored restore plan's namespace or effects do not match".to_owned(),
        ));
    }
    Ok(plan)
}

/// Stores the exact trusted control-plane plan after the download hash has been checked.
pub fn keep_plan(states: &FileStates, hash: Hash, bytes: &[u8]) -> Result<()> {
    if Hash::of(bytes) != hash {
        return Err(HostError::Changed(
            "the downloaded restore plan changed".to_owned(),
        ));
    }
    let _: StoredPlan = serde_json::from_slice(bytes)
        .map_err(|_| HostError::Unsupported("the downloaded restore plan is invalid".to_owned()))?;
    atomic_write(
        &states.store().root().join("plans").join(hash.to_string()),
        bytes,
    )?;
    read_plan(states, hash).map(|_| ())
}

/// Refuses all workspace effects if any expected name, version, generation or ignore rule changed.
pub fn recheck(
    files: &Files,
    states: &FileStates,
    plan: &StoredPlan,
    generation: i64,
    now: Time,
) -> Result<()> {
    files.gates().require_remote_write()?;
    let target = states.store().load(plan.snapshot_id, now)?;
    if target.0.manifest_blob != plan.target_manifest || target.1.entries != plan.target {
        return Err(HostError::Changed(
            "the recorded target changed after this plan".to_owned(),
        ));
    }
    recheck_namespace(files, plan, generation)
}

fn recheck_namespace(files: &Files, plan: &StoredPlan, generation: i64) -> Result<()> {
    files.gates().require_remote_write()?;
    if generation != plan.generation {
        return Err(HostError::Changed(
            "the environment generation changed after this plan".to_owned(),
        ));
    }
    let observed = scan(files, &files.parse(&plan.directory)?, plan.record_policy)?;
    if observed.fingerprint != plan.namespace_fingerprint
        || observed.ignore_fingerprint != plan.ignore_fingerprint
        || observed.directory_version != plan.directory_version
        || observed.versions != plan.versions
    {
        return Err(HostError::Changed(
            "the complete included namespace changed; make a new restore plan".to_owned(),
        ));
    }
    Ok(())
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;
    use crate::environment::{GateState, Gates, Limits};
    use crate::ignore::IgnoreRules;
    use crate::index::{HashCache, Index};
    use crate::paths::{PathStyle, Workspace};
    use crate::state_store::ObjectStore;
    use orch_link_contract::commands::IndexMode;
    use orch_link_contract::source::{FsRecord, RecordKind};
    use orch_types::new_id;
    use std::sync::Arc;

    fn at() -> Time {
        "2026-10-14T13:00:00.000Z".parse().unwrap()
    }

    fn fixture() -> (tempfile::TempDir, Files, FileStates, FsRestorePlan) {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        std::fs::write(workspace.join("code"), "before").unwrap();
        let rules = Arc::new(IgnoreRules::new(&[], true));
        let files = Files::workspace(
            Workspace::open(&workspace, &[], PathStyle::Posix, true).unwrap(),
            rules,
            Arc::new(HashCache::new()),
            Limits::default(),
            Arc::new(GateState::new(Gates::OPEN)),
        );
        let states =
            FileStates::new(ObjectStore::open(&dir.path().join("kept"), &workspace).unwrap());
        let index = Index::new(IndexMode::Full, Limits::default().index_max_entries);
        index.scan(files.root(), files.ignore().unwrap()).unwrap();
        states.refresh(&files, &index, None, true).unwrap();
        let environment = new_id();
        let snapshot = new_id();
        states
            .record(
                &files,
                &FsRecord {
                    environment_id: environment,
                    snapshot_id: snapshot,
                    directory: String::new(),
                    kind: RecordKind::Turn,
                    record_days: crate::file_states::DEFAULT_RECORD_DAYS,
                    max_bytes: crate::file_states::DEFAULT_RECORD_MAX_BYTES,
                },
                1,
                at(),
            )
            .unwrap();
        (
            dir,
            files,
            states,
            FsRestorePlan {
                environment_id: environment,
                snapshot_id: snapshot,
                directory: String::new(),
                turn_id: None,
                record_days: None,
                max_bytes: None,
            },
        )
    }

    #[test]
    fn the_plan_names_replacements_and_later_additions_and_any_new_path_refuses_all_effects() {
        let (_dir, files, states, request) = fixture();
        std::fs::write(files.root().root_path().join("code"), "current").unwrap();
        std::fs::write(files.root().root_path().join("added"), "made later").unwrap();
        let (plan, result) = super::plan(&files, &states, &request, 1, at()).unwrap();
        assert!(result.undo_available);
        assert!(
            result
                .paths
                .iter()
                .any(|path| path.path == "code" && path.action == RestoreAction::Change)
        );
        assert!(
            result
                .paths
                .iter()
                .any(|path| path.path == "added" && path.action == RestoreAction::Remove)
        );
        assert!(
            result.paths.iter().all(|path| path.later_change.is_none()),
            "missing end observation never means unchanged"
        );
        recheck(&files, &states, &plan, 1, at()).unwrap();
        std::fs::write(files.root().root_path().join("unexpected"), "keep me").unwrap();
        assert!(matches!(
            recheck(&files, &states, &plan, 1, at()),
            Err(HostError::Changed(_))
        ));
        assert_eq!(
            std::fs::read(files.root().root_path().join("code")).unwrap(),
            b"current"
        );
        assert_eq!(
            std::fs::read(files.root().root_path().join("unexpected")).unwrap(),
            b"keep me"
        );
    }

    #[test]
    fn identical_bytes_recreated_paths_mode_changes_and_new_ignores_require_a_fresh_plan() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, files, states, request) = fixture();
        let (plan, _) = super::plan(&files, &states, &request, 1, at()).unwrap();
        std::fs::rename(
            files.root().root_path().join("code"),
            dir.path().join("previous"),
        )
        .unwrap();
        std::fs::write(files.root().root_path().join("code"), "before").unwrap();
        assert!(matches!(
            recheck(&files, &states, &plan, 1, at()),
            Err(HostError::Changed(_))
        ));
        let (fresh, _) = super::plan(&files, &states, &request, 1, at()).unwrap();
        std::fs::set_permissions(
            files.root().root_path().join("code"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        assert!(matches!(
            recheck(&files, &states, &fresh, 1, at()),
            Err(HostError::Changed(_))
        ));
        std::fs::write(files.root().root_path().join(".gitignore"), "code\n").unwrap();
        assert!(matches!(
            super::plan(&files, &states, &request, 1, at()),
            Err(HostError::Ignored(_))
        ));
        assert_eq!(
            std::fs::read(files.root().root_path().join("code")).unwrap(),
            b"before"
        );
    }

    fn confirmation(
        request: &FsRestorePlan,
        result: &FsRestorePlanResult,
    ) -> orch_link_contract::source::FsRestore {
        orch_link_contract::source::FsRestore {
            work: None,
            environment_id: request.environment_id,
            job_id: new_id(),
            snapshot_id: request.snapshot_id,
            undo_snapshot_id: new_id(),
            plan_blob: result.plan_blob,
            generation: 1,
        }
    }

    #[test]
    fn raw_capture_and_restore_report_tree_and_configured_byte_boundary_measurements() {
        use std::io::Write as _;
        // A modest code-tree shape exercises path overhead separately from the shipped byte
        // bound. The boundary is the real configured default, not an invented smaller limit.
        const CODE_FILES: usize = 512;
        const CODE_BYTES: usize = 4096;
        fn stored_bytes(path: &std::path::Path) -> u64 {
            std::fs::read_dir(path)
                .unwrap()
                .map(|entry| {
                    let entry = entry.unwrap();
                    let metadata = entry.metadata().unwrap();
                    if metadata.is_dir() {
                        stored_bytes(&entry.path())
                    } else {
                        metadata.len()
                    }
                })
                .sum()
        }
        for boundary in [false, true] {
            let (_dir, files, states, mut request) = fixture();
            let root = files.root().root_path();
            let expected_bytes = if boundary {
                let file = std::fs::File::create(root.join("code")).unwrap();
                file.set_len(crate::file_states::DEFAULT_RECORD_MAX_BYTES as u64)
                    .unwrap();
                crate::file_states::DEFAULT_RECORD_MAX_BYTES
            } else {
                std::fs::create_dir(root.join("src")).unwrap();
                for number in 0..CODE_FILES {
                    let mut bytes = vec![b' '; CODE_BYTES];
                    let source = format!("fn generated_{number}() -> usize {{ {number} }}\n");
                    bytes[..source.len()].copy_from_slice(source.as_bytes());
                    std::fs::write(root.join(format!("src/code-{number:04}.rs")), bytes).unwrap();
                }
                (CODE_FILES * CODE_BYTES + b"before".len()) as i64
            };
            let index = Index::new(IndexMode::Full, files.limits().index_max_entries);
            index.scan(files.root(), files.ignore().unwrap()).unwrap();
            let store_before = stored_bytes(states.store().root());
            let baseline_start = std::time::Instant::now();
            states.refresh(&files, &index, None, true).unwrap();
            let baseline_time = baseline_start.elapsed();
            request.snapshot_id = new_id();
            let record_start = std::time::Instant::now();
            let recorded = states
                .record(
                    &files,
                    &FsRecord {
                        environment_id: request.environment_id,
                        snapshot_id: request.snapshot_id,
                        directory: String::new(),
                        kind: RecordKind::Turn,
                        record_days: crate::file_states::DEFAULT_RECORD_DAYS,
                        max_bytes: crate::file_states::DEFAULT_RECORD_MAX_BYTES,
                    },
                    1,
                    at(),
                )
                .unwrap();
            let record_time = record_start.elapsed();
            assert_eq!(
                recorded.state,
                orch_link_contract::source::RecordState::Complete
            );
            assert_eq!(recorded.bytes, expected_bytes);
            let store_after_record = stored_bytes(states.store().root());
            let (_saved, entries) = states.store().load(request.snapshot_id, at()).unwrap();
            let original_hash = entries
                .entries
                .iter()
                .find(|entry| entry.p == "code")
                .unwrap()
                .h
                .unwrap();
            std::fs::OpenOptions::new()
                .write(true)
                .open(root.join("code"))
                .unwrap()
                .write_all(b"changed\n")
                .unwrap();
            let plan_start = std::time::Instant::now();
            let (_, shown) = super::plan(&files, &states, &request, 1, at()).unwrap();
            let plan_time = plan_start.elapsed();
            let restore_start = std::time::Instant::now();
            let restored =
                apply(&files, &states, &confirmation(&request, &shown), 1, at()).unwrap();
            let restore_time = restore_start.elapsed();
            assert!(!restored.undone);
            assert_eq!(
                crate::blobs::hash_file(&std::fs::File::open(root.join("code")).unwrap()).unwrap(),
                original_hash
            );
            let store_after_restore = stored_bytes(states.store().root());
            eprintln!(
                "raw-file-measurement boundary={boundary} paths={} bytes={} baseline_ms={} record_us={} plan_ms={} restore_ms={} capture_store_growth={} restore_store_growth={}",
                recorded.files,
                recorded.bytes,
                baseline_time.as_millis(),
                record_time.as_micros(),
                plan_time.as_millis(),
                restore_time.as_millis(),
                store_after_record.saturating_sub(store_before),
                store_after_restore.saturating_sub(store_after_record)
            );
        }
    }

    #[test]
    fn restoring_a_whole_state_keeps_an_exact_undo_and_never_changes_project_git() {
        let (_dir, files, states, request) = fixture();
        let workspace = files.root().root_path();
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .current_dir(workspace)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "Git fixture failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["init", "-b", "test"]);
        git(&["add", "code"]);
        git(&[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@orch.invalid",
            "commit",
            "-m",
            "base",
        ]);
        std::fs::write(workspace.join("code"), "current").unwrap();
        std::fs::write(workspace.join("added"), [0, 255, 4]).unwrap();
        git(&["add", "code"]);
        let git_paths = [
            ".git/index",
            ".git/HEAD",
            ".git/config",
            ".git/refs/heads/test",
            ".git/logs/HEAD",
        ];
        let unchanged: Vec<Vec<u8>> = git_paths
            .iter()
            .map(|path| std::fs::read(workspace.join(path)).unwrap())
            .collect();
        let (_, planned) = super::plan(&files, &states, &request, 1, at()).unwrap();
        let apply = confirmation(&request, &planned);
        let restored = super::apply(&files, &states, &apply, 1, at()).unwrap();
        assert!(!restored.undone);
        assert_eq!(std::fs::read(workspace.join("code")).unwrap(), b"before");
        assert!(!workspace.join("added").exists());
        assert_eq!(
            super::apply(&files, &states, &apply, 1, at()).unwrap(),
            restored,
            "lost replies return the same result"
        );
        let (_, undo_plan) = super::plan(
            &files,
            &states,
            &FsRestorePlan {
                snapshot_id: restored.undo_snapshot_id,
                ..request.clone()
            },
            1,
            at(),
        )
        .unwrap();
        let undo = orch_link_contract::source::FsRestore {
            work: None,
            snapshot_id: restored.undo_snapshot_id,
            ..confirmation(&request, &undo_plan)
        };
        let returned = super::apply(&files, &states, &undo, 1, at()).unwrap();
        assert!(!returned.undone);
        assert_eq!(std::fs::read(workspace.join("code")).unwrap(), b"current");
        assert_eq!(std::fs::read(workspace.join("added")).unwrap(), [0, 255, 4]);
        for (path, expected) in git_paths.iter().zip(unchanged) {
            assert_eq!(
                std::fs::read(workspace.join(path)).unwrap(),
                expected,
                "restore/Undo altered {path}"
            );
        }
    }

    #[test]
    fn a_restart_at_each_apply_boundary_finishes_the_same_state_and_keeps_original_undo() {
        for stopped_at in [
            "prepared",
            "applying",
            "before_effect",
            "after_effect",
            "after_advance",
            "complete",
        ] {
            let (_dir, files, states, request) = fixture();
            std::fs::write(files.root().root_path().join("code"), "current").unwrap();
            std::fs::write(files.root().root_path().join("added"), "keep in undo").unwrap();
            let (_, result) = super::plan(&files, &states, &request, 1, at()).unwrap();
            let apply = confirmation(&request, &result);
            let mut stopped = false;
            let outcome =
                apply_with_checkpoints(&files, &states, &apply, 1, at(), &mut |phase, _| {
                    if !stopped && phase == stopped_at {
                        stopped = true;
                        return Err(HostError::Internal(
                            "simulated connector restart".to_owned(),
                        ));
                    }
                    Ok(())
                });
            assert!(
                stopped && outcome.is_err(),
                "checkpoint {stopped_at} was not reached"
            );
            let store_path = states.store().root().to_owned();
            drop(states);
            let restarted =
                FileStates::new(ObjectStore::open(&store_path, files.root().root_path()).unwrap());
            let restored = super::apply(&files, &restarted, &apply, 1, at()).unwrap();
            assert!(!restored.undone, "{stopped_at}");
            assert_eq!(
                std::fs::read(files.root().root_path().join("code")).unwrap(),
                b"before",
                "{stopped_at}"
            );
            assert!(
                !files.root().root_path().join("added").exists(),
                "{stopped_at}"
            );
            let (undo, parsed) = restarted
                .store()
                .load(restored.undo_snapshot_id, at())
                .unwrap();
            let code = parsed
                .entries
                .iter()
                .find(|entry| entry.p == "code")
                .unwrap();
            assert_eq!(
                std::fs::read(restarted.store().read_entry(&undo, code).unwrap().path()).unwrap(),
                b"current",
                "{stopped_at}"
            );
            assert!(
                parsed.entries.iter().any(|entry| entry.p == "added"),
                "{stopped_at}"
            );
        }
    }

    #[test]
    fn interrupted_apply_keeps_roots_past_expiry_and_does_not_replace_a_later_writer() {
        let (_dir, files, states, request) = fixture();
        std::fs::write(files.root().root_path().join("code"), "current").unwrap();
        let (_, result) = super::plan(&files, &states, &request, 1, at()).unwrap();
        let apply = confirmation(&request, &result);
        let outcome = apply_with_checkpoints(&files, &states, &apply, 1, at(), &mut |phase, _| {
            if phase == "applying" {
                return Err(HostError::Internal("simulated restart".to_owned()));
            }
            Ok(())
        });
        assert!(outcome.is_err());
        let expired = Time::new(
            at().get() + chrono::Duration::days(crate::file_states::DEFAULT_RECORD_DAYS + 1),
        );
        assert!(
            states.store().load(request.snapshot_id, expired).is_err(),
            "ordinary expired reads still refuse"
        );
        assert_eq!(
            states
                .prune(&[request.snapshot_id, apply.undo_snapshot_id], expired)
                .unwrap(),
            0,
            "unfinished journals protect both immutable roots after pin expiry"
        );
        let restored = super::apply(&files, &states, &apply, 1, expired).unwrap();
        assert!(!restored.undone);
        assert_eq!(
            std::fs::read(files.root().root_path().join("code")).unwrap(),
            b"before"
        );

        let (_dir, files, states, request) = fixture();
        std::fs::write(files.root().root_path().join("code"), "current").unwrap();
        let (_, result) = super::plan(&files, &states, &request, 1, at()).unwrap();
        let apply = confirmation(&request, &result);
        apply_with_checkpoints(&files, &states, &apply, 1, at(), &mut |phase, _| {
            if phase == "after_effect" {
                return Err(HostError::Internal("simulated restart".to_owned()));
            }
            Ok(())
        })
        .unwrap_err();
        std::fs::write(files.root().root_path().join("code"), "later writer").unwrap();
        assert!(matches!(
            super::apply(&files, &states, &apply, 1, at()),
            Err(HostError::Changed(_))
        ));
        assert_eq!(
            std::fs::read(files.root().root_path().join("code")).unwrap(),
            b"later writer"
        );
        let bytes = std::fs::read(journal_path(&states, apply.job_id)).unwrap();
        let journal: Journal = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            journal.phase,
            JournalPhase::RollingBack,
            "recovery remains held instead of claiming success"
        );
    }

    #[test]
    fn corrupted_target_bytes_write_nothing_and_a_mid_apply_failure_recovers_complete_undo() {
        let (_dir, files, states, request) = fixture();
        std::fs::write(files.root().root_path().join("code"), "current").unwrap();
        let (_, result) = super::plan(&files, &states, &request, 1, at()).unwrap();
        let apply = confirmation(&request, &result);
        let root = states.store().load(request.snapshot_id, at()).unwrap().0;
        let oid = root.objects.get("code").unwrap();
        let object = states
            .store()
            .root()
            .join("git/objects")
            .join(&oid[..2])
            .join(&oid[2..]);
        std::fs::write(&object, "corrupt target").unwrap();
        assert!(super::apply(&files, &states, &apply, 1, at()).is_err());
        assert_eq!(
            std::fs::read(files.root().root_path().join("code")).unwrap(),
            b"current",
            "every target byte is verified before any workspace effect"
        );

        let (_dir, files, states, request) = fixture();
        std::fs::write(files.root().root_path().join("code"), "current").unwrap();
        std::fs::write(files.root().root_path().join("added"), "made later").unwrap();
        let (_, result) = super::plan(&files, &states, &request, 1, at()).unwrap();
        let apply = confirmation(&request, &result);
        let root = states.store().load(request.snapshot_id, at()).unwrap().0;
        let oid = root.objects.get("code").unwrap();
        let object = states
            .store()
            .root()
            .join("git/objects")
            .join(&oid[..2])
            .join(&oid[2..]);
        let mut damaged = false;
        let outcome = apply_with_checkpoints(&files, &states, &apply, 1, at(), &mut |phase, _| {
            if phase == "after_effect" && !damaged {
                damaged = true;
                std::fs::write(&object, "a disk failure after the first effect").unwrap();
            }
            if phase == "after_rollback_effect" {
                return Err(HostError::Internal(
                    "simulated restart during rollback".to_owned(),
                ));
            }
            Ok(())
        });
        assert!(damaged && outcome.is_err());
        let store_path = states.store().root().to_owned();
        drop(states);
        let restarted =
            FileStates::new(ObjectStore::open(&store_path, files.root().root_path()).unwrap());
        let recovered = super::apply(&files, &restarted, &apply, 1, at()).unwrap();
        assert!(
            recovered.undone,
            "an incomplete apply returns the whole prior state"
        );
        assert_eq!(
            std::fs::read(files.root().root_path().join("code")).unwrap(),
            b"current"
        );
        assert_eq!(
            std::fs::read(files.root().root_path().join("added")).unwrap(),
            b"made later"
        );
        assert_eq!(
            super::apply(&files, &restarted, &apply, 1, at()).unwrap(),
            recovered
        );
    }

    #[test]
    fn whole_restore_keeps_binary_bytes_raw_links_empty_directories_and_executable_bits() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let (dir, files, states, mut request) = fixture();
        let workspace = files.root().root_path();
        std::fs::write(workspace.join("binary"), [0, 255, 1, 128]).unwrap();
        std::fs::set_permissions(
            workspace.join("binary"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        std::fs::create_dir(workspace.join("empty")).unwrap();
        std::fs::write(dir.path().join("outside"), "never follow this link").unwrap();
        symlink("../outside", workspace.join("raw-link")).unwrap();
        std::fs::write(workspace.join(".gitignore"), "private\n").unwrap();
        std::fs::write(workspace.join("private"), "ignored before").unwrap();
        let index = Index::new(IndexMode::Full, files.limits().index_max_entries);
        index.scan(files.root(), files.ignore().unwrap()).unwrap();
        states.refresh(&files, &index, None, true).unwrap();
        request.snapshot_id = new_id();
        states
            .record(
                &files,
                &FsRecord {
                    environment_id: request.environment_id,
                    snapshot_id: request.snapshot_id,
                    directory: String::new(),
                    kind: RecordKind::Turn,
                    record_days: crate::file_states::DEFAULT_RECORD_DAYS,
                    max_bytes: crate::file_states::DEFAULT_RECORD_MAX_BYTES,
                },
                1,
                at(),
            )
            .unwrap();
        std::fs::write(workspace.join("binary"), "current").unwrap();
        std::fs::set_permissions(
            workspace.join("binary"),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        std::fs::remove_dir(workspace.join("empty")).unwrap();
        std::fs::remove_file(workspace.join("raw-link")).unwrap();
        symlink("../different-outside", workspace.join("raw-link")).unwrap();
        std::fs::write(workspace.join("private"), "ignored later").unwrap();
        let (_, result) = super::plan(&files, &states, &request, 1, at()).unwrap();
        super::apply(&files, &states, &confirmation(&request, &result), 1, at()).unwrap();
        assert_eq!(
            std::fs::read(workspace.join("binary")).unwrap(),
            [0, 255, 1, 128]
        );
        assert_eq!(
            std::fs::metadata(workspace.join("binary"))
                .unwrap()
                .permissions()
                .mode()
                & EXECUTABLE_BITS,
            0o111
        );
        assert!(workspace.join("empty").is_dir());
        assert_eq!(
            std::fs::read_link(workspace.join("raw-link")).unwrap(),
            std::path::PathBuf::from("../outside")
        );
        assert_eq!(
            std::fs::read(workspace.join("private")).unwrap(),
            b"ignored later"
        );
        assert_eq!(
            std::fs::read(dir.path().join("outside")).unwrap(),
            b"never follow this link"
        );
    }
}
