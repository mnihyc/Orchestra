//! A document as an agent reads and changes it: its blocks written as Markdown by the one renderer, cut into the pieces that a change touches, and read back into blocks by the one reader.
//!
//! What `read_shared` returns of a document is what `replace` matches, so both ask this module. The document is
//! written whole by `orch_documents::markdown`; here each top-level block, or each item of a list, is also written
//! on its own, so that a passage can be found in the piece it lies in and a change touches that piece and no
//! other. The pieces are checked against the whole: they put together are the text the renderer gave, or the
//! document is said not to be cut into pieces and a change by passage is refused, never guessed at.
//!
//! A reference in a document is written for the agent that reads it ([`Known::for_reader`]): a link with the
//! thing's name and the reader's own path, or, where it may not read the thing, words that name nothing and are
//! numbered in the order of the page, so that a change written around them keeps the reference they stand for.

use crate::error::EngineError;
use orch_api_contract::documents::{BlockType, DocumentBlock, Inline, InlineRefKind, RefPart};
use orch_db::Conn;
use orch_db::queries::documents::{self as rows, BlockRow, DocumentRow};
use orch_db::queries::identity;
use orch_db::queries::shares as share_rows;
use orch_documents::blocks::{self, Lookup};
use orch_documents::files::FileInfo;
use orch_documents::markdown::{self, RenderOptions};
use orch_documents::mentions;
use orch_documents::references::{Typed, Writing};
use orch_types::{Id, Time};
use std::collections::{HashMap, HashSet};

/// What a document names: the people mentioned in it, and the places and documents its chips point to, and the
/// files it holds, which a text links to by the directory they lie in beside it.
#[derive(Debug, Clone, Default)]
pub struct Known {
    users: HashMap<Id, (String, String)>,
    nodes: HashMap<Id, String>,
    documents: HashMap<Id, String>,
    files: HashMap<Id, FileInfo>,
    /// The directory the files lie in, as the text links to them; empty where nothing says (`files`).
    beside: String,
    /// How each reference is written for the agent that reads, by what it names; none when the text is written for
    /// nobody in particular, and a reference is then written as its address.
    writings: Option<HashMap<Typed, Writing>>,
    /// The numbers of the references that reader may not read, in the order of the page.
    hidden: HashMap<Typed, usize>,
    /// Authored names, never names looked up while reading as a different agent.
    shared_labels: Vec<(String, Typed)>,
}

impl Known {
    /// The files of the document by their names, as the reader of Markdown asks for them.
    #[must_use]
    pub fn file_refs(&self) -> Vec<(String, orch_documents::markdown::FileRef)> {
        self.files
            .iter()
            .map(|(id, file)| {
                (
                    file.name.clone(),
                    orch_documents::markdown::FileRef {
                        id: *id,
                        media_type: file.media_type.clone(),
                    },
                )
            })
            .collect()
    }

    /// The directory a link to a file of the document leads into, from the place the text lies in.
    #[must_use]
    pub fn beside(&self) -> Option<&str> {
        (!self.beside.is_empty()).then_some(self.beside.as_str())
    }

    /// The same, written for an agent: each reference as `writings` says, and each that is not there (one the agent
    /// may not read) as words that name nothing, numbered in the order the references stand in `order`.
    #[must_use]
    pub fn for_reader(mut self, writings: HashMap<Typed, Writing>, order: &[Typed]) -> Self {
        let mut hidden: HashMap<Typed, usize> = HashMap::new();
        for typed in order {
            if !writings.contains_key(typed) && !hidden.contains_key(typed) {
                let number = hidden.len() + 1;
                hidden.insert(typed.clone(), number);
            }
        }
        self.writings = Some(writings);
        self.hidden = hidden;
        self
    }

    /// The reference that the hidden words of this number stand for, as this reader was given the text.
    #[must_use]
    pub fn hidden(&self, number: usize) -> Option<&Typed> {
        self.hidden
            .iter()
            .find(|(_, at)| **at == number)
            .map(|(typed, _)| typed)
    }

    /// Unambiguous shared labels that can preserve unreadable references through an agent edit.
    #[must_use]
    pub fn hidden_names(&self) -> Vec<(String, Typed)> {
        let mut names: HashMap<String, Option<Typed>> = HashMap::new();
        for (label, typed) in &self.shared_labels {
            if !self.hidden.contains_key(typed) {
                continue;
            }
            names
                .entry(label.clone())
                .and_modify(|known| {
                    if known.as_ref() != Some(typed) {
                        *known = None;
                    }
                })
                .or_insert_with(|| Some(typed.clone()));
        }
        names
            .into_iter()
            .filter_map(|(label, typed)| typed.map(|typed| (label, typed)))
            .collect()
    }

    /// Shared words that name several hidden identities cannot be safely rebound by a whole-document edit.
    #[must_use]
    pub fn ambiguous_hidden_names(&self) -> Vec<String> {
        let safe: HashSet<_> = self
            .hidden_names()
            .into_iter()
            .map(|(words, _)| words)
            .collect();
        self.shared_labels
            .iter()
            .filter(|(_, typed)| self.hidden.contains_key(typed))
            .map(|(words, _)| words)
            .filter(|words| !safe.contains(*words))
            .cloned()
            .collect::<HashSet<_>>()
            .into_iter()
            .collect()
    }

    /// Only the writer of this already authorized block, from the same bounded
    /// identity lookup as its mentions; no directory is queried or exposed.
    pub fn block_writer(&self, row: &BlockRow) -> Option<String> {
        match row.last_writer()? {
            orch_documents::versions::Writer::User(user) => {
                Some(self.users.get(&user).map_or_else(
                    || "a removed person".to_owned(),
                    |(username, name)| orch_chat::written::person_name(name, username),
                ))
            }
            orch_documents::versions::Writer::Agent { agent, .. } => Some(agent),
        }
    }
}

impl Lookup for Known {
    fn username(&self, user: Id) -> Option<String> {
        self.users.get(&user).map(|(username, _)| username.clone())
    }

    fn display_name(&self, user: Id) -> Option<String> {
        self.users
            .get(&user)
            .map(|(username, name)| orch_chat::written::person_name(name, username))
    }

    fn name_of(&self, kind: InlineRefKind, id: Id) -> Option<String> {
        match kind {
            InlineRefKind::Node => self.nodes.get(&id).cloned(),
            InlineRefKind::Document => self.documents.get(&id).cloned(),
            InlineRefKind::Chat | InlineRefKind::File | InlineRefKind::EnvironmentFile => None,
        }
    }

    fn writing_path(
        &self,
        kind: InlineRefKind,
        id: Id,
        path: Option<&str>,
        part: Option<&RefPart>,
    ) -> Option<Writing> {
        let writings = self.writings.as_ref()?;
        let typed = Typed {
            kind,
            id,
            path: path.map(str::to_owned),
            part: part.copied(),
        };
        Some(writings.get(&typed).cloned().unwrap_or_else(|| {
            Writing::Hidden(self.hidden.get(&typed).copied().unwrap_or_default())
        }))
    }

    fn file(&self, id: Id) -> Option<FileInfo> {
        self.files.get(&id).cloned()
    }
}

fn block_type_of(word: &str) -> BlockType {
    serde_json::from_value(serde_json::Value::String(word.to_owned()))
        .unwrap_or(BlockType::Paragraph)
}

/// A stored block as the renderer takes it.
#[must_use]
pub fn block_of(row: &BlockRow) -> DocumentBlock {
    DocumentBlock {
        id: row.id,
        document_id: row.document_id,
        parent_id: row.parent_id,
        position: row.position.clone(),
        block_type: block_type_of(&row.block_type),
        props: blocks::props_from_json(&row.props),
        content: blocks::content_from_json(&row.content),
        removed: row.removed_at.is_some(),
        seq_created: row.seq_created,
        seq_updated: row.seq_updated,
        updated_by: None,
        updated_at: Time::new(row.updated_at),
    }
}

/// The names the blocks mention and the files their document holds. `beside` is the directory the text links its
/// files in (see [`crate::written::files_beside`]).
pub async fn known_for(
    conn: &mut Conn,
    rows: &[BlockRow],
    beside: &str,
) -> Result<Known, EngineError> {
    let mut users: Vec<Id> = Vec::new();
    let mut nodes: Vec<Id> = Vec::new();
    let mut documents: Vec<Id> = Vec::new();
    let mut shared_labels = Vec::new();
    for row in rows {
        users.extend(row.updated_by);
        let content = blocks::content_from_json(&row.content);
        users.extend(mentions::in_content(&content));
        let runs: Vec<&[orch_api_contract::documents::Inline]> = match &content {
            orch_api_contract::documents::BlockContent::Inline(runs) => vec![runs.as_slice()],
            orch_api_contract::documents::BlockContent::Rows(table) => table
                .iter()
                .flat_map(|row| row.cells.iter().map(|cell| cell.content.as_slice()))
                .collect(),
        };
        for run in runs {
            for inline in run {
                if let Inline::Ref {
                    kind,
                    id,
                    path,
                    part,
                    label,
                    ..
                } = inline
                {
                    shared_labels.push((
                        orch_documents::references::no_access(label.as_deref(), *kind),
                        Typed {
                            kind: *kind,
                            id: *id,
                            path: path.clone(),
                            part: *part,
                        },
                    ));
                }
            }
            for (kind, id) in mentions::chips_in(run) {
                match kind {
                    InlineRefKind::Node => nodes.push(id),
                    InlineRefKind::Document => documents.push(id),
                    InlineRefKind::Chat | InlineRefKind::File | InlineRefKind::EnvironmentFile => {}
                }
            }
        }
    }
    let mut known = Known {
        beside: beside.to_owned(),
        shared_labels,
        ..Known::default()
    };
    if let Some(document) = rows.first().map(|row| row.document_id) {
        for file in self::rows::files(conn, document).await? {
            known.files.insert(
                file.id,
                FileInfo {
                    name: file.name,
                    media_type: file.media_type,
                    size: file.size,
                    is_text: file.is_text,
                    bytes: None,
                },
            );
        }
    }
    if !users.is_empty() {
        users.sort_unstable();
        users.dedup();
        for person in identity::users_brief(conn, &users).await? {
            known
                .users
                .insert(person.id, (person.username, person.display_name));
        }
    }
    if !nodes.is_empty() {
        for place in share_rows::places(conn, &nodes).await? {
            known.nodes.insert(place.id, place.name);
        }
    }
    if !documents.is_empty() {
        for document in rows::get_many(conn, &documents).await? {
            known.documents.insert(document.id, document.title);
        }
    }
    Ok(known)
}

/// The references the blocks hold, in the order of the page: the blocks one after another as they are written,
/// what is nested in a block after it, and in a table cell by cell.
#[must_use]
pub fn references_in(rows: &[BlockRow]) -> Vec<Typed> {
    let shown: Vec<DocumentBlock> = rows.iter().map(block_of).collect();
    let mut out = Vec::new();
    let mut seen: HashSet<Id> = HashSet::new();
    let mut stack: Vec<&DocumentBlock> = top_level(&shown).into_iter().rev().collect();
    while let Some(block) = stack.pop() {
        if !seen.insert(block.id) {
            continue;
        }
        let runs: Vec<&[Inline]> = match &block.content {
            orch_api_contract::documents::BlockContent::Inline(runs) => vec![runs.as_slice()],
            orch_api_contract::documents::BlockContent::Rows(table) => table
                .iter()
                .flat_map(|row| row.cells.iter().map(|cell| cell.content.as_slice()))
                .collect(),
        };
        for run in runs.into_iter().flatten() {
            if let Inline::Ref {
                kind,
                id,
                path,
                part,
                ..
            } = run
            {
                out.push(Typed {
                    kind: *kind,
                    id: *id,
                    path: path.clone(),
                    part: *part,
                });
            }
        }
        let mut children: Vec<&DocumentBlock> = shown
            .iter()
            .filter(|child| child.parent_id == Some(block.id) && !child.removed)
            .collect();
        children.sort_by(|a, b| {
            orch_documents::position::Position::parse(&a.position)
                .cmp(&orch_documents::position::Position::parse(&b.position))
                .then(a.id.cmp(&b.id))
        });
        stack.extend(children.into_iter().rev());
    }
    out
}

/// The text of a document's blocks as Markdown, with no trailing line break.
#[must_use]
pub fn markdown_of(blocks: &[DocumentBlock], known: &Known) -> String {
    markdown::render(
        blocks,
        known,
        &RenderOptions {
            discussions: None,
            files_dir: known.beside(),
            preserve_reference_labels: known.writings.is_none(),
        },
    )
    .text
    .trim_end_matches('\n')
    .to_owned()
}

/// What follows a top-level block in the text: a blank line, or only a line break inside a list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gap {
    /// A blank line.
    Paragraph,
    /// A line break, between the items of one list.
    List,
}

impl Gap {
    /// The text.
    #[must_use]
    pub const fn text(self) -> &'static str {
        match self {
            Self::Paragraph => "\n\n",
            Self::List => "\n",
        }
    }
}

/// One top-level block written on its own, with everything under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Piece {
    /// The top-level block.
    pub block: Id,
    /// Its text.
    pub text: String,
    /// What follows it.
    pub gap: Gap,
}

/// A document cut into the pieces its top-level blocks are written as.
#[derive(Debug, Clone)]
pub struct Pieces {
    /// The text, whole, as the renderer gave it.
    pub whole: String,
    /// The pieces, in order; empty where the document could not be cut.
    pub pieces: Vec<Piece>,
    /// Whether the pieces put together are the whole.
    pub cut: bool,
    /// How many top-level blocks there are.
    pub count: i64,
}

fn is_unordered(kind: BlockType) -> bool {
    matches!(
        kind,
        BlockType::Bullet | BlockType::Todo | BlockType::Toggle
    )
}

fn is_listed(kind: BlockType) -> bool {
    is_unordered(kind) || kind == BlockType::Numbered
}

/// The block and everything under it.
fn subtree(all: &[DocumentBlock], root: Id) -> Vec<DocumentBlock> {
    let mut keep: HashSet<Id> = HashSet::from([root]);
    let mut out: Vec<DocumentBlock> = Vec::new();
    // Siblings are in no order here, and a parent may come after its child: repeat until nothing is added.
    loop {
        let before = keep.len();
        for block in all {
            if block.parent_id.is_some_and(|parent| keep.contains(&parent)) {
                keep.insert(block.id);
            }
        }
        if keep.len() == before {
            break;
        }
    }
    for block in all {
        if keep.contains(&block.id) {
            out.push(block.clone());
        }
    }
    out
}

/// Splits the text of a list into its items: every line that begins at the left edge with a marker begins one.
fn split_items(run: &str) -> Vec<String> {
    let mut items: Vec<String> = Vec::new();
    for line in run.lines() {
        let begins = line.starts_with("- ")
            || line.split_once(". ").is_some_and(|(number, _)| {
                !number.is_empty() && number.chars().all(|c| c.is_ascii_digit())
            });
        if begins || items.is_empty() {
            items.push(line.to_owned());
        } else if let Some(last) = items.last_mut() {
            last.push('\n');
            last.push_str(line);
        }
    }
    items
}

/// The top-level blocks of a document in the order they are written.
#[must_use]
pub fn top_level(all: &[DocumentBlock]) -> Vec<&DocumentBlock> {
    let mut roots: Vec<&DocumentBlock> = all
        .iter()
        .filter(|block| block.parent_id.is_none() && !block.removed)
        .collect();
    roots.sort_by(|a, b| {
        orch_documents::position::Position::parse(&a.position)
            .cmp(&orch_documents::position::Position::parse(&b.position))
            .then(a.id.cmp(&b.id))
    });
    roots
}

/// Cuts a document into pieces.
#[must_use]
pub fn pieces_of(all: &[DocumentBlock], known: &Known) -> Pieces {
    let whole = markdown_of(all, known);
    let roots = top_level(all);
    let count = i64::try_from(roots.len()).unwrap_or(i64::MAX);
    let mut pieces: Vec<Piece> = Vec::new();
    let mut at = 0;
    while at < roots.len() {
        let kind = roots[at].block_type;
        if is_listed(kind) {
            let mut end = at + 1;
            while end < roots.len()
                && ((is_unordered(kind) && is_unordered(roots[end].block_type))
                    || (kind == BlockType::Numbered
                        && roots[end].block_type == BlockType::Numbered))
            {
                end += 1;
            }
            let run: Vec<DocumentBlock> = roots[at..end]
                .iter()
                .flat_map(|root| subtree(all, root.id))
                .collect();
            let items = split_items(&markdown_of(&run, known));
            if items.len() == end - at {
                for (offset, item) in items.into_iter().enumerate() {
                    pieces.push(Piece {
                        block: roots[at + offset].id,
                        text: item,
                        gap: if at + offset + 1 < end {
                            Gap::List
                        } else {
                            Gap::Paragraph
                        },
                    });
                }
            } else {
                // The run could not be cut into its items: it is one piece, of its first block.
                pieces.push(Piece {
                    block: roots[at].id,
                    text: markdown_of(&run, known),
                    gap: Gap::Paragraph,
                });
            }
            at = end;
        } else {
            let group = subtree(all, roots[at].id);
            pieces.push(Piece {
                block: roots[at].id,
                text: markdown_of(&group, known),
                gap: Gap::Paragraph,
            });
            at += 1;
        }
    }
    let joined = join(&pieces);
    let cut = joined == whole && i64::try_from(pieces.len()).unwrap_or(i64::MAX) == count;
    Pieces {
        whole,
        pieces: if cut { pieces } else { Vec::new() },
        cut,
        count,
    }
}

/// The pieces put together: each followed by its gap, none after the last.
#[must_use]
pub fn join(pieces: &[Piece]) -> String {
    let mut out = String::new();
    for (at, piece) in pieces.iter().enumerate() {
        out.push_str(&piece.text);
        if at + 1 < pieces.len() {
            out.push_str(piece.gap.text());
        }
    }
    out
}

/// The text of the blocks `from` to `to`, counted from 1 in order and both counted in.
#[must_use]
pub fn range_text(pieces: &Pieces, from: i64, to: i64) -> String {
    if !pieces.cut {
        return pieces.whole.clone();
    }
    let first = usize::try_from(from.max(1) - 1).unwrap_or(0);
    let end = usize::try_from(to.max(0))
        .unwrap_or(0)
        .min(pieces.pieces.len());
    pieces.pieces.get(first..end).map(join).unwrap_or_default()
}

/// The blocks a document holds now.
pub async fn live(conn: &mut Conn, document: Id) -> Result<Vec<BlockRow>, EngineError> {
    Ok(rows::live_blocks(conn, document).await?)
}

/// Who last changed a document, as a line says it.
pub async fn last_changed_by(conn: &mut Conn, row: &DocumentRow) -> Result<String, EngineError> {
    if let Some(user) = row.updated_by
        && let Some(person) = identity::users_brief(conn, &[user])
            .await?
            .into_iter()
            .next()
    {
        return Ok(orch_chat::written::person_name(
            &person.display_name,
            &person.username,
        ));
    }
    let agents = rows::last_agents(conn, &[row.id]).await?;
    if let Some((_, agent)) = agents.get(&row.id) {
        return Ok(agent.clone());
    }
    if let Some(agent) = &row.created_by_agent {
        return Ok(agent.clone());
    }
    Ok(identity::users_brief(conn, &[row.owner_id])
        .await?
        .into_iter()
        .next()
        .map_or_else(
            || "Orch".to_owned(),
            |person| orch_chat::written::person_name(&person.display_name, &person.username),
        ))
}
