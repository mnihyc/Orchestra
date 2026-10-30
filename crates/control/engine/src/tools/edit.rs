//! Make a document, replace one, or change part of one, by one command a call: on the state the agent read, under its name, and in the document's history like anybody's writing.
//!
//! `edit_document` makes a document and changes one, one `command` a call, and each command takes its own
//! arguments and no others: `create`, `rewrite`, `replace`, `insert`, `rename`, `describe`, `attach` and `detach`.
//! `describe` sets the description shown beside the document's title where documents are listed, as `create` may
//! at once; a description an agent writes is kept as a person's is, and automation leaves it alone. What it needs
//! is **Edit on the thing it changes**: on that document, to change it (by its own row, or by following its node's);
//! on the immediate document folder (or the node's virtual `docs` directory), to make one there, as making a file
//! takes the right to write its directory, and `docs.enabled` resolving true at its hierarchy anchor. A document an agent makes belongs to the first owner of
//! its thread, who answers for what the agent does; its blocks are the agent's, and it says that an agent made it. In
//! the same transaction the agents of that thread are named in the new document's own Edit row (`source =
//! 'creation'`), so that the agent reads, changes and replaces what it made also after the place it made it in was
//! closed to it.
//!
//! **What `replace` matches is what `read_shared` returned**: the document written as Markdown by the one renderer.
//! The passage is found in the top-level block it lies in (or the item of a list), and that block alone is changed;
//! the others stay as they are, with their colours, their columns and whatever else Markdown has no form for. A
//! passage that lies across blocks changes those blocks and no others. **`seq`** is the `entry.seq` of the read the
//! change was written on: with it, a change that would touch a block which changed since is refused (`changed`)
//! and the agent reads again. Replacing a whole document always takes it.
//!
//! A change is one batch of operations on the document's blocks under the limits of the place the document is in
//! (`docs.max_bytes`, `docs.max_blocks`); one that would pass a limit is refused with the limit and where it is set.
//! The people who have the document open see the blocks change at once, each with the agent's name. Naming a person
//! does not send a notice. Each change is audited
//! (`document.created_by_agent`, or `document.edited_by_agent` with what kind of change and how many blocks).
//! Files use the shared bounded workspace reader before the writing transaction; attach and detach name the
//! file in the same history, under the calling thread. Every access and limit is checked again before commit.
//!
//! **References.** A shared-tree path or `id:` in a Markdown link or in backquotes, with `#n` for a part, is resolved
//! from the calling thread as it may read now and kept as a typed reference (a block by its identifier); one that
//! does not resolve keeps its words, and the answer lists both (`references`). The document is matched as the agent
//! was given it, references in its own terms and those it may not read as numbered words, which a change written
//! around them gives back as the references they stand for.

use crate::document_tree as organization;
use crate::engine::Engine;
use crate::error::EngineError;
use crate::ids;
use crate::tools::definitions::{Code, Refusal};
use crate::tools::documents::{self, Pieces};
use crate::tools::edit_source::{Found, Imported};
use crate::tools::references::Resolution;
use crate::tools::shared::Target;
use crate::tools::tree::Where;
use crate::tools::workspace_files::{WorkspaceFile, path_refusal, same_workspace};
use crate::tools::{Answer, CallContext};
use orch_access_contract::audit::AuditAction;
use orch_api_contract::documents::{
    DOCUMENT_DESCRIPTION_MAX_CHARS, DOCUMENT_TITLE_MAX_CHARS, DocumentFileChange,
    DocumentFileChangeKind, DocumentOp, NewBlock, VersionCause,
};
use orch_db::queries::documents::batch::{self, Limits, Outcome, Rules};
use orch_db::queries::documents::{self as rows, DocumentRow, history};
use orch_db::queries::storage;
use orch_db::queries::threads::ThreadRow;
use orch_db::{Conn, DbError};
use orch_documents::convert::{self, Format};
use orch_documents::markdown::{self, Directory, FileRef, Local};
use orch_documents::merge::{self, Existing};
use orch_documents::references::{Form, Reference, Typed};
use orch_documents::versions::Writer;
use orch_settings::Place;
use orch_settings_contract::registry::keys;
use orch_types::{Id, Json, Time, new_id};
use serde::Deserialize;
use serde_json::json;
use std::collections::{BTreeSet, HashMap, HashSet};

/// How long ago the last version was cut before another is, for a change by an agent: the shipped value of the
/// window that people's edits are cut by.
pub const VERSION_WINDOW_SECS: i64 = 600;

/// The arguments of `edit_document`, as the schema lets them through.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct Arguments {
    pub(super) command: String,
    pub(super) path: String,
    pub(super) markdown: Option<String>,
    pub(super) from: Option<String>,
    pub(super) old: Option<String>,
    pub(super) new: Option<String>,
    pub(super) all: Option<bool>,
    pub(super) after: Option<String>,
    pub(super) title: Option<String>,
    pub(super) description: Option<String>,
    pub(super) name: Option<String>,
    pub(super) file: Option<String>,
    pub(super) overwrite: Option<bool>,
    pub(super) seq: Option<i64>,
    pub(super) format: Option<String>,
    pub(super) block: Option<String>,
    pub(super) place: Option<bool>,
    pub(super) caption: Option<String>,
    pub(super) edits: Option<Vec<Edit>>,
    pub(super) cancel: Option<bool>,
    pub(super) resume: Option<bool>,
}

/// One edit of a `batch`: the arguments of a passage or a block command, in the call's own terms.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Edit {
    command: String,
    block: Option<String>,
    markdown: Option<String>,
    old: Option<String>,
    new: Option<String>,
    all: Option<bool>,
    after: Option<String>,
}

/// The commands an edit of a `batch` may be.
const BATCH_COMMANDS: [&str; 6] = [
    "replace",
    "insert",
    "block_replace",
    "block_insert_after",
    "block_delete",
    "block_move_after",
];

/// Which arguments each command takes: the names it requires and the names it may have.
fn takes(command: &str) -> Option<(&'static [&'static str], &'static [&'static str])> {
    Some(match command {
        "create" => (
            &["path"],
            &["markdown", "from", "description", "file", "format"],
        ),
        "rewrite" => (&["path", "seq"], &["seq", "markdown", "file", "format"]),
        "replace" => (&["path", "old", "new"], &["all", "seq"]),
        "insert" => (&["path"], &["markdown", "file", "format", "after", "seq"]),
        "rename" => (&["path", "title"], &[]),
        "describe" => (&["path", "description"], &[]),
        "attach" => (
            &["path", "file"],
            &["name", "overwrite", "place", "after", "caption"],
        ),
        "detach" => (&["path", "name"], &[]),
        "block_replace" => (&["path", "block", "markdown"], &["seq"]),
        "block_insert_after" => (&["path", "block", "markdown"], &["seq"]),
        "block_delete" => (&["path", "block"], &["seq"]),
        "block_move_after" => (&["path", "block", "after"], &["seq"]),
        "batch" => (&["path", "edits", "seq"], &[]),
        "import" => (&["path", "file"], &["cancel", "resume"]),
        _ => return None,
    })
}

/// Checks that a command was given what it needs and nothing it does not take.
fn check_command(arguments: &Json) -> Result<(), Refusal> {
    let command = arguments
        .get("command")
        .and_then(Json::as_str)
        .unwrap_or_default();
    let Some((required, optional)) = takes(command) else {
        return Err(Refusal::invalid(
            "\"command\" is create, rewrite, replace, insert, rename, describe, attach, detach, block_replace, block_insert_after, block_delete, block_move_after, batch or import.",
        ));
    };
    let given: BTreeSet<&str> = arguments
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(_, value)| !value.is_null())
        .map(|(name, _)| name.as_str())
        .filter(|name| !matches!(*name, "command" | "max_bytes" | "ids"))
        .collect();
    // A file checked out of a document carries the sequence it was written at, so a commit need not repeat it.
    let required: Vec<&str> = required
        .iter()
        .copied()
        .filter(|name| !(command == "rewrite" && *name == "seq" && given.contains("file")))
        .collect();
    let takes_sentence = || {
        let mut names: Vec<String> = required
            .iter()
            .map(|name| format!("{name} (required)"))
            .collect();
        names.extend(optional.iter().map(|name| (*name).to_owned()));
        format!("{command} takes: {}.", names.join(", "))
    };
    if let Some(wrong) = given
        .iter()
        .find(|name| !required.contains(name) && !optional.contains(name))
    {
        return Err(Refusal::invalid(format!(
            "{command} does not take \"{wrong}\". {}",
            takes_sentence()
        )));
    }
    if let Some(missing) = required.iter().find(|name| !given.contains(*name)) {
        return Err(Refusal::invalid(format!(
            "{command} needs \"{missing}\". {}",
            takes_sentence()
        )));
    }
    if arguments["cancel"] == true && arguments["resume"] == true {
        return Err(Refusal::invalid(
            "import takes cancel or resume, one at a time.",
        ));
    }
    // The text of a create, a rewrite and an insert is given once: as markdown, or as a file of the workspace.
    let sources = ["markdown", "file", "from"]
        .iter()
        .filter(|name| given.contains(**name) && (command == "create" || **name != "from"))
        .count();
    if matches!(command, "create") && sources > 1 {
        return Err(Refusal::invalid(
            "create takes markdown, file or from, one of them.",
        ));
    }
    if matches!(command, "rewrite" | "insert") && sources != 1 {
        return Err(Refusal::invalid(format!(
            "{command} takes markdown or file, one of them."
        )));
    }
    if given.contains("format") && !given.contains("file") {
        return Err(Refusal::invalid(
            "format says what a file is written in, and goes with file.",
        ));
    }
    if command == "create" && given.contains("from") && given.contains("description") {
        return Err(Refusal::invalid(
            "create takes description with markdown; a copy made with from is described with describe afterwards",
        ));
    }
    let most = usize::try_from(DOCUMENT_DESCRIPTION_MAX_CHARS).unwrap_or(usize::MAX);
    if arguments
        .get("description")
        .and_then(Json::as_str)
        .is_some_and(|text| text.chars().count() > most)
    {
        return Err(Refusal::invalid(format!(
            "A description is at most {DOCUMENT_DESCRIPTION_MAX_CHARS} characters; its first line is what lists show."
        )));
    }
    Ok(())
}

/// What a command changed.
pub(super) struct Done {
    document: DocumentRow,
    created: bool,
    /// The references of what was written, resolved and kept as text; null where it named none.
    references: Json,
    /// What the reader and the merge said, to go into the answer; null where they said nothing.
    said: Json,
}

/// Bytes prepared without a transaction; their destination and sending workspace are rechecked at commit.
struct PreparedAttachment {
    document: Id,
    source: ThreadRow,
    name: String,
    file: WorkspaceFile,
}

/// The names `@username` stands for in a text.
struct People(HashMap<String, Id>);

impl Directory for People {
    fn user_id(&self, username: &str) -> Option<Id> {
        self.0.get(&username.to_lowercase()).copied()
    }
}

/// What the reader of an agent's text is told of the place it is written for: the files the document holds by their
/// names, which a text links as `<title>.files/<name>`, and the files read from the workspace beside the text, by
/// their path there, which become files of the document.
#[derive(Default)]
struct Scene {
    files: Vec<(String, FileRef)>,
    base: String,
    local: HashMap<String, FileRef>,
}

impl Scene {
    fn of(known: &documents::Known, source: Option<&Imported>) -> Self {
        Self {
            files: known.file_refs(),
            base: source.map_or_else(String::new, |source| source.base.clone()),
            local: source.map_or_else(HashMap::new, |source| {
                source
                    .found
                    .iter()
                    .map(|(path, found)| {
                        (
                            path.clone(),
                            FileRef {
                                id: found.id,
                                media_type: found.file.media_type.clone(),
                            },
                        )
                    })
                    .collect()
            }),
        }
    }
}

/// What the reader of an agent's Markdown is told: whom its `@` names stand for, what its references were resolved
/// to, which reference each numbered word that names nothing stands for in what the agent was given, and the
/// files the text can name.
struct Told<'a> {
    people: People,
    references: &'a Resolution,
    hidden: HashMap<usize, Typed>,
    hidden_names: Vec<(String, Typed)>,
    scene: &'a Scene,
}

impl Directory for Told<'_> {
    fn user_id(&self, username: &str) -> Option<Id> {
        self.people.user_id(username)
    }

    fn file(&self, name: &str) -> Option<FileRef> {
        self.scene
            .files
            .iter()
            .find(|(each, _)| each == name)
            .map(|(_, file)| file.clone())
    }

    fn local(&self, path: &str) -> Option<Local> {
        let joined = convert::join_path(&self.scene.base, path)?;
        self.scene.local.get(&joined).cloned().map(Local::File)
    }

    fn reference(&self, text: &str, form: Form) -> Reference {
        if let Some(typed) = self.references.typed(text) {
            return Reference::Typed(typed.clone());
        }
        if form == Form::Link && self.references.is_plain(text) {
            return Reference::Plain;
        }
        Reference::Untouched
    }

    fn hidden(&self, number: usize) -> Option<Typed> {
        self.hidden.get(&number).cloned()
    }

    fn hidden_names(&self) -> Vec<(String, Typed)> {
        self.hidden_names.clone()
    }
}

/// The references part of an answer: null where the text named none.
fn told_back(resolution: &Resolution) -> Json {
    if resolution.is_empty() {
        Json::Null
    } else {
        resolution.feedback()
    }
}

async fn people_in(conn: &mut Conn, text: &str) -> Result<People, DbError> {
    let names = markdown::mentioned_names(text);
    if names.is_empty() {
        return Ok(People(HashMap::new()));
    }
    Ok(People(
        rows::users_by_username(conn, &names)
            .await?
            .into_iter()
            .map(|(name, id)| (name.to_lowercase(), id))
            .collect(),
    ))
}

pub(super) fn creation_ops(blocks: Vec<NewBlock>, after: Option<Id>) -> Vec<DocumentOp> {
    let mut previous = after;
    blocks
        .into_iter()
        .map(|block| {
            let id = block.id;
            let op = DocumentOp::Insert {
                block,
                parent_id: None,
                after_id: previous,
            };
            previous = Some(id);
            op
        })
        .collect()
}

/// What a document may hold where it is, and when versions are cut.
pub(crate) struct Settings {
    pub(crate) enabled: bool,
    pub(crate) limits: Limits,
    pub(crate) rules: Rules,
    pub(crate) files: orch_documents::files::Limits,
}

pub(crate) async fn settings_at(conn: &mut Conn, node: Id) -> Result<Settings, EngineError> {
    let place = Place::Node(node);
    let number = |value: Option<i64>, fallback: i64| value.unwrap_or(fallback);
    Ok(Settings {
        enabled: orch_settings::get::<bool>(conn, place, keys::DOCS_ENABLED)
            .await?
            .unwrap_or(true),
        limits: Limits {
            max_bytes: number(
                orch_settings::get::<i64>(conn, place, keys::DOCS_MAX_BYTES).await?,
                i64::MAX,
            ),
            max_blocks: number(
                orch_settings::get::<i64>(conn, place, keys::DOCS_MAX_BLOCKS).await?,
                i64::MAX,
            ),
        },
        rules: Rules {
            window_secs: VERSION_WINDOW_SECS,
            kept: number(
                orch_settings::get::<i64>(conn, place, keys::DOCS_VERSIONS_KEPT).await?,
                i64::MAX,
            ),
            history_max_bytes: number(
                orch_settings::get::<i64>(conn, place, keys::DOCS_HISTORY_MAX_BYTES).await?,
                i64::MAX,
            ),
        },
        files: orch_documents::files::Limits {
            file_bytes: number(
                orch_settings::get::<i64>(conn, place, keys::DOCS_FILE_MAX_BYTES).await?,
                i64::MAX,
            ),
            files_bytes: number(
                orch_settings::get::<i64>(conn, place, keys::DOCS_FILES_MAX_BYTES).await?,
                i64::MAX,
            ),
        },
    })
}

fn fixed_limit(what: &str, value: &str) -> Refusal {
    Refusal::new(
        Code::Limit,
        format!(
            "A change would pass a limit the system fixes: {what} is at most {value}. Make the change smaller."
        ),
    )
}

fn file_limit(limit: orch_documents::files::TooLarge) -> Refusal {
    let (key, bytes) = match limit {
        orch_documents::files::TooLarge::File { limit } => (keys::DOCS_FILE_MAX_BYTES, limit),
        orch_documents::files::TooLarge::Files { limit } => (keys::DOCS_FILES_MAX_BYTES, limit),
    };
    Refusal::new(
        Code::Limit,
        format!(
            "This file would exceed {key} ({bytes} bytes) at the document's place. Choose a smaller file or ask that place's managers to raise the limit. Nothing was attached."
        ),
    )
}

fn file_applied(seq: i64) -> batch::Applied {
    batch::Applied {
        seq,
        touched: Vec::new(),
        named: Vec::new(),
        tally: orch_documents::versions::Tally::default(),
        cut: None,
        changed: true,
    }
}

/// The refusal for a batch that was not applied.
fn refused(outcome: Outcome, settings: &Settings) -> Refusal {
    match outcome {
        Outcome::Gone => Refusal::new(Code::NotFound, "That document is gone."),
        Outcome::Stale { .. } => Refusal::new(
            Code::Changed,
            "A block of the document changed while the change was being made. Read the document again with read_shared.",
        ),
        Outcome::Exists { .. } => {
            Refusal::invalid("A block of the change has an identifier in use.")
        }
        Outcome::Invalid(faults) => Refusal::invalid(
            faults
                .iter()
                .map(|fault| format!("{}: {}", fault.path, fault.message))
                .collect::<Vec<_>>()
                .join("; "),
        ),
        Outcome::BlockTooLarge { .. } => fixed_limit("one block", "64 KiB"),
        Outcome::TooDeep { .. } => fixed_limit("nesting", "16 levels"),
        Outcome::TooManyBlocks { limit } => Refusal::new(
            Code::Limit,
            format!(
                "A document may hold at most {limit} blocks here, and this change would pass it. The limit is the setting docs.max_blocks of the place the document is in; a person can raise it there. Make the document smaller, or split it."
            ),
        ),
        Outcome::TooLarge { limit } => Refusal::new(
            Code::Limit,
            format!(
                "A document may hold at most {limit} bytes of text here, and this change would pass it ({} now allowed). The limit is the setting docs.max_bytes of the place the document is in; a person can raise it there.",
                settings.limits.max_bytes
            ),
        ),
        Outcome::Applied(_) | Outcome::Repeated { .. } => {
            Refusal::invalid("A change that was kept is not refused.")
        }
    }
}

impl Engine {
    /// Authorize the destination first; then read the source with no transaction or pool lease held.
    async fn prepare_document_attachment(
        &self,
        thread: &ThreadRow,
        arguments: &Arguments,
    ) -> Result<Result<PreparedAttachment, Refusal>, EngineError> {
        let given = arguments.file.as_deref().unwrap_or_default();
        // A path the workspace does not take is refused as a path, with what to give instead, before a name is
        // taken from it: the agent may have given no name at all.
        if let Some(refusal) = path_refusal(given) {
            return Ok(Err(refusal));
        }
        let name = arguments
            .name
            .as_deref()
            .unwrap_or_else(|| given.rsplit('/').next().unwrap_or(given));
        let name = match orch_documents::files::clean_name(name) {
            Ok(name) if name.chars().count() <= orch_chat::attachments::NAME_MAX_CHARS => name,
            _ => {
                return Ok(Err(Refusal::invalid(
                    "Give a nonempty file name without slashes or control characters, at most 200 characters, as for an uploaded file.",
                )));
            }
        };
        let (document, source, largest) = {
            let mut conn = self.inner.db.acquire().await?;
            let source = orch_db::queries::threads::thread(&mut conn, thread.node_id)
                .await?
                .ok_or_else(|| EngineError::unreadable("the calling thread is gone"))?;
            let resolved = match self
                .resolve_write(&mut conn, source.node_id, &arguments.path, false)
                .await?
            {
                Ok(resolved) => resolved,
                Err(refusal) => return Ok(Err(refusal)),
            };
            let Target::Document { document } = resolved.target else {
                return Ok(Err(Refusal::new(
                    Code::WrongKind,
                    "attach needs a current document path; list_shared shows its docs directory.",
                )));
            };
            let Some(entry) = resolved.tree.document(document) else {
                return Ok(Err(Refusal::new(Code::NotFound, "That document is gone.")));
            };
            if !entry.standing.writes() || resolved.tree.reach().archived(entry.row.node_id) {
                return Ok(Err(Refusal::new(
                    Code::NotWritable,
                    "Attaching a file needs doc.edit on this live document, or its place where it follows. Ask a person to share it for editing.",
                )));
            }
            if arguments.overwrite != Some(true)
                && rows::file_named(&mut conn, document, &name)
                    .await?
                    .is_some()
            {
                return Ok(Err(Refusal::new(
                    Code::Exists,
                    format!(
                        "The document already holds a file named {name:?}. Pass overwrite: true to replace its bytes, or choose another name."
                    ),
                )));
            }
            let settings = settings_at(&mut conn, entry.row.node_id).await?;
            (document, source, settings.files.file_bytes)
        };
        let mut files = match self
            .read_workspace_files(&source, &[given.to_owned()], largest)
            .await?
        {
            Ok(files) => files,
            Err(refusal) => return Ok(Err(refusal)),
        };
        let file = files
            .pop()
            .ok_or_else(|| EngineError::unreadable("the workspace reader returned no file"))?;
        Ok(Ok(PreparedAttachment {
            document,
            source,
            name,
            file,
        }))
    }

    /// `edit_document`: the call of an agent.
    pub(crate) async fn edit_document_call(
        &self,
        call: &CallContext,
        thread: &ThreadRow,
    ) -> Result<Answer, EngineError> {
        if let Err(refusal) = check_command(&call.arguments) {
            return Ok(refusal.into());
        }
        let arguments: Arguments = match serde_json::from_value(call.arguments.clone()) {
            Ok(arguments) => arguments,
            Err(error) => return Ok(Refusal::invalid(error.to_string()).into()),
        };
        // A text that is a file of the workspace is read, with the files it names, before anything is changed.
        let source = match (&arguments.file, arguments.command.as_str()) {
            (Some(file), "create" | "rewrite" | "insert") => {
                match self
                    .read_source(thread, file, arguments.format.as_deref())
                    .await?
                {
                    Ok(source) => Some(source),
                    Err(refusal) => return Ok(refusal.into()),
                }
            }
            _ => None,
        };
        if arguments.command == "import" {
            return Box::pin(self.import_documents(thread, &arguments, call)).await;
        }
        let outcome = if arguments.command == "create" {
            Box::pin(self.create_document(thread, &arguments, call, source)).await?
        } else if arguments.command == "rename" {
            let title = arguments.title.as_deref().unwrap_or_default().trim();
            if title.is_empty()
                || title.chars().count()
                    > usize::try_from(DOCUMENT_TITLE_MAX_CHARS).unwrap_or(usize::MAX)
            {
                return Ok(Refusal::invalid(format!(
                    "The title of a document is from 1 to {DOCUMENT_TITLE_MAX_CHARS} characters."
                ))
                .into());
            }
            self.rename_document_for_edit(thread, call, &arguments.path, title)
                .await?
                .map(|(document, path)| {
                    (
                        Done {
                            document,
                            created: false,
                            references: Json::Null,
                            said: Json::Null,
                        },
                        path,
                    )
                })
        } else {
            let attachment = if arguments.command == "attach" {
                match self.prepare_document_attachment(thread, &arguments).await? {
                    Ok(file) => Some(file),
                    Err(refusal) => return Ok(refusal.into()),
                }
            } else {
                None
            };
            #[cfg(test)]
            if attachment.is_some() {
                super::source::tests::pause(call, super::source::tests::Point::Prepared).await;
            }
            self.change_document(thread, &arguments, attachment.as_ref(), source)
                .await?
        };
        let (done, path) = match outcome {
            Ok(done) => done,
            Err(refusal) => return Ok(refusal.into()),
        };
        let mut conn = self.inner.db.acquire().await?;
        let blocks = documents::live(&mut conn, done.document.id).await?;
        let known =
            documents::known_for(&mut conn, &blocks, &crate::written::files_beside(&path)).await?;
        let shown: Vec<_> = blocks.iter().map(documents::block_of).collect();
        let count = documents::pieces_of(&shown, &known).count;
        let files =
            i64::try_from(rows::files(&mut conn, done.document.id).await?.len()).unwrap_or(0);
        let mut answer = json!({
            "path": path,
            "id": done.document.id,
            "created": done.created,
            "seq": done.document.doc_seq,
            "version": done.document.version,
            "blocks": count,
            "files": files,
        });
        if !done.references.is_null() {
            answer["references"] = done.references.clone();
        }
        if let (Some(fields), Some(said)) = (answer.as_object_mut(), done.said.as_object()) {
            for (name, value) in said {
                fields.insert(name.clone(), value.clone());
            }
        }
        Ok(Answer::Done(answer))
    }

    /// Makes a document at a path in a docs directory.
    async fn create_document(
        &self,
        thread: &ThreadRow,
        arguments: &Arguments,
        call: &CallContext,
        mut source: Option<Imported>,
    ) -> Result<Result<(Done, String), Refusal>, EngineError> {
        if let Some(source) = &arguments.from {
            return Ok(Box::pin(self.copy_document_for_edit(
                thread,
                call,
                source,
                &arguments.path,
            ))
            .await?
            .map(|(document, path)| {
                (
                    Done {
                        document,
                        created: true,
                        references: Json::Null,
                        said: Json::Null,
                    },
                    path,
                )
            }));
        }
        let asked = arguments.path.trim().trim_end_matches('/');
        let Some((directory, last)) = asked.rsplit_once('/') else {
            return Ok(Err(Refusal::invalid(
                "The path of a new document is in a docs directory, ending in the title and .md, for example \"docs/Week 41.md\".",
            )));
        };
        let Some(title) = last.strip_suffix(".md") else {
            return Ok(Err(Refusal::invalid(
                "The path of a new document ends in its title and .md, for example \"docs/Week 41.md\".",
            )));
        };
        let title = title.trim();
        if title.is_empty()
            || title.chars().count()
                > usize::try_from(DOCUMENT_TITLE_MAX_CHARS).unwrap_or(usize::MAX)
        {
            return Ok(Err(Refusal::invalid(format!(
                "The title of a document is from 1 to {DOCUMENT_TITLE_MAX_CHARS} characters."
            ))));
        }
        let mut tx = orch_db::queries::transaction_guard::request_scope(
            orch_db::queries::transaction_guard::Scope::Creation,
            self.inner.db.begin(),
        )
        .await?;
        let current = orch_db::queries::threads::thread(&mut tx, thread.node_id)
            .await?
            .ok_or_else(|| EngineError::unreadable("the calling thread is gone"))?;
        let thread = &current;
        let conn = &mut *tx;
        let resolved = match self
            .resolve_write(conn, thread.node_id, &format!("{directory}/"), false)
            .await?
        {
            Ok(resolved) => resolved,
            Err(refusal) => return Ok(Err(refusal)),
        };
        let parent = match resolved.target {
            Target::Directory(Where::Docs(node)) => organization::Directory::NodeDocs(node),
            Target::Directory(Where::DocumentFolder(id)) => organization::Directory::Container(id),
            _ => {
                return Ok(Err(Refusal::new(
                    Code::WrongKind,
                    format!(
                        "{} is not a docs directory. A new document is made in the docs directory of a place that list_shared shows as writable.",
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
        if !writable {
            return Ok(Err(Refusal::new(
                Code::NotWritable,
                format!(
                    "You may read the documents in {} and not make one there: that takes the right doc.edit on the place itself, which a person can give you by sharing it with you for editing.",
                    resolved.path
                ),
            )));
        }
        if resolved.tree.reach().archived(node) {
            return Ok(Err(Refusal::new(
                Code::NotWritable,
                format!(
                    "{} is archived: documents there can be read and not made or changed.",
                    resolved.path
                ),
            )));
        }
        let name = format!("{title}.md");
        let id = ids::derived(
            "document",
            &[
                thread.node_id.as_bytes().as_slice(),
                call.call_ref.as_bytes(),
            ],
        );
        let settings = settings_at(conn, node).await?;
        if !settings.enabled {
            return Ok(Err(Refusal::new(
                Code::Limit,
                "Documents are switched off where that is (the setting docs.enabled), and a person can switch them on there.",
            )));
        }
        let text = match &source {
            Some(imported) => imported.text.clone(),
            None => arguments.markdown.clone().unwrap_or_default(),
        };
        let source_files: Option<HashMap<String, Found>> =
            source.as_ref().map(|imported| imported.found.clone());
        let now = self.inner.clock.now();
        let agent = agent_name(conn, thread).await?;
        let path = format!("{}{name}", resolved.path);
        if !orch_agent_contract::tool_answers::fits(
            "edit_document",
            &call.arguments,
            &json!({"path":path,"id":id,"created":true,"seq":i64::MAX,"version":i64::MAX,"blocks":i64::MAX,"files":i64::MAX}),
        ) {
            return Ok(Err(Refusal::new(
                Code::TooLarge,
                "The new document's complete outcome does not fit max_bytes. Nothing was changed; use a shorter path or a larger bound.",
            )));
        }
        let creation = organization::create_text_in(
            &mut tx,
            organization::Actor::Agent {
                thread: thread.node_id,
            },
            parent,
            id,
            title,
            Some(&name),
            Time::new(now),
        )
        .await;
        if matches!(&creation, Err(organization::Error::Conflict)) {
            let message = match self.resolve_write(&mut tx, thread.node_id, &path, false).await? {
                Ok(current) => match current.target {
                    Target::Document { document } => current.tree.document(document).map(|entry|
                        format!("A document already exists there at seq {}. Nothing was changed. Read it with read_shared, then use rewrite with its current seq to replace its text, or choose another name.",entry.row.doc_seq)),
                    _ => None,
                },
                Err(_) => None,
            }.unwrap_or_else(||"An entry already occupies that name. Nothing was changed; choose another name.".to_owned());
            return Ok(Err(Refusal::new(Code::Exists, message)));
        }
        let created = match crate::tools::manage::domain_result(
            creation,
            organization::Command::CreateFolder,
        )? {
            Ok(created) => created,
            Err(refusal) => return Ok(Err(refusal)),
        };
        if !created.created {
            let current = match self
                .resolve_write(
                    &mut tx,
                    thread.node_id,
                    &format!("id:{}", created.document.id),
                    false,
                )
                .await?
            {
                Ok(current) => current,
                Err(refusal) => return Ok(Err(refusal)),
            };
            tx.commit().await.map_err(DbError::from)?;
            return Ok(Ok((
                Done {
                    document: created.document,
                    created: false,
                    references: Json::Null,
                    said: Json::Null,
                },
                current.path,
            )));
        }
        let names = people_in(&mut tx, &text).await?;
        let resolution = self
            .resolve_references(&mut tx, thread.node_id, &text)
            .await?;
        let scene = Scene::of(&documents::Known::default(), source.as_ref());
        let told = Told {
            people: names,
            references: &resolution,
            hidden: HashMap::new(),
            hidden_names: Vec::new(),
            scene: &scene,
        };
        let mut said = Said::default();
        let read = match read_text(
            &told,
            source.take().map(|imported| imported.prepared),
            &text,
        ) {
            Ok(read) => read,
            Err(refusal) => return Ok(Err(refusal)),
        };
        said.take(&read);
        let ops = creation_ops(read.blocks, None);
        if let Some(description) = arguments
            .description
            .as_deref()
            .filter(|text| !text.trim().is_empty())
        {
            Box::pin(rows::describe(
                &mut tx,
                id,
                description,
                &Writer::Agent {
                    thread: thread.node_id,
                    agent: agent.clone(),
                },
                now,
            ))
            .await?
            .ok_or_else(|| EngineError::unreadable("a document just made is not there"))?;
        }
        let applied = if ops.is_empty() {
            batch::Applied {
                seq: 0,
                touched: Vec::new(),
                named: Vec::new(),
                tally: orch_documents::versions::Tally::default(),
                cut: None,
                changed: false,
            }
        } else {
            // The files the text named come into the document before the blocks that show them.
            let writer = Writer::Agent {
                thread: thread.node_id,
                agent: agent.clone(),
            };
            said.files = match self
                .hold_files(
                    &mut tx,
                    id,
                    thread.node_id,
                    &settings,
                    &writer,
                    now,
                    &said.embedded,
                    source_files.as_ref(),
                    &ops,
                )
                .await?
            {
                Ok(held) => held,
                Err(refusal) => return Ok(Err(refusal)),
            };
            match apply_chunks(&mut tx, id, thread, &agent, &ops, &settings, now).await? {
                Ok(applied) => applied,
                Err(refusal) => return Ok(Err(refusal)),
            }
        };
        let mut done = self
            .finish_edit(&mut tx, thread, id, "create", &applied, now)
            .await?;
        done.references = told_back(&resolution);
        done.said = said.json();
        tx.commit().await.map_err(DbError::from)?;
        Ok(Ok((done, path)))
    }

    /// Audits a change that was kept, and tells the people it names.
    pub(super) async fn finish_edit(
        &self,
        tx: &mut Conn,
        thread: &ThreadRow,
        document: Id,
        command: &str,
        applied: &batch::Applied,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Done, EngineError> {
        let row = rows::get(tx, document)
            .await?
            .ok_or_else(|| EngineError::unreadable("a document just changed is not there"))?;
        let created = command == "create";
        let tally = json!({
            "inserted": applied.tally.inserted,
            "updated": applied.tally.updated,
            "moved": applied.tally.moved,
            "removed": applied.tally.removed,
        });
        let mut entry = orch_db::audit::Entry::new(
            orch_db::audit::Actor::Agent(thread.node_id),
            if created {
                orch_db::audit::action!(AuditAction::DocumentCreatedByAgent).as_str()
            } else {
                orch_db::audit::action!(AuditAction::DocumentEditedByAgent).as_str()
            },
            "document",
            Some(document),
            if created {
                "An agent made a document."
            } else {
                "An agent changed a document."
            },
        );
        entry.history = if created {
            AuditAction::DocumentCreatedByAgent.is_history_line()
        } else {
            AuditAction::DocumentEditedByAgent.is_history_line()
        };
        entry.node_id = Some(row.node_id);
        entry.org_id = Some(row.org_id);
        entry.details = json!({ "command": command, "blocks": tally, "title": row.title });
        orch_db::audit::append(tx, &entry, &orch_db::audit::Origin::default(), now).await?;
        Ok(Done {
            document: row,
            created,
            references: Json::Null,
            said: Json::Null,
        })
    }

    /// Changes a document that is there.
    #[allow(
        clippy::too_many_lines,
        reason = "each command is told apart where it differs, around one transaction"
    )]
    async fn change_document(
        &self,
        thread: &ThreadRow,
        arguments: &Arguments,
        attachment: Option<&PreparedAttachment>,
        mut source: Option<Imported>,
    ) -> Result<Result<(Done, String), Refusal>, EngineError> {
        let source_files: Option<HashMap<String, Found>> =
            source.as_ref().map(|imported| imported.found.clone());
        let mut tx = self.inner.db.begin().await?;
        orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
        let current = if attachment.is_some() {
            orch_db::queries::threads::lock(&mut tx, thread.node_id).await?
        } else {
            orch_db::queries::threads::thread(&mut tx, thread.node_id).await?
        }
        .ok_or_else(|| EngineError::unreadable("the calling thread is gone"))?;
        let thread = &current;
        let conn = &mut *tx;
        let resolved = match self
            .resolve_write(conn, thread.node_id, &arguments.path, false)
            .await?
        {
            Ok(resolved) => resolved,
            Err(refusal) => return Ok(Err(refusal)),
        };
        let resolved = &resolved;
        let document = match &resolved.target {
            Target::DocumentFile { .. } | Target::ChatFile { .. } => {
                return Ok(Err(Refusal::new(
                    Code::WrongKind,
                    "This path names an attachment. Name its document to change text or use attach/detach for a document file.",
                )));
            }
            Target::Document { document } => *document,
            Target::Version { .. } => {
                return Ok(Err(Refusal::new(
                    Code::WrongKind,
                    format!(
                        "{} is an earlier version, which is read and not edited. Use manage_shared restore with this version path to put it back.",
                        resolved.path
                    ),
                )));
            }
            Target::NodeChat { .. } | Target::PeopleChat { .. } => {
                return Ok(Err(Refusal::new(
                    Code::WrongKind,
                    format!(
                        "{} is a chat, not a document. A chat is written in with send_message and target.kind \"chat\".",
                        resolved.path
                    ),
                )));
            }
            Target::Conversation { .. } => {
                return Ok(Err(Refusal::new(
                    Code::WrongKind,
                    format!(
                        "{} is a conversation, which is not changed. To hand something to its agent, use send_message with target.kind \"thread\".",
                        resolved.path
                    ),
                )));
            }
            Target::Directory(Where::DocumentFolder(_)) => {
                return Ok(Err(Refusal::new(
                    Code::WrongKind,
                    "This path names a document folder, not a text document. List its documents with list_shared; use manage_shared to copy, move, delete or restore the folder.",
                )));
            }
            Target::Directory(_) => {
                return Ok(Err(Refusal::new(
                    Code::WrongKind,
                    format!(
                        "{} is a directory. Name the document, as list_shared shows it, or use create for a new one.",
                        resolved.path
                    ),
                )));
            }
        };
        let Some(entry) = resolved.tree.document(document) else {
            return Ok(Err(Refusal::new(Code::NotFound, "That document is gone.")));
        };
        if !entry.standing.writes() {
            return Ok(Err(Refusal::new(
                Code::NotWritable,
                format!(
                    "You may read {} and not change it: that takes the right doc.edit on the document (or on its place, where it follows), which a person can give you by sharing it with you for editing.",
                    resolved.path
                ),
            )));
        }
        let node = entry.row.node_id;
        if resolved.tree.reach().archived(node) {
            return Ok(Err(Refusal::new(
                Code::NotWritable,
                format!(
                    "{} is archived: it can be read and not changed.",
                    resolved.path
                ),
            )));
        }
        let settings = settings_at(conn, node).await?;
        let now = self.inner.clock.now();
        let agent = agent_name(conn, thread).await?;
        let Some(row) = rows::lock(&mut tx, document)
            .await?
            .filter(|row| row.deleted_at.is_none())
        else {
            return Ok(Err(Refusal::new(Code::NotFound, "That document is gone.")));
        };
        let command = arguments.command.as_str();
        if command == "describe" {
            let description = arguments.description.as_deref().unwrap_or_default();
            if Box::pin(rows::describe(
                &mut tx,
                document,
                description,
                &Writer::Agent {
                    thread: thread.node_id,
                    agent: agent.clone(),
                },
                now,
            ))
            .await?
            .is_none()
            {
                return Ok(Err(Refusal::new(Code::NotFound, "That document is gone.")));
            }
            let applied = file_applied(row.doc_seq);
            let done = self
                .finish_edit(&mut tx, thread, document, command, &applied, now)
                .await?;
            tx.commit().await.map_err(DbError::from)?;
            return Ok(Ok((done, resolved.path.clone())));
        }
        if command == "attach" {
            let prepared = attachment
                .ok_or_else(|| EngineError::unreadable("the workspace file was not prepared"))?;
            if prepared.document != document || !same_workspace(&prepared.source, thread) {
                return Ok(Err(crate::tools::workspace_files::changed()));
            }
            let existing = rows::file_named(&mut tx, document, &prepared.name).await?;
            if existing.is_some() && arguments.overwrite != Some(true) {
                return Ok(Err(Refusal::new(
                    Code::Exists,
                    format!(
                        "The document already holds a file named {:?}. Pass overwrite: true to replace its bytes, or choose another name.",
                        prepared.name
                    ),
                )));
            }
            if let Err(too_large) = orch_documents::files::check_fits(
                prepared.file.size,
                row.files_bytes,
                existing.as_ref().map_or(0, |old| old.file.size),
                settings.files,
            ) {
                return Ok(Err(file_limit(too_large)));
            }
            orch_db::queries::uploads::lock_blobs(&mut tx, &[prepared.file.blob]).await?;
            storage::register_blob(&mut tx, &prepared.file.blob, prepared.file.size, now).await?;
            storage::pin_blobs(&mut tx, &[prepared.file.blob]).await?;
            let new = rows::NewFile {
                id: existing.as_ref().map_or_else(new_id, |old| old.file.id),
                document,
                blob: prepared.file.blob,
                name: &prepared.name,
                media_type: &prepared.file.media_type,
                size: prepared.file.size,
                is_text: prepared.file.is_text,
                added_by: None,
                added_by_thread: Some(thread.node_id),
                at: now,
            };
            if let Some(old) = &existing {
                rows::replace_file(&mut tx, &new).await?;
                storage::unpin_blobs(&mut tx, &[old.blob], now).await?;
            } else {
                rows::add_file(&mut tx, &new).await?;
            }
            history::record_file(
                &mut tx,
                document,
                &Writer::Agent {
                    thread: thread.node_id,
                    agent: agent.clone(),
                },
                DocumentFileChange {
                    name: prepared.name.clone(),
                    kind: if existing.is_some() {
                        DocumentFileChangeKind::Replaced
                    } else {
                        DocumentFileChangeKind::Added
                    },
                },
                settings.rules,
                now,
            )
            .await?;
            rows::touch(&mut tx, document).await?;
            let mut applied = file_applied(row.doc_seq);
            // The file is shown by a block, unless one shows it already or the agent asked for none.
            if arguments.place != Some(false) {
                let held_id = new.id;
                let live = rows::live_blocks(&mut tx, document).await?;
                let shown_already = live.iter().any(|block| {
                    block.props.get("file_id").and_then(Json::as_str)
                        == Some(held_id.to_string().as_str())
                });
                if !shown_already {
                    let seen: Vec<_> = live.iter().map(documents::block_of).collect();
                    let after = match arguments.after.as_deref() {
                        Some(text) => {
                            let known = documents::known_for(
                                &mut tx,
                                &live,
                                &crate::written::files_beside(&resolved.path),
                            )
                            .await?;
                            match anchor(&documents::pieces_of(&seen, &known), Some(text)) {
                                Ok(after) => after,
                                Err(refusal) => return Ok(Err(refusal)),
                            }
                        }
                        None => documents::top_level(&seen).last().map(|block| block.id),
                    };
                    let media = prepared.file.media_type.to_ascii_lowercase();
                    let block_type = if orch_documents::files::is_image(&media) {
                        orch_api_contract::documents::BlockType::Image
                    } else if media.starts_with("video/") {
                        orch_api_contract::documents::BlockType::Video
                    } else if media.starts_with("audio/") {
                        orch_api_contract::documents::BlockType::Audio
                    } else {
                        orch_api_contract::documents::BlockType::File
                    };
                    let caption = arguments
                        .caption
                        .as_deref()
                        .filter(|words| !words.trim().is_empty())
                        .map(|words| {
                            vec![orch_api_contract::documents::Inline::Text {
                                text: words.to_owned(),
                                marks: None,
                                href: None,
                                color: None,
                                background: None,
                            }]
                        })
                        .unwrap_or_default();
                    let block = NewBlock {
                        id: new_id(),
                        block_type,
                        props: serde_json::from_value(json!({ "file_id": held_id })).ok(),
                        content: Some(orch_api_contract::documents::BlockContent::Inline(caption)),
                        children: None,
                    };
                    let op = DocumentOp::Insert {
                        block,
                        parent_id: None,
                        after_id: after,
                    };
                    applied = match apply_chunks(
                        &mut tx,
                        document,
                        thread,
                        &agent,
                        std::slice::from_ref(&op),
                        &settings,
                        now,
                    )
                    .await?
                    {
                        Ok(applied) => applied,
                        Err(refusal) => return Ok(Err(refusal)),
                    };
                }
            }
            let done = self
                .finish_edit(&mut tx, thread, document, command, &applied, now)
                .await?;
            tx.commit().await.map_err(DbError::from)?;
            return Ok(Ok((done, resolved.path.clone())));
        }
        if command == "detach" {
            let name = arguments.name.clone().unwrap_or_default();
            let files = rows::files(&mut tx, document).await?;
            let Some(file) = files.iter().find(|file| file.name == name) else {
                return Ok(Err(Refusal::new(
                    Code::NotFound,
                    format!(
                        "The document holds no file named \"{name}\". It holds: {}.",
                        if files.is_empty() {
                            "no files".to_owned()
                        } else {
                            files
                                .iter()
                                .map(|file| format!("\"{}\"", file.name))
                                .collect::<Vec<_>>()
                                .join(", ")
                        }
                    ),
                )));
            };
            let blocks = rows::live_blocks(&mut tx, document).await?;
            let showing: Vec<Id> = blocks
                .iter()
                .filter(|block| {
                    block.props.get("file_id").and_then(Json::as_str)
                        == Some(file.id.to_string().as_str())
                })
                .map(|block| block.id)
                .collect();
            let mut applied = file_applied(row.doc_seq);
            if !showing.is_empty() {
                let ops: Vec<DocumentOp> = showing
                    .iter()
                    .map(|id| DocumentOp::Remove { id: *id })
                    .collect();
                let outcome =
                    apply_chunks(&mut tx, document, thread, &agent, &ops, &settings, now).await?;
                applied = match outcome {
                    Ok(applied) => applied,
                    Err(refusal) => return Ok(Err(refusal)),
                };
            }
            let held = rows::file(&mut tx, document, file.id)
                .await?
                .ok_or_else(|| EngineError::unreadable("a locked document file disappeared"))?;
            rows::remove_file(&mut tx, document, file.id).await?;
            storage::unpin_blobs(&mut tx, &[held.blob], now).await?;
            history::record_file(
                &mut tx,
                document,
                &Writer::Agent {
                    thread: thread.node_id,
                    agent: agent.clone(),
                },
                DocumentFileChange {
                    name: file.name.clone(),
                    kind: DocumentFileChangeKind::Removed,
                },
                settings.rules,
                now,
            )
            .await?;
            rows::touch(&mut tx, document).await?;
            let done = self
                .finish_edit(&mut tx, thread, document, command, &applied, now)
                .await?;
            tx.commit().await.map_err(DbError::from)?;
            return Ok(Ok((done, resolved.path.clone())));
        }
        // The blocks as they stand under the lock, written as this agent was given them.
        let stored = rows::live_blocks(&mut tx, document).await?;
        let order = documents::references_in(&stored);
        // The text the change is written in: the one that references and people are looked up in.
        let written: String = match command {
            "rewrite" | "insert" => match &source {
                Some(imported) => imported.text.clone(),
                None => arguments.markdown.clone().unwrap_or_default(),
            },
            "replace" => arguments.new.clone().unwrap_or_default(),
            "block_replace" | "block_insert_after" => {
                arguments.markdown.clone().unwrap_or_default()
            }
            "batch" => arguments
                .edits
                .iter()
                .flatten()
                .filter_map(|edit| edit.markdown.as_deref().or(edit.new.as_deref()))
                .collect::<Vec<_>>()
                .join("\n\n"),
            _ => String::new(),
        };
        let written = written.as_str();
        // The agent's whole reach is read only where the document or what it writes names something.
        let mut reach = if order.is_empty() && orch_documents::references::find(written).is_empty()
        {
            None
        } else {
            self.reach_of(&mut tx, thread.node_id).await?
        };
        let writings = match reach.as_mut() {
            Some(reach) => reach.writings(&mut tx, &order).await?,
            None => HashMap::new(),
        };
        let known = documents::known_for(
            &mut tx,
            &stored,
            &crate::written::files_beside(&resolved.path),
        )
        .await?
        .for_reader(writings, &order);
        // What the agent may not read it was given as numbered words; written around, they stay what they are.
        let hidden: HashMap<usize, Typed> = order
            .iter()
            .filter_map(|typed| {
                (1..=order.len())
                    .find(|number| known.hidden(*number) == Some(typed))
                    .map(|number| (number, typed.clone()))
            })
            .collect();
        let ambiguous = known.ambiguous_hidden_names();
        let has_ambiguous = |text: &str| ambiguous.iter().any(|words| text.contains(words));
        if command == "rewrite" && !ambiguous.is_empty() || has_ambiguous(written) {
            return Ok(Err(ambiguous_references()));
        }
        let hidden_names = known.hidden_names();
        let shown: Vec<_> = stored.iter().map(documents::block_of).collect();
        let pieces = documents::pieces_of(&shown, &known);
        let tree = merge::tree_of(&shown);
        // Where each top-level block stands, so that the stretches a change rewrites are found without a search each.
        let top: HashMap<Id, usize> = tree
            .iter()
            .enumerate()
            .map(|(at, block)| (block.id, at))
            .collect();
        let changed_after = |ids: &[Id], seq: Option<i64>| -> bool {
            seq.is_some_and(|seq| {
                // A set, so that a change of many blocks of a large document is checked in one pass.
                let touched: HashSet<&Id> = ids.iter().collect();
                stored
                    .iter()
                    .filter(|block| touched.contains(&block.id))
                    .any(|block| block.seq_updated > seq)
            })
        };
        let names = people_in(&mut tx, written).await?;
        // The references the agent wrote, for the answer; those of the text read back are resolved where it is.
        let resolution = match reach.as_mut() {
            Some(reach) => reach.resolve_text(&mut tx, written).await?,
            None => Resolution::loose_only(written),
        };
        let scene = Scene::of(&known, source.as_ref());
        let mut said = Said::default();
        let mut ops: Vec<DocumentOp> = Vec::new();
        let mut touched: Vec<Id> = Vec::new();
        match command {
            "rewrite" => {
                let checkout = source
                    .as_ref()
                    .and_then(|imported| imported.checkout.clone());
                if let Some(checkout) = &checkout
                    && checkout.document != document
                {
                    return Ok(Err(Refusal::invalid(format!(
                        "That file was checked out of another document ({}). Give the path of that document, or write the text of this one again.",
                        checkout.document
                    ))));
                }
                let Some(seq) = arguments
                    .seq
                    .or_else(|| checkout.as_ref().map(|each| each.seq))
                else {
                    return Ok(Err(Refusal::invalid(
                        "rewrite needs \"seq\": the seq of the read it was written on. A file made by download_shared carries it.",
                    )));
                };
                // A block that went since cannot be told from one that was never there.
                let removed_since =
                    orch_db::queries::shares::blocks_removed_since(&mut tx, document, seq).await?;
                if removed_since > 0 {
                    return Ok(Err(changed_refusal(&resolved.path, row.doc_seq)));
                }
                let told = Told {
                    people: names,
                    references: &resolution,
                    hidden: hidden.clone(),
                    hidden_names: hidden_names.clone(),
                    scene: &scene,
                };
                let read = match read_text(
                    &told,
                    source.take().map(|imported| imported.prepared),
                    written,
                ) {
                    Ok(read) => read,
                    Err(refusal) => return Ok(Err(refusal)),
                };
                said.take(&read);
                let merged = merge::plan(&tree, read.blocks, None, None, &known, known.beside());
                touched = touched_by(&merged, &tree);
                // Only what the change would touch has to be as it was read.
                if changed_after(&touched, Some(seq)) {
                    return Ok(Err(changed_refusal(&resolved.path, row.doc_seq)));
                }
                said.merged(&merged);
                ops = merged.ops;
                // A version is cut first, when anything changes.
                if !ops.is_empty() && !row.span().is_empty() {
                    history::cut(&mut tx, &row, VersionCause::Rewrite, now).await?;
                }
            }
            "replace" => {
                let old = arguments.old.clone().unwrap_or_default();
                let new = arguments.new.clone().unwrap_or_default();
                let plan = match replacement(&pieces, &old, &new, arguments.all == Some(true)) {
                    Ok(plan) => plan,
                    Err(refusal) => return Ok(Err(refusal)),
                };
                // A passage edit reparses whole blocks, including references beside the changed words.
                if plan.runs.iter().any(|run| {
                    has_ambiguous(&run.text)
                        || pieces.pieces.iter().any(|piece| {
                            run.blocks.contains(&piece.block) && has_ambiguous(&piece.text)
                        })
                }) {
                    return Ok(Err(ambiguous_references()));
                }
                let touching: Vec<Id> = plan
                    .runs
                    .iter()
                    .flat_map(|run| run.blocks.clone())
                    .collect();
                if changed_after(&touching, arguments.seq) {
                    return Ok(Err(changed_refusal(&resolved.path, row.doc_seq)));
                }
                // The blocks a run rewrites are read back whole: the references in the words around the passage are
                // the agent's own terms for what they were, and resolve to it again.
                let read_back: Vec<&str> = plan.runs.iter().map(|run| run.text.as_str()).collect();
                let again = match reach.as_mut() {
                    Some(reach) => reach.resolve_text(&mut tx, &read_back.join("\n\n")).await?,
                    None => Resolution::default(),
                };
                let told = Told {
                    people: names,
                    references: &again,
                    hidden: hidden.clone(),
                    hidden_names: hidden_names.clone(),
                    scene: &scene,
                };
                for run in &plan.runs {
                    let read = match read_text(&told, None, &run.text) {
                        Ok(read) => read,
                        Err(refusal) => return Ok(Err(refusal)),
                    };
                    said.take(&read);
                    let old_blocks = run_blocks(&tree, &top, &run.blocks);
                    let merged = merge::plan(
                        &old_blocks,
                        read.blocks,
                        None,
                        run.after,
                        &known,
                        known.beside(),
                    );
                    touched.extend(touched_by(&merged, &tree));
                    said.merged(&merged);
                    ops.extend(merged.ops);
                }
            }
            "insert" => {
                let after = match anchor(&pieces, arguments.after.as_deref()) {
                    Ok(after) => after,
                    Err(refusal) => return Ok(Err(refusal)),
                };
                if changed_after(&after.into_iter().collect::<Vec<_>>(), arguments.seq) {
                    return Ok(Err(changed_refusal(&resolved.path, row.doc_seq)));
                }
                let told = Told {
                    people: names,
                    references: &resolution,
                    hidden: HashMap::new(),
                    hidden_names: Vec::new(),
                    scene: &scene,
                };
                let read = match read_text(
                    &told,
                    source.take().map(|imported| imported.prepared),
                    written,
                ) {
                    Ok(read) => read,
                    Err(refusal) => return Ok(Err(refusal)),
                };
                said.take(&read);
                ops = creation_ops(read.blocks, after);
            }
            "block_replace" | "block_insert_after" | "block_delete" | "block_move_after" => {
                let told = Told {
                    people: names,
                    references: &resolution,
                    hidden: hidden.clone(),
                    hidden_names: hidden_names.clone(),
                    scene: &scene,
                };
                let edit = Edit {
                    command: command.to_owned(),
                    block: arguments.block.clone(),
                    markdown: arguments.markdown.clone(),
                    old: None,
                    new: None,
                    all: None,
                    after: arguments.after.clone(),
                };
                let made = match block_edit(&edit, &tree, &told, &known, &mut said) {
                    Ok(made) => made,
                    Err(refusal) => return Ok(Err(refusal)),
                };
                if changed_after(&made.touched, arguments.seq) {
                    return Ok(Err(changed_refusal(&resolved.path, row.doc_seq)));
                }
                ops = made.ops;
            }
            "batch" => {
                let seq = arguments.seq;
                let told = Told {
                    people: names,
                    references: &resolution,
                    hidden: hidden.clone(),
                    hidden_names: hidden_names.clone(),
                    scene: &scene,
                };
                let mut used: HashSet<Id> = HashSet::new();
                for (number, edit) in arguments.edits.iter().flatten().enumerate() {
                    let made =
                        match batch_edit(edit, &tree, &top, &pieces, &told, &known, &mut said) {
                            Ok(made) => made,
                            Err(refusal) => {
                                return Ok(Err(Refusal::new(
                                    refusal.code,
                                    format!("edits[{number}]: {}", refusal.message),
                                )));
                            }
                        };
                    if made.touched.iter().any(|id| used.contains(id)) {
                        return Ok(Err(Refusal::invalid(format!(
                            "edits[{number}] touches a block that an earlier edit of this batch touches. Put the two in one edit."
                        ))));
                    }
                    used.extend(made.touched.iter().copied());
                    touched.extend(made.touched);
                    ops.extend(made.ops);
                }
                if changed_after(&touched, seq) {
                    return Ok(Err(changed_refusal(&resolved.path, row.doc_seq)));
                }
            }
            _ => {}
        }
        if ops.is_empty() {
            let done = Done {
                document: row,
                created: false,
                references: Json::Null,
                said: said.json(),
            };
            tx.commit().await.map_err(DbError::from)?;
            return Ok(Ok((done, resolved.path.clone())));
        }
        // The files the text named come into the document before the blocks that show them.
        let writer = Writer::Agent {
            thread: thread.node_id,
            agent: agent.clone(),
        };
        let held = match self
            .hold_files(
                &mut tx,
                document,
                thread.node_id,
                &settings,
                &writer,
                now,
                &said.embedded,
                source_files.as_ref(),
                &ops,
            )
            .await?
        {
            Ok(held) => held,
            Err(refusal) => return Ok(Err(refusal)),
        };
        said.files = held;
        let outcome = apply_chunks(&mut tx, document, thread, &agent, &ops, &settings, now).await?;
        let applied = match outcome {
            Ok(applied) => applied,
            Err(refusal) => return Ok(Err(refusal)),
        };
        let mut done = self
            .finish_edit(&mut tx, thread, document, command, &applied, now)
            .await?;
        done.references = told_back(&resolution);
        done.said = said.json();
        tx.commit().await.map_err(DbError::from)?;
        Ok(Ok((done, resolved.path.clone())))
    }
}

// ---------------------------------------------------------------- what the reader said, and where a block is

/// What the reader said while an edit was made, for the answer.
#[derive(Default)]
struct Said {
    warnings: Vec<String>,
    missing: Vec<String>,
    embedded: Vec<orch_documents::markdown::Embedded>,
    kept: i64,
    changed: i64,
    removed: i64,
    inserted: i64,
    merged: bool,
    files: Vec<(String, Id)>,
}

impl Said {
    /// Takes what a reading said.
    fn take(&mut self, read: &markdown::Read) {
        for warning in &read.warnings {
            if !self.warnings.contains(&warning.message) {
                self.warnings.push(warning.message.clone());
            }
        }
        for path in &read.missing {
            if !self.missing.contains(path) {
                self.missing.push(path.clone());
            }
        }
        self.embedded.extend(read.embedded.iter().cloned());
    }

    /// Takes what a laying of new blocks against old ones did.
    fn merged(&mut self, merge: &merge::Merge) {
        self.merged = true;
        self.kept += merge.kept;
        self.changed += i64::try_from(merge.updated.len()).unwrap_or(i64::MAX);
        self.removed += i64::try_from(merge.removed.len()).unwrap_or(i64::MAX);
        self.inserted += merge.inserted;
    }

    /// The parts of an answer that this adds: `merged`, `files`, `missing` and `warnings`, where there are any.
    fn json(&self) -> Json {
        let mut out = serde_json::Map::new();
        if self.merged {
            out.insert(
                "merged".to_owned(),
                json!({
                    "kept": self.kept,
                    "changed": self.changed,
                    "removed": self.removed,
                    "inserted": self.inserted,
                }),
            );
        }
        if !self.files.is_empty() {
            out.insert(
                "added_files".to_owned(),
                json!(
                    self.files
                        .iter()
                        .map(|(name, id)| json!({ "name": name, "id": id }))
                        .collect::<Vec<_>>()
                ),
            );
        }
        if !self.missing.is_empty() {
            out.insert("missing".to_owned(), json!(self.missing));
        }
        if !self.warnings.is_empty() {
            out.insert("warnings".to_owned(), json!(self.warnings));
        }
        if out.is_empty() {
            Json::Null
        } else {
            Json::Object(out)
        }
    }
}

/// What the one reader makes of an agent's text: the source it was prepared from, or the Markdown itself.
fn read_text(
    told: &Told<'_>,
    prepared: Option<convert::Prepared>,
    text: &str,
) -> Result<markdown::Read, Refusal> {
    let prepared = match prepared {
        Some(prepared) => prepared,
        None => convert::prepare(Format::Markdown, text.as_bytes())
            .map_err(|error| Refusal::invalid(error.to_string()))?,
    };
    Ok(prepared.finish(told))
}

/// The operations of one edit, and the blocks it touches (with what is under them), for the check against `seq`.
struct Made {
    ops: Vec<DocumentOp>,
    touched: Vec<Id>,
}

/// A block of the document found by its address.
struct Located<'a> {
    block: &'a Existing,
    /// What it is under; none at the top level.
    parent: Option<Id>,
    /// The sibling before it.
    previous: Option<Id>,
}

fn bad_address(address: &str) -> Refusal {
    Refusal::invalid(format!(
        "{address:?} is not a block address. `#n` is the nth block as read_shared counts them, `#n.m` the mth block directly under it, and `#0` the start of the document."
    ))
}

/// The numbers of an address: `#3` is `[3]`, `#3.2` is `[3, 2]`.
fn parse_address(address: &str) -> Result<Vec<usize>, Refusal> {
    let digits = address
        .trim()
        .strip_prefix('#')
        .ok_or_else(|| bad_address(address))?;
    let numbers: Result<Vec<usize>, _> = digits.split('.').map(str::parse::<usize>).collect();
    numbers.map_err(|_| bad_address(address))
}

/// Finds the block at an address in the tree of the document.
fn locate<'a>(tree: &'a [Existing], address: &str) -> Result<Located<'a>, Refusal> {
    let numbers = parse_address(address)?;
    let mut level = tree;
    let mut parent: Option<Id> = None;
    let mut found: Option<Located<'a>> = None;
    for (depth, number) in numbers.iter().enumerate() {
        if *number == 0 || *number > level.len() {
            return Err(Refusal::new(
                Code::NoMatch,
                format!(
                    "{address} names no block: there {} {} here. Read the document again with read_shared.",
                    if level.len() == 1 { "is" } else { "are" },
                    if level.len() == 1 {
                        "1 block".to_owned()
                    } else {
                        format!("{} blocks", level.len())
                    }
                ),
            ));
        }
        let block = &level[number - 1];
        found = Some(Located {
            block,
            parent,
            previous: number.checked_sub(2).map(|before| level[before].id),
        });
        if depth + 1 < numbers.len() {
            parent = Some(block.id);
            level = &block.children;
        }
    }
    found.ok_or_else(|| bad_address(address))
}

/// What a new block goes after: the block at the address (with what it is under), or the start of the document for `#0`.
fn anchor_of(tree: &[Existing], address: &str) -> Result<(Option<Id>, Option<Id>), Refusal> {
    if parse_address(address)? == [0] {
        return Ok((None, None));
    }
    let at = locate(tree, address)?;
    Ok((at.parent, Some(at.block.id)))
}

/// The identifier of a block and of everything under it.
fn ids_under(block: &Existing, out: &mut Vec<Id>) {
    out.push(block.id);
    for child in &block.children {
        ids_under(child, out);
    }
}

fn index_of<'a>(tree: &'a [Existing], into: &mut HashMap<Id, &'a Existing>) {
    for block in tree {
        into.insert(block.id, block);
        index_of(&block.children, into);
    }
}

/// The blocks that a laying of new blocks against old ones changes: those given other words, and those taken away
/// with what is under them.
fn touched_by(merge: &merge::Merge, tree: &[Existing]) -> Vec<Id> {
    let mut index = HashMap::new();
    index_of(tree, &mut index);
    let mut out: Vec<Id> = merge.updated.clone();
    for id in &merge.removed {
        match index.get(id) {
            Some(block) => ids_under(block, &mut out),
            None => out.push(*id),
        }
    }
    out
}

/// The top-level blocks with these identifiers, with what is under them, in the order given.
fn run_blocks(tree: &[Existing], top: &HashMap<Id, usize>, ids: &[Id]) -> Vec<Existing> {
    ids.iter()
        .filter_map(|id| top.get(id).and_then(|at| tree.get(*at)).cloned())
        .collect()
}

/// The operations that put blocks under a parent, after a sibling, one after the other.
fn creation_ops_under(
    blocks: Vec<NewBlock>,
    parent: Option<Id>,
    after: Option<Id>,
) -> Vec<DocumentOp> {
    let mut previous = after;
    blocks
        .into_iter()
        .map(|block| {
            let id = block.id;
            let op = DocumentOp::Insert {
                block,
                parent_id: parent,
                after_id: previous,
            };
            previous = Some(id);
            op
        })
        .collect()
}

/// One of the block commands, made into operations.
fn block_edit(
    edit: &Edit,
    tree: &[Existing],
    told: &Told<'_>,
    known: &documents::Known,
    said: &mut Said,
) -> Result<Made, Refusal> {
    let command = edit.command.as_str();
    let address = edit.block.as_deref().ok_or_else(|| {
        Refusal::invalid(format!(
            "{command} needs \"block\": the address of a block."
        ))
    })?;
    let text = || {
        edit.markdown
            .as_deref()
            .ok_or_else(|| Refusal::invalid(format!("{command} needs \"markdown\".")))
    };
    match command {
        "block_replace" => {
            let at = locate(tree, address)?;
            let read = read_text(told, None, text()?)?;
            said.take(&read);
            let merged = merge::plan(
                std::slice::from_ref(at.block),
                read.blocks,
                at.parent,
                at.previous,
                known,
                known.beside(),
            );
            said.merged(&merged);
            let mut touched = Vec::new();
            ids_under(at.block, &mut touched);
            Ok(Made {
                ops: merged.ops,
                touched,
            })
        }
        "block_insert_after" => {
            let (parent, after) = anchor_of(tree, address)?;
            let read = read_text(told, None, text()?)?;
            said.take(&read);
            let inserted = i64::try_from(read.blocks.len()).unwrap_or(i64::MAX);
            said.merged = true;
            said.inserted += inserted;
            Ok(Made {
                ops: creation_ops_under(read.blocks, parent, after),
                touched: after.into_iter().collect(),
            })
        }
        "block_delete" => {
            let at = locate(tree, address)?;
            let mut touched = Vec::new();
            ids_under(at.block, &mut touched);
            said.merged = true;
            said.removed += 1;
            Ok(Made {
                ops: vec![DocumentOp::Remove { id: at.block.id }],
                touched,
            })
        }
        "block_move_after" => {
            let at = locate(tree, address)?;
            let anchor = edit.after.as_deref().ok_or_else(|| {
                Refusal::invalid("block_move_after needs \"after\": the address of the block it goes after, or #0.")
            })?;
            let (parent, after) = anchor_of(tree, anchor)?;
            let mut touched = Vec::new();
            ids_under(at.block, &mut touched);
            if after.is_some_and(|id| touched.contains(&id)) {
                return Err(Refusal::invalid(
                    "A block cannot be moved after a block that is under it.",
                ));
            }
            touched.extend(after);
            said.merged = true;
            Ok(Made {
                ops: vec![DocumentOp::Move {
                    id: at.block.id,
                    parent_id: parent,
                    after_id: after,
                }],
                touched,
            })
        }
        other => Err(Refusal::invalid(format!(
            "\"{other}\" is not an edit of a batch: replace, insert, block_replace, block_insert_after, block_delete or block_move_after."
        ))),
    }
}

/// One edit of a batch, made into operations against the document as it was read.
fn batch_edit(
    edit: &Edit,
    tree: &[Existing],
    top: &HashMap<Id, usize>,
    pieces: &Pieces,
    told: &Told<'_>,
    known: &documents::Known,
    said: &mut Said,
) -> Result<Made, Refusal> {
    if !BATCH_COMMANDS.contains(&edit.command.as_str()) {
        return Err(Refusal::invalid(format!(
            "\"{}\" is not an edit of a batch: replace, insert, block_replace, block_insert_after, block_delete or block_move_after.",
            edit.command
        )));
    }
    match edit.command.as_str() {
        "replace" => {
            let (old, new) = (
                edit.old.clone().unwrap_or_default(),
                edit.new.clone().unwrap_or_default(),
            );
            let plan = replacement(pieces, &old, &new, edit.all == Some(true))?;
            let mut made = Made {
                ops: Vec::new(),
                touched: Vec::new(),
            };
            for run in &plan.runs {
                let read = read_text(told, None, &run.text)?;
                said.take(&read);
                let old_blocks = run_blocks(tree, top, &run.blocks);
                let merged = merge::plan(
                    &old_blocks,
                    read.blocks,
                    None,
                    run.after,
                    known,
                    known.beside(),
                );
                said.merged(&merged);
                for block in &old_blocks {
                    ids_under(block, &mut made.touched);
                }
                made.ops.extend(merged.ops);
            }
            Ok(made)
        }
        "insert" => {
            let after = anchor(pieces, edit.after.as_deref())?;
            let text = edit
                .markdown
                .as_deref()
                .ok_or_else(|| Refusal::invalid("insert needs \"markdown\"."))?;
            let read = read_text(told, None, text)?;
            said.take(&read);
            said.merged = true;
            said.inserted += i64::try_from(read.blocks.len()).unwrap_or(i64::MAX);
            Ok(Made {
                ops: creation_ops(read.blocks, after),
                touched: after.into_iter().collect(),
            })
        }
        _ => block_edit(edit, tree, told, known, said),
    }
}

fn ambiguous_references() -> Refusal {
    Refusal::new(
        Code::Ambiguous,
        "Those shared words name several private references. Edit a block without them, or ask a person to give them distinct labels.",
    )
}

fn changed_refusal(path: &str, seq: i64) -> Refusal {
    Refusal::new(
        Code::Changed,
        format!(
            "{path} changed after the state you wrote this on (it is at seq {seq} now). Read it again with read_shared, and pass the new seq as seq."
        ),
    )
}

/// The name an agent's changes are shown under: its profile's.
pub(crate) async fn agent_name(conn: &mut Conn, thread: &ThreadRow) -> Result<String, EngineError> {
    Ok(match thread.agent_id {
        Some(agent) => orch_db::queries::shares::agent_names(conn, &[agent])
            .await?
            .remove(&agent)
            .unwrap_or_else(|| "Agent".to_owned()),
        None => "Agent".to_owned(),
    })
}

/// Applies operations as the agent, in batches of the size a batch may have, and says what the first came to.
pub(super) async fn apply_chunks(
    conn: &mut Conn,
    document: Id,
    thread: &ThreadRow,
    agent: &str,
    ops: &[DocumentOp],
    settings: &Settings,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<Result<batch::Applied, Refusal>, EngineError> {
    let writer = Writer::Agent {
        thread: thread.node_id,
        agent: agent.to_owned(),
    };
    if ops.is_empty() {
        return Ok(Err(Refusal::invalid("There is nothing to change.")));
    }
    match batch::apply_chunks(
        conn,
        document,
        &writer,
        ops,
        settings.limits,
        settings.rules,
        now,
    )
    .await?
    {
        Outcome::Applied(applied) => Ok(Ok(applied)),
        other => Ok(Err(refused(other, settings))),
    }
}

impl Engine {
    /// Makes files of the document of what a text named and carried: those read from the workspace, and the pictures
    /// the text held inside it. Only those that a block of the change shows are made; each within what the document's
    /// place allows, under a name the document does not have already.
    #[allow(
        clippy::too_many_arguments,
        reason = "the files are made in the transaction of the change, by the one that makes it, for the files its blocks name"
    )]
    pub(super) async fn hold_files(
        &self,
        tx: &mut Conn,
        document: Id,
        thread: Id,
        settings: &Settings,
        writer: &Writer,
        now: chrono::DateTime<chrono::Utc>,
        embedded: &[orch_documents::markdown::Embedded],
        found: Option<&HashMap<String, Found>>,
        ops: &[DocumentOp],
    ) -> Result<Result<Vec<(String, Id)>, Refusal>, EngineError> {
        struct Candidate {
            id: Id,
            name: String,
            blob: orch_types::Hash,
            size: i64,
            media_type: String,
            is_text: bool,
        }
        let used = file_ids_in(ops);
        if used.is_empty() {
            return Ok(Ok(Vec::new()));
        }
        let mut candidates: Vec<Candidate> = Vec::new();
        for each in found.into_iter().flat_map(HashMap::values) {
            if used.contains(&each.id) {
                candidates.push(Candidate {
                    id: each.id,
                    name: each.name.clone(),
                    blob: each.file.blob,
                    size: each.file.size,
                    media_type: each.file.media_type.clone(),
                    is_text: each.file.is_text,
                });
            }
        }
        for each in embedded {
            if !used.contains(&each.id) {
                continue;
            }
            let Some(store) = self.blobs() else {
                return Ok(Err(Refusal::new(
                    Code::Closed,
                    "The file store is not there. Nothing was changed. Try again.",
                )));
            };
            let hash = orch_types::Hash::of(&each.bytes);
            let size = i64::try_from(each.bytes.len()).unwrap_or(i64::MAX);
            store
                .put(
                    &hash,
                    Some(size),
                    orch_blob_contract::store::stream_of(vec![bytes::Bytes::from(
                        each.bytes.clone(),
                    )]),
                )
                .await
                .map_err(|error| EngineError::unreadable(error.to_string()))?;
            candidates.push(Candidate {
                id: each.id,
                name: each.name.clone(),
                blob: hash,
                size,
                media_type: each.media_type.clone(),
                is_text: false,
            });
        }
        if candidates.is_empty() {
            return Ok(Ok(Vec::new()));
        }
        let row = rows::get(tx, document)
            .await?
            .ok_or_else(|| EngineError::unreadable("a document just changed is not there"))?;
        let mut taken: HashSet<String> = rows::files(tx, document)
            .await?
            .into_iter()
            .map(|file| file.name)
            .collect();
        let mut held_bytes = row.files_bytes;
        let mut made = Vec::new();
        for each in candidates {
            if let Err(too_large) =
                orch_documents::files::check_fits(each.size, held_bytes, 0, settings.files)
            {
                return Ok(Err(file_limit(too_large)));
            }
            held_bytes = held_bytes.saturating_add(each.size);
            let name = orch_documents::files::unique_name(&mut taken, &each.name);
            orch_db::queries::uploads::lock_blobs(tx, &[each.blob]).await?;
            storage::register_blob(tx, &each.blob, each.size, now).await?;
            storage::pin_blobs(tx, &[each.blob]).await?;
            rows::add_file(
                tx,
                &rows::NewFile {
                    id: each.id,
                    document,
                    blob: each.blob,
                    name: &name,
                    media_type: &each.media_type,
                    size: each.size,
                    is_text: each.is_text,
                    added_by: None,
                    added_by_thread: Some(thread),
                    at: now,
                },
            )
            .await?;
            history::record_file(
                tx,
                document,
                writer,
                DocumentFileChange {
                    name: name.clone(),
                    kind: DocumentFileChangeKind::Added,
                },
                settings.rules,
                now,
            )
            .await?;
            made.push((name, each.id));
        }
        rows::touch(tx, document).await?;
        Ok(Ok(made))
    }
}

/// The files that the blocks of some operations name.
fn file_ids_in(ops: &[DocumentOp]) -> HashSet<Id> {
    use orch_api_contract::documents::BlockProps;
    fn named(props: Option<&BlockProps>, out: &mut HashSet<Id>) {
        match props {
            Some(BlockProps::Image(props)) => {
                out.insert(props.file_id);
            }
            Some(BlockProps::File(props)) => {
                out.insert(props.file_id);
            }
            Some(BlockProps::Media(props)) => out.extend(props.file_id),
            _ => {}
        }
    }
    fn walk(block: &NewBlock, out: &mut HashSet<Id>) {
        named(block.props.as_ref(), out);
        for child in block.children.iter().flatten() {
            walk(child, out);
        }
    }
    let mut out = HashSet::new();
    for op in ops {
        match op {
            DocumentOp::Insert { block, .. } => walk(block, &mut out),
            DocumentOp::Update { props, .. } => named(props.as_ref(), &mut out),
            DocumentOp::Move { .. } | DocumentOp::Remove { .. } => {}
        }
    }
    out
}

// ---------------------------------------------------------------- finding the passage

/// A stretch of consecutive top-level blocks that a change rewrites, and what it is rewritten as.
#[derive(Debug, PartialEq, Eq)]
struct Run {
    /// The top-level blocks it replaces.
    blocks: Vec<Id>,
    /// The Markdown they are replaced by.
    text: String,
    /// The block the new ones go after: the one before the run.
    after: Option<Id>,
}

#[derive(Debug, PartialEq, Eq)]
struct Plan {
    runs: Vec<Run>,
}

/// Where a piece stands in the whole text: from where its own text begins to where it ends, and where the gap
/// after it ends.
fn offsets(pieces: &Pieces) -> Vec<(usize, usize, usize)> {
    let mut out = Vec::with_capacity(pieces.pieces.len());
    let mut at = 0;
    for piece in &pieces.pieces {
        let end = at + piece.text.len();
        let next = end + piece.gap.text().len();
        out.push((at, end, next));
        at = next;
    }
    out
}

/// The blocks, by number, that the occurrences lie in, for a refusal to say where.
fn where_text(pieces: &Pieces, found: &[usize], length: usize) -> String {
    let places = offsets(pieces);
    let mut numbers: Vec<String> = Vec::new();
    for start in found {
        if let Some(at) = places
            .iter()
            .position(|(begin, _, next)| start >= begin && start < next)
        {
            numbers.push(format!("block {}", at + 1));
        }
        let _ = length;
    }
    numbers.dedup();
    numbers.join(", ")
}

/// Plans a replacement: which stretches of blocks are written again, and how.
fn replacement(pieces: &Pieces, old: &str, new: &str, all: bool) -> Result<Plan, Refusal> {
    if !pieces.cut {
        return Err(Refusal::invalid(
            "This document cannot be changed by passage: its blocks do not write out one by one. Replace the whole of it with rewrite.",
        ));
    }
    if old.is_empty() {
        return Err(Refusal::invalid(
            "\"old\" is the text to replace, and is not empty.",
        ));
    }
    let spans = crate::instructions::stored_spans(&pieces.whole, old);
    let found: Vec<usize> = spans.iter().map(|(start, _)| *start).collect();
    if found.is_empty() {
        return Err(Refusal::new(
            Code::NoMatch,
            "\"old\" does not occur in the document as read_shared gives it. Read the document again, and copy the text exactly.",
        ));
    }
    if found.len() > 1 && !all {
        return Err(Refusal::new(
            Code::Ambiguous,
            format!(
                "\"old\" occurs {} times, in {}. Give more of the text around it so that it occurs once, or pass all.",
                found.len(),
                where_text(pieces, &found, old.len())
            ),
        ));
    }
    let places = offsets(pieces);
    // The pieces each occurrence touches: where its text lies, and across a gap, both sides.
    let mut touched: BTreeSet<usize> = BTreeSet::new();
    for (start, end) in &spans {
        for (index, (begin, text_end, gap_end)) in places.iter().enumerate() {
            if *start < *text_end && *end > *begin {
                touched.insert(index);
            }
            if *start < *gap_end && *end > *text_end {
                touched.insert(index);
                if index + 1 < places.len() {
                    touched.insert(index + 1);
                }
            }
        }
    }
    // Consecutive pieces make one run.
    let mut runs: Vec<Run> = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for index in touched {
        if current.last().is_some_and(|last| index != last + 1) {
            groups.push(std::mem::take(&mut current));
        }
        current.push(index);
    }
    if !current.is_empty() {
        groups.push(current);
    }
    for group in groups {
        let (Some(first), Some(last)) = (group.first().copied(), group.last().copied()) else {
            continue;
        };
        let begin = places[first].0;
        let end = places[last].1;
        let mut text = String::new();
        let mut cursor = begin;
        for (start, stop) in spans
            .iter()
            .filter(|(start, stop)| *start >= begin && *stop <= end)
        {
            text.push_str(&pieces.whole[cursor..*start]);
            text.push_str(new);
            cursor = *stop;
            if !all {
                break;
            }
        }
        text.push_str(&pieces.whole[cursor..end]);
        runs.push(Run {
            blocks: group.iter().map(|at| pieces.pieces[*at].block).collect(),
            text,
            after: first
                .checked_sub(1)
                .map(|before| pieces.pieces[before].block),
        });
    }
    Ok(Plan { runs })
}

/// Where an insertion goes: after the top-level block that holds the text `after` (which occurs once), or at the
/// end. Nothing, for the start of an empty document.
fn anchor(pieces: &Pieces, after: Option<&str>) -> Result<Option<Id>, Refusal> {
    let Some(after) = after else {
        return Ok(pieces.pieces.last().map(|piece| piece.block));
    };
    if !pieces.cut {
        return Err(Refusal::invalid(
            "This document cannot be changed by passage: its blocks do not write out one by one. Replace the whole of it with rewrite.",
        ));
    }
    if after.is_empty() {
        return Err(Refusal::invalid(
            "\"after\" is text in the block the new text follows, and is not empty.",
        ));
    }
    let found: Vec<usize> = crate::instructions::stored_spans(&pieces.whole, after)
        .iter()
        .map(|(at, _)| *at)
        .collect();
    if found.is_empty() {
        return Err(Refusal::new(
            Code::NoMatch,
            "\"after\" does not occur in the document as read_shared gives it. Read the document again, and copy the text exactly.",
        ));
    }
    if found.len() > 1 {
        return Err(Refusal::new(
            Code::Ambiguous,
            format!(
                "\"after\" occurs {} times, in {}. Give more of the text so that it occurs once.",
                found.len(),
                where_text(pieces, &found, after.len())
            ),
        ));
    }
    let start = found[0];
    let places = offsets(pieces);
    let at = places
        .iter()
        .position(|(begin, _, next)| start >= *begin && start < *next)
        .unwrap_or(places.len().saturating_sub(1));
    Ok(pieces.pieces.get(at).map(|piece| piece.block))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::documents::{Gap, Piece};

    fn pieces(texts: &[(&str, Gap)]) -> Pieces {
        let list: Vec<Piece> = texts
            .iter()
            .map(|(text, gap)| Piece {
                block: new_id(),
                text: (*text).to_owned(),
                gap: *gap,
            })
            .collect();
        Pieces {
            whole: documents::join(&list),
            count: i64::try_from(list.len()).unwrap(),
            pieces: list,
            cut: true,
        }
    }

    #[test]
    fn each_command_takes_its_own_arguments_and_no_others() {
        assert!(check_command(&json!({ "command": "create", "path": "docs/A.md" })).is_ok());
        let wrong = check_command(&json!({ "command": "create", "path": "docs/A.md", "old": "x" }))
            .unwrap_err();
        assert!(
            wrong.message.contains("does not take \"old\"")
                && wrong
                    .message
                    .contains("create takes: path (required), markdown")
        );
        let missing =
            check_command(&json!({ "command": "rewrite", "path": "docs/A.md", "markdown": "x" }))
                .unwrap_err();
        assert!(missing.message.contains("needs \"seq\""));
        assert!(
            check_command(
                &json!({ "command": "replace", "path": "p", "old": "a", "new": "b", "all": true })
            )
            .is_ok()
        );
        assert!(check_command(&json!({ "command": "dance", "path": "p" })).is_err());
    }

    #[test]
    fn only_import_takes_cancel_or_resume_and_they_cannot_be_combined() {
        for option in ["cancel", "resume"] {
            let mut request = json!({"command":"import","path":"docs/","file":"site"});
            request[option] = json!(true);
            assert!(check_command(&request).is_ok());
            request["command"] = json!("attach");
            assert!(check_command(&request).is_err());
        }
        assert!(check_command(&json!({"command":"import","path":"docs/","file":"site","cancel":true,"resume":true})).is_err());
    }

    #[test]
    fn a_description_is_set_by_describe_or_given_at_create_and_is_bounded() {
        assert!(
            check_command(
                &json!({ "command": "describe", "path": "docs/A.md", "description": "What it is" })
            )
            .is_ok()
        );
        assert!(
            check_command(&json!({ "command": "describe", "path": "docs/A.md" }))
                .unwrap_err()
                .message
                .contains("needs \"description\"")
        );
        assert!(
            check_command(&json!({
                "command": "create", "path": "docs/A.md", "markdown": "x", "description": "What it is"
            }))
            .is_ok()
        );
        assert!(
            check_command(
                &json!({ "command": "rename", "path": "docs/A.md", "title": "B", "description": "x" })
            )
            .is_err(),
            "only create and describe take a description"
        );
        assert!(
            check_command(&json!({
                "command": "create", "path": "docs/A.md", "from": "docs/B.md", "description": "x"
            }))
            .is_err()
        );
        let long = "x".repeat(usize::try_from(DOCUMENT_DESCRIPTION_MAX_CHARS).unwrap() + 1);
        assert!(
            check_command(
                &json!({ "command": "describe", "path": "docs/A.md", "description": long })
            )
            .unwrap_err()
            .message
            .contains("at most 4000 characters")
        );
    }

    #[test]
    fn a_passage_in_one_block_changes_that_block_and_no_other() {
        let p = pieces(&[
            ("First we build.", Gap::Paragraph),
            ("Then we ship.", Gap::Paragraph),
            ("Last, rest.", Gap::Paragraph),
        ]);
        let plan = replacement(&p, "ship", "test and ship", false).unwrap();
        assert_eq!(plan.runs.len(), 1);
        assert_eq!(plan.runs[0].text, "Then we test and ship.");
        assert_eq!(plan.runs[0].blocks, vec![p.pieces[1].block]);
        assert_eq!(plan.runs[0].after, Some(p.pieces[0].block));
    }

    #[test]
    fn a_passage_that_occurs_twice_is_refused_unless_all_is_asked_and_says_where() {
        let p = pieces(&[
            ("a cat", Gap::Paragraph),
            ("b", Gap::Paragraph),
            ("another cat", Gap::Paragraph),
        ]);
        let refusal = replacement(&p, "cat", "dog", false).unwrap_err();
        assert_eq!(refusal.code, Code::Ambiguous);
        assert!(
            refusal.message.contains("2 times")
                && refusal.message.contains("block 1")
                && refusal.message.contains("block 3"),
            "{}",
            refusal.message
        );
        let plan = replacement(&p, "cat", "dog", true).unwrap();
        assert_eq!(
            plan.runs.len(),
            2,
            "two blocks that are not neighbours are two runs"
        );
        let none = replacement(&p, "fish", "x", false).unwrap_err();
        assert_eq!(none.code, Code::NoMatch);
    }

    #[test]
    fn a_passage_across_blocks_changes_those_blocks_as_one_run() {
        let p = pieces(&[
            ("one", Gap::Paragraph),
            ("two", Gap::Paragraph),
            ("three", Gap::Paragraph),
        ]);
        let plan = replacement(&p, "one\n\ntwo", "one and two", false).unwrap();
        assert_eq!(plan.runs.len(), 1);
        assert_eq!(plan.runs[0].blocks.len(), 2);
        assert_eq!(plan.runs[0].text, "one and two");
        assert_eq!(plan.runs[0].after, None);
    }

    #[test]
    fn an_item_of_a_list_is_changed_alone() {
        let p = pieces(&[
            ("- milk", Gap::List),
            ("- eggs", Gap::List),
            ("- tea", Gap::Paragraph),
            ("After.", Gap::Paragraph),
        ]);
        let plan = replacement(&p, "eggs", "bread", false).unwrap();
        assert_eq!(plan.runs[0].blocks, vec![p.pieces[1].block]);
        assert_eq!(plan.runs[0].text, "- bread");
    }

    #[test]
    fn an_insertion_goes_after_the_block_that_holds_a_text_that_occurs_once_or_at_the_end() {
        let p = pieces(&[
            ("alpha", Gap::Paragraph),
            ("beta", Gap::Paragraph),
            ("gamma", Gap::Paragraph),
        ]);
        assert_eq!(anchor(&p, Some("beta")).unwrap(), Some(p.pieces[1].block));
        assert_eq!(anchor(&p, None).unwrap(), Some(p.pieces[2].block));
        assert_eq!(anchor(&p, Some("zeta")).unwrap_err().code, Code::NoMatch);
        let twice = pieces(&[("x", Gap::Paragraph), ("x", Gap::Paragraph)]);
        assert_eq!(anchor(&twice, Some("x")).unwrap_err().code, Code::Ambiguous);
    }

    #[test]
    fn a_document_that_could_not_be_cut_is_not_changed_by_passage() {
        let mut p = pieces(&[("alpha", Gap::Paragraph)]);
        p.cut = false;
        assert_eq!(
            replacement(&p, "a", "b", false).unwrap_err().code,
            Code::Invalid
        );
        assert_eq!(anchor(&p, Some("a")).unwrap_err().code, Code::Invalid);
    }
}
