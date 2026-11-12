//! Text turned into blocks and stored nowhere: the one reader that a paste goes through, and the files that a pasted page carries inside it.
//!
//! The browser sends what was pasted and the document it is for; the server reads it with the same reader
//! that an import and an agent's file go through (`orch_documents::convert`), so that what is read from a
//! paste, an import and an agent's text agree. With the document named, its files and its people are known to the
//! reader, and the pictures that the text carries in its own address (a page pasted from a word processor, a
//! Markdown file with a `data:` picture) are stored as files of that document, within its limits; without one
//! they are left out and the answer says so. Nothing in the answer is stored but those files.

use std::cell::RefCell;

use std::collections::BTreeSet;

use std::collections::HashSet;

use super::*;

use orch_api_contract::documents::DocumentConvert;

use orch_blob_contract::range::BlobMeta;

use orch_db::queries::documents::HeldFile;

use orch_documents::markdown::Embedded;

use orch_documents::markdown::FileRef;

use orch_types::Hash;
/// Bind both conversion intents to their one initial and native effect handoff.
pub(super) fn mount(registry: &mut Registry) {
    registry.on_with_initial_operation(
        DocumentConvert,
        crate::key_operations::native::effects::initial::<operation::Conversions>,
        crate::key_operations::native::effects::prepare::<operation::Conversions>,
        operation::convert_text,
    );
}

/// What the reader is told of the document it reads for: its people, and its files by their names.
pub(super) struct Held {
    pub(super) people: HashMap<String, Id>,
    pub(super) files: Vec<(String, FileRef)>,
    used_files: RefCell<BTreeSet<String>>,
    used_people: RefCell<BTreeSet<String>>,
}

impl Directory for Held {
    fn user_id(&self, username: &str) -> Option<Id> {
        let username = username.to_ascii_lowercase();
        self.used_people.borrow_mut().insert(username.clone());
        self.people.get(&username).copied()
    }

    fn file(&self, name: &str) -> Option<markdown::FileRef> {
        self.used_files.borrow_mut().insert(name.to_owned());
        self.files
            .iter()
            .find(|(each, _)| each == name)
            .map(|(_, file)| file.clone())
    }
    fn local(&self, path: &str) -> Option<markdown::Local> {
        self.file(path).map(markdown::Local::File)
    }
}

/// Puts the bytes of files that a source carried into the store, and says what each is as a file a document
/// holds. The names are made unique among `taken`. Nothing is recorded in the database yet.
pub(super) async fn hold(
    ctx: &Ctx,
    embedded: &[Embedded],
    taken: &mut HashSet<String>,
    by: Id,
    at: DateTime<Utc>,
) -> ApiResult<Vec<HeldFile>> {
    let mut held = Vec::with_capacity(embedded.len());
    for each in embedded {
        let meta = ctx
            .app
            .blobs
            .put_bytes(bytes::Bytes::from(each.bytes.clone()))
            .await
            .map_err(ApiError::internal)?;
        held.push(HeldFile {
            file: rows::FileRow {
                id: each.id,
                name: orch_documents::files::unique_name(taken, &each.name),
                media_type: each.media_type.clone(),
                size: meta.size,
                is_text: false,
                added_by: Some(by),
                added_by_thread: None,
                added_at: at,
            },
            blob: meta.hash,
        });
    }
    Ok(held)
}

/// The native creation producer already consumes its header first-effect cut before installing files.
pub(super) async fn register(
    ctx: &Ctx,
    tx: &mut orch_db::Tx,
    held: &[HeldFile],
    at: DateTime<Utc>,
) -> ApiResult<()> {
    register_checked(ctx, tx, held, at, || Ok(())).await
}

/// Registers the already supplied blobs after their real serialization wait and first-effect cut.
pub(super) async fn register_checked<F>(
    ctx: &Ctx,
    tx: &mut orch_db::Tx,
    held: &[HeldFile],
    at: DateTime<Utc>,
    before: F,
) -> ApiResult<()>
where
    F: FnOnce() -> ApiResult<()>,
{
    let hashes: Vec<Hash> = held.iter().map(|file| file.blob).collect();
    orch_db::queries::uploads::lock_blobs(tx, &hashes).await?;
    if held.is_empty() {
        return Ok(());
    }
    before()?;
    for file in held {
        ctx.app
            .blobs
            .register(
                tx,
                BlobMeta {
                    hash: file.blob,
                    size: file.file.size,
                },
                at,
            )
            .await?;
    }
    Ok(())
}

/// The identifiers of the files that blocks name: the pictures, films, sounds and files they show.
pub(super) fn named_files(blocks: &[NewBlock]) -> HashSet<Id> {
    use orch_api_contract::documents::BlockProps;
    let mut out = HashSet::new();
    for block in blocks {
        match &block.props {
            Some(BlockProps::Image(props)) => {
                out.insert(props.file_id);
            }
            Some(BlockProps::File(props)) => {
                out.insert(props.file_id);
            }
            Some(BlockProps::Media(props)) => out.extend(props.file_id),
            _ => {}
        }
        if let Some(children) = &block.children {
            out.extend(named_files(children));
        }
    }
    out
}

/// The blocks of a text that name files which will not be held: an image block becomes the words that said it, any
/// other block that names one is taken out.
fn without_files(blocks: &mut [NewBlock], gone: &HashSet<Id>) {
    use orch_api_contract::documents::{BlockContent, BlockProps, BlockType, Inline};
    for block in blocks.iter_mut() {
        if let Some(children) = block.children.as_mut() {
            without_files(children, gone);
        }
        let names = match &block.props {
            Some(BlockProps::Image(props)) => Some(props.file_id),
            Some(BlockProps::File(props)) => Some(props.file_id),
            Some(BlockProps::Media(props)) => props.file_id,
            _ => None,
        };
        if names.is_some_and(|id| gone.contains(&id)) {
            let caption = match &block.content {
                Some(BlockContent::Inline(runs)) => runs.clone(),
                _ => Vec::new(),
            };
            block.block_type = BlockType::Paragraph;
            block.props = None;
            block.content = Some(BlockContent::Inline(if caption.is_empty() {
                vec![Inline::Text {
                    text: "image".to_owned(),
                    marks: None,
                    href: None,
                    color: None,
                    background: None,
                }]
            } else {
                caption
            }));
        }
    }
}

#[path = "convert/operation.rs"]
pub mod operation;

#[path = "convert/refusal.rs"]
pub mod refusal;
