//! What a plain file names beside itself: the pictures it shows and the other texts it links to, found in the source it came with and told apart into the files a document will hold and the documents the links will lead to.
//!
//! A text of plain files writes paths relative to where it lies (`images/a.png`, `../Plan.md`). Whoever reads it resolves them against
//! the entries of the source: a path that leads to another text of the source is a link to the document that text becomes, a path that
//! leads to anything else is a file of this document, and a path that leads nowhere is the words it was written with.

use super::identity;
use super::model::{Opened, Plain, PlainFormat};
use orch_documents::convert::{self, Format, Prepared};
use orch_documents::markdown::TargetKind;
use orch_types::Id;

/// The most bytes of one text an import reads into memory: sixty-four mebibytes, as for a record of a package.
pub const TEXT_MAX_BYTES: u64 = 64 * 1024 * 1024;

/// What a text of the source is written in, as the converter of documents names it.
#[must_use]
pub const fn format_of(format: PlainFormat) -> Format {
    match format {
        PlainFormat::Markdown => Format::Markdown,
        PlainFormat::Html => Format::Html,
        PlainFormat::Text => Format::Text,
        PlainFormat::Word => Format::Docx,
    }
}

/// A path a text names that leads somewhere in the source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Named {
    /// The path as the text wrote it, which is what the converter asks about.
    pub written: String,
    /// The entry of the source it leads to.
    pub entry: String,
    /// The object of the source that is the document the entry becomes, where it is a text the import reads.
    pub document: Option<Id>,
    /// What the text uses it for.
    pub kind: TargetKind,
}

/// The paths a prepared text names that lead to an entry of the source, in the order the text names them.
#[must_use]
pub fn resolve(opened: &Opened, entry: &str, prepared: &Prepared) -> Vec<Named> {
    let base = entry
        .rsplit_once('/')
        .map_or("", |(directory, _)| directory);
    prepared
        .targets()
        .into_iter()
        .filter_map(|target| {
            let joined = convert::join_path(base, &target.path)?;
            let source = identity::path(Id::nil(), &joined);
            if opened.plain.contains_key(&source) {
                Some(Named {
                    written: target.path,
                    entry: joined,
                    document: Some(source),
                    kind: target.kind,
                })
            } else if opened.entry_names.contains(&joined) {
                Some(Named {
                    written: target.path,
                    entry: joined,
                    document: None,
                    kind: target.kind,
                })
            } else {
                None
            }
        })
        .collect()
}

/// What the text of a plain file names beside itself, read from the source: nothing where it cannot be read, which the reader
/// of the document says again when the document is made.
pub async fn named(opened: &Opened, plain: &Plain) -> Vec<Named> {
    let Ok(bytes) = opened.entries.read(&plain.entry, TEXT_MAX_BYTES).await else {
        return Vec::new();
    };
    match convert::prepare(format_of(plain.format), &bytes) {
        Ok(prepared) => resolve(opened, &plain.entry, &prepared),
        Err(_) => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_kind_of_text_is_read_by_the_converter_that_knows_it() {
        assert_eq!(format_of(PlainFormat::Word), Format::Docx);
        assert_eq!(format_of(PlainFormat::Text), Format::Text);
        assert_eq!(format_of(PlainFormat::Html), Format::Html);
        assert_eq!(format_of(PlainFormat::Markdown), Format::Markdown);
    }
}
