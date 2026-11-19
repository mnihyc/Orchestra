//! The job that brings an import: the source opened and every record and file of a package checked against the hashes of its manifest, the plan made as the person who asked, the objects made in batches that ask the access engine again for each kind they make, and a report with a row for each object.
//!
//! **As the person who asked, at the moment it is made.** The job holds what the request was made with (a browser session, or a
//! token with its scopes) and loads the person again for each batch of a hundred objects: what they lost the right to make since
//! they asked is a row of the report and nothing of it is made. **Nothing is half made.** An object is made in a savepoint of its
//! own, and what cannot be made is one failed row; what a batch made is committed with its rows, so that a job taken over by
//! another process goes on from the first row that is not written and makes nothing twice (every identifier is derived).
//! **Where it ends.** A job that failed or was cancelled leaves what it made, and its report says how far it came; the person
//! may take it back as a whole.

use crate::engine::Engine;
use crate::error::EngineError;
use crate::imports::make::{Batch, Made, PlanMaps, Staged, make_object};
use crate::imports::{
    self, Counts, ImportError, Plan, kind_word, outcome_word, row_of, stored_row,
};
use crate::jobs::runner::{Done, Failure, INTERNAL, Job, Phase, Phased, Stop, Undone};
use async_trait::async_trait;
use futures::StreamExt;
use orch_api_contract::imports::{
    ImportAction, ImportActionCounts, ImportCounts, ImportLoss, ImportOutcome, ImportRequest,
};
use orch_api_contract::package::PackageKind;
use orch_authz::{Caller, Principal, TokenGrant};
use orch_blob_contract::store::{BlobError, ByteStream};
use orch_db::queries::imports::{self as rows, ImportRow};
use orch_db::queries::jobs::{JobRow, kind};
use orch_db::queries::storage;
use orch_types::{Hash, Id, Json, Time};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio_util::io::ReaderStream;

/// What the job is called in the table of jobs.
pub const KIND: &str = kind::IMPORT;

/// The phases of the job that brings an import, with the weight of each: checking reads the whole source once, making is where
/// the time goes, and the report is a few statements.
pub const PHASES: [Phase; 4] = [
    Phase {
        name: "validate",
        weight: 20,
    },
    Phase {
        name: "map",
        weight: 5,
    },
    Phase {
        name: "create",
        weight: 70,
    },
    Phase {
        name: "report",
        weight: 5,
    },
];

/// The phases of the job that takes an import back.
pub const UNDO_PHASES: [Phase; 2] = [
    Phase {
        name: "remove",
        weight: 90,
    },
    Phase {
        name: "finish",
        weight: 10,
    },
];

/// How many objects are made in one transaction: the same hundred that an export asks the access engine for at once, so that
/// the decision is as fresh as the making and the statements are few.
const BATCH: usize = 100;

/// How many places the chats of are asked for in one statement.
const CHATS_PAGE: usize = 5_000;

/// How many entries are checked before the percentage is written down again.
const CHECK_PROGRESS_EVERY: usize = 25;

/// How often, at the most, a long record says how far it has come in the progress of the job: the job is looked at for a cancel at each page of
/// the record, and the words are written down once in this time.
const SAY_EVERY: std::time::Duration = std::time::Duration::from_secs(1);

fn plan_id(plan: &Json, member: &str) -> Option<Id> {
    plan.get(member)?.as_str()?.parse().ok()
}

fn failure(error: &ImportError) -> Failure {
    match error {
        ImportError::Blocked { code, message, .. } => Failure::new(*code, message.clone()),
        ImportError::Source(message) | ImportError::Undone(message) => {
            Failure::new("import.invalid", message.clone())
        }
        other => Failure::new(INTERNAL, other.to_string()),
    }
}

/// The phased work of one job that brings an import, or takes one back.
pub struct Importing {
    engine: Engine,
    import: Id,
    user: Id,
    node: Id,
    credential: Option<(Vec<String>, Option<Id>)>,
    undo: bool,
    /// The plan, once it is made: kept for the phases of this run.
    plan: tokio::sync::Mutex<Option<Arc<Plan>>>,
}

impl Importing {
    /// The work of the job `row`, from what its plan holds.
    pub fn of(engine: Engine, row: &JobRow) -> Result<Self, EngineError> {
        let missing = |member: &str| {
            EngineError::unreadable(format!("a job that brings an import lacks its {member}"))
        };
        let credential = row
            .plan
            .get("credential")
            .filter(|each| !each.is_null())
            .map(|each| {
                let scopes: Vec<String> = each
                    .get("scopes")
                    .and_then(Json::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|scope| scope.as_str().map(str::to_owned))
                    .collect();
                (scopes, plan_id(each, "subtree"))
            });
        Ok(Self {
            engine,
            import: plan_id(&row.plan, "import").ok_or_else(|| missing("import"))?,
            user: plan_id(&row.plan, "user").ok_or_else(|| missing("person"))?,
            node: plan_id(&row.plan, "node").ok_or_else(|| missing("place"))?,
            credential,
            undo: row
                .plan
                .get("undo")
                .and_then(Json::as_bool)
                .unwrap_or(false),
            plan: tokio::sync::Mutex::new(None),
        })
    }

    /// Whoever asked, reading with what they asked with.
    fn caller(&self) -> Result<Caller, Failure> {
        match &self.credential {
            None => Ok(Caller::session(self.user)),
            Some((scopes, subtree)) => TokenGrant::from_stored(scopes, *subtree)
                .map(|grant| Caller::token(self.user, grant))
                .map_err(|error| Failure::new(INTERNAL, error.to_string())),
        }
    }

    async fn principal(&self, conn: &mut orch_db::Conn) -> Result<Principal, Stop> {
        Principal::load(
            conn,
            self.caller()?,
            Time::new(self.engine.inner.clock.now()),
        )
        .await
        .map_err(|error| Stop::Failed(Failure::new(INTERNAL, error.to_string())))
    }

    /// The import as it was asked, and its row.
    async fn asked(&self, conn: &mut orch_db::Conn) -> Result<(ImportRow, ImportRequest), Stop> {
        let row = rows::get(conn, self.import).await?.ok_or(Stop::Cancelled)?;
        let request: ImportRequest = serde_json::from_value(row.request.clone())
            .map_err(|error| Failure::new(INTERNAL, error.to_string()))?;
        Ok((row, request))
    }

    fn scratch(&self) -> std::path::PathBuf {
        self.engine.scratch_dir()
    }

    /// The plan, made once for this run.
    async fn plan(&self) -> Result<Arc<Plan>, Stop> {
        let mut held = self.plan.lock().await;
        if let Some(plan) = held.as_ref() {
            return Ok(Arc::clone(plan));
        }
        let mut conn = self.engine.inner.db.acquire().await?;
        let (_, request) = self.asked(&mut conn).await?;
        let principal = self.principal(&mut conn).await?;
        let plan = self
            .engine
            .plan_of_import(&mut conn, &principal, self.node, &request)
            .await
            .map_err(|error| failure(&error))?;
        let plan = Arc::new(plan);
        *held = Some(Arc::clone(&plan));
        Ok(plan)
    }
}

#[async_trait]
impl Phased for Importing {
    async fn run(&self, job: &Job, phase: &str) -> Result<Done, Stop> {
        match phase {
            "validate" => self.validate(job).await,
            "map" => self.map(job).await,
            "create" => self.create(job).await,
            "report" => self.report(job, rows::state::DONE, None).await,
            "remove" => self.remove(job).await,
            "finish" => self.finish_undo(job).await,
            other => Err(Failure::new(
                INTERNAL,
                format!("a job that brings an import has no phase {other}"),
            )
            .into()),
        }
    }

    async fn undo(&self, job: &Job, why: &Stop) -> Undone {
        // What was made stays: the import ends as what it came to, and the report says how far.
        let (state, failure) = match why {
            Stop::Cancelled => (rows::state::CANCELLED, None),
            Stop::Failed(failure) => (
                rows::state::FAILED,
                Some(json!({ "code": failure.code, "message": failure.message })),
            ),
            Stop::Lost => return Undone::Deferred("the job was taken over".to_owned()),
        };
        if self.undo {
            return match self.engine.inner.db.acquire().await {
                Ok(mut conn) => {
                    let now = self.engine.inner.clock.now();
                    match rows::end_undo(&mut conn, self.import, false, now).await {
                        Ok(()) => Undone::Everything,
                        Err(_) => Undone::Partly("the import was not marked".to_owned()),
                    }
                }
                Err(_) => Undone::Partly("the import was not marked".to_owned()),
            };
        }
        match self.report(job, state, failure).await {
            Ok(_) => Undone::Everything,
            Err(_) => Undone::Partly("the report of the import was not closed".to_owned()),
        }
    }

    async fn result(&self, _job: &Job) -> Option<Json> {
        Some(json!({ "import_id": self.import }))
    }
}

// ---------------------------------------------------------------- validate

impl Importing {
    /// Opens the source and checks every path, size and hash of a package against its manifest.
    async fn validate(&self, job: &Job) -> Result<Done, Stop> {
        if self.undo {
            return Ok(Done::Skipped("nothing to check".to_owned()));
        }
        let mut conn = self.engine.inner.db.acquire().await?;
        let (_, request) = self.asked(&mut conn).await?;
        let limits = self
            .engine
            .import_limits(&mut conn, self.node)
            .await
            .map_err(|error| failure(&error))?;
        drop(conn);
        let opened = imports::open(&self.engine, &request.source, &limits)
            .await
            .map_err(|error| failure(&ImportError::from(error)))?;
        if opened.mode == orch_api_contract::imports::ImportSourceMode::Package {
            let most_file =
                u64::try_from(self.engine.inner.config.export_file_max_bytes).unwrap_or(u64::MAX);
            // A record is hashed as it is read through, never held, so it may be as large as the import may be: a thread of a hundred
            // million tokens has a record of gigabytes.
            let most_record = u64::try_from(limits.max_bytes).unwrap_or(u64::MAX);
            let mut checks: Vec<(&str, &str, u64)> = Vec::new();
            for object in &opened.objects {
                if let (Some(path), Some(sha)) = (&object.path, &object.sha256) {
                    checks.push((path, sha, most_record));
                }
                for rendering in &object.renderings {
                    checks.push((&rendering.path, &rendering.sha256, most_record));
                }
                for file in &object.files {
                    checks.push((&file.path, &file.sha256, most_file));
                }
                for version in &object.versions {
                    checks.push((&version.path, &version.sha256, most_record));
                }
            }
            let total = checks.len().max(1);
            for (at, (path, sha, limit)) in checks.iter().enumerate() {
                job.check()?;
                let found = opened
                    .entries
                    .sha256(path, *limit)
                    .await
                    .map_err(|error| Failure::new("import.damaged", error.to_string()))?;
                if found != *sha {
                    return Err(Failure::new(
                        "import.damaged",
                        format!("{path} does not match the hash the manifest gives it"),
                    )
                    .into());
                }
                if at % CHECK_PROGRESS_EVERY == 0 {
                    job.progress(
                        at as f64 / total as f64,
                        Some(&format!("{at} of {total} entries checked")),
                    )
                    .await?;
                }
            }
        }
        Ok(Done::Finished)
    }

    /// Makes the plan as the person who asked, and refuses what no longer fits.
    async fn map(&self, job: &Job) -> Result<Done, Stop> {
        if self.undo {
            return Ok(Done::Skipped("nothing to match".to_owned()));
        }
        let plan = self.plan().await?;
        if let Some((code, _, _, message)) = Engine::import_blockers(&plan).into_iter().next() {
            return Err(Failure::new(code, message).into());
        }
        let (items, bytes) = (plan.items, plan.bytes);
        job.amend(move |progress, _| {
            progress.files_total = Some(items);
            progress.bytes_total = Some(bytes);
        })
        .await?;
        Ok(Done::Finished)
    }
}

// ---------------------------------------------------------------- create

/// What the job remembers of how far it has come.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct Progress {
    counts: ImportCounts,
    actions: ImportActionCounts,
    failed: i64,
    carried_warnings: i64,
}

impl Progress {
    fn counts(&self) -> Counts {
        Counts {
            counts: self.counts,
            actions: self.actions,
            failed: self.failed,
        }
    }
}

impl Importing {
    /// Puts the files of the documents of a batch in the store, outside the transaction that will hold them.
    async fn stage(
        &self,
        plan: &Plan,
        chunk: &[&imports::Object],
        staged: &mut HashMap<String, Staged>,
    ) -> Result<(), Stop> {
        let Some(store) = self.engine.blobs() else {
            return Err(Failure::new(
                "unsupported",
                "no store of content is attached, so files cannot be brought",
            )
            .into());
        };
        let most =
            u64::try_from(self.engine.inner.config.export_file_max_bytes).unwrap_or(u64::MAX);
        for object in chunk.iter().filter(|each| each.goes()) {
            // What a package says a thing carries, and what a text of plain files names beside itself in the source.
            let mut wanted: Vec<(String, String)> = object
                .object
                .files
                .iter()
                .map(|file| (file.path.clone(), file.sha256.clone()))
                .collect();
            if let Some(plain) = plan.opened.plain.get(&object.object.id) {
                for named in imports::plain::named(&plan.opened, plain).await {
                    if named.document.is_none() {
                        wanted.push((named.entry, String::new()));
                    }
                }
            }
            for (path, sha256) in &wanted {
                if staged.contains_key(path) {
                    continue;
                }
                let spooled = match plan.opened.entries.spool(path, most, &self.scratch()).await {
                    Ok(spooled) => spooled,
                    Err(error) => {
                        tracing::warn!(%error, %path, "a file of an import could not be read");
                        continue;
                    }
                };
                if !sha256.is_empty() && spooled.sha256 != *sha256 {
                    let _ = std::fs::remove_file(&spooled.path);
                    continue;
                }
                let size = i64::try_from(spooled.size).unwrap_or(i64::MAX);
                let opened = tokio::fs::File::open(&spooled.path)
                    .await
                    .map_err(|error| Failure::new(INTERNAL, error.to_string()))?;
                let content: ByteStream = ReaderStream::with_capacity(opened, 256 * 1024)
                    .map(|piece| piece.map_err(|error| BlobError::Io(error.to_string())))
                    .boxed();
                let put = store.put(&spooled.blob, Some(size), content).await;
                let _ = std::fs::remove_file(&spooled.path);
                put.map_err(|error| Failure::new(INTERNAL, error.to_string()))?;
                let mut conn = self.engine.inner.db.acquire().await?;
                storage::register_blob(
                    &mut conn,
                    &spooled.blob,
                    size,
                    self.engine.inner.clock.now(),
                )
                .await?;
                staged.insert(
                    path.clone(),
                    Staged {
                        blob: spooled.blob,
                        size,
                    },
                );
            }
        }
        Ok(())
    }

    /// Makes the objects of the plan, a batch at a time.
    #[allow(
        clippy::too_many_lines,
        reason = "one loop in the order a batch is made: stage the files, make each object in its savepoint, write the rows, commit"
    )]
    async fn create(&self, job: &Job) -> Result<Done, Stop> {
        if self.undo {
            return Ok(Done::Skipped("nothing to make".to_owned()));
        }
        let plan = self.plan().await?;
        let remembered: Progress = job
            .plan()
            .await
            .ok()
            .and_then(|plan| plan.get("progress").cloned())
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default();
        let mut progress = remembered;
        let first = {
            let mut conn = self.engine.inner.db.acquire().await?;
            usize::try_from(rows::last_position(&mut conn, self.import).await? + 1).unwrap_or(0)
        };
        let all: Vec<&imports::Object> = plan.objects.iter().collect();
        let total = all.len().max(1);
        // The maps of the plan are made once and kept: a batch borrows them, and only the chats of the places made so far are added between
        // batches (a map is never cloned for a batch, so the work of a batch does not grow with the size of the import).
        let mut known = PlanMaps::of(&plan, self.import);
        let mut chats_in: HashMap<Id, Vec<Id>> = HashMap::new();
        for each in plan
            .objects
            .iter()
            .filter(|each| each.made_as == PackageKind::Chat && each.goes())
        {
            chats_in
                .entry(each.lies_in)
                .or_default()
                .push(each.object.id);
        }
        {
            // The chats of the places that are there already, asked in pages of identifiers.
            let nodes: Vec<Id> = chats_in.keys().copied().collect();
            let mut conn = self.engine.inner.db.acquire().await?;
            for page in nodes.chunks(CHATS_PAGE) {
                let found: Vec<(Id, Id)> =
                    sqlx::query_as("SELECT node_id, id FROM chats WHERE node_id = ANY($1)")
                        .bind(page)
                        .fetch_all(&mut *conn)
                        .await
                        .map_err(orch_db::DbError::from)?;
                for (node, chat) in found {
                    for source in chats_in.get(&node).into_iter().flatten() {
                        known.chats.insert(*source, chat);
                    }
                }
            }
        }
        let mut staged: HashMap<String, Staged> = HashMap::new();
        let mut done = first.min(all.len());
        // The objects of the source that were not made because they failed or because what they lie in was not made: what lies
        // in them is left, with that said, and is not tried.
        let mut lost: HashSet<Id> = HashSet::new();
        for chunk in all[first.min(all.len())..].chunks(BATCH) {
            job.check()?;
            // The files of the batch are staged outside its transaction, and let go of when it ends.
            staged.clear();
            self.stage(&plan, chunk, &mut staged).await?;
            let now = self.engine.inner.clock.now();
            let mut tx = self.engine.inner.db.begin().await?;
            let principal = self.principal(&mut tx).await?;
            let (notes, mut said) = tokio::sync::mpsc::unbounded_channel::<String>();
            let batch = Batch {
                engine: &self.engine,
                plan: &plan,
                principal: &principal,
                import: self.import,
                now,
                staged: &staged,
                maps: &known,
                notes: Some(notes),
            };
            let mut last_said = std::time::Instant::now();
            let mut written = Vec::with_capacity(chunk.len());
            for (offset, object) in chunk.iter().enumerate() {
                let position = i64::try_from(done + offset).unwrap_or(i64::MAX);
                let parent_lost = object
                    .object
                    .parent
                    .is_some_and(|parent| lost.contains(&parent));
                let made = if let Some(block) = &object.blocked {
                    Made::failed(&block.code, block.message.clone())
                } else if parent_lost {
                    let mut left = Made::left(
                        ImportOutcome::Skipped,
                        Some("what it lies in could not be made, and so it was not".to_owned()),
                    );
                    left.code = Some("import.parent_failed".to_owned());
                    left
                } else if object.skipped_above {
                    Made::left(
                        ImportOutcome::Skipped,
                        Some("the place it lies in is left alone, and so is it".to_owned()),
                    )
                } else if object.action == ImportAction::Skip {
                    Made::left(ImportOutcome::Skipped, None)
                } else {
                    // A record of a thread or a chat may take minutes: while it is read a page at a time the job is looked at for a cancel,
                    // and says how far the record has come. A cancel drops the work where it stands, and its transaction with it.
                    let work = make_object(&mut tx, &batch, object);
                    tokio::pin!(work);
                    let result = loop {
                        tokio::select! {
                            result = &mut work => break result,
                            words = said.recv() => {
                                job.check()?;
                                if let Some(words) = words
                                    && last_said.elapsed() >= SAY_EVERY
                                {
                                    last_said = std::time::Instant::now();
                                    let at = done + offset;
                                    job.progress(
                                        at as f64 / total as f64,
                                        Some(&format!("{at} of {total} things; {words}")),
                                    )
                                    .await?;
                                }
                            }
                        }
                    };
                    result.map_err(|error| match error {
                        imports::make::MakeError::Internal(error) => failure(&error),
                        other => Failure::new(INTERNAL, other.to_string()),
                    })?
                };
                if made.outcome == ImportOutcome::Failed || parent_lost {
                    lost.insert(object.object.id);
                }
                progress.counts_note(object, &made);
                let row = row_of(position, object, &made);
                written.push(stored_row(&row));
            }
            rows::add_report_rows(&mut tx, self.import, &written).await?;
            tx.commit().await.map_err(orch_db::DbError::from)?;
            // The chats of the places this batch made, for the batches that follow.
            let made_nodes: Vec<Id> = chunk
                .iter()
                .filter(|each| {
                    each.goes()
                        && each.action != ImportAction::Replace
                        && chats_in.contains_key(&each.target)
                })
                .map(|each| each.target)
                .collect();
            if !made_nodes.is_empty() {
                let mut conn = self.engine.inner.db.acquire().await?;
                for page in made_nodes.chunks(CHATS_PAGE) {
                    let found: Vec<(Id, Id)> =
                        sqlx::query_as("SELECT node_id, id FROM chats WHERE node_id = ANY($1)")
                            .bind(page)
                            .fetch_all(&mut *conn)
                            .await
                            .map_err(orch_db::DbError::from)?;
                    for (node, chat) in found {
                        for source in chats_in.get(&node).into_iter().flatten() {
                            known.chats.insert(*source, chat);
                        }
                    }
                }
            }
            done += chunk.len();
            let _ = job.remember(&json!({ "progress": progress })).await;
            job.progress(
                done as f64 / total as f64,
                Some(&format!("{done} of {total} things")),
            )
            .await?;
            job.amend(move |step, _| {
                step.files_done = i64::try_from(done).unwrap_or(i64::MAX);
            })
            .await?;
        }
        Ok(Done::Finished)
    }
}

impl Progress {
    /// Takes one object's outcome into the totals.
    fn counts_note(&mut self, object: &imports::Object, made: &Made) {
        let mut counts = self.counts();
        counts.note(object, made.outcome);
        counts.add(
            made.carried.files,
            made.carried.messages,
            made.carried.turns,
            made.carried.comments,
            made.carried.versions,
        );
        self.counts = counts.counts;
        self.actions = counts.actions;
        self.failed = counts.failed;
        self.carried_warnings += i64::try_from(made.warnings.len()).unwrap_or(0);
    }
}

// ---------------------------------------------------------------- report

impl Importing {
    /// Closes the import: what it came to, what did not come back, and the uploads let go.
    async fn report(&self, job: &Job, state: &str, failure: Option<Json>) -> Result<Done, Stop> {
        let mut conn = self.engine.inner.db.acquire().await?;
        let Some(row) = rows::get(&mut conn, self.import).await? else {
            return Ok(Done::Finished);
        };
        if row.state != rows::state::PENDING {
            return Ok(Done::Finished);
        }
        let remembered: Progress = job
            .plan()
            .await
            .ok()
            .and_then(|plan| plan.get("progress").cloned())
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default();
        let plan = self.plan.lock().await.clone();
        let mut losses: Vec<ImportLoss> = plan
            .as_ref()
            .map(|plan| plan.losses.clone())
            .unwrap_or_default();
        // What came back changed, by kind of change, from the rows themselves.
        let (by_outcome, warned) = rows::report_counts(&mut conn, self.import).await?;
        let _ = by_outcome;
        if warned > 0 {
            losses.push(ImportLoss {
                code: "warnings".to_owned(),
                count: warned,
                message: format!("{warned} things came back changed: the report says how"),
                object: None,
                params: None,
            });
        }
        let now = self.engine.inner.clock.now();
        let counts = serde_json::to_value(remembered.counts)
            .map_err(|error| Failure::new(INTERNAL, error.to_string()))?;
        let actions = serde_json::to_value(remembered.actions)
            .map_err(|error| Failure::new(INTERNAL, error.to_string()))?;
        let losses_json = serde_json::to_value(&losses)
            .map_err(|error| Failure::new(INTERNAL, error.to_string()))?;
        let package = row.package.clone();
        let mut tx = self.engine.inner.db.begin().await?;
        let blobs: Vec<Hash> = row
            .source_blobs
            .iter()
            .filter_map(|text| text.parse::<Hash>().ok())
            .collect();
        storage::unpin_blobs(&mut tx, &blobs, now).await?;
        rows::end(
            &mut tx,
            self.import,
            &rows::Ended {
                state,
                counts: &counts,
                actions: &actions,
                losses: &losses_json,
                failure: failure.as_ref(),
                package: package.as_ref(),
                at: now,
            },
        )
        .await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(Done::Finished)
    }

    /// Takes back what the import made.
    async fn remove(&self, job: &Job) -> Result<Done, Stop> {
        let _ = job;
        crate::imports::undo::remove(self, job).await
    }

    async fn finish_undo(&self, job: &Job) -> Result<Done, Stop> {
        let _ = job;
        let mut conn = self.engine.inner.db.acquire().await?;
        let now = self.engine.inner.clock.now();
        rows::end_undo(&mut conn, self.import, true, now).await?;
        Ok(Done::Finished)
    }
}

impl Importing {
    /// The import this job is about, for the undo.
    #[must_use]
    pub fn import(&self) -> Id {
        self.import
    }

    /// The person who asked.
    #[must_use]
    pub const fn user(&self) -> Id {
        self.user
    }

    /// The engine.
    #[must_use]
    pub const fn engine(&self) -> &Engine {
        &self.engine
    }

    /// The person, loaded for a batch.
    pub async fn principal_for(&self, conn: &mut orch_db::Conn) -> Result<Principal, Stop> {
        self.principal(conn).await
    }
}

/// The word of an outcome, for logs.
#[must_use]
pub fn outcome_text(outcome: ImportOutcome) -> &'static str {
    outcome_word(outcome)
}

/// The word of a kind, for logs.
#[must_use]
pub fn kind_text(kind: PackageKind) -> &'static str {
    kind_word(kind)
}
