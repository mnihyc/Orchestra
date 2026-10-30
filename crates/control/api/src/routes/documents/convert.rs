//! Text turned into blocks and stored nowhere: the one reader that a paste goes through, and the files that a pasted page carries inside it.
//!
//! The browser sends what was pasted and the document it is for; the server reads it with the same reader
//! that an import and an agent's file go through (`orch_documents::convert`), so that what is read from a
//! paste, an import and an agent's text agree. With the document named, its files and its people are known to the
//! reader, and the pictures that the text carries in its own address (a page pasted from a word processor, a
//! Markdown file with a `data:` picture) are stored as files of that document, within its limits; without one
//! they are left out and the answer says so. Nothing in the answer is stored but those files.

use std::collections::HashSet;

use super::build::{Cast, Wanted};
use super::limits::{self, DocSettings, Limit};
use super::*;
use orch_api_contract::documents::{
    DocumentConvert, DocumentConvertBody, DocumentConvertFormat, DocumentConvertReply,
    DocumentFile, ReaderWarning,
};
use orch_blob_contract::range::BlobMeta;
use orch_db::queries::documents::HeldFile;
use orch_documents::convert::{self, Format};
use orch_documents::markdown::{Embedded, FileRef};
use orch_types::Hash;

/// Binds this module's request.
pub(super) fn mount(registry: &mut Registry) {
    registry.on(DocumentConvert, convert_text);
}

/// What the reader is told of the document it reads for: its people, and its files by their names.
pub(super) struct Held {
    pub(super) people: HashMap<String, Id>,
    pub(super) files: Vec<(String, FileRef)>,
}

impl Directory for Held {
    fn user_id(&self, username: &str) -> Option<Id> {
        self.people.get(&username.to_ascii_lowercase()).copied()
    }

    fn file(&self, name: &str) -> Option<markdown::FileRef> {
        self.files
            .iter()
            .find(|(each, _)| each == name)
            .map(|(_, file)| file.clone())
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

/// Registers the blobs of files about to be held, so that they can be pinned.
pub(super) async fn register(
    ctx: &Ctx,
    tx: &mut orch_db::Tx,
    held: &[HeldFile],
    at: DateTime<Utc>,
) -> ApiResult<()> {
    let hashes: Vec<Hash> = held.iter().map(|file| file.blob).collect();
    orch_db::queries::uploads::lock_blobs(tx, &hashes).await?;
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
fn without_files(blocks: &mut Vec<NewBlock>, gone: &HashSet<Id>) {
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

async fn convert_text(
    mut ctx: Ctx,
    (): (),
    body: DocumentConvertBody,
) -> ApiResult<DocumentConvertReply> {
    let format = match body.format {
        DocumentConvertFormat::Markdown => Format::Markdown,
        DocumentConvertFormat::Html => Format::Html,
        DocumentConvertFormat::Text => Format::Text,
    };
    // What the text may be, and what the document it is for already holds.
    let (limit, document, names) = {
        let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
        ctx.now = guard.now().get();
        let principal = guard.principal().clone();
        match body.document_id {
            Some(id) => {
                authorization::require_document(
                    &mut tx,
                    &principal,
                    id,
                    Permission::DocEdit,
                    Policy::default(),
                )
                .await?;
                let row = super::live(&mut tx, id).await?;
                let settings = DocSettings::at(&mut tx, row.node_id).await?;
                let files = rows::files(&mut tx, id).await?;
                (
                    settings.max_bytes.value,
                    Some((row.id, principal.user())),
                    files,
                )
            }
            None => {
                let limit = orch_settings::get::<i64>(
                    &mut tx,
                    orch_settings::Place::System,
                    orch_settings_contract::registry::keys::DOCS_MAX_BYTES,
                )
                .await
                .map_err(ApiError::internal)?
                .unwrap_or(i64::MAX);
                (limit, None, Vec::new())
            }
        }
    };
    if i64::try_from(body.text.len()).unwrap_or(i64::MAX) > limit {
        return Err(ApiError::new(
            ErrorCode::RequestTooLarge,
            "the text is larger than a document may be",
        )
        .with("limit", limit));
    }
    let bytes = body.text.into_bytes();
    let prepared = tokio::task::spawn_blocking(move || convert::prepare(format, &bytes))
        .await
        .map_err(ApiError::internal)?
        .map_err(|error| invalid("text", &error.to_string()))?;
    let people = if document.is_some() {
        let mut conn = ctx.app.db.acquire().await?;
        rows::users_by_username(&mut conn, &prepared.mentioned_names())
            .await?
            .into_iter()
            .map(|(name, id)| (name.to_ascii_lowercase(), id))
            .collect()
    } else {
        HashMap::new()
    };
    let held = Held {
        people,
        files: names
            .iter()
            .map(|file| {
                (
                    file.name.clone(),
                    FileRef {
                        id: file.id,
                        media_type: file.media_type.clone(),
                    },
                )
            })
            .collect(),
    };
    let mut read = prepared.finish(&held);
    let mut stored: Vec<DocumentFile> = Vec::new();
    if !read.embedded.is_empty() {
        match document {
            Some((id, user)) => {
                let mut taken: HashSet<String> =
                    names.iter().map(|file| file.name.clone()).collect();
                let files = hold(&ctx, &read.embedded, &mut taken, user, ctx.now).await?;
                stored = install(&ctx, id, files).await?;
            }
            _ => {
                let gone: HashSet<Id> = read.embedded.iter().map(|each| each.id).collect();
                without_files(&mut read.blocks, &gone);
                orch_documents::warnings::note(
                    &mut read.warnings,
                    orch_documents::warnings::Warning::new("reader.pictures_left_out", &[]),
                );
            }
        }
    }
    if let Some((id, _)) = document {
        // Parsing may take time; the returned references are for this document only while its edit grant holds.
        let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
        authorization::require_document(
            &mut tx,
            guard.principal(),
            id,
            Permission::DocEdit,
            Policy::default(),
        )
        .await?;
        super::live(&mut tx, id).await?;
        tx.commit().await.map_err(DbError::from)?;
    }
    Ok(DocumentConvertReply {
        blocks: read.blocks,
        warnings: read
            .warnings
            .into_iter()
            .map(|warning| ReaderWarning {
                code: warning.code,
                params: warning.params,
                message: warning.message,
            })
            .collect(),
        files: stored,
    })
}

/// Makes `files` files of the document, within what its place allows.
async fn install(ctx: &Ctx, document: Id, files: Vec<HeldFile>) -> ApiResult<Vec<DocumentFile>> {
    let (mut tx, _guard) = transaction_guard::begin(ctx).await?;
    orch_db::queries::documents::containers::lock_document_scope(&mut tx, document).await?;
    let row = super::live_locked(&mut tx, document).await?;
    let guard = transaction_guard::check(ctx, &mut tx).await?;
    let principal = guard.principal();
    authorization::require_document(
        &mut tx,
        principal,
        document,
        Permission::DocEdit,
        Policy::default(),
    )
    .await?;
    let settings = DocSettings::at(&mut tx, row.node_id).await?;
    let now = guard.now().get();
    let mut total = row.files_bytes;
    for file in &files {
        if file.file.size > settings.file_max_bytes.value {
            return Err(limits::too_large(
                &mut tx,
                principal,
                &settings,
                Limit::FileBytes,
                Policy::default(),
            )
            .await);
        }
        total = total.saturating_add(file.file.size);
    }
    if total > settings.files_max_bytes.value {
        return Err(limits::too_large(
            &mut tx,
            principal,
            &settings,
            Limit::FilesBytes,
            Policy::default(),
        )
        .await);
    }
    register(ctx, &mut tx, &files, now).await?;
    rows::copy::install_files(&mut tx, document, &files, Some(principal.user()), None, now)
        .await
        .map_err(ApiError::internal)?;
    for file in &files {
        rows::history::record_file(
            &mut tx,
            document,
            &Writer::User(principal.user()),
            DocumentFileChange {
                name: file.file.name.clone(),
                kind: DocumentFileChangeKind::Added,
            },
            settings.version_rules(ctx.app.config.documents.version_window_secs),
            now,
        )
        .await?;
    }
    rows::touch(&mut tx, document).await?;
    let mut wanted = Wanted::default();
    wanted.user(principal.user());
    let cast = Cast::load(&mut tx, principal, wanted, Policy::default()).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(files
        .into_iter()
        .map(|file| DocumentFile {
            id: file.file.id,
            name: file.file.name,
            media_type: file.file.media_type,
            size: file.file.size,
            is_text: file.file.is_text,
            added_by: cast.author(Some(principal.user()), None, None),
            added_at: Time::new(now),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::middleware::authentication;
    use crate::rig::Scene;
    use orch_api_contract::routes::Endpoint;
    use orch_db::queries::access::{self, PageKey};

    #[tokio::test]
    async fn pasted_files_require_edit_access_when_they_are_installed() {
        let scene = Scene::new().await;
        let app = scene.rig.world.app.app.clone();
        let document = scene
            .rig
            .world
            .app
            .call(
                orch_api_contract::documents::NodeDocumentCreate,
                &[&scene.tree.topic.base.id],
            )
            .token(&scene.ada.token)
            .body(&orch_api_contract::documents::NodeDocumentCreateBody {
                title: "A document".into(),
                blocks: None,
                markdown: None,
                access: None,
            })
            .send()
            .await
            .ok()
            .base
            .id;
        let mut conn = app.db.acquire().await.unwrap();
        let page = PageKey::new("document", document, "live");
        access::testing::set_row(&mut conn, &page, "any", "doc.edit", false, false)
            .await
            .unwrap();
        let entry = access::testing::add_entry(
            &mut conn,
            &page,
            "any",
            "doc.edit",
            ("user", Some(scene.dan.id()), None),
            None,
            app.clock.now(),
        )
        .await
        .unwrap();
        drop(conn);
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::AUTHORIZATION,
            format!("Bearer {}", scene.dan.token).parse().unwrap(),
        );
        let caller = authentication::authenticate(&app, &headers, app.clock.now())
            .await
            .unwrap();
        let mut ctx = Ctx::for_tool(app.clone(), caller, DocumentConvert::ROUTE, document);
        ctx.headers = headers;
        let (mut tx, guard) = transaction_guard::begin(&ctx).await.unwrap();
        let principal = guard.into_principal();
        authorization::require_document(
            &mut tx,
            &principal,
            document,
            Permission::DocEdit,
            Policy::default(),
        )
        .await
        .unwrap();
        tx.rollback().await.unwrap();
        let blob = app
            .blobs
            .put_bytes(bytes::Bytes::from_static(b"picture"))
            .await
            .unwrap();
        let file = |name: &str| HeldFile {
            file: rows::FileRow {
                id: orch_types::new_id(),
                name: name.to_owned(),
                media_type: "image/png".into(),
                size: blob.size,
                is_text: false,
                added_by: Some(scene.dan.id()),
                added_by_thread: None,
                added_at: app.clock.now(),
            },
            blob: blob.hash,
        };
        install(&ctx, document, vec![file("first.png")])
            .await
            .unwrap();
        let mut conn = app.db.acquire().await.unwrap();
        assert!(
            access::remove_entry(&mut conn, entry, scene.ada.id(), app.clock.now())
                .await
                .unwrap()
        );
        access::bump_epoch(&mut conn).await.unwrap();
        let current = authorization::principal(&ctx, &mut conn).await.unwrap();
        assert!(
            authorization::require_document(
                &mut conn,
                &current,
                document,
                Permission::DocEdit,
                Policy::default(),
            )
            .await
            .is_err(),
            "the caller no longer holds edit"
        );
        drop(conn);
        let result = install(&ctx, document, vec![file("after-revocation.png")]).await;
        assert!(result.is_err(), "a revoked editor cannot install a file");
        let mut conn = app.db.acquire().await.unwrap();
        let files = rows::files(&mut conn, document).await.unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].name, "first.png");
    }
}
