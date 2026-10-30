//! Markdown at the edges: one reader for what is pasted, imported or written by an agent, and one renderer for an export and for what an agent reads, whole, as its headings alone, or with the discussions beside it; it says as much of a document as Markdown can and names what it could not.
//!
//! What is stored is the tree of blocks, never a text to be parsed; Markdown
//! is how a document feels to type and what it is read from and written to at
//! its edges. The Markdown is GitHub's, with formulas between dollar signs and
//! diagrams as fenced `mermaid` blocks, and a callout is a GitHub alert
//! (`> [!WARNING]`). What GitHub's Markdown has no form for is written with a
//! closed set of tags, and only where a block needs one: `<details>` with a
//! `<summary>` for a toggle (`<toggle>` is read as well), `<columns>` holding
//! `<column ratio="2">`, `<span color="red" background="amber">` and `<u>` in a
//! line, `<video src>` and `<audio src>` for a film or a sound that is an
//! address, and `<page-break/>`. Any other tag is the text it was written
//! with. A mention is written `@username`, a reference as a link with the
//! thing's name (for an agent, its own path to the thing and the part's
//! number, `crate::references`). Bounded `orch:block` comments retain layout,
//! spans, presentation, blank blocks and otherwise flattened children beside
//! the ordinary Markdown. They carry no block or author identities. A round
//! trip makes the same content and layout with new identities; history and
//! discussions are outside it. [`read`] makes new blocks from the text.
//!
//! The reader takes what it can from any Markdown: it recognizes what the
//! renderer writes, so that what is written out and read in again gives the
//! same blocks for every type the text can carry, and it reads what other
//! tools write for the same things: a quote that opens with its tone in bold,
//! the tags above with their children indented, relative links and pictures
//! that lead to what is imported along with it. Nothing in a block is ever
//! HTML: a tag outside the set is kept as the text it is, and a link that
//! leads nowhere is kept as its words, never as a fault of the block.

mod layout;

use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;

use orch_api_contract::documents::{
    BlockContent, BlockType, CalloutTone, DocumentBlock, DocumentDiscussion, HEADING_LEVEL_MAX,
    Inline, InlineRefKind, Mark, NewBlock, TableCell, TableRow,
};
use orch_types::{Id, Json, Time, new_id};
use pulldown_cmark::{
    BlockQuoteKind, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd,
};

use crate::blocks::{self, Lookup, Outline};
use crate::references::{self, Form, Reference, Typed, Writing};
use crate::tags::{self, BlockTag, Container, Scanned};
use crate::warnings::{self, Warning};
use layout::Annotation;

/// The line that stands for a page break.
pub const PAGE_BREAK_LINE: &str = "<page-break/>";
/// The comment line that stood for a page break before the tag did; it is still read.
pub const LEGACY_PAGE_BREAK_LINE: &str = "<!-- page break -->";
/// The directory, beside the text, that the files of a document are named in where nothing says otherwise. A text
/// that lies as `<title>.md` links its files as `<title>.files/<name>` (see [`RenderOptions::files_dir`]).
pub const FILES_DIR: &str = "files";

/// The names of the twelve colours of the palette, in the order of their indexes. A tag says a colour by its
/// name; the same names are the screens' own.
pub const TINT_NAMES: [&str; 12] = [
    "red", "orange", "amber", "lime", "green", "teal", "cyan", "blue", "indigo", "purple", "pink",
    "grey",
];

/// The colour a name or a number stands for, and whether the name said it is a background (Notion writes
/// `red_bg`). Other tools' names are taken to the nearest colour of the palette: `gray` is grey, `yellow` amber,
/// `brown` orange.
#[must_use]
pub fn tint_from(word: &str) -> Option<(u32, bool)> {
    let lower = word.trim().to_ascii_lowercase();
    let (name, background) = match lower
        .strip_suffix("_bg")
        .or_else(|| lower.strip_suffix("_background"))
    {
        Some(name) => (name, true),
        None => (lower.as_str(), false),
    };
    let name = match name {
        "gray" => "grey",
        "yellow" => "amber",
        "brown" => "orange",
        other => other,
    };
    if let Some(index) = TINT_NAMES.iter().position(|each| *each == name) {
        return u32::try_from(index).ok().map(|index| (index, background));
    }
    name.parse::<u32>()
        .ok()
        .filter(|index| usize::try_from(*index).is_ok_and(|index| index < TINT_NAMES.len()))
        .map(|index| (index, background))
}

fn tint_name(tint: u32) -> Option<&'static str> {
    usize::try_from(tint)
        .ok()
        .and_then(|index| TINT_NAMES.get(index))
        .copied()
}

/// What the closed set of tags has no form for either, and so is written as the nearest thing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LossKind {
    /// A heading that folds is a heading.
    FoldingHeadings,
    /// A cell that spans several is its text in the first of them.
    Spans,
    /// The colour of a block, or of a cell, is left out.
    Colours,
    /// Alignment is left out.
    Alignment,
    /// Blocks under a block that cannot hold them come after it.
    Nesting,
    /// A table without a header row gets one.
    HeaderlessTables,
    /// The shares of a table's columns are left out.
    TableWidths,
}

/// How many blocks lost something to Markdown, of one kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Loss {
    /// What.
    pub kind: LossKind,
    /// In how many blocks.
    pub count: i64,
}

impl Loss {
    /// A sentence for a person about to export: "2 headings no longer fold".
    #[must_use]
    pub fn describe(&self) -> String {
        let n = self.count;
        let (one, many) = (n == 1, n != 1);
        let s = if many { "s" } else { "" };
        match self.kind {
            LossKind::FoldingHeadings => format!(
                "{n} heading{s} no longer fold{}",
                if one { "s" } else { "" }
            ),
            LossKind::Spans => format!(
                "{n} table{s} with cells that span several {} their text in the first of them",
                if one { "has" } else { "have" }
            ),
            LossKind::Colours => "colours of blocks and cells are left out".to_owned(),
            LossKind::Alignment => "alignment is left out".to_owned(),
            LossKind::Nesting => format!(
                "blocks under {n} block{s} that cannot hold them come after {}",
                if one { "it" } else { "them" }
            ),
            LossKind::HeaderlessTables => format!(
                "{n} table{s} {} a header row",
                if one { "gets" } else { "get" }
            ),
            LossKind::TableWidths => "the widths of table columns are left out".to_owned(),
        }
    }
}

/// A document written as Markdown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    /// The text.
    pub text: String,
    /// What it could not keep, in a fixed order.
    pub losses: Vec<Loss>,
}

/// What a rendering is asked to hold beside the text.
#[derive(Debug, Clone, Copy, Default)]
pub struct RenderOptions<'a> {
    /// The discussions beside the document, to be written after the text.
    pub discussions: Option<&'a [DocumentDiscussion]>,
    /// The directory the files of the document lie in, as the text links to them from where it lies: `<title>.files`
    /// beside a text that is written as `<title>.md`, and `../<title>.files` from a version in `<title>.history`. A
    /// link to a file is that directory, a slash and the file's name, so that it leads to the file wherever the text
    /// and its files lie together. [`FILES_DIR`] unless given.
    pub files_dir: Option<&'a str>,
    /// Keep authored reference labels in intermediate files before projecting them for each recipient.
    pub preserve_reference_labels: bool,
}

// ------------------------------------------------------------------ writing

const SPACE: &str = " ";
const HARD_BREAK: &str = "\\\n";
/// A bar written inside a table's cell, where a bare one would end the cell.
const BAR_IN_CELL: &str = "\\|";

/// The word of the GitHub alert that a tone is written as.
fn alert_word(tone: CalloutTone) -> &'static str {
    match tone {
        CalloutTone::Info => "NOTE",
        CalloutTone::Success => "TIP",
        CalloutTone::Warning => "WARNING",
        CalloutTone::Danger => "CAUTION",
    }
}

/// Puts `first` before the first line of `text` and `rest` before the others;
/// a line that is blank gets the prefix without its trailing space.
fn prefixed(text: &str, first: &str, rest: &str) -> String {
    let mut out = String::with_capacity(text.len() + first.len());
    for (index, line) in text.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        let prefix = if index == 0 { first } else { rest };
        if line.is_empty() {
            out.push_str(prefix.trim_end());
        } else {
            out.push_str(prefix);
            out.push_str(line);
        }
    }
    out
}

/// Text as it is written inside a line, so that it reads back as the same text.
fn escape_text(text: &str, at_line_start: bool, in_cell: bool) -> String {
    let mut out = String::with_capacity(text.len());
    let mut first = at_line_start;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '>' | '$' | '~' | '&' => {
                out.push('\\');
                out.push(c);
            }
            '|' if in_cell => out.push_str("\\|"),
            '#' | '+' | '-' | '=' | '|' if first => {
                out.push('\\');
                out.push(c);
            }
            '!' if chars.peek() == Some(&'[') => out.push_str("\\!"),
            '0'..='9' if first => {
                // "1. " at the start of a line is a list.
                let mut number = String::from(c);
                while let Some(next) = chars.peek().copied().filter(char::is_ascii_digit) {
                    number.push(next);
                    chars.next();
                }
                out.push_str(&number);
                if matches!(chars.peek(), Some('.' | ')')) {
                    out.push('\\');
                }
            }
            _ => out.push(c),
        }
        first = false;
    }
    out
}

fn code_span(text: &str) -> String {
    let longest = text.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest + 1);
    let pad = if text.starts_with('`')
        || text.ends_with('`')
        || text.starts_with(' ') && text.ends_with(' ') && !text.trim().is_empty()
    {
        SPACE
    } else {
        ""
    };
    format!("{fence}{pad}{text}{pad}{fence}")
}

fn destination(dest: &str) -> String {
    if dest
        .chars()
        .any(|c| c.is_whitespace() || c == '(' || c == ')' || c == '<' || c == '>')
    {
        let escaped: String = dest
            .chars()
            .map(|c| match c {
                '<' => "%3C".to_owned(),
                '>' => "%3E".to_owned(),
                c => c.to_string(),
            })
            .collect();
        format!("<{escaped}>")
    } else {
        dest.to_owned()
    }
}

/// The formatting a run of text has, for merging neighbours that share it.
type Look<'a> = (Vec<Mark>, Option<&'a str>, Option<u32>, Option<u32>);

fn look(run: &Inline) -> Option<Look<'_>> {
    match run {
        Inline::Text {
            marks,
            href,
            color,
            background,
            ..
        } => {
            let mut marks = marks.clone().unwrap_or_default();
            marks.sort_by_key(|mark| *mark as u8);
            marks.dedup();
            Some((marks, href.as_deref(), *color, *background))
        }
        _ => None,
    }
}

/// Neighbouring runs of text that look alike, as one.
#[must_use]
pub fn merged(runs: &[Inline]) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::with_capacity(runs.len());
    for run in runs {
        let alike = out
            .last()
            .is_some_and(|previous| look(previous).is_some() && look(previous) == look(run));
        if alike
            && let (Some(Inline::Text { text: last, .. }), Inline::Text { text, .. }) =
                (out.last_mut(), run)
        {
            last.push_str(text);
        } else {
            out.push(run.clone());
        }
    }
    out.retain(|run| !matches!(run, Inline::Text { text, .. } if text.is_empty()));
    out
}

struct Writer<'a> {
    lookup: &'a dyn Lookup,
    losses: BTreeMap<LossKind, HashSet<Id>>,
    /// Where a link to a file of the document leads: the directory the files lie in, from the text.
    files_dir: &'a str,
    preserve_reference_labels: bool,
}

impl<'a> Writer<'a> {
    /// Inline content as one line, with hard breaks where the content has
    /// them. `in_cell` writes a table cell: a break is `<br>`, a bar is escaped.
    fn inline(
        &mut self,
        runs: &[Inline],
        _block: Id,
        in_cell: bool,
        at_line_start: bool,
    ) -> String {
        let runs = merged(runs);
        let mut out = String::new();
        let mut start = at_line_start;
        let mut index = 0;
        while index < runs.len() {
            // Neighbouring runs under one address are one link.
            let href = match &runs[index] {
                Inline::Text {
                    href: Some(href), ..
                } => Some(href.as_str()),
                _ => None,
            };
            let mut end = index + 1;
            if href.is_some() {
                while end < runs.len()
                    && matches!(&runs[end], Inline::Text { href: other, .. } if other.as_deref() == href)
                {
                    end += 1;
                }
            }
            let mut inner = String::new();
            for run in &runs[index..end] {
                inner.push_str(&self.run(run, in_cell, &mut start));
            }
            match href {
                Some(href) => {
                    let _ = write!(out, "[{inner}]({})", destination(href));
                }
                None => out.push_str(&inner),
            }
            index = end;
        }
        // CommonMark trims a line's boundary spaces before producing text events. Entities keep those
        // content characters without changing the ordinary spaces between its inline runs.
        let leading = out.len() - out.trim_start_matches([' ', '\t']).len();
        let trailing = out.trim_end_matches([' ', '\t']).len().max(leading);
        if leading != 0 || trailing != out.len() {
            let entities = |space: &str| {
                space
                    .chars()
                    .map(|c| if c == ' ' { "&#32;" } else { "&#9;" })
                    .collect::<String>()
            };
            format!(
                "{}{}{}",
                entities(&out[..leading]),
                &out[leading..trailing],
                entities(&out[trailing..]),
            )
        } else {
            out
        }
    }

    fn run(&mut self, run: &Inline, in_cell: bool, start: &mut bool) -> String {
        match run {
            Inline::Text {
                text,
                marks,
                color,
                background,
                ..
            } => {
                let marks = marks.clone().unwrap_or_default();
                let has = |mark| marks.contains(&mark);
                let mut out = String::new();
                for (index, line) in text.split('\n').enumerate() {
                    if index > 0 {
                        out.push_str(if in_cell { "<br>" } else { HARD_BREAK });
                        *start = true;
                    }
                    if line.is_empty() {
                        continue;
                    }
                    let (lead, body, trail) = split_space(line);
                    out.push_str(lead);
                    let written = if has(Mark::Code) {
                        wrap(&code_span(body), &marks)
                    } else {
                        wrap(
                            &escape_text(body, *start && lead.is_empty(), in_cell),
                            &marks,
                        )
                    };
                    out.push_str(&tinted(written, *color, *background));
                    out.push_str(trail);
                    *start = false;
                }
                out
            }
            Inline::Mention { user_id, .. } => {
                *start = false;
                match self.lookup.username(*user_id) {
                    Some(name) => format!("@{name}"),
                    None => "@unknown".to_owned(),
                }
            }
            Inline::Ref {
                kind,
                id,
                path,
                part,
                label,
                ..
            } => {
                *start = false;
                match self
                    .lookup
                    .writing_path(*kind, *id, path.as_deref(), part.as_ref())
                {
                    Some(Writing::Link { name, path, part }) => {
                        let destination = references::destination(&path, part);
                        // A bar inside a table's cell would end the cell.
                        let destination = if in_cell {
                            destination.replace('|', BAR_IN_CELL)
                        } else {
                            destination
                        };
                        format!("[{}]({destination})", references::label(&name))
                    }
                    Some(Writing::Hidden(_)) => references::no_access(label.as_deref(), *kind),
                    None => {
                        let what = match kind {
                            InlineRefKind::Node => "a node",
                            InlineRefKind::Document => "a document",
                            InlineRefKind::Chat => "a chat",
                            InlineRefKind::File | InlineRefKind::EnvironmentFile => "a file",
                        };
                        let name = if self.preserve_reference_labels {
                            label.clone().unwrap_or_else(|| what.to_owned())
                        } else {
                            self.lookup
                                .name_of(*kind, *id)
                                .unwrap_or_else(|| what.to_owned())
                        };
                        let typed = Typed {
                            kind: *kind,
                            id: *id,
                            path: path.clone(),
                            part: *part,
                        };
                        format!(
                            "[{}]({})",
                            escape_text(&name, false, in_cell),
                            references::address(&typed)
                        )
                    }
                }
            }
            Inline::Math { source } => {
                *start = false;
                if source.is_empty()
                    || source.trim() != source
                    || source.contains(['$', '\n', '\r'])
                    || in_cell && source.contains('|')
                {
                    use base64::Engine as _;
                    let encoded =
                        base64::engine::general_purpose::STANDARD.encode(source.as_bytes());
                    let body = if source.is_empty() {
                        String::new()
                    } else {
                        // Keep a table row on one physical line too; the attribute retains the original newlines.
                        let visible = source.replace("\r\n", "\n").replace(['\r', '\n'], " ");
                        let code = code_span(&visible);
                        if in_cell {
                            code.replace('|', "\\|")
                        } else {
                            code
                        }
                    };
                    format!("<span orch-math=\"{encoded}\">{body}</span>")
                } else {
                    format!("${source}$")
                }
            }
            Inline::Break => {
                *start = true;
                (if in_cell { "<br>" } else { HARD_BREAK }).to_owned()
            }
        }
    }
}

fn split_space(line: &str) -> (&str, &str, &str) {
    let body = line.trim();
    if body.is_empty() {
        return (line, "", "");
    }
    let lead_len = line.len() - line.trim_start().len();
    let trail_len = line.len() - line.trim_end().len();
    (&line[..lead_len], body, &line[line.len() - trail_len..])
}

/// Wraps already escaped text in the delimiters of its marks: underline
/// outermost, then strike-through, then bold, then italic.
fn wrap(text: &str, marks: &[Mark]) -> String {
    let mut out = text.to_owned();
    if marks.contains(&Mark::Italic) {
        out = format!("*{out}*");
    }
    if marks.contains(&Mark::Bold) {
        out = format!("**{out}**");
    }
    if marks.contains(&Mark::Strike) {
        out = format!("~~{out}~~");
    }
    if marks.contains(&Mark::Underline) {
        out = format!("<u>{out}</u>");
    }
    out
}

/// Text in the colours it has, as a `<span>` that says them; text without colours as it is.
fn tinted(text: String, color: Option<u32>, background: Option<u32>) -> String {
    let color = color.and_then(tint_name);
    let background = background.and_then(tint_name);
    if color.is_none() && background.is_none() {
        return text;
    }
    let mut open = String::from("<span");
    if let Some(name) = color {
        let _ = write!(open, " color=\"{name}\"");
    }
    if let Some(name) = background {
        let _ = write!(open, " background=\"{name}\"");
    }
    format!("{open}>{text}</span>")
}

fn prop(block: &DocumentBlock, member: &str) -> Option<Json> {
    serde_json::to_value(&block.props)
        .ok()
        .and_then(|props| props.get(member).cloned())
}

fn runs_of(block: &DocumentBlock) -> &[Inline] {
    match &block.content {
        BlockContent::Inline(runs) => runs,
        BlockContent::Rows(_) => &[],
    }
}

struct Render<'a, 'b> {
    writer: Writer<'a>,
    outline: &'b Outline<'b>,
    suppress_children: Option<Id>,
}

impl<'a, 'b> Render<'a, 'b> {
    fn children_of(&self, id: Id) -> Vec<&'b DocumentBlock> {
        if self.suppress_children == Some(id) {
            Vec::new()
        } else {
            self.outline.children_of(id)
        }
    }

    fn is_list_item(block: &DocumentBlock) -> bool {
        matches!(
            block.block_type,
            BlockType::Bullet | BlockType::Todo | BlockType::Numbered
        )
    }

    fn sequence(&mut self, siblings: &[&'b DocumentBlock]) -> String {
        let mut chunks: Vec<String> = Vec::new();
        let mut list: Vec<String> = Vec::new();
        let mut ordered = false;
        let mut number = 0_u64;
        let flush = |list: &mut Vec<String>, chunks: &mut Vec<String>| {
            if !list.is_empty() {
                chunks.push(list.join("\n"));
                list.clear();
            }
        };
        // A layout annotation separates list items in Markdown. Preserve the whole affected numbered run so
        // later displayed numbers do not turn into new `start` properties on import.
        let mut numbered_layout = vec![false; siblings.len()];
        let mut first = 0;
        while first < siblings.len() {
            if siblings[first].block_type != BlockType::Numbered {
                first += 1;
                continue;
            }
            let mut end = first + 1;
            while end < siblings.len() && siblings[end].block_type == BlockType::Numbered {
                end += 1;
            }
            let annotated = siblings[first..end].iter().any(|block| {
                Annotation::of(block, !self.children_of(block.id).is_empty(), false).is_some()
            });
            numbered_layout[first..end].fill(annotated);
            first = end;
        }
        for (at, block) in siblings.iter().enumerate() {
            let children = self.children_of(block.id);
            if let Some(annotation) =
                Annotation::of(block, !children.is_empty(), numbered_layout[at])
            {
                let next = if block.block_type == BlockType::Numbered {
                    if !ordered {
                        number = prop(block, "start").and_then(|v| v.as_u64()).unwrap_or(1);
                    } else {
                        number += 1;
                    }
                    ordered = true;
                    Some(number)
                } else {
                    ordered = false;
                    None
                };
                flush(&mut list, &mut chunks);
                chunks.push(self.annotated(block, annotation, next));
                continue;
            }
            match block.block_type {
                BlockType::Bullet | BlockType::Todo => {
                    if ordered {
                        flush(&mut list, &mut chunks);
                    }
                    ordered = false;
                    list.push(self.item(block, None));
                }
                BlockType::Numbered => {
                    if !ordered {
                        flush(&mut list, &mut chunks);
                    }
                    if list.is_empty() {
                        number = prop(block, "start")
                            .and_then(|start| start.as_u64())
                            .unwrap_or(1);
                    } else {
                        number += 1;
                    }
                    ordered = true;
                    list.push(self.item(block, Some(number)));
                }
                BlockType::Toggle => {
                    flush(&mut list, &mut chunks);
                    chunks.push(self.toggle(block));
                }
                BlockType::Columns => {
                    flush(&mut list, &mut chunks);
                    chunks.push(self.columns(block));
                }
                BlockType::Column => {
                    flush(&mut list, &mut chunks);
                    let inside = self.children_of(block.id);
                    let text = self.sequence(&inside);
                    if !text.is_empty() {
                        chunks.push(text);
                    }
                }
                _ => {
                    flush(&mut list, &mut chunks);
                    if let Some(text) = self.block(block) {
                        chunks.push(text);
                    }
                    let children = self.children_of(block.id);
                    if !children.is_empty()
                        && !matches!(block.block_type, BlockType::Quote | BlockType::Callout)
                    {
                        let text = self.sequence(&children);
                        if !text.is_empty() {
                            chunks.push(text);
                        }
                    }
                }
            }
        }
        flush(&mut list, &mut chunks);
        chunks.join("\n\n")
    }

    fn annotated(
        &mut self,
        block: &'b DocumentBlock,
        mut annotation: Annotation,
        number: Option<u64>,
    ) -> String {
        if annotation.caption.is_some() {
            annotation.caption = Some(self.writer.inline(runs_of(block), block.id, false, false));
        }
        let previous = self.suppress_children;
        if annotation.children {
            self.suppress_children = Some(block.id);
        }
        let mut body = match block.block_type {
            BlockType::Bullet | BlockType::Todo | BlockType::Numbered => self.item(block, number),
            BlockType::Toggle => self.toggle(block),
            BlockType::Columns => self.columns(block),
            _ => self.block(block).unwrap_or_default(),
        };
        self.suppress_children = previous;
        if annotation.children {
            body.push_str("\n\n<!-- orch:children -->\n\n");
            body.push_str(&self.sequence(&self.children_of(block.id)));
        }
        let metadata = serde_json::to_string(&annotation)
            .unwrap_or_default()
            .replace("-->", "\\u002d->");
        format!("<!-- orch:block {metadata} -->\n\n{body}\n\n<!-- /orch:block -->")
    }

    /// One item of a list, with the blocks under it indented to its text.
    fn item(&mut self, block: &'b DocumentBlock, number: Option<u64>) -> String {
        let marker = match (block.block_type, number) {
            (BlockType::Todo, _) => {
                let ticked = prop(block, "checked")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if ticked {
                    "- [x] ".to_owned()
                } else {
                    "- [ ] ".to_owned()
                }
            }
            (BlockType::Numbered, Some(n)) => format!("{n}. "),
            _ => "- ".to_owned(),
        };
        let text = self.writer.inline(runs_of(block), block.id, false, true);
        let indent = " ".repeat(marker.len());
        let mut out = prefixed(&text, &marker, &indent);
        let children = self.children_of(block.id);
        if !children.is_empty() {
            let inside = self.sequence(&children);
            let tight = children.iter().all(|child| Self::is_list_item(child));
            out.push_str(if tight { "\n" } else { "\n\n" });
            out.push_str(&prefixed(&inside, &indent, &indent));
        }
        out
    }

    /// A toggle: its line in a `<summary>`, and the blocks under it between the tags, as people and tools that know
    /// `<details>` write it.
    fn toggle(&mut self, block: &'b DocumentBlock) -> String {
        let title = self
            .writer
            .inline(runs_of(block), block.id, false, false)
            .replace(HARD_BREAK, "<br>");
        let mut out = format!("<details>\n<summary>{title}</summary>");
        let children = self.children_of(block.id);
        if !children.is_empty() {
            out.push_str("\n\n");
            out.push_str(&self.sequence(&children));
        }
        out.push_str("\n\n</details>");
        out
    }

    /// Columns: each between its own tags, with the share it has as its `ratio`.
    fn columns(&mut self, block: &'b DocumentBlock) -> String {
        let mut out = String::from("<columns>");
        for column in self.children_of(block.id) {
            let share = prop(column, "share")
                .and_then(|v| v.as_f64())
                .filter(|share| share.is_finite() && *share > 0.0);
            match share {
                Some(share) => {
                    let _ = write!(out, "\n\n<column ratio=\"{share}\">");
                }
                None => out.push_str("\n\n<column>"),
            }
            let inside = self.children_of(column.id);
            let text = self.sequence(&inside);
            if !text.is_empty() {
                out.push_str("\n\n");
                out.push_str(&text);
            }
            out.push_str("\n\n</column>");
        }
        out.push_str("\n\n</columns>");
        out
    }

    fn file_destination(&self, id: Id) -> (String, Option<String>) {
        match self.writer.lookup.file(id) {
            Some(file) => {
                let name = file.name;
                (format!("{}/{name}", self.writer.files_dir), Some(name))
            }
            None => (format!("{}/{id}", self.writer.files_dir), None),
        }
    }

    fn block(&mut self, block: &'b DocumentBlock) -> Option<String> {
        let id = block.id;
        match block.block_type {
            BlockType::Paragraph => {
                let text = self.writer.inline(runs_of(block), id, false, true);
                (!text.trim().is_empty()).then_some(text)
            }
            BlockType::Heading => {
                let level = prop(block, "level")
                    .and_then(|v| v.as_u64())
                    .map_or(1, |level| level.clamp(1, u64::from(HEADING_LEVEL_MAX)));
                let text = self.writer.inline(runs_of(block), id, false, false);
                let text = text.replace(HARD_BREAK, "<br>");
                let hashes = "#".repeat(usize::try_from(level).unwrap_or(1));
                Some(format!("{hashes} {text}").trim_end().to_owned())
            }
            BlockType::Quote | BlockType::Callout => {
                let callout = block.block_type == BlockType::Callout;
                let mut text = self.writer.inline(runs_of(block), id, false, true);
                if callout {
                    let tone = prop(block, "tone")
                        .and_then(|v| serde_json::from_value::<CalloutTone>(v).ok())
                        .unwrap_or(CalloutTone::Info);
                    let alert = alert_word(tone);
                    text = if text.is_empty() {
                        format!("[!{alert}]")
                    } else {
                        format!("[!{alert}]\n{text}")
                    };
                }
                let children = self.children_of(id);
                let mut inside = text;
                if !children.is_empty() {
                    let more = self.sequence(&children);
                    if inside.is_empty() {
                        inside = more;
                    } else {
                        inside.push_str("\n\n");
                        inside.push_str(&more);
                    }
                }
                if inside.is_empty() {
                    return Some(">".to_owned());
                }
                Some(prefixed(&inside, "> ", "> "))
            }
            BlockType::Code => {
                let language = prop(block, "language")
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_default();
                let language: String = language
                    .chars()
                    .filter(|c| !c.is_whitespace() && *c != '`')
                    .collect();
                let body = blocks::inline_text(runs_of(block));
                Some(fenced(&language, &body))
            }
            BlockType::Diagram => {
                let source = prop(block, "source")
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_default();
                Some(fenced("mermaid", &source))
            }
            BlockType::Math => {
                let source = prop(block, "source")
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_default();
                let core = source.trim();
                Some(if core.is_empty() {
                    "$$ $$".to_owned()
                } else {
                    format!("$$\n{core}\n$$")
                })
            }
            BlockType::Divider => Some("---".to_owned()),
            BlockType::PageBreak => Some(PAGE_BREAK_LINE.to_owned()),
            BlockType::Image | BlockType::Video | BlockType::Audio | BlockType::File => {
                self.media(block)
            }
            BlockType::Table => self.table(block),
            BlockType::Bullet
            | BlockType::Numbered
            | BlockType::Todo
            | BlockType::Toggle
            | BlockType::Columns
            | BlockType::Column => None,
        }
    }

    fn media(&mut self, block: &'b DocumentBlock) -> Option<String> {
        let id = block.id;
        let (dest, name) = match prop(block, "file_id")
            .and_then(|v| v.as_str().and_then(|s| s.parse::<Id>().ok()))
        {
            Some(file) => self.file_destination(file),
            None => {
                let url = prop(block, "url").and_then(|v| v.as_str().map(str::to_owned))?;
                let tag = if block.block_type == BlockType::Audio {
                    "audio"
                } else {
                    "video"
                };
                let caption = self
                    .writer
                    .inline(runs_of(block), id, false, false)
                    .replace(HARD_BREAK, "<br>");
                let url = url
                    .replace('"', "%22")
                    .replace('<', "%3C")
                    .replace('>', "%3E");
                return Some(format!("<{tag} src=\"{url}\">{caption}</{tag}>"));
            }
        };
        if block.block_type == BlockType::Image {
            let alt = escape_text(&blocks::inline_text(runs_of(block)), false, false);
            return Some(format!("![{alt}]({})", destination(&dest)));
        }
        let caption = self.writer.inline(runs_of(block), id, false, false);
        let caption = if caption.trim().is_empty() {
            escape_text(&name.unwrap_or_else(|| dest.clone()), false, false)
        } else {
            caption.replace(HARD_BREAK, "<br>")
        };
        Some(format!("[{caption}]({})", destination(&dest)))
    }

    fn table(&mut self, block: &'b DocumentBlock) -> Option<String> {
        let BlockContent::Rows(rows) = &block.content else {
            return None;
        };
        let id = block.id;
        let first = rows.first()?;
        let span = |value: Option<u32>| usize::try_from(value.unwrap_or(1)).unwrap_or(1).max(1);
        let width: usize = first.cells.iter().map(|cell| span(cell.colspan)).sum();
        if width == 0 {
            return None;
        }
        // Lay the cells out on the grid, skipping the places a span from above covers.
        let mut grid: Vec<Vec<Option<String>>> = vec![vec![None; width]; rows.len()];
        let mut covered = vec![vec![false; width]; rows.len()];
        for (r, row) in rows.iter().enumerate() {
            let mut column = 0;
            for cell in &row.cells {
                while column < width && covered[r][column] {
                    column += 1;
                }
                if column >= width {
                    break;
                }
                let (colspan, rowspan) = (span(cell.colspan), span(cell.rowspan));
                grid[r][column] = Some(self.writer.inline(&cell.content, id, true, false));
                let down = (r + rowspan).min(rows.len());
                let across = (column + colspan).min(width);
                for covered_row in &mut covered[r..down] {
                    covered_row[column..across].fill(true);
                }
                column += colspan;
            }
        }
        let line = |cells: &[Option<String>]| {
            let inner: Vec<String> = cells
                .iter()
                .map(|cell| cell.clone().unwrap_or_default())
                .collect();
            format!("| {} |", inner.join(" | "))
        };
        let mut lines = vec![line(&grid[0])];
        lines.push(format!("|{}|", vec![" --- "; width].join("|")));
        for row in &grid[1..] {
            lines.push(line(row));
        }
        Some(lines.join("\n"))
    }
}

fn fenced(language: &str, body: &str) -> String {
    let longest = body
        .lines()
        .map(|line| line.trim_start().chars().take_while(|c| *c == '`').count())
        .max()
        .unwrap_or(0);
    let fence = "`".repeat(longest.max(2) + 1);
    format!("{fence}{language}\n{body}\n{fence}")
}

fn block_number_of(blocks: &[DocumentBlock]) -> Vec<(usize, &DocumentBlock)> {
    Outline::new(blocks)
        .walk()
        .into_iter()
        .enumerate()
        .map(|(at, (block, _))| (at + 1, block))
        .collect()
}

/// The words, shortened, that say which block a discussion is about.
fn excerpt(text: &str) -> String {
    const EXCERPT_CHARS: usize = 60;
    let line = text.lines().next().unwrap_or("").trim();
    if line.chars().count() > EXCERPT_CHARS {
        let mut cut: String = line.chars().take(EXCERPT_CHARS).collect();
        cut.push('…');
        cut
    } else {
        line.to_owned()
    }
}

fn moment(time: Time) -> String {
    time.get().format("%Y-%m-%d %H:%M UTC").to_string()
}

fn discussions_text(
    writer: &mut Writer<'_>,
    blocks: &[DocumentBlock],
    discussions: &[DocumentDiscussion],
) -> String {
    let numbered = block_number_of(blocks);
    let order = |discussion: &DocumentDiscussion| {
        discussion
            .block_id
            .and_then(|id| numbered.iter().find(|(_, block)| block.id == id))
            .map_or(usize::MAX, |(number, _)| *number)
    };
    let mut sorted: Vec<&DocumentDiscussion> = discussions.iter().collect();
    sorted.sort_by_key(|discussion| (order(discussion), discussion.created_at));
    let mut out = vec!["## Comments".to_owned()];
    for discussion in sorted {
        let about = match discussion
            .block_id
            .and_then(|id| numbered.iter().find(|(_, block)| block.id == id))
        {
            Some((number, block)) => {
                let text = blocks::plain_text(
                    block.block_type,
                    &serde_json::to_value(&block.props).unwrap_or(Json::Null),
                    &block.content,
                );
                format!("block {number}, “{}”", excerpt(&text))
            }
            None => "the document as a whole".to_owned(),
        };
        let mut head = format!("**On {about}**");
        if !discussion.quote.is_empty() {
            let _ = write!(
                head,
                "\n\n> {}",
                escape_text(&discussion.quote.replace('\n', " "), true, false)
            );
        }
        if let Some(resolved) = &discussion.resolved {
            let _ = write!(
                head,
                "\n\n(resolved by {} at {})",
                escape_text(&resolved.by.display_name, false, false),
                moment(resolved.at)
            );
        }
        out.push(head);
        let mut items = Vec::new();
        for comment in &discussion.comments {
            let who = format!(
                "{} (@{})",
                escape_text(&comment.author.display_name, false, false),
                comment.author.username
            );
            let said = if comment.deleted {
                "(deleted)".to_owned()
            } else {
                writer
                    .inline(&comment.content, comment.id, false, false)
                    .replace(HARD_BREAK, SPACE)
            };
            items.push(format!("- {who}, {}: {said}", moment(comment.created_at)));
        }
        out.push(items.join("\n"));
    }
    out.join("\n\n")
}

/// Writes the live blocks of a document as Markdown, and says what Markdown
/// could not keep of them.
#[must_use]
pub fn render(
    blocks: &[DocumentBlock],
    lookup: &dyn Lookup,
    options: &RenderOptions<'_>,
) -> Rendered {
    let outline = Outline::new(blocks);
    let mut render = Render {
        suppress_children: None,
        writer: Writer {
            lookup,
            losses: BTreeMap::new(),
            files_dir: options.files_dir.unwrap_or(FILES_DIR),
            preserve_reference_labels: options.preserve_reference_labels,
        },
        outline: &outline,
    };
    let roots = outline.roots();
    let mut text = render.sequence(&roots);
    if let Some(discussions) = options.discussions
        && !discussions.is_empty()
    {
        let more = discussions_text(&mut render.writer, blocks, discussions);
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str(&more);
    }
    if !text.is_empty() {
        text.push('\n');
    }
    let losses = render
        .writer
        .losses
        .into_iter()
        .map(|(kind, blocks)| Loss {
            kind,
            count: i64::try_from(blocks.len()).unwrap_or(i64::MAX),
        })
        .collect();
    Rendered { text, losses }
}

/// The headings of a document alone, one a line, each after the number of the
/// block it begins at (the blocks counted in the order of the page, from one).
#[must_use]
pub fn headings(blocks: &[DocumentBlock]) -> String {
    let mut out = String::new();
    for (number, block) in block_number_of(blocks) {
        if block.block_type != BlockType::Heading {
            continue;
        }
        let level = prop(block, "level")
            .and_then(|v| v.as_u64())
            .map_or(1, |level| level.clamp(1, u64::from(HEADING_LEVEL_MAX)));
        let text = blocks::inline_text(runs_of(block)).replace('\n', " ");
        let hashes = "#".repeat(usize::try_from(level).unwrap_or(1));
        let _ = writeln!(out, "{number}: {hashes} {text}");
    }
    out
}

/// The line that precedes a document read into an agent's context: its
/// title, and who changed it last and when.
#[must_use]
pub fn context_line(title: &str, changed_by: &str, at: Time) -> String {
    format!("{title} (last changed by {changed_by} at {})", moment(at))
}

// ------------------------------------------------------------------ reading

/// A file of the document, as the reader finds it by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRef {
    /// Its identifier.
    pub id: Id,
    /// Its type.
    pub media_type: String,
}

/// What the reader may ask about the names a text uses.
pub trait Directory {
    /// The person a name is that they are mentioned by.
    fn user_id(&self, _username: &str) -> Option<Id> {
        None
    }

    /// The file of the document with that name.
    fn file(&self, _name: &str) -> Option<FileRef> {
        None
    }

    /// What a link's destination, or a text in backquotes, is as a reference: the typed reference it was
    /// resolved to, words to keep without the link (a link meant as a reference that did not resolve), or
    /// nothing to do with references. Untouched unless a caller resolved the text's references first.
    fn reference(&self, _text: &str, _form: Form) -> Reference {
        Reference::Untouched
    }

    /// The reference that the hidden words of this number stand for, where the text being read was given to a
    /// reader who could not read it and is written back (see [`references::hidden`]).
    fn hidden(&self, _number: usize) -> Option<Typed> {
        None
    }

    /// Shared no-access words that can be restored without looking up a hidden current name.
    fn hidden_names(&self) -> Vec<(String, Typed)> {
        Vec::new()
    }

    /// What a path leads to that a text wrote relative to itself (`images/a.png`, `../Plan.md`), where the text
    /// is read along with what it came with: an archive, a directory of the workspace. The path is as written,
    /// without its address part, and decoded. Nothing by default: a path that leads nowhere is the words it
    /// was written with, and the reader says so.
    fn local(&self, _path: &str) -> Option<Local> {
        None
    }
}

/// What a relative path leads to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Local {
    /// A file that is, or is to become, a file of the document: a picture, a film, a sound, anything.
    File(FileRef),
    /// Another text that is read with this one: the reference to the document it becomes.
    Document(Typed),
}

/// A file found inside what was read: a picture a text carries in its own address, or one a Word file holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Embedded {
    /// The identifier its blocks name it by; the caller makes it a file of the document under it.
    pub id: Id,
    /// A name for it, from where it was found.
    pub name: String,
    /// Its type.
    pub media_type: String,
    /// Its bytes.
    pub bytes: Vec<u8>,
}

/// What a text said, with what could not be kept of it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Read {
    /// The blocks, with new identifiers.
    pub blocks: Vec<NewBlock>,
    /// What was kept as words or left out, each once, with a code and its parameters and the English sentence.
    pub warnings: Vec<Warning>,
    /// The relative paths that led to nothing, each once.
    pub missing: Vec<String>,
    /// The files the text carried inside it.
    pub embedded: Vec<Embedded>,
}

/// Knows nobody and no file.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoDirectory;

impl Directory for NoDirectory {}

#[derive(Debug, Clone, Copy, Default)]
struct Formats {
    bold: u32,
    italic: u32,
    strike: u32,
}

/// What an inline tag does to the words between it and its partner.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct TagLook {
    underline: bool,
    color: Option<u32>,
    background: Option<u32>,
    math_source: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Element {
    Underline,
    Span,
}

/// How many looks (`<u>`, `<span>`) may be open at once inside a block: more are words. A writing nests two or three;
/// the bound keeps the pairing of a line of thousands of tags linear.
const LOOKS_DEPTH_MAX: usize = 16;

/// How many containers (quotes, lists and their items) may be open at once: a document holds sixteen levels, and the
/// reader takes twice that; a container opened beyond it is read as if it were not there, so that the blocks inside
/// stay and a text of hundreds of thousands of nested quotes is read in the time of its length.
const FRAMES_MAX: usize = 64;

enum Part {
    Run(Inline),
    Image {
        dest: String,
        alt: String,
    },
    Display(String),
    /// A tag that opens a look, until its partner is found.
    Open {
        raw: String,
        element: Element,
        look: TagLook,
    },
    /// A tag that closes one.
    Close {
        raw: String,
        element: Element,
    },
    /// A tag that was paired and has done its work.
    Spent,
}

enum Frame {
    Layout {
        // Only annotated blocks allocate this metadata; ordinary reader frames keep their small stack footprint.
        annotation: Option<Box<Annotation>>,
        written: String,
        own: Option<Vec<NewBlock>>,
        blocks: Vec<NewBlock>,
    },
    Doc(Vec<NewBlock>),
    Quote {
        kind: Option<BlockQuoteKind>,
        lead: Option<Vec<Inline>>,
        children: Vec<NewBlock>,
    },
    List {
        start: Option<u64>,
        items: Vec<NewBlock>,
    },
    Item {
        checked: Option<bool>,
        lead: Option<Vec<Inline>>,
        children: Vec<NewBlock>,
    },
    Toggle {
        lead: Option<Vec<Inline>>,
        children: Vec<NewBlock>,
    },
    Columns {
        columns: Vec<NewBlock>,
        stray: Vec<NewBlock>,
    },
    Column {
        share: Option<f64>,
        children: Vec<NewBlock>,
    },
    Paragraph,
    Heading(u32),
    Table {
        rows: Vec<Vec<TableCell>>,
        row: Vec<TableCell>,
    },
    Cell,
}

struct Reader<'a> {
    directory: &'a dyn Directory,
    frames: Vec<Frame>,
    parts: Vec<Part>,
    formats: Formats,
    links: Vec<String>,
    image: Option<(String, String)>,
    code: Option<(String, String)>,
    html: Option<String>,
    /// The tags of the set that the text was scanned for, and what stands for each in the text.
    scanned: Option<Scanned>,
    warnings: Vec<Warning>,
    missing: Vec<String>,
    embedded: Vec<Embedded>,
    /// For each quote, list and item that is open: whether it was read as if it were not there, for being too deep.
    skips: Vec<bool>,
}

fn block(block_type: BlockType) -> NewBlock {
    NewBlock {
        id: new_id(),
        block_type,
        props: None,
        content: None,
        children: None,
    }
}

fn with_props(mut made: NewBlock, props: Json) -> NewBlock {
    made.props = serde_json::from_value(props).ok();
    made
}

fn with_text(mut made: NewBlock, runs: Vec<Inline>) -> NewBlock {
    made.content = Some(BlockContent::Inline(runs));
    made
}

fn with_children(mut made: NewBlock, children: Vec<NewBlock>) -> NewBlock {
    made.children = (!children.is_empty()).then_some(children);
    made
}

fn text_run(text: &str, formats: Formats, link: Option<&str>, code: bool) -> Inline {
    let mut marks = Vec::new();
    if formats.bold > 0 {
        marks.push(Mark::Bold);
    }
    if formats.italic > 0 {
        marks.push(Mark::Italic);
    }
    if formats.strike > 0 {
        marks.push(Mark::Strike);
    }
    if code {
        marks.push(Mark::Code);
    }
    Inline::Text {
        text: text.to_owned(),
        marks: (!marks.is_empty()).then_some(marks),
        href: link.map(str::to_owned),
        color: None,
        background: None,
    }
}

/// A typed reference as a chip in a line.
fn chip_of(typed: Typed) -> Inline {
    Inline::Ref {
        display: None,
        kind: typed.kind,
        id: typed.id,
        path: typed.path,
        part: typed.part,
        name: None,
        label: None,
    }
}

fn percent_decoded(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && let Some(hex) = text.get(index + 1..index + 3)
            && let Ok(byte) = u8::from_str_radix(hex, 16)
        {
            out.push(byte);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

impl<'a> Reader<'a> {
    fn new(directory: &'a dyn Directory) -> Self {
        Self {
            directory,
            frames: vec![Frame::Doc(Vec::new())],
            parts: Vec::new(),
            formats: Formats::default(),
            links: Vec::new(),
            image: None,
            code: None,
            html: None,
            scanned: None,
            warnings: Vec::new(),
            missing: Vec::new(),
            embedded: Vec::new(),
            skips: Vec::new(),
        }
    }

    /// Whether a container that opens now is read as if it were not there, for the stack being at its depth.
    fn skip_container(&mut self) -> bool {
        let skip = self.frames.len() >= FRAMES_MAX;
        self.skips.push(skip);
        skip
    }

    /// Whether the container that ends now was skipped when it opened.
    fn skipped(&mut self) -> bool {
        self.skips.pop().unwrap_or(false)
    }

    /// Says something about the text once.
    fn warn(&mut self, warning: Warning) {
        warnings::note(&mut self.warnings, warning);
    }

    /// Notes a path that led nowhere, once.
    fn lost(&mut self, path: &str) {
        if self.missing.len() < warnings::LISTED_MAX
            && !self.missing.iter().any(|each| each == path)
        {
            self.missing.push(path.to_owned());
        }
    }

    /// The file of the document that a link names: a path whose directory is `files` or ends in `.files` (the
    /// directory a text links its files in, `<title>.files`), from the place the text lies in or one level above it.
    fn file_named(&self, dest: &str) -> Option<FileRef> {
        let mut path = dest;
        while let Some(rest) = path.strip_prefix("./").or_else(|| path.strip_prefix("../")) {
            path = rest;
        }
        let (directory, name) = path.split_once('/')?;
        let ours = directory == FILES_DIR || directory.ends_with(&format!(".{FILES_DIR}"));
        if !ours || name.is_empty() {
            return None;
        }
        self.directory.file(&percent_decoded(name))
    }

    /// The file that a destination leads to: a file of the document, by the directory the files are linked in, or
    /// one that came along with the text.
    fn file_for(&self, dest: &str) -> Option<FileRef> {
        if let Some(file) = self.file_named(dest) {
            return Some(file);
        }
        let path = relative_path(dest)?;
        match self.directory.local(&path) {
            Some(Local::File(file)) => Some(file),
            _ => None,
        }
    }

    /// A picture whose bytes the text carries in its address, as a file the document is to hold.
    fn embed(&mut self, dest: &str) -> Option<FileRef> {
        let (media_type, bytes) = data_image(dest)?;
        let id = new_id();
        let name = format!(
            "image-{}.{}",
            self.embedded.len() + 1,
            image_extension(&media_type)
        );
        self.embedded.push(Embedded {
            id,
            name,
            media_type: media_type.clone(),
            bytes,
        });
        Some(FileRef { id, media_type })
    }

    /// The words that stand for a picture that could not be a block: what it said it was, with its address where
    /// that may be followed.
    fn image_words(&mut self, dest: String, alt: String) -> Inline {
        let allowed = blocks::allowed_href(&dest);
        if dest.starts_with("data:") {
            self.warn(Warning::new("reader.image_inline", &[]));
        } else if allowed && dest.contains("://") {
            self.warn(Warning::new("reader.image_remote", &[]));
        } else if let Some(path) = relative_path(&dest) {
            self.warn(Warning::new("reader.image_missing", &[("path", &path)]));
            self.lost(&path);
        }
        let label = if !alt.is_empty() {
            alt
        } else if dest.starts_with("data:") {
            "image".to_owned()
        } else {
            dest.clone()
        };
        Inline::Text {
            text: label,
            marks: None,
            href: allowed.then_some(dest),
            color: None,
            background: None,
        }
    }

    /// Pairs the tags that look at words (`<u>`, `<span>`) and gives the words between each pair their look; a tag
    /// that has no partner is the text it was written as.
    fn looks(&self, parts: Vec<Part>) -> Vec<Part> {
        let mut out: Vec<Part> = Vec::with_capacity(parts.len());
        let mut open: Vec<(usize, Element, TagLook)> = Vec::new();
        for part in parts {
            match part {
                Part::Open { element, look, raw } => {
                    // Looks inside looks are kept to a depth no writing needs, so that a line of thousands of
                    // opening tags is words and is read at once.
                    if open.len() >= LOOKS_DEPTH_MAX {
                        out.push(Part::Run(text_run(&raw, Formats::default(), None, false)));
                    } else {
                        open.push((out.len(), element, look.clone()));
                        out.push(Part::Open { element, look, raw });
                    }
                }
                Part::Close { raw, element } => {
                    let Some(at) = open.iter().rposition(|(_, each, _)| *each == element) else {
                        out.push(Part::Run(text_run(&raw, Formats::default(), None, false)));
                        continue;
                    };
                    let (from, _, look) = open.remove(at);
                    // What was opened after it, and not closed, stays words.
                    open.truncate(at);
                    if let Some(source) = &look.math_source {
                        // Code spans turn a physical newline into a space. A changed visible body must never be
                        // replaced by the saved source.
                        let expected = source.replace("\r\n", "\n").replace(['\r', '\n'], " ");
                        let agrees = match &out[from + 1..] {
                            [] => source.is_empty(),
                            [
                                Part::Run(Inline::Text {
                                    text,
                                    marks: Some(marks),
                                    href: None,
                                    color: None,
                                    background: None,
                                }),
                            ] if marks.len() == 1 && marks[0] == Mark::Code => text == &expected,
                            _ => false,
                        };
                        if agrees {
                            out.truncate(from);
                            out.push(Part::Run(Inline::Math {
                                source: source.clone(),
                            }));
                            continue;
                        }
                    }
                    for inside in &mut out[from + 1..] {
                        if let Part::Run(Inline::Text {
                            marks,
                            color,
                            background,
                            ..
                        }) = inside
                        {
                            if look.underline {
                                let marks = marks.get_or_insert_with(Vec::new);
                                if !marks.contains(&Mark::Underline) {
                                    marks.push(Mark::Underline);
                                }
                            }
                            if color.is_none() {
                                *color = look.color;
                            }
                            if background.is_none() {
                                *background = look.background;
                            }
                        }
                    }
                    out[from] = Part::Spent;
                }
                other => out.push(other),
            }
        }
        for (at, _, _) in open {
            let raw = match &out[at] {
                Part::Open { raw, .. } => raw.clone(),
                _ => continue,
            };
            out[at] = Part::Run(text_run(&raw, Formats::default(), None, false));
        }
        out
    }

    /// The text collected so far, as runs, and the collection emptied.
    fn take_runs(&mut self) -> Vec<Inline> {
        let parts = std::mem::take(&mut self.parts);
        let parts = self.looks(parts);
        let mut runs = Vec::new();
        for part in parts {
            match part {
                Part::Run(run) => runs.push(run),
                Part::Image { dest, alt } => runs.push(self.image_words(dest, alt)),
                Part::Display(source) => runs.push(Inline::Math { source }),
                Part::Open { .. } | Part::Close { .. } | Part::Spent => {}
            }
        }
        self.finish_runs(runs)
    }

    /// Links to a thing of this system are chips: by the address of a typed reference, or by a reference the
    /// caller resolved; one link is one chip, whatever marks its words carry. A link meant as a reference that did
    /// not resolve keeps its words, without the link.
    fn chips(&self, runs: Vec<Inline>) -> Vec<Inline> {
        let mut out: Vec<Inline> = Vec::with_capacity(runs.len());
        for run in runs {
            let found = match &run {
                Inline::Text {
                    href: Some(href), ..
                } => match references::typed(href) {
                    Some(typed) => Reference::Typed(typed),
                    None => self.directory.reference(href, Form::Link),
                },
                _ => Reference::Untouched,
            };
            match found {
                Reference::Typed(typed) => {
                    let mut chip = chip_of(typed);
                    if let (Inline::Ref { label, .. }, Inline::Text { text, .. }) =
                        (&mut chip, &run)
                    {
                        *label = Some(text.clone());
                    }
                    if out.last() != Some(&chip) {
                        out.push(chip);
                    }
                }
                Reference::Plain => {
                    if let Inline::Text {
                        text,
                        marks,
                        color,
                        background,
                        ..
                    } = run
                    {
                        out.push(Inline::Text {
                            text,
                            marks,
                            href: None,
                            color,
                            background,
                        });
                    }
                }
                Reference::Untouched => out.push(run),
            }
        }
        out
    }

    /// Hidden words that the caller knows the reference of are that reference again: a reader who could not
    /// read it wrote around it, and it stays.
    fn unhidden(&self, runs: Vec<Inline>) -> Vec<Inline> {
        let mut out: Vec<Inline> = Vec::with_capacity(runs.len());
        for run in runs {
            let Inline::Text {
                text,
                marks,
                href: None,
                color,
                background,
            } = &run
            else {
                out.push(run);
                continue;
            };
            if marks
                .as_ref()
                .is_some_and(|marks| marks.contains(&Mark::Code))
            {
                out.push(run);
                continue;
            }
            let mut found: Vec<_> = references::hidden_in(text)
                .into_iter()
                .filter_map(|(range, number)| {
                    self.directory.hidden(number).map(|typed| (range, typed))
                })
                .collect();
            for (words, typed) in self.directory.hidden_names() {
                let plain: String = Parser::new_ext(&words, references::options())
                    .filter_map(|event| match event {
                        Event::Text(text) | Event::Code(text) => Some(text.into_string()),
                        _ => None,
                    })
                    .collect();
                found.extend(
                    text.match_indices(&plain)
                        .map(|(at, words)| (at..at + words.len(), typed.clone())),
                );
            }
            found.sort_by_key(|(range, _)| (range.start, std::cmp::Reverse(range.end)));
            found.dedup();
            if found.is_empty() {
                out.push(run);
                continue;
            }
            let piece = |words: &str| Inline::Text {
                text: words.to_owned(),
                marks: marks.clone(),
                href: None,
                color: *color,
                background: *background,
            };
            let mut at = 0;
            for (range, typed) in found {
                if range.start < at {
                    continue;
                }
                if at < range.start {
                    out.push(piece(&text[at..range.start]));
                }
                let mut chip = chip_of(typed);
                if let Inline::Ref { label, .. } = &mut chip
                    && let Some(shared) = text[range.clone()].strip_suffix(" (no access)")
                {
                    *label = Some(shared.to_owned());
                }
                out.push(chip);
                at = range.end;
            }
            if at < text.len() {
                out.push(piece(&text[at..]));
            }
        }
        out
    }

    fn finish_runs(&self, runs: Vec<Inline>) -> Vec<Inline> {
        let mut runs = merged(&self.unhidden(self.mentions(self.chips(runs))));
        // The parser has already removed Markdown's indentation. Decoded entities are content,
        // including the boundary spaces written by the renderer and whitespace-only toggle titles.
        runs.retain(|run| !matches!(run, Inline::Text { text, .. } if text.is_empty()));
        runs
    }

    /// Splits `@name` in text into mentions, where the name is a person's.
    fn mentions(&self, runs: Vec<Inline>) -> Vec<Inline> {
        let mut out = Vec::with_capacity(runs.len());
        for run in runs {
            let Inline::Text {
                text,
                marks,
                href,
                color,
                background,
            } = &run
            else {
                out.push(run);
                continue;
            };
            let plain = href.is_none() && !marks.as_ref().is_some_and(|m| m.contains(&Mark::Code));
            if !plain || !text.contains('@') {
                out.push(run);
                continue;
            }
            let rest_of = |text: &str| Inline::Text {
                text: text.to_owned(),
                marks: marks.clone(),
                href: None,
                color: *color,
                background: *background,
            };
            let mut start = 0;
            let mut cursor = 0;
            let bytes = text.as_bytes();
            while let Some(found) = text[cursor..].find('@') {
                let at = cursor + found;
                cursor = at + 1;
                let boundary = at == 0
                    || !text[..at]
                        .chars()
                        .next_back()
                        .is_some_and(char::is_alphanumeric);
                if !boundary {
                    continue;
                }
                let name_end = bytes[at + 1..]
                    .iter()
                    .position(|b| !(b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-')))
                    .map_or(text.len(), |len| at + 1 + len);
                let mut name = &text[at + 1..name_end];
                let mut user = None;
                while !name.is_empty() {
                    user = self.directory.user_id(name);
                    if user.is_some() {
                        break;
                    }
                    name = &name[..name.len() - 1];
                }
                if let Some(user) = user {
                    if start < at {
                        out.push(rest_of(&text[start..at]));
                    }
                    out.push(Inline::Mention {
                        user_id: user,
                        user: None,
                    });
                    start = at + 1 + name.len();
                    cursor = start;
                }
            }
            if start < text.len() {
                out.push(rest_of(&text[start..]));
            }
        }
        out
    }

    /// Adds a finished block to the block being filled.
    fn add(&mut self, made: NewBlock) {
        if made.block_type == BlockType::Columns
            && matches!(self.frames.last(), Some(Frame::Column { .. }))
        {
            // A column holds any block but other columns: those are written one after another.
            self.warn(Warning::new("reader.columns_in_column", &[]));
            for column in made.children.unwrap_or_default() {
                for inner in column.children.unwrap_or_default() {
                    self.add(inner);
                }
            }
            return;
        }
        match self.frames.last_mut() {
            Some(
                Frame::Doc(blocks)
                | Frame::Layout { blocks, .. }
                | Frame::Quote {
                    children: blocks, ..
                }
                | Frame::Item {
                    children: blocks, ..
                }
                | Frame::Toggle {
                    children: blocks, ..
                }
                | Frame::Column {
                    children: blocks, ..
                },
            ) => blocks.push(made),
            Some(Frame::List { items, .. }) => items.push(made),
            Some(Frame::Columns { columns, stray }) => {
                if made.block_type == BlockType::Column {
                    columns.push(made);
                } else {
                    stray.push(made);
                }
            }
            _ => {}
        }
    }

    /// Text that came before a nested block, in a list item whose text is not in a paragraph.
    fn before_block(&mut self) {
        if matches!(self.frames.last(), Some(Frame::Item { lead: None, .. }))
            && !self.parts.is_empty()
        {
            let runs = self.take_runs();
            if let Some(Frame::Item { lead, .. }) = self.frames.last_mut() {
                *lead = Some(runs);
            }
        }
    }

    fn paragraph(&mut self) {
        let parts = std::mem::take(&mut self.parts);
        // A paragraph that is one mark is a tag of the set.
        if let Some(number) = self.mark_in(&parts) {
            self.apply_tag(number);
            return;
        }
        // A paragraph that is one image, one formula or one link to a file is a block of its own.
        if let [Part::Image { dest, alt }] = parts.as_slice() {
            let file = self.file_for(dest).or_else(|| self.embed(dest));
            if let Some(file) = file {
                let caption = if alt.is_empty() {
                    Vec::new()
                } else {
                    vec![text_run(alt, Formats::default(), None, false)]
                };
                let made = with_text(
                    with_props(
                        block(BlockType::Image),
                        serde_json::json!({ "file_id": file.id }),
                    ),
                    caption,
                );
                self.add(made);
            } else {
                self.parts = parts;
                let runs = self.take_runs();
                self.finish_paragraph(runs);
            }
            return;
        }
        if let [Part::Display(source)] = parts.as_slice() {
            self.add(with_props(
                block(BlockType::Math),
                serde_json::json!({ "source": source }),
            ));
            return;
        }
        if let Some(made) = self.media_link(&parts) {
            self.add(made);
            return;
        }
        self.parts = parts;
        let runs = self.take_runs();
        self.finish_paragraph(runs);
    }

    /// A paragraph that is one link to a file: of the document, or one that came along with the text.
    fn media_link(&mut self, parts: &[Part]) -> Option<NewBlock> {
        let mut href: Option<&str> = None;
        let mut caption: Vec<Inline> = Vec::new();
        for part in parts {
            let Part::Run(
                run @ Inline::Text {
                    href: Some(this), ..
                },
            ) = part
            else {
                return None;
            };
            if href.is_some_and(|known| known != this) {
                return None;
            }
            href = Some(this);
            let mut plain = run.clone();
            if let Inline::Text { href, .. } = &mut plain {
                *href = None;
            }
            caption.push(plain);
        }
        let file = self.file_for(href?)?;
        let media = file.media_type.to_ascii_lowercase();
        let block_type = if crate::files::is_image(&media) {
            BlockType::Image
        } else if media.starts_with("video/") {
            BlockType::Video
        } else if media.starts_with("audio/") {
            BlockType::Audio
        } else {
            BlockType::File
        };
        let caption = self.finish_runs(caption);
        Some(with_text(
            with_props(block(block_type), serde_json::json!({ "file_id": file.id })),
            caption,
        ))
    }

    fn finish_paragraph(&mut self, runs: Vec<Inline>) {
        if runs.is_empty() {
            return;
        }
        // The first paragraph of an item, a quote or a toggle is its text.
        match self.frames.last_mut() {
            Some(
                Frame::Item { lead, children, .. }
                | Frame::Quote { lead, children, .. }
                | Frame::Toggle { lead, children },
            ) if lead.is_none() && children.is_empty() => {
                *lead = Some(runs);
            }
            _ => self.add(with_text(block(BlockType::Paragraph), runs)),
        }
    }

    // ---------------------------------------------------------- the tags of the set

    /// The tag that a paragraph stands for, if all it holds is its mark.
    fn mark_in(&self, parts: &[Part]) -> Option<usize> {
        let scanned = self.scanned.as_ref()?;
        let mut text = String::new();
        for part in parts {
            match part {
                Part::Run(Inline::Text {
                    text: each,
                    marks: None,
                    href: None,
                    ..
                }) => text.push_str(each),
                _ => return None,
            }
        }
        scanned.mark_at(&text)
    }

    /// The runs a line of Markdown says, for the title of a toggle and the caption of a film.
    fn runs_of_line(&mut self, line: &str) -> Vec<Inline> {
        if line.trim().is_empty() {
            return Vec::new();
        }
        let mut inner = Reader::new(self.directory);
        for event in Parser::new_ext(line, options()) {
            inner.handle(event);
        }
        inner.settle();
        for warning in inner.warnings {
            self.warn(warning);
        }
        for path in inner.missing {
            self.lost(&path);
        }
        self.embedded.extend(inner.embedded);
        let Some(Frame::Doc(blocks)) = inner.frames.into_iter().next() else {
            return vec![text_run(line, Formats::default(), None, false)];
        };
        match blocks.into_iter().next() {
            Some(NewBlock {
                block_type: BlockType::Paragraph,
                content: Some(BlockContent::Inline(runs)),
                ..
            }) => runs,
            _ => vec![text_run(line, Formats::default(), None, false)],
        }
    }

    fn apply_tag(&mut self, number: usize) {
        let Some((tag, written)) = self
            .scanned
            .as_ref()
            .and_then(|scanned| scanned.tags.get(number).cloned())
        else {
            return;
        };
        self.before_block();
        match tag {
            BlockTag::Layout(metadata) => self.frames.push(Frame::Layout {
                annotation: Annotation::read(&metadata).map(Box::new),
                written,
                own: None,
                blocks: Vec::new(),
            }),
            BlockTag::LayoutChildren => {
                if let Some(Frame::Layout {
                    annotation: Some(annotation),
                    own,
                    blocks,
                    ..
                }) = self.frames.last_mut()
                    && annotation.children
                    && own.is_none()
                {
                    *own = Some(std::mem::take(blocks));
                } else {
                    self.kept_as_text(&written);
                    self.finish_paragraph(vec![text_run(
                        &written,
                        Formats::default(),
                        None,
                        false,
                    )]);
                }
            }
            BlockTag::Toggle { summary } => {
                let lead = summary.map(|title| self.runs_of_line(&title));
                self.frames.push(Frame::Toggle {
                    lead,
                    children: Vec::new(),
                });
            }
            BlockTag::Columns => self.frames.push(Frame::Columns {
                columns: Vec::new(),
                stray: Vec::new(),
            }),
            BlockTag::Column { share } => self.frames.push(Frame::Column {
                share,
                children: Vec::new(),
            }),
            BlockTag::Close(kind) => self.close(kind, &written),
            BlockTag::PageBreak => self.add(block(BlockType::PageBreak)),
            BlockTag::Media {
                audio,
                src,
                caption,
            } => self.media_tag(audio, &src, &caption),
        }
    }

    /// A film or a sound written as a tag: a file that came with the text, or an address.
    fn media_tag(&mut self, audio: bool, src: &str, caption: &str) {
        let block_type = if audio {
            BlockType::Audio
        } else {
            BlockType::Video
        };
        let runs = self.runs_of_line(caption);
        let props = if let Some(file) = self.file_for(src) {
            serde_json::json!({ "file_id": file.id })
        } else if blocks::allowed_media_url(src) {
            serde_json::json!({ "url": src })
        } else {
            // What cannot be shown stays as the words that said it.
            if let Some(path) = relative_path(src) {
                self.warn(Warning::new(
                    if audio {
                        "reader.sound_missing"
                    } else {
                        "reader.film_missing"
                    },
                    &[("path", &path)],
                ));
                self.lost(&path);
            } else {
                self.warn(Warning::new("reader.media_address", &[("address", src)]));
            }
            let words = if runs.is_empty() {
                vec![text_run(src, Formats::default(), None, false)]
            } else {
                runs
            };
            self.finish_paragraph(words);
            return;
        };
        self.add(with_text(with_props(block(block_type), props), runs));
    }

    /// Ends the container that a closing tag names, and what was opened inside it and never closed. A closing tag
    /// that closes nothing in this place is left out.
    fn close(&mut self, kind: Container, written: &str) {
        let mut depth = 0;
        let mut found = None;
        for frame in self.frames.iter().rev() {
            match container_of(frame) {
                Some(each) if each == kind => {
                    found = Some(depth);
                    break;
                }
                Some(_) => depth += 1,
                None => break,
            }
        }
        match found {
            Some(depth) => {
                for _ in 0..=depth {
                    self.finish_container();
                }
            }
            None => {
                let name = written
                    .trim()
                    .trim_start_matches('<')
                    .trim_end_matches('>')
                    .trim_end_matches('/')
                    .to_owned();
                self.warn(Warning::new("reader.tag_kept_as_text", &[("tag", &name)]));
            }
        }
    }

    /// Ends every container that stands open at the top of the stack: what its parent ends with them.
    fn settle(&mut self) {
        while self
            .frames
            .last()
            .is_some_and(|frame| container_of(frame).is_some())
        {
            self.finish_container();
        }
    }

    /// Makes the block of the container that is open at the top, and adds it to what holds it.
    fn finish_container(&mut self) {
        match self.frames.pop() {
            Some(Frame::Layout {
                annotation,
                written,
                own,
                mut blocks,
            }) => {
                let mut content = own.unwrap_or_else(|| std::mem::take(&mut blocks));
                if let Some(annotation) = &annotation {
                    if let Some(caption) = &annotation.caption
                        && content.len() == 1
                        && matches!(
                            annotation.kind,
                            BlockType::Image
                                | BlockType::Video
                                | BlockType::Audio
                                | BlockType::File
                        )
                    {
                        content[0].content = Some(BlockContent::Inline(self.runs_of_line(caption)));
                    }
                    if let Some(mut restored) = annotation.restore(content.clone()) {
                        if annotation.children {
                            restored.children = (!blocks.is_empty()).then_some(blocks);
                        }
                        self.add(restored);
                        return;
                    }
                }
                self.warn(Warning::new(
                    "reader.tag_kept_as_text",
                    &[("tag", "orch:block")],
                ));
                self.finish_paragraph(vec![text_run(&written, Formats::default(), None, false)]);
                for child in content.into_iter().chain(blocks) {
                    self.add(child);
                }
            }
            Some(Frame::Toggle { lead, children }) => {
                let made = with_children(
                    with_text(block(BlockType::Toggle), lead.unwrap_or_default()),
                    children,
                );
                self.add(made);
            }
            Some(Frame::Column { share, children }) => {
                if matches!(self.frames.last(), Some(Frame::Columns { .. })) {
                    let props = share.map_or_else(
                        || serde_json::json!({}),
                        |share| serde_json::json!({ "share": share }),
                    );
                    self.add(with_children(
                        with_props(block(BlockType::Column), props),
                        children,
                    ));
                } else {
                    self.warn(Warning::new("reader.column_alone", &[]));
                    for child in children {
                        self.add(child);
                    }
                }
            }
            Some(Frame::Columns { columns, stray }) => {
                if blocks::columns_fit(columns.len()) {
                    self.add(with_children(block(BlockType::Columns), columns));
                } else {
                    self.warn(Warning::new("reader.columns_count", &[]));
                    for column in columns {
                        for child in column.children.unwrap_or_default() {
                            self.add(child);
                        }
                    }
                }
                for each in stray {
                    self.add(each);
                }
            }
            Some(other) => self.frames.push(other),
            None => {}
        }
    }

    fn handle(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(end) => self.end(end),
            Event::Text(text) => self.text(&text),
            Event::Code(code) => {
                let link = self.links.last().cloned();
                // A path in backquotes that the caller resolved is a chip; inside a link the link decides.
                if link.is_none()
                    && let Reference::Typed(typed) = self.directory.reference(&code, Form::Code)
                {
                    let mut chip = chip_of(typed);
                    if let Inline::Ref { label, .. } = &mut chip {
                        *label = Some(code.to_string());
                    }
                    self.parts.push(Part::Run(chip));
                    return;
                }
                self.parts.push(Part::Run(text_run(
                    &code,
                    self.formats,
                    link.as_deref(),
                    true,
                )));
            }
            Event::InlineMath(source) => self.parts.push(Part::Run(Inline::Math {
                source: source.to_string(),
            })),
            Event::DisplayMath(source) => self.parts.push(Part::Display(source.trim().to_owned())),
            Event::Html(html) => match self.html.as_mut() {
                Some(collected) => collected.push_str(&html),
                None => {
                    self.kept_as_text(&html);
                    self.literal(&html);
                }
            },
            Event::InlineHtml(html) => {
                let tag = html.trim().to_ascii_lowercase();
                if matches!(tag.as_str(), "<br>" | "<br/>" | "<br />") {
                    self.parts.push(Part::Run(Inline::Break));
                } else if tag.starts_with("<!--") {
                    // A comment is left out.
                } else if let Some(part) = inline_tag(html.trim()) {
                    self.parts.push(part);
                } else {
                    self.kept_as_text(&html);
                    self.literal(&html);
                }
            }
            Event::SoftBreak if self.image.is_some() => self.text("\n"),
            Event::SoftBreak => self.literal(SPACE),
            Event::HardBreak if self.image.is_some() => self.text("\n"),
            Event::HardBreak => self.parts.push(Part::Run(Inline::Break)),
            Event::Rule => {
                self.before_block();
                self.add(block(BlockType::Divider));
            }
            Event::TaskListMarker(checked) => {
                if let Some(Frame::Item { checked: mark, .. }) = self.frames.last_mut() {
                    *mark = Some(checked);
                }
            }
            Event::FootnoteReference(label) => self.literal(&format!("[^{label}]")),
        }
    }

    /// Says which tags a piece of raw HTML holds, that stay as the text they were written with.
    fn kept_as_text(&mut self, html: &str) {
        for (at, _) in html.match_indices('<') {
            let Some(tag) = tags::parse_tag(&html[at..]) else {
                continue;
            };
            if tag.name == "br" {
                continue;
            }
            let name = if tag.closing {
                format!("/{}", tag.name)
            } else {
                tag.name
            };
            self.warn(Warning::new("reader.tag_kept_as_text", &[("tag", &name)]));
        }
    }

    fn literal(&mut self, text: &str) {
        let link = self.links.last().cloned();
        self.parts.push(Part::Run(text_run(
            text,
            self.formats,
            link.as_deref(),
            false,
        )));
    }

    fn text(&mut self, text: &str) {
        if let Some((_, body)) = self.code.as_mut() {
            body.push_str(text);
        } else if let Some(html) = self.html.as_mut() {
            html.push_str(text);
        } else if let Some((_, alt)) = self.image.as_mut() {
            alt.push_str(text);
        } else {
            self.literal(text);
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {
                self.before_block_paragraph();
                self.frames.push(Frame::Paragraph);
            }
            Tag::Heading { level, .. } => {
                self.before_block();
                self.frames.push(Frame::Heading(heading_level(level)));
            }
            Tag::BlockQuote(kind) => {
                if self.skip_container() {
                    return;
                }
                self.before_block();
                self.frames.push(Frame::Quote {
                    kind,
                    lead: None,
                    children: Vec::new(),
                });
            }
            Tag::CodeBlock(kind) => {
                self.before_block();
                let language = match kind {
                    CodeBlockKind::Fenced(info) => {
                        info.split_whitespace().next().unwrap_or("").to_owned()
                    }
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some((language, String::new()));
            }
            Tag::HtmlBlock => {
                self.before_block();
                self.html = Some(String::new());
            }
            Tag::List(start) => {
                if self.skip_container() {
                    return;
                }
                self.before_block();
                self.frames.push(Frame::List {
                    start,
                    items: Vec::new(),
                });
            }
            Tag::Item => {
                if self.skip_container() {
                    return;
                }
                self.frames.push(Frame::Item {
                    checked: None,
                    lead: None,
                    children: Vec::new(),
                });
            }
            Tag::Table(_) => {
                self.before_block();
                self.frames.push(Frame::Table {
                    rows: Vec::new(),
                    row: Vec::new(),
                });
            }
            Tag::TableHead | Tag::TableRow => {}
            Tag::TableCell => self.frames.push(Frame::Cell),
            Tag::Emphasis => self.formats.italic += 1,
            Tag::Strong => self.formats.bold += 1,
            Tag::Strikethrough => self.formats.strike += 1,
            Tag::Link { dest_url, .. } => self.links.push(dest_url.to_string()),
            Tag::Image { dest_url, .. } => self.image = Some((dest_url.to_string(), String::new())),
            _ => {}
        }
    }

    /// A paragraph in an item whose text came before it directly: it is the
    /// item's text no more, and the text before it is.
    fn before_block_paragraph(&mut self) {
        self.before_block();
    }

    fn end(&mut self, end: TagEnd) {
        match end {
            TagEnd::Paragraph => {
                self.frames.pop();
                self.paragraph();
            }
            TagEnd::Heading(_) => {
                let level = match self.frames.pop() {
                    Some(Frame::Heading(level)) => level,
                    _ => 1,
                };
                let runs = self.take_runs();
                let made = with_text(
                    with_props(
                        block(BlockType::Heading),
                        serde_json::json!({ "level": level }),
                    ),
                    runs,
                );
                self.add(made);
            }
            TagEnd::BlockQuote(_) => {
                if self.skipped() {
                    return;
                }
                self.settle();
                let Some(Frame::Quote {
                    kind,
                    lead,
                    children,
                }) = self.frames.pop()
                else {
                    return;
                };
                let made = quote_block(kind, lead.unwrap_or_default(), children);
                self.add(made);
            }
            TagEnd::CodeBlock => {
                if let Some((language, mut body)) = self.code.take() {
                    if body.ends_with('\n') {
                        body.pop();
                    }
                    if let Some(scanned) = &self.scanned {
                        // A tag that was indented so far that Markdown took it for code is still a tag.
                        let marks: Vec<Option<usize>> = body
                            .lines()
                            .filter(|line| !line.trim().is_empty())
                            .map(|line| scanned.mark_at(line))
                            .collect();
                        if !marks.is_empty() && marks.iter().all(Option::is_some) {
                            for number in marks.into_iter().flatten() {
                                self.apply_tag(number);
                            }
                            return;
                        }
                        body = scanned.restored(&body);
                    }
                    let made = if language.eq_ignore_ascii_case("mermaid") {
                        with_props(
                            block(BlockType::Diagram),
                            serde_json::json!({ "source": body }),
                        )
                    } else {
                        let runs = if body.is_empty() {
                            Vec::new()
                        } else {
                            vec![text_run(&body, Formats::default(), None, false)]
                        };
                        with_text(
                            with_props(
                                block(BlockType::Code),
                                serde_json::json!({ "language": language }),
                            ),
                            runs,
                        )
                    };
                    self.add(made);
                }
            }
            TagEnd::HtmlBlock => {
                if let Some(html) = self.html.take() {
                    let trimmed = html.trim();
                    if trimmed.eq_ignore_ascii_case(LEGACY_PAGE_BREAK_LINE)
                        || trimmed.eq_ignore_ascii_case(PAGE_BREAK_LINE)
                    {
                        self.add(block(BlockType::PageBreak));
                    } else if !trimmed.starts_with("<!--") {
                        self.kept_as_text(trimmed);
                        let runs = vec![text_run(trimmed, Formats::default(), None, false)];
                        self.finish_paragraph(runs);
                    }
                }
            }
            TagEnd::List(_) => {
                if self.skipped() {
                    return;
                }
                let Some(Frame::List { start, mut items }) = self.frames.pop() else {
                    return;
                };
                if let Some(start) = start
                    && start != 1
                    && let Some(first) = items.first_mut()
                    && first.block_type == BlockType::Numbered
                {
                    *first = with_props(
                        std::mem::replace(first, block(BlockType::Numbered)),
                        serde_json::json!({ "start": start }),
                    );
                }
                for item in items {
                    self.add(item);
                }
            }
            TagEnd::Item => {
                if self.skipped() {
                    // The words of an item read as no item are a paragraph of their own.
                    if !self.parts.is_empty() {
                        let runs = self.take_runs();
                        self.finish_paragraph(runs);
                    }
                    return;
                }
                self.settle();
                let Some(Frame::Item {
                    checked,
                    mut lead,
                    children,
                }) = self.frames.pop()
                else {
                    return;
                };
                if lead.is_none() && !self.parts.is_empty() {
                    lead = Some(self.take_runs());
                }
                let ordered =
                    matches!(self.frames.last(), Some(Frame::List { start: Some(_), .. }));
                let made = match (checked, ordered) {
                    (Some(ticked), _) => with_props(
                        block(BlockType::Todo),
                        serde_json::json!({ "checked": ticked }),
                    ),
                    (None, true) => block(BlockType::Numbered),
                    (None, false) => block(BlockType::Bullet),
                };
                let made = with_text(made, lead.unwrap_or_default());
                self.add(with_children(made, children));
            }
            TagEnd::TableCell => {
                self.frames.pop();
                let runs = self.take_runs();
                if let Some(Frame::Table { row, .. }) = self.frames.last_mut() {
                    row.push(TableCell {
                        content: runs,
                        ..TableCell::default()
                    });
                }
            }
            TagEnd::TableHead | TagEnd::TableRow => {
                if let Some(Frame::Table { rows, row }) = self.frames.last_mut() {
                    rows.push(std::mem::take(row));
                }
            }
            TagEnd::Table => {
                let Some(Frame::Table { rows, .. }) = self.frames.pop() else {
                    return;
                };
                let rows: Vec<TableRow> =
                    rows.into_iter().map(|cells| TableRow { cells }).collect();
                let mut made = with_props(
                    block(BlockType::Table),
                    serde_json::json!({ "header_row": true }),
                );
                made.content = Some(BlockContent::Rows(rows));
                self.add(made);
            }
            TagEnd::Emphasis => self.formats.italic = self.formats.italic.saturating_sub(1),
            TagEnd::Strong => self.formats.bold = self.formats.bold.saturating_sub(1),
            TagEnd::Strikethrough => self.formats.strike = self.formats.strike.saturating_sub(1),
            TagEnd::Link => {
                self.links.pop();
            }
            TagEnd::Image => {
                if let Some((dest, alt)) = self.image.take() {
                    self.parts.push(Part::Image { dest, alt });
                }
            }
            _ => {}
        }
    }
}

/// The tag of a line of text that looks at the words between it and its partner: `<u>` and `<span color=…>`.
/// Anything else, and a `<span>` that says nothing the reader knows, is no such tag.
fn inline_tag(raw: &str) -> Option<Part> {
    let tag = tags::parse_tag(raw).filter(|tag| tag.rest.is_empty() && !tag.self_closing)?;
    let element = match tag.name.as_str() {
        "u" => Element::Underline,
        "span" => Element::Span,
        _ => return None,
    };
    if tag.closing {
        return Some(Part::Close {
            raw: raw.to_owned(),
            element,
        });
    }
    let mut look = TagLook::default();
    if element == Element::Underline {
        look.underline = true;
    } else {
        for (name, value) in &tag.attrs {
            match name.as_str() {
                "color" => match tint_from(value) {
                    Some((tint, true)) => look.background = Some(tint),
                    Some((tint, false)) => look.color = Some(tint),
                    None => {}
                },
                "background" | "bg" => {
                    if let Some((tint, _)) = tint_from(value) {
                        look.background = Some(tint);
                    }
                }
                "underline" => look.underline = !value.eq_ignore_ascii_case("false"),
                "orch-math" => look.math_source = math_source(value),
                _ => {}
            }
        }
        if look == TagLook::default() {
            return None;
        }
    }
    Some(Part::Open {
        raw: raw.to_owned(),
        element,
        look,
    })
}

/// Formula attributes share the layout metadata bound; their decoded source must fit the existing block bound.
fn math_source(encoded: &str) -> Option<String> {
    use base64::Engine as _;
    if encoded.len() > layout::BYTES_MAX {
        return None;
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()?;
    if i64::try_from(bytes.len()).ok()? > orch_api_contract::documents::DOCUMENT_BLOCK_MAX_BYTES {
        return None;
    }
    String::from_utf8(bytes).ok()
}

fn container_of(frame: &Frame) -> Option<Container> {
    match frame {
        Frame::Toggle { .. } => Some(Container::Toggle),
        Frame::Layout { .. } => Some(Container::Layout),
        Frame::Columns { .. } => Some(Container::Columns),
        Frame::Column { .. } => Some(Container::Column),
        _ => None,
    }
}

/// What the reader reads Markdown with: GitHub's, with tables, to-dos, strike-through, formulas and alerts.
fn options() -> Options {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_MATH);
    options.insert(Options::ENABLE_GFM);
    options
}

/// A path that a link or a picture wrote relative to the text, as the reader asks the caller about it: without
/// its fragment or its query, and decoded. `None` for an address that goes out of the text (a scheme, a path of
/// this system, a fragment of the same page).
#[must_use]
pub fn relative_path(dest: &str) -> Option<String> {
    let dest = dest.trim();
    let scheme = dest.split_once(':').is_some_and(|(scheme, _)| {
        !scheme.is_empty()
            && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    });
    if dest.is_empty() || scheme || dest.starts_with(['/', '#', '\\']) {
        return None;
    }
    let end = dest.find(['#', '?']).unwrap_or(dest.len());
    let path = percent_decoded(&dest[..end]);
    (!path.is_empty()).then_some(path)
}

/// The type and the bytes of a picture that an address carries inside it (`data:image/png;base64,…`): of the
/// kinds that every browser shows as a picture and that run nothing.
pub(crate) fn data_image(dest: &str) -> Option<(String, Vec<u8>)> {
    use base64::Engine as _;
    let rest = dest.trim().strip_prefix("data:")?;
    let (head, data) = rest.split_once(',')?;
    let mut fields = head.split(';');
    let media_type = fields.next()?.trim().to_ascii_lowercase();
    if !fields.any(|field| field.trim().eq_ignore_ascii_case("base64")) {
        return None;
    }
    if !matches!(
        media_type.as_str(),
        "image/png" | "image/jpeg" | "image/jpg" | "image/gif" | "image/webp" | "image/bmp"
    ) {
        return None;
    }
    let squeezed: String = data.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(squeezed.as_bytes())
        .ok()?;
    let media_type = if media_type == "image/jpg" {
        "image/jpeg".to_owned()
    } else {
        media_type
    };
    (!bytes.is_empty()).then_some((media_type, bytes))
}

/// The ending of a name for a picture of this type.
pub(crate) fn image_extension(media_type: &str) -> &'static str {
    match media_type {
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/bmp" => "bmp",
        _ => "png",
    }
}

fn heading_level(level: HeadingLevel) -> u32 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// A quote, or a callout where it opens with its tone in bold (as the
/// renderer writes one) or is a GitHub-style alert.
fn quote_block(
    kind: Option<BlockQuoteKind>,
    mut lead: Vec<Inline>,
    children: Vec<NewBlock>,
) -> NewBlock {
    let from_alert = kind.map(|kind| match kind {
        BlockQuoteKind::Note | BlockQuoteKind::Important => "info",
        BlockQuoteKind::Tip => "success",
        BlockQuoteKind::Warning => "warning",
        BlockQuoteKind::Caution => "danger",
    });
    let from_bold = match lead.first() {
        Some(Inline::Text {
            text,
            marks,
            href: None,
            ..
        }) if marks.as_deref() == Some(&[Mark::Bold]) => match text.as_str() {
            "Info:" => Some("info"),
            "Success:" => Some("success"),
            "Warning:" => Some("warning"),
            "Danger:" => Some("danger"),
            _ => None,
        },
        _ => None,
    };
    if let Some(tone) = from_bold.filter(|_| from_alert.is_none()) {
        lead.remove(0);
        if let Some(Inline::Text { text, .. }) = lead.first_mut() {
            *text = text.trim_start().to_owned();
        }
        return with_children(
            with_text(
                with_props(
                    block(BlockType::Callout),
                    serde_json::json!({ "tone": tone }),
                ),
                lead,
            ),
            children,
        );
    }
    if let Some(tone) = from_alert {
        return with_children(
            with_text(
                with_props(
                    block(BlockType::Callout),
                    serde_json::json!({ "tone": tone }),
                ),
                lead,
            ),
            children,
        );
    }
    with_children(with_text(block(BlockType::Quote), lead), children)
}

/// Makes the blocks that Markdown says: new identifiers for each, in the order
/// of the text. Nested deeper than a document may be, a block is put beside
/// the one it was under. Links that lead nowhere are the words they were written with; see [`read_checked`] for
/// what the reader says about them.
#[must_use]
pub fn read(markdown: &str, directory: &dyn Directory) -> Vec<NewBlock> {
    read_checked(markdown, directory).blocks
}

/// Like [`read`], and says what it kept as words or left out, which paths led nowhere, and which pictures the
/// text carried inside it.
#[must_use]
pub fn read_checked(markdown: &str, directory: &dyn Directory) -> Read {
    let scanned = tags::scan(markdown);
    let source: std::borrow::Cow<'_, str> = match &scanned {
        Some(scanned) => std::borrow::Cow::Borrowed(scanned.text.as_str()),
        None => std::borrow::Cow::Borrowed(markdown),
    };
    let mut reader = Reader::new(directory);
    reader.scanned = scanned.clone();
    for name in scanned.iter().flat_map(|scanned| &scanned.unmatched) {
        reader.warn(Warning::new("reader.tag_kept_as_text", &[("tag", name)]));
    }
    for event in Parser::new_ext(&source, options()) {
        reader.handle(event);
    }
    reader.settle();
    let mut blocks = match std::mem::take(&mut reader.frames).into_iter().next() {
        Some(Frame::Doc(blocks)) => blocks,
        _ => Vec::new(),
    };
    reader.settle_links(&mut blocks);
    flatten_deep(&mut blocks, 0);
    Read {
        blocks,
        warnings: reader.warnings,
        missing: reader.missing,
        embedded: reader.embedded,
    }
}

/// What a relative path of a text is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    /// A picture: `![…](path)`.
    Image,
    /// A link: `[…](path)`.
    Link,
    /// A film or a sound: `<video src="path">`.
    Media,
}

/// A path that a text wrote relative to itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// The path, decoded and without its fragment.
    pub path: String,
    /// What it is for, by the first place that wrote it.
    pub kind: TargetKind,
}

/// The relative paths a text names, each once, in the order they stand: what its reader will ask
/// [`Directory::local`] about, so that a caller can have them ready.
#[must_use]
pub fn local_targets(markdown: &str) -> Vec<Target> {
    let scanned = tags::scan(markdown);
    let mut out: Vec<Target> = Vec::new();
    let mut note = |dest: &str, kind: TargetKind| {
        if let Some(path) = relative_path(dest)
            && !out.iter().any(|each| each.path == path)
        {
            out.push(Target { path, kind });
        }
    };
    let source = scanned
        .as_ref()
        .map_or(markdown, |scanned| scanned.text.as_str());
    for event in Parser::new_ext(source, options()) {
        match event {
            Event::Start(Tag::Link { dest_url, .. }) => note(&dest_url, TargetKind::Link),
            Event::Start(Tag::Image { dest_url, .. }) => note(&dest_url, TargetKind::Image),
            _ => {}
        }
    }
    if let Some(scanned) = &scanned {
        for (tag, _) in &scanned.tags {
            if let BlockTag::Media { src, .. } = tag {
                note(src, TargetKind::Media);
            }
        }
    }
    out
}

/// Settles the links of blocks that another reader made (HTML, Word): what a link to another text read with
/// them is, and what a link that leads nowhere says. Gives the sentences and the paths that led nowhere.
pub(crate) fn settle_blocks(
    blocks: &mut [NewBlock],
    directory: &dyn Directory,
) -> (Vec<Warning>, Vec<String>) {
    let mut reader = Reader::new(directory);
    reader.settle_links(blocks);
    (reader.warnings, reader.missing)
}

impl Reader<'_> {
    /// Settles the links of blocks that were read: one that leads out of the text is kept as it is; one that
    /// leads to another text read with this one is a reference to the document it becomes; any other is the
    /// words it was written with, and is said.
    fn settle_links(&mut self, blocks: &mut [NewBlock]) {
        for each in blocks {
            match each.content.take() {
                Some(BlockContent::Inline(runs)) => {
                    each.content = Some(BlockContent::Inline(self.settle_runs(runs)));
                }
                Some(BlockContent::Rows(mut rows)) => {
                    for row in &mut rows {
                        for cell in &mut row.cells {
                            let content = std::mem::take(&mut cell.content);
                            cell.content = self.settle_runs(content);
                        }
                    }
                    each.content = Some(BlockContent::Rows(rows));
                }
                None => {}
            }
            if let Some(children) = each.children.as_mut() {
                self.settle_links(children);
            }
        }
    }

    fn settle_runs(&mut self, runs: Vec<Inline>) -> Vec<Inline> {
        let mut out: Vec<Inline> = Vec::with_capacity(runs.len());
        let mut index = 0;
        while index < runs.len() {
            let address = match &runs[index] {
                Inline::Text {
                    href: Some(href), ..
                } if !blocks::allowed_href(href) => Some(href.clone()),
                _ => None,
            };
            let Some(address) = address else {
                out.push(runs[index].clone());
                index += 1;
                continue;
            };
            let mut end = index + 1;
            while end < runs.len()
                && matches!(&runs[end], Inline::Text { href: Some(other), .. } if *other == address)
            {
                end += 1;
            }
            let words: Vec<Inline> = runs[index..end]
                .iter()
                .map(|run| match run {
                    Inline::Text {
                        text,
                        marks,
                        color,
                        background,
                        ..
                    } => Inline::Text {
                        text: text.clone(),
                        marks: marks.clone(),
                        href: None,
                        color: *color,
                        background: *background,
                    },
                    other => other.clone(),
                })
                .collect();
            match relative_path(&address) {
                Some(path) => match self.directory.local(&path) {
                    Some(Local::Document(typed)) => {
                        let mut chip = chip_of(typed);
                        if let Inline::Ref { label, .. } = &mut chip {
                            *label = Some(blocks::inline_text(&words));
                        }
                        out.push(chip);
                    }
                    found => {
                        self.warn(Warning::new(
                            if found.is_some() {
                                "reader.link_is_file"
                            } else {
                                "reader.link_unresolved"
                            },
                            &[("target", &path)],
                        ));
                        if found.is_none() {
                            self.lost(&path);
                        }
                        out.extend(words);
                    }
                },
                None => {
                    // A place on the same page cannot be followed from a block; any other kind of address is
                    // not one a link may have.
                    if !address.trim_start().starts_with('#') {
                        self.warn(Warning::new("reader.link_not_followed", &[]));
                    }
                    out.extend(words);
                }
            }
            index = end;
        }
        merged(&out)
    }
}

/// The most names [`mentioned_names`] gives, whatever the text: a text that names more
/// people than this is read with the first of them known.
pub const MENTIONED_NAMES_MAX: usize = 500;

/// The names that `@` in a text might mean, for whoever has to look them up
/// before [`read`]: after each `@` that does not follow a letter or a digit,
/// the run of letters, digits, `_`, `.` and `-`, and every shorter start of
/// it, since the reader takes the longest start that is somebody. Lower case,
/// each once, in the order they come.
#[must_use]
pub fn mentioned_names(markdown: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut previous: Option<char> = None;
    for (at, c) in markdown.char_indices() {
        if c == '@' && !previous.is_some_and(char::is_alphanumeric) {
            let rest = &markdown[at + 1..];
            let run: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
                .collect();
            for end in (1..=run.len()).rev() {
                let name = run[..end].to_ascii_lowercase();
                if seen.insert(name.clone()) {
                    found.push(name);
                }
            }
            if found.len() >= MENTIONED_NAMES_MAX {
                found.truncate(MENTIONED_NAMES_MAX);
                break;
            }
        }
        previous = Some(c);
    }
    found
}

/// Blocks nested deeper than the limit are put after the block they were under.
fn flatten_deep(siblings: &mut Vec<NewBlock>, depth: usize) {
    let mut at = 0;
    while at < siblings.len() {
        if depth >= blocks::nesting_limit() {
            let lifted = siblings[at].children.take().unwrap_or_default();
            for (offset, child) in lifted.into_iter().enumerate() {
                siblings.insert(at + 1 + offset, child);
            }
        } else if let Some(children) = siblings[at].children.as_mut() {
            flatten_deep(children, depth + 1);
        }
        at += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_api_contract::documents::BlockProps;
    use orch_types::new_id;
    use serde_json::json;
    use std::collections::HashMap;

    #[derive(Default)]
    struct World {
        users: HashMap<String, Id>,
        files: HashMap<String, (Id, String)>,
    }

    impl World {
        fn user(&mut self, name: &str) -> Id {
            *self.users.entry(name.to_owned()).or_insert_with(new_id)
        }

        fn add_file(&mut self, name: &str, media_type: &str) -> Id {
            self.files
                .entry(name.to_owned())
                .or_insert_with(|| (new_id(), media_type.to_owned()))
                .0
        }
    }

    impl Lookup for World {
        fn username(&self, user: Id) -> Option<String> {
            self.users
                .iter()
                .find_map(|(name, id)| (*id == user).then(|| name.clone()))
        }

        fn name_of(&self, _kind: InlineRefKind, _id: Id) -> Option<String> {
            Some("Plans".to_owned())
        }

        fn file(&self, id: Id) -> Option<crate::files::FileInfo> {
            self.files.iter().find_map(|(name, (file, media_type))| {
                (*file == id).then(|| crate::files::FileInfo {
                    name: name.clone(),
                    media_type: media_type.clone(),
                    size: 1,
                    is_text: false,
                    bytes: None,
                })
            })
        }
    }

    impl Directory for World {
        fn user_id(&self, username: &str) -> Option<Id> {
            self.users.get(username).copied()
        }

        fn file(&self, name: &str) -> Option<FileRef> {
            self.files.get(name).map(|(id, media_type)| FileRef {
                id: *id,
                media_type: media_type.clone(),
            })
        }
    }

    fn plain(text: &str) -> Inline {
        Inline::Text {
            text: text.to_owned(),
            marks: None,
            href: None,
            color: None,
            background: None,
        }
    }

    fn marked(text: &str, marks: &[Mark]) -> Inline {
        Inline::Text {
            text: text.to_owned(),
            marks: Some(marks.to_vec()),
            href: None,
            color: None,
            background: None,
        }
    }

    fn linked(text: &str, href: &str) -> Inline {
        Inline::Text {
            text: text.to_owned(),
            marks: None,
            href: Some(href.to_owned()),
            color: None,
            background: None,
        }
    }

    /// A tree of new blocks as a flat list of live document blocks, as the server holds them.
    fn stored(blocks: &[NewBlock]) -> Vec<DocumentBlock> {
        fn walk(into: &mut Vec<DocumentBlock>, parent: Option<Id>, siblings: &[NewBlock]) {
            for (index, new) in siblings.iter().enumerate() {
                let checked =
                    blocks::check(new.block_type, new.props.as_ref(), new.content.as_ref())
                        .unwrap();
                into.push(DocumentBlock {
                    id: new.id,
                    document_id: Id::nil(),
                    parent_id: parent,
                    position: (index + 1).to_string(),
                    block_type: new.block_type,
                    props: blocks::props_from_json(&checked.props),
                    content: checked.content,
                    removed: false,
                    seq_created: 1,
                    seq_updated: 1,
                    updated_by: None,
                    updated_at: "2026-10-08T17:04:05.123Z".parse().unwrap(),
                });
                walk(into, Some(new.id), new.children.as_deref().unwrap_or(&[]));
            }
        }
        let mut out = Vec::new();
        walk(&mut out, None, blocks);
        out
    }

    /// What a block says without its identifier, to compare two trees.
    fn shape(blocks: &[NewBlock]) -> Json {
        Json::Array(
            blocks
                .iter()
                .map(|block| {
                    let checked = blocks::check(
                        block.block_type,
                        block.props.as_ref(),
                        block.content.as_ref(),
                    )
                    .unwrap();
                    let content = match &checked.content {
                        BlockContent::Inline(runs) => json!(merged_for_compare(runs)),
                        rows => serde_json::to_value(rows).unwrap(),
                    };
                    json!({
                        "type": block.block_type,
                        "props": checked.props,
                        "content": content,
                        "children": shape(block.children.as_deref().unwrap_or(&[])),
                    })
                })
                .collect(),
        )
    }

    fn merged_for_compare(runs: &[Inline]) -> Vec<Inline> {
        merged(runs)
            .into_iter()
            .map(|run| match run {
                Inline::Text {
                    text,
                    marks,
                    href,
                    color,
                    background,
                } => {
                    let mut marks = marks.unwrap_or_default();
                    marks.sort_by_key(|mark| *mark as u8);
                    Inline::Text {
                        text,
                        marks: (!marks.is_empty()).then_some(marks),
                        href,
                        color,
                        background,
                    }
                }
                other => other,
            })
            .collect()
    }

    fn block_of(block_type: BlockType, props: Json, runs: Vec<Inline>) -> NewBlock {
        with_text(with_props(block(block_type), props), runs)
    }

    fn children_of(mut parent: NewBlock, children: Vec<NewBlock>) -> NewBlock {
        parent.children = Some(children);
        parent
    }

    fn there_and_back(world: &World, blocks: &[NewBlock]) -> (Rendered, Vec<NewBlock>) {
        let rendered = render(&stored(blocks), world, &RenderOptions::default());
        let back = read(&rendered.text, world);
        (rendered, back)
    }

    #[test]
    fn the_names_an_at_sign_might_mean_are_found_before_the_text_is_read() {
        let names =
            mentioned_names("Ask @Ada.Lovelace, and mail ada@example.org or @bob-2: thanks");
        assert!(names.contains(&"ada.lovelace".to_owned()));
        assert!(
            names.contains(&"ada".to_owned()),
            "a start of the name may be the person"
        );
        assert!(names.contains(&"bob-2".to_owned()));
        assert!(
            !names.iter().any(|name| name.contains("example")),
            "an @ that follows a letter is part of an address, not a mention"
        );
    }

    #[test]
    fn text_with_every_mark_a_link_a_mention_a_chip_and_a_formula_comes_back_as_it_went() {
        let mut world = World::default();
        let ada = world.user("ada");
        let node = new_id();
        let paragraph = block_of(
            BlockType::Paragraph,
            json!({}),
            vec![
                plain("Some "),
                marked("bold", &[Mark::Bold]),
                plain(", "),
                marked("italic", &[Mark::Italic]),
                plain(", "),
                marked("struck", &[Mark::Strike]),
                plain(", "),
                marked("code()", &[Mark::Code]),
                plain(", "),
                marked("both", &[Mark::Bold, Mark::Italic]),
                plain(" and a "),
                linked("link", "https://example.org/a?b=c"),
                plain(" for "),
                Inline::Mention {
                    user_id: ada,
                    user: None,
                },
                plain(" about "),
                Inline::Ref {
                    display: None,
                    path: None,
                    kind: InlineRefKind::Node,
                    id: node,
                    part: None,
                    name: None,
                    label: Some("Plans".to_owned()),
                },
                plain(" where "),
                Inline::Math {
                    source: "x^2".into(),
                },
                plain(" holds."),
            ],
        );
        let (rendered, back) = there_and_back(&world, std::slice::from_ref(&paragraph));
        assert!(rendered.losses.is_empty(), "{:?}", rendered.losses);
        assert_eq!(shape(&back), shape(&[paragraph]), "{}", rendered.text);
    }

    #[test]
    fn what_looks_like_markdown_in_a_text_stays_text() {
        let tricky = [
            "# not a heading",
            "- not a list",
            "+ nor this",
            "1. nor this",
            "10) nor this",
            "> not a quote",
            "a * b * c and a_b_c",
            "[not](a link) and ![not an image](x)",
            "`not code` and ``` fence",
            "$5 and $6, a ~tilde~ & an &amp; entity <b>not html</b>",
            "back\\slash | bar",
            "---",
            "===",
        ];
        for text in tricky {
            let paragraph = block_of(BlockType::Paragraph, json!({}), vec![plain(text)]);
            let world = World::default();
            let (rendered, back) = there_and_back(&world, std::slice::from_ref(&paragraph));
            assert_eq!(
                shape(&back),
                shape(&[paragraph]),
                "{text:?} was written as {:?}",
                rendered.text
            );
        }
    }

    #[test]
    fn headings_lists_todos_and_quotes_come_back_with_what_is_under_them() {
        let world = World::default();
        let item = |t: BlockType, props: Json, words: &str, kids: Vec<NewBlock>| {
            let made = block_of(t, props, vec![plain(words)]);
            if kids.is_empty() {
                made
            } else {
                children_of(made, kids)
            }
        };
        let blocks = vec![
            block_of(
                BlockType::Heading,
                json!({ "level": 1 }),
                vec![plain("Title")],
            ),
            item(BlockType::Paragraph, json!({}), "Intro.", vec![]),
            block_of(
                BlockType::Heading,
                json!({ "level": 3 }),
                vec![plain("Deep")],
            ),
            item(
                BlockType::Bullet,
                json!({}),
                "one",
                vec![
                    item(BlockType::Bullet, json!({}), "one.a", vec![]),
                    item(BlockType::Todo, json!({ "checked": true }), "one.b", vec![]),
                ],
            ),
            item(BlockType::Todo, json!({ "checked": false }), "two", vec![]),
            item(BlockType::Numbered, json!({ "start": 3 }), "three", vec![]),
            item(
                BlockType::Numbered,
                json!({}),
                "four",
                vec![item(
                    BlockType::Paragraph,
                    json!({}),
                    "para under four",
                    vec![],
                )],
            ),
            item(
                BlockType::Quote,
                json!({}),
                "quoted",
                vec![item(
                    BlockType::Paragraph,
                    json!({}),
                    "second paragraph",
                    vec![],
                )],
            ),
        ];
        let (rendered, back) = there_and_back(&world, &blocks);
        assert!(rendered.losses.is_empty(), "{:?}", rendered.losses);
        assert_eq!(shape(&back), shape(&blocks), "{}", rendered.text);
    }

    #[test]
    fn code_formulas_diagrams_dividers_and_page_breaks_come_back() {
        let world = World::default();
        let blocks = vec![
            block_of(
                BlockType::Code,
                json!({ "language": "rust" }),
                vec![plain("fn main() {\n    println!(\"```\");\n}")],
            ),
            block_of(BlockType::Code, json!({ "language": "" }), vec![]),
            with_props(
                block(BlockType::Math),
                json!({ "source": "\\int_0^1 x\\,dx" }),
            ),
            with_props(
                block(BlockType::Diagram),
                json!({ "source": "graph TD\n  A-->B" }),
            ),
            block(BlockType::Divider),
            block(BlockType::PageBreak),
            block_of(BlockType::Paragraph, json!({}), vec![plain("after")]),
        ];
        let (rendered, back) = there_and_back(&world, &blocks);
        assert!(rendered.losses.is_empty(), "{:?}", rendered.losses);
        assert_eq!(shape(&back), shape(&blocks), "{}", rendered.text);
    }

    #[test]
    fn a_table_with_its_header_row_comes_back_and_a_bar_in_a_cell_does_not_split_it() {
        let world = World::default();
        let cell = |t: &str| TableCell {
            content: vec![plain(t)],
            ..TableCell::default()
        };
        let mut table = with_props(block(BlockType::Table), json!({ "header_row": true }));
        table.content = Some(BlockContent::Rows(vec![
            TableRow {
                cells: vec![cell("Name"), cell("Role")],
            },
            TableRow {
                cells: vec![cell("Ada | the first"), cell("writes")],
            },
            TableRow {
                cells: vec![
                    cell("Bo"),
                    TableCell {
                        content: vec![plain("one"), Inline::Break, plain("two")],
                        ..TableCell::default()
                    },
                ],
            },
        ]));
        let (rendered, back) = there_and_back(&world, &[table.clone()]);
        assert!(rendered.losses.is_empty(), "{:?}", rendered.losses);
        assert_eq!(shape(&back), shape(&[table]), "{}", rendered.text);
    }

    #[test]
    fn images_films_sounds_and_files_are_links_to_the_files_beside_the_text() {
        let mut world = World::default();
        let (png, mp4, mp3, zip) = (
            world.add_file("diagram one.png", "image/png"),
            world.add_file("clip.mp4", "video/mp4"),
            world.add_file("song.mp3", "audio/mpeg"),
            world.add_file("data.zip", "application/zip"),
        );
        let blocks = vec![
            block_of(
                BlockType::Image,
                json!({ "file_id": png }),
                vec![plain("The diagram")],
            ),
            block_of(
                BlockType::Video,
                json!({ "file_id": mp4 }),
                vec![plain("A clip")],
            ),
            block_of(
                BlockType::Audio,
                json!({ "file_id": mp3 }),
                vec![plain("A song")],
            ),
            block_of(
                BlockType::File,
                json!({ "file_id": zip }),
                vec![plain("The data")],
            ),
        ];
        let (rendered, back) = there_and_back(&world, &blocks);
        assert!(rendered.losses.is_empty(), "{:?}", rendered.losses);
        assert!(
            rendered
                .text
                .contains("![The diagram](<files/diagram one.png>)")
        );
        assert_eq!(shape(&back), shape(&blocks), "{}", rendered.text);
    }

    #[test]
    fn a_link_to_a_file_is_the_path_where_the_file_lies_beside_the_text_and_reads_back_as_the_file()
    {
        let mut world = World::default();
        let (png, zip) = (
            world.add_file("diagram one.png", "image/png"),
            world.add_file("data.zip", "application/zip"),
        );
        let blocks = vec![
            block_of(
                BlockType::Image,
                json!({ "file_id": png }),
                vec![plain("The diagram")],
            ),
            block_of(
                BlockType::File,
                json!({ "file_id": zip }),
                vec![plain("The data")],
            ),
        ];
        // A text that lies as `Rollout plan.md` links its files in `Rollout plan.files`, and one of its earlier
        // versions, in `Rollout plan.history`, links them one level up.
        for (directory, link_png, link_zip) in [
            (
                "Rollout plan.files",
                "![The diagram](<Rollout plan.files/diagram one.png>)",
                "[The data](<Rollout plan.files/data.zip>)",
            ),
            (
                "../Plan.files",
                "![The diagram](<../Plan.files/diagram one.png>)",
                "[The data](../Plan.files/data.zip)",
            ),
        ] {
            let rendered = render(
                &stored(&blocks),
                &world,
                &RenderOptions {
                    files_dir: Some(directory),
                    ..RenderOptions::default()
                },
            );
            assert!(rendered.text.contains(link_png), "{}", rendered.text);
            assert!(rendered.text.contains(link_zip), "{}", rendered.text);
            let back = read(&rendered.text, &world);
            assert_eq!(shape(&back), shape(&blocks), "{}", rendered.text);
        }
        // Without a directory the files are named in `files`, as they always were, and that reads back too.
        let plain_dir = render(&stored(&blocks), &world, &RenderOptions::default());
        assert!(
            plain_dir
                .text
                .contains("![The diagram](<files/diagram one.png>)")
        );
    }

    #[test]
    fn a_film_or_a_sound_by_address_is_a_tag_and_comes_back() {
        let world = World::default();
        let film = block_of(
            BlockType::Video,
            json!({ "url": "https://example.org/clip.mp4" }),
            vec![plain("A clip")],
        );
        let sound = block_of(
            BlockType::Audio,
            json!({ "url": "https://example.org/song.mp3" }),
            vec![],
        );
        let (rendered, back) = there_and_back(&world, &[film.clone(), sound.clone()]);
        assert!(rendered.losses.is_empty(), "{:?}", rendered.losses);
        assert!(
            rendered
                .text
                .contains("<video src=\"https://example.org/clip.mp4\">A clip</video>"),
            "{}",
            rendered.text
        );
        assert_eq!(shape(&back), shape(&[film, sound]), "{}", rendered.text);
    }

    #[test]
    fn a_callout_is_a_github_alert_and_comes_back_as_a_callout_of_its_tone() {
        let world = World::default();
        for (tone, word) in [
            ("info", "NOTE"),
            ("success", "TIP"),
            ("warning", "WARNING"),
            ("danger", "CAUTION"),
        ] {
            let callout = children_of(
                block_of(
                    BlockType::Callout,
                    json!({ "tone": tone }),
                    vec![plain("Mind the gap")],
                ),
                vec![block_of(
                    BlockType::Paragraph,
                    json!({}),
                    vec![plain("more")],
                )],
            );
            let (rendered, back) = there_and_back(&world, std::slice::from_ref(&callout));
            assert_eq!(
                rendered.text,
                format!("> [!{word}]\n> Mind the gap\n>\n> more\n")
            );
            assert!(rendered.losses.is_empty(), "{:?}", rendered.losses);
            assert_eq!(shape(&back), shape(&[callout]), "{}", rendered.text);
        }
        let empty = block_of(BlockType::Callout, json!({ "tone": "info" }), vec![]);
        let (rendered, back) = there_and_back(&world, std::slice::from_ref(&empty));
        assert_eq!(rendered.text, "> [!NOTE]\n");
        assert_eq!(shape(&back), shape(&[empty]));
        // What older exports wrote, and what other tools write, is read as a callout too.
        let older = read("> **Warning:** Mind the gap\n", &NoDirectory);
        assert_eq!(older[0].block_type, BlockType::Callout);
        let important = read("> [!IMPORTANT]\n> Read this\n", &NoDirectory);
        assert_eq!(important[0].block_type, BlockType::Callout);
        // A quote that merely opens with bold words is no callout.
        let quote = read("> **Note** this is a quote\n", &NoDirectory);
        assert_eq!(quote[0].block_type, BlockType::Quote);
    }

    #[test]
    fn layout_comments_keep_folding_colour_alignment_nesting_and_spans() {
        let world = World::default();
        let fold = block_of(
            BlockType::Heading,
            json!({ "level": 2, "toggle": true }),
            vec![plain("Folds")],
        );
        let aligned = block_of(
            BlockType::Paragraph,
            json!({ "align": "center", "background": 3 }),
            vec![plain("centred")],
        );
        let nested = children_of(
            block_of(BlockType::Paragraph, json!({}), vec![plain("parent")]),
            vec![block_of(
                BlockType::Paragraph,
                json!({}),
                vec![plain("child")],
            )],
        );
        let mut spans = with_props(block(BlockType::Table), json!({ "header_row": false }));
        let cell = |t: &str, colspan: Option<u32>| TableCell {
            content: vec![plain(t)],
            colspan,
            ..TableCell::default()
        };
        spans.content = Some(BlockContent::Rows(vec![
            TableRow {
                cells: vec![cell("wide", Some(2))],
            },
            TableRow {
                cells: vec![cell("a", None), cell("b", None)],
            },
        ]));
        let blocks = vec![fold, aligned, nested, spans];
        let (rendered, back) = there_and_back(&world, &blocks);
        assert!(rendered.losses.is_empty(), "{:?}", rendered.losses);
        assert_eq!(shape(&back), shape(&blocks), "{}", rendered.text);
        for text in ["## Folds", "centred", "parent", "child", "| wide |  |"] {
            assert!(
                rendered.text.contains(text),
                "{text:?} is not in\n{}",
                rendered.text
            );
        }
        assert!(rendered.text.contains("<!-- orch:block "));
    }

    #[test]
    fn blank_blocks_empty_parents_and_layout_in_numbered_lists_round_trip() {
        let world = World::default();
        let paragraph = |text: &str| block_of(BlockType::Paragraph, json!({}), vec![plain(text)]);
        let mut blocks = vec![paragraph(""), paragraph("   "), paragraph("after blanks")];
        for kind in [
            BlockType::Paragraph,
            BlockType::Quote,
            BlockType::Bullet,
            BlockType::Numbered,
            BlockType::Todo,
        ] {
            let props = if kind == BlockType::Todo {
                json!({"checked":false})
            } else {
                json!({})
            };
            blocks.push(children_of(
                block_of(kind, props, vec![]),
                vec![paragraph("child stays a child")],
            ));
        }
        blocks.extend([
            block_of(
                BlockType::Numbered,
                json!({"start":3}),
                vec![plain("three")],
            ),
            block_of(BlockType::Numbered, json!({"color":1}), vec![plain("four")]),
            block_of(BlockType::Numbered, json!({}), vec![plain("five")]),
            paragraph("after the list"),
        ]);
        let (rendered, back) = there_and_back(&world, &blocks);
        assert!(rendered.losses.is_empty(), "{:?}", rendered.losses);
        assert_eq!(shape(&back), shape(&blocks), "{}", rendered.text);
        assert!(rendered.text.contains("3. three"));
        assert!(rendered.text.contains("4. four"));
        assert!(rendered.text.contains("5. five"));
    }

    #[test]
    fn table_layout_keeps_row_spans_colours_widths_and_both_header_choices() {
        let world = World::default();
        for header in [false, true] {
            let mut table = with_props(
                block(BlockType::Table),
                json!({
                    "header_row":header, "header_column":true, "widths":[2.0,1.0,3.0]
                }),
            );
            let cell = |words: &str| TableCell {
                content: vec![plain(words)],
                ..TableCell::default()
            };
            table.content = Some(BlockContent::Rows(vec![
                TableRow {
                    cells: vec![
                        TableCell {
                            rowspan: Some(2),
                            color: Some(2),
                            background: Some(3),
                            ..cell("down")
                        },
                        TableCell {
                            colspan: Some(2),
                            ..cell("across")
                        },
                    ],
                },
                TableRow {
                    cells: vec![cell("left"), cell("right")],
                },
            ]));
            let (rendered, back) = there_and_back(&world, std::slice::from_ref(&table));
            assert!(rendered.losses.is_empty(), "{:?}", rendered.losses);
            assert_eq!(shape(&back), shape(&[table]), "{}", rendered.text);
        }
    }

    #[test]
    fn an_edit_in_a_covered_table_cell_is_never_discarded_by_old_layout() {
        let world = World::default();
        let mut table = with_props(block(BlockType::Table), json!({"header_row":false}));
        table.content = Some(BlockContent::Rows(vec![TableRow {
            cells: vec![TableCell {
                content: vec![plain("wide")],
                colspan: Some(2),
                ..TableCell::default()
            }],
        }]));
        let rendered = render(&stored(&[table]), &world, &RenderOptions::default());
        let edited = rendered.text.replace("| wide |  |", "| wide | added |");
        assert_ne!(edited, rendered.text);
        let back = read_checked(&edited, &world);
        assert!(!back.warnings.is_empty());
        let cells: Vec<_> = back
            .blocks
            .iter()
            .filter_map(|block| match &block.content {
                Some(BlockContent::Rows(rows)) => Some(rows),
                _ => None,
            })
            .flatten()
            .flat_map(|row| &row.cells)
            .collect();
        assert!(
            cells
                .iter()
                .any(|cell| blocks::inline_text(&cell.content) == "added")
        );
    }

    #[test]
    fn invalid_layout_and_layout_inside_code_keep_the_ordinary_words() {
        let bad = "<!-- orch:block {\"kind\":\"paragraph\",\"props\":{\"file_id\":\"hidden\"}} -->\n\nwords\n\n<!-- /orch:block -->";
        let read = read_checked(bad, &NoDirectory);
        assert!(read.blocks.iter().any(|block| match &block.content {
            Some(BlockContent::Inline(runs)) => blocks::inline_text(runs) == "words",
            _ => false,
        }));
        assert!(!read.warnings.is_empty());
        let code = read_checked(&fenced("text", bad), &NoDirectory);
        assert_eq!(code.blocks.len(), 1);
        assert_eq!(code.blocks[0].block_type, BlockType::Code);
        assert_eq!(
            match &code.blocks[0].content {
                Some(BlockContent::Inline(runs)) => blocks::inline_text(runs),
                _ => String::new(),
            },
            bad
        );
        assert!(code.warnings.is_empty());
    }

    #[test]
    fn media_presentation_code_wrapping_reference_sizes_and_line_breaks_round_trip() {
        let mut world = World::default();
        let image = world.add_file("picture.png", "image/png");
        let archive = world.add_file("files.zip", "application/zip");
        let reference = Inline::Ref {
            display: Some(orch_api_contract::documents::ReferenceSize::Large),
            kind: InlineRefKind::Node,
            id: new_id(),
            path: None,
            part: None,
            name: None,
            label: Some("Plans".to_owned()),
        };
        let blocks = vec![
            block_of(
                BlockType::Image,
                json!({"file_id":image,"display":"medium","width":320.0,"align":"right"}),
                vec![
                    marked("a caption -->", &[Mark::Bold]),
                    Inline::Break,
                    plain("next line"),
                ],
            ),
            block_of(
                BlockType::File,
                json!({"file_id":archive,"display":"small"}),
                vec![],
            ),
            block_of(
                BlockType::Code,
                json!({"language":"mermaid","wrap":true}),
                vec![plain("words, not a diagram")],
            ),
            block_of(
                BlockType::Code,
                json!({"language":"C sharp","wrap":false}),
                vec![plain("code")],
            ),
            block_of(BlockType::Paragraph, json!({}), vec![reference]),
            block_of(
                BlockType::Heading,
                json!({"level":2}),
                vec![plain("first"), Inline::Break, plain("second")],
            ),
            block_of(
                BlockType::Toggle,
                json!({"align":"center"}),
                vec![plain("first"), Inline::Break, plain("second")],
            ),
            with_props(block(BlockType::Math), json!({"source":"  x + y\n"})),
            with_props(block(BlockType::Math), json!({"source":""})),
            with_props(block(BlockType::Math), json!({"source":" \t\n"})),
        ];
        let (rendered, back) = there_and_back(&world, &blocks);
        assert!(rendered.losses.is_empty(), "{:?}", rendered.losses);
        assert_eq!(shape(&back), shape(&blocks), "{}", rendered.text);
        for line in rendered
            .text
            .lines()
            .filter(|line| line.starts_with("<!-- orch:block "))
        {
            assert!(!line.contains(&image.to_string()));
            assert!(!line.contains(&archive.to_string()));
            assert!(line.len() < layout::BYTES_MAX);
        }
    }

    #[test]
    fn annotated_parents_keep_all_sixteen_document_levels() {
        let mut nested = block_of(
            BlockType::Paragraph,
            json!({"background":3}),
            vec![plain("last")],
        );
        for level in (1..16).rev() {
            nested = children_of(
                block_of(
                    BlockType::Paragraph,
                    json!({"color":1}),
                    vec![plain(&format!("level {level}"))],
                ),
                vec![nested],
            );
        }
        let world = World::default();
        let (rendered, back) = there_and_back(&world, std::slice::from_ref(&nested));
        assert_eq!(shape(&back), shape(&[nested]), "{}", rendered.text);
    }

    #[test]
    fn formulas_without_a_dollar_form_keep_their_sources_and_table_pipes() {
        let world = World::default();
        let mut content = Vec::new();
        for source in [
            "",
            "a$b",
            "one\ntwo",
            " x ",
            "   ",
            "@somebody $x$",
            "α $β$",
        ] {
            content.push(Inline::Math {
                source: source.to_owned(),
            });
            content.push(plain(" between "));
        }
        let paragraph = block_of(BlockType::Paragraph, json!({}), content);
        let mut table = with_props(block(BlockType::Table), json!({"header_row":true}));
        table.content = Some(BlockContent::Rows(vec![TableRow {
            cells: ["left|right", "one\ntwo", "x$y"]
                .into_iter()
                .map(|source| TableCell {
                    content: vec![Inline::Math {
                        source: source.to_owned(),
                    }],
                    ..TableCell::default()
                })
                .collect(),
        }]));
        let blocks = vec![paragraph, table];
        let (rendered, back) = there_and_back(&world, &blocks);
        assert_eq!(shape(&back), shape(&blocks), "{}", rendered.text);
        assert!(rendered.text.contains("orch-math="));
    }

    #[test]
    fn whitespace_in_native_parent_blocks_is_content_too() {
        let world = World::default();
        let blocks = [
            (BlockType::Heading, json!({"level":2})),
            (BlockType::Bullet, json!({})),
            (BlockType::Numbered, json!({})),
            (BlockType::Todo, json!({"checked":false})),
            (BlockType::Toggle, json!({})),
            (BlockType::Quote, json!({})),
            (BlockType::Callout, json!({"tone":"info"})),
            (BlockType::Code, json!({"language":"rust"})),
        ]
        .into_iter()
        .map(|(kind, props)| block_of(kind, props, vec![plain(" \t ")]))
        .collect::<Vec<_>>();
        let (rendered, back) = there_and_back(&world, &blocks);
        assert_eq!(shape(&back), shape(&blocks), "{}", rendered.text);
    }

    /// A paragraph of one plain run.
    fn paragraph(text: &str) -> NewBlock {
        block_of(BlockType::Paragraph, json!({}), vec![plain(text)])
    }

    #[test]
    fn entity_spaces_are_content_and_markdown_indentation_is_syntax() {
        let world = World::default();
        let read = read_checked("   ordinary words\n\n&#32;kept&#9;&#32;", &world);
        assert_eq!(
            shape(&read.blocks),
            shape(&[paragraph("ordinary words"), paragraph(" kept\t ")]),
        );
        let blocks = vec![paragraph(" \tfirst"), paragraph("last\t ")];
        let (rendered, back) = there_and_back(&world, &blocks);
        assert_eq!(shape(&back), shape(&blocks), "{}", rendered.text);
    }

    #[test]
    fn changed_formula_code_is_never_replaced_by_the_old_source_attribute() {
        let world = World::default();
        let formula = block_of(
            BlockType::Paragraph,
            json!({}),
            vec![Inline::Math {
                source: "a$b".to_owned(),
            }],
        );
        let rendered = render(&stored(&[formula]), &world, &RenderOptions::default());
        let changed = rendered.text.replace("`a$b`", "`new words`");
        assert_ne!(changed, rendered.text);
        let back = read_checked(&changed, &world);
        let runs = match &back.blocks[0].content {
            Some(BlockContent::Inline(runs)) => runs,
            _ => panic!("the edited formula is still inline content"),
        };
        assert_eq!(blocks::inline_text(runs), "new words");
        assert!(!runs.iter().any(|run| matches!(run, Inline::Math { .. })));
    }

    #[test]
    fn toggles_columns_colours_and_underlines_are_written_with_tags_and_come_back_as_they_went() {
        let world = World::default();
        let coloured = Inline::Text {
            text: "red words".into(),
            marks: Some(vec![Mark::Underline, Mark::Bold]),
            href: None,
            color: Some(0),
            background: Some(2),
        };
        let blocks = vec![
            block_of(
                BlockType::Paragraph,
                json!({}),
                vec![
                    plain("plain "),
                    coloured,
                    plain(" and "),
                    marked("under", &[Mark::Underline]),
                ],
            ),
            children_of(
                block_of(
                    BlockType::Toggle,
                    json!({}),
                    vec![plain("More "), marked("details", &[Mark::Bold])],
                ),
                vec![
                    block_of(BlockType::Paragraph, json!({}), vec![plain("inside")]),
                    children_of(
                        block_of(BlockType::Bullet, json!({}), vec![plain("a list")]),
                        vec![block_of(
                            BlockType::Bullet,
                            json!({}),
                            vec![plain("nested")],
                        )],
                    ),
                    children_of(
                        block_of(BlockType::Toggle, json!({}), vec![plain("deeper")]),
                        vec![block_of(
                            BlockType::Paragraph,
                            json!({}),
                            vec![plain("twice in")],
                        )],
                    ),
                ],
            ),
            block_of(BlockType::Toggle, json!({}), vec![plain("empty")]),
            children_of(
                block(BlockType::Columns),
                vec![
                    children_of(
                        with_props(block(BlockType::Column), json!({ "share": 2 })),
                        vec![
                            block_of(BlockType::Paragraph, json!({}), vec![plain("left")]),
                            block_of(
                                BlockType::Heading,
                                json!({ "level": 2 }),
                                vec![plain("Title")],
                            ),
                        ],
                    ),
                    children_of(
                        block(BlockType::Column),
                        vec![block_of(
                            BlockType::Paragraph,
                            json!({}),
                            vec![plain("right")],
                        )],
                    ),
                ],
            ),
            block(BlockType::PageBreak),
            block_of(BlockType::Paragraph, json!({}), vec![plain("after")]),
        ];
        let (rendered, back) = there_and_back(&world, &blocks);
        assert!(rendered.losses.is_empty(), "{:?}", rendered.losses);
        for text in [
            "<span color=\"red\" background=\"amber\"><u>**red words**</u></span>",
            "<u>under</u>",
            "<details>\n<summary>More **details**</summary>\n\ninside",
            "\n\n</details>",
            "<columns>\n\n<column ratio=\"2\">\n\nleft",
            "<column>\n\nright\n\n</column>\n\n</columns>",
            "<page-break/>",
        ] {
            assert!(
                rendered.text.contains(text),
                "{text:?} is not in\n{}",
                rendered.text
            );
        }
        assert_eq!(shape(&back), shape(&blocks), "{}", rendered.text);
    }

    #[test]
    fn tags_of_the_set_stand_where_markdown_puts_blocks_and_any_other_tag_stays_text() {
        let world = World::default();
        // A toggle and columns inside a list item and a quote; unclosed and stray tags are text.
        let text = "- item\n\n  <details>\n  <summary>in a list</summary>\n\n  body\n\n  </details>\n\n> <details>\n> <summary>in a quote</summary>\n>\n> body\n>\n> </details>\n\n<details>\nnever closed\n\n</columns>\n\n<div>raw</div> and <b>bold</b>\n";
        let blocks = read(text, &world);
        let rendered = render(&stored(&blocks), &world, &RenderOptions::default());
        let types: Vec<BlockType> = blocks.iter().map(|b| b.block_type).collect();
        assert_eq!(types[0], BlockType::Bullet, "{types:?}\n{}", rendered.text);
        let kids = blocks[0].children.as_ref().unwrap();
        assert_eq!(kids[0].block_type, BlockType::Toggle, "{}", rendered.text);
        assert_eq!(types[1], BlockType::Quote);
        assert_eq!(
            blocks[1].children.as_ref().unwrap()[0].block_type,
            BlockType::Toggle,
            "{}",
            rendered.text
        );
        assert!(rendered.text.contains("never closed"));
        assert!(
            rendered.text.contains("<div>raw</div> and <b>bold</b>")
                || rendered.text.contains("\\<div\\>raw")
        );
    }

    #[test]
    fn what_notion_writes_is_read_with_its_children_indented_and_its_colour_names() {
        let world = World::default();
        let text = "<columns>\n\t<column>\n\t\tleft **bold**\n\n\t\t- one\n\t\t- two\n\t</column>\n\t<column ratio=\"50%\">\n\t\tright\n\t</column>\n</columns>\n\n<details>\n<summary>Toggle</summary>\n\tinside the toggle\n</details>\n\n<span color=\"blue_bg\">on blue</span> and <span color=\"gray\">gray</span> and <span underline=\"true\">under</span>\n\n<video src=\"https://example.org/v.mp4\">Caption</video>\n";
        let read = read_checked(text, &world);
        assert!(read.warnings.is_empty(), "{:?}", read.warnings);
        let blocks = read.blocks;
        assert_eq!(blocks[0].block_type, BlockType::Columns);
        let columns = blocks[0].children.as_ref().unwrap();
        assert_eq!(columns.len(), 2);
        let left = columns[0].children.as_ref().unwrap();
        assert_eq!(left[0].block_type, BlockType::Paragraph);
        assert_eq!(left[1].block_type, BlockType::Bullet);
        assert_eq!(left.len(), 3);
        assert_eq!(blocks[1].block_type, BlockType::Toggle);
        let Some(BlockContent::Inline(title)) = &blocks[1].content else {
            panic!("title");
        };
        assert_eq!(blocks::inline_text(title), "Toggle");
        assert_eq!(blocks[1].children.as_ref().unwrap().len(), 1);
        let Some(BlockContent::Inline(runs)) = &blocks[2].content else {
            panic!("runs");
        };
        let backgrounds: Vec<Option<u32>> = runs
            .iter()
            .filter_map(|run| match run {
                Inline::Text {
                    background,
                    color,
                    text,
                    ..
                } if text.contains("blue") => Some(*background),
                Inline::Text { color, text, .. } if text == "gray" => Some(*color),
                _ => None,
            })
            .collect();
        assert_eq!(backgrounds, [Some(7), Some(11)]);
        assert_eq!(blocks[3].block_type, BlockType::Video);
    }

    #[test]
    fn the_headings_alone_come_one_a_line_after_the_number_of_their_block() {
        let world = World::default();
        let blocks = vec![
            block_of(
                BlockType::Heading,
                json!({ "level": 1 }),
                vec![plain("Title")],
            ),
            block_of(BlockType::Paragraph, json!({}), vec![plain("text")]),
            children_of(
                block_of(BlockType::Bullet, json!({}), vec![plain("item")]),
                vec![block_of(
                    BlockType::Heading,
                    json!({ "level": 2 }),
                    vec![plain("Inside")],
                )],
            ),
        ];
        let _ = &world;
        assert_eq!(headings(&stored(&blocks)), "1: # Title\n4: ## Inside\n");
    }

    #[test]
    fn a_document_read_into_a_context_is_preceded_by_who_changed_it_and_when() {
        let at: Time = "2026-10-08T17:04:05.123Z".parse().unwrap();
        assert_eq!(
            context_line("Handbook", "Ada", at),
            "Handbook (last changed by Ada at 2026-10-08 17:04 UTC)"
        );
    }

    #[test]
    fn text_a_person_pastes_becomes_blocks_and_names_that_are_known_become_mentions() {
        let mut world = World::default();
        let ada = world.user("ada");
        let pasted = "# Plan\n\nAsk @ada. or mail bo@example.org about @nobody.\n\n- [ ] one\n- [x] two\n\n```py\nprint(1)\n```\n\nline one  \nline two\n";
        let blocks = read(pasted, &world);
        let types: Vec<BlockType> = blocks.iter().map(|b| b.block_type).collect();
        assert_eq!(
            types,
            [
                BlockType::Heading,
                BlockType::Paragraph,
                BlockType::Todo,
                BlockType::Todo,
                BlockType::Code,
                BlockType::Paragraph
            ]
        );
        let Some(BlockContent::Inline(runs)) = &blocks[1].content else {
            panic!("text");
        };
        assert!(
            runs.iter()
                .any(|run| matches!(run, Inline::Mention { user_id, .. } if *user_id == ada))
        );
        assert_eq!(
            blocks::inline_text(runs).replace('\n', " "),
            "Ask . or mail bo@example.org about @nobody."
        );
        let Some(BlockContent::Inline(last)) = &blocks[5].content else {
            panic!("text");
        };
        assert!(last.iter().any(|run| matches!(run, Inline::Break)));
        // Every block that was read is a block the editor would have made.
        for made in &blocks {
            blocks::check(made.block_type, made.props.as_ref(), made.content.as_ref()).unwrap();
        }
        assert!(matches!(blocks[0].props, Some(BlockProps::Heading(_))));
    }

    #[test]
    fn html_is_never_kept_as_html_and_comments_are_dropped() {
        let blocks = read(
            "<script>alert(1)</script>\n\n<!-- a comment -->\n\ntext <b>bold</b> <br> more\n",
            &NoDirectory,
        );
        let texts: Vec<String> = blocks
            .iter()
            .map(|block| match &block.content {
                Some(BlockContent::Inline(runs)) => blocks::inline_text(runs),
                _ => String::new(),
            })
            .collect();
        assert_eq!(texts[0], "<script>alert(1)</script>");
        assert_eq!(blocks.len(), 2);
        assert_eq!(texts[1], "text <b>bold</b> \n more");
    }

    #[test]
    fn a_list_nested_deeper_than_a_block_may_be_is_flattened_not_refused() {
        let mut text = String::new();
        for depth in 0..25 {
            text.push_str(&"  ".repeat(depth));
            text.push_str("- item\n");
        }
        let blocks = read(&text, &NoDirectory);
        fn deepest(blocks: &[NewBlock]) -> usize {
            blocks
                .iter()
                .map(|b| 1 + deepest(b.children.as_deref().unwrap_or(&[])))
                .max()
                .unwrap_or(0)
        }
        assert!(deepest(&blocks) <= blocks::nesting_limit() + 1);
        let count: usize = {
            fn all(blocks: &[NewBlock]) -> usize {
                blocks
                    .iter()
                    .map(|b| 1 + all(b.children.as_deref().unwrap_or(&[])))
                    .sum()
            }
            all(&blocks)
        };
        assert_eq!(count, 25, "nothing is lost");
    }

    #[test]
    fn the_discussions_are_written_after_the_text_each_under_the_words_it_is_about() {
        let world = World::default();
        let blocks = vec![block_of(
            BlockType::Paragraph,
            json!({}),
            vec![plain("The second paragraph")],
        )];
        let doc = stored(&blocks);
        let author = |name: &str| -> orch_api_contract::users::UserRef {
            serde_json::from_value(json!({
                "id": new_id(), "kind": "human", "username": name.to_lowercase(), "display_name": name,
                "avatar": { "text": "X", "color": 1 }
            }))
            .unwrap()
        };
        let comment =
            |name: &str, said: &str, deleted: bool| orch_api_contract::documents::DocumentComment {
                id: new_id(),
                discussion_id: Id::nil(),
                author: author(name),
                content: if deleted { vec![] } else { vec![plain(said)] },
                reactions: vec![],
                edited_at: None,
                deleted,
                created_at: "2026-10-08T17:10:00.000Z".parse().unwrap(),
                seq_updated: 3,
            };
        let discussion = DocumentDiscussion {
            id: new_id(),
            document_id: Id::nil(),
            block_id: Some(doc[0].id),
            quote: "second paragraph".into(),
            anchored: true,
            resolved: Some(orch_api_contract::documents::DiscussionResolved {
                at: "2026-10-08T18:00:00.000Z".parse().unwrap(),
                by: author("Bo"),
            }),
            created_by: author("Ada"),
            created_at: "2026-10-08T17:10:00.000Z".parse().unwrap(),
            comments: vec![comment("Ada", "Why this?", false), comment("Bo", "", true)],
            seq_updated: 3,
        };
        let without = render(&doc, &world, &RenderOptions::default());
        assert!(!without.text.contains("Comments"));
        let with = render(
            &doc,
            &world,
            &RenderOptions {
                discussions: Some(std::slice::from_ref(&discussion)),
                ..RenderOptions::default()
            },
        );
        assert!(
            with.text
                .starts_with("The second paragraph\n\n## Comments\n"),
            "{}",
            with.text
        );
        for text in [
            "**On block 1, “The second paragraph”**",
            "> second paragraph",
            "(resolved by Bo at 2026-10-08 18:00 UTC)",
            "- Ada (@ada), 2026-10-08 17:10 UTC: Why this?",
            "- Bo (@bo), 2026-10-08 17:10 UTC: (deleted)",
        ] {
            assert!(
                with.text.contains(text),
                "{text:?} is not in\n{}",
                with.text
            );
        }
    }
}
