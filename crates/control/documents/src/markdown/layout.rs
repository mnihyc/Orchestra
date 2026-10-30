//! Bounded, identity-free layout comments beside ordinary Markdown, used only where its native syntax has no form.

use orch_api_contract::documents::{
    BlockContent, BlockType, DocumentBlock, Inline, NewBlock, ReferenceSize, TableCell, TableRow,
};
use orch_types::Json;
use serde::{Deserialize, Serialize};

use crate::blocks;

/// Caption markup, comment escaping and cell coordinates fit within four times the existing block allowance.
pub(super) const BYTES_MAX: usize = 4 * 64 * 1024;
const PROPERTIES: &[&str] = &[
    "color",
    "background",
    "align",
    "toggle",
    "wrap",
    "width",
    "display",
    "header_column",
    "widths",
    "header_row",
    "start",
    "language",
];

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Annotation {
    pub kind: BlockType,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    props: serde_json::Map<String, Json>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cells: Option<Vec<Vec<Cell>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    blank: Option<Vec<Inline>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    refs: Option<Vec<Option<ReferenceSize>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_padding: Option<(String, String)>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub children: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cell {
    column: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    colspan: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rowspan: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    color: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    background: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    refs: Option<Vec<Option<ReferenceSize>>>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn whitespace(run: &Inline) -> bool {
    matches!(run, Inline::Text { text, .. } if text.trim().is_empty())
        || matches!(run, Inline::Break)
}

impl Annotation {
    pub fn of(block: &DocumentBlock, has_children: bool, forced: bool) -> Option<Self> {
        let native = serde_json::to_value(&block.props).ok()?;
        let mut props = serde_json::Map::new();
        for name in PROPERTIES {
            if let Some(value) = native.get(*name)
                && !matches!(*name, "start")
                && !(*name == "header_row" && value == &Json::Bool(true))
                && !(*name == "language"
                    && value.as_str().is_some_and(|language| {
                        !language.eq_ignore_ascii_case("mermaid")
                            && !language.chars().any(|c| c.is_whitespace() || c == '`')
                    }))
            {
                props.insert((*name).to_owned(), value.clone());
            }
        }
        let runs = match &block.content {
            BlockContent::Inline(runs) => runs.as_slice(),
            BlockContent::Rows(_) => &[],
        };
        let blank = ((block.block_type == BlockType::Paragraph || !runs.is_empty())
            && runs.iter().all(whitespace))
        .then(|| runs.to_vec());
        let empty_native_parent = runs.iter().all(whitespace)
            && matches!(
                block.block_type,
                BlockType::Quote | BlockType::Bullet | BlockType::Numbered | BlockType::Todo
            );
        let children = has_children
            && (empty_native_parent
                || !matches!(
                    block.block_type,
                    BlockType::Bullet
                        | BlockType::Numbered
                        | BlockType::Todo
                        | BlockType::Toggle
                        | BlockType::Quote
                        | BlockType::Callout
                        | BlockType::Columns
                        | BlockType::Column
                ));
        let refs: Vec<_> = runs
            .iter()
            .filter_map(|run| match run {
                Inline::Ref { display, .. } => Some(*display),
                _ => None,
            })
            .collect();
        let refs = refs.iter().any(Option::is_some).then_some(refs);
        let cells = match &block.content {
            BlockContent::Rows(rows)
                if rows.is_empty()
                    || rows.first().is_some_and(|first| first.cells.is_empty())
                    || rows
                        .iter()
                        .any(|row| row.cells.len() != rows[0].cells.len())
                    || rows.iter().flat_map(|row| &row.cells).any(|cell| {
                        cell.colspan.is_some()
                            || cell.rowspan.is_some()
                            || cell.color.is_some()
                            || cell.background.is_some()
                            || displays(&cell.content).is_some()
                    }) =>
            {
                Some(cell_layout(rows))
            }
            _ => None,
        };
        let styled_caption = block.block_type == BlockType::Image
            && runs.iter().any(|run| {
                !matches!(
                    run,
                    Inline::Text {
                        marks: None,
                        href: None,
                        color: None,
                        background: None,
                        ..
                    }
                )
            });
        let blank_file_caption = matches!(
            block.block_type,
            BlockType::File | BlockType::Video | BlockType::Audio
        ) && native.get("file_id").is_some()
            && runs.iter().all(whitespace);
        let caption = (styled_caption || blank_file_caption).then(String::new);
        let source_padding = if block.block_type == BlockType::Math {
            native
                .get("source")
                .and_then(Json::as_str)
                .and_then(|source| {
                    let core = source.trim();
                    (source != core || core.is_empty()).then(|| {
                        let lead = source.len() - source.trim_start().len();
                        let tail = if core.is_empty() {
                            source.len()
                        } else {
                            source.trim_end().len()
                        };
                        (source[..lead].to_owned(), source[tail..].to_owned())
                    })
                })
        } else {
            None
        };
        if !forced
            && props.is_empty()
            && blank.is_none()
            && !children
            && refs.is_none()
            && cells.is_none()
            && caption.is_none()
            && source_padding.is_none()
        {
            return None;
        }
        // A separately written numbered item otherwise acquires its displayed number as a new `start` property.
        if block.block_type == BlockType::Numbered {
            props.insert(
                "start".to_owned(),
                native.get("start").cloned().unwrap_or(Json::Null),
            );
        }
        Some(Self {
            kind: block.block_type,
            props,
            cells,
            blank,
            caption,
            refs,
            source_padding,
            children,
        })
    }

    pub fn read(text: &str) -> Option<Self> {
        if text.len() > BYTES_MAX {
            return None;
        }
        let annotation: Self = serde_json::from_str(text).ok()?;
        if annotation
            .props
            .keys()
            .any(|name| !PROPERTIES.contains(&name.as_str()))
            || annotation
                .blank
                .as_ref()
                .is_some_and(|runs| !runs.iter().all(whitespace))
            || annotation
                .source_padding
                .as_ref()
                .is_some_and(|(lead, tail)| !lead.trim().is_empty() || !tail.trim().is_empty())
        {
            return None;
        }
        Some(annotation)
    }

    /// Restores layout only when the ordinary Markdown still agrees with its shape. No words are supplied by props.
    pub fn restore(&self, mut parsed: Vec<NewBlock>) -> Option<NewBlock> {
        if parsed.is_empty() && self.kind == BlockType::Paragraph && self.blank.is_some() {
            parsed.push(super::block(BlockType::Paragraph));
        }
        if parsed.is_empty()
            && self.kind == BlockType::Table
            && let Some(cells) = &self.cells
            && cells.iter().all(Vec::is_empty)
        {
            let mut table = super::with_props(
                super::block(BlockType::Table),
                serde_json::json!({"header_row":true}),
            );
            table.content = Some(BlockContent::Rows(
                cells
                    .iter()
                    .map(|_| TableRow { cells: Vec::new() })
                    .collect(),
            ));
            parsed.push(table);
        }
        if parsed.len() == 1
            && self.kind == BlockType::Code
            && parsed[0].block_type == BlockType::Diagram
        {
            let source = parsed[0]
                .props
                .as_ref()
                .and_then(|props| serde_json::to_value(props).ok())?
                .get("source")?
                .as_str()?
                .to_owned();
            parsed[0] = super::with_text(
                super::with_props(
                    super::block(BlockType::Code),
                    serde_json::json!({"language":"mermaid"}),
                ),
                if source.is_empty() {
                    Vec::new()
                } else {
                    vec![super::text_run(
                        &source,
                        super::Formats::default(),
                        None,
                        false,
                    )]
                },
            );
        }
        if parsed.len() != 1 || parsed[0].block_type != self.kind {
            return None;
        }
        let mut made = parsed.pop()?;
        let mut props = made
            .props
            .as_ref()
            .and_then(|props| serde_json::to_value(props).ok())
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default();
        for (name, value) in &self.props {
            if value.is_null() {
                props.remove(name);
            } else {
                props.insert(name.clone(), value.clone());
            }
        }
        if let Some((lead, tail)) = &self.source_padding {
            if self.kind != BlockType::Math {
                return None;
            }
            let core = props.get("source")?.as_str()?;
            props.insert(
                "source".to_owned(),
                Json::String(format!("{lead}{core}{tail}")),
            );
        }
        made.props = Some(serde_json::from_value(Json::Object(props)).ok()?);
        if let Some(blank) = &self.blank {
            let current = match &made.content {
                Some(BlockContent::Inline(runs)) => runs.as_slice(),
                None => &[],
                Some(BlockContent::Rows(_)) => return None,
            };
            if !current.iter().all(whitespace) {
                return None;
            }
            made.content = Some(BlockContent::Inline(blank.clone()));
        }
        if let Some(hints) = &self.cells {
            let Some(BlockContent::Rows(rows)) = &made.content else {
                return None;
            };
            made.content = Some(BlockContent::Rows(restore_cells(rows, hints)?));
        }
        if let Some(displays) = &self.refs
            && let Some(BlockContent::Inline(runs)) = &mut made.content
        {
            restore_refs(runs, displays);
        }
        blocks::check(made.block_type, made.props.as_ref(), made.content.as_ref()).ok()?;
        Some(made)
    }
}

fn span(value: Option<u32>) -> usize {
    usize::try_from(value.unwrap_or(1)).unwrap_or(1).max(1)
}

fn cell_layout(rows: &[TableRow]) -> Vec<Vec<Cell>> {
    let width = rows.first().map_or(0, |row| {
        row.cells.iter().map(|cell| span(cell.colspan)).sum()
    });
    let mut occupied = vec![vec![false; width]; rows.len()];
    rows.iter()
        .enumerate()
        .map(|(r, row)| {
            let mut column = 0;
            row.cells
                .iter()
                .map(|cell| {
                    while column < width && occupied[r][column] {
                        column += 1;
                    }
                    let hint = Cell {
                        column,
                        colspan: cell.colspan,
                        rowspan: cell.rowspan,
                        color: cell.color,
                        background: cell.background,
                        refs: displays(&cell.content),
                    };
                    let end = (column + span(cell.colspan)).min(width);
                    for covered in occupied
                        .iter_mut()
                        .take((r + span(cell.rowspan)).min(rows.len()))
                        .skip(r)
                    {
                        covered[column.min(width)..end].fill(true);
                    }
                    column = end;
                    hint
                })
                .collect()
        })
        .collect()
}

fn restore_cells(rows: &[TableRow], hints: &[Vec<Cell>]) -> Option<Vec<TableRow>> {
    if rows.len() != hints.len() {
        return None;
    }
    if rows.is_empty() {
        return Some(Vec::new());
    }
    let width = rows.first()?.cells.len();
    if rows.iter().any(|row| row.cells.len() != width) {
        return None;
    }
    let mut occupied = vec![vec![false; width]; rows.len()];
    let mut restored = Vec::with_capacity(rows.len());
    for (r, row) in hints.iter().enumerate() {
        let mut cells = Vec::with_capacity(row.len());
        for hint in row {
            let col = hint.column;
            let across = col.checked_add(span(hint.colspan))?;
            let down = r.checked_add(span(hint.rowspan))?;
            if col >= width || across > width || down > rows.len() {
                return None;
            }
            for (y, covered) in occupied.iter_mut().enumerate().take(down).skip(r) {
                for (x, used) in covered.iter_mut().enumerate().take(across).skip(col) {
                    if *used || ((y, x) != (r, col) && !rows[y].cells[x].content.is_empty()) {
                        return None;
                    }
                    *used = true;
                }
            }
            let mut content = rows[r].cells[col].content.clone();
            if let Some(displays) = &hint.refs {
                restore_refs(&mut content, displays);
            }
            cells.push(TableCell {
                content,
                colspan: hint.colspan,
                rowspan: hint.rowspan,
                color: hint.color,
                background: hint.background,
            });
        }
        restored.push(TableRow { cells });
    }
    for (r, row) in occupied.iter().enumerate() {
        for (c, used) in row.iter().enumerate() {
            if !used && !rows[r].cells[c].content.is_empty() {
                return None;
            }
        }
    }
    Some(restored)
}

fn displays(runs: &[Inline]) -> Option<Vec<Option<ReferenceSize>>> {
    let displays: Vec<_> = runs
        .iter()
        .filter_map(|run| match run {
            Inline::Ref { display, .. } => Some(*display),
            _ => None,
        })
        .collect();
    displays.iter().any(Option::is_some).then_some(displays)
}

fn restore_refs(runs: &mut [Inline], displays: &[Option<ReferenceSize>]) {
    if runs
        .iter()
        .filter(|run| matches!(run, Inline::Ref { .. }))
        .count()
        == displays.len()
    {
        let mut displays = displays.iter();
        for run in runs {
            if let Inline::Ref { display, .. } = run
                && let Some(saved) = displays.next()
            {
                *display = *saved;
            }
        }
    }
}
