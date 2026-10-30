//! The shared inert HTML converter and fresh block identities for ordinary document creation.

use orch_api_contract::documents::{BlockContent, BlockProps, BlockType, NewBlock};
use orch_types::{Id, new_id};
use rquickjs::context::EvalOptions;
use rquickjs::{ArrayBuffer, Context, Function, Object, Runtime, Value};
use serde::Deserialize;
use std::time::{Duration, Instant};

const CONVERTER: &str = include_str!("../../../../shared/document-html.js");
// The sole module export is removed for the native script context; its implementation is unchanged.
const EXPORT: &str = "\nexport { readHtml };\n";
// Linear conversion has room for strings, inert nodes and the JSON projection, while each import
// remains bounded even where a deployment permits unusually large source files.
const BYTES_PER_SOURCE_BYTE: usize = 64;
const MEMORY_MIN: usize = 16 * 1024 * 1024;
const MEMORY_MAX: usize = 256 * 1024 * 1024;
// Conversion is synchronous CPU work, not I/O; an entry cannot monopolize a worker indefinitely.
const CONVERSION_TIME: Duration = Duration::from_secs(2);
// A Word file is a zip of XML that the converter unpacks and reads whole: far more work for the same bytes than HTML is.
const DOCX_TIME: Duration = Duration::from_secs(20);
// The converter and the unpacking library alone take some fifteen megabytes of a sandbox before a byte of the file is read.
const DOCX_MEMORY_MIN: usize = 64 * 1024 * 1024;
// What a Word file holds may be many times its size once unpacked: pictures, XML and the HTML made of them.
const DOCX_BYTES_PER_SOURCE_BYTE: usize = 128;
// Mammoth's own scheduling asks the host for timers; the host runs what was scheduled this many times at most
// for one conversion, which no document comes near and no loop of a hostile one gets past (its time is bounded too).
const DOCX_SCHEDULED_MAX: usize = 5_000_000;
// The converter of Word files, the vendored mammoth and its small adapter, run in a sandbox that has none of
// the page a browser would give them but timers and a UTF-8 reader, which the shim stands in for.
const DOCX_SHIM: &str = include_str!("../../../../shared/document-docx-shim.js");
const MAMMOTH: &str = include_str!("../../../../shared/vendor/mammoth.browser.min.js");
const DOCX_ADAPTER: &str = include_str!("../../../../shared/document-docx.js");
// The converter's explicitly bounded nesting fits this native stack allowance.
const STACK_BYTES: usize = 1024 * 1024;

/// Why a source could not become ordinary document blocks.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The bounded conversion could not finish.
    #[error("HTML could not be converted: {0}")]
    Conversion(String),
    /// The converter did not produce the document's actual wire types.
    #[error("converted HTML is not valid document content: {0}")]
    Content(#[from] serde_json::Error),
}

#[derive(Debug, Deserialize)]
struct Piece {
    #[serde(rename = "type")]
    block_type: BlockType,
    props: BlockProps,
    content: BlockContent,
    children: Vec<Piece>,
}

impl Piece {
    fn fresh(self) -> NewBlock {
        NewBlock {
            id: new_id(),
            block_type: self.block_type,
            props: Some(self.props),
            content: Some(self.content),
            children: Some(self.children.into_iter().map(Self::fresh).collect()),
        }
    }
}

#[derive(Debug, Deserialize)]
struct Converted {
    pieces: Vec<Piece>,
    warnings: Vec<String>,
    #[serde(default)]
    images: Vec<Image>,
}

#[derive(Debug, Deserialize)]
struct Image {
    id: Id,
    src: String,
    #[serde(default)]
    alt: String,
}

/// A picture the HTML names: the block that shows it carries `id` in place of a file until the caller says what
/// the address leads to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picture {
    /// What the block that shows it names it by, for now.
    pub id: Id,
    /// Its address as written: `data:`, a path relative to the page, or an address on the web.
    pub src: String,
    /// What it says it is.
    pub alt: String,
}

/// HTML as normal fresh blocks, with every conversion loss retained for the person importing it.
#[derive(Debug)]
pub struct Html {
    /// The ordinary creation blocks, with fresh identifiers at every depth.
    pub blocks: Vec<NewBlock>,
    /// Human-readable losses, once each.
    pub warnings: Vec<String>,
    /// The pictures its image blocks name, to be settled by the caller.
    pub pictures: Vec<Picture>,
}

/// Runs the exact converter used by browser paste, with source passed as a value rather than code.
pub fn html(source: &str) -> Result<Html, Error> {
    let fail = |error: rquickjs::Error| Error::Conversion(error.to_string());
    let runtime = Runtime::new().map_err(fail)?;
    runtime.set_memory_limit(
        source
            .len()
            .saturating_mul(BYTES_PER_SOURCE_BYTE)
            .clamp(MEMORY_MIN, MEMORY_MAX),
    );
    runtime.set_max_stack_size(STACK_BYTES);
    let started = Instant::now();
    runtime.set_interrupt_handler(Some(Box::new(move || started.elapsed() >= CONVERSION_TIME)));
    let context = Context::full(&runtime).map_err(fail)?;
    let script = CONVERTER
        .strip_suffix(EXPORT)
        .ok_or_else(|| Error::Conversion("the shared converter has no module export".to_owned()))?;
    let json = context
        .with(|ctx| -> Result<String, rquickjs::Error> {
            ctx.eval::<(), _>(script.as_bytes())?;
            let convert: Function<'_> = ctx.globals().get("readHtml")?;
            let value: Value<'_> = convert.call((source,))?;
            let json: Object<'_> = ctx.globals().get("JSON")?;
            let stringify: Function<'_> = json.get("stringify")?;
            stringify.call((value,))
        })
        .map_err(fail)?;
    let converted: Converted = serde_json::from_str(&json)?;
    Ok(Html {
        blocks: converted.pieces.into_iter().map(Piece::fresh).collect(),
        warnings: converted.warnings,
        pictures: converted
            .images
            .into_iter()
            .map(|image| Picture {
                id: image.id,
                src: image.src,
                alt: image.alt,
            })
            .collect(),
    })
}

#[derive(Debug, Deserialize)]
struct Word {
    done: bool,
    html: String,
    messages: usize,
    error: Option<String>,
}

/// A Word document as blocks: converted to HTML by mammoth in a sandbox of the same kind as the HTML converter's,
/// then read as HTML, so that there is one reader of what a page says and a Word file costs no other.
pub fn docx(bytes: &[u8]) -> Result<Html, Error> {
    let fail = |error: rquickjs::Error| Error::Conversion(error.to_string());
    let runtime = Runtime::new().map_err(fail)?;
    runtime.set_memory_limit(
        bytes
            .len()
            .saturating_mul(DOCX_BYTES_PER_SOURCE_BYTE)
            .clamp(DOCX_MEMORY_MIN, MEMORY_MAX),
    );
    runtime.set_max_stack_size(STACK_BYTES);
    let started = Instant::now();
    runtime.set_interrupt_handler(Some(Box::new(move || started.elapsed() >= DOCX_TIME)));
    let context = Context::full(&runtime).map_err(fail)?;
    let json = context
        .with(|ctx| -> Result<String, rquickjs::Error> {
            // The bundle is a classic script: it is not read in strict mode, which it was not written for.
            let sloppy = || {
                let mut options = EvalOptions::default();
                options.strict = false;
                options
            };
            ctx.eval_with_options::<(), _>(DOCX_SHIM, sloppy())?;
            ctx.eval_with_options::<(), _>(MAMMOTH, sloppy())?;
            ctx.eval_with_options::<(), _>(DOCX_ADAPTER, sloppy())?;
            let buffer = ArrayBuffer::new(ctx.clone(), bytes.to_vec())?;
            ctx.globals().set("__docx", buffer)?;
            ctx.eval::<(), _>("__start();")?;
            let drain: Function<'_> = ctx.globals().get("__drain")?;
            let mut ran = 0_usize;
            loop {
                let jobs = ctx.execute_pending_job();
                let scheduled: usize = drain.call((1_000_usize,))?;
                ran = ran.saturating_add(scheduled);
                let done: bool = ctx.eval("__state.done")?;
                if done || (!jobs && scheduled == 0) || ran > DOCX_SCHEDULED_MAX {
                    break;
                }
            }
            ctx.eval::<String, _>("JSON.stringify(__state)")
        })
        .map_err(fail)?;
    let word: Word = serde_json::from_str(&json)?;
    if let Some(error) = word.error {
        return Err(Error::Conversion(format!(
            "the Word document could not be read: {error}"
        )));
    }
    if !word.done {
        return Err(Error::Conversion(
            "the Word document could not be read to its end".to_owned(),
        ));
    }
    let mut converted = html(&word.html)?;
    if word.messages > 0 {
        let sentence = crate::warnings::Warning::new("reader.word_styles", &[]).message;
        // The converter's own sentences end with a full stop; the code's table has none.
        let sentence = format!("{sentence}.");
        if !converted.warnings.iter().any(|each| each == &sentence) {
            converted.warnings.push(sentence);
        }
    }
    Ok(converted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn imported_html_preserves_nested_lists_table_headers_and_safe_text_without_active_markup() {
        let converted = html("<h1>Notes</h1><ul><li>Outer<ul><li>Inner</li></ul></li></ul><table><tr><th>Name</th></tr><tr><td>Ada</td></tr></table><script>throw Error('never run')</script><p><a href='javascript:alert(1)'>Keep text</a></p>").unwrap();
        assert_eq!(
            converted.blocks[1].children.as_ref().unwrap()[0].block_type,
            BlockType::Bullet
        );
        let table = serde_json::to_value(&converted.blocks[2]).unwrap();
        assert_eq!(table["props"]["header_row"], true);
        assert_eq!(
            table["content"][0]["cells"][0]["content"][0]["text"],
            json!("Name")
        );
        assert_eq!(converted.warnings.len(), 2);
        let first = html("<p>Same</p>").unwrap();
        let second = html("<p>Same</p>").unwrap();
        assert_ne!(first.blocks[0].id, second.blocks[0].id);
    }
}
