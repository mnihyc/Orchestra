//! Files of an input, shared copies and what an agent downloads written into the workspace; large ones as visible jobs.
//!
//! The copies shared with a thread are written first, by [`crate::parcels`], which also writes what an agent
//! downloads; what follows is about the files an input carries.
//!
//! A person attaches a file to a message, or forwards a message that carries images and files, and
//! the agent is to find them as files: before a turn begins (and before a steer is sent) the engine
//! writes into the thread's directory everything the input carries that has content and is not
//! there yet. It does so with one `fs.materialize` for the whole input, whose list names each file
//! by its hash, so that the machine takes the content from the store of content by itself and
//! leaves alone what is there already: asking again is harmless, and the command is asked under an
//! identifier made of the input and the list, so that a process that is started in the middle of it
//! goes on with the same command and not a second one.
//!
//! **Where files go.** All below the thread's `.orch/` directory, which is the only place a
//! machine takes a delivery to: new message attachments and direct images at their promised
//! `.orch/attachments/<name>` paths; older attachments at the path they were originally given;
//! and the images and files of forwarded messages at `.orch/uploads/<input id>/<n>-<name>`, with
//! `n` counting them in the order of the input. A path a client gave that is not under `.orch/`
//! is not trusted with a place: the file goes to that numbered place instead.
//!
//! **When it does not work** the input is held for its files before any turn, lease or steer: the files
//! of an input are written before its turn is made (see [`crate::preparation`]), and a steer's before it
//! is sent, and each file that could not be written is an issue its author recovers. Files written before
//! the turn are not written again by its start; where a turn's start writes files and one cannot be
//! written all the same, the agent is told `[file not delivered: <name> (<why>)]` in the place of its
//! path (see [`crate::instructions`]). **A big delivery is a job people can watch and stop**: from
//! [`VISIBLE_DELIVERY_BYTES`] bytes or [`VISIBLE_DELIVERY_FILES`] files the delivery is a `delivery`
//! job, carried by the runner of the engine like any other ([`Delivery`]: a lease, a request to
//! cancel, and a restart that goes on with it); the machine reports its progress on it, and the turn
//! that waits for the files looks at the job until it ends. A delivery that is cancelled or fails
//! holds the turn back no more than a refusal does.

use crate::commands::{Outcome, Outgoing};
use crate::engine::Engine;
use crate::error::EngineError;
use crate::hosting::WATCH_EVERY;
use crate::ids;
use crate::jobs::runner::{
    Done, Failure, INTERNAL, Job, Phase, Phased, Stop, Undone, initial_progress,
};
use crate::manifest::{Entry, EntryKind, Header, MANIFEST_VERSION, build};
use crate::sessions::StartSnapshot;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use orch_api_contract::threads::ContentPart;
use orch_db::queries::environments::thread_engine::EnvironmentRow;
use orch_db::queries::jobs::{self as rows, JobRow, NewJob, kind, state};
use orch_link_contract::commands::{CommandError, FsMaterialize, MaterializePurpose, SerialKey};
use orch_types::{Hash, Id, Json, Time};
use serde_json::json;
use std::collections::BTreeMap;
use std::time::Duration;

/// The name of the directory under a thread's own that holds what the system puts there.
pub const ORCH_DIR: &str = ".orch";

/// The directory of [`ORCH_DIR`] that holds what people send.
pub const UPLOADS_DIR: &str = "uploads";

/// The step a starting thread shows while its files are written.
pub const FILES_STEP: &str = "files";

/// From how many bytes a delivery is a job that people watch: a few seconds of a slow line, below
/// which a bar would be gone as soon as it was drawn.
pub const VISIBLE_DELIVERY_BYTES: i64 = 8 * 1024 * 1024;

/// From how many files a delivery is a job that people watch: many small files take long though
/// they weigh little, since each is its own request.
pub const VISIBLE_DELIVERY_FILES: usize = 50;

/// How often the wait for a command that is not a job looks up from it, to see whether the process is
/// stopping.
const RECHECK_EVERY: Duration = Duration::from_secs(2);

/// The permission bits a delivered file gets: readable by all, written by its owner.
const FILE_MODE: u32 = 0o644;

/// The longest name of a delivered file, in characters.
pub const NAME_MAX_CHARS: usize = 200;

/// The longest path a client may give a file, in bytes.
const PATH_MAX_BYTES: usize = 4_096;

/// The first phase of a delivery job: the machine is asked to write the files, in one command.
const DIRECTORIES: Phase = Phase {
    name: "directories",
    weight: 5,
};
/// The second phase, as the machine's writer reports it: the files, where the time goes.
const FILES: Phase = Phase {
    name: "files",
    weight: 90,
};
/// The third phase, as the machine's writer reports it: the links.
const LINKS: Phase = Phase {
    name: "links",
    weight: 5,
};

/// The phases a delivery job shows.
pub const PHASES: [Phase; 3] = [DIRECTORIES, FILES, LINKS];

/// Which file of an input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Slot {
    /// A file sent with the message, by its place among the parts.
    File(usize),
    /// An image of the input, by its place among the parts.
    Image(usize),
    /// An image of a forwarded message: the place of the part, and the place among its images.
    ForwardedImage(usize, usize),
    /// A file of a forwarded message: the place of the part, and the place among its files.
    ForwardedFile(usize, usize),
}

/// A file of an input that is to be written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    /// Which file it is.
    pub slot: Slot,
    /// Its name.
    pub name: String,
    /// Its content, by its hash.
    pub blob: Hash,
    /// Where it is written, from the thread's directory: always below [`ORCH_DIR`].
    pub path: String,
}

/// What became of the files of an input.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Placement {
    written: BTreeMap<Slot, String>,
    refused: BTreeMap<Slot, String>,
    sizes: BTreeMap<Slot, i64>,
}

/// The placement of an input about which nothing is known: no file of it was written or refused.
pub static NO_PLACEMENT: Placement = Placement {
    written: BTreeMap::new(),
    refused: BTreeMap::new(),
    sizes: BTreeMap::new(),
};

impl Placement {
    /// Where a file was written, from the thread's directory.
    #[must_use]
    pub fn path_of(&self, slot: Slot) -> Option<&str> {
        self.written.get(&slot).map(String::as_str)
    }

    /// The original size checked in the store before delivery.
    #[must_use]
    pub fn size_of(&self, slot: Slot) -> Option<i64> {
        self.sizes.get(&slot).copied()
    }

    /// Retains original sizes when restoring an already-completed delivery.
    pub async fn with_sizes(
        mut self,
        planned: &[Planned],
        store: &dyn orch_blob_contract::store::BlobStore,
    ) -> Result<Self, EngineError> {
        for file in planned {
            if let Some(size) = store
                .size(&file.blob)
                .await
                .map_err(EngineError::unreadable)?
            {
                self.sizes.insert(file.slot, size);
            }
        }
        Ok(self)
    }

    /// Why a file was not written.
    #[must_use]
    pub fn refusal_of(&self, slot: Slot) -> Option<&str> {
        self.refused.get(&slot).map(String::as_str)
    }

    /// Whether anything is known of any file.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.written.is_empty() && self.refused.is_empty()
    }

    /// A placement in which the files of `planned` were written.
    #[must_use]
    pub fn written(planned: &[Planned]) -> Self {
        Self {
            written: planned
                .iter()
                .map(|file| (file.slot, file.path.clone()))
                .collect(),
            refused: BTreeMap::new(),
            sizes: BTreeMap::new(),
        }
    }

    /// A placement in which none of the files of `planned` was, for `why`.
    #[must_use]
    pub fn refused(planned: &[Planned], why: &str) -> Self {
        Self {
            written: BTreeMap::new(),
            refused: planned
                .iter()
                .map(|file| (file.slot, why.to_owned()))
                .collect(),
            sizes: BTreeMap::new(),
        }
    }

    /// The same, with a file taken out of those written, for `why`.
    #[must_use]
    pub fn with_refusal(mut self, slot: Slot, why: &str) -> Self {
        self.refuse(slot, why);
        self
    }

    fn refuse(&mut self, slot: Slot, why: &str) {
        self.written.remove(&slot);
        self.refused.insert(slot, why.to_owned());
    }
}

// ---------------------------------------------------------------- planning

/// A name that is safe to put in a path as one component: no separator, no control character, no
/// leading dot, not empty, not long.
#[must_use]
pub fn safe_name(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim().trim_start_matches('.');
    let name: String = trimmed.chars().take(NAME_MAX_CHARS).collect();
    if name.is_empty() {
        "file".to_owned()
    } else {
        name
    }
}

/// The path a client gave a file, if it is one the system will write to: relative, below
/// [`ORCH_DIR`], without an empty, `.` or `..` component, of ordinary length.
#[must_use]
pub fn deliverable_path(path: &str) -> Option<String> {
    if path.len() > PATH_MAX_BYTES
        || path.starts_with('/')
        || path.contains('\\')
        || path.chars().any(char::is_control)
    {
        return None;
    }
    let mut parts = path.split('/');
    if parts.next() != Some(ORCH_DIR) {
        return None;
    }
    let rest: Vec<&str> = parts.collect();
    if rest.is_empty()
        || rest
            .iter()
            .any(|part| part.is_empty() || *part == "." || *part == "..")
    {
        return None;
    }
    Some(path.to_owned())
}

/// Where the files of an input go, in the order of the input.
#[must_use]
pub fn plan(input: Id, content: &[ContentPart]) -> Vec<Planned> {
    let mut planned = Vec::new();
    let mut numbered = 0_usize;
    let mut next_numbered = |name: &str| {
        numbered += 1;
        format!(
            "{ORCH_DIR}/{UPLOADS_DIR}/{input}/{numbered}-{}",
            safe_name(name)
        )
    };
    for (at, part) in content.iter().enumerate() {
        match part {
            ContentPart::Text { .. } => {}
            ContentPart::PastedText(paste) => {
                if paste.inline_text.is_none() {
                    planned.push(Planned {
                        slot: Slot::File(at),
                        name: paste.name.clone(),
                        blob: paste.blob,
                        path: deliverable_path(&paste.path).unwrap_or_else(|| {
                            format!(".orch/attachments/{}", safe_name(&paste.name))
                        }),
                    });
                }
            }
            ContentPart::Image {
                blob, name, path, ..
            } => {
                // Keep the existing positions of forwarded files while direct images use their promised names.
                let legacy = next_numbered(name);
                planned.push(Planned {
                    slot: Slot::Image(at),
                    name: name.clone(),
                    blob: *blob,
                    path: path.as_deref().and_then(deliverable_path).unwrap_or(legacy),
                });
            }
            ContentPart::File {
                path,
                name,
                blob: Some(blob),
                ..
            } => {
                let given = deliverable_path(path);
                planned.push(Planned {
                    slot: Slot::File(at),
                    name: name.clone(),
                    blob: *blob,
                    path: given.unwrap_or_else(|| next_numbered(name)),
                });
            }
            ContentPart::File { blob: None, .. } => {}
            ContentPart::Forwarded(forwarded) => {
                for (n, image) in forwarded.images.iter().enumerate() {
                    planned.push(Planned {
                        slot: Slot::ForwardedImage(at, n),
                        name: image.name.clone(),
                        blob: image.blob,
                        path: next_numbered(&image.name),
                    });
                }
                for (n, attachment) in forwarded.attachments.iter().enumerate() {
                    planned.push(Planned {
                        slot: Slot::ForwardedFile(at, n),
                        name: attachment.name.clone(),
                        blob: attachment.blob,
                        path: next_numbered(&attachment.name),
                    });
                }
            }
        }
    }
    planned
}

/// What the owner of a machine, or a failure, says of why files were not written, in words the
/// agent can pass on to a person.
#[must_use]
pub fn reason_of(error: &CommandError) -> String {
    reason_of_code(&error.code, &error.detail)
}

/// The same, from the code and the sentence a failure carries.
#[must_use]
pub fn reason_of_code(code: &str, detail: &str) -> String {
    match code {
        "policy.deliver_files_disabled" => {
            "the owner of this machine does not accept delivered files".to_owned()
        }
        "policy.root_not_allowed" => "the machine does not offer this directory".to_owned(),
        "unsupported" => "the machine cannot write files here".to_owned(),
        "too_large" => "a file is larger than the machine takes".to_owned(),
        "expired" => "the machine did not take the files up".to_owned(),
        _ if detail.is_empty() => format!("the machine failed: {code}"),
        _ => format!("the machine failed: {detail}"),
    }
}

// ---------------------------------------------------------------- writing them

/// What is delivered, and where.
pub struct Delivering<'a> {
    /// The thread.
    pub thread: Id,
    /// The input the files belong to.
    pub input: Id,
    /// What the input carries.
    pub content: &'a [ContentPart],
    /// The environment the thread runs in.
    pub environment: &'a EnvironmentRow,
    /// The thread's directory, absolute.
    pub directory: &'a str,
    /// Who sent the input.
    pub sender: Option<Id>,
}

impl Engine {
    /// Writes the files of the input a starting turn answers, if it carries any. `None` when the
    /// process is stopping: the next one finds the same work.
    pub(crate) async fn deliver_input_files(
        &self,
        snap: &StartSnapshot,
    ) -> Result<Option<Placement>, EngineError> {
        // What was handed to the thread since its last turn goes first: this turn tells the agent of it.
        if self.deliver_shared(snap).await?.is_none() {
            return Ok(None);
        }
        let content: Vec<ContentPart> = serde_json::from_value(snap.input.content.clone())?;
        let planned = plan(snap.input.id, &content);
        if planned.is_empty() {
            return Ok(Some(Placement::default()));
        }
        // Written before the turn was made (see `preparation`): each file is where the plan puts it.
        if snap.input.state_detail.as_deref() == Some(crate::preparation::FILES_READY) {
            let placement = Placement::written(&planned);
            return Ok(Some(match self.blobs() {
                Some(store) => placement.with_sizes(&planned, store.as_ref()).await?,
                None => placement,
            }));
        }
        self.set_step(snap.thread.node_id, FILES_STEP).await?;
        let directory = snap.directory();
        self.deliver_files(&Delivering {
            thread: snap.thread.node_id,
            input: snap.input.id,
            content: &content,
            environment: &snap.environment,
            directory: &directory,
            sender: snap.input.author_id,
        })
        .await
    }

    /// Writes the files of an input into the thread's directory and says where each went or why it
    /// did not. The turn is never held back by a refusal; only a process that is stopping returns
    /// `None`.
    pub async fn deliver_files(
        &self,
        delivering: &Delivering<'_>,
    ) -> Result<Option<Placement>, EngineError> {
        let planned = plan(delivering.input, delivering.content);
        if planned.is_empty() {
            return Ok(Some(Placement::default()));
        }
        let Some(store) = self.blobs() else {
            return Ok(Some(Placement::refused(
                &planned,
                "this system has no store for files",
            )));
        };
        let Some(machine) = delivering.environment.machine_id else {
            return Ok(Some(Placement::refused(
                &planned,
                "the environment is not on a machine",
            )));
        };
        // What the store holds: a file whose content is gone cannot be written, and the rest can.
        let mut placement = Placement::default();
        let mut sizes: BTreeMap<Slot, i64> = BTreeMap::new();
        for file in &planned {
            match store.size(&file.blob).await {
                Ok(Some(size)) => {
                    sizes.insert(file.slot, size);
                    placement.sizes.insert(file.slot, size);
                    placement.written.insert(file.slot, file.path.clone());
                }
                Ok(None) => placement.refuse(file.slot, "its content is no longer kept"),
                Err(error) => {
                    placement.refuse(file.slot, &format!("its content cannot be read: {error}"));
                }
            }
        }
        let writable: Vec<&Planned> = planned
            .iter()
            .filter(|file| sizes.contains_key(&file.slot))
            .collect();
        if writable.is_empty() {
            return Ok(Some(placement));
        }
        let now = self.inner.clock.now();
        // The list of what to write, relative to the directory the machine is told to fill.
        let target_root = format!("{}/{ORCH_DIR}", delivering.directory.trim_end_matches('/'));
        let entries: Vec<Entry> = writable
            .iter()
            .map(|file| Entry {
                path: file
                    .path
                    .strip_prefix(&format!("{ORCH_DIR}/"))
                    .unwrap_or(&file.path)
                    .to_owned(),
                kind: EntryKind::File,
                size: sizes.get(&file.slot).copied().unwrap_or(0),
                mtime_ns: now.timestamp_nanos_opt().unwrap_or(0),
                mode: FILE_MODE,
                hash: Some(file.blob),
                link: None,
            })
            .collect();
        let written = build(
            &Header {
                v: MANIFEST_VERSION,
                root: target_root.clone(),
                generated_at: Time::new(now),
                generation: i64::from(delivering.environment.generation),
                case_sensitive: true,
            },
            entries,
        )
        .map_err(EngineError::unreadable)?;
        {
            let mut conn = self.inner.db.acquire().await?;
            self.put_manifest(&mut conn, written.bytes.clone()).await?;
        }
        let command_id = ids::derived(
            "fs.materialize",
            &[
                delivering.input.as_bytes().as_slice(),
                machine.as_bytes().as_slice(),
                written.hash.as_bytes().as_slice(),
            ],
        );
        let visible = written.total_bytes >= VISIBLE_DELIVERY_BYTES
            || writable.len() >= VISIBLE_DELIVERY_FILES;
        let delivered = if visible {
            let job = self
                .begin_delivery_job(
                    delivering,
                    &Writing {
                        command: command_id,
                        machine,
                        manifest: written.hash,
                        target_root,
                        files: written.file_count,
                        bytes: written.total_bytes,
                    },
                    now,
                )
                .await?;
            self.await_delivery_job(job).await?
        } else {
            self.write_inline(delivering, machine, command_id, &written.hash, &target_root)
                .await?
        };
        match delivered {
            None => Ok(None),
            Some(Delivered::Written) => Ok(Some(placement)),
            Some(Delivered::Refused(why)) => {
                for file in &writable {
                    placement.refuse(file.slot, &why);
                }
                Ok(Some(placement))
            }
        }
    }

    /// Writes the files of a small delivery with one command and waits for it here: nobody would see a
    /// bar for it. `None` when the process is stopping.
    async fn write_inline(
        &self,
        delivering: &Delivering<'_>,
        machine: Id,
        command_id: Id,
        manifest: &Hash,
        target_root: &str,
    ) -> Result<Option<Delivered>, EngineError> {
        let command = Outgoing::new(
            command_id,
            machine,
            SerialKey::Environment(delivering.environment.id),
            &FsMaterialize {
                work: None,
                environment_id: delivering.environment.id,
                job_id: None,
                manifest_blob: *manifest,
                target_root: target_root.to_owned(),
                purpose: MaterializePurpose::Delivery,
            },
        )?;
        self.inner.commands.issue(command).await?;
        let outcome = loop {
            match tokio::time::timeout(RECHECK_EVERY, self.inner.commands.wait(command_id)).await {
                Ok(outcome) => break outcome?,
                Err(_elapsed) => {
                    if self.is_stopped() {
                        return Ok(None);
                    }
                }
            }
        };
        Ok(Some(match outcome {
            Outcome::Succeeded(_) => Delivered::Written,
            Outcome::Failed(error) => Delivered::Refused(reason_of(&error)),
            Outcome::Expired | Outcome::Cancelled => {
                Delivered::Refused("the machine did not take the files up".to_owned())
            }
        }))
    }

    /// Makes (or finds) the job that carries a large delivery. The job has an identifier made of the
    /// command it sends, so that a turn that is started again after a restart finds the same job.
    async fn begin_delivery_job(
        &self,
        delivering: &Delivering<'_>,
        writing: &Writing,
        now: DateTime<Utc>,
    ) -> Result<Id, EngineError> {
        let id = ids::derived("delivery.job", &[writing.command.as_bytes().as_slice()]);
        let mut tx = self.inner.db.begin().await?;
        if rows::job(&mut tx, id).await?.is_none() {
            let mut progress = initial_progress(&PHASES);
            progress.files_total = Some(writing.files);
            progress.bytes_total = Some(writing.bytes);
            rows::insert(
                &mut tx,
                &NewJob {
                    id,
                    kind: kind::DELIVERY,
                    node_id: Some(delivering.thread),
                    environment_id: Some(delivering.environment.id),
                    provider_id: None,
                    plan: &json!({
                        "input_id": delivering.input,
                        "machine_id": writing.machine,
                        "command_id": writing.command,
                        "manifest_blob": writing.manifest,
                        "target_root": writing.target_root,
                    }),
                    progress: &serde_json::to_value(&progress)?,
                    created_by: delivering.sender,
                },
                now,
            )
            .await?;
        }
        tx.commit().await.map_err(orch_db::DbError::from)?;
        // The runner looks at once and not at the end of its interval.
        self.kick();
        Ok(id)
    }

    /// Waits until the job of a delivery is over and says how it ended. `None` when the process is
    /// stopping: the next one finds the same job and waits for it.
    async fn await_delivery_job(&self, job: Id) -> Result<Option<Delivered>, EngineError> {
        loop {
            let row = {
                let mut conn = self.inner.db.acquire().await?;
                rows::job(&mut conn, job).await?
            };
            let Some(row) = row else {
                return Ok(Some(Delivered::Refused(
                    "the delivery of the files is gone".to_owned(),
                )));
            };
            match row.state.as_str() {
                state::SUCCEEDED => return Ok(Some(Delivered::Written)),
                state::FAILED => {
                    let why = row
                        .error
                        .as_ref()
                        .and_then(|error| error.get("message"))
                        .and_then(Json::as_str)
                        .unwrap_or("the files could not be written")
                        .to_owned();
                    return Ok(Some(Delivered::Refused(why)));
                }
                state::CANCELLED => {
                    return Ok(Some(Delivered::Refused(
                        "the delivery of the files was cancelled".to_owned(),
                    )));
                }
                _ => {}
            }
            if self.is_stopped() {
                return Ok(None);
            }
            tokio::time::sleep(WATCH_EVERY).await;
        }
    }
}

/// How a delivery ended.
enum Delivered {
    /// The machine wrote the files.
    Written,
    /// It did not, for the reason said in words a person can read.
    Refused(String),
}

/// What a delivery that is a job is made of: the command, the machine it goes to, the list of what
/// to write, and where.
struct Writing {
    command: Id,
    machine: Id,
    manifest: Hash,
    target_root: String,
    files: i64,
    bytes: i64,
}

// ---------------------------------------------------------------- the job

/// The phases of writing a large delivery. The machine writes everything with the one command that
/// the first phase sends and reports how far it has come; the others show that.
#[derive(Debug, Clone)]
pub struct Delivery {
    command: Id,
    machine: Id,
    environment: Id,
    manifest: Hash,
    target_root: String,
}

impl Delivery {
    /// The work of a job row of the kind `delivery`.
    pub fn of(row: &JobRow) -> Result<Self, EngineError> {
        let id = |member: &str| {
            row.plan
                .get(member)
                .and_then(Json::as_str)
                .and_then(|text| text.parse::<Id>().ok())
                .ok_or_else(|| {
                    EngineError::unreadable(format!("the plan of a delivery lacks {member}"))
                })
        };
        let manifest = row
            .plan
            .get("manifest_blob")
            .and_then(Json::as_str)
            .and_then(|text| text.parse::<Hash>().ok())
            .ok_or_else(|| EngineError::unreadable("the plan of a delivery lacks manifest_blob"))?;
        let target_root = row
            .plan
            .get("target_root")
            .and_then(Json::as_str)
            .map(str::to_owned)
            .ok_or_else(|| EngineError::unreadable("the plan of a delivery lacks target_root"))?;
        Ok(Self {
            command: id("command_id")?,
            machine: id("machine_id")?,
            environment: row
                .environment_id
                .ok_or_else(|| EngineError::unreadable("a delivery is about no environment"))?,
            manifest,
            target_root,
        })
    }
}

#[async_trait]
impl Phased for Delivery {
    async fn run(&self, job: &Job, phase: &str) -> Result<Done, Stop> {
        if phase != DIRECTORIES.name {
            return Ok(Done::Finished);
        }
        let command = Outgoing::new(
            self.command,
            self.machine,
            SerialKey::Environment(self.environment),
            &FsMaterialize {
                work: None,
                environment_id: self.environment,
                job_id: Some(job.id()),
                manifest_blob: self.manifest,
                target_root: self.target_root.clone(),
                purpose: MaterializePurpose::Delivery,
            },
        )
        .map_err(|error| Stop::Failed(Failure::new(INTERNAL, error.to_string())))?;
        match job.command(command).await {
            Ok(written) => {
                job.remember(&json!({ "written": written })).await?;
                Ok(Done::Finished)
            }
            // What the machine said, in words the agent can pass on to a person.
            Err(Stop::Failed(failure)) if failure.code != INTERNAL => {
                let why = reason_of_code(&failure.code, &failure.message);
                Err(Stop::Failed(Failure::new(failure.code, why)))
            }
            Err(other) => Err(other),
        }
    }

    async fn undo(&self, _job: &Job, _why: &Stop) -> Undone {
        // What the machine wrote stays where it was written; nothing here takes it back.
        Undone::Partly("the files that were written stay in the workspace".to_owned())
    }

    async fn result(&self, job: &Job) -> Option<Json> {
        job.plan()
            .await
            .ok()
            .and_then(|plan| plan.get("written").cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_api_contract::threads::{
        Forwarded, ForwardedAttachment, ForwardedFrom, ForwardedImage, ImageMediaType,
    };
    use orch_types::new_id;

    fn file(path: &str, name: &str, blob: Option<Hash>) -> ContentPart {
        ContentPart::File {
            path: path.to_owned(),
            name: name.to_owned(),
            size: 4,
            blob,
        }
    }

    #[test]
    fn a_file_attached_to_the_message_is_written_where_it_was_told_it_would_be() {
        let input = new_id();
        let blob = Hash::of(b"notes");
        let planned = plan(
            input,
            &[
                ContentPart::Text { text: "see".into() },
                file(".orch/uploads/0199-notes.txt", "notes.txt", Some(blob)),
            ],
        );
        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].slot, Slot::File(1));
        assert_eq!(planned[0].path, ".orch/uploads/0199-notes.txt");
        assert_eq!(planned[0].blob, blob);
    }

    #[test]
    fn a_file_that_is_in_the_workspace_already_is_not_written() {
        let planned = plan(new_id(), &[file("src/main.rs", "main.rs", None)]);
        assert!(planned.is_empty());
    }

    #[test]
    fn a_path_outside_the_orch_directory_is_not_trusted_with_a_place() {
        let input = new_id();
        let blob = Hash::of(b"x");
        for bad in [
            "src/main.rs",
            "/etc/passwd",
            ".orch/../src/x",
            ".orch//x",
            ".orch/./x",
            ".orch",
            ".orch/",
            "a/.orch/x",
            ".orch\\x",
            ".orchestra/x",
        ] {
            let planned = plan(input, &[file(bad, "x.txt", Some(blob))]);
            assert_eq!(
                planned[0].path,
                format!(".orch/uploads/{input}/1-x.txt"),
                "{bad}"
            );
        }
    }

    #[test]
    fn images_and_forwarded_files_are_numbered_in_the_order_of_the_input() {
        let input = new_id();
        let forwarded = ContentPart::Forwarded(Box::new(Forwarded {
            r#type: orch_api_contract::threads::ForwardedType::Forwarded,
            from: serde_json::from_value::<ForwardedFrom>(serde_json::json!({
                "kind": "thread",
                "thread": { "id": new_id(), "kind": "thread", "name": "Team", "redacted": false }
            }))
            .unwrap_or_else(|_| panic!("a source")),
            source_id: new_id(),
            author: None,
            at: orch_types::Time::new(chrono::Utc::now()),
            text: "look".into(),
            images: vec![ForwardedImage {
                blob: Hash::of(b"i"),
                media_type: "image/png".into(),
                name: "shot.png".into(),
            }],
            attachments: vec![ForwardedAttachment {
                blob: Hash::of(b"a"),
                name: "../../etc/shadow".into(),
                size: 1,
                media_type: "text/plain".into(),
            }],
        }));
        let planned = plan(
            input,
            &[
                ContentPart::Image {
                    blob: Hash::of(b"direct"),
                    media_type: ImageMediaType::Png,
                    name: "direct.png".into(),
                    path: Some(".orch/attachments/direct.png".to_owned()),
                },
                forwarded,
            ],
        );
        let paths: Vec<_> = planned.iter().map(|each| each.path.clone()).collect();
        assert_eq!(
            paths,
            [
                ".orch/attachments/direct.png".to_owned(),
                format!(".orch/uploads/{input}/2-shot.png"),
                format!(".orch/uploads/{input}/3-_.._etc_shadow"),
            ],
            "a name never climbs out of the directory"
        );
        assert_eq!(
            planned.iter().map(|each| each.slot).collect::<Vec<_>>(),
            [
                Slot::Image(0),
                Slot::ForwardedImage(1, 0),
                Slot::ForwardedFile(1, 0)
            ]
        );
    }

    #[test]
    fn a_legacy_image_keeps_the_path_its_prepared_input_used() {
        let input = new_id();
        let blob = Hash::of(b"image");
        let planned = plan(
            input,
            &[ContentPart::Image {
                blob,
                media_type: ImageMediaType::Png,
                name: "shot.png".to_owned(),
                path: None,
            }],
        );
        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].path, format!(".orch/uploads/{input}/1-shot.png"));
        assert_eq!(planned[0].blob, blob);
    }

    #[test]
    fn a_name_is_made_one_safe_component() {
        assert_eq!(safe_name("a/b\\c.txt"), "a_b_c.txt");
        assert_eq!(safe_name("..hidden"), "hidden");
        assert_eq!(safe_name("  "), "file");
        assert_eq!(safe_name("a\nb"), "a_b");
        assert_eq!(safe_name(&"é".repeat(300)).chars().count(), NAME_MAX_CHARS);
    }

    #[test]
    fn a_refusal_of_a_machine_is_told_in_words_a_person_can_read() {
        let off = CommandError::new(
            orch_link_contract::commands::ErrorCode::PolicyDeliverFilesDisabled,
            "x",
        );
        assert_eq!(
            reason_of(&off),
            "the owner of this machine does not accept delivered files"
        );
        let other = CommandError::new(
            orch_link_contract::commands::ErrorCode::Internal,
            "disk full",
        );
        assert_eq!(reason_of(&other), "the machine failed: disk full");
    }

    #[test]
    fn what_became_of_the_files_is_told_by_slot() {
        let input = new_id();
        let planned = plan(
            input,
            &[
                file(".orch/uploads/a-1.txt", "1.txt", Some(Hash::of(b"1"))),
                file(".orch/uploads/a-2.txt", "2.txt", Some(Hash::of(b"2"))),
            ],
        );
        let mut placement = Placement::written(&planned);
        placement.refuse(Slot::File(1), "gone");
        assert_eq!(
            placement.path_of(Slot::File(0)),
            Some(".orch/uploads/a-1.txt")
        );
        assert_eq!(placement.path_of(Slot::File(1)), None);
        assert_eq!(placement.refusal_of(Slot::File(1)), Some("gone"));
        assert!(Placement::default().is_empty());
        let refused = Placement::refused(&planned, "no");
        assert_eq!(refused.refusal_of(Slot::File(0)), Some("no"));
    }
}
