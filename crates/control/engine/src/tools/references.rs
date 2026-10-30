//! References in what an agent writes and reads: found in its Markdown, resolved from its own thread into typed references with their parts, told back in two lists, and written out for each agent that reads them in its own paths.
//!
//! An agent names a thing of its shared tree as the destination of a Markdown link or in backquotes, a path or
//! `id:` with an optional `#n` (`orch_documents::references`). What it writes is resolved as it reads: from its
//! own thread, with what it may read now. A reference that resolves is kept typed (what kind of thing, which, and
//! which part; a document's part by the identifier of its block); one that does not is kept as the words it was
//! written in. The answer of the call lists both, so that a dead link is never silent.
//!
//! What another agent reads is written for it: each typed reference as a link with the thing's current name and that
//! reader's own path to it (relative to its own topic where the thing lies in it), with the number its part has now,
//! or, where it may not read the thing, as words that name nothing.

use crate::commands::{Outcome, Outgoing};
use crate::engine::Engine;
use crate::error::EngineError;
use crate::tools::documents::{self, Known};
use crate::tools::shared::Asking;
use crate::tools::tree::{self, FoundEntry, Named, Scope, Tree, Where};
use orch_api_contract::documents::{InlineRefKind, RefPart};
use orch_api_contract::workspace::{DirEntryKind, FileStat};
use orch_db::Conn;
use orch_db::queries::documents::BlockRow;
use orch_documents::references::{self, Form, Typed, Writing, Written};
use orch_link_contract::commands::{FsStat, SerialKey};
use orch_types::{Id, Json};
use serde_json::json;
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};

/// The most references one text resolves: a message or a change names a handful, and a text that names more keeps
/// the rest as written, with that reason, so that one call cannot keep the tree searched without end.
pub const REFERENCES_MAX: usize = 50;

/// The most distinct references one answer writes out for its reader: a document that names more is a list of
/// links rather than a text, and the rest of them are written as words that name nothing.
pub const WRITTEN_MAX: usize = 500;

/// Why a reference was kept as words, as the answer says it.
const NOTHING: &str = "nothing you may read is at that path";
const NOT_A_THING: &str = "it names a directory or a version; a reference names a place, a thread, a chat, a document or a file";
const NO_PARTS: &str = "a place has no numbered parts; name its thread, chat or document";
const TWINS: &str =
    "it names more than one thing; use the suffixed path or id: of the one you mean";
const TOO_MANY: &str = "more references than one text resolves; name fewer in one call";
const LOOSE: &str = "Markdown does not read it as a link: write a destination that holds a space or a parenthesis in angle brackets, [name](<path#n>)";

/// What the references of a text came to.
#[derive(Debug, Default)]
pub(crate) struct Resolution {
    /// The typed reference of each text that resolved, by the text as written.
    typed: HashMap<String, Typed>,
    /// The destinations of links that did not resolve: they stay as their words.
    plain: HashSet<String>,
    /// Each one that resolved: the path as written, the kind and the name of what it names, and its part.
    resolved: Vec<Json>,
    /// Each one kept as text, and why.
    kept: Vec<Json>,
}

impl Resolution {
    /// The typed reference a text resolved to.
    #[must_use]
    pub(crate) fn typed(&self, text: &str) -> Option<&Typed> {
        self.typed.get(text.trim())
    }

    /// Stores typed identities while keeping failed links as words.
    pub(crate) fn store(&self, markdown: &str) -> String {
        orch_documents::references::stored_with(markdown, &|found| match self.typed(&found.text) {
            Some(typed) => orch_documents::references::Reference::Typed(typed.clone()),
            None if found.form == orch_documents::references::Form::Link
                && self.is_plain(&found.text) =>
            {
                orch_documents::references::Reference::Plain
            }
            None => orch_documents::references::Reference::Untouched,
        })
    }

    /// Whether a link's destination was meant as a reference and did not resolve: it is kept as its words.
    #[must_use]
    pub(crate) fn is_plain(&self, text: &str) -> bool {
        self.plain.contains(text.trim())
    }

    /// What the answer of the call tells of them: those resolved and those kept as text.
    #[must_use]
    pub(crate) fn feedback(&self) -> Json {
        json!({ "resolved": self.resolved, "kept": self.kept })
    }

    /// Whether the text named no reference at all.
    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.resolved.is_empty() && self.kept.is_empty()
    }

    /// What a text that names no reference is told: each link Markdown does not read as one, kept as words.
    #[must_use]
    pub(crate) fn loose_only(markdown: &str) -> Self {
        let mut resolution = Self::default();
        resolution.tell_loose(markdown);
        resolution
    }

    /// Tells each link of `markdown` that Markdown does not read as one: it stays words, and why is said.
    fn tell_loose(&mut self, markdown: &str) {
        for text in references::loose(markdown) {
            self.kept.push(json!({ "text": text, "reason": LOOSE }));
        }
    }
}

/// A thing found for a reference, as the resolver tells it.
struct Hit {
    typed: Typed,
    kind: &'static str,
    name: String,
    part: Option<i64>,
}

/// The agent of a thread with everything it reaches now, read once for all the references of one call.
pub(crate) struct Reach {
    engine: Engine,
    asking: Asking,
    tree: Tree,
    /// The path of the agent's own topic, from which a path without `/` is read.
    topic: Option<String>,
    /// The top-level blocks of the documents looked at, in the order of the page, for parts.
    blocks: HashMap<Id, Vec<(Id, Option<Id>)>>,
    workspace: Option<Workspace>,
    /// Normalized paths share one native stat even when written in several forms or at several lines.
    files: HashMap<String, bool>,
}

struct Workspace {
    id: Id,
    machine: Option<Id>,
    root: String,
    directory: String,
    live: bool,
}

impl Engine {
    /// The reach of a thread's agent, read whole: nothing where the thread is gone.
    pub(crate) async fn reach_of(
        &self,
        conn: &mut Conn,
        thread: Id,
    ) -> Result<Option<Reach>, EngineError> {
        let Some(asking) = self.asking(conn, thread).await? else {
            return Ok(None);
        };
        let tree = Tree::load(
            conn,
            asking.agent.clone(),
            asking.policy,
            asking.documents_policy,
            asking.now,
            Scope::Everything,
        )
        .await?;
        let topic = tree.path_of_node(asking.agent.topic);
        let workspace = match orch_db::queries::threads::thread(conn, thread).await? {
            Some(row) => match row.environment_id {
                Some(id) => match orch_db::queries::environments::workspace::environment(conn, id)
                    .await?
                {
                    Some(env) => {
                        let cwd = orch_settings::get::<String>(
                            conn,
                            orch_settings::Place::Node(thread),
                            orch_settings_contract::registry::keys::THREAD_CWD,
                        )
                        .await?
                        .unwrap_or_default();
                        let online = match env.machine_id {
                            Some(machine) => orch_db::queries::resources::machine(conn, machine)
                                .await?
                                .is_some_and(|row| row.online && row.is_active()),
                            None => false,
                        };
                        Some(Workspace {
                            id,
                            machine: env.machine_id,
                            root: crate::sessions::workspace_root(
                                &env.workspace_root,
                                &env.facts,
                                env.generation,
                            )
                            .replace('\\', "/"),
                            directory: crate::sessions::workspace_path(&row.workspace_dir, &cwd)
                                .replace('\\', "/"),
                            live: online && env.status == "running",
                        })
                    }
                    None => None,
                },
                None => None,
            },
            None => None,
        };
        Ok(Some(Reach {
            engine: self.clone(),
            asking,
            tree,
            topic,
            blocks: HashMap::new(),
            workspace,
            files: HashMap::new(),
        }))
    }

    /// Finds the references an agent wrote in `markdown` and resolves them from its thread, as it may read now.
    pub(crate) async fn resolve_references(
        &self,
        conn: &mut Conn,
        thread: Id,
        markdown: &str,
    ) -> Result<Resolution, EngineError> {
        if references::find(markdown).is_empty() {
            return Ok(Resolution::loose_only(markdown));
        }
        match self.reach_of(conn, thread).await? {
            Some(mut reach) => reach.resolve_text(conn, markdown).await,
            None => Ok(Resolution::loose_only(markdown)),
        }
    }

    /// How each of `wanted` is written for the agent of `thread`: a link in its own terms where it may read the thing;
    /// left out (words that name nothing) where it may not, or where the thread is gone.
    pub(crate) async fn writings_for(
        &self,
        conn: &mut Conn,
        thread: Id,
        wanted: &[Typed],
    ) -> Result<HashMap<Typed, Writing>, EngineError> {
        if wanted.is_empty() {
            return Ok(HashMap::new());
        }
        match self.reach_of(conn, thread).await? {
            Some(mut reach) => reach.writings(conn, wanted).await,
            None => Ok(HashMap::new()),
        }
    }

    /// What a document names, and its references written for the agent of `thread`, as `read_shared` gives it to
    /// that agent and `edit_document` matches what it quotes.
    pub(crate) async fn known_for_reader(
        &self,
        conn: &mut Conn,
        thread: Id,
        rows: &[BlockRow],
        beside: &str,
    ) -> Result<Known, EngineError> {
        let known = documents::known_for(conn, rows, beside).await?;
        let order = documents::references_in(rows);
        let writings = self.writings_for(conn, thread, &order).await?;
        Ok(known.for_reader(writings, &order))
    }
}

impl Reach {
    /// Finds the references written in `markdown` and resolves them as this agent may read now.
    pub(crate) async fn resolve_text(
        &mut self,
        conn: &mut Conn,
        markdown: &str,
    ) -> Result<Resolution, EngineError> {
        let found = references::find(markdown);
        let mut resolution = Resolution::default();
        let mut seen: HashSet<String> = HashSet::new();
        for one in found {
            let text = one.text.trim().to_owned();
            if !seen.insert(text.clone()) {
                continue;
            }
            let Some(written) = references::parse(&text) else {
                continue;
            };
            let told = one.form == Form::Link
                || written.path.starts_with(references::ID_PREFIX)
                || written.part.is_some();
            let outcome = if seen.len() > REFERENCES_MAX {
                Err(TOO_MANY)
            } else {
                self.resolve(conn, &written, &text).await?
            };
            match outcome {
                Ok(named) => {
                    let mut entry = json!({ "path": text, "kind": named.kind, "name": named.name });
                    if let Some(part) = named.part {
                        entry["part"] = json!(part);
                    }
                    resolution.resolved.push(entry);
                    resolution.typed.insert(text, named.typed);
                }
                Err(reason) => {
                    if told {
                        resolution
                            .kept
                            .push(json!({ "text": text, "reason": reason }));
                    }
                    if one.form == Form::Link {
                        resolution.plain.insert(text);
                    }
                }
            }
        }
        resolution.tell_loose(markdown);
        Ok(resolution)
    }

    /// How each of `wanted` is written for this agent: a link in its own terms where it may read the thing; left out
    /// where it may not.
    pub(crate) async fn writings(
        &mut self,
        conn: &mut Conn,
        wanted: &[Typed],
    ) -> Result<HashMap<Typed, Writing>, EngineError> {
        let mut out = HashMap::new();
        let mut looked: HashSet<&Typed> = HashSet::new();
        for typed in wanted {
            if looked.len() >= WRITTEN_MAX {
                break;
            }
            if !looked.insert(typed) {
                continue;
            }
            if let Some(writing) = self.writing(conn, typed).await? {
                out.insert(typed.clone(), writing);
            }
        }
        Ok(out)
    }

    /// A path as the agent's own: relative to its topic where the thing lies in it, from the top elsewhere.
    fn own(&self, path: &str) -> String {
        match &self.topic {
            Some(topic) if path.len() > topic.len() && path.starts_with(topic.as_str()) => {
                path[topic.len()..].to_owned()
            }
            _ => path.to_owned(),
        }
    }

    /// What one path names that the agent may read: none where it names nothing readable, and every candidate
    /// where a name names twins.
    async fn locate(&self, conn: &mut Conn, path: &str) -> Result<Vec<tree::Found>, EngineError> {
        let Some(named) = tree::parse_path(path) else {
            return Ok(Vec::new());
        };
        let named = tree::absolute(conn, &self.asking.agent, &named).await?;
        let mut found = self
            .tree
            .find_all(&named, 0)
            .into_iter()
            .filter(|found| self.readable(&found.entry))
            .collect::<Vec<_>>();
        if found.is_empty() {
            found = self.tree.find_attachment(conn, &named, 0).await?;
        }
        found.retain(|found| self.readable(&found.entry));
        Ok(found)
    }

    /// Whether the agent may read what was found, or see it in its tree as a place or a file.
    fn readable(&self, entry: &FoundEntry) -> bool {
        let tree = &self.tree;
        match entry {
            FoundEntry::Directory(at) => tree.directory_at(*at),
            FoundEntry::File(Where::Document(id)) => tree
                .document(*id)
                .is_some_and(|doc| doc.standing.reads() && doc.row.deleted_at.is_none()),
            FoundEntry::File(Where::DocumentFile { document, .. }) => tree
                .document(*document)
                .is_some_and(|doc| doc.standing.reads() && doc.row.deleted_at.is_none()),
            FoundEntry::File(Where::ChatFile { chat, .. }) => tree.chat_origin(*chat).is_some(),
            FoundEntry::File(Where::Conversation(id)) => tree
                .conversation_of(*id)
                .is_some_and(|(standing, _)| standing.reads()),
            FoundEntry::File(Where::NodeChat(node)) => tree
                .node_chat_of(*node)
                .is_some_and(|(_, standing, _)| standing.reads()),
            FoundEntry::File(Where::PeopleChat(chat)) => tree
                .people_chat_of(*chat)
                .is_some_and(|entry| entry.standing.reads()),
            // A file of a document or of a chat, or a version: there to be named, though no reference names it,
            // so that a link to it is told as what it is.
            FoundEntry::File(_) => true,
        }
    }

    /// The top-level blocks of a document in the order of the page, each with the block it is (and none for a
    /// nested one, which carries the top-level block it lies under).
    async fn blocks_of(
        &mut self,
        conn: &mut Conn,
        document: Id,
    ) -> Result<&[(Id, Option<Id>)], EngineError> {
        if let Entry::Vacant(vacant) = self.blocks.entry(document) {
            let rows = orch_db::queries::documents::live_blocks(conn, document).await?;
            let shown: Vec<_> = rows.iter().map(crate::tools::documents::block_of).collect();
            let top: Vec<Id> = crate::tools::documents::top_level(&shown)
                .into_iter()
                .map(|block| block.id)
                .collect();
            // Every block, with the top-level block it lies under.
            let parents: HashMap<Id, Option<Id>> =
                rows.iter().map(|row| (row.id, row.parent_id)).collect();
            let mut all: Vec<(Id, Option<Id>)> = top.iter().map(|id| (*id, None)).collect();
            for row in &rows {
                if row.parent_id.is_none() {
                    continue;
                }
                let mut at = row.id;
                let mut steps = 0_usize;
                while let Some(Some(parent)) = parents.get(&at) {
                    at = *parent;
                    steps += 1;
                    if steps > orch_documents::blocks::nesting_limit() {
                        break;
                    }
                }
                all.push((row.id, Some(at)));
            }
            vacant.insert(all);
        }
        Ok(self
            .blocks
            .get(&document)
            .map(Vec::as_slice)
            .unwrap_or_default())
    }

    /// The block a document's part number names: its top-level block of that number, counted from 1.
    async fn block_numbered(
        &mut self,
        conn: &mut Conn,
        document: Id,
        number: i64,
    ) -> Result<Option<Id>, EngineError> {
        let index = usize::try_from(number.saturating_sub(1)).ok();
        let blocks = self.blocks_of(conn, document).await?;
        let top: Vec<Id> = blocks
            .iter()
            .filter(|(_, under)| under.is_none())
            .map(|(id, _)| *id)
            .collect();
        Ok(index.and_then(|index| top.get(index).copied()))
    }

    /// The number a block of a document has now: that of the top-level block it is, or lies under; none where it
    /// is gone.
    async fn number_of_block(
        &mut self,
        conn: &mut Conn,
        document: Id,
        block: Id,
    ) -> Result<Option<i64>, EngineError> {
        let blocks = self.blocks_of(conn, document).await?;
        let top = blocks
            .iter()
            .find(|(id, _)| *id == block)
            .map(|(id, under)| under.unwrap_or(*id));
        let Some(top) = top else {
            return Ok(None);
        };
        Ok(blocks
            .iter()
            .filter(|(_, under)| under.is_none())
            .position(|(id, _)| *id == top)
            .and_then(|at| i64::try_from(at + 1).ok()))
    }

    /// Resolves one written reference: the path first; where a part was read off its end and the shorter path names
    /// nothing, the text whole, since a name may end in `#` and digits.
    async fn resolve(
        &mut self,
        conn: &mut Conn,
        written: &Written,
        whole: &str,
    ) -> Result<Result<Hit, &'static str>, EngineError> {
        if let Some(rest) = written.path.strip_prefix(references::ID_PREFIX)
            && let Some((id, path)) = rest.split_once('/')
        {
            let Some(environment) = id.parse::<Id>().ok() else {
                return Ok(Err(NOTHING));
            };
            if self
                .workspace
                .as_ref()
                .is_none_or(|workspace| workspace.id != environment)
                || !references::relative_path(path)
            {
                return Ok(Err(NOTHING));
            }
            let hit = self.workspace_hit(path.to_owned(), written.part).await?;
            if hit.is_err()
                && written.part.is_some()
                && let Some((_, whole_path)) = whole
                    .strip_prefix(references::ID_PREFIX)
                    .and_then(|rest| rest.split_once('/'))
                && references::relative_path(whole_path)
            {
                return self.workspace_hit(whole_path.to_owned(), None).await;
            }
            return Ok(hit);
        }
        // A colon is a file-line separator. Shared things use a hash, so try their whole name first.
        let colon_line = written.part.is_some()
            && whole.rsplit_once(':').is_some_and(|(path, digits)| {
                path == written.path && digits.parse::<i64>().ok() == written.part
            });
        let mut candidates = self
            .locate(conn, if colon_line { whole } else { &written.path })
            .await?;
        let mut part = if colon_line { None } else { written.part };
        if candidates.is_empty() && part.is_some() {
            candidates = self.locate(conn, whole).await?;
            part = None;
        }
        match candidates.len() {
            0 => {
                if !written.path.starts_with(references::ID_PREFIX)
                    && let Some(workspace) = &self.workspace
                    && let Some(path) = references::workspace_path(
                        &written.path,
                        &workspace.root,
                        &workspace.directory,
                    )
                {
                    let hit = self.workspace_hit(path, written.part).await?;
                    if hit.is_err()
                        && written.part.is_some()
                        && let Some(workspace) = &self.workspace
                        && let Some(path) =
                            references::workspace_path(whole, &workspace.root, &workspace.directory)
                    {
                        return self.workspace_hit(path, None).await;
                    }
                    return Ok(hit);
                }
                return Ok(Err(NOTHING));
            }
            1 => {}
            _ => return Ok(Err(TWINS)),
        }
        let found = &candidates[0];
        let tree = &self.tree;
        let (typed, kind, name) = match found.entry {
            FoundEntry::Directory(Where::Place(id)) => {
                let Some(place) = tree.place(id) else {
                    return Ok(Err(NOTHING));
                };
                if part.is_some() && place.kind != "thread" {
                    return Ok(Err(NO_PARTS));
                }
                let kind = match place.kind.as_str() {
                    "org" => "org",
                    "project" => "project",
                    "topic" => "topic",
                    _ if part.is_some() => "conversation",
                    _ => "thread",
                };
                (
                    Typed {
                        path: None,
                        kind: InlineRefKind::Node,
                        id,
                        part: None,
                    },
                    kind,
                    place.name.clone(),
                )
            }
            FoundEntry::File(Where::Conversation(id)) => {
                let name = tree
                    .place(id)
                    .map(|place| place.name.clone())
                    .unwrap_or_default();
                (
                    Typed {
                        path: None,
                        kind: InlineRefKind::Node,
                        id,
                        part: None,
                    },
                    "conversation",
                    name,
                )
            }
            FoundEntry::File(Where::Document(id)) => {
                let Some(doc) = tree.document(id) else {
                    return Ok(Err(NOTHING));
                };
                (
                    Typed {
                        path: None,
                        kind: InlineRefKind::Document,
                        id,
                        part: None,
                    },
                    "document",
                    doc.row.title.clone(),
                )
            }
            FoundEntry::File(Where::NodeChat(node)) => {
                let Some((chat, _, _)) = tree.node_chat_of(node) else {
                    return Ok(Err(NOTHING));
                };
                let name = tree
                    .place(node)
                    .map(|place| place.name.clone())
                    .unwrap_or_default();
                (
                    Typed {
                        path: None,
                        kind: InlineRefKind::Chat,
                        id: chat,
                        part: None,
                    },
                    "chat",
                    format!("Chat of {name}"),
                )
            }
            FoundEntry::File(Where::PeopleChat(chat)) => {
                let Some(entry) = tree.people_chat_of(chat) else {
                    return Ok(Err(NOTHING));
                };
                (
                    Typed {
                        path: None,
                        kind: InlineRefKind::Chat,
                        id: chat,
                        part: None,
                    },
                    "chat",
                    entry.label.clone(),
                )
            }
            FoundEntry::File(Where::DocumentFile { file, .. }) => (
                Typed {
                    kind: InlineRefKind::File,
                    id: file,
                    path: None,
                    part: None,
                },
                "file",
                found.path.rsplit('/').next().unwrap_or_default().to_owned(),
            ),
            FoundEntry::File(Where::ChatFile {
                chat,
                message,
                carried,
            }) => (
                Typed {
                    kind: InlineRefKind::File,
                    id: tree::attachments::chat_file_id(chat, message, carried),
                    path: None,
                    part: None,
                },
                "file",
                found.path.rsplit('/').next().unwrap_or_default().to_owned(),
            ),
            _ => return Ok(Err(NOT_A_THING)),
        };
        let Some(number) = part else {
            return Ok(Ok(Hit {
                typed,
                kind,
                name,
                part: None,
            }));
        };
        let typed = match typed.kind {
            InlineRefKind::File | InlineRefKind::EnvironmentFile => {
                if number < 1 {
                    return Ok(Err("a line is numbered from one"));
                }
                Typed {
                    part: Some(RefPart::Line(number)),
                    ..typed
                }
            }
            InlineRefKind::Node => {
                let turns = self
                    .tree
                    .conversation_of(typed.id)
                    .filter(|(standing, _)| standing.reads())
                    .map(|(_, turns)| turns);
                match turns {
                    None => return Ok(Err(NOTHING)),
                    Some(turns) if number < 1 || number > turns => {
                        return Ok(Err("there is no turn of that number in the conversation"));
                    }
                    Some(_) => Typed {
                        part: Some(RefPart::Turn(number)),
                        ..typed
                    },
                }
            }
            InlineRefKind::Chat => {
                let messages = match found.entry {
                    FoundEntry::File(Where::NodeChat(node)) => self
                        .tree
                        .node_chat_of(node)
                        .map(|(_, _, messages)| messages),
                    _ => self
                        .tree
                        .people_chat_of(typed.id)
                        .map(|entry| entry.messages),
                };
                if number < 1 || messages.is_none_or(|messages| number > messages) {
                    return Ok(Err("there is no message of that number in the chat"));
                }
                Typed {
                    part: Some(RefPart::Message(number)),
                    ..typed
                }
            }
            InlineRefKind::Document => {
                let Some(block) = self.block_numbered(conn, typed.id, number).await? else {
                    return Ok(Err("there is no block of that number in the document"));
                };
                Typed {
                    part: Some(RefPart::Block(block)),
                    ..typed
                }
            }
        };
        Ok(Ok(Hit {
            typed,
            kind,
            name,
            part: Some(number),
        }))
    }

    /// How one typed reference is written for this agent; none where it may not read the thing.
    async fn writing(
        &mut self,
        conn: &mut Conn,
        typed: &Typed,
    ) -> Result<Option<Writing>, EngineError> {
        if typed.kind == InlineRefKind::EnvironmentFile {
            let Some(workspace) = self
                .workspace
                .as_ref()
                .filter(|workspace| workspace.id == typed.id)
            else {
                return Ok(None);
            };
            let Some(path) = typed
                .path
                .as_deref()
                .filter(|path| references::relative_path(path))
            else {
                return Ok(None);
            };
            let own = if workspace.directory.is_empty() {
                path.to_owned()
            } else if let Some(relative) = path.strip_prefix(&format!("{}/", workspace.directory)) {
                relative.to_owned()
            } else {
                crate::sessions::absolute_directory(&workspace.root, path)
            };
            return Ok(Some(Writing::Link {
                name: path.rsplit('/').next().unwrap_or(path).to_owned(),
                path: own,
                part: match typed.part {
                    Some(RefPart::Line(line)) => Some(line),
                    _ => None,
                },
            }));
        }
        let mut found = self
            .tree
            .find_all(&Named::Id(typed.id), 0)
            .into_iter()
            .next();
        if found.is_none() && typed.kind == InlineRefKind::File {
            found = self
                .tree
                .find_attachment(conn, &Named::Id(typed.id), 0)
                .await?
                .into_iter()
                .next();
        }
        let Some(found) = found.filter(|found| self.readable(&found.entry)) else {
            return Ok(None);
        };
        let tree = &self.tree;
        let written = match (&found.entry, &typed.part) {
            (FoundEntry::Directory(Where::Place(id)), Some(RefPart::Turn(turn))) => {
                if !tree
                    .conversation_of(*id)
                    .is_some_and(|(standing, _)| standing.reads())
                {
                    return Ok(None);
                }
                let name = tree
                    .place(*id)
                    .map(|place| place.name.clone())
                    .unwrap_or_default();
                Writing::Link {
                    name,
                    path: self.own(&format!("{}conversation.md", found.path)),
                    part: Some(*turn),
                }
            }
            (FoundEntry::Directory(Where::Place(id)), _) => Writing::Link {
                name: tree
                    .place(*id)
                    .map(|place| place.name.clone())
                    .unwrap_or_default(),
                path: self.own(&found.path),
                part: None,
            },
            (FoundEntry::File(Where::Document(id)), part) => {
                let name = tree
                    .document(*id)
                    .map(|doc| doc.row.title.clone())
                    .unwrap_or_default();
                let path = self.own(&found.path);
                let number = match part {
                    Some(RefPart::Block(block)) => self.number_of_block(conn, *id, *block).await?,
                    _ => None,
                };
                Writing::Link {
                    name,
                    path,
                    part: number,
                }
            }
            (FoundEntry::File(Where::NodeChat(node)), part) => Writing::Link {
                name: format!(
                    "Chat of {}",
                    tree.place(*node)
                        .map(|place| place.name.as_str())
                        .unwrap_or_default()
                ),
                path: self.own(&found.path),
                part: match part {
                    Some(RefPart::Message(number)) => Some(*number),
                    _ => None,
                },
            },
            (FoundEntry::File(Where::PeopleChat(chat)), part) => Writing::Link {
                name: tree
                    .people_chat_of(*chat)
                    .map(|entry| entry.label.clone())
                    .unwrap_or_default(),
                path: self.own(&found.path),
                part: match part {
                    Some(RefPart::Message(number)) => Some(*number),
                    _ => None,
                },
            },
            (FoundEntry::File(Where::DocumentFile { .. } | Where::ChatFile { .. }), part) => {
                Writing::Link {
                    name: found.path.rsplit('/').next().unwrap_or_default().to_owned(),
                    path: self.own(&found.path),
                    part: match part {
                        Some(RefPart::Line(line)) => Some(*line),
                        _ => None,
                    },
                }
            }
            _ => return Ok(None),
        };
        Ok(Some(written))
    }

    async fn workspace_hit(
        &mut self,
        path: String,
        line: Option<i64>,
    ) -> Result<Result<Hit, &'static str>, EngineError> {
        if line.is_some_and(|line| line < 1) {
            return Ok(Err("a line is numbered from one"));
        }
        let Some(workspace) = &self.workspace else {
            return Ok(Err(NOTHING));
        };
        let environment = workspace.id;
        if !self.files.contains_key(&path) {
            if self.files.len() >= REFERENCES_MAX {
                return Ok(Err(TOO_MANY));
            }
            let exists = match workspace.machine.filter(|_| workspace.live) {
                Some(machine) => {
                    let command = Outgoing::new(
                        orch_types::new_id(),
                        machine,
                        SerialKey::None,
                        &FsStat {
                            environment_id: environment,
                            mount: None,
                            path: path.clone(),
                        },
                    )?;
                    match self.engine.ask_bounded(command).await? {
                        Some(Outcome::Succeeded(value)) => {
                            serde_json::from_value::<FileStat>(value)
                                .ok()
                                .is_some_and(|stat| {
                                    matches!(
                                        stat.entry.kind,
                                        DirEntryKind::File | DirEntryKind::Symlink
                                    )
                                })
                        }
                        _ => false,
                    }
                }
                None => false,
            };
            self.files.insert(path.clone(), exists);
        }
        if self.files.get(&path) != Some(&true) {
            return Ok(Err(NOTHING));
        }
        Ok(Ok(Hit {
            typed: Typed {
                kind: InlineRefKind::EnvironmentFile,
                id: environment,
                path: Some(path.clone()),
                part: line.map(RefPart::Line),
            },
            kind: "environment_file",
            name: path.rsplit('/').next().unwrap_or(&path).to_owned(),
            part: line,
        }))
    }
}

impl Engine {
    /// Writes copied identifiers and stored references for the receiving agent without statting ordinary prose.
    pub(crate) async fn copied_content_for_agent(
        &self,
        conn: &mut Conn,
        thread: Id,
        content: &[orch_api_contract::threads::ContentPart],
    ) -> Result<Vec<orch_api_contract::threads::ContentPart>, EngineError> {
        use orch_api_contract::threads::ContentPart;
        let mut out = content.to_vec();
        fn text(part: &ContentPart) -> Option<&str> {
            match part {
                ContentPart::Text { text } => Some(text.as_str()),
                ContentPart::Forwarded(value) => Some(value.text.as_str()),
                ContentPart::PastedText(paste) => paste.inline_text.as_deref(),
                _ => None,
            }
        }
        let fragments: Vec<String> = out
            .iter()
            .filter_map(text)
            .filter(|text| {
                !references::typed_in(text).is_empty()
                    || references::find(text)
                        .iter()
                        .any(|found| found.text.starts_with(references::ID_PREFIX))
            })
            .map(str::to_owned)
            .collect();
        if fragments.is_empty() {
            return Ok(out);
        }
        let Some(mut reach) = self.reach_of(conn, thread).await? else {
            return Ok(out);
        };
        let mut resolved: HashMap<String, Typed> = HashMap::new();
        let mut seen = HashSet::new();
        for fragment in &fragments {
            for found in references::find(fragment) {
                let Some(written) = references::parse(&found.text)
                    .filter(|written| written.path.starts_with(references::ID_PREFIX))
                else {
                    continue;
                };
                if !seen.insert(found.text.clone()) || seen.len() > REFERENCES_MAX {
                    continue;
                }
                let Some(rest) = written.path.strip_prefix(references::ID_PREFIX) else {
                    continue;
                };
                if let Some((environment, path)) = rest.split_once('/') {
                    if let Ok(id) = environment.parse::<Id>()
                        && references::relative_path(path)
                        && written.part.is_none_or(|number| number >= 1)
                    {
                        resolved.insert(
                            found.text,
                            Typed {
                                kind: InlineRefKind::EnvironmentFile,
                                id,
                                path: Some(path.to_owned()),
                                part: written.part.map(RefPart::Line),
                            },
                        );
                    }
                } else {
                    match reach.resolve(conn, &written, &found.text).await? {
                        Ok(hit) => {
                            resolved.insert(found.text, hit.typed);
                        }
                        Err(NOTHING) => {
                            if let Ok(id) = rest.parse::<Id>() {
                                resolved.insert(
                                    found.text,
                                    Typed {
                                        kind: InlineRefKind::Node,
                                        id,
                                        path: None,
                                        part: None,
                                    },
                                );
                            }
                        }
                        Err(_) => {}
                    }
                }
            }
        }
        let stored: Vec<(String, String)> = fragments
            .into_iter()
            .map(|text| {
                let kept = references::stored(&text, &|found| resolved.get(&found.text).cloned());
                (text, kept)
            })
            .collect();
        let wanted: Vec<Typed> = stored
            .iter()
            .flat_map(|(_, text)| references::typed_in(text))
            .collect();
        let stored: HashMap<String, String> = stored.into_iter().collect();
        let mut writings = reach.writings(conn, &wanted).await?;
        // A relative path that looks like a framing tag is unambiguous when written from its root.
        for (typed, writing) in &mut writings {
            if let Writing::Link { path, .. } = writing
                && (path.starts_with("orch") || path.starts_with("relayed"))
                && references::destination(path, None).starts_with('<')
            {
                if typed.kind == InlineRefKind::EnvironmentFile {
                    if let (Some(workspace), Some(relative)) = (&reach.workspace, &typed.path) {
                        *path = crate::sessions::absolute_directory(&workspace.root, relative);
                    }
                } else if let Some(topic) = &reach.topic {
                    *path = format!("{topic}{path}");
                }
            }
        }
        let mut numbered = 0;
        for part in &mut out {
            let text = match part {
                ContentPart::Text { text } => text,
                ContentPart::Forwarded(value) => &mut value.text,
                ContentPart::PastedText(paste) => match &mut paste.inline_text {
                    Some(text) => text,
                    None => continue,
                },
                _ => continue,
            };
            if let Some(stored) = stored.get(text) {
                *text = references::for_reader(
                    stored,
                    &|typed| writings.get(typed).cloned(),
                    &mut numbered,
                );
            }
        }
        Ok(out)
    }
}
