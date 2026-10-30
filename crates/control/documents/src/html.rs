//! A document written out as one HTML file that carries its images and needs nothing else to be read.
//!
//! Colours, alignment, columns, page breaks and spans are kept, which
//! Markdown cannot. The page holds no script and fetches nothing: its
//! style is in it, its images are in it as data, a formula or a diagram is
//! shown as its source, and a film or a sound, which is an attachment, is
//! named where it stood (and linked, when the download is an archive with a
//! `files/` directory beside the page). A film or a sound by address shows
//! the address and where it plays from, and fetches nothing until a reader
//! follows the link. Everything written in a block is text, escaped here;
//! no block can put markup into the page.

use std::fmt::Write as _;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use orch_api_contract::documents::{
    BlockContent, BlockType, DocumentBlock, DocumentDiscussion, Inline, InlineRefKind, Mark,
    TableCell,
};
use orch_types::{Id, Json};

use crate::blocks::{self, Lookup, Outline};
use crate::files::{self, FileInfo};

/// What the page is asked to hold beside its text and its images.
#[derive(Debug, Clone, Copy, Default)]
pub struct Options<'a> {
    /// The discussions beside the document, to be written after the text.
    pub discussions: Option<&'a [DocumentDiscussion]>,
    /// Whether the files that are no images go with the page in an archive,
    /// under `files/`: they are then linked, and otherwise only named.
    pub attachments_beside: bool,
    /// The local directory of a package's images and attachments. Their bytes stay in separate archive entries.
    pub files_dir: Option<&'a str>,
}

/// The style of the page: one sheet, in the page. The twelve tints are the
/// palette that a block's colour indexes, in a light and a dark theme.
const STYLE: &str = "\
:root{color-scheme:light dark;--fg:#1f2328;--bg:#ffffff;--muted:#59636e;--line:#d1d9e0;--soft:#f6f8fa;\
--t0:#59636e;--t1:#8a5a2b;--t2:#bc4c00;--t3:#7d6b00;--t4:#1a7f37;--t5:#0f7b7b;--t6:#0969da;--t7:#8250df;--t8:#bf3989;--t9:#cf222e;--t10:#44546a;--t11:#6e7781;\
--b0:#eef0f2;--b1:#f3e6d8;--b2:#ffe8d1;--b3:#fff4c2;--b4:#dcf5e1;--b5:#d5f3f3;--b6:#ddeeff;--b7:#ece4ff;--b8:#ffe3f1;--b9:#ffe0e2;--b10:#e1e6ee;--b11:#e9ebee}\
@media (prefers-color-scheme:dark){:root{--fg:#e6edf3;--bg:#0d1117;--muted:#9198a1;--line:#3d444d;--soft:#151b23;\
--t0:#9198a1;--t1:#d2a679;--t2:#ffa657;--t3:#e3c35b;--t4:#56d364;--t5:#4fd1d1;--t6:#79c0ff;--t7:#d2a8ff;--t8:#ff9bce;--t9:#ff7b72;--t10:#9bb0cc;--t11:#8b949e;\
--b0:#262c34;--b1:#3a2d20;--b2:#40290f;--b3:#3d3305;--b4:#12351c;--b5:#0e3535;--b6:#10304f;--b7:#2f2250;--b8:#491a35;--b9:#4a1b1f;--b10:#242d3b;--b11:#2a2f36}}\
body{margin:0;background:var(--bg);color:var(--fg);font:16px/1.6 system-ui,-apple-system,Segoe UI,Roboto,sans-serif}\
main{max-width:820px;margin:0 auto;padding:2rem 1.25rem 4rem}\
h1.title{font-size:2.1rem;margin:0 0 1.5rem}h1,h2,h3,h4,h5,h6{line-height:1.25;margin:1.6em 0 .5em}\
p{margin:.6em 0}ul,ol{margin:.4em 0;padding-left:1.6em}li{margin:.2em 0}ul.todos{list-style:none;padding-left:.4em}\
blockquote{margin:.8em 0;padding:.1em 1em;border-left:4px solid var(--line);color:var(--muted)}\
aside.callout{margin:.8em 0;padding:.7em 1em;border-radius:6px;border-left:4px solid var(--line);background:var(--soft)}\
aside.tone-info{border-left-color:#0969da}aside.tone-success{border-left-color:#1a7f37}aside.tone-warning{border-left-color:#bf8700}aside.tone-danger{border-left-color:#cf222e}\
pre{background:var(--soft);padding:.8em 1em;border-radius:6px;overflow-x:auto;font:14px/1.5 ui-monospace,SFMono-Regular,Menlo,monospace}\
code{font:.92em ui-monospace,SFMono-Regular,Menlo,monospace}:not(pre)>code{background:var(--soft);padding:.1em .3em;border-radius:4px}\
pre.wrap{white-space:pre-wrap}.math{font-style:italic}div.math{text-align:center;margin:.8em 0}\
figure{margin:1em 0}figure.align-center{text-align:center}figure.align-right{text-align:right}img{max-width:100%;height:auto}figcaption{color:var(--muted);font-size:.9em}\
table{border-collapse:collapse;margin:1em 0;max-width:100%}th,td{border:1px solid var(--line);padding:.35em .7em;text-align:left;vertical-align:top}th{background:var(--soft)}\
details.toggle>summary,details.fold>summary{cursor:pointer}details.fold>summary>*{display:inline}\
div.columns{display:flex;gap:1.5rem;flex-wrap:wrap}div.column{flex:1 1 12rem;min-width:0}\
div.page-break{border-top:1px dashed var(--line);margin:2rem 0}@media print{div.page-break{break-after:page;border:0}}\
.attachment,.media{color:var(--muted)}.mention{background:var(--b6);border-radius:4px;padding:0 .2em}.ref{background:var(--soft);border-radius:4px;padding:0 .2em}\
.align-center{text-align:center}.align-right{text-align:right}.align-justify{text-align:justify}\
section.comments{margin-top:3rem;border-top:1px solid var(--line)}article.discussion{margin:1.2em 0}article.resolved{opacity:.7}\
article.discussion blockquote{margin:.3em 0}.who{font-weight:600}.when{color:var(--muted);font-size:.85em}\
.t0{color:var(--t0)}.t1{color:var(--t1)}.t2{color:var(--t2)}.t3{color:var(--t3)}.t4{color:var(--t4)}.t5{color:var(--t5)}.t6{color:var(--t6)}.t7{color:var(--t7)}.t8{color:var(--t8)}.t9{color:var(--t9)}.t10{color:var(--t10)}.t11{color:var(--t11)}\
.b0{background:var(--b0)}.b1{background:var(--b1)}.b2{background:var(--b2)}.b3{background:var(--b3)}.b4{background:var(--b4)}.b5{background:var(--b5)}.b6{background:var(--b6)}.b7{background:var(--b7)}.b8{background:var(--b8)}.b9{background:var(--b9)}.b10{background:var(--b10)}.b11{background:var(--b11)}";

/// The fixed style and room for a page's header and footer in an archive's size estimate.
pub const PAGE_OVERHEAD_BYTES: usize = STYLE.len() + 1024;

/// Text as it is written in a page: nothing in it is markup.
#[must_use]
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
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

fn tint_classes(color: Option<u32>, background: Option<u32>) -> Vec<String> {
    let mut classes = Vec::new();
    if let Some(color) = color.filter(|tint| *tint < 12) {
        classes.push(format!("t{color}"));
    }
    if let Some(background) = background.filter(|tint| *tint < 12) {
        classes.push(format!("b{background}"));
    }
    classes
}

fn look_of(block: &DocumentBlock) -> Vec<String> {
    let number = |member: &str| {
        prop(block, member)
            .and_then(|v| v.as_u64())
            .and_then(|n| u32::try_from(n).ok())
    };
    let mut classes = tint_classes(number("color"), number("background"));
    if let Some(align) = prop(block, "align").and_then(|v| v.as_str().map(str::to_owned))
        && matches!(align.as_str(), "center" | "right" | "justify")
    {
        classes.push(format!("align-{align}"));
    }
    classes
}

fn class_attr(classes: &[String]) -> String {
    if classes.is_empty() {
        String::new()
    } else {
        format!(" class=\"{}\"", classes.join(" "))
    }
}

fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    rest.split(['/', '?', '#'])
        .next()
        .unwrap_or(rest)
        .rsplit('@')
        .next()
        .unwrap_or(rest)
        .to_owned()
}

fn size_text(size: i64) -> String {
    const KIB: i64 = 1024;
    if size >= KIB * KIB {
        format!("{:.1} MB", size as f64 / (KIB * KIB) as f64)
    } else if size >= KIB {
        format!("{:.1} KB", size as f64 / KIB as f64)
    } else {
        format!("{size} bytes")
    }
}

/// A safe type for a data address: the characters a media type is made of.
fn data_type(media_type: &str) -> Option<String> {
    let lower = media_type.to_ascii_lowercase();
    let ok = lower.starts_with("image/")
        && lower
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '+' | '-' | '.'));
    ok.then_some(lower)
}

struct Page<'a, 'b> {
    lookup: &'a dyn Lookup,
    outline: &'b Outline<'b>,
    options: Options<'a>,
}

impl<'b> Page<'_, 'b> {
    fn inline(&self, runs: &[Inline]) -> String {
        let mut out = String::new();
        let mut index = 0;
        let runs = crate::markdown::merged(runs);
        while index < runs.len() {
            let href = match &runs[index] {
                Inline::Text {
                    href: Some(href), ..
                } if blocks::allowed_href(href) => Some(href.as_str()),
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
                inner.push_str(&self.run(run));
            }
            match href {
                Some(href) => {
                    let _ = write!(
                        out,
                        "<a href=\"{}\" rel=\"noopener noreferrer\">{inner}</a>",
                        escape(href)
                    );
                }
                None => out.push_str(&inner),
            }
            index = end;
        }
        out
    }

    fn run(&self, run: &Inline) -> String {
        match run {
            Inline::Text {
                text,
                marks,
                color,
                background,
                ..
            } => {
                let marks = marks.clone().unwrap_or_default();
                let mut out = escape(text).replace('\n', "<br>");
                if marks.contains(&Mark::Code) {
                    out = format!("<code>{out}</code>");
                }
                for (mark, tag) in [
                    (Mark::Italic, "em"),
                    (Mark::Bold, "strong"),
                    (Mark::Underline, "u"),
                    (Mark::Strike, "s"),
                ] {
                    if marks.contains(&mark) {
                        out = format!("<{tag}>{out}</{tag}>");
                    }
                }
                let classes = tint_classes(*color, *background);
                if classes.is_empty() {
                    out
                } else {
                    format!("<span{}>{out}</span>", class_attr(&classes))
                }
            }
            Inline::Mention { user_id, .. } => {
                let name = self
                    .lookup
                    .username(*user_id)
                    .unwrap_or_else(|| "unknown".to_owned());
                format!("<span class=\"mention\">@{}</span>", escape(&name))
            }
            Inline::Ref { kind, id, .. } => {
                let what = match kind {
                    InlineRefKind::Node => "a node",
                    InlineRefKind::Document => "a document",
                    InlineRefKind::Chat => "a chat",
                    InlineRefKind::File | InlineRefKind::EnvironmentFile => "a file",
                };
                let name = self
                    .lookup
                    .name_of(*kind, *id)
                    .unwrap_or_else(|| what.to_owned());
                format!("<span class=\"ref\">{}</span>", escape(&name))
            }
            Inline::Math { source } => format!("<code class=\"math\">{}</code>", escape(source)),
            Inline::Break => "<br>".to_owned(),
        }
    }

    fn children(&self, block: &DocumentBlock) -> String {
        let kids = self.outline.children_of(block.id);
        self.sequence(&kids)
    }

    fn sequence(&self, siblings: &[&'b DocumentBlock]) -> String {
        let mut out = String::new();
        let mut index = 0;
        while index < siblings.len() {
            let block = siblings[index];
            match block.block_type {
                BlockType::Bullet | BlockType::Todo => {
                    let mut end = index;
                    while end < siblings.len()
                        && matches!(
                            siblings[end].block_type,
                            BlockType::Bullet | BlockType::Todo
                        )
                    {
                        end += 1;
                    }
                    let todos = siblings[index..end]
                        .iter()
                        .all(|each| each.block_type == BlockType::Todo);
                    let class = if todos { " class=\"todos\"" } else { "" };
                    let _ = write!(out, "<ul{class}>");
                    for each in &siblings[index..end] {
                        out.push_str(&self.item(each));
                    }
                    out.push_str("</ul>\n");
                    index = end;
                }
                BlockType::Numbered => {
                    let mut end = index;
                    while end < siblings.len() && siblings[end].block_type == BlockType::Numbered {
                        end += 1;
                    }
                    let start = prop(block, "start")
                        .and_then(|v| v.as_u64())
                        .filter(|n| *n != 1);
                    match start {
                        Some(start) => {
                            let _ = write!(out, "<ol start=\"{start}\">");
                        }
                        None => out.push_str("<ol>"),
                    }
                    for each in &siblings[index..end] {
                        out.push_str(&self.item(each));
                    }
                    out.push_str("</ol>\n");
                    index = end;
                }
                _ => {
                    out.push_str(&self.block(block));
                    index += 1;
                }
            }
        }
        out
    }

    fn item(&self, block: &DocumentBlock) -> String {
        let classes = look_of(block);
        let text = self.inline(runs_of(block));
        let children = self.children(block);
        if block.block_type == BlockType::Todo {
            let ticked = prop(block, "checked").and_then(|v| v.as_bool()) == Some(true);
            let mark = if ticked { " checked" } else { "" };
            format!(
                "<li{}><input type=\"checkbox\" disabled{mark}> {text}{children}</li>",
                class_attr(&classes)
            )
        } else {
            format!("<li{}>{text}{children}</li>", class_attr(&classes))
        }
    }

    fn file(&self, block: &DocumentBlock) -> Option<(Id, FileInfo)> {
        let id = prop(block, "file_id")?.as_str()?.parse::<Id>().ok()?;
        Some((id, self.lookup.file(id)?))
    }

    fn block(&self, block: &'b DocumentBlock) -> String {
        let classes = look_of(block);
        let class = class_attr(&classes);
        let text = self.inline(runs_of(block));
        match block.block_type {
            BlockType::Paragraph => {
                let children = self.children(block);
                if text.is_empty() {
                    format!("<p class=\"empty\"><br></p>\n{children}")
                } else {
                    format!("<p{class}>{text}</p>\n{children}")
                }
            }
            BlockType::Heading => {
                let level = prop(block, "level")
                    .and_then(|v| v.as_u64())
                    .map_or(1, |level| level.clamp(1, 6));
                let children = self.children(block);
                let heading = format!("<h{level}{class}>{text}</h{level}>");
                if prop(block, "toggle").and_then(|v| v.as_bool()) == Some(true) {
                    format!(
                        "<details class=\"fold\" open><summary>{heading}</summary>\n{children}</details>\n"
                    )
                } else {
                    format!("{heading}\n{children}")
                }
            }
            BlockType::Toggle => format!(
                "<details class=\"toggle\"><summary{class}>{text}</summary>\n{}</details>\n",
                self.children(block)
            ),
            BlockType::Quote => {
                format!(
                    "<blockquote{class}><p>{text}</p>\n{}</blockquote>\n",
                    self.children(block)
                )
            }
            BlockType::Callout => {
                let tone = prop(block, "tone")
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .filter(|tone| {
                        matches!(tone.as_str(), "info" | "success" | "warning" | "danger")
                    })
                    .unwrap_or_else(|| "info".to_owned());
                let mut all = vec!["callout".to_owned(), format!("tone-{tone}")];
                all.extend(classes);
                format!(
                    "<aside{}><p>{text}</p>\n{}</aside>\n",
                    class_attr(&all),
                    self.children(block)
                )
            }
            BlockType::Code => {
                let language = prop(block, "language")
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_default();
                let wrap = prop(block, "wrap").and_then(|v| v.as_bool()) == Some(true);
                let body = escape(&blocks::inline_text(runs_of(block)));
                let pre = if wrap { " class=\"wrap\"" } else { "" };
                let lang: String = language
                    .chars()
                    .filter(|c| {
                        c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '+' | '#' | '.')
                    })
                    .collect();
                if lang.is_empty() {
                    format!("<pre{pre}><code>{body}</code></pre>\n")
                } else {
                    format!("<pre{pre}><code class=\"language-{lang}\">{body}</code></pre>\n")
                }
            }
            BlockType::Math => {
                let source = prop(block, "source")
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_default();
                format!(
                    "<div class=\"math\"><code>{}</code></div>\n",
                    escape(&source)
                )
            }
            BlockType::Diagram => {
                let source = prop(block, "source")
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_default();
                format!(
                    "<figure class=\"diagram\"><pre><code class=\"language-mermaid\">{}</code></pre><figcaption>Diagram, as its Mermaid source</figcaption></figure>\n",
                    escape(&source)
                )
            }
            BlockType::Divider => "<hr>\n".to_owned(),
            BlockType::PageBreak => {
                "<div class=\"page-break\" role=\"separator\"></div>\n".to_owned()
            }
            BlockType::Image => self.image(block, &text),
            BlockType::Video | BlockType::Audio | BlockType::File => self.attachment(block, &text),
            BlockType::Table => self.table(block),
            BlockType::Columns => {
                let mut out = String::from("<div class=\"columns\">");
                for column in self.outline.children_of(block.id) {
                    let share = prop(column, "share")
                        .and_then(|v| v.as_f64())
                        .filter(|s| s.is_finite() && *s > 0.0);
                    let style = share
                        .map_or_else(String::new, |share| format!(" style=\"flex-grow:{share}\""));
                    let _ = write!(
                        out,
                        "<div class=\"column\"{style}>{}</div>",
                        self.children(column)
                    );
                }
                out.push_str("</div>\n");
                out
            }
            BlockType::Column => self.children(block),
            BlockType::Bullet | BlockType::Numbered | BlockType::Todo => self.item(block),
        }
    }

    fn image(&self, block: &DocumentBlock, caption: &str) -> String {
        let align = prop(block, "align")
            .and_then(|v| v.as_str().map(str::to_owned))
            .filter(|align| matches!(align.as_str(), "center" | "right"))
            .map_or_else(String::new, |align| format!(" class=\"align-{align}\""));
        let width = prop(block, "width")
            .and_then(|v| v.as_f64())
            .filter(|w| w.is_finite() && *w > 0.0)
            .map_or_else(String::new, |w| format!(" style=\"width:{w}px\""));
        let plain = escape(&blocks::inline_text(runs_of(block)));
        let figcaption = if caption.is_empty() {
            String::new()
        } else {
            format!("<figcaption>{caption}</figcaption>")
        };
        let embedded = self.file(block).and_then(|(_, file)| {
            if let Some(directory) = self.options.files_dir {
                if !files::is_image(&file.media_type) {
                    return None;
                }
                let path = escape(&companion_path(directory, &file.name));
                let alt = if plain.is_empty() {
                    escape(&file.name)
                } else {
                    plain.clone()
                };
                return Some(format!("<img src=\"{path}\" alt=\"{alt}\"{width}>"));
            }
            let bytes = file.bytes.as_ref()?;
            let media_type = data_type(&file.media_type)?;
            Some(format!(
                "<img src=\"data:{media_type};base64,{}\" alt=\"{}\"{width}>",
                STANDARD.encode(bytes),
                if plain.is_empty() {
                    escape(&file.name)
                } else {
                    plain.clone()
                }
            ))
        });
        match embedded {
            Some(img) => format!("<figure{align}>{img}{figcaption}</figure>\n"),
            None => format!(
                "<figure{align}><p class=\"attachment\">An image that is not in this file.</p>{figcaption}</figure>\n"
            ),
        }
    }

    fn attachment(&self, block: &DocumentBlock, caption: &str) -> String {
        let kind = match block.block_type {
            BlockType::Video => "Film",
            BlockType::Audio => "Sound",
            _ => "File",
        };
        if let Some(url) = prop(block, "url").and_then(|v| v.as_str().map(str::to_owned))
            && blocks::allowed_media_url(&url)
        {
            let host = escape(&host_of(&url));
            let url = escape(&url);
            return format!(
                "<p class=\"media\">{kind}: <a href=\"{url}\" rel=\"noopener noreferrer\">Play from {host}</a>{}</p>\n",
                if caption.is_empty() {
                    String::new()
                } else {
                    format!(" — {caption}")
                }
            );
        }
        let Some((_, file)) = self.file(block) else {
            return format!(
                "<p class=\"attachment\">{kind}: {}</p>\n",
                if caption.is_empty() {
                    "(not in this file)"
                } else {
                    caption
                }
            );
        };
        let name = escape(&file.name);
        let what = format!("{}, {}", escape(&file.media_type), size_text(file.size));
        let label = if caption.is_empty() {
            name.clone()
        } else {
            caption.to_owned()
        };
        if let Some(directory) = self.options.files_dir {
            let path = escape(&companion_path(directory, &file.name));
            format!("<p class=\"attachment\">{kind}: <a href=\"{path}\">{label}</a> ({what})</p>\n")
        } else if self.options.attachments_beside && !files::is_image(&file.media_type) {
            format!(
                "<p class=\"attachment\">{kind}: <a href=\"files/{}\">{label}</a> ({what})</p>\n",
                escape(&percent_path(&file.name))
            )
        } else {
            format!(
                "<p class=\"attachment\">{kind}: {label} ({name}, {what}; not in this file)</p>\n"
            )
        }
    }

    fn table(&self, block: &DocumentBlock) -> String {
        let BlockContent::Rows(rows) = &block.content else {
            return String::new();
        };
        let header_row = prop(block, "header_row").and_then(|v| v.as_bool()) == Some(true);
        let header_column = prop(block, "header_column").and_then(|v| v.as_bool()) == Some(true);
        let widths: Vec<f64> = prop(block, "widths")
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();
        let mut out = String::from("<table>");
        let total: f64 = widths.iter().sum();
        if total > 0.0 {
            out.push_str("<colgroup>");
            for width in &widths {
                let _ = write!(out, "<col style=\"width:{:.2}%\">", width / total * 100.0);
            }
            out.push_str("</colgroup>");
        }
        for (r, row) in rows.iter().enumerate() {
            if r == 0 && header_row {
                out.push_str("<thead>");
            } else if r == usize::from(header_row) {
                out.push_str("<tbody>");
            }
            out.push_str("<tr>");
            for (c, cell) in row.cells.iter().enumerate() {
                let head = (r == 0 && header_row) || (c == 0 && header_column);
                out.push_str(&self.cell(cell, head, r == 0 && header_row));
            }
            out.push_str("</tr>");
            if r == 0 && header_row {
                out.push_str("</thead>");
            }
        }
        if rows.len() > usize::from(header_row) {
            out.push_str("</tbody>");
        }
        out.push_str("</table>\n");
        out
    }

    fn cell(&self, cell: &TableCell, head: bool, column_head: bool) -> String {
        let tag = if head { "th" } else { "td" };
        let mut attrs = String::new();
        if head {
            attrs.push_str(if column_head {
                " scope=\"col\""
            } else {
                " scope=\"row\""
            });
        }
        if let Some(span) = cell.colspan.filter(|n| *n > 1) {
            let _ = write!(attrs, " colspan=\"{span}\"");
        }
        if let Some(span) = cell.rowspan.filter(|n| *n > 1) {
            let _ = write!(attrs, " rowspan=\"{span}\"");
        }
        let classes = tint_classes(cell.color, cell.background);
        attrs.push_str(&class_attr(&classes));
        format!("<{tag}{attrs}>{}</{tag}>", self.inline(&cell.content))
    }

    fn comments(
        &self,
        blocks_list: &[DocumentBlock],
        discussions: &[DocumentDiscussion],
    ) -> String {
        let mut out = String::from("<section class=\"comments\"><h2>Comments</h2>\n");
        let order: Vec<Id> = Outline::new(blocks_list)
            .walk()
            .into_iter()
            .map(|(block, _)| block.id)
            .collect();
        let mut sorted: Vec<&DocumentDiscussion> = discussions.iter().collect();
        sorted.sort_by_key(|discussion| {
            (
                discussion
                    .block_id
                    .and_then(|id| order.iter().position(|each| *each == id))
                    .unwrap_or(usize::MAX),
                discussion.created_at,
            )
        });
        for discussion in sorted {
            let resolved = discussion.resolved.is_some();
            let _ = writeln!(
                out,
                "<article class=\"discussion{}\">",
                if resolved { " resolved" } else { "" }
            );
            let about = match discussion
                .block_id
                .and_then(|id| blocks_list.iter().find(|block| block.id == id))
            {
                Some(block) => {
                    let text = blocks::plain_text(
                        block.block_type,
                        &serde_json::to_value(&block.props).unwrap_or(Json::Null),
                        &block.content,
                    );
                    format!("On “{}”", escape(text.lines().next().unwrap_or("").trim()))
                }
                None => "On the document as a whole".to_owned(),
            };
            let _ = write!(out, "<p class=\"when\">{about}");
            if let Some(resolved) = &discussion.resolved {
                let _ = write!(
                    out,
                    " — resolved by {} at {}",
                    escape(&resolved.by.display_name),
                    resolved.at.get().format("%Y-%m-%d %H:%M UTC")
                );
            }
            out.push_str("</p>\n");
            if !discussion.quote.is_empty() {
                let _ = writeln!(
                    out,
                    "<blockquote>{}</blockquote>",
                    escape(&discussion.quote)
                );
            }
            for comment in &discussion.comments {
                let who = escape(&comment.author.display_name);
                let when = comment.created_at.get().format("%Y-%m-%d %H:%M UTC");
                let said = if comment.deleted {
                    "<em>(deleted)</em>".to_owned()
                } else {
                    self.inline(&comment.content)
                };
                let _ = writeln!(
                    out,
                    "<p><span class=\"who\">{who}</span> <span class=\"when\">{when}</span><br>{said}</p>"
                );
            }
            out.push_str("</article>\n");
        }
        out.push_str("</section>\n");
        out
    }
}

fn percent_path(name: &str) -> String {
    name.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                char::from(byte).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

fn companion_path(directory: &str, name: &str) -> String {
    directory
        .split('/')
        .map(percent_path)
        .chain(std::iter::once(percent_path(name)))
        .collect::<Vec<_>>()
        .join("/")
}

/// Writes the live blocks of a document, under its title, as one HTML page.
#[must_use]
pub fn render(
    title: &str,
    blocks_list: &[DocumentBlock],
    lookup: &dyn Lookup,
    options: &Options<'_>,
) -> String {
    let outline = Outline::new(blocks_list);
    let page = Page {
        lookup,
        outline: &outline,
        options: *options,
    };
    let title = escape(title);
    let image_sources = if options.files_dir.is_some() {
        "data: 'self' file:"
    } else {
        "data:"
    };
    let mut out = String::new();
    let _ = write!(
        out,
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; style-src 'unsafe-inline'; img-src {image_sources}\">\n\
<title>{title}</title>\n<style>{STYLE}</style>\n</head>\n<body>\n<main>\n<h1 class=\"title\">{title}</h1>\n"
    );
    out.push_str(&page.sequence(&outline.roots()));
    if let Some(discussions) = options.discussions
        && !discussions.is_empty()
    {
        out.push_str(&page.comments(blocks_list, discussions));
    }
    out.push_str("</main>\n</body>\n</html>\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_api_contract::documents::{BlockProps, TableRow};
    use orch_types::new_id;
    use serde_json::json;
    use std::collections::HashMap;

    struct World {
        files: HashMap<Id, FileInfo>,
    }

    impl Lookup for World {
        fn username(&self, _user: Id) -> Option<String> {
            Some("ada".into())
        }

        fn name_of(&self, _kind: InlineRefKind, _id: Id) -> Option<String> {
            Some("Plans & <more>".into())
        }

        fn file(&self, id: Id) -> Option<FileInfo> {
            self.files.get(&id).cloned()
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

    fn make(
        block_type: BlockType,
        props: Json,
        content: BlockContent,
        parent: Option<Id>,
        position: u32,
    ) -> DocumentBlock {
        let checked = blocks::check(
            block_type,
            serde_json::from_value::<BlockProps>(props).ok().as_ref(),
            Some(&content),
        )
        .unwrap();
        DocumentBlock {
            id: new_id(),
            document_id: Id::nil(),
            parent_id: parent,
            position: position.to_string(),
            block_type,
            props: blocks::props_from_json(&checked.props),
            content: checked.content,
            removed: false,
            seq_created: 1,
            seq_updated: 1,
            updated_by: None,
            updated_at: "2026-10-08T17:04:05.123Z".parse().unwrap(),
        }
    }

    fn text_block(block_type: BlockType, props: Json, words: &str, position: u32) -> DocumentBlock {
        make(
            block_type,
            props,
            BlockContent::Inline(vec![plain(words)]),
            None,
            position,
        )
    }

    fn page(blocks_list: &[DocumentBlock], world: &World) -> String {
        render("Handbook", blocks_list, world, &Options::default())
    }

    fn nothing() -> World {
        World {
            files: HashMap::new(),
        }
    }

    #[test]
    fn the_page_is_one_file_with_its_style_in_it_and_no_script_and_nothing_to_fetch() {
        let html = page(
            &[text_block(BlockType::Paragraph, json!({}), "Hello", 1)],
            &nothing(),
        );
        assert!(html.starts_with("<!doctype html>"));
        assert!(html.contains("<title>Handbook</title>"));
        assert!(html.contains("<style>"));
        assert!(html.contains("<p>Hello</p>"));
        assert!(!html.contains("<script"));
        assert!(!html.contains("<link"));
        assert!(html.contains("Content-Security-Policy"));
    }

    #[test]
    fn nothing_written_in_a_block_can_put_markup_into_the_page() {
        let nasty = "<script>alert(1)</script> & \"quotes\" 'single'";
        let mut para = text_block(BlockType::Paragraph, json!({}), nasty, 1);
        para.content = BlockContent::Inline(vec![
            plain(nasty),
            Inline::Text {
                text: "x".into(),
                marks: None,
                href: Some("https://example.org/\"onmouseover=\"alert(1)".into()),
                color: None,
                background: None,
            },
            Inline::Text {
                text: "bad".into(),
                marks: None,
                href: Some("javascript:alert(1)".into()),
                color: None,
                background: None,
            },
        ]);
        let code = text_block(
            BlockType::Code,
            json!({ "language": "x\"><script>" }),
            "</pre><script>",
            2,
        );
        let html = page(&[para, code], &nothing());
        assert!(!html.contains("<script>"), "{html}");
        assert!(
            !html.contains("javascript:"),
            "an address that is none is not linked"
        );
        assert!(html.contains(
            "&lt;script&gt;alert(1)&lt;/script&gt; &amp; &quot;quotes&quot; &#39;single&#39;"
        ));
        assert!(!html.contains("onmouseover=\"alert"), "{html}");
    }

    #[test]
    fn marks_colours_links_mentions_chips_and_formulas_are_kept() {
        let id = new_id();
        let para = make(
            BlockType::Paragraph,
            json!({ "align": "center", "color": 4, "background": 3 }),
            BlockContent::Inline(vec![
                Inline::Text {
                    text: "all".into(),
                    marks: Some(vec![
                        Mark::Bold,
                        Mark::Italic,
                        Mark::Underline,
                        Mark::Strike,
                        Mark::Code,
                    ]),
                    href: None,
                    color: Some(9),
                    background: Some(2),
                },
                plain(" "),
                Inline::Text {
                    text: "link".into(),
                    marks: None,
                    href: Some("https://example.org/a".into()),
                    color: None,
                    background: None,
                },
                Inline::Mention {
                    user_id: id,
                    user: None,
                },
                Inline::Ref {
                    display: None,
                    path: None,
                    kind: InlineRefKind::Node,
                    id,
                    part: None,
                    name: None,
                    label: None,
                },
                Inline::Math {
                    source: "a<b".into(),
                },
                Inline::Break,
            ]),
            None,
            1,
        );
        let html = page(&[para], &nothing());
        for expected in [
            "<p class=\"t4 b3 align-center\">",
            "<span class=\"t9 b2\"><s><u><strong><em><code>all</code></em></strong></u></s></span>",
            "<a href=\"https://example.org/a\" rel=\"noopener noreferrer\">link</a>",
            "<span class=\"mention\">@ada</span>",
            "<span class=\"ref\">Plans &amp; &lt;more&gt;</span>",
            "<code class=\"math\">a&lt;b</code>",
            "<br></p>",
        ] {
            assert!(html.contains(expected), "{expected} is not in\n{html}");
        }
    }

    #[test]
    fn every_block_type_has_its_form() {
        let mut all: Vec<DocumentBlock> = Vec::new();
        let mut position = 0;
        let mut add = |block_type, props: Json, words: &str| {
            position += 1;
            all.push(text_block(block_type, props, words, position));
        };
        add(BlockType::Heading, json!({ "level": 2 }), "Head");
        add(BlockType::Bullet, json!({}), "bullet one");
        add(BlockType::Bullet, json!({}), "bullet two");
        add(BlockType::Numbered, json!({ "start": 4 }), "numbered");
        add(BlockType::Todo, json!({ "checked": true }), "done");
        add(BlockType::Todo, json!({ "checked": false }), "to do");
        add(BlockType::Quote, json!({}), "quoted");
        add(BlockType::Callout, json!({ "tone": "danger" }), "careful");
        add(
            BlockType::Code,
            json!({ "language": "rust", "wrap": true }),
            "let x = 1;",
        );
        add(BlockType::Toggle, json!({}), "more");
        let mut next = |block_type, props: Json| {
            position += 1;
            all.push(make(
                block_type,
                props,
                BlockContent::Inline(vec![]),
                None,
                position,
            ));
        };
        next(BlockType::Math, json!({ "source": "E=mc^2" }));
        next(BlockType::Diagram, json!({ "source": "graph TD; A-->B" }));
        next(BlockType::Divider, json!({}));
        next(BlockType::PageBreak, json!({}));
        let html = page(&all, &nothing());
        for expected in [
            "<h2>Head</h2>",
            "<ul><li>bullet one</li><li>bullet two</li></ul>",
            "<ol start=\"4\"><li>numbered</li></ol>",
            "<ul class=\"todos\"><li><input type=\"checkbox\" disabled checked> done</li><li><input type=\"checkbox\" disabled> to do</li></ul>",
            "<blockquote><p>quoted</p>",
            "<aside class=\"callout tone-danger\"><p>careful</p>",
            "<pre class=\"wrap\"><code class=\"language-rust\">let x = 1;</code></pre>",
            "<details class=\"toggle\"><summary>more</summary>",
            "<div class=\"math\"><code>E=mc^2</code></div>",
            "<code class=\"language-mermaid\">graph TD; A--&gt;B</code>",
            "<hr>",
            "<div class=\"page-break\"",
        ] {
            assert!(html.contains(expected), "{expected} is not in\n{html}");
        }
    }

    #[test]
    fn what_a_block_holds_under_it_is_inside_it_and_columns_stand_side_by_side() {
        let parent = text_block(BlockType::Bullet, json!({}), "parent", 1);
        let mut child = text_block(BlockType::Bullet, json!({}), "child", 1);
        child.parent_id = Some(parent.id);
        let columns = make(
            BlockType::Columns,
            json!({}),
            BlockContent::Inline(vec![]),
            None,
            2,
        );
        let mut left = make(
            BlockType::Column,
            json!({ "share": 2.0 }),
            BlockContent::Inline(vec![]),
            Some(columns.id),
            1,
        );
        left.parent_id = Some(columns.id);
        let mut right = make(
            BlockType::Column,
            json!({}),
            BlockContent::Inline(vec![]),
            Some(columns.id),
            2,
        );
        right.parent_id = Some(columns.id);
        let mut in_left = text_block(BlockType::Paragraph, json!({}), "left words", 1);
        in_left.parent_id = Some(left.id);
        let mut in_right = text_block(BlockType::Paragraph, json!({}), "right words", 1);
        in_right.parent_id = Some(right.id);
        let html = page(
            &[parent, child, columns, left, right, in_left, in_right],
            &nothing(),
        );
        assert!(
            html.contains("<li>parent<ul><li>child</li></ul>\n</li>"),
            "{html}"
        );
        assert!(html.contains("<div class=\"columns\"><div class=\"column\" style=\"flex-grow:2\"><p>left words</p>"), "{html}");
        assert!(
            html.contains("<div class=\"column\"><p>right words</p>"),
            "{html}"
        );
    }

    #[test]
    fn a_table_keeps_its_header_its_spans_its_widths_and_its_colours() {
        let cell = |text: &str,
                    colspan: Option<u32>,
                    rowspan: Option<u32>,
                    background: Option<u32>| TableCell {
            content: vec![plain(text)],
            colspan,
            rowspan,
            color: None,
            background,
        };
        let table = make(
            BlockType::Table,
            json!({ "header_row": true, "header_column": true, "widths": [1.0, 3.0] }),
            BlockContent::Rows(vec![
                TableRow {
                    cells: vec![cell("A", None, None, None), cell("B", None, None, Some(5))],
                },
                TableRow {
                    cells: vec![
                        cell("tall", None, Some(2), None),
                        cell("x", None, None, None),
                    ],
                },
                TableRow {
                    cells: vec![cell("y", None, None, None)],
                },
            ]),
            None,
            1,
        );
        let html = page(&[table], &nothing());
        for expected in [
            "<col style=\"width:25.00%\"><col style=\"width:75.00%\">",
            "<thead><tr><th scope=\"col\">A</th><th scope=\"col\" class=\"b5\">B</th></tr></thead>",
            "<tbody><tr><th scope=\"row\" rowspan=\"2\">tall</th><td>x</td></tr>",
            "<tr><th scope=\"row\">y</th></tr></tbody>",
        ] {
            assert!(html.contains(expected), "{expected} is not in\n{html}");
        }
    }

    #[test]
    fn an_image_is_carried_inside_the_page_and_a_film_or_a_file_is_named_where_it_stood() {
        let (png, mp4, zip) = (new_id(), new_id(), new_id());
        let mut world = World {
            files: HashMap::from([
                (
                    png,
                    FileInfo {
                        name: "pic 1.png".into(),
                        media_type: "image/png".into(),
                        size: 4,
                        is_text: false,
                        bytes: Some(vec![1, 2, 3, 4]),
                    },
                ),
                (
                    mp4,
                    FileInfo {
                        name: "clip.mp4".into(),
                        media_type: "video/mp4".into(),
                        size: 5_000_000,
                        is_text: false,
                        bytes: None,
                    },
                ),
                (
                    zip,
                    FileInfo {
                        name: "data & more.zip".into(),
                        media_type: "application/zip".into(),
                        size: 900,
                        is_text: false,
                        bytes: None,
                    },
                ),
            ]),
        };
        let image = make(
            BlockType::Image,
            json!({ "file_id": png, "width": 320.0, "align": "center" }),
            BlockContent::Inline(vec![plain("The picture")]),
            None,
            1,
        );
        let film = make(
            BlockType::Video,
            json!({ "file_id": mp4 }),
            BlockContent::Inline(vec![plain("A clip")]),
            None,
            2,
        );
        let file = make(
            BlockType::File,
            json!({ "file_id": zip }),
            BlockContent::Inline(vec![]),
            None,
            3,
        );
        let url = make(
            BlockType::Video,
            json!({ "url": "https://media.example.org/v.mp4" }),
            BlockContent::Inline(vec![]),
            None,
            4,
        );
        let all = [image, film, file, url];
        let alone = render("T", &all, &world, &Options::default());
        assert!(alone.contains("<img src=\"data:image/png;base64,AQIDBA==\" alt=\"The picture\" style=\"width:320px\">"), "{alone}");
        assert!(alone.contains("<figcaption>The picture</figcaption>"));
        assert!(
            alone.contains("Film: A clip (clip.mp4, video/mp4, 4.8 MB; not in this file)"),
            "{alone}"
        );
        assert!(alone.contains("File: data &amp; more.zip (data &amp; more.zip, application/zip, 900 bytes; not in this file)"), "{alone}");
        assert!(alone.contains("<a href=\"https://media.example.org/v.mp4\" rel=\"noopener noreferrer\">Play from media.example.org</a>"));
        let beside = render(
            "T",
            &all,
            &world,
            &Options {
                attachments_beside: true,
                ..Options::default()
            },
        );
        assert!(
            beside.contains("<a href=\"files/data%20%26%20more.zip\">data &amp; more.zip</a>"),
            "{beside}"
        );
        assert!(!alone.contains("src=\"files/"), "nothing is fetched");

        // A package keeps even a very large image outside the HTML page. No image bytes are available to the renderer.
        world.files.get_mut(&png).unwrap().bytes = None;
        let packaged = render(
            "T",
            &all,
            &world,
            &Options {
                files_dir: Some("Notes & plans.files"),
                ..Options::default()
            },
        );
        assert!(
            packaged.contains("<img src=\"Notes%20%26%20plans.files/pic%201.png\""),
            "{packaged}"
        );
        assert!(
            packaged.contains("<a href=\"Notes%20%26%20plans.files/data%20%26%20more.zip\">"),
            "{packaged}"
        );
        assert!(
            packaged.contains("<a href=\"Notes%20%26%20plans.files/clip.mp4\">"),
            "{packaged}"
        );
        assert!(!packaged.contains("data:image/png;base64,"));
        assert!(!packaged.contains("src=\"https://"));
        assert!(packaged.contains("img-src data: 'self' file:"));
    }

    #[test]
    fn an_image_whose_bytes_are_not_at_hand_is_said_to_be_missing_and_not_left_broken() {
        let png = new_id();
        let world = World {
            files: HashMap::from([(
                png,
                FileInfo {
                    name: "p.png".into(),
                    media_type: "image/png".into(),
                    size: 4,
                    is_text: false,
                    bytes: None,
                },
            )]),
        };
        let image = make(
            BlockType::Image,
            json!({ "file_id": png }),
            BlockContent::Inline(vec![]),
            None,
            1,
        );
        let html = page(&[image], &world);
        assert!(html.contains("An image that is not in this file."));
        assert!(!html.contains("<img"));
    }

    #[test]
    fn the_discussions_are_written_after_the_text_each_under_the_words_it_is_about() {
        let para = text_block(BlockType::Paragraph, json!({}), "The second paragraph", 1);
        let author = |name: &str| -> orch_api_contract::users::UserRef {
            serde_json::from_value(json!({
                "id": new_id(), "kind": "human", "username": name.to_lowercase(), "display_name": name,
                "avatar": { "text": "X", "color": 1 }
            }))
            .unwrap()
        };
        let discussion = DocumentDiscussion {
            id: new_id(),
            document_id: Id::nil(),
            block_id: Some(para.id),
            quote: "second <b>".into(),
            anchored: true,
            resolved: None,
            created_by: author("Ada"),
            created_at: "2026-10-08T17:10:00.000Z".parse().unwrap(),
            comments: vec![orch_api_contract::documents::DocumentComment {
                id: new_id(),
                discussion_id: Id::nil(),
                author: author("Ada"),
                content: vec![plain("Why <this>?")],
                reactions: vec![],
                edited_at: None,
                deleted: false,
                created_at: "2026-10-08T17:10:00.000Z".parse().unwrap(),
                seq_updated: 2,
            }],
            seq_updated: 2,
        };
        let without = page(std::slice::from_ref(&para), &nothing());
        assert!(!without.contains("Comments"));
        let with = render(
            "T",
            &[para],
            &nothing(),
            &Options {
                discussions: Some(std::slice::from_ref(&discussion)),
                ..Options::default()
            },
        );
        for expected in [
            "<section class=\"comments\"><h2>Comments</h2>",
            "On “The second paragraph”",
            "<blockquote>second &lt;b&gt;</blockquote>",
            "<span class=\"who\">Ada</span> <span class=\"when\">2026-10-08 17:10 UTC</span><br>Why &lt;this&gt;?",
        ] {
            assert!(with.contains(expected), "{expected} is not in\n{with}");
        }
    }
}
