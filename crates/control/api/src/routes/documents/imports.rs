//! Bounded Markdown, HTML, Word and plain-text imports with per-entry guarded creation and durable document identities.
//!
//! Every entry is read by the one reader (`orch_documents::convert`): a link to another entry of the same import
//! is a reference to the document that entry becomes, a link or a picture that leads nowhere is the words it was
//! written with and a sentence in the entry's report, and the pictures a Word file or a page carries inside it become
//! files of the document made.

use super::*;
use orch_api_contract::documents::{
    BlockType, DocumentImportRequest, DocumentImportRequestFileFormat as Format,
    DocumentImportResultEntry, DocumentImportResultError, InlineRefKind, NodeDocumentImport,
};
use orch_blob_contract::range::BlobMeta;
use orch_documents::convert;
use orch_documents::markdown::{FileRef, Local};
use orch_documents::references::Typed;
use orch_types::Hash;

mod operation;

// Import uses the published 200-entry list bound, independently of larger archive-browser pages.
const ENTRIES_MAX: usize = orch_api_contract::common::MAX_LIMIT as usize;
// This is the registry's shipped expanded-source allowance when no override is stored.
const SHIPPED_EXPANDED_BYTES: i64 = 64 * 1024 * 1024;

pub(super) fn mount(registry: &mut Registry) {
    registry.on_with_initial_operation(
        NodeDocumentImport,
        crate::key_operations::native::effects::initial::<operation::Imports>,
        crate::key_operations::native::effects::prepare::<operation::Imports>,
        operation::import,
    );
}

struct Input {
    name: String,
    source: Hash,
    format: Format,
    bytes: Vec<u8>,
    error: Option<ApiError>,
    /// A file of the archive that is no text but may be shown by one (a picture, a film, a sheet).
    companion: bool,
}

fn refused(name: String, error: ApiError) -> DocumentImportResultEntry {
    DocumentImportResultEntry {
        name,
        document: None,
        warnings: Vec::new(),
        error: Some(DocumentImportResultError {
            code: error.code().as_str().to_owned(),
            message: error.problem.detail.clone(),
        }),
    }
}

async fn allowance(conn: &mut Conn, node: Id) -> ApiResult<i64> {
    orch_settings::get::<i64>(
        conn,
        orch_settings::Place::Node(node),
        orch_settings_contract::registry::keys::SHARING_SNAPSHOT_MAX_BYTES,
    )
    .await
    .map_err(ApiError::internal)
    .map(|value| value.unwrap_or(SHIPPED_EXPANDED_BYTES))
}

fn archive_format(name: &str) -> Format {
    match convert::Format::of_name(name) {
        Some(convert::Format::Html) => Format::Html,
        Some(convert::Format::Docx) => Format::Docx,
        Some(convert::Format::Text) => Format::Text,
        _ => Format::Markdown,
    }
}

/// The reader that a source of this format is read by; none for an archive, which is no document.
fn reader_format(format: Format) -> Option<convert::Format> {
    match format {
        Format::Markdown => Some(convert::Format::Markdown),
        Format::Html => Some(convert::Format::Html),
        Format::Docx => Some(convert::Format::Docx),
        Format::Text => Some(convert::Format::Text),
        Format::Archive => None,
    }
}

/// A file that came with the texts of an archive: what a picture or a file named by a text is read from.
struct Companion {
    bytes: Vec<u8>,
    media_type: String,
}

/// What the reader is told of an import: whom `@` names stand for, which document each other entry of the same
/// import becomes, so that a link between two of them is a reference, and which files came along, so that a picture
/// that a text names is found.
struct Entries<'a> {
    people: HashMap<String, Id>,
    /// The directory of the entry being read, in the archive.
    base: &'a str,
    /// The path of each entry that will be a document, and its identifier.
    siblings: &'a HashMap<String, Id>,
    /// The files that came with them, by their path in the archive.
    companions: &'a HashMap<String, Companion>,
    /// The identifier each companion asked about was given in this document, by its path.
    asked: std::sync::Mutex<HashMap<String, Id>>,
}

impl Entries<'_> {
    /// The companions that blocks name, as files the document is to hold.
    fn named_files(self, blocks: &[NewBlock]) -> Vec<orch_documents::markdown::Embedded> {
        let named = super::convert::named_files(blocks);
        let asked = self.asked.into_inner().unwrap_or_default();
        let mut out: Vec<_> = asked
            .into_iter()
            .filter(|(_, id)| named.contains(id))
            .filter_map(|(path, id)| {
                let found = self.companions.get(&path)?;
                Some(orch_documents::markdown::Embedded {
                    id,
                    name: path.rsplit('/').next().unwrap_or(&path).to_owned(),
                    media_type: found.media_type.clone(),
                    bytes: found.bytes.clone(),
                })
            })
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
        out
    }
}

impl Directory for Entries<'_> {
    fn user_id(&self, username: &str) -> Option<Id> {
        self.people.get(&username.to_ascii_lowercase()).copied()
    }

    fn local(&self, path: &str) -> Option<Local> {
        let joined = convert::join_path(self.base, path)?;
        if let Some(id) = self.siblings.get(&joined) {
            return Some(Local::Document(Typed {
                kind: InlineRefKind::Document,
                id: *id,
                path: None,
                part: None,
            }));
        }
        let found = self.companions.get(&joined)?;
        let mut asked = self.asked.lock().ok()?;
        let id = *asked.entry(joined).or_insert_with(orch_types::new_id);
        Some(Local::File(FileRef {
            id,
            media_type: found.media_type.clone(),
        }))
    }
}

async fn inputs(
    ctx: &Ctx,
    body: &DocumentImportRequest,
    limit: i64,
    origin: &orch_authz::CapturedOperation,
    sources: &operation::Sources,
    source_errors: &[Option<DocumentImportResultError>],
) -> ApiResult<Vec<Input>> {
    let mut out = Vec::new();
    let mut used = 0_i64;
    let mut names = std::collections::HashSet::new();
    for (index, file) in body.files.iter().enumerate() {
        if let Some(error) = &source_errors[index] {
            out.push(Input {
                name: file.name.clone(),
                source: file.blob,
                format: file.format,
                bytes: Vec::new(),
                error: Some(ApiError::new(
                    serde_json::from_value(json!(error.code)).map_err(ApiError::internal)?,
                    &error.message,
                )),
                companion: false,
            });
            continue;
        }
        let source = sources.for_hash(file.blob);
        let prepared = async {
            let name = crate::archives::import_path(&file.name)
                .map_err(|error| invalid("files", error))?;
            if !names.insert(name.clone()) {
                return Err(invalid("files", "duplicate source file name"));
            }
            operation::check_inputs(ctx, origin, &source).await?;
            let size = ctx
                .app
                .blobs
                .store()
                .size(&file.blob)
                .await
                .map_err(ApiError::internal)?
                .ok_or_else(ApiError::not_found)?;
            operation::check_inputs(ctx, origin, &source).await?;
            if file.format != Format::Archive && size > limit.saturating_sub(used) {
                return Err(invalid(
                    "files",
                    "the selected sources exceed the expanded import byte allowance",
                ));
            }
            if file.format == Format::Archive {
                let archived = crate::archives::documents(
                    &ctx.app.blobs,
                    BlobMeta {
                        hash: file.blob,
                        size,
                    },
                    ENTRIES_MAX,
                    u64::try_from(limit).map_err(ApiError::internal)?,
                )
                .await
                .map_err(|error| invalid("files", &error.to_string()))?;
                operation::check_inputs(ctx, origin, &source).await?;
                let sources = archived
                    .into_iter()
                    .map(|source| {
                        let format = archive_format(&source.name);
                        Input {
                            name: source.name,
                            source: file.blob,
                            format,
                            bytes: source.bytes.unwrap_or_default(),
                            error: source.error.map(|error| invalid("files", &error)),
                            companion: source.companion,
                        }
                    })
                    .collect();
                return Ok(sources);
            }
            let bytes = ctx
                .app
                .blobs
                .read_whole(&file.blob, limit.saturating_sub(used))
                .await
                .map_err(ApiError::internal)?;
            operation::check_inputs(ctx, origin, &source).await?;
            used = used.saturating_add(i64::try_from(bytes.len()).unwrap_or(i64::MAX));
            Ok(vec![Input {
                name,
                source: file.blob,
                format: file.format,
                bytes: bytes.to_vec(),
                error: None,
                companion: false,
            }])
        }
        .await;
        match prepared {
            Ok(mut entries) => out.append(&mut entries),
            Err(error) => out.push(Input {
                name: file.name.clone(),
                source: file.blob,
                format: file.format,
                bytes: Vec::new(),
                error: Some(error),
                companion: false,
            }),
        }
    }
    Ok(out)
}

fn title(name: &str, blocks: &mut Vec<NewBlock>, warnings: &mut Vec<String>) -> String {
    let heading = blocks
        .first()
        .filter(|block| block.block_type == BlockType::Heading)
        .and_then(|block| block.content.as_ref())
        .and_then(|content| {
            let orch_api_contract::documents::BlockContent::Inline(runs) = content else {
                return None;
            };
            runs.iter()
                .map(|run| match run {
                    orch_api_contract::documents::Inline::Text { text, .. } => Some(text.clone()),
                    orch_api_contract::documents::Inline::Break => Some(" ".to_owned()),
                    orch_api_contract::documents::Inline::Math { source } => Some(source.clone()),
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()
                .map(|parts| parts.join("").trim().to_owned())
                .filter(|text| !text.is_empty())
        });
    let fallback = name.rsplit('/').next().unwrap_or(name);
    let fallback = fallback.rsplit_once('.').map_or(fallback, |(base, _)| base);
    let full = heading
        .clone()
        .unwrap_or_else(|| fallback.trim().to_owned());
    let most = usize::try_from(DOCUMENT_TITLE_MAX_CHARS).unwrap_or(usize::MAX);
    let chosen: String = full.chars().take(most).collect();
    if chosen.chars().count() < full.chars().count() {
        warnings.push(format!(
            "The title was shortened to {DOCUMENT_TITLE_MAX_CHARS} characters."
        ));
    }
    if heading.as_deref() == Some(chosen.as_str()) {
        blocks.remove(0);
    }
    if chosen.is_empty() {
        "Untitled".to_owned()
    } else {
        chosen
    }
}
