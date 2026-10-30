//! Making a spec's source available on the host (pull, build, load) and the periodic look for a newer image behind a tag.
//!
//! **Making an image available** is `node.image.ensure`: the driver pulls a reference, builds a Dockerfile or
//! loads an archive, and answers the digest it now holds. What is long tells its progress as it goes: the
//! lines the driver tells become the progress of the job the command belongs to (`job.progress`, at most one
//! a second), with a percentage where the lines allow one to be worked out (the step of a build, the layers
//! of a pull), and the end of a build's log, at most [`LOG_TAIL_BYTES`], which stays on the job when the build
//! fails. A build's Dockerfile context and an image archive are blobs of the system; they are fetched for the
//! command that names them, by the desk ([`BlobDesk`]) the driver asks for them through, and the files go
//! when the command is over.
//!
//! **A newer image behind a tag.** A unit runs what it was made with, and nothing replaces an image behind its
//! back. The node notes the digest each image had when it was made available, and [`Images::look`] asks the
//! registry what is behind each tag in use now (for a build, behind each base image it started from). What it
//! finds is kept: the next `node.image.ensure` of a build whose bases moved builds it again instead of
//! finding it, and [`Images::behind`] says which units run an image that has fallen behind, so that the node
//! can tell the system.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use orch_api_contract::workspace::{JobPhase, JobProgress, PhaseState};
use orch_connector_core::content::Content;
use orch_driver_contract::sources::{BuildContext, ImageEnsured, Source};
use orch_link_contract::commands::{
    CommandError, ErrorCode, NodeImageEnsureBase, NodeImageEnsureBuilt, NodeImageEnsureResult,
};
use orch_types::{Hash, Id, Json};
use tokio_util::sync::CancellationToken;

use crate::drivers::{DriverHost, failure};
use crate::facts::Reports;
use crate::ledger::{ImageNote, ImageUse, Ledger};

/// How much of the end of a build's log stays on the job: 64 KiB, which is what a job's log holds.
pub const LOG_TAIL_BYTES: usize = 64 * 1024;

/// The most progress reports of one job that are sent in a second.
pub const PROGRESS_EVERY: Duration = Duration::from_secs(1);

/// How long the line shown beside a job's bar may be, in characters: a line of a build can be a whole shell
/// command, and the bar's line is for a glance.
const MESSAGE_CHARS: usize = 200;

/// The name of the phase of a provisioning job that the node's progress is of.
pub const IMAGE_PHASE: &str = "image";

pub use orch_driver_contract::sources::{ImageProgress, ImageStep};

/// A listener that wants to hear nothing.
#[derive(Debug, Clone, Copy, Default)]
pub struct Silent;

impl ImageProgress for Silent {
    fn line(&self, _: ImageStep, _: &str) {}
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

// ------------------------------------------------------------------------------------------------ blobs

/// Fetches the blobs a command names, for the driver that is asked for them by hash.
///
/// The system moves content only for a command that is open and names it, and a driver asks for a blob by its
/// hash alone. So the node tells the desk, before it asks the driver, which command names which blobs
/// ([`BlobDesk::announce`]); when the driver asks, the desk fetches the blob for that command into the
/// command's scratch directory and says where the file is. The desk is made before the connector (the driver
/// needs it to be made) and given its means of moving content when the connector has them.
#[derive(Default)]
pub struct BlobDesk {
    content: Mutex<Option<Arc<Content>>>,
    announced: Mutex<HashMap<Hash, Id>>,
}

impl std::fmt::Debug for BlobDesk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlobDesk").finish_non_exhaustive()
    }
}

/// The blobs of one command, announced until it is dropped.
pub struct Announcement {
    desk: Arc<BlobDesk>,
    hashes: Vec<Hash>,
}

impl Drop for Announcement {
    fn drop(&mut self) {
        let mut announced = lock(&self.desk.announced);
        for hash in &self.hashes {
            announced.remove(hash);
        }
    }
}

impl BlobDesk {
    /// A desk with no means of moving content yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Gives the desk the connector's means of moving content.
    pub fn attach(&self, content: Arc<Content>) {
        *lock(&self.content) = Some(content);
    }

    /// Says that `command` names these blobs, until the returned value is dropped.
    #[must_use]
    pub fn announce(self: &Arc<Self>, command: Id, hashes: Vec<Hash>) -> Announcement {
        let mut announced = lock(&self.announced);
        for hash in &hashes {
            announced.insert(*hash, command);
        }
        drop(announced);
        Announcement {
            desk: Arc::clone(self),
            hashes,
        }
    }

    /// Fetches the blob `hash` for the command that announced it, and answers where the file is. A blob
    /// that no open command names is refused: the system would not move it either.
    pub async fn fetch(&self, hash: &Hash) -> Result<PathBuf, CommandError> {
        let command = lock(&self.announced).get(hash).copied().ok_or_else(|| {
            CommandError::new(
                ErrorCode::NotFound,
                format!("no command that is running names the blob {hash}"),
            )
        })?;
        let content = lock(&self.content).clone().ok_or_else(|| {
            CommandError::new(
                ErrorCode::Internal,
                "this node cannot move content: it is not connected to a system yet",
            )
        })?;
        let size = content.size_of(command, *hash).await?;
        content
            .fetch(command, *hash, size, &format!("{hash}.blob"))
            .await
    }

    /// Removes the files fetched for a command that is over.
    pub async fn finish(&self, command: Id) {
        let content = lock(&self.content).clone();
        if let Some(content) = content {
            content.finish(command).await;
        }
    }
}

/// The blobs a spec names as the source of its image: the context archive of a build, an image archive.
#[must_use]
pub fn blobs_named_by(spec: &Json) -> Vec<Hash> {
    match Source::from_spec(spec) {
        Ok(Source::Build(build)) => match build.context {
            Some(BuildContext::Blob { blob }) => vec![blob],
            _ => Vec::new(),
        },
        Ok(Source::Archive(archive)) => vec![archive.blob],
        _ => Vec::new(),
    }
}

// ------------------------------------------------------------------------------------------------ progress

/// What a source is noted under: its reference, or what defines a build or an archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceKey {
    /// The key.
    pub key: String,
    /// The registry reference of an image source.
    pub reference: Option<String>,
    /// Whether a digest pins it.
    pub pinned: bool,
    /// Whether it is a build.
    pub build: bool,
}

impl SourceKey {
    /// The key of the source a spec names; `None` when it names none the node can read.
    #[must_use]
    pub fn of(spec: &Json) -> Option<Self> {
        match Source::from_spec(spec).ok()? {
            Source::Image { reference, digest } => Some(Self {
                key: digest.as_ref().map_or_else(
                    || reference.clone(),
                    |digest| format!("{reference}@{digest}"),
                ),
                pinned: digest.is_some() || reference.contains("@sha256:"),
                reference: Some(reference),
                build: false,
            }),
            Source::Build(build) => {
                let identity = match &build.context {
                    Some(BuildContext::Blob { blob }) => Some(blob.to_string()),
                    Some(BuildContext::Git {
                        git,
                        reference,
                        path,
                    }) => Some(format!("{git}#{reference}:{path}")),
                    None => None,
                };
                Some(Self {
                    key: format!("build:{}", build.definition_hash(identity.as_deref())),
                    reference: None,
                    pinned: false,
                    build: true,
                })
            }
            Source::Archive(archive) => Some(Self {
                key: format!("archive:{}", archive.blob),
                reference: None,
                pinned: true,
                build: false,
            }),
        }
    }
}

#[derive(Default)]
struct Shown {
    step: Option<ImageStep>,
    message: Option<String>,
    percent: u32,
    log: String,
    layers: HashSet<String>,
    layers_done: HashSet<String>,
    version: u64,
    sent: u64,
}

/// Collects the lines an operation tells and reports them as the progress of a job.
pub struct Reporter {
    job: Option<Id>,
    reports: Arc<Reports>,
    shown: Mutex<Shown>,
}

impl ImageProgress for Reporter {
    fn line(&self, step: ImageStep, text: &str) {
        let mut shown = lock(&self.shown);
        shown.step = Some(step);
        shown.message = Some(text.chars().take(MESSAGE_CHARS).collect());
        shown.log.push_str(text);
        shown.log.push('\n');
        cut_to_tail(&mut shown.log);
        let percent = match step {
            ImageStep::Build => step_of_build(text),
            ImageStep::Pull => layers_of_pull(&mut shown, text),
            ImageStep::Load => None,
        };
        if let Some(percent) = percent {
            // A bar never goes back.
            shown.percent = shown.percent.max(percent.min(99));
        }
        shown.version += 1;
    }
}

/// Keeps the last [`LOG_TAIL_BYTES`] of a log, from the start of a line where there is one near.
fn cut_to_tail(log: &mut String) {
    if log.len() <= LOG_TAIL_BYTES {
        return;
    }
    let mut cut = log.len() - LOG_TAIL_BYTES;
    while !log.is_char_boundary(cut) {
        cut += 1;
    }
    if let Some(newline) = log[cut..].find('\n') {
        cut += newline + 1;
    }
    log.drain(..cut);
}

/// How far a classic build is, from a line such as `Step 3/7 : RUN make`.
fn step_of_build(line: &str) -> Option<u32> {
    let rest = line.strip_prefix("Step ")?;
    let (counts, _) = rest.split_once(" : ").unwrap_or((rest, ""));
    let (done, all) = counts.split_once('/')?;
    let (done, all): (u32, u32) = (done.trim().parse().ok()?, all.trim().parse().ok()?);
    (all > 0).then(|| (done.saturating_sub(1)).saturating_mul(100) / all)
}

/// How far a pull is, from lines such as `3f9a: Pull complete`: the layers done out of the layers seen.
fn layers_of_pull(shown: &mut Shown, line: &str) -> Option<u32> {
    let (layer, status) = line.split_once(": ")?;
    if layer.is_empty() || layer.contains(' ') {
        return None;
    }
    shown.layers.insert(layer.to_owned());
    if status.starts_with("Pull complete") || status.starts_with("Already exists") {
        shown.layers_done.insert(layer.to_owned());
    }
    let (done, seen) = (shown.layers_done.len(), shown.layers.len());
    u32::try_from(done.saturating_mul(100) / seen.max(1)).ok()
}

impl Reporter {
    /// A reporter for the job `job`; without one the lines are told to the log alone.
    #[must_use]
    pub fn new(job: Option<Id>, reports: Arc<Reports>) -> Arc<Self> {
        Arc::new(Self {
            job,
            reports,
            shown: Mutex::new(Shown::default()),
        })
    }

    fn progress(shown: &Shown, state: PhaseState, percent: u32) -> JobProgress {
        JobProgress {
            activity: None,
            phase: IMAGE_PHASE.to_owned(),
            phases: vec![JobPhase {
                name: IMAGE_PHASE.to_owned(),
                weight: 100,
                state,
            }],
            percent,
            files_done: 0,
            files_total: None,
            bytes_done: 0,
            bytes_total: None,
            rate_bps: None,
            eta_seconds: None,
            message: shown.message.clone(),
        }
    }

    /// Reports what changed since the last report, once every [`PROGRESS_EVERY`], for as long as it is not
    /// dropped. It never ends, so it is raced against the work it reports on.
    pub async fn pump(&self) -> Infallible {
        loop {
            tokio::time::sleep(PROGRESS_EVERY).await;
            let report = {
                let mut shown = lock(&self.shown);
                (shown.version != shown.sent).then(|| {
                    shown.sent = shown.version;
                    Self::progress(&shown, PhaseState::Running, shown.percent)
                })
            };
            if let (Some(job), Some(report)) = (self.job, report) {
                self.reports.job(job, &report, None).await;
            }
        }
    }

    /// Reports the end: the bar full when it worked, and the end of the log in both cases.
    pub async fn finish(&self, worked: bool) {
        let Some(job) = self.job else { return };
        let (report, log) = {
            let shown = lock(&self.shown);
            let (state, percent) = if worked {
                (PhaseState::Done, 100)
            } else {
                (PhaseState::Running, shown.percent)
            };
            (
                Self::progress(&shown, state, percent),
                (shown.step == Some(ImageStep::Build) && !shown.log.is_empty())
                    .then(|| shown.log.clone()),
            )
        };
        self.reports.job(job, &report, log).await;
    }

    /// The end of what the operation told, for a log line or a test.
    #[must_use]
    pub fn log(&self) -> String {
        lock(&self.shown).log.clone()
    }
}

// ------------------------------------------------------------------------------------------------ the node's images

/// An image that has fallen behind what the registry has now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Behind {
    /// The units that run it.
    pub runtimes: Vec<Id>,
    /// What moved, in a sentence.
    pub why: String,
    /// The key the image is noted under.
    pub key: String,
}

/// The images of the host.
pub struct Images {
    policy: Mutex<Option<Arc<orch_connector_core::policy::PolicyStore>>>,
    drivers: Arc<DriverHost>,
    ledger: Arc<Ledger>,
    reports: Arc<Reports>,
    desk: Arc<BlobDesk>,
    behind: Mutex<BTreeMap<String, Behind>>,
}

impl std::fmt::Debug for Images {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Images").finish_non_exhaustive()
    }
}

fn built_of(ensured: ImageEnsured) -> NodeImageEnsureResult {
    NodeImageEnsureResult {
        programs: ensured.programs,
        digest: ensured.digest,
        built: ensured.built.map(|built| NodeImageEnsureBuilt {
            commit: built.commit,
            bases: built
                .bases
                .into_iter()
                .map(|base| NodeImageEnsureBase {
                    reference: base.reference,
                    digest: base.digest,
                })
                .collect(),
        }),
    }
}

fn source_scope(driver: &str, key: &str) -> String {
    // Length-prefix the implementation id so opaque source keys cannot meet another driver's cache.
    format!("{}:{driver}:{key}", driver.len())
}

impl Images {
    /// The images of a host whose drivers are `drivers`.
    #[must_use]
    pub fn new(
        drivers: Arc<DriverHost>,
        ledger: Arc<Ledger>,
        reports: Arc<Reports>,
        desk: Arc<BlobDesk>,
    ) -> Self {
        Self {
            policy: Mutex::new(None),
            drivers,
            ledger,
            reports,
            desk,
            behind: Mutex::new(BTreeMap::new()),
        }
    }

    /// The same live machine policy used by runtime creation.
    pub fn set_policy(&self, policy: Arc<orch_connector_core::policy::PolicyStore>) {
        *lock(&self.policy) = Some(policy);
    }

    /// Makes the source `spec` names available with the driver `driver`, for `command`, reporting its progress
    /// for `job`. A driver without sources does nothing and answers no digest.
    pub async fn ensure(
        &self,
        command: Id,
        driver: &str,
        spec: &Json,
        job: Option<Id>,
        cancel: &CancellationToken,
    ) -> Result<NodeImageEnsureResult, CommandError> {
        let host = self.drivers.get(driver).ok_or_else(|| {
            CommandError::new(
                ErrorCode::Unsupported,
                format!("this host has no driver {driver:?}"),
            )
        })?;
        if let Some(policy) = lock(&self.policy).as_ref().and_then(|store| store.policy()) {
            self.drivers
                .check_configuration(driver, policy.drivers.configurations.get(driver))?;
            if !policy.drivers.offers(driver) {
                return Err(CommandError::new(
                    ErrorCode::Unsupported,
                    format!("drivers (node role): the machine's operator does not offer {driver}"),
                ));
            }
            policy
                .drivers
                .check(&host.describe(), spec, None, &Default::default())
                .map_err(|reason| CommandError::new(ErrorCode::Unsupported, reason))?;
        }
        let key = SourceKey::of(spec);
        let again = key.as_ref().is_some_and(|key| {
            key.build && lock(&self.behind).contains_key(&source_scope(driver, &key.key))
        });
        let _announced = self.desk.announce(command, blobs_named_by(spec));
        let reporter = Reporter::new(job, Arc::clone(&self.reports));
        let work = async {
            if again {
                host.rebuild_image(spec, reporter.as_ref()).await
            } else {
                host.ensure_image(spec, reporter.as_ref()).await
            }
        };
        let outcome = tokio::select! {
            outcome = work => outcome,
            never = reporter.pump() => match never {},
            () = cancel.cancelled() => {
                reporter.finish(false).await;
                return Err(CommandError::new(
                    ErrorCode::Timeout,
                    "the system asked for the image not to be made after all",
                ));
            }
        };
        reporter.finish(outcome.is_ok()).await;
        match outcome {
            Ok(ensured) => {
                if let Some(key) = &key {
                    let scoped = source_scope(driver, &key.key);
                    lock(&self.behind).remove(&scoped);
                    let note = ImageNote {
                        digest: ensured.digest.clone(),
                        bases: ensured
                            .built
                            .as_ref()
                            .map(|built| built.bases.clone())
                            .unwrap_or_default(),
                    };
                    if let Err(error) = self.ledger.note_image(&scoped, note) {
                        tracing::warn!(%error, "what was noted of an image could not be kept");
                    }
                }
                Ok(built_of(ensured))
            }
            Err(error) => {
                let mut failed = failure(&error);
                if failed.detail.is_empty() {
                    failed.detail = reporter.log();
                }
                Err(failed)
            }
        }
    }

    /// What the spec of a runtime makes the ledger note about its image.
    #[must_use]
    pub fn use_of(spec: &Json) -> Option<ImageUse> {
        SourceKey::of(spec).map(|key| ImageUse {
            key: key.key,
            reference: key.reference,
            pinned: key.pinned,
        })
    }

    /// Asks each driver's registry what is behind the tags in use, and keeps what has moved. An image
    /// pinned by a digest never moves, and an archive never falls behind by itself.
    pub async fn look(&self) -> Vec<Behind> {
        let notes = self.ledger.images();
        // The units in use, by the image they run; an image that a digest pins is not looked at.
        let mut in_use: BTreeMap<(String, String), Vec<crate::ledger::Record>> = BTreeMap::new();
        let mut owners: BTreeMap<String, std::collections::BTreeSet<String>> = BTreeMap::new();
        for record in self.ledger.all() {
            if let Some(image) = record.image.clone().filter(|image| !image.pinned) {
                owners
                    .entry(image.key.clone())
                    .or_default()
                    .insert(record.driver.clone());
                in_use
                    .entry((record.driver.clone(), image.key))
                    .or_default()
                    .push(record);
            }
        }
        let mut found: Vec<Behind> = Vec::new();
        for ((driver, key), records) in in_use {
            let scoped = source_scope(&driver, &key);
            // Older ledgers have no namespace. Reuse such an observation only when its owner is unambiguous.
            let note = notes.get(&scoped).or_else(|| {
                (owners.get(&key).is_some_and(|owners| owners.len() == 1))
                    .then(|| notes.get(&key))
                    .flatten()
            });
            let (Some(note), Some(first)) = (note, records.first()) else {
                continue;
            };
            let Some(host) = self.drivers.get(&first.driver) else {
                continue;
            };
            // What is asked of the registry: the tag itself, and for a build the bases it started from.
            let mut asked: Vec<(String, Option<String>)> = Vec::new();
            if let Some(reference) = first
                .image
                .as_ref()
                .and_then(|image| image.reference.clone())
            {
                asked.push((reference, note.digest.clone()));
            }
            asked.extend(
                note.bases
                    .iter()
                    .map(|base| (base.reference.clone(), Some(base.digest.clone()))),
            );
            for (reference, had) in asked {
                match host.extras().registry_digest(&reference).await {
                    Ok(Some(now)) if had.as_deref().is_some_and(|had| had != now) => {
                        found.push(Behind {
                            runtimes: records.iter().map(|record| record.runtime_id).collect(),
                            why: format!(
                                "{reference} is {now} in its registry now, and the unit was made with {}",
                                had.unwrap_or_default()
                            ),
                            key: scoped.clone(),
                        });
                        break;
                    }
                    Ok(_) => {}
                    Err(error) => {
                        tracing::warn!(%reference, %error, "the registry could not be asked what is behind a tag");
                    }
                }
            }
        }
        let mut behind = lock(&self.behind);
        behind.clear();
        behind.extend(found.iter().map(|moved| (moved.key.clone(), moved.clone())));
        found
    }

    /// The images that have fallen behind, as the last look found them.
    #[must_use]
    pub fn behind(&self) -> Vec<Behind> {
        lock(&self.behind).values().cloned().collect()
    }

    /// Why the unit of a runtime runs an image that has fallen behind, if it does.
    #[must_use]
    pub fn why_behind(&self, runtime_id: Id) -> Option<String> {
        lock(&self.behind)
            .values()
            .find(|moved| moved.runtimes.contains(&runtime_id))
            .map(|moved| moved.why.clone())
    }

    /// Removes the files fetched for a command that is over.
    pub async fn finish(&self, command: Id) {
        self.desk.finish(command).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_step_of_a_build_is_the_share_of_the_steps_before_it() {
        assert_eq!(step_of_build("Step 3/7 : RUN make"), Some(28));
        assert_eq!(step_of_build("Step 1/4 : FROM debian"), Some(0));
        assert_eq!(step_of_build("Successfully built 1a2b"), None);
        assert_eq!(step_of_build("Step x/y : RUN"), None);
    }

    #[test]
    fn a_log_keeps_its_end_from_the_start_of_a_line() {
        let mut log = String::new();
        for number in 0..20_000 {
            log.push_str(&format!(
                "line {number}: doing something that takes a line\n"
            ));
            cut_to_tail(&mut log);
        }
        assert!(log.len() <= LOG_TAIL_BYTES);
        assert!(log.starts_with("line "));
        assert!(log.ends_with("line 19999: doing something that takes a line\n"));
    }

    #[test]
    fn a_spec_is_noted_under_its_reference_its_definition_or_its_archive() {
        let image = SourceKey::of(&serde_json::json!({ "image": "debian:13" })).unwrap();
        assert_eq!(
            (image.key.as_str(), image.pinned, image.build),
            ("debian:13", false, false)
        );
        let pinned = SourceKey::of(
            &serde_json::json!({ "image": "debian:13", "image_digest": "sha256:ab" }),
        )
        .unwrap();
        assert!(pinned.pinned);
        assert_eq!(pinned.key, "debian:13@sha256:ab");
        let build =
            SourceKey::of(&serde_json::json!({ "build": { "dockerfile": "FROM x" } })).unwrap();
        let again =
            SourceKey::of(&serde_json::json!({ "build": { "dockerfile": "FROM x" } })).unwrap();
        let other =
            SourceKey::of(&serde_json::json!({ "build": { "dockerfile": "FROM y" } })).unwrap();
        assert!(build.build && build.key.starts_with("build:"));
        assert_eq!(build.key, again.key);
        assert_ne!(build.key, other.key);
        assert!(SourceKey::of(&serde_json::json!({ "runtime": "runc" })).is_none());
    }

    #[test]
    fn the_blobs_a_spec_names_are_its_build_context_and_its_archive() {
        let blob = Hash::of(b"context");
        let build =
            serde_json::json!({ "build": { "dockerfile": "FROM x", "context": { "blob": blob } } });
        assert_eq!(blobs_named_by(&build), [blob]);
        let archive = serde_json::json!({ "archive": { "blob": blob, "image": null } });
        assert_eq!(blobs_named_by(&archive), [blob]);
        assert!(blobs_named_by(&serde_json::json!({ "image": "x" })).is_empty());
    }

    #[tokio::test]
    async fn a_blob_no_running_command_names_is_not_fetched() {
        let desk = Arc::new(BlobDesk::new());
        let error = desk.fetch(&Hash::of(b"nobody asked")).await.unwrap_err();
        assert_eq!(error.code, "not_found");
        let command = orch_types::new_id();
        let hash = Hash::of(b"named");
        let announcement = desk.announce(command, vec![hash]);
        let error = desk.fetch(&hash).await.unwrap_err();
        assert_eq!(
            error.code, "internal",
            "named, and no means of moving content yet"
        );
        drop(announcement);
        assert_eq!(desk.fetch(&hash).await.unwrap_err().code, "not_found");
    }
}
