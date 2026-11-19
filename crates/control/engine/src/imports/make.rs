//! Making what the plan says: one object at a time, inside the transaction of a batch, each in a savepoint of its own so that what cannot be made is one row of the report and nothing else.
//!
//! A [`Batch`] holds what making needs besides the object: the plan, the person who imports, the moment, and the files of the batch
//! that were put in the store before its transaction began (a file is read and hashed outside a transaction, which would hold its
//! locks for as long as a large file takes). An object that is made, or taken over, leaves a row in `import_origins`, so that the
//! next import of the same thing finds it and an undo can take it back.

use super::ImportError;
use super::model::{Object, Plan};
use super::plan::{another_name, creation_right, origin_kind};
use crate::engine::Engine;
use chrono::{DateTime, Utc};
use orch_api_contract::imports::ImportAction as Action;
use orch_api_contract::imports::{ImportAction, ImportMade, ImportOutcome, ImportWarning};
use orch_api_contract::package::PackageKind;
use orch_authz::Principal;
use orch_db::queries::imports::{self as rows, OriginRow};
use orch_db::{Conn, DbError, Tx};
use orch_types::{Hash, Id};
use serde_json::json;
use std::collections::HashMap;

mod access;
mod chats;
mod comments;
mod documents;
mod places;
mod threads;
mod versions;

/// How many rows of one kind an undo takes back in one transaction: a batch, so that taking back a million messages is a million
/// rows in two thousand small transactions and memory holds one batch.
pub const TAKE_BACK_BATCH: i64 = 500;

/// What an import adds to things that were there, by the kind of thing it is added to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Merged {
    /// Messages added to chats.
    Chat,
    /// Items added to threads.
    Thread,
}

impl Merged {
    /// The word of what is added, as the rows that say so write it.
    const fn word(self) -> &'static str {
        match self {
            Self::Chat => "message",
            Self::Thread => "item",
        }
    }
}

/// What asking for a batch of taking back came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Taken {
    /// So many rows of the import were looked at, and what they say the import added was taken back.
    Some(i64),
    /// There is nothing more to take back there.
    Nothing,
    /// The person may not take it back now: it stays.
    Refused,
}

/// A page of the chats or threads that the import added things to, by identifier after `after`.
pub async fn merged_parents(
    conn: &mut Conn,
    import: Id,
    which: Merged,
    after: Option<Id>,
    limit: i64,
) -> Result<Vec<Id>, DbError> {
    rows::parents_with_added(conn, import, which.word(), after, limit).await
}

/// Takes back a batch of what the import `import` added to the chat or thread `parent`, as the person `principal` may now: asked again
/// until it answers [`Taken::Nothing`] or [`Taken::Refused`], each time in a transaction of its own.
pub async fn take_back(
    tx: &mut Tx,
    import: Id,
    which: Merged,
    parent: Id,
    principal: &orch_authz::Principal,
    now: DateTime<Utc>,
) -> Result<Taken, MakeError> {
    match which {
        Merged::Chat => chats::take_back(tx, import, parent, principal, now, TAKE_BACK_BATCH).await,
        Merged::Thread => threads::take_back(tx, import, parent, principal, TAKE_BACK_BATCH).await,
    }
}

/// What remains to be tidied once nothing more is taken back: the segments of imported history that hold nothing.
pub async fn take_back_finish(tx: &mut Tx, import: Id) -> Result<(), MakeError> {
    threads::take_back_finish(tx, import).await?;
    Ok(())
}

/// A record of the package (the messages of a chat, the history of a thread) copied to a scratch file and checked against the hash the
/// manifest gives it, to be read a page at a time; the file is removed when this is dropped.
pub struct Record {
    path: std::path::PathBuf,
}

impl Record {
    /// Copies the entry that holds the record of `object`, which is a `what`.
    pub async fn open(batch: &Batch<'_>, object: &Object, what: &str) -> Result<Self, MakeError> {
        let Some(path) = object.object.path.as_deref() else {
            return refuse(format!("the package has no record of this {what}"));
        };
        // A record may be as large as the import may be: what the place lets an import hold bounds it.
        let most = u64::try_from(batch.plan.limits.max_bytes).unwrap_or(u64::MAX);
        let spooled = batch
            .plan
            .opened
            .entries
            .spool(path, most, &batch.engine.scratch_dir())
            .await
            .map_err(|error| MakeError::Refused(error.to_string()))?;
        let record = Self {
            path: spooled.path.clone(),
        };
        if let Some(expected) = &object.object.sha256
            && spooled.sha256 != *expected
        {
            return Err(MakeError::Damaged(path.to_owned()));
        }
        Ok(record)
    }

    /// The copy, opened for reading.
    pub fn file(&self) -> Result<std::fs::File, MakeError> {
        std::fs::File::open(&self.path)
            .map_err(|error| MakeError::Refused(format!("its record cannot be read: {error}")))
    }
}

impl Drop for Record {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// What the plan came to as maps from what the source calls things to what they are here, made once for the job and added to
/// as the chats of the nodes it makes are known.
#[derive(Debug, Default)]
pub struct PlanMaps {
    /// What each object of the source that is here, or that goes, is here.
    pub targets: HashMap<Id, Id>,
    /// The chat of each node of the source, as it is here, once it is.
    pub chats: HashMap<Id, Id>,
    /// What each file of a document of the source is here.
    pub files: HashMap<Id, Id>,
    /// The objects the source holds that are not here and do not come.
    pub gone: std::collections::HashSet<Id>,
    /// Everything the source holds.
    pub held: std::collections::HashSet<Id>,
}

impl PlanMaps {
    /// The maps of a plan.
    #[must_use]
    pub fn of(plan: &Plan, import: Id) -> Self {
        let mut maps = Self::default();
        for each in &plan.objects {
            maps.held.insert(each.object.id);
            if each.goes() {
                maps.targets.insert(each.object.id, each.target);
                if each.made_as == PackageKind::Document {
                    for file in &each.object.files {
                        if let Some(id) = file.id {
                            maps.files
                                .insert(id, super::identity::file(import, each.target, id));
                        }
                    }
                }
            } else if each.action == Action::Skip
                && let Some(found) = &each.existing
                && each.blocked.is_none()
                && !each.skipped_above
            {
                // What is skipped because it is there already is what a reference to it names.
                maps.targets.insert(each.object.id, found.id);
            }
        }
        for source in &maps.held {
            if !maps.targets.contains_key(source) {
                maps.gone.insert(*source);
            }
        }
        // The people of a package name their things that are not objects of it as well: nothing is gone that was never held.
        maps
    }
}

/// A file of the source that was put in the store before the batch began.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Staged {
    /// Where its content is in the store.
    pub blob: Hash,
    /// Its size.
    pub size: i64,
}

/// What making needs besides the object.
pub struct Batch<'a> {
    /// The engine.
    pub engine: &'a Engine,
    /// The plan.
    pub plan: &'a Plan,
    /// The person who imports, as they are now.
    pub principal: &'a Principal,
    /// The import.
    pub import: Id,
    /// The moment of the batch.
    pub now: DateTime<Utc>,
    /// The files of the source put in the store, by their path in it.
    pub staged: &'a HashMap<String, Staged>,
    /// What things of the source are here.
    pub maps: &'a PlanMaps,
    /// Where a long piece of work (the history of a thread, the messages of a chat) says how far it has come, a page of its record at a time:
    /// the job shows the words, and looks whether it is cancelled, between the pages of a record that takes minutes.
    pub notes: Option<tokio::sync::mpsc::UnboundedSender<String>>,
}

impl Batch<'_> {
    /// Says how far a long piece of work has come; nobody need be listening.
    pub fn say(&self, words: impl FnOnce() -> String) {
        if let Some(notes) = &self.notes {
            let _ = notes.send(words());
        }
    }
}

/// What came of making an object.
#[derive(Debug, Clone)]
pub struct Made {
    /// How it ended.
    pub outcome: ImportOutcome,
    /// What it is here.
    pub made: Option<ImportMade>,
    /// Why it failed or was left alone.
    pub message: Option<String>,
    /// The code of that reason.
    pub code: Option<String>,
    /// What came back changed.
    pub warnings: Vec<ImportWarning>,
    /// Who wrote it there, where that is shown beside it.
    pub author: Option<String>,
    /// What it carried: files, messages, turns, comments and versions.
    pub carried: Carried,
}

/// What a made object carried.
#[derive(Debug, Clone, Copy, Default)]
pub struct Carried {
    /// Files.
    pub files: i64,
    /// Messages.
    pub messages: i64,
    /// Turns.
    pub turns: i64,
    /// Comments.
    pub comments: i64,
    /// Earlier versions.
    pub versions: i64,
}

impl Made {
    /// An object that was made as the plan said.
    #[must_use]
    pub fn done(object: &Object, id: Id) -> Self {
        let outcome = match object.action {
            ImportAction::Replace => ImportOutcome::Replaced,
            ImportAction::KeepBoth => ImportOutcome::KeptBoth,
            _ => ImportOutcome::Created,
        };
        Self {
            outcome,
            made: Some(ImportMade {
                id,
                kind: object.made_as,
                node: None,
            }),
            message: None,
            code: None,
            warnings: Vec::new(),
            author: None,
            carried: Carried::default(),
        }
    }

    /// An object that was left as it was.
    #[must_use]
    pub fn left(outcome: ImportOutcome, message: Option<String>) -> Self {
        Self {
            outcome,
            made: None,
            message,
            code: None,
            warnings: Vec::new(),
            author: None,
            carried: Carried::default(),
        }
    }

    /// An object that could not be made.
    #[must_use]
    pub fn failed(code: &str, message: impl Into<String>) -> Self {
        let mut made = Self::left(ImportOutcome::Failed, Some(message.into()));
        made.code = Some(code.to_owned());
        made
    }

    /// Adds a thing that came back changed.
    pub fn warn(&mut self, code: &str, message: impl Into<String>) {
        self.warnings.push(ImportWarning {
            code: code.to_owned(),
            message: message.into(),
            params: None,
        });
    }
}

/// Why an object could not be made.
#[derive(Debug, thiserror::Error)]
pub enum MakeError {
    /// It cannot be made, and the sentence says why: the row of the report.
    #[error("{0}")]
    Refused(String),
    /// What the package holds of it does not match its hash: it is damaged, and nothing of it is made.
    #[error("{0} does not match its hash")]
    Damaged(String),
    /// It cannot be made, for a reason with a code the screens word.
    #[error("{message}")]
    Coded {
        /// The code.
        code: &'static str,
        /// The sentence.
        message: String,
    },
    /// Something the engine stands on failed: the batch stops.
    #[error(transparent)]
    Internal(#[from] ImportError),
}

impl From<DbError> for MakeError {
    fn from(error: DbError) -> Self {
        Self::Internal(error.into())
    }
}

impl From<crate::error::EngineError> for MakeError {
    fn from(error: crate::error::EngineError) -> Self {
        Self::Internal(error.into())
    }
}

impl From<orch_authz::Error> for MakeError {
    fn from(error: orch_authz::Error) -> Self {
        Self::Internal(error.into())
    }
}

impl From<crate::document_tree::Error> for MakeError {
    fn from(error: crate::document_tree::Error) -> Self {
        use crate::document_tree::Error as E;
        match error {
            E::Denied => Self::Refused("you may not make this here".to_owned()),
            E::Limit => Self::Refused("it would pass what a document may hold".to_owned()),
            E::NotFound => Self::Refused("what it lies in is not there any more".to_owned()),
            E::Conflict => Self::Refused("something of the same name is there".to_owned()),
            E::Invalid(message) => Self::Refused(message.to_owned()),
            other => Self::Internal(ImportError::Internal(
                crate::error::EngineError::unreadable(other.to_string()),
            )),
        }
    }
}

/// Refused with a sentence.
pub fn refuse<T>(message: impl Into<String>) -> Result<T, MakeError> {
    Err(MakeError::Refused(message.into()))
}

/// Refused for a reason with a code.
pub fn refuse_with<T>(code: &'static str, message: impl Into<String>) -> Result<T, MakeError> {
    Err(MakeError::Coded {
        code,
        message: message.into(),
    })
}

/// The row of `import_origins` for an object that was made or taken over.
#[must_use]
pub fn origin_of(
    batch: &Batch<'_>,
    object: &Object,
    id: Id,
    parent: Option<Id>,
    made_new: bool,
) -> OriginRow {
    let creator = object
        .object
        .creator
        .and_then(|who| {
            batch
                .plan
                .opened
                .principals
                .iter()
                .find(|each| each.id == who)
        })
        .map(|who| json!({ "name": who.display_name, "username": who.username }));
    OriginRow {
        scope_id: batch.plan.destination.id,
        source_id: object.object.id,
        kind: origin_kind(object.object.kind).to_owned(),
        object_id: id,
        parent_id: parent,
        created_by_import: made_new.then_some(batch.import),
        last_import: batch.import,
        source_name: Some(object.object.name.clone()),
        source_path: Some(
            object
                .path
                .iter()
                .cloned()
                .chain(std::iter::once(object.object.name.clone()))
                .collect::<Vec<_>>()
                .join("/"),
        ),
        source_sha256: object.object.sha256.clone(),
        source_updated_at: object.object.updated_at.map(|at| at.get()),
        source_creator: creator,
        imported_by: Some(batch.principal.user()),
        imported_at: batch.now,
    }
}

/// Writes the origin of an object.
pub async fn note_origin(
    conn: &mut Conn,
    batch: &Batch<'_>,
    object: &Object,
    id: Id,
    parent: Option<Id>,
    made_new: bool,
) -> Result<(), DbError> {
    let row = origin_of(batch, object, id, parent, made_new);
    rows::put_origin(conn, &row, !made_new).await
}

/// A name for something made beside what is there: `Name (2)`, and the first number whose name nothing of the kind holds.
pub async fn free_name(
    conn: &mut Conn,
    kind: PackageKind,
    lies_in: Id,
    container: Option<Id>,
    name: &str,
    longest: usize,
) -> Result<String, DbError> {
    let mut candidates: Vec<String> = Vec::with_capacity(31);
    let mut tried: std::collections::HashSet<String> = std::collections::HashSet::new();
    for _ in 2..=32 {
        let candidate = another_name(name, &tried, longest);
        tried.insert(candidate.to_lowercase());
        candidates.push(candidate);
    }
    let taken = match kind {
        PackageKind::Folder => rows::sibling_folders(conn, lies_in, container, &candidates).await?,
        PackageKind::Document => {
            rows::sibling_documents(conn, lies_in, container, &candidates).await?
        }
        other => {
            rows::sibling_nodes(
                conn,
                lies_in,
                super::model::node_word(other).unwrap_or("project"),
                &candidates,
            )
            .await?
        }
    };
    let used: std::collections::HashSet<String> = taken
        .into_iter()
        .map(|each| each.asked.to_lowercase())
        .collect();
    Ok(candidates
        .into_iter()
        .find(|candidate| !used.contains(&candidate.to_lowercase()))
        .unwrap_or_else(|| {
            let tail = orch_types::new_id().simple().to_string();
            let stem: String = name.chars().take(longest.saturating_sub(9)).collect();
            format!("{stem} ({})", &tail[..6])
        }))
}

/// The name a thing is made under beside what is there: its own where nothing of the kind has it, and otherwise the first `Name (n)` that
/// is free. What has the name may be a thing the person cannot see; it is not told of, and it does not stop the import.
pub async fn settle_name(
    conn: &mut Conn,
    kind: PackageKind,
    lies_in: Id,
    container: Option<Id>,
    name: &str,
    longest: usize,
) -> Result<String, DbError> {
    let asked = [name.to_owned()];
    let taken = match kind {
        PackageKind::Folder => rows::sibling_folders(conn, lies_in, container, &asked).await?,
        PackageKind::Document => rows::sibling_documents(conn, lies_in, container, &asked).await?,
        other => {
            rows::sibling_nodes(
                conn,
                lies_in,
                super::model::node_word(other).unwrap_or("project"),
                &asked,
            )
            .await?
        }
    };
    if taken.is_empty() {
        Ok(name.to_owned())
    } else {
        free_name(conn, kind, lies_in, container, name, longest).await
    }
}

/// Whether the importer may still make `object` where it goes. It is asked again as each object is made that does not lie in
/// something this import made, so that a role taken away during a long import is honoured from that moment on.
async fn may_still(tx: &mut Tx, batch: &Batch<'_>, object: &Object) -> Result<bool, MakeError> {
    if object.parent_made {
        return Ok(true);
    }
    let Some(place) = orch_db::queries::shares::places(tx, &[object.lies_in])
        .await?
        .into_iter()
        .next()
    else {
        return Ok(false);
    };
    let kind = crate::exports::node_kind(&place.kind);
    let Some(permission) = creation_right(kind, object.made_as) else {
        return Ok(true);
    };
    match orch_authz::nodes::check(
        tx,
        batch.principal,
        object.lies_in,
        permission,
        orch_authz::Act::Use,
        orch_authz::Policy::default(),
    )
    .await
    {
        Ok(orch_access_contract::decisions::Decision::Allowed { .. }) => Ok(true),
        Ok(orch_access_contract::decisions::Decision::Refused(_))
        | Err(orch_authz::Error::NotFound(_) | orch_authz::Error::Archived { .. }) => Ok(false),
        Err(error) => Err(error.into()),
    }
}

/// Makes one object, in a savepoint of its own.
pub async fn make_object(
    tx: &mut Tx,
    batch: &Batch<'_>,
    object: &Object,
) -> Result<Made, MakeError> {
    if !may_still(tx, batch, object).await? {
        return Ok(Made::failed(
            "import.forbidden",
            "you may not make this here any more".to_owned(),
        ));
    }
    sqlx::query("SAVEPOINT import_object")
        .execute(&mut **tx)
        .await
        .map_err(DbError::from)?;
    let made = match object.made_as {
        PackageKind::Org | PackageKind::Project | PackageKind::Topic => {
            places::make_place(tx, batch, object).await
        }
        PackageKind::Folder => places::make_folder(tx, batch, object).await,
        PackageKind::Document => documents::make_document(tx, batch, object).await,
        PackageKind::Chat => chats::make_chat(tx, batch, object).await,
        PackageKind::Thread => threads::make_thread(tx, batch, object).await,
    };
    match made {
        Ok(made) => {
            sqlx::query("RELEASE SAVEPOINT import_object")
                .execute(&mut **tx)
                .await
                .map_err(DbError::from)?;
            Ok(made)
        }
        Err(error @ (MakeError::Refused(_) | MakeError::Damaged(_) | MakeError::Coded { .. })) => {
            sqlx::query("ROLLBACK TO SAVEPOINT import_object")
                .execute(&mut **tx)
                .await
                .map_err(DbError::from)?;
            sqlx::query("RELEASE SAVEPOINT import_object")
                .execute(&mut **tx)
                .await
                .map_err(DbError::from)?;
            Ok(match error {
                MakeError::Damaged(entry) => Made::failed(
                    "import.damaged",
                    format!("{entry} does not match the hash the manifest gives it"),
                ),
                MakeError::Coded { code, message } => Made::failed(code, message),
                MakeError::Refused(message) => Made::failed("import.refused", message),
                MakeError::Internal(error) => return Err(MakeError::Internal(error)),
            })
        }
        Err(error) => Err(error),
    }
}
