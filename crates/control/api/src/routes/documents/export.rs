//! A document written out as a file: Markdown, one HTML page, Typst source with what it names, or its blocks as JSON, with the files and the discussions beside it when they are asked for.
//!
//! Every renderer is a pure function of the document as it stands; here the
//! names a reader would see are put in, the parts that were asked for are
//! gathered, and the answer is a download: one file, or a zip when the
//! document has files to go beside it. Markdown says as much of a document
//! as Markdown can, and what it could not keep is told in the answer's
//! `X-Orch-Export-Losses` header and in the audit row of the export. A PDF is
//! made in the reader's browser from the Typst export, so that no service of
//! Orch compiles a document.
//!
//! The files a document holds are kept in the blob store. A Markdown export that has files to go beside it is a zip
//! of the text and of them, and the text links each file by the path it lies at in the zip, `<title>.files/<name>`
//! (see `orch_engine::written::files_beside`: the one rule of such a link, which an archive of a place and what an
//! agent is given follow as well). An HTML page holds its images inside it, as data addresses, so that it needs
//! nothing else; with the attachments asked for it is a zip of the page and of them, at `files/<name>`, the path the
//! page links them by. Typst source is always a zip: of the source, of the images it uses at the paths it names them
//! by, and of the attachments at `files/<name>` when they are asked for. A file whose bytes the store lost is left
//! out, and the page says that an image is not there.

use std::collections::HashMap;
use std::io::Write as _;

use super::{build, record};
use crate::middleware::authorization;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use crate::routes::nodes::actor_name;
use axum::body::Body;
use axum::response::Response;
use http::{HeaderValue, StatusCode, header};
use orch_access_contract::audit::AuditAction;
use orch_access_contract::permissions::Permission;
use orch_api_contract::documents::{
    DocumentDiscussion, DocumentExport, DocumentExportChoice, DocumentExportGet,
    DocumentExportGetQuery, InlineRefKind,
};
use orch_authz::Policy;
use orch_blob_contract::store::BlobError;
use orch_db::Conn;
use orch_db::queries::documents::discussions;
use orch_db::queries::documents::{self as rows, FileRow};
use orch_db::queries::exports as export_rows;
use orch_documents::blocks::Lookup;
use orch_documents::files::FileInfo;
use orch_documents::markdown::{self, RenderOptions};
use orch_documents::{html, typst};
use orch_engine::exports::DocumentHow;
use orch_engine::written::files_beside;
use orch_types::{Hash, Id};
use serde_json::json;

/// The name a file gets when a title leaves nothing that a file name can hold.
const FALLBACK_NAME: &str = "document";
/// The most characters of a title that go into a file name.
const NAME_MAX_CHARS: usize = 100;
/// The header that says what an export could not keep.
const LOSSES_HEADER: &str = "x-orch-export-losses";

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry.on_raw(DocumentExportGet, export);
}

/// What the renderers ask about the things a document names.
struct Known<'a> {
    cast: &'a build::Cast,
    files: HashMap<Id, FileInfo>,
}

impl Lookup for Known<'_> {
    fn username(&self, user: Id) -> Option<String> {
        self.cast.person(user).map(|user| user.username)
    }

    fn display_name(&self, user: Id) -> Option<String> {
        self.cast.person(user).map(|user| user.display_name)
    }

    fn name_of(&self, kind: InlineRefKind, id: Id) -> Option<String> {
        match kind {
            InlineRefKind::Node => self.cast.node(id).and_then(|node| node.name),
            InlineRefKind::Document => self.cast.title_of(id),
            InlineRefKind::Chat => self.cast.chat_of(id),
            InlineRefKind::File => self.cast.file_of(id),
            InlineRefKind::EnvironmentFile => None,
        }
    }

    fn file(&self, id: Id) -> Option<FileInfo> {
        self.files.get(&id).cloned()
    }
}

/// What the renderers are told of a file, without its bytes: Markdown and Typst link a file by the path it lies at
/// in the archive, and an HTML page is given the bytes of its images before it is written (`with_image_bytes`).
fn info_of(file: &FileRow) -> FileInfo {
    FileInfo {
        name: file.name.clone(),
        media_type: file.media_type.clone(),
        size: file.size,
        is_text: file.is_text,
        bytes: None,
    }
}

/// A title as a file name: letters, digits and a few marks, the rest an underscore.
fn file_stem(title: &str) -> String {
    let stem: String = title
        .trim()
        .chars()
        .take(NAME_MAX_CHARS)
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.' | '(' | ')') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let stem = stem.trim_matches(|c: char| c == '.' || c.is_whitespace());
    if stem.is_empty() {
        FALLBACK_NAME.to_owned()
    } else {
        stem.to_owned()
    }
}

/// A download: the bytes, their type, and the name to save them under.
fn download(
    name: &str,
    media_type: &str,
    bytes: Vec<u8>,
    losses: Option<&str>,
) -> ApiResult<Response> {
    let ascii: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_graphic() || c == ' ' {
                c
            } else {
                '_'
            }
        })
        .filter(|c| *c != '"' && *c != '\\')
        .collect();
    let encoded: String = form_urlencoded::byte_serialize(name.as_bytes())
        .collect::<String>()
        .replace('+', "%20");
    let disposition = format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{encoded}");
    let mut response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, media_type)
        .header(header::CONTENT_DISPOSITION, disposition)
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .body(Body::from(bytes))
        .map_err(ApiError::internal)?;
    if let Some(losses) = losses.filter(|losses| !losses.is_empty())
        && let Ok(value) = HeaderValue::from_str(losses)
    {
        response.headers_mut().insert(LOSSES_HEADER, value);
    }
    Ok(response)
}

/// A zip of the named entries.
fn archive(entries: &[(String, Vec<u8>)]) -> ApiResult<Vec<u8>> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, bytes) in entries {
        writer
            .start_file(name.as_str(), options)
            .map_err(ApiError::internal)?;
        writer.write_all(bytes).map_err(ApiError::internal)?;
    }
    Ok(writer.finish().map_err(ApiError::internal)?.into_inner())
}

/// A file of the store that goes beside a text in a zip: the path it lies at, its content, and its size.
type Stored = (String, Hash, i64);

/// The files that go beside a Markdown text: the images, and the files that are no images when asked, each at the
/// path in `directory` that a link in the text leads to. `held` says where the content of each is, by name.
fn companions(
    files: &[FileRow],
    held: &HashMap<String, Hash>,
    directory: &str,
    attachments: bool,
) -> Vec<Stored> {
    files
        .iter()
        .filter(|file| attachments || orch_documents::files::is_image(&file.media_type))
        .filter_map(|file| {
            held.get(&file.name)
                .map(|blob| (format!("{directory}/{}", file.name), *blob, file.size))
        })
        .collect()
}

/// Where a page or a Typst source finds the files that are no images when they go beside it: the directory the
/// renderers link them in.
const ATTACHMENTS_DIR: &str = "files";

/// The files that are no images, which go beside a page or a source in its archive when they are asked for, each
/// at `files/<name>`: the path the page and the source link them by.
fn attachments_of(files: &[FileRow], held: &HashMap<String, Hash>) -> Vec<Stored> {
    files
        .iter()
        .filter(|file| !orch_documents::files::is_image(&file.media_type))
        .filter_map(|file| {
            held.get(&file.name)
                .map(|blob| (format!("{ATTACHMENTS_DIR}/{}", file.name), *blob, file.size))
        })
        .collect()
}

/// Puts the bytes of the images among `files` into what the renderers are told of them, read from the store, so
/// that a page holds them inside it. An image whose bytes the store lost stays without them, and the page says that
/// it is not there.
async fn with_image_bytes(
    known: &mut Known<'_>,
    files: &[FileRow],
    held: &HashMap<String, Hash>,
    blobs: &crate::blobs::Blobs,
) -> ApiResult<()> {
    for file in files
        .iter()
        .filter(|file| orch_documents::files::is_image(&file.media_type))
    {
        let (Some(blob), Some(info)) = (held.get(&file.name), known.files.get_mut(&file.id)) else {
            continue;
        };
        match blobs.read_whole(blob, file.size.max(0)).await {
            Ok(bytes) => info.bytes = Some(bytes.to_vec()),
            Err(crate::blobs::BlobsError::Store(BlobError::NotFound(_))) => {}
            Err(error) => return Err(ApiError::internal(error)),
        }
    }
    Ok(())
}

/// Writes the discussions of a document, the newest answer first, when they are asked for.
async fn discussions_of(
    conn: &mut Conn,
    principal: &orch_authz::Principal,
    document: Id,
    upto: i64,
) -> ApiResult<Vec<DocumentDiscussion>> {
    let found = discussions::changed(conn, document, 0, upto).await?;
    build::discussions_for(conn, principal, document, &found, Policy::default()).await
}

/// What is asked of one export of a document.
#[derive(Clone, Copy)]
pub(crate) struct Asked<'a> {
    /// The form.
    pub format: DocumentExport,
    /// Whether the files that are no images go beside the text.
    pub attachments: bool,
    /// Whether the discussions go with it.
    pub comments: bool,
    /// The directory a Markdown text links its files in: `files_beside` of the file it is kept in.
    pub beside: &'a str,
    /// HTML files in a package link their images and attachments beside them, without reading image blobs.
    pub files_beside: bool,
    /// The store an HTML page reads the bytes of its images from, to hold them inside it; none where the form
    /// made is Markdown or JSON, whose files are read after it is written.
    pub blobs: Option<&'a crate::blobs::Blobs>,
}

/// What one export of a document comes to before it is a download: the file the format is made of, and what goes
/// beside it when there is an archive. A single export and a document in an archive of a place are both made here,
/// so that the one is the very bytes of the other.
pub(crate) struct Produced {
    format: DocumentExport,
    /// The text or the records themselves: the Markdown, the page, the Typst source or the JSON.
    pub main: Vec<u8>,
    /// What goes beside it where an archive is made, whose bytes are at hand.
    beside: Vec<(String, Vec<u8>)>,
    /// The files of the store that go beside the text: for Markdown its images and the files asked for, for an HTML
    /// page the files asked for, for Typst source the images it uses and the files asked for.
    stored: Vec<Stored>,
    /// What Markdown could not keep, as a sentence; empty where it kept all.
    pub losses: String,
}

/// Writes a document as `principal` exports it, in the form, with the files and the discussions that are asked
/// for. The row has been read in the caller's transaction.
pub(crate) async fn produce(
    conn: &mut Conn,
    principal: &orch_authz::Principal,
    row: &rows::DocumentRow,
    asked: &Asked<'_>,
    now: chrono::DateTime<chrono::Utc>,
) -> ApiResult<Produced> {
    let Asked {
        format,
        attachments,
        comments: with_comments,
        beside: directory,
        files_beside,
        blobs,
    } = *asked;
    let id = row.id;
    let policy = Policy::default();
    if format == DocumentExport::Json {
        let mut synced = super::read_sync(conn, principal, id, None, now).await?;
        if !with_comments {
            synced.discussions.clear();
        }
        let main = serde_json::to_vec_pretty(&synced).map_err(ApiError::internal)?;
        return Ok(Produced {
            format,
            main,
            beside: Vec::new(),
            stored: Vec::new(),
            losses: String::new(),
        });
    }
    let stored = rows::live_blocks(conn, id).await?;
    let files = rows::files(conn, id).await?;
    let discussions = if with_comments {
        Some(discussions_of(conn, principal, id, row.doc_seq).await?)
    } else {
        None
    };
    let mut wanted = build::Wanted::default();
    for each in &stored {
        wanted.block(each);
    }
    for each in discussions.iter().flatten() {
        for comment in &each.comments {
            wanted.runs(&comment.content);
        }
    }
    let cast = build::Cast::load(conn, principal, wanted, policy).await?;
    let blocks: Vec<_> = stored.iter().map(|each| cast.block(each)).collect();
    let mut known = Known {
        cast: &cast,
        files: files.iter().map(|file| (file.id, info_of(file))).collect(),
    };
    // Where the content of each file is in the store, by its name in the document.
    let held: HashMap<String, Hash> = export_rows::document_files(conn, &[id])
        .await?
        .into_iter()
        .map(|file| (file.name, file.blob))
        .collect();
    let kept = discussions.as_deref();
    Ok(match format {
        DocumentExport::Markdown => {
            let rendered = markdown::render(
                &blocks,
                &known,
                &RenderOptions {
                    discussions: kept,
                    files_dir: Some(directory),
                    ..RenderOptions::default()
                },
            );
            let text = format!("# {}\n\n{}", row.title, rendered.text);
            let losses = rendered
                .losses
                .iter()
                .map(markdown::Loss::describe)
                .collect::<Vec<_>>()
                .join("; ");
            Produced {
                format,
                main: text.into_bytes(),
                beside: Vec::new(),
                stored: companions(&files, &held, directory, attachments),
                losses,
            }
        }
        DocumentExport::Html => {
            // A standalone page embeds images; a package page links to the files the archive streams separately.
            if !files_beside && let Some(blobs) = blobs {
                with_image_bytes(&mut known, &files, &held, blobs).await?;
            }
            let page = html::render(
                &row.title,
                &blocks,
                &known,
                &html::Options {
                    discussions: kept,
                    attachments_beside: attachments,
                    files_dir: files_beside.then_some(directory),
                },
            );
            Produced {
                format,
                main: page.into_bytes(),
                beside: Vec::new(),
                stored: if files_beside {
                    companions(&files, &held, directory, true)
                } else if attachments {
                    attachments_of(&files, &held)
                } else {
                    Vec::new()
                },
                losses: String::new(),
            }
        }
        DocumentExport::Typst => {
            let written = typst::render(
                &row.title,
                &blocks,
                &known,
                &typst::Options {
                    discussions: kept,
                    attachments_beside: attachments,
                },
            );
            // The images the source uses, each at the path it names it by.
            let mut named: Vec<Stored> = written
                .images
                .iter()
                .filter_map(|image| {
                    let file = files.iter().find(|file| file.id == image.file_id)?;
                    held.get(&file.name)
                        .map(|blob| (image.path.clone(), *blob, file.size))
                })
                .collect();
            if attachments {
                named.extend(attachments_of(&files, &held));
            }
            Produced {
                format,
                main: written.source.into_bytes(),
                beside: Vec::new(),
                stored: named,
                losses: String::new(),
            }
        }
        DocumentExport::Json => {
            return Err(ApiError::internal(
                "JSON is written before the blocks are read",
            ));
        }
    })
}

impl Produced {
    /// The files of the store that go beside the text, read: each at the path a link in the text leads to. One that
    /// the store lost is left out; the rest are there.
    pub(crate) async fn read_stored(&mut self, blobs: &crate::blobs::Blobs) -> ApiResult<()> {
        for (path, blob, size) in std::mem::take(&mut self.stored) {
            match blobs.read_whole(&blob, size.max(0)).await {
                Ok(bytes) => self.beside.push((path, bytes.to_vec())),
                Err(crate::blobs::BlobsError::Store(BlobError::NotFound(_))) => {}
                Err(error) => return Err(ApiError::internal(error)),
            }
        }
        Ok(())
    }

    /// The download: one file, or an archive of it and what goes beside it.
    fn response(self, stem: &str) -> ApiResult<Response> {
        let losses = self.losses.clone();
        match self.format {
            DocumentExport::Json => {
                download(&format!("{stem}.json"), "application/json", self.main, None)
            }
            DocumentExport::Markdown if self.beside.is_empty() => download(
                &format!("{stem}.md"),
                "text/markdown; charset=utf-8",
                self.main,
                Some(&losses),
            ),
            DocumentExport::Markdown => {
                let mut all = vec![(format!("{stem}.md"), self.main)];
                all.extend(self.beside);
                download(
                    &format!("{stem}.zip"),
                    "application/zip",
                    archive(&all)?,
                    Some(&losses),
                )
            }
            DocumentExport::Html if self.beside.is_empty() => download(
                &format!("{stem}.html"),
                "text/html; charset=utf-8",
                self.main,
                None,
            ),
            DocumentExport::Html => {
                let mut all = vec![(format!("{stem}.html"), self.main)];
                all.extend(self.beside);
                download(
                    &format!("{stem}.zip"),
                    "application/zip",
                    archive(&all)?,
                    None,
                )
            }
            DocumentExport::Typst => {
                let mut all = vec![("main.typ".to_owned(), self.main)];
                all.extend(self.beside);
                download(
                    &format!("{stem}.zip"),
                    "application/zip",
                    archive(&all)?,
                    None,
                )
            }
        }
    }
}

/// Writes a document as Markdown, HTML, Typst or JSON.
async fn export(ctx: Ctx, query: DocumentExportGetQuery, _body: Body) -> ApiResult<Response> {
    let id = ctx.id("id")?;
    let format = query.format.unwrap_or_default();
    let attachments = query.attachments == Some(DocumentExportChoice::Include);
    let with_comments = query.comments == Some(DocumentExportChoice::Include);
    let mut tx = ctx.app.db.begin().await?;
    let principal = authorization::principal(&ctx, &mut tx).await?;
    let row = super::live(&mut tx, id).await?;
    // Taking a document away as a file asks for its Export row, and for reading it as well: the row may be
    // held where the document is not (the export of a place).
    match orch_authz::documents::check(
        &mut tx,
        &principal,
        id,
        Permission::DocView,
        Policy::default(),
    )
    .await
    .map_err(ApiError::internal)?
    {
        orch_access_contract::decisions::Decision::Allowed { .. } => {}
        orch_access_contract::decisions::Decision::Refused(_) => return Err(ApiError::not_found()),
    }
    let stem = file_stem(&row.title);
    // The text is kept in `<stem>.md`, and links its files by the path they lie at beside it.
    let beside = files_beside(&format!("{stem}.md"));
    let mut produced = produce(
        &mut tx,
        &principal,
        &row,
        &Asked {
            format,
            attachments,
            comments: with_comments,
            beside: &beside,
            files_beside: false,
            blobs: Some(&ctx.app.blobs),
        },
        ctx.now,
    )
    .await?;
    produced.read_stored(&ctx.app.blobs).await?;
    let losses = produced.losses.clone();
    let response = produced.response(&stem)?;

    let who = actor_name(&ctx)?;
    record(
        &ctx,
        &mut tx,
        orch_db::audit::action!(AuditAction::DocumentExported),
        &row,
        &format!("{who} exported the document \"{}\"", row.title),
        json!({
            "format": format,
            "attachments": query.attachments.unwrap_or_default(),
            "comments": query.comments.unwrap_or_default(),
            "losses": Some(losses.as_str()).filter(|losses| !losses.is_empty()),
        }),
    )
    .await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    let label = match format {
        orch_api_contract::documents::DocumentExport::Markdown => "markdown",
        orch_api_contract::documents::DocumentExport::Html => "html",
        orch_api_contract::documents::DocumentExport::Typst => "typst",
        orch_api_contract::documents::DocumentExport::Json => "json",
    };
    metrics::counter!("orch_document_exports_total","format"=>label).increment(1);
    Ok(response)
}

/// A document in an archive: its JSON record, Markdown with losses, or an HTML page that links files in `how.beside`.
/// Discussions follow the request; HTML companion files are streamed by the archive rather than embedded in the page.
pub(crate) async fn single_form(
    conn: &mut Conn,
    principal: &orch_authz::Principal,
    id: Id,
    how: &DocumentHow,
    now: chrono::DateTime<chrono::Utc>,
) -> ApiResult<(Vec<u8>, String)> {
    let row = super::live(conn, id).await?;
    let asked = Asked {
        format: how.format,
        attachments: false,
        comments: how.comments,
        beside: &how.beside,
        files_beside: how.format == DocumentExport::Html && how.files,
        blobs: None,
    };
    let produced = produce(conn, principal, &row, &asked, now).await?;
    Ok((produced.main, produced.losses))
}
