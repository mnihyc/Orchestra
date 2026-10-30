//! One way into blocks for every kind of text a document can be made from: Orch Markdown, HTML, a Word file and plain text, each read once, with the relative paths it names resolved by whoever holds what it came with.
//!
//! A caller [`prepare`]s the bytes of a source in its [`Format`], asks which relative paths it names
//! ([`Prepared::targets`]: pictures, films, files and links to other texts), has those ready (it reads them from an
//! archive, from a workspace, from the files of the document), and [`Prepared::finish`]es with a
//! [`Directory`] that says what each path leads to. What it gets is the blocks with fresh identifiers, the
//! sentences that say what was kept as words or left out, the paths that led nowhere, and the files the source
//! carried inside it (a picture in a `data:` address, a Word file's pictures) for the caller to store as files of
//! the document. A link that leads nowhere is never a fault of the entry: it is the words it was written with.
//! Nothing here reads a file, opens an address or runs what a source holds.

use std::collections::HashMap;

use orch_api_contract::documents::{BlockContent, BlockProps, BlockType, Inline, NewBlock};
use orch_types::{Id, new_id};

use crate::import::{self, Picture};
use crate::markdown::{self, Directory, Embedded, Local, Read, Target, TargetKind};
use crate::warnings::{self, Warning};

/// What a source is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Format {
    /// Orch Markdown.
    Markdown,
    /// HTML or XHTML.
    Html,
    /// A Word document.
    Docx,
    /// Plain text.
    Text,
}

impl Format {
    /// The format that a file's name says, by its ending.
    #[must_use]
    pub fn of_name(name: &str) -> Option<Self> {
        let lower = name.rsplit('/').next().unwrap_or(name).to_ascii_lowercase();
        let ending = lower.rsplit_once('.')?.1;
        match ending {
            "md" | "markdown" | "mdown" | "mkd" => Some(Self::Markdown),
            "html" | "htm" | "xhtml" => Some(Self::Html),
            "docx" => Some(Self::Docx),
            "txt" | "text" => Some(Self::Text),
            _ => None,
        }
    }

    /// The format a name given to a tool says: `markdown`, `html`, `docx` or `text`.
    #[must_use]
    pub fn of_word(word: &str) -> Option<Self> {
        match word.trim().to_ascii_lowercase().as_str() {
            "markdown" | "md" => Some(Self::Markdown),
            "html" | "xhtml" => Some(Self::Html),
            "docx" | "word" => Some(Self::Docx),
            "text" | "txt" | "plain" => Some(Self::Text),
            _ => None,
        }
    }

    /// Whether the bytes of a source in this format are text.
    #[must_use]
    pub const fn is_text(self) -> bool {
        !matches!(self, Self::Docx)
    }
}

/// Why a source could not be read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A source that is text is not UTF-8.
    #[error("a source in this format must be UTF-8 text")]
    NotUtf8,
    /// The converter could not finish; the words say why.
    #[error("{0}")]
    Conversion(String),
}

impl From<import::Error> for Error {
    fn from(error: import::Error) -> Self {
        Self::Conversion(error.to_string())
    }
}

/// What a source is, ready to be finished.
#[derive(Debug)]
pub struct Prepared {
    kind: Kind,
}

#[derive(Debug)]
enum Kind {
    Markdown(String),
    Text(String),
    Html(import::Html),
}

/// Reads the bytes of a source in `format`.
pub fn prepare(format: Format, bytes: &[u8]) -> Result<Prepared, Error> {
    let kind = match format {
        Format::Docx => Kind::Html(import::docx(bytes)?),
        Format::Markdown | Format::Html | Format::Text => {
            let text = std::str::from_utf8(bytes).map_err(|_| Error::NotUtf8)?;
            let text = text.strip_prefix('\u{feff}').unwrap_or(text).to_owned();
            match format {
                Format::Markdown => Kind::Markdown(text),
                Format::Text => Kind::Text(text),
                _ => Kind::Html(import::html(&text)?),
            }
        }
    };
    Ok(Prepared { kind })
}

/// The directory a path is relative to, joined with it: `docs/a` and `../img/x.png` are `img/x.png`. `None` where
/// the path leads out of the root.
#[must_use]
pub fn join_path(base: &str, relative: &str) -> Option<String> {
    let mut parts: Vec<&str> = base
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    for part in relative.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

impl Prepared {
    /// The relative paths the source names, each once, in the order they stand. Pictures that the source carries
    /// inside it and addresses on the web are not among them.
    #[must_use]
    pub fn targets(&self) -> Vec<Target> {
        match &self.kind {
            Kind::Markdown(text) => markdown::local_targets(text),
            Kind::Text(_) => Vec::new(),
            Kind::Html(html) => {
                let mut out: Vec<Target> = Vec::new();
                for picture in &html.pictures {
                    if let Some(path) = markdown::relative_path(&picture.src)
                        && !out.iter().any(|each| each.path == path)
                    {
                        out.push(Target {
                            path,
                            kind: TargetKind::Image,
                        });
                    }
                }
                for block in &html.blocks {
                    collect_links(block, &mut out);
                }
                out
            }
        }
    }

    /// The names that `@` stands before in the source, in lower case, for the caller to look up in its directory
    /// of people before it finishes.
    #[must_use]
    pub fn mentioned_names(&self) -> Vec<String> {
        match &self.kind {
            Kind::Markdown(text) => markdown::mentioned_names(text),
            _ => Vec::new(),
        }
    }

    /// Makes the blocks, with `directory` saying what each name and path leads to.
    #[must_use]
    pub fn finish(self, directory: &dyn Directory) -> Read {
        match self.kind {
            Kind::Markdown(text) => markdown::read_checked(&text, directory),
            Kind::Text(text) => Read {
                blocks: plain_text(&text),
                ..Read::default()
            },
            Kind::Html(html) => finish_html(html, directory),
        }
    }
}

fn collect_links(block: &NewBlock, out: &mut Vec<Target>) {
    let runs: Vec<&Inline> = match &block.content {
        Some(BlockContent::Inline(runs)) => runs.iter().collect(),
        Some(BlockContent::Rows(rows)) => rows
            .iter()
            .flat_map(|row| row.cells.iter().flat_map(|cell| cell.content.iter()))
            .collect(),
        None => Vec::new(),
    };
    for run in runs {
        if let Inline::Text {
            href: Some(href), ..
        } = run
            && !crate::blocks::allowed_href(href)
            && let Some(path) = markdown::relative_path(href)
            && !out.iter().any(|each| each.path == path)
        {
            out.push(Target {
                path,
                kind: TargetKind::Link,
            });
        }
    }
    for child in block.children.iter().flatten() {
        collect_links(child, out);
    }
}

/// Plain text: paragraphs between blank lines, a line break where a line ends inside one, and nothing in it read as
/// Markdown.
fn plain_text(text: &str) -> Vec<NewBlock> {
    let mut blocks = Vec::new();
    let mut lines: Vec<&str> = Vec::new();
    let mut flush = |lines: &mut Vec<&str>| {
        if lines.is_empty() {
            return;
        }
        let mut runs: Vec<Inline> = Vec::new();
        for (index, line) in lines.iter().enumerate() {
            if index > 0 {
                runs.push(Inline::Break);
            }
            runs.push(Inline::Text {
                text: (*line).trim().to_owned(),
                marks: None,
                href: None,
                color: None,
                background: None,
            });
        }
        blocks.push(NewBlock {
            id: new_id(),
            block_type: BlockType::Paragraph,
            props: None,
            content: Some(BlockContent::Inline(runs)),
            children: None,
        });
        lines.clear();
    };
    for line in text.lines() {
        if line.trim().is_empty() {
            flush(&mut lines);
        } else {
            lines.push(line);
        }
    }
    flush(&mut lines);
    blocks
}

/// The blocks of an HTML source, with what its pictures and its links lead to.
fn finish_html(html: import::Html, directory: &dyn Directory) -> Read {
    let mut read = Read {
        blocks: html.blocks,
        warnings: html
            .warnings
            .iter()
            .map(|each| Warning::from_script(each))
            .collect(),
        ..Read::default()
    };
    let pictures: HashMap<Id, Picture> = html
        .pictures
        .into_iter()
        .map(|picture| (picture.id, picture))
        .collect();
    let mut blocks = std::mem::take(&mut read.blocks);
    settle_pictures(&mut blocks, &pictures, directory, &mut read);
    let (warnings, missing) = markdown::settle_blocks(&mut blocks, directory);
    for warning in warnings {
        warnings::note(&mut read.warnings, warning);
    }
    for path in missing {
        if read.missing.len() < warnings::LISTED_MAX && !read.missing.contains(&path) {
            read.missing.push(path);
        }
    }
    read.blocks = blocks;
    read
}

/// Makes each image block of `blocks` show a file, or — where its picture cannot be a file — the words it says.
fn settle_pictures(
    blocks: &mut [NewBlock],
    pictures: &HashMap<Id, Picture>,
    directory: &dyn Directory,
    read: &mut Read,
) {
    for block in blocks.iter_mut() {
        if let Some(children) = block.children.as_mut() {
            settle_pictures(children, pictures, directory, read);
        }
        if block.block_type != BlockType::Image {
            continue;
        }
        let Some(BlockProps::Image(props)) = block.props.as_mut() else {
            continue;
        };
        let Some(picture) = pictures.get(&props.file_id) else {
            continue;
        };
        if let Some((media_type, bytes)) = markdown::data_image(&picture.src) {
            let id = new_id();
            let name = format!(
                "image-{}.{}",
                read.embedded.len() + 1,
                markdown::image_extension(&media_type)
            );
            read.embedded.push(Embedded {
                id,
                name,
                media_type,
                bytes,
            });
            props.file_id = id;
            continue;
        }
        if let Some(path) = markdown::relative_path(&picture.src) {
            if let Some(Local::File(file)) = directory.local(&path) {
                props.file_id = file.id;
                continue;
            }
            warnings::note(
                &mut read.warnings,
                Warning::new("reader.image_missing", &[("path", &path)]),
            );
            if read.missing.len() < warnings::LISTED_MAX && !read.missing.contains(&path) {
                read.missing.push(path);
            }
        } else if picture.src.starts_with("data:") {
            warnings::note(&mut read.warnings, Warning::new("reader.image_kind", &[]));
        } else if crate::blocks::allowed_href(&picture.src) {
            warnings::note(&mut read.warnings, Warning::new("reader.image_remote", &[]));
        }
        // The picture cannot be shown: the block becomes the words that described it.
        let words = if picture.alt.trim().is_empty() {
            let name = picture.src.rsplit('/').next().unwrap_or("image");
            if picture.src.starts_with("data:") {
                "image".to_owned()
            } else {
                name.to_owned()
            }
        } else {
            picture.alt.clone()
        };
        let href = (crate::blocks::allowed_href(&picture.src) && !picture.src.starts_with("data:"))
            .then(|| picture.src.clone());
        block.block_type = BlockType::Paragraph;
        block.props = None;
        block.content = Some(BlockContent::Inline(vec![Inline::Text {
            text: words,
            marks: None,
            href,
            color: None,
            background: None,
        }]));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_format_is_known_by_the_ending_of_a_name() {
        assert_eq!(Format::of_name("docs/Plan.MD"), Some(Format::Markdown));
        assert_eq!(Format::of_name("a.xhtml"), Some(Format::Html));
        assert_eq!(Format::of_name("report.docx"), Some(Format::Docx));
        assert_eq!(Format::of_name("notes.txt"), Some(Format::Text));
        assert_eq!(Format::of_name("data.csv"), None);
        assert_eq!(Format::of_word("Word"), Some(Format::Docx));
    }

    #[test]
    fn a_relative_path_is_joined_to_the_directory_it_is_relative_to() {
        assert_eq!(
            join_path("docs/a", "../img/x.png").as_deref(),
            Some("docs/img/x.png")
        );
        assert_eq!(join_path("", "./a.md").as_deref(), Some("a.md"));
        assert_eq!(
            join_path("docs", "../../x"),
            None,
            "it leads out of the root"
        );
    }

    #[test]
    fn plain_text_is_paragraphs_and_nothing_in_it_is_markdown() {
        let blocks = plain_text("# not a heading\nsecond line\n\n\n  - not a list\n");
        assert_eq!(blocks.len(), 2);
        assert!(
            blocks
                .iter()
                .all(|block| block.block_type == BlockType::Paragraph)
        );
        let Some(BlockContent::Inline(runs)) = &blocks[0].content else {
            panic!("text");
        };
        assert_eq!(
            crate::blocks::inline_text(runs),
            "# not a heading\nsecond line"
        );
    }
}
