//! The tree of blocks: its twenty types, inline content, nesting, and the limits a document is held to.
//!
//! A block is checked against its type before it is stored: the props are
//! those of the type and nothing else, a tint is a colour of the palette, a
//! heading's level is one to six, the spans of a table stay inside it and do
//! not overlap, an address is `http`, `https` or `mailto` or a path of this
//! system. What the checks let through is stored as typed: the text of a
//! block is never HTML and nothing here renders any. What is only
//! expanded when a block is read (the person behind a mention, the name
//! behind a chip) is taken off when it is written.
//!
//! [`Outline`] is the other half: the live blocks of a document as the tree
//! they make, in order, for whatever walks a document (the renderers, the
//! difference of two states).

use std::collections::HashMap;

use orch_api_contract::documents::{
    BlockContent, BlockProps, BlockType, CalloutProps, CodeProps, ColumnProps,
    DOCUMENT_BLOCK_MAX_BYTES, DOCUMENT_NESTING_MAX, DocumentBlock, FileProps, HEADING_LEVEL_MAX,
    HEADING_LEVEL_MIN, HeadingProps, ImageProps, Inline, InlineRefKind, MediaProps, NoProps,
    NumberedProps, SourceProps, TableProps, TableRow, TextProps, TodoProps,
};
use orch_api_contract::users::COLOR_COUNT;
use orch_types::{Id, Json};

use crate::files::FileInfo;
use crate::position::Position;

/// The most characters of the plain text kept of a block for search: what an
/// item of a conversation keeps, so that one search treats both alike.
pub const SEARCH_TEXT_MAX_CHARS: usize = 20_000;
/// The most columns a table may have. A page shows no more than a few
/// legibly; a hundred is far past that and keeps a single cell's span from
/// asking for a grid of billions.
pub const TABLE_COLUMNS_MAX: usize = 100;
/// The longest address a link or a film may have.
pub const HREF_MAX_BYTES: usize = 2_048;
/// The longest name of the language of a code block.
pub const LANGUAGE_MAX_CHARS: usize = 64;

/// A fault found in a block: where in it, and what is wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fault {
    /// The member, written from the block: `props.level`, `content[2].href`.
    pub path: String,
    /// What is wrong with it.
    pub message: String,
}

impl Fault {
    /// A fault at `path`.
    #[must_use]
    pub fn at(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            message: message.into(),
        }
    }
}

/// A block that was checked, in the form in which it is stored.
#[derive(Debug, Clone, PartialEq)]
pub struct Checked {
    /// Its type.
    pub block_type: BlockType,
    /// Its props, as an object.
    pub props: Json,
    /// Its content, without what is only added when a block is read.
    pub content: BlockContent,
    /// Its text as a plain string, for search.
    pub text: String,
    /// The size of its props and content together, as compact JSON.
    pub bytes: i64,
}

impl Checked {
    /// The content as stored.
    #[must_use]
    pub fn content_json(&self) -> Json {
        content_to_json(&self.content)
    }

    /// The files of the document that the block names.
    #[must_use]
    pub fn file_ids(&self) -> Vec<Id> {
        let file = |member: &str| {
            self.props
                .get(member)
                .and_then(Json::as_str)
                .and_then(|id| id.parse::<Id>().ok())
        };
        match self.block_type {
            BlockType::Image | BlockType::Video | BlockType::Audio | BlockType::File => {
                file("file_id").into_iter().collect()
            }
            _ => Vec::new(),
        }
    }
}

/// Whether blocks of this type may have blocks under them. A code block, a
/// formula, a diagram, a divider, a page break, an image, a film, a sound, a
/// file and a table hold none.
#[must_use]
pub const fn may_have_children(block_type: BlockType) -> bool {
    !matches!(
        block_type,
        BlockType::Code
            | BlockType::Math
            | BlockType::Diagram
            | BlockType::Divider
            | BlockType::PageBreak
            | BlockType::Image
            | BlockType::Video
            | BlockType::Audio
            | BlockType::File
            | BlockType::Table
    )
}

/// Whether a block of type `child` may stand directly under one of type
/// `parent`, or at the top level when `parent` is none: why not, if not.
pub fn may_hold(parent: Option<BlockType>, child: BlockType) -> Result<(), &'static str> {
    match parent {
        None if child == BlockType::Column => Err("a column stands only in a columns block"),
        None => Ok(()),
        Some(parent) if !may_have_children(parent) => Err("this block has no blocks under it"),
        Some(BlockType::Columns) if child != BlockType::Column => {
            Err("a columns block holds columns alone")
        }
        Some(BlockType::Columns) => Ok(()),
        Some(_) if child == BlockType::Column => Err("a column stands only in a columns block"),
        Some(BlockType::Column) if child == BlockType::Columns => {
            Err("a column holds any block but another columns block")
        }
        Some(_) => Ok(()),
    }
}

/// Whether a count of columns is one a columns block may have.
#[must_use]
pub fn columns_fit(count: usize) -> bool {
    let low = usize::try_from(orch_api_contract::documents::COLUMNS_MIN).unwrap_or(usize::MAX);
    let high = usize::try_from(orch_api_contract::documents::COLUMNS_MAX).unwrap_or(0);
    (low..=high).contains(&count)
}

/// How many ancestors a block may have at most: sixteen indents.
#[must_use]
pub fn nesting_limit() -> usize {
    usize::try_from(DOCUMENT_NESTING_MAX).unwrap_or(usize::MAX)
}

/// Whether an address in a link may be followed: `http`, `https` or
/// `mailto`, or a path of this system. Nothing else is: a script address is
/// no address.
#[must_use]
pub fn allowed_href(href: &str) -> bool {
    if href.len() > HREF_MAX_BYTES || href.chars().any(char::is_control) {
        return false;
    }
    let lower = href.to_ascii_lowercase();
    for scheme in ["http://", "https://", "mailto:"] {
        if let Some(rest) = lower.strip_prefix(scheme) {
            return !rest.is_empty();
        }
    }
    // A path of this system: one slash, then not another (which would be an
    // address of some other host) and not a backslash (which a browser reads as one).
    href.starts_with('/') && !href.starts_with("//") && !href.starts_with("/\\")
}

/// Whether an address for a film or a sound may be fetched by whoever asks:
/// `https` and nothing else.
#[must_use]
pub fn allowed_media_url(url: &str) -> bool {
    url.len() <= HREF_MAX_BYTES
        && !url.chars().any(char::is_control)
        && url
            .get(..8)
            .is_some_and(|start| start.eq_ignore_ascii_case("https://"))
        && url.len() > 8
}

fn tint_fault(tint: u32, path: &str, faults: &mut Vec<Fault>) {
    if tint >= COLOR_COUNT {
        faults.push(Fault::at(
            path,
            format!("must be a colour of the palette, 0 to {}", COLOR_COUNT - 1),
        ));
    }
}

fn optional_tint(tint: Option<u32>, path: &str, faults: &mut Vec<Fault>) {
    if let Some(tint) = tint {
        tint_fault(tint, path, faults);
    }
}

fn positive(number: f64, path: &str, faults: &mut Vec<Fault>) {
    if !number.is_finite() || number <= 0.0 {
        faults.push(Fault::at(path, "must be a number above zero"));
    }
}

/// Checks the inline content `runs` at `path` and gives it as it is stored:
/// what is expanded when read (a person's details, a chip's name) is left
/// off, and marks that repeat are said once.
pub fn check_inline(runs: &[Inline], path: &str, faults: &mut Vec<Fault>) -> Vec<Inline> {
    runs.iter()
        .enumerate()
        .map(|(index, run)| match run {
            Inline::Text {
                text,
                marks,
                href,
                color,
                background,
            } => {
                let here = format!("{path}[{index}]");
                optional_tint(*color, &format!("{here}.color"), faults);
                optional_tint(*background, &format!("{here}.background"), faults);
                if let Some(href) = href
                    && !allowed_href(href)
                {
                    faults.push(Fault::at(
                        format!("{here}.href"),
                        "must be an http, https or mailto address, or a path of this system",
                    ));
                }
                let marks = marks.as_ref().map(|marks| {
                    let mut once = Vec::with_capacity(marks.len());
                    for mark in marks {
                        if !once.contains(mark) {
                            once.push(*mark);
                        }
                    }
                    once
                });
                Inline::Text {
                    text: text.clone(),
                    marks,
                    href: href.clone(),
                    color: *color,
                    background: *background,
                }
            }
            Inline::Mention { user_id, .. } => Inline::Mention {
                user_id: *user_id,
                user: None,
            },
            Inline::Ref {
                display,
                kind,
                id,
                path,
                part,
                label,
                ..
            } => Inline::Ref {
                display: *display,
                kind: *kind,
                id: *id,
                path: path.clone(),
                part: *part,
                label: label.clone(),
                name: None,
            },
            other => other.clone(),
        })
        .collect()
}

fn is_plain_text(runs: &[Inline]) -> bool {
    runs.iter().all(|run| {
        matches!(
            run,
            Inline::Text { marks, href, color, background, .. }
                if marks.as_ref().is_none_or(Vec::is_empty)
                    && href.is_none()
                    && color.is_none()
                    && background.is_none()
        )
    })
}

/// Reads a block's props from the JSON it was stored as. Props that do not
/// read (which nothing stored here would) are taken to be none.
#[must_use]
pub fn props_from_json(props: &Json) -> BlockProps {
    serde_json::from_value(props.clone()).unwrap_or(BlockProps::Empty(NoProps {}))
}

/// Reads a block's content from the JSON it was stored as; none if it does
/// not read.
#[must_use]
pub fn content_from_json(content: &Json) -> BlockContent {
    serde_json::from_value(content.clone()).unwrap_or(BlockContent::Inline(Vec::new()))
}

/// A block's content as JSON, to be stored.
#[must_use]
pub fn content_to_json(content: &BlockContent) -> Json {
    serde_json::to_value(content).unwrap_or_else(|_| Json::Array(Vec::new()))
}

fn parsed<T: serde::de::DeserializeOwned + serde::Serialize>(
    value: &Json,
    faults: &mut Vec<Fault>,
) -> Option<T> {
    match serde_json::from_value::<T>(value.clone()) {
        Ok(props) => Some(props),
        Err(error) => {
            faults.push(Fault::at("props", error.to_string()));
            None
        }
    }
}

fn text_look(color: Option<u32>, background: Option<u32>, faults: &mut Vec<Fault>) {
    optional_tint(color, "props.color", faults);
    optional_tint(background, "props.background", faults);
}

/// The props of a block of `block_type`, as the type takes them, or what is
/// wrong with them.
fn check_props(block_type: BlockType, props: &Json, faults: &mut Vec<Fault>) -> Json {
    fn finish<T: serde::Serialize>(props: &T) -> Json {
        serde_json::to_value(props).unwrap_or_else(|_| Json::Object(serde_json::Map::new()))
    }
    let empty = || Json::Object(serde_json::Map::new());
    match block_type {
        BlockType::Paragraph | BlockType::Bullet | BlockType::Toggle | BlockType::Quote => {
            let Some(p) = parsed::<TextProps>(props, faults) else {
                return empty();
            };
            text_look(p.color, p.background, faults);
            finish(&p)
        }
        BlockType::Heading => {
            let Some(p) = parsed::<HeadingProps>(props, faults) else {
                return empty();
            };
            text_look(p.color, p.background, faults);
            if !(HEADING_LEVEL_MIN..=HEADING_LEVEL_MAX).contains(&p.level) {
                faults.push(Fault::at(
                    "props.level",
                    format!("must be {HEADING_LEVEL_MIN} to {HEADING_LEVEL_MAX}"),
                ));
            }
            finish(&p)
        }
        BlockType::Numbered => {
            let Some(p) = parsed::<NumberedProps>(props, faults) else {
                return empty();
            };
            text_look(p.color, p.background, faults);
            finish(&p)
        }
        BlockType::Todo => {
            let Some(p) = parsed::<TodoProps>(props, faults) else {
                return empty();
            };
            text_look(p.color, p.background, faults);
            finish(&p)
        }
        BlockType::Callout => {
            let Some(p) = parsed::<CalloutProps>(props, faults) else {
                return empty();
            };
            text_look(p.color, p.background, faults);
            finish(&p)
        }
        BlockType::Code => {
            let Some(p) = parsed::<CodeProps>(props, faults) else {
                return empty();
            };
            if p.language.chars().count() > LANGUAGE_MAX_CHARS {
                faults.push(Fault::at(
                    "props.language",
                    format!("must be at most {LANGUAGE_MAX_CHARS} characters"),
                ));
            }
            finish(&p)
        }
        BlockType::Math | BlockType::Diagram => {
            let Some(p) = parsed::<SourceProps>(props, faults) else {
                return empty();
            };
            finish(&p)
        }
        BlockType::Divider | BlockType::PageBreak | BlockType::Columns => {
            let Some(p) = parsed::<NoProps>(props, faults) else {
                return empty();
            };
            finish(&p)
        }
        BlockType::Image => {
            let Some(p) = parsed::<ImageProps>(props, faults) else {
                return empty();
            };
            if let Some(width) = p.width {
                positive(width, "props.width", faults);
            }
            finish(&p)
        }
        BlockType::Video | BlockType::Audio => {
            let Some(p) = parsed::<MediaProps>(props, faults) else {
                return empty();
            };
            match (&p.file_id, &p.url) {
                (None, None) => faults.push(Fault::at(
                    "props",
                    "names a file of the document or an https address",
                )),
                (Some(_), Some(_)) => faults.push(Fault::at(
                    "props.url",
                    "is given beside a file: name one of them",
                )),
                (None, Some(url)) if !allowed_media_url(url) => {
                    faults.push(Fault::at("props.url", "must be an https address"));
                }
                _ => {}
            }
            if let Some(width) = p.width {
                positive(width, "props.width", faults);
            }
            finish(&p)
        }
        BlockType::File => {
            let Some(p) = parsed::<FileProps>(props, faults) else {
                return empty();
            };
            finish(&p)
        }
        BlockType::Table => {
            let Some(p) = parsed::<TableProps>(props, faults) else {
                return empty();
            };
            if let Some(widths) = &p.widths {
                for (index, width) in widths.iter().enumerate() {
                    positive(*width, &format!("props.widths[{index}]"), faults);
                }
            }
            finish(&p)
        }
        BlockType::Column => {
            let Some(p) = parsed::<ColumnProps>(props, faults) else {
                return empty();
            };
            if let Some(share) = p.share {
                positive(share, "props.share", faults);
            }
            finish(&p)
        }
    }
}

/// The grid a table's cells make: spans that reach outside it or overlap
/// another are faults. The first row says how wide the table is; a cell is
/// put in the first free place of its row, and the places that a span from
/// above covers are skipped, as the cells that sit there are left out of the
/// row.
fn check_table(rows: &[TableRow], widths: Option<&[f64]>, faults: &mut Vec<Fault>) {
    let Some(first) = rows.first() else {
        return;
    };
    let span = |value: Option<u32>| usize::try_from(value.unwrap_or(1)).unwrap_or(usize::MAX);
    let mut width = 0usize;
    for cell in &first.cells {
        width = width.saturating_add(span(cell.colspan));
    }
    if width > TABLE_COLUMNS_MAX {
        faults.push(Fault::at(
            "content[0]",
            format!("a table has at most {TABLE_COLUMNS_MAX} columns"),
        ));
        return;
    }
    if let Some(widths) = widths
        && widths.len() != width
    {
        faults.push(Fault::at(
            "props.widths",
            format!("must give a share for each of the {width} columns"),
        ));
    }
    let mut covered = vec![vec![false; width]; rows.len()];
    for (r, row) in rows.iter().enumerate() {
        let mut column = 0usize;
        for (c, cell) in row.cells.iter().enumerate() {
            let path = format!("content[{r}].cells[{c}]");
            let (colspan, rowspan) = (span(cell.colspan), span(cell.rowspan));
            if colspan == 0 || rowspan == 0 {
                faults.push(Fault::at(path, "a span is at least one"));
                continue;
            }
            while column < width && covered[r][column] {
                column += 1;
            }
            if column >= width || column + colspan > width {
                faults.push(Fault::at(path, "reaches outside the table"));
                continue;
            }
            if r + rowspan > rows.len() {
                faults.push(Fault::at(path, "reaches below the last row of the table"));
                continue;
            }
            let overlaps = (r..r + rowspan)
                .any(|down| (column..column + colspan).any(|across| covered[down][across]));
            if overlaps {
                faults.push(Fault::at(path, "overlaps another cell"));
                continue;
            }
            for covered_row in &mut covered[r..r + rowspan] {
                covered_row[column..column + colspan].fill(true);
            }
            column += colspan;
        }
    }
}

fn check_cells(rows: &[TableRow], faults: &mut Vec<Fault>) -> Vec<TableRow> {
    rows.iter()
        .enumerate()
        .map(|(r, row)| TableRow {
            cells: row
                .cells
                .iter()
                .enumerate()
                .map(|(c, cell)| {
                    let here = format!("content[{r}].cells[{c}]");
                    optional_tint(cell.color, &format!("{here}.color"), faults);
                    optional_tint(cell.background, &format!("{here}.background"), faults);
                    orch_api_contract::documents::TableCell {
                        content: check_inline(&cell.content, &format!("{here}.content"), faults),
                        colspan: cell.colspan,
                        rowspan: cell.rowspan,
                        color: cell.color,
                        background: cell.background,
                    }
                })
                .collect(),
        })
        .collect()
}

/// Checks and normalizes the content of a block of `block_type`.
fn check_content(
    block_type: BlockType,
    content: Option<&BlockContent>,
    widths: Option<&[f64]>,
    faults: &mut Vec<Fault>,
) -> BlockContent {
    let none = BlockContent::Inline(Vec::new());
    match block_type {
        BlockType::Table => match content {
            None => none,
            Some(BlockContent::Inline(runs)) if runs.is_empty() => none,
            Some(BlockContent::Inline(_)) => {
                faults.push(Fault::at("content", "must be the rows of the table"));
                none
            }
            Some(BlockContent::Rows(rows)) => {
                let rows = check_cells(rows, faults);
                check_table(&rows, widths, faults);
                BlockContent::Rows(rows)
            }
        },
        BlockType::Math
        | BlockType::Diagram
        | BlockType::Divider
        | BlockType::PageBreak
        | BlockType::Columns
        | BlockType::Column => match content {
            None => none,
            Some(BlockContent::Inline(runs)) if runs.is_empty() => none,
            Some(_) => {
                faults.push(Fault::at("content", "must be empty for this type"));
                none
            }
        },
        BlockType::Code => match content {
            None => none,
            Some(BlockContent::Inline(runs)) => {
                if runs.len() > 1 || !is_plain_text(runs) {
                    faults.push(Fault::at("content", "must be one text without marks"));
                }
                check_inline(runs, "content", faults);
                BlockContent::Inline(runs.clone())
            }
            Some(BlockContent::Rows(_)) => {
                faults.push(Fault::at("content", "must be one text without marks"));
                none
            }
        },
        _ => match content {
            None => none,
            Some(BlockContent::Inline(runs)) => {
                BlockContent::Inline(check_inline(runs, "content", faults))
            }
            Some(BlockContent::Rows(_)) => {
                faults.push(Fault::at("content", "must be text, not table rows"));
                none
            }
        },
    }
}

/// Checks a block of type `block_type` with `props` and `content` as they
/// were written (none: what the type starts with) and gives it as it is
/// stored, or everything that is wrong with it.
pub fn check(
    block_type: BlockType,
    props: Option<&BlockProps>,
    content: Option<&BlockContent>,
) -> Result<Checked, Vec<Fault>> {
    let mut faults = Vec::new();
    let written = match props {
        Some(props) => {
            serde_json::to_value(props).unwrap_or_else(|_| Json::Object(serde_json::Map::new()))
        }
        None => Json::Object(serde_json::Map::new()),
    };
    let props = check_props(block_type, &written, &mut faults);
    let widths: Option<Vec<f64>> = props
        .get("widths")
        .and_then(|widths| serde_json::from_value(widths.clone()).ok());
    let content = check_content(block_type, content, widths.as_deref(), &mut faults);
    if !faults.is_empty() {
        return Err(faults);
    }
    let bytes = compact_len(&props) + compact_len(&content_to_json(&content));
    let text = plain_text(block_type, &props, &content);
    Ok(Checked {
        block_type,
        props,
        content,
        text,
        bytes: i64::try_from(bytes).unwrap_or(i64::MAX),
    })
}

/// The block types that start a block with these props when none are given.
#[must_use]
pub fn starting_props(block_type: BlockType) -> Json {
    match block_type {
        BlockType::Heading => serde_json::json!({ "level": HEADING_LEVEL_MIN }),
        BlockType::Todo => serde_json::json!({ "checked": false }),
        BlockType::Callout => serde_json::json!({ "tone": "info" }),
        BlockType::Code => serde_json::json!({ "language": "" }),
        BlockType::Math | BlockType::Diagram => serde_json::json!({ "source": "" }),
        BlockType::Table => serde_json::json!({ "header_row": false }),
        _ => Json::Object(serde_json::Map::new()),
    }
}

fn compact_len(value: &Json) -> usize {
    serde_json::to_string(value).map_or(0, |text| text.len())
}

/// Whether a block of this size is within what one block may be.
#[must_use]
pub fn fits_a_block(bytes: i64) -> bool {
    bytes <= DOCUMENT_BLOCK_MAX_BYTES
}

/// The text of a run of inline content as a plain string.
#[must_use]
pub fn inline_text(runs: &[Inline]) -> String {
    let mut out = String::new();
    for run in runs {
        match run {
            Inline::Text { text, .. } => out.push_str(text),
            Inline::Math { source } => out.push_str(source),
            Inline::Break => out.push('\n'),
            Inline::Mention { .. } | Inline::Ref { .. } => {}
        }
    }
    out
}

/// A block's text as a plain string: its words, the source of a formula or a
/// diagram, a table's cells row by row. For search, and for finding the words
/// a discussion is about.
#[must_use]
pub fn plain_text(block_type: BlockType, props: &Json, content: &BlockContent) -> String {
    let mut out = match content {
        BlockContent::Inline(runs) => inline_text(runs),
        BlockContent::Rows(rows) => rows
            .iter()
            .map(|row| {
                row.cells
                    .iter()
                    .map(|cell| inline_text(&cell.content))
                    .collect::<Vec<_>>()
                    .join("\t")
            })
            .collect::<Vec<_>>()
            .join("\n"),
    };
    if matches!(block_type, BlockType::Math | BlockType::Diagram)
        && let Some(source) = props.get("source").and_then(Json::as_str)
    {
        out = source.to_owned();
    }
    if out.chars().count() > SEARCH_TEXT_MAX_CHARS {
        out = out.chars().take(SEARCH_TEXT_MAX_CHARS).collect();
    }
    out
}

/// What a renderer or a reader may ask about the things that a document
/// names and holds, which only the caller knows. Every answer may be none.
pub trait Lookup {
    /// The name a person is mentioned by.
    fn username(&self, _user: Id) -> Option<String> {
        None
    }

    /// The name shown for a person.
    fn display_name(&self, _user: Id) -> Option<String> {
        None
    }

    /// The name of a node or a document that a chip names.
    fn name_of(&self, _kind: InlineRefKind, _id: Id) -> Option<String> {
        None
    }

    /// How a reference is written for the reader of the text: for an agent, a link in its own terms or words that
    /// name nothing; none for the address form that an export writes.
    fn writing(
        &self,
        _kind: InlineRefKind,
        _id: Id,
        _part: Option<&orch_api_contract::documents::RefPart>,
    ) -> Option<crate::references::Writing> {
        None
    }

    /// How a reference with a workspace path is written for this reader.
    fn writing_path(
        &self,
        kind: InlineRefKind,
        id: Id,
        _path: Option<&str>,
        part: Option<&orch_api_contract::documents::RefPart>,
    ) -> Option<crate::references::Writing> {
        self.writing(kind, id, part)
    }

    /// A file the document holds.
    fn file(&self, _id: Id) -> Option<FileInfo> {
        None
    }
}

/// Knows nothing: what a document alone says.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoLookup;

impl Lookup for NoLookup {}

/// The live blocks of a document as the tree they make, siblings in order.
#[derive(Debug, Clone)]
pub struct Outline<'a> {
    blocks: HashMap<Id, &'a DocumentBlock>,
    children: HashMap<Option<Id>, Vec<Id>>,
}

impl<'a> Outline<'a> {
    /// The tree of `blocks`; removed ones are left out, and a block whose
    /// parent is not among them stands at the top.
    #[must_use]
    pub fn new(blocks: &'a [DocumentBlock]) -> Self {
        let live: HashMap<Id, &DocumentBlock> = blocks
            .iter()
            .filter(|block| !block.removed)
            .map(|block| (block.id, block))
            .collect();
        let mut children: HashMap<Option<Id>, Vec<Id>> = HashMap::new();
        for block in live.values() {
            let parent = block.parent_id.filter(|parent| live.contains_key(parent));
            children.entry(parent).or_default().push(block.id);
        }
        for siblings in children.values_mut() {
            siblings.sort_by_cached_key(|id| {
                let position = live
                    .get(id)
                    .and_then(|block| Position::parse(&block.position))
                    .unwrap_or_else(|| Position::nth(0));
                (position, *id)
            });
        }
        Self {
            blocks: live,
            children,
        }
    }

    /// The blocks at the top level, in order.
    #[must_use]
    pub fn roots(&self) -> Vec<&'a DocumentBlock> {
        self.under(None)
    }

    /// The blocks directly under `parent`, in order.
    #[must_use]
    pub fn children_of(&self, parent: Id) -> Vec<&'a DocumentBlock> {
        self.under(Some(parent))
    }

    fn under(&self, parent: Option<Id>) -> Vec<&'a DocumentBlock> {
        self.children
            .get(&parent)
            .into_iter()
            .flatten()
            .filter_map(|id| self.blocks.get(id).copied())
            .collect()
    }

    /// The block `id`, if it is live.
    #[must_use]
    pub fn get(&self, id: Id) -> Option<&'a DocumentBlock> {
        self.blocks.get(&id).copied()
    }

    /// Every live block, parents before their children, each with the number
    /// of blocks it stands under.
    #[must_use]
    pub fn walk(&self) -> Vec<(&'a DocumentBlock, usize)> {
        let mut out = Vec::with_capacity(self.blocks.len());
        let mut stack: Vec<(&'a DocumentBlock, usize)> =
            self.roots().into_iter().rev().map(|b| (b, 0)).collect();
        while let Some((block, depth)) = stack.pop() {
            out.push((block, depth));
            for child in self.children_of(block.id).into_iter().rev() {
                stack.push((child, depth + 1));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_api_contract::documents::{Mark, TableCell};
    use orch_types::new_id;
    use serde_json::json;

    fn text(words: &str) -> Inline {
        Inline::Text {
            text: words.to_owned(),
            marks: None,
            href: None,
            color: None,
            background: None,
        }
    }

    fn props(value: Json) -> BlockProps {
        serde_json::from_value(value).unwrap()
    }

    fn faults_of(result: Result<Checked, Vec<Fault>>) -> Vec<String> {
        result
            .unwrap_err()
            .into_iter()
            .map(|fault| fault.path)
            .collect()
    }

    #[test]
    fn a_block_takes_the_props_of_its_type_and_nothing_else() {
        let heading = props(json!({ "level": 2, "toggle": true }));
        assert!(check(BlockType::Heading, Some(&heading), None).is_ok());
        assert_eq!(
            faults_of(check(BlockType::Paragraph, Some(&heading), None)),
            ["props"],
            "a paragraph has no level"
        );
        let divider = props(json!({ "color": 1 }));
        assert_eq!(
            faults_of(check(BlockType::Divider, Some(&divider), None)),
            ["props"]
        );
        assert!(check(BlockType::Divider, None, None).is_ok());
        let level_missing = props(json!({ "color": 1 }));
        assert_eq!(
            faults_of(check(BlockType::Heading, Some(&level_missing), None)),
            ["props"],
            "a heading has to say its level"
        );
    }

    #[test]
    fn a_heading_level_outside_one_to_six_and_a_tint_outside_the_palette_are_refused() {
        let seven = props(json!({ "level": 7 }));
        assert_eq!(
            faults_of(check(BlockType::Heading, Some(&seven), None)),
            ["props.level"]
        );
        let zero = props(json!({ "level": 0 }));
        assert_eq!(
            faults_of(check(BlockType::Heading, Some(&zero), None)),
            ["props.level"]
        );
        let twelve = props(json!({ "color": 12 }));
        assert_eq!(
            faults_of(check(BlockType::Paragraph, Some(&twelve), None)),
            ["props.color"]
        );
        let eleven = props(json!({ "background": 11 }));
        assert!(check(BlockType::Quote, Some(&eleven), None).is_ok());
        let tinted = BlockContent::Inline(vec![Inline::Text {
            text: "x".into(),
            marks: None,
            href: None,
            color: Some(40),
            background: None,
        }]);
        assert_eq!(
            faults_of(check(BlockType::Paragraph, None, Some(&tinted))),
            ["content[0].color"]
        );
    }

    #[test]
    fn an_address_is_http_https_or_mailto_or_a_path_of_this_system() {
        for good in [
            "http://example.org",
            "HTTPS://example.org/a?b=c",
            "mailto:ada@example.org",
            "/documents/0199",
            "/",
        ] {
            assert!(allowed_href(good), "{good}");
        }
        for bad in [
            "javascript:alert(1)",
            "data:text/html;base64,AAAA",
            "//evil.example",
            "/\\evil.example",
            "ftp://example.org",
            "https://",
            "https://exa\nmple.org",
            "relative/path",
            "",
        ] {
            assert!(!allowed_href(bad), "{bad:?}");
        }
        let linked = BlockContent::Inline(vec![Inline::Text {
            text: "x".into(),
            marks: None,
            href: Some("javascript:alert(1)".into()),
            color: None,
            background: None,
        }]);
        assert_eq!(
            faults_of(check(BlockType::Paragraph, None, Some(&linked))),
            ["content[0].href"]
        );
        assert!(allowed_media_url("https://example.org/clip.mp4"));
        assert!(!allowed_media_url("http://example.org/clip.mp4"));
    }

    #[test]
    fn what_is_only_added_when_a_block_is_read_is_taken_off_when_it_is_written() {
        let person = new_id();
        let node = new_id();
        let content = BlockContent::Inline(vec![
            Inline::Mention {
                user_id: person,
                user: Some(
                    serde_json::from_value(json!({
                        "id": person, "kind": "human", "username": "ada", "display_name": "Ada",
                        "avatar": { "text": "A", "color": 1 }
                    }))
                    .unwrap(),
                ),
            },
            Inline::Ref {
                display: None,
                path: None,
                kind: InlineRefKind::Node,
                id: node,
                part: None,
                name: Some(Some("Plans".into())),
                label: None,
            },
            Inline::Text {
                text: "x".into(),
                marks: Some(vec![Mark::Bold, Mark::Italic, Mark::Bold]),
                href: None,
                color: None,
                background: None,
            },
        ]);
        let checked = check(BlockType::Paragraph, None, Some(&content)).unwrap();
        let BlockContent::Inline(runs) = &checked.content else {
            panic!("inline content");
        };
        assert_eq!(
            runs[0],
            Inline::Mention {
                user_id: person,
                user: None
            }
        );
        assert_eq!(
            runs[1],
            Inline::Ref {
                display: None,
                path: None,
                kind: InlineRefKind::Node,
                id: node,
                part: None,
                name: None,
                label: None,
            }
        );
        assert!(matches!(
            &runs[2],
            Inline::Text { marks: Some(marks), .. } if marks == &[Mark::Bold, Mark::Italic]
        ));
    }

    #[test]
    fn a_saved_reference_size_is_document_content_and_survives_name_redaction() {
        for display in [
            orch_api_contract::documents::ReferenceSize::Small,
            orch_api_contract::documents::ReferenceSize::Medium,
            orch_api_contract::documents::ReferenceSize::Large,
        ] {
            let content = BlockContent::Inline(vec![Inline::Ref {
                display: Some(display),
                label: None,
                kind: InlineRefKind::File,
                id: new_id(),
                path: None,
                part: None,
                name: Some(Some("a readable name".into())),
            }]);
            let checked = check(BlockType::Paragraph, None, Some(&content)).unwrap();
            let BlockContent::Inline(runs) = checked.content else {
                panic!("inline content")
            };
            assert!(
                matches!(&runs[0], Inline::Ref { display: Some(saved), name: None, .. } if *saved == display)
            );
        }
    }

    #[test]
    fn code_is_one_unmarked_text_a_formula_has_no_content_and_a_table_has_rows() {
        let one = BlockContent::Inline(vec![text("let x = 1;")]);
        let code = props(json!({ "language": "rust" }));
        assert!(check(BlockType::Code, Some(&code), Some(&one)).is_ok());
        let bold = BlockContent::Inline(vec![Inline::Text {
            text: "x".into(),
            marks: Some(vec![Mark::Bold]),
            href: None,
            color: None,
            background: None,
        }]);
        assert_eq!(
            faults_of(check(BlockType::Code, Some(&code), Some(&bold))),
            ["content"]
        );
        let two = BlockContent::Inline(vec![text("a"), text("b")]);
        assert_eq!(
            faults_of(check(BlockType::Code, Some(&code), Some(&two))),
            ["content"]
        );
        let source = props(json!({ "source": "x^2" }));
        assert!(check(BlockType::Math, Some(&source), None).is_ok());
        assert_eq!(
            faults_of(check(BlockType::Math, Some(&source), Some(&one))),
            ["content"]
        );
        let rows = BlockContent::Rows(vec![TableRow {
            cells: vec![TableCell::default()],
        }]);
        let table = props(json!({ "header_row": true }));
        assert!(check(BlockType::Table, Some(&table), Some(&rows)).is_ok());
        assert_eq!(
            faults_of(check(BlockType::Paragraph, None, Some(&rows))),
            ["content"]
        );
        assert_eq!(
            faults_of(check(BlockType::Table, Some(&table), Some(&one))),
            ["content"]
        );
    }

    fn cell(colspan: Option<u32>, rowspan: Option<u32>) -> TableCell {
        TableCell {
            content: vec![text("c")],
            colspan,
            rowspan,
            ..TableCell::default()
        }
    }

    fn table(rows: Vec<Vec<TableCell>>) -> Result<Checked, Vec<Fault>> {
        let rows = BlockContent::Rows(rows.into_iter().map(|cells| TableRow { cells }).collect());
        check(
            BlockType::Table,
            Some(&props(json!({ "header_row": true }))),
            Some(&rows),
        )
    }

    #[test]
    fn a_span_may_not_reach_outside_its_table_or_overlap_another() {
        // A cell spanning two columns and two rows, with the cells it covers left out.
        assert!(
            table(vec![
                vec![cell(Some(2), Some(2)), cell(None, None)],
                vec![cell(None, None)],
                vec![cell(None, None), cell(None, None), cell(None, None)],
            ])
            .is_ok()
        );
        assert_eq!(
            faults_of(table(vec![
                vec![cell(None, None), cell(None, None)],
                vec![cell(Some(3), None)],
            ])),
            ["content[1].cells[0]"],
            "wider than the table"
        );
        assert_eq!(
            faults_of(table(vec![
                vec![cell(None, Some(2)), cell(None, None)],
                vec![cell(Some(2), None)],
            ])),
            ["content[1].cells[0]"],
            "its second column is covered from above"
        );
        assert_eq!(
            faults_of(table(vec![
                vec![cell(None, Some(3)), cell(None, None)],
                vec![cell(None, None)],
            ])),
            ["content[0].cells[0]"],
            "below the last row"
        );
        assert_eq!(
            faults_of(table(vec![
                vec![cell(None, None), cell(None, None)],
                vec![cell(None, None), cell(None, None), cell(None, None)],
            ])),
            ["content[1].cells[2]"],
            "a third cell in a row of two columns"
        );
        assert_eq!(
            faults_of(table(vec![vec![cell(Some(0), None)]])),
            ["content[0].cells[0]"]
        );
        assert_eq!(
            faults_of(table(vec![vec![cell(Some(4_000_000_000), None)]])),
            ["content[0]"]
        );
    }

    #[test]
    fn the_widths_of_a_table_are_one_share_for_each_column() {
        let rows = BlockContent::Rows(vec![TableRow {
            cells: vec![cell(None, None), cell(None, None)],
        }]);
        let one = props(json!({ "header_row": false, "widths": [1.0] }));
        assert_eq!(
            faults_of(check(BlockType::Table, Some(&one), Some(&rows))),
            ["props.widths"]
        );
        let two = props(json!({ "header_row": false, "widths": [1.0, 2.5] }));
        assert!(check(BlockType::Table, Some(&two), Some(&rows)).is_ok());
        let negative = props(json!({ "header_row": false, "widths": [1.0, -2.0] }));
        assert_eq!(
            faults_of(check(BlockType::Table, Some(&negative), Some(&rows))),
            ["props.widths[1]"]
        );
    }

    #[test]
    fn a_film_names_a_file_or_an_https_address_and_not_both() {
        let neither = props(json!({ "width": 400.0 }));
        assert_eq!(
            faults_of(check(BlockType::Video, Some(&neither), None)),
            ["props"]
        );
        let url = props(json!({ "url": "https://example.org/a.mp4" }));
        assert!(check(BlockType::Video, Some(&url), None).is_ok());
        let insecure = props(json!({ "url": "http://example.org/a.mp4" }));
        assert_eq!(
            faults_of(check(BlockType::Audio, Some(&insecure), None)),
            ["props.url"]
        );
        let both = props(json!({ "url": "https://example.org/a.mp4", "file_id": new_id() }));
        assert_eq!(
            faults_of(check(BlockType::Video, Some(&both), None)),
            ["props.url"]
        );
        let file = props(json!({ "file_id": new_id() }));
        let checked = check(BlockType::Video, Some(&file), None).unwrap();
        assert_eq!(checked.file_ids().len(), 1);
    }

    #[test]
    fn blocks_nest_as_the_types_allow() {
        use BlockType as B;
        assert!(may_hold(None, B::Paragraph).is_ok());
        assert!(may_hold(None, B::Column).is_err());
        assert!(may_hold(Some(B::Toggle), B::Paragraph).is_ok());
        assert!(may_hold(Some(B::Toggle), B::Column).is_err());
        for leaf in [
            B::Code,
            B::Math,
            B::Diagram,
            B::Divider,
            B::PageBreak,
            B::Image,
            B::Video,
            B::Audio,
            B::File,
            B::Table,
        ] {
            assert!(may_hold(Some(leaf), B::Paragraph).is_err(), "{leaf:?}");
        }
        assert!(may_hold(Some(B::Columns), B::Column).is_ok());
        assert!(may_hold(Some(B::Columns), B::Paragraph).is_err());
        assert!(may_hold(Some(B::Column), B::Paragraph).is_ok());
        assert!(may_hold(Some(B::Column), B::Columns).is_err());
        assert!(may_hold(Some(B::Column), B::Column).is_err());
        assert!(!columns_fit(1) && columns_fit(2) && columns_fit(6) && !columns_fit(7));
    }

    #[test]
    fn the_plain_text_of_a_block_is_what_search_finds() {
        let content = BlockContent::Inline(vec![
            text("Release "),
            Inline::Text {
                text: "notes".into(),
                marks: Some(vec![Mark::Bold]),
                href: None,
                color: None,
                background: None,
            },
            Inline::Break,
            Inline::Math {
                source: "x^2".into(),
            },
        ]);
        let checked = check(BlockType::Paragraph, None, Some(&content)).unwrap();
        assert_eq!(checked.text, "Release notes\nx^2");
        let diagram = check(
            BlockType::Diagram,
            Some(&props(json!({ "source": "graph TD; A-->B" }))),
            None,
        )
        .unwrap();
        assert_eq!(diagram.text, "graph TD; A-->B");
        let long = BlockContent::Inline(vec![text(&"é".repeat(SEARCH_TEXT_MAX_CHARS + 50))]);
        let checked = check(BlockType::Paragraph, None, Some(&long)).unwrap();
        assert_eq!(checked.text.chars().count(), SEARCH_TEXT_MAX_CHARS);
    }

    fn block(id: Id, parent: Option<Id>, position: &str, removed: bool) -> DocumentBlock {
        serde_json::from_value(json!({
            "id": id, "document_id": new_id(), "parent_id": parent, "position": position,
            "type": "paragraph", "props": {}, "content": [], "removed": removed,
            "seq_created": 1, "seq_updated": 1, "updated_by": null,
            "updated_at": "2026-10-08T17:04:05.123Z"
        }))
        .unwrap()
    }

    #[test]
    fn an_outline_orders_siblings_by_position_as_numbers_and_leaves_out_what_is_removed() {
        let (a, b, c, gone, child, orphan) =
            (new_id(), new_id(), new_id(), new_id(), new_id(), new_id());
        let blocks = vec![
            block(a, None, "10", false),
            block(b, None, "9", false),
            block(c, None, "9.5", false),
            block(gone, None, "9.7", true),
            block(child, Some(b), "1", false),
            block(orphan, Some(new_id()), "100", false),
        ];
        let outline = Outline::new(&blocks);
        let ids = |list: Vec<&DocumentBlock>| list.iter().map(|b| b.id).collect::<Vec<_>>();
        assert_eq!(ids(outline.roots()), [b, c, a, orphan]);
        assert_eq!(ids(outline.children_of(b)), [child]);
        assert!(outline.get(gone).is_none());
        let walked: Vec<(Id, usize)> = outline.walk().iter().map(|(b, n)| (b.id, *n)).collect();
        assert_eq!(walked, [(b, 0), (child, 1), (c, 0), (a, 0), (orphan, 0)]);
    }
}
