//! `edit_document` `import`: a directory or a zip of the workspace read into documents, its folders into document folders, with the links between its files made references and the pictures and files they name made files of the documents.
//!
//! The same readers that a person's import and an agent's `file` go through make the blocks; what is particular here is
//! the walk. A directory is listed through the machine (hidden entries are left out), a zip is read whole from the
//! store and expanded in memory within `sharing.snapshot_max_bytes`. Every Markdown, HTML, Word and text file becomes
//! a document in the folder its own folder becomes, under an identifier derived from where it came from, so that
//! asking again makes nothing twice. A link from one of them to another is a reference to the document that other
//! becomes; a picture or a file it names becomes a file of the document that names it; what was not found is listed.
//! The work goes on after the call's own wait, which is long enough for a handful of files: the same call again says
//! how far it is, and what it made.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use orch_api_contract::documents::{BlockContent, BlockType, Inline, NewBlock};
use orch_api_contract::workspace::DirEntryKind;
use orch_db::DbError;
use orch_db::queries::documents::containers;
use orch_db::queries::threads::ThreadRow;
use orch_documents::convert::{self, Format};
use orch_documents::markdown::{Directory, FileRef, Local};
use orch_documents::references::Typed;
use orch_documents::versions::Writer;
use orch_settings::Place;
use orch_settings_contract::registry::keys;
use orch_types::{Id, Json, Time, new_id};
use serde_json::json;

use crate::document_tree as organization;
use crate::engine::Engine;
use crate::error::EngineError;
use crate::ids;
use crate::tools::definitions::{Code, Refusal};
use crate::tools::edit::{Arguments, agent_name, apply_chunks, creation_ops, settings_at};
use crate::tools::edit_source::Found;
use crate::tools::shared::Target;
use crate::tools::tree::Where;
use crate::tools::workspace_files::{WorkspaceFile, relative_file};
use crate::tools::{Answer, CallContext};

/// How long a call waits for an import before it says how far it is: long enough for a handful of files, short enough
/// that a tool call does not hang on a large tree.
const IMPORT_WAIT: Duration = Duration::from_secs(20);
/// How long the result of an import that nobody asked about is kept.
const RESULT_KEPT: Duration = Duration::from_secs(600);
/// How many results of an import an answer lists, newest last; the rest are counted.
const RESULTS_SHOWN: usize = 60;
/// How many results a running import keeps for its answers: of a longer one only the latest, the counts being of all.
const RESULTS_HELD_MAX: usize = 200;
/// How many files (and folders) one import takes from a directory or a zip: a larger tree is imported a folder at a
/// time, and the plan of an import is a list of this many paths at most.
const ENTRIES_MAX: usize = 100_000;
/// Bound long-name metadata independently of the entry count; maps borrow or copy at most this many path bytes.
const PLAN_PATH_BYTES_MAX: usize = 16 * 1024 * 1024;
/// Two independent imports can progress without multiplying the bounded Word sandboxes across every agent.
const ACTIVE_MAX: usize = 2;
/// Completed diagnostics cannot accumulate for every caller during the retention interval.
const RUNS_MAX: usize = 64;
/// Count and byte bounds both apply: one diagnostic may name many missing files.
const RESULTS_BYTES_MAX: usize = 1024 * 1024;
/// What an import may expand to where no setting says: the shipped bound of a person's import.
const EXPANDED_BYTES_DEFAULT: i64 = 64 * 1024 * 1024;

/// Where the bytes of an entry are.
#[derive(Clone)]
enum Origin {
    /// A file of the workspace, by its path from the thread's directory, and the most bytes it may have.
    Workspace(String, i64),
    /// Bytes expanded from an archive.
    Bytes(Arc<Vec<u8>>),
}

#[derive(Clone)]
struct Entry {
    /// Its path inside the thing imported, with `/`.
    rel: String,
    origin: Origin,
}

/// What an import is going to do.
struct Plan {
    parent: organization::Directory,
    node: Id,
    root_path: String,
    entries: Vec<Entry>,
    source: String,
    destination: String,
}

/// How far an import is.
#[derive(Default)]
struct Progress {
    total: usize,
    done: usize,
    created: usize,
    existing: usize,
    failed: usize,
    folders: usize,
    finished: bool,
    cancelled: bool,
    planned: bool,
    finished_at: Option<Instant>,
    results_total: usize,
    results_bytes: usize,
    results: Vec<Json>,
    refusal: Option<Refusal>,
}

impl Progress {
    /// Keeps a result for the answers; only the latest are kept.
    fn keep(&mut self, mut result: Json) {
        self.results_total += 1;
        let mut size = result.to_string().len();
        if size > RESULTS_BYTES_MAX {
            result = json!({
                "status":"note", "details_omitted":true,
                "reason":"This entry's diagnostics exceed the retained byte allowance; the import counts include its outcome.",
            });
            size = result.to_string().len();
        }
        self.results_bytes += size;
        self.results.push(result);
        while self.results.len() > RESULTS_HELD_MAX || self.results_bytes > RESULTS_BYTES_MAX {
            self.results_bytes -= self.results.remove(0).to_string().len();
        }
    }
}

#[derive(Default)]
struct Run {
    progress: Mutex<Progress>,
    cancelled: AtomicBool,
    changed: tokio::sync::Notify,
}

/// Tuple fields cannot collide when a valid file or destination name contains a separator.
type RunKey = (Id, String, String);

#[derive(Default)]
struct Registry {
    runs: HashMap<RunKey, Arc<Run>>,
}

impl Registry {
    fn prune(&mut self) {
        self.runs.retain(|_, run| {
            let progress = lock(&run.progress);
            !progress.finished
                || progress
                    .finished_at
                    .is_some_and(|at| at.elapsed() < RESULT_KEPT)
        });
    }

    /// Reservation precedes all source reads; duplicate concurrent calls share the same job.
    fn admit(&mut self, key: RunKey, resume: bool) -> Result<(Arc<Run>, bool), Refusal> {
        self.prune();
        if let Some(run) = self.runs.get(&key) {
            let restart = resume && lock(&run.progress).finished;
            if !restart {
                return Ok((Arc::clone(run), false));
            }
        }
        let active = self
            .runs
            .values()
            .filter(|run| !lock(&run.progress).finished)
            .count();
        if active >= ACTIVE_MAX {
            return Err(Refusal::new(
                Code::Limit,
                "Two agent imports are running. Poll an existing import, cancel one with the same path and file plus cancel: true, or retry when one finishes.",
            ));
        }
        self.runs.remove(&key);
        if self.runs.len() >= RUNS_MAX {
            let oldest = self
                .runs
                .iter()
                .filter_map(|(key, run)| {
                    lock(&run.progress).finished_at.map(|at| (key.clone(), at))
                })
                .min_by_key(|(_, at)| *at)
                .map(|(key, _)| key);
            if let Some(oldest) = oldest {
                self.runs.remove(&oldest);
            } else {
                return Err(Refusal::new(
                    Code::Limit,
                    "The import jobs are occupied. Poll or cancel an existing import, then retry.",
                ));
            }
        }
        let run = Arc::new(Run::default());
        self.runs.insert(key, Arc::clone(&run));
        Ok((run, true))
    }

    fn forget(&mut self, key: &RunKey, run: &Arc<Run>) {
        if self
            .runs
            .get(key)
            .is_some_and(|current| Arc::ptr_eq(current, run))
        {
            self.runs.remove(key);
        }
    }
}

/// An aborted or panicking task releases admission too, while preserving its last committed counts.
struct Completion {
    run: Arc<Run>,
    completed: bool,
}

impl Drop for Completion {
    fn drop(&mut self) {
        let mut progress = lock(&self.run.progress);
        progress.cancelled = self.run.cancelled.load(Ordering::Acquire);
        if !self.completed && !progress.cancelled {
            progress.failed += 1;
            progress.keep(json!({"status":"failed","reason":"The import task stopped unexpectedly. Resume with the same path and file."}));
        }
        progress.finished = true;
        progress.finished_at = Some(Instant::now());
        drop(progress);
        self.run.changed.notify_waiters();
    }
}

static RUNS: LazyLock<Mutex<Registry>> = LazyLock::new(|| Mutex::new(Registry::default()));

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The folder part of an entry's path.
fn folder_of(rel: &str) -> &str {
    rel.rsplit_once('/').map_or("", |(folder, _)| folder)
}

/// What the reader is told while it reads one file of an import.
struct Neighbours<'a> {
    base: String,
    documents: &'a HashMap<String, Id>,
    files: HashMap<String, FileRef>,
}

impl Directory for Neighbours<'_> {
    fn local(&self, path: &str) -> Option<Local> {
        let joined = convert::join_path(&self.base, path)?;
        if let Some(id) = self.documents.get(&joined) {
            return Some(Local::Document(Typed {
                kind: orch_api_contract::documents::InlineRefKind::Document,
                id: *id,
                path: None,
                part: None,
            }));
        }
        self.files.get(&joined).cloned().map(Local::File)
    }
}

/// A title: the first heading, once, else the name of the file.
fn title_of(rel: &str, blocks: &mut Vec<NewBlock>) -> String {
    let heading = blocks
        .first()
        .filter(|block| block.block_type == BlockType::Heading)
        .and_then(|block| match &block.content {
            Some(BlockContent::Inline(runs)) => Some(
                runs.iter()
                    .filter_map(|run| match run {
                        Inline::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<String>()
                    .trim()
                    .to_owned(),
            ),
            _ => None,
        })
        .filter(|text| !text.is_empty());
    let name = rel.rsplit('/').next().unwrap_or(rel);
    let name = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
    let most = usize::try_from(orch_api_contract::documents::DOCUMENT_TITLE_MAX_CHARS)
        .unwrap_or(usize::MAX);
    let title: String = heading
        .clone()
        .unwrap_or_else(|| name.trim().to_owned())
        .chars()
        .take(most)
        .collect();
    if heading.as_deref() == Some(title.as_str()) {
        blocks.remove(0);
    }
    if title.is_empty() {
        "Untitled".to_owned()
    } else {
        title
    }
}

impl Engine {
    /// `import`: reads what is in a workspace directory or zip into documents.
    pub(super) async fn import_documents(
        &self,
        thread: &ThreadRow,
        arguments: &Arguments,
        _call: &CallContext,
    ) -> Result<Answer, EngineError> {
        let source = arguments.file.clone().unwrap_or_default();
        if !relative_file(source.trim_end_matches('/')) {
            return Ok(Refusal::invalid(
                "file must be a relative workspace directory or zip path.",
            )
            .into());
        }
        let key = (
            thread.node_id,
            arguments.path.trim().to_owned(),
            source.clone(),
        );
        let cancelling = arguments.cancel == Some(true);
        let (run, start) = {
            let mut registry = lock(&RUNS);
            if cancelling {
                registry.prune();
                let Some(run) = registry.runs.get(&key).cloned() else {
                    return Ok(Refusal::new(
                        Code::NotFound,
                        "No import is running for this path and file. Cancel creates no job.",
                    )
                    .into());
                };
                run.cancelled.store(true, Ordering::Release);
                (run, false)
            } else {
                match registry.admit(key.clone(), arguments.resume == Some(true)) {
                    Ok(admitted) => admitted,
                    Err(refusal) => return Ok(refusal.into()),
                }
            }
        };
        if start {
            let engine = self.clone();
            let thread = thread.clone();
            let arguments = arguments.clone();
            let task = Arc::clone(&run);
            tokio::spawn(async move {
                let mut completion = Completion {
                    run: Arc::clone(&task),
                    completed: false,
                };
                let outcome = engine.plan_agent_import(&thread, &arguments, &task).await;
                match outcome {
                    Ok(Ok(plan)) => {
                        {
                            let mut progress = lock(&task.progress);
                            progress.total = plan
                                .entries
                                .iter()
                                .filter(|entry| Format::of_name(&entry.rel).is_some())
                                .count();
                            progress.planned = true;
                        }
                        task.changed.notify_waiters();
                        if let Err(error) = engine.run_import(&thread, &plan, &task).await {
                            let mut progress = lock(&task.progress);
                            progress.failed += 1;
                            progress.keep(json!({"status":"failed","reason":format!("The import stopped: {error}")}));
                        }
                        // Archive bytes and all planning metadata leave with the task, before admission is released.
                        drop(plan);
                    }
                    Ok(Err(refusal)) => {
                        if !task.cancelled.load(Ordering::Acquire) {
                            let mut progress = lock(&task.progress);
                            progress.failed += 1;
                            progress.keep(json!({"status":"failed","reason":refusal.message}));
                            progress.refusal = Some(refusal);
                        }
                    }
                    Err(error) => {
                        let mut progress = lock(&task.progress);
                        progress.failed += 1;
                        progress.keep(json!({"status":"failed","reason":error.to_string()}));
                    }
                }
                completion.completed = true;
            });
        }
        let _ = tokio::time::timeout(IMPORT_WAIT, async {
            loop {
                let waiting = run.changed.notified();
                tokio::pin!(waiting);
                waiting.as_mut().enable();
                if lock(&run.progress).finished {
                    return;
                }
                waiting.await;
            }
        })
        .await;
        let progress = lock(&run.progress);
        let shown = progress.results.len().min(RESULTS_SHOWN);
        let skipped = progress.results.len() - shown;
        let answer = json!({
            "path": arguments.path.trim(), "source":source,
            "status": if progress.cancelled { "cancelled" } else if progress.finished { "done" }
                else if run.cancelled.load(Ordering::Acquire) { "stopping" }
                else if progress.planned { "importing" } else { "planning" },
            "total":progress.total,"done":progress.done,"created":progress.created,
            "existing":progress.existing,"failed":progress.failed,"folders":progress.folders,
            "results":progress.results.iter().skip(skipped).collect::<Vec<_>>(),
            "results_omitted":progress.results_total - shown,
        });
        let forget = progress.finished && !progress.cancelled;
        let refusal = progress.refusal.clone();
        drop(progress);
        if forget {
            lock(&RUNS).forget(&key, &run);
        }
        if let Some(refusal) = refusal {
            return Ok(refusal.into());
        }
        Ok(Answer::Done(answer))
    }

    /// Finds what is to be imported, and where it goes, before anything is made.
    #[allow(
        clippy::too_many_lines,
        reason = "the destination, the source and what lies in it are settled together, in one order"
    )]
    async fn plan_agent_import(
        &self,
        thread: &ThreadRow,
        arguments: &Arguments,
        run: &Run,
    ) -> Result<Result<Plan, Refusal>, EngineError> {
        let directory = format!("{}/", arguments.path.trim().trim_end_matches('/'));
        let source = arguments.file.clone().unwrap_or_default();
        let (parent, node, root_path, limit) = {
            let mut conn = self.inner.db.acquire().await?;
            let resolved = match self
                .resolve_write(&mut conn, thread.node_id, &directory, false)
                .await?
            {
                Ok(resolved) => resolved,
                Err(refusal) => return Ok(Err(refusal)),
            };
            let parent = match resolved.target {
                Target::Directory(Where::Docs(node)) => organization::Directory::NodeDocs(node),
                Target::Directory(Where::DocumentFolder(id)) => {
                    organization::Directory::Container(id)
                }
                _ => {
                    return Ok(Err(Refusal::new(
                        Code::WrongKind,
                        format!(
                            "{} is not a docs directory or a document folder. Import into the docs directory of a place that list_shared shows as writable.",
                            resolved.path
                        ),
                    )));
                }
            };
            let Some(node) = resolved.tree.directory_node(parent) else {
                return Ok(Err(Refusal::new(
                    Code::NotFound,
                    "That document directory is not available.",
                )));
            };
            let writable = match parent {
                organization::Directory::NodeDocs(node) => {
                    resolved.tree.documents_standing(node).writes()
                }
                organization::Directory::Container(id) => resolved.tree.folder_standing(id).write,
            };
            if !writable || resolved.tree.reach().archived(node) {
                return Ok(Err(Refusal::new(
                    Code::NotWritable,
                    format!(
                        "You may not make documents in {}: that takes the right doc.edit on the place itself, and the place may not be archived.",
                        resolved.path
                    ),
                )));
            }
            if !settings_at(&mut conn, node).await?.enabled {
                return Ok(Err(Refusal::new(
                    Code::Limit,
                    "Documents are switched off where that is (the setting docs.enabled), and a person can switch them on there.",
                )));
            }
            let limit = orch_settings::get::<i64>(
                &mut conn,
                Place::Node(node),
                keys::SHARING_SNAPSHOT_MAX_BYTES,
            )
            .await?
            .unwrap_or(EXPANDED_BYTES_DEFAULT);
            (parent, node, resolved.path, limit)
        };
        if !relative_file(source.trim_end_matches('/')) {
            return Ok(Err(Refusal::invalid(
                "\"file\" is a directory or a zip, relative to your thread's directory, without .. or an absolute path.",
            )));
        }
        if run.cancelled.load(Ordering::Acquire) {
            return Ok(Err(Refusal::new(Code::Closed, "The import was cancelled.")));
        }
        let kind = match self.workspace_kind(thread, &source).await? {
            Ok(kind) => kind,
            Err(refusal) => return Ok(Err(refusal)),
        };
        let mut entries: Vec<Entry> = Vec::new();
        match kind {
            DirEntryKind::Dir => {
                let mut pending: Vec<String> = vec![String::new()];
                let mut folders_seen = 0_usize;
                let mut path_bytes = 0_usize;
                while let Some(folder) = pending.pop() {
                    if run.cancelled.load(Ordering::Acquire) {
                        return Ok(Err(Refusal::new(Code::Closed, "The import was cancelled.")));
                    }
                    folders_seen += 1;
                    if folders_seen > ENTRIES_MAX || entries.len() > ENTRIES_MAX {
                        return Ok(Err(Refusal::invalid(format!(
                            "{source:?} holds more than {ENTRIES_MAX} files or folders. Import it a folder at a time."
                        ))));
                    }
                    let at = if folder.is_empty() {
                        source.trim_end_matches('/').to_owned()
                    } else {
                        format!("{}/{folder}", source.trim_end_matches('/'))
                    };
                    let listed = match self.workspace_list(thread, &at).await? {
                        Ok(listed) => listed,
                        Err(refusal) => return Ok(Err(refusal)),
                    };
                    for each in listed {
                        // What is hidden is not a document: `.git` and its kind stay where they are.
                        if each.name.starts_with('.') {
                            continue;
                        }
                        let rel = if folder.is_empty() {
                            each.name.clone()
                        } else {
                            format!("{folder}/{}", each.name)
                        };
                        path_bytes = path_bytes.saturating_add(rel.len());
                        if path_bytes > PLAN_PATH_BYTES_MAX
                            || entries.len() + pending.len() + folders_seen >= ENTRIES_MAX
                        {
                            return Ok(Err(Refusal::new(
                                Code::Limit,
                                "The import plan exceeds its bounded path allowance. Import one subdirectory at a time.",
                            )));
                        }
                        if !relative_file(&format!("{}/{rel}", source.trim_end_matches('/'))) {
                            return Ok(Err(Refusal::invalid(
                                "An import entry exceeds the workspace path rules.",
                            )));
                        }
                        match each.kind {
                            DirEntryKind::Dir => pending.push(rel),
                            DirEntryKind::File => entries.push(Entry {
                                origin: Origin::Workspace(
                                    format!("{}/{rel}", source.trim_end_matches('/')),
                                    limit,
                                ),
                                rel,
                            }),
                            DirEntryKind::Symlink | DirEntryKind::Other => {}
                        }
                    }
                }
            }
            DirEntryKind::File => {
                let mut files = match self
                    .read_workspace_files(thread, std::slice::from_ref(&source), limit)
                    .await?
                {
                    Ok(files) => files,
                    Err(refusal) => return Ok(Err(refusal)),
                };
                let file = files.pop().ok_or_else(|| {
                    EngineError::unreadable("the workspace reader returned no file")
                })?;
                let bytes = match self.workspace_bytes(&file).await {
                    Ok(bytes) => bytes,
                    Err(refusal) => return Ok(Err(refusal)),
                };
                entries = match tokio::task::spawn_blocking(move || expand_zip(&bytes, limit)).await
                {
                    Ok(Ok(entries)) => entries,
                    Ok(Err(why)) => {
                        return Ok(Err(Refusal::invalid(format!(
                            "{source:?} is not a zip that can be read here: {why}. Unpack another kind of archive in your workspace and import the directory."
                        ))));
                    }
                    Err(error) => return Err(EngineError::unreadable(error.to_string())),
                };
            }
            _ => {
                return Ok(Err(Refusal::invalid(format!(
                    "{source:?} is neither a directory nor a file."
                ))));
            }
        }
        entries.sort_by(|a, b| a.rel.cmp(&b.rel));
        Ok(Ok(Plan {
            parent,
            node,
            root_path,
            entries,
            source,
            destination: directory,
        }))
    }

    /// Makes the documents of a plan, one after the other, each in a transaction of its own.
    #[allow(
        clippy::too_many_lines,
        reason = "the folders, the neighbours and the document of each file are made in the order they depend on each other"
    )]
    async fn run_import(
        &self,
        thread: &ThreadRow,
        plan: &Plan,
        run: &Arc<Run>,
    ) -> Result<(), EngineError> {
        let identity = |what: &str, rel: &str| {
            ids::derived(
                what,
                &[
                    thread.node_id.as_bytes().as_slice(),
                    plan.destination.as_bytes(),
                    plan.source.as_bytes(),
                    rel.as_bytes(),
                ],
            )
        };
        // Which entries are documents, and what each becomes.
        let documents: HashMap<String, Id> = plan
            .entries
            .iter()
            .filter(|entry| Format::of_name(&entry.rel).is_some())
            .map(|entry| (entry.rel.clone(), identity("import-document", &entry.rel)))
            .collect();
        let by_path: HashMap<&str, &Entry> = plan
            .entries
            .iter()
            .map(|entry| (entry.rel.as_str(), entry))
            .collect();
        let mut folders: HashMap<String, organization::Directory> = HashMap::new();
        folders.insert(String::new(), plan.parent);
        let options = organization::Options::for_engine(self);
        for entry in plan
            .entries
            .iter()
            .filter(|entry| documents.contains_key(&entry.rel))
        {
            if run.cancelled.load(Ordering::Acquire) {
                break;
            }
            let outcome = self
                .import_one(
                    thread,
                    plan,
                    entry,
                    &documents,
                    &by_path,
                    &mut folders,
                    &options,
                    identity("import-document", &entry.rel),
                    run,
                )
                .await;
            if run.cancelled.load(Ordering::Acquire) && !matches!(&outcome, Ok(Ok(_))) {
                break;
            }
            let mut progress = lock(&run.progress);
            progress.done += 1;
            match outcome {
                Ok(Ok(result)) => {
                    if result["status"] == "exists" {
                        progress.existing += 1;
                    } else {
                        progress.created += 1;
                    }
                    progress.keep(result);
                }
                Ok(Err(refusal)) => {
                    progress.failed += 1;
                    progress.keep(json!({
                        "source": entry.rel,
                        "status": "failed",
                        "reason": refusal.message,
                    }));
                }
                Err(error) => {
                    progress.failed += 1;
                    progress.keep(json!({
                        "source": entry.rel,
                        "status": "failed",
                        "reason": error.to_string(),
                    }));
                }
            }
            progress.folders = folders.len().saturating_sub(1);
            drop(progress);
            run.changed.notify_waiters();
        }
        // Files that no document names are left where they are: they are no documents.
        let unnamed = plan.entries.len() - documents.len();
        if unnamed > 0 {
            lock(&run.progress).keep(json!({
                "status": "note",
                "reason": format!("{unnamed} file(s) that are not Markdown, HTML, Word or text were not documents; those that a document names became its files."),
            }));
        }
        Ok(())
    }

    /// Makes the document of one entry, with its folders, its references and its files.
    #[allow(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        reason = "one entry is read, resolved against its neighbours and written in one transaction"
    )]
    async fn import_one(
        &self,
        thread: &ThreadRow,
        plan: &Plan,
        entry: &Entry,
        documents: &HashMap<String, Id>,
        by_path: &HashMap<&str, &Entry>,
        folders: &mut HashMap<String, organization::Directory>,
        options: &organization::Options,
        id: Id,
        run: &Run,
    ) -> Result<Result<Json, Refusal>, EngineError> {
        let Some(format) = Format::of_name(&entry.rel) else {
            return Ok(Err(Refusal::invalid("not a document")));
        };
        let bytes = match self.entry_bytes(thread, entry).await? {
            Ok(bytes) => bytes,
            Err(refusal) => return Ok(Err(refusal)),
        };
        let prepared =
            match tokio::task::spawn_blocking(move || convert::prepare(format, &bytes)).await {
                Ok(Ok(prepared)) => prepared,
                Ok(Err(error)) => {
                    return Ok(Err(Refusal::invalid(format!("could not be read: {error}"))));
                }
                Err(error) => return Err(EngineError::unreadable(error.to_string())),
            };
        // The files it names that came with it are read once each.
        let base = folder_of(&entry.rel).to_owned();
        let mut found: HashMap<String, Found> = HashMap::new();
        for target in prepared.targets() {
            if run.cancelled.load(Ordering::Acquire) {
                return Ok(Err(Refusal::new(
                    Code::Closed,
                    "The import was cancelled before this file was committed.",
                )));
            }
            let Some(path) = convert::join_path(&base, &target.path) else {
                continue;
            };
            if documents.contains_key(&path) || found.contains_key(&path) {
                continue;
            }
            if found.len() >= crate::tools::edit_source::FOUND_MAX {
                break;
            }
            let Some(named) = by_path.get(path.as_str()) else {
                continue;
            };
            let Ok(file) = self.entry_file(thread, named).await? else {
                continue;
            };
            let name = path.rsplit('/').next().unwrap_or(&path).to_owned();
            found.insert(
                path,
                Found {
                    id: new_id(),
                    name,
                    file,
                },
            );
        }
        let neighbours = Neighbours {
            base: base.clone(),
            documents,
            files: found
                .iter()
                .map(|(path, each)| {
                    (
                        path.clone(),
                        FileRef {
                            id: each.id,
                            media_type: each.file.media_type.clone(),
                        },
                    )
                })
                .collect(),
        };
        let mut read = prepared.finish(&neighbours);
        if run.cancelled.load(Ordering::Acquire) {
            return Ok(Err(Refusal::new(
                Code::Closed,
                "The import was cancelled before this file was committed.",
            )));
        }
        let title = title_of(&entry.rel, &mut read.blocks);
        // Its folder, and the ones above it, made where they are not there.
        let parent = match self
            .ensure_folders(thread, plan, &base, folders, options)
            .await?
        {
            Ok(parent) => parent,
            Err(refusal) => return Ok(Err(refusal)),
        };
        let mut tx = orch_db::queries::transaction_guard::request_scope(
            orch_db::queries::transaction_guard::Scope::Creation,
            self.inner.db.begin(),
        )
        .await?;
        let now = self.inner.clock.now();
        let settings = settings_at(&mut tx, plan.node).await?;
        let created = organization::create_text_in(
            &mut tx,
            organization::Actor::Agent {
                thread: thread.node_id,
            },
            parent,
            id,
            &title,
            None,
            Time::new(now),
        )
        .await;
        let created = match crate::tools::manage::domain_result(
            created,
            organization::Command::CreateFolder,
        )? {
            Ok(created) => created,
            Err(refusal) => return Ok(Err(refusal)),
        };
        let shown = format!(
            "{}{}{}.md",
            plan.root_path,
            if base.is_empty() {
                String::new()
            } else {
                format!("{base}/")
            },
            title
        );
        if !created.created {
            tx.commit().await.map_err(DbError::from)?;
            return Ok(Ok(json!({
                "source": entry.rel,
                "status": "exists",
                "path": shown,
                "id": id,
            })));
        }
        let agent = agent_name(&mut tx, thread).await?;
        let ops = creation_ops(read.blocks, None);
        let writer = Writer::Agent {
            thread: thread.node_id,
            agent: agent.clone(),
        };
        let added = match self
            .hold_files(
                &mut tx,
                id,
                thread.node_id,
                &settings,
                &writer,
                now,
                &read.embedded,
                Some(&found),
                &ops,
            )
            .await?
        {
            Ok(added) => added,
            Err(refusal) => return Ok(Err(refusal)),
        };
        let applied = if ops.is_empty() {
            None
        } else {
            match apply_chunks(&mut tx, id, thread, &agent, &ops, &settings, now).await? {
                Ok(applied) => Some(applied),
                Err(refusal) => return Ok(Err(refusal)),
            }
        };
        if let Some(applied) = &applied {
            self.finish_edit(&mut tx, thread, id, "create", applied, now)
                .await?;
        }
        tx.commit().await.map_err(DbError::from)?;
        let warnings: Vec<String> = read
            .warnings
            .iter()
            .map(|each| each.message.clone())
            .collect();
        Ok(Ok(json!({
            "source": entry.rel,
            "status": "created",
            "path": shown,
            "id": id,
            "files": added.len(),
            "missing": read.missing,
            "warnings": warnings,
        })))
    }

    /// The bytes of an entry.
    async fn entry_bytes(
        &self,
        thread: &ThreadRow,
        entry: &Entry,
    ) -> Result<Result<Vec<u8>, Refusal>, EngineError> {
        match &entry.origin {
            Origin::Bytes(bytes) => Ok(Ok(bytes.as_ref().clone())),
            Origin::Workspace(..) => {
                let file = match self.entry_file(thread, entry).await? {
                    Ok(file) => file,
                    Err(refusal) => return Ok(Err(refusal)),
                };
                Ok(self.workspace_bytes(&file).await)
            }
        }
    }

    /// An entry as a file in the store: read from the workspace, or put in the store from what a zip held.
    async fn entry_file(
        &self,
        thread: &ThreadRow,
        entry: &Entry,
    ) -> Result<Result<WorkspaceFile, Refusal>, EngineError> {
        match &entry.origin {
            Origin::Workspace(path, largest) => {
                match self
                    .read_workspace_files(thread, std::slice::from_ref(path), *largest)
                    .await?
                {
                    Ok(mut files) => match files.pop() {
                        Some(file) => Ok(Ok(file)),
                        None => Err(EngineError::unreadable(
                            "the workspace reader returned no file",
                        )),
                    },
                    Err(refusal) => Ok(Err(refusal)),
                }
            }
            Origin::Bytes(bytes) => {
                let Some(store) = self.blobs() else {
                    return Ok(Err(Refusal::new(
                        Code::Closed,
                        "The file store is not there. Try again.",
                    )));
                };
                let hash = orch_types::Hash::of(bytes);
                let size = i64::try_from(bytes.len()).unwrap_or(i64::MAX);
                store
                    .put(
                        &hash,
                        Some(size),
                        orch_blob_contract::store::stream_of(vec![bytes::Bytes::from(
                            bytes.as_ref().clone(),
                        )]),
                    )
                    .await
                    .map_err(|error| EngineError::unreadable(error.to_string()))?;
                let name = entry.rel.rsplit('/').next().unwrap_or(&entry.rel);
                let head = &bytes[..bytes.len().min(512)];
                Ok(Ok(WorkspaceFile {
                    name: name.to_owned(),
                    size,
                    blob: hash,
                    media_type: orch_documents::files::media_type_of(name, None),
                    is_text: orch_documents::files::looks_like_text(head),
                }))
            }
        }
    }

    /// The folder for a path inside the import, and the ones above it: found where they are, made where they are not.
    async fn ensure_folders(
        &self,
        thread: &ThreadRow,
        plan: &Plan,
        folder: &str,
        folders: &mut HashMap<String, organization::Directory>,
        options: &organization::Options,
    ) -> Result<Result<organization::Directory, Refusal>, EngineError> {
        let mut walked = String::new();
        let mut current = plan.parent;
        for part in folder.split('/').filter(|part| !part.is_empty()) {
            if !walked.is_empty() {
                walked.push('/');
            }
            walked.push_str(part);
            if let Some(known) = folders.get(&walked) {
                current = *known;
                continue;
            }
            let mut tx = self.inner.db.begin().await?;
            let at = match current {
                organization::Directory::Container(id) => Some(id),
                organization::Directory::NodeDocs(_) => None,
            };
            let existing = containers::folder_named(&mut tx, plan.node, at, part).await?;
            let made = match existing {
                Some(id) => organization::Directory::Container(id),
                None => {
                    let request = organization::Request {
                        command: organization::Command::CreateFolder,
                        items: Vec::new(),
                        to: Some(current),
                        name: Some(part.to_owned()),
                        request_ref: format!("import-folder:{}:{walked}", plan.source),
                        version: None,
                    };
                    let receipt = match crate::tools::manage::domain_result(
                        organization::execute_in(
                            &mut tx,
                            organization::Actor::Agent {
                                thread: thread.node_id,
                            },
                            &request,
                            None,
                            Time::new(self.inner.clock.now()),
                            *options,
                        )
                        .await,
                        organization::Command::CreateFolder,
                    )? {
                        Ok(receipt) => receipt,
                        Err(refusal) => return Ok(Err(refusal)),
                    };
                    let Some(id) = receipt.entries.iter().find_map(|each| match each.item {
                        organization::Item::Container(id) => Some(id),
                        _ => None,
                    }) else {
                        return Ok(Err(Refusal::new(
                            Code::NotFound,
                            "The folder that was made is not there.",
                        )));
                    };
                    organization::Directory::Container(id)
                }
            };
            tx.commit().await.map_err(DbError::from)?;
            folders.insert(walked.clone(), made);
            current = made;
        }
        Ok(Ok(current))
    }
}

/// The files of a zip, read whole into memory within what an import may expand to.
fn expand_zip(bytes: &[u8], limit: i64) -> Result<Vec<Entry>, String> {
    use std::io::Read as _;
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|error| error.to_string())?;
    let mut entries = Vec::new();
    let mut used: u64 = 0;
    let mut path_bytes = 0_usize;
    let most = u64::try_from(limit).unwrap_or(0);
    for index in 0..archive.len() {
        let file = archive.by_index(index).map_err(|error| error.to_string())?;
        if file.is_dir() || file.encrypted() {
            continue;
        }
        let Some(path) = file.enclosed_name() else {
            continue;
        };
        let rel = path
            .components()
            .filter_map(|part| match part {
                std::path::Component::Normal(part) => part.to_str().map(str::to_owned),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("/");
        if rel.is_empty()
            || rel
                .split('/')
                .any(|part| part.starts_with('.') || part == "__MACOSX")
        {
            continue;
        }
        if entries.len() >= ENTRIES_MAX {
            return Err(format!("it holds more than {ENTRIES_MAX} files"));
        }
        path_bytes = path_bytes.saturating_add(rel.len());
        if path_bytes > PLAN_PATH_BYTES_MAX || !relative_file(&rel) {
            return Err(
                "its paths exceed the bounded import metadata or workspace path rules".to_owned(),
            );
        }
        let remaining = most.saturating_sub(used);
        if file.size() > remaining {
            return Err(format!(
                "it expands to more than {most} bytes (sharing.snapshot_max_bytes)"
            ));
        }
        let mut data = Vec::with_capacity(usize::try_from(file.size().min(remaining)).unwrap_or(0));
        file.take(remaining.saturating_add(1))
            .read_to_end(&mut data)
            .map_err(|error| error.to_string())?;
        used = used.saturating_add(u64::try_from(data.len()).unwrap_or(u64::MAX));
        if used > most {
            return Err(format!(
                "it expands to more than {most} bytes (sharing.snapshot_max_bytes)"
            ));
        }
        entries.push(Entry {
            rel,
            origin: Origin::Bytes(Arc::new(data)),
        });
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(number: usize) -> RunKey {
        (Id::nil(), "/docs/".to_owned(), format!("source-{number}"))
    }

    #[test]
    fn a_hundred_thousand_admissions_hold_at_most_two_jobs_and_duplicate_calls_share_one() {
        let registry = Arc::new(Mutex::new(Registry::default()));
        let admitted = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        std::thread::scope(|scope| {
            for worker in 0..8 {
                let registry = Arc::clone(&registry);
                let admitted = Arc::clone(&admitted);
                scope.spawn(move || {
                    for number in 0..12_500 {
                        if let Ok((_, true)) =
                            lock(&registry).admit(key(worker * 12_500 + number), false)
                        {
                            admitted.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                });
            }
        });
        assert_eq!(admitted.load(Ordering::Relaxed), 2);
        let mut registry = lock(&registry);
        assert_eq!(registry.runs.len(), 2);
        let Some((key, first)) = registry
            .runs
            .iter()
            .next()
            .map(|(key, run)| (key.clone(), Arc::clone(run)))
        else {
            panic!("an admitted import");
        };
        let Ok((again, started)) = registry.admit(key, false) else {
            panic!("an existing import is available while all places are occupied");
        };
        assert!(!started && Arc::ptr_eq(&first, &again));
    }

    #[test]
    fn cancellation_keeps_committed_counts_resume_replaces_only_that_job_and_old_cleanup_cannot_erase_it()
     {
        let mut registry = Registry::default();
        let Ok((first, true)) = registry.admit(key(1), false) else {
            panic!("first import")
        };
        lock(&first.progress).created = 7;
        first.cancelled.store(true, Ordering::Release);
        drop(Completion {
            run: Arc::clone(&first),
            completed: false,
        });
        {
            let progress = lock(&first.progress);
            assert!(progress.finished && progress.cancelled);
            assert_eq!(progress.created, 7);
            assert_eq!(progress.failed, 0);
        }
        let Ok((same, false)) = registry.admit(key(1), false) else {
            panic!("cancelled receipt")
        };
        assert!(Arc::ptr_eq(&first, &same));
        let Ok((resumed, true)) = registry.admit(key(1), true) else {
            panic!("resumed import")
        };
        assert!(!Arc::ptr_eq(&first, &resumed));
        registry.forget(&key(1), &first);
        assert!(
            registry
                .runs
                .get(&key(1))
                .is_some_and(|run| Arc::ptr_eq(run, &resumed))
        );
        // Task loss must release admission even when its ordinary completion path never executes.
        drop(Completion {
            run: Arc::clone(&resumed),
            completed: false,
        });
        assert!(lock(&resumed.progress).finished);
        assert_eq!(lock(&resumed.progress).failed, 1);
    }

    #[test]
    fn a_hundred_thousand_receipts_and_large_diagnostics_keep_bounded_memory_and_exact_omissions() {
        let mut registry = Registry::default();
        for number in 0..100_000 {
            let Ok((run, true)) = registry.admit(key(number), false) else {
                panic!("bounded receipt")
            };
            drop(Completion {
                run,
                completed: true,
            });
        }
        assert_eq!(registry.runs.len(), RUNS_MAX);
        assert!(!registry.runs.contains_key(&key(0)));
        let mut progress = Progress::default();
        for number in 0..100_000 {
            progress.keep(json!({"source":number,"status":"created"}));
        }
        assert_eq!(progress.results_total, 100_000);
        assert_eq!(progress.results.len(), RESULTS_HELD_MAX);
        for number in 0..1_000 {
            progress.keep(json!({"source":number,"reason":"x".repeat(16_384)}));
        }
        assert!(
            progress.results.len() < RESULTS_HELD_MAX,
            "the byte bound acts before the row bound"
        );
        let held: usize = progress
            .results
            .iter()
            .map(|value| value.to_string().len())
            .sum();
        assert!(held <= RESULTS_BYTES_MAX);
        assert_eq!(held, progress.results_bytes);
        assert_eq!(
            progress.results.last().map(|value| &value["source"]),
            Some(&json!(999))
        );
        assert_eq!(
            progress.results_total - progress.results.len().min(RESULTS_SHOWN),
            101_000 - RESULTS_SHOWN
        );
        progress.keep(json!({"reason":"x".repeat(RESULTS_BYTES_MAX + 1)}));
        assert_eq!(
            progress
                .results
                .last()
                .map(|value| &value["details_omitted"]),
            Some(&json!(true))
        );
    }
}
