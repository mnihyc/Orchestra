//! Bounded Markdown, HTML, Word and plain-text imports with per-entry guarded creation and durable document identities.
//!
//! Every entry is read by the one reader (`orch_documents::convert`): a link to another entry of the same import
//! is a reference to the document that entry becomes, a link or a picture that leads nowhere is the words it was
//! written with and a sentence in the entry's report, and the pictures a Word file or a page carries inside it become
//! files of the document made.

use super::*;
use orch_api_contract::documents::{
    BlockType, DocumentImportRequest, DocumentImportRequestFileFormat as Format,
    DocumentImportResult, DocumentImportResultEntry, DocumentImportResultError, InlineRefKind,
    NodeDocumentImport,
};
use orch_blob_contract::range::BlobMeta;
use orch_documents::convert;
use orch_documents::markdown::{FileRef, Local};
use orch_documents::references::Typed;
use orch_types::Hash;

// Import uses the published 200-entry list bound, independently of larger archive-browser pages.
const ENTRIES_MAX: usize = orch_api_contract::common::MAX_LIMIT as usize;
// This is the registry's shipped expanded-source allowance when no override is stored.
const SHIPPED_EXPANDED_BYTES: i64 = 64 * 1024 * 1024;

pub(super) fn mount(registry: &mut Registry) {
    registry.on(NodeDocumentImport, import);
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

async fn source_size(ctx: &Ctx, source: Hash) -> ApiResult<i64> {
    let (mut tx, guard) = transaction_guard::begin(ctx).await?;
    crate::attachments::require(&mut tx, guard.principal(), &source).await?;
    tx.commit().await.map_err(DbError::from)?;
    let size = ctx
        .app
        .blobs
        .store()
        .size(&source)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::not_found)?;
    Ok(size)
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

async fn inputs(ctx: &Ctx, body: &DocumentImportRequest, limit: i64) -> ApiResult<Vec<Input>> {
    let mut out = Vec::new();
    let mut used = 0_i64;
    let mut names = std::collections::HashSet::new();
    for file in &body.files {
        let prepared = async {
            let name = crate::archives::import_path(&file.name)
                .map_err(|error| invalid("files", error))?;
            if !names.insert(name.clone()) {
                return Err(invalid("files", "duplicate source file name"));
            }
            let size = source_size(ctx, file.blob).await?;
            if file.format != Format::Archive && size > limit.saturating_sub(used) {
                return Err(invalid(
                    "files",
                    "the selected sources exceed the expanded import byte allowance",
                ));
            }
            if file.format == Format::Archive {
                let sources = crate::archives::documents(
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
                let sources = sources
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

async fn make(
    ctx: &Ctx,
    client: &str,
    index: usize,
    input: Input,
    expanded: i64,
    siblings: &HashMap<String, Id>,
    companions: &HashMap<String, Companion>,
) -> ApiResult<(Document, Vec<String>)> {
    let Some(format) = reader_format(input.format) else {
        return Err(invalid(
            "files",
            "an archive entry must be Markdown, HTML, Word or text",
        ));
    };
    // Reading an untrusted source runs before the short action transaction. The actual source grant and
    // destination allowance are checked again after this work has completed.
    let name = input.name.clone();
    let bytes = input.bytes;
    let prepared = tokio::task::spawn_blocking(move || convert::prepare(format, &bytes))
        .await
        .map_err(ApiError::internal)?
        .map_err(|error| invalid("files", &error.to_string()))?;
    let (mut ctx, mut tx, principal) = crate::routes::nodes::action(ctx).await?;
    let node = ctx.id("id")?;
    let id = rows::copy::identity(
        principal.user(),
        node,
        client,
        &format!("import:{index}:{name}"),
    );
    if let Some(made) = creation::existing(&ctx, &mut tx, &principal, id).await? {
        let warnings: Option<orch_types::Json> = sqlx::query_scalar("SELECT COALESCE(details->'import_warnings','[]'::jsonb) FROM audit_log WHERE action='document.created' AND object_id=$1 ORDER BY at,id LIMIT 1")
            .bind(id).fetch_optional(&mut *tx).await.map_err(DbError::from)?;
        let warnings = warnings
            .map(serde_json::from_value::<Vec<String>>)
            .transpose()
            .map_err(ApiError::internal)?
            .unwrap_or_default();
        tx.commit().await.map_err(DbError::from)?;
        return Ok((made, warnings));
    }
    crate::attachments::require(&mut tx, &principal, &input.source).await?;
    if expanded > allowance(&mut tx, node).await? {
        return Err(invalid(
            "files",
            "the import exceeds the current expanded byte allowance",
        ));
    }
    let base = name.rsplit_once('/').map_or("", |(base, _)| base);
    let people = {
        let found = rows::users_by_username(&mut tx, &prepared.mentioned_names()).await?;
        found
            .into_iter()
            .map(|(name, id)| (name.to_ascii_lowercase(), id))
            .collect()
    };
    let entries = Entries {
        people,
        base,
        siblings,
        companions,
        asked: std::sync::Mutex::new(HashMap::new()),
    };
    let read = prepared.finish(&entries);
    let mut blocks = read.blocks;
    let mut warnings: Vec<String> = read.warnings.into_iter().map(|each| each.message).collect();
    // The pictures and files of the archive that the text shows are files of this document.
    let mut embedded = read.embedded;
    embedded.extend(entries.named_files(&blocks));
    let mut taken = std::collections::HashSet::new();
    let files = super::convert::hold(
        &ctx,
        &embedded,
        &mut taken,
        principal.user(),
        principal.now().get(),
    )
    .await?;
    let title = title(&name, &mut blocks, &mut warnings);
    ctx.now = principal.now().get();
    let body = NodeDocumentCreateBody {
        title,
        blocks: Some(blocks),
        markdown: None,
        access: None,
    };
    let made = create_prepared(
        ctx,
        body,
        Some(id),
        files,
        Some((tx, principal)),
        warnings.clone(),
    )
    .await?;
    Ok((made, warnings))
}

async fn import(ctx: Ctx, (): (), body: DocumentImportRequest) -> ApiResult<DocumentImportResult> {
    creation::client_id(&body.client_id)?;
    if body.files.is_empty() || body.files.len() > ENTRIES_MAX {
        return Err(invalid("files", "select one to two hundred sources"));
    }
    if body.files.iter().any(|file| file.format == Format::Archive) && body.files.len() != 1 {
        return Err(invalid(
            "files",
            "select several text files or one archive, not both",
        ));
    }
    let (ctx, mut tx, _) = crate::routes::nodes::action(&ctx).await?;
    let limit = allowance(&mut tx, ctx.id("id")?).await?;
    tx.commit().await.map_err(DbError::from)?;
    let selected = inputs(&ctx, &body, limit).await?;
    let expanded = selected.iter().fold(0_i64, |total, input| {
        total.saturating_add(i64::try_from(input.bytes.len()).unwrap_or(i64::MAX))
    });
    // The files that came with the texts of an archive, by their path in it.
    let companions: HashMap<String, Companion> = selected
        .iter()
        .filter(|input| input.companion && input.error.is_none())
        .map(|input| {
            (
                input.name.clone(),
                Companion {
                    media_type: orch_types::media::media_type(&input.name, &[]).to_owned(),
                    bytes: input.bytes.clone(),
                },
            )
        })
        .collect();
    // Which document each entry becomes, from what a retry would make again: a link from one to another is a reference.
    let siblings: HashMap<String, Id> = {
        let (_, tx, principal) = crate::routes::nodes::action(&ctx).await?;
        let node = ctx.id("id")?;
        let planned = selected
            .iter()
            .enumerate()
            .filter(|(_, input)| {
                input.error.is_none() && !input.companion && reader_format(input.format).is_some()
            })
            .map(|(index, input)| {
                (
                    input.name.clone(),
                    rows::copy::identity(
                        principal.user(),
                        node,
                        &body.client_id,
                        &format!("import:{index}:{}", input.name),
                    ),
                )
            })
            .collect();
        tx.commit().await.map_err(DbError::from)?;
        planned
    };
    let mut entries = Vec::new();
    for (index, mut input) in selected.into_iter().enumerate() {
        let name = input.name.clone();
        if let Some(error) = input.error.take() {
            entries.push(refused(name, error));
            continue;
        }
        if input.companion {
            continue;
        }
        match make(
            &ctx,
            &body.client_id,
            index,
            input,
            expanded,
            &siblings,
            &companions,
        )
        .await
        {
            Ok((document, warnings)) => entries.push(DocumentImportResultEntry {
                name,
                document: Some(document.base),
                warnings,
                error: None,
            }),
            Err(error) => entries.push(refused(name, error)),
        }
    }
    Ok(DocumentImportResult { entries })
}
