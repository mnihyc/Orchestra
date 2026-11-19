//! Making a document: its blocks written under fresh identifiers with the references, mentions and files of the record read through the map; its files put in it from those the batch staged; its description, its times and where it lies; and, where the plan says one is there already, a version cut of it first and the new content in the place of the old.

use super::{Batch, Made, MakeError, Staged, note_origin, refuse, settle_name};
use crate::document_tree::{Actor, Directory};
use crate::imports::identity;
use crate::imports::model::Object;
use crate::imports::plain;
use crate::imports::remap::{Found, Maps, remap};
use futures::StreamExt;
use orch_access_contract::audit::AuditAction;
use orch_access_contract::permissions::Permission;
use orch_api_contract::documents::{
    DOCUMENT_OPS_MAX, DocumentBlock, DocumentOp, InlineRefKind, NewBlock, VersionCause,
};
use orch_api_contract::imports::{ImportAction, ImportOutcome};
use orch_api_contract::package::PackageKind;
use orch_authz::Policy;
use orch_blob_contract::store::{BlobError, ByteStream};
use orch_db::audit::{Actor as AuditActor, Entry as AuditEntry, append};
use orch_db::queries::documents::{self as rows, batch, history};
use orch_db::queries::storage;
use orch_db::{DbError, Tx};
use orch_documents::convert;
use orch_documents::markdown::{Directory as Names, Embedded, FileRef, Local};
use orch_documents::references::Typed;
use orch_documents::versions::Writer;
use orch_types::{Hash, Id, Json, Time};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

/// The most bytes of a record an import reads into memory: sixty-four mebibytes, the largest copy one request shares.
pub const RECORD_MAX_BYTES: u64 = 64 * 1024 * 1024;

/// The longest title of a document, in characters.
const TITLE_MAX: usize = orch_api_contract::documents::DOCUMENT_TITLE_MAX_CHARS as usize;

/// The longest description of a document, in characters.
const DESCRIPTION_MAX: usize =
    orch_api_contract::documents::DOCUMENT_DESCRIPTION_MAX_CHARS as usize;

/// The blocks of a document of the source.
enum Body {
    /// As a package records them: with their identifiers, parents and positions, to be read through the map.
    Records(Vec<DocumentBlock>),
    /// As a reader of plain files made them: new, in order, with nothing to map.
    Fresh(Vec<NewBlock>),
}

/// What a document of the source holds, read.
struct Read {
    title: String,
    description: String,
    creator: Option<orch_api_contract::documents::Author>,
    updater: Option<orch_api_contract::documents::Author>,
    blocks: Body,
    files: Vec<FileFacts>,
    /// The files the text carried inside it (a picture in its own address, a Word file's pictures).
    embedded: Vec<Embedded>,
    discussions: Vec<Json>,
    warnings: Vec<(String, String)>,
}

/// A file of a document of the source.
#[derive(Debug, Clone)]
struct FileFacts {
    id: Id,
    name: String,
    media_type: String,
    is_text: bool,
    entry: String,
}

/// The blocks of a record as a tree of new blocks under fresh identifiers, with everything they name read through the map.
fn tree_of(
    blocks: &[DocumentBlock],
    document: Id,
    maps: &Maps<'_>,
    found: &mut Found,
) -> Result<Vec<NewBlock>, MakeError> {
    let live: Vec<&DocumentBlock> = blocks.iter().filter(|block| !block.removed).collect();
    let mut children: BTreeMap<Option<Id>, Vec<&DocumentBlock>> = BTreeMap::new();
    let known: HashSet<Id> = live.iter().map(|block| block.id).collect();
    for block in &live {
        // A block whose parent is gone lies at the top.
        let parent = block.parent_id.filter(|parent| known.contains(parent));
        children.entry(parent).or_default().push(block);
    }
    for list in children.values_mut() {
        list.sort_by(|a, b| {
            let key = |block: &DocumentBlock| block.position.parse::<f64>().unwrap_or(0.0);
            key(a)
                .partial_cmp(&key(b))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.id.cmp(&b.id))
        });
    }
    fn build(
        parent: Option<Id>,
        children: &BTreeMap<Option<Id>, Vec<&DocumentBlock>>,
        document: Id,
        maps: &Maps<'_>,
        found: &mut Found,
        depth: usize,
    ) -> Result<Vec<NewBlock>, MakeError> {
        if depth > 64 {
            return refuse("its blocks lie more levels inside one another than a document holds");
        }
        let mut out = Vec::new();
        for block in children.get(&parent).into_iter().flatten() {
            let mut props = serde_json::to_value(&block.props)
                .map_err(|error| MakeError::Refused(error.to_string()))?;
            let mut content = serde_json::to_value(&block.content)
                .map_err(|error| MakeError::Refused(error.to_string()))?;
            remap(&mut props, maps, found);
            remap(&mut content, maps, found);
            let nested = build(Some(block.id), children, document, maps, found, depth + 1)?;
            let value = json!({
                "id": identity::block(maps.import, document, block.id),
                "type": block.block_type,
                "props": props,
                "content": content,
                "children": nested,
            });
            out.push(
                serde_json::from_value::<NewBlock>(value).map_err(|error| {
                    MakeError::Refused(format!("a block cannot be made: {error}"))
                })?,
            );
        }
        Ok(out)
    }
    build(None, &children, document, maps, found, 0)
}

/// The operations that put top-level blocks one after the other at the top of a document.
fn creation_ops(blocks: Vec<NewBlock>) -> Vec<DocumentOp> {
    let mut previous = None;
    blocks
        .into_iter()
        .map(|block| {
            let id = block.id;
            let op = DocumentOp::Insert {
                block,
                parent_id: None,
                after_id: previous,
            };
            previous = Some(id);
            op
        })
        .collect()
}

fn cut_text(text: &str, most: usize) -> String {
    text.chars()
        .take(most)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// Reads the record of a document of a package.
async fn read_package(batch: &Batch<'_>, object: &Object) -> Result<Read, MakeError> {
    let Some(path) = object.object.path.as_deref() else {
        return refuse("the package has no record of this document");
    };
    let bytes = batch
        .plan
        .opened
        .entries
        .read(path, RECORD_MAX_BYTES)
        .await
        .map_err(|error| MakeError::Refused(error.to_string()))?;
    if let Some(expected) = &object.object.sha256
        && hex::encode(<sha2::Sha256 as sha2::Digest>::digest(&bytes)) != *expected
    {
        return Err(MakeError::Damaged(path.to_owned()));
    }
    let value: Json = serde_json::from_slice(&bytes)
        .map_err(|error| MakeError::Refused(format!("its record cannot be read: {error}")))?;
    let blocks: Vec<DocumentBlock> = serde_json::from_value(
        value
            .get("blocks")
            .cloned()
            .unwrap_or_else(|| Json::Array(Vec::new())),
    )
    .map_err(|error| MakeError::Refused(format!("its blocks cannot be read: {error}")))?;
    let document = value.get("document");
    let title = document
        .and_then(|each| each.get("title"))
        .and_then(Json::as_str)
        .unwrap_or(&object.object.name)
        .to_owned();
    let description = document
        .and_then(|each| each.get("description"))
        .and_then(Json::as_str)
        .unwrap_or(&object.object.description)
        .to_owned();
    // The files the record lists, with the entries of the archive that hold their bytes.
    let by_id: HashMap<Id, &orch_api_contract::package::PackageFile> = object
        .object
        .files
        .iter()
        .filter_map(|file| file.id.map(|id| (id, file)))
        .collect();
    let by_name: HashMap<&str, &orch_api_contract::package::PackageFile> = object
        .object
        .files
        .iter()
        .map(|file| (file.name.as_str(), file))
        .collect();
    let mut files = Vec::new();
    let listed = document
        .and_then(|each| each.get("files"))
        .and_then(Json::as_array)
        .cloned()
        .unwrap_or_default();
    for file in &listed {
        let (Some(id), Some(name)) = (
            file.get("id")
                .and_then(Json::as_str)
                .and_then(|text| text.parse::<Id>().ok()),
            file.get("name").and_then(Json::as_str),
        ) else {
            continue;
        };
        let Some(entry) = by_id.get(&id).or_else(|| by_name.get(name)) else {
            continue;
        };
        files.push(FileFacts {
            id,
            name: name.to_owned(),
            media_type: file
                .get("media_type")
                .and_then(Json::as_str)
                .unwrap_or("application/octet-stream")
                .to_owned(),
            is_text: file.get("is_text").and_then(Json::as_bool).unwrap_or(false),
            entry: entry.path.clone(),
        });
    }
    let discussions = value
        .get("discussions")
        .and_then(Json::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(Read {
        title,
        description,
        creator: document
            .and_then(|value| value.get("created_by"))
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok()),
        updater: document
            .and_then(|value| value.get("updated_by"))
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok()),
        blocks: Body::Records(blocks),
        files,
        embedded: Vec::new(),
        discussions,
        warnings: Vec::new(),
    })
}

/// What a text of plain files finds beside it: the documents its links lead to and the files its pictures show, by the path as the text
/// wrote it. Nobody is known by name: an `@` in such a text stays the words it was.
#[derive(Default)]
struct Beside {
    documents: HashMap<String, Typed>,
    files: HashMap<String, FileRef>,
}

impl Names for Beside {
    fn local(&self, path: &str) -> Option<Local> {
        self.documents
            .get(path)
            .cloned()
            .map(Local::Document)
            .or_else(|| self.files.get(path).cloned().map(Local::File))
    }
}

/// The code a loss of the report has for a sentence of the reader of documents.
fn loss_code(reader: &str) -> &'static str {
    if reader.ends_with("_missing") || reader == "reader.image_remote" {
        "missing_file"
    } else if reader.starts_with("reader.link_") {
        "references_outside"
    } else {
        "unsupported_block"
    }
}

/// Reads a document of plain files: Markdown, HTML, Word or text, by the one converter of documents, with the paths it writes
/// resolved against the source it came with.
async fn read_plain(batch: &Batch<'_>, object: &Object) -> Result<Read, MakeError> {
    let Some(plain) = batch.plan.opened.plain.get(&object.object.id) else {
        return refuse("this file is not one that can be read as a document");
    };
    let bytes = batch
        .plan
        .opened
        .entries
        .read(&plain.entry, RECORD_MAX_BYTES)
        .await
        .map_err(|error| MakeError::Refused(error.to_string()))?;
    let prepared = convert::prepare(plain::format_of(plain.format), &bytes)
        .map_err(|error| MakeError::Refused(error.to_string()))?;
    let document = object.target;
    let mut beside = Beside::default();
    let mut files: Vec<FileFacts> = Vec::new();
    let mut names: HashSet<String> = HashSet::new();
    for named in plain::resolve(&batch.plan.opened, &plain.entry, &prepared) {
        if let Some(source) = named.document {
            // A link to another text of the source is a link to the document that text becomes, where it is made.
            if let Some(target) = batch.maps.targets.get(&source) {
                beside.documents.insert(
                    named.written,
                    Typed {
                        kind: InlineRefKind::Document,
                        id: *target,
                        path: None,
                        part: None,
                    },
                );
            }
            continue;
        }
        if !batch.staged.contains_key(&named.entry) {
            continue;
        }
        let source_id = identity::path(Id::nil(), &named.entry);
        let id = identity::file(batch.import, document, source_id);
        let leaf = named
            .entry
            .rsplit('/')
            .next()
            .unwrap_or(&named.entry)
            .to_owned();
        let media_type = orch_documents::files::media_type_of(&leaf, None);
        if !files.iter().any(|each| each.id == source_id) {
            // Two files of different folders may share a name; a document holds one of each name.
            let mut name = leaf.clone();
            let mut number = 2_u32;
            while !names.insert(name.to_lowercase()) {
                name = format!("{number}-{leaf}");
                number += 1;
            }
            files.push(FileFacts {
                id: source_id,
                name,
                media_type: media_type.clone(),
                is_text: media_type.starts_with("text/"),
                entry: named.entry.clone(),
            });
        }
        beside
            .files
            .insert(named.written, FileRef { id, media_type });
    }
    let finished = prepared.finish(&beside);
    let warnings = finished
        .warnings
        .iter()
        .map(|warning| (loss_code(&warning.code).to_owned(), warning.message.clone()))
        .collect();
    Ok(Read {
        creator: None,
        updater: None,
        title: object.object.name.clone(),
        description: String::new(),
        blocks: Body::Fresh(finished.blocks),
        files,
        embedded: finished.embedded,
        discussions: Vec::new(),
        warnings,
    })
}

/// Keeps the files a text carried inside it as files of the document, under the identifiers its blocks name them by.
async fn put_embedded(
    tx: &mut Tx,
    batch: &Batch<'_>,
    document: Id,
    embedded: &[Embedded],
    made: &mut Made,
) -> Result<(), MakeError> {
    if embedded.is_empty() {
        return Ok(());
    }
    let Some(store) = batch.engine.blobs() else {
        made.warn(
            "missing_file",
            "the pictures the text carries inside it were left out: no store keeps files here",
        );
        return Ok(());
    };
    for each in embedded {
        let hash = Hash::of(&each.bytes);
        let size = i64::try_from(each.bytes.len()).unwrap_or(i64::MAX);
        let content: ByteStream = futures::stream::once(std::future::ready(Ok::<_, BlobError>(
            bytes::Bytes::from(each.bytes.clone()),
        )))
        .boxed();
        if let Err(error) = store.put(&hash, Some(size), content).await {
            made.warn(
                "missing_file",
                format!("the file {} could not be kept: {error}", each.name),
            );
            continue;
        }
        storage::register_blob(tx, &hash, size, batch.now).await?;
        if storage::pin_blobs(tx, &[hash]).await? != 1 {
            continue;
        }
        rows::add_file(
            tx,
            &rows::NewFile {
                id: each.id,
                document,
                blob: hash,
                name: &each.name,
                media_type: &each.media_type,
                size,
                is_text: false,
                added_by: Some(batch.principal.user()),
                added_by_thread: None,
                at: batch.now,
            },
        )
        .await?;
        made.carried.files += 1;
    }
    Ok(())
}

/// Puts the files the plan names in the document, from the files the batch put in the store.
async fn put_files(
    tx: &mut Tx,
    batch: &Batch<'_>,
    document: Id,
    files: &[FileFacts],
    made: &mut Made,
) -> Result<HashMap<Id, Id>, MakeError> {
    let mut mapping = HashMap::new();
    for file in files {
        let Some(Staged { blob, size }) = batch.staged.get(&file.entry).copied() else {
            made.warn(
                "missing_file",
                format!("the file {} could not be read from the package", file.name),
            );
            continue;
        };
        if storage::pin_blobs(tx, &[blob]).await? != 1 {
            made.warn(
                "missing_file",
                format!("the file {} is no longer in the store", file.name),
            );
            continue;
        }
        let fresh = identity::file(batch.import, document, file.id);
        rows::add_file(
            tx,
            &rows::NewFile {
                id: fresh,
                document,
                blob,
                name: &file.name,
                media_type: &file.media_type,
                size,
                is_text: file.is_text,
                added_by: Some(batch.principal.user()),
                added_by_thread: None,
                at: batch.now,
            },
        )
        .await?;
        mapping.insert(file.id, fresh);
        made.carried.files += 1;
    }
    Ok(mapping)
}

/// Makes a document, or brings the new content into the one the plan says is there.
#[allow(
    clippy::too_many_lines,
    reason = "one document: its header, its files, its blocks, its description, its times and its origin, in the order they depend on one another"
)]
pub(super) async fn make_document(
    tx: &mut Tx,
    batch: &Batch<'_>,
    object: &Object,
) -> Result<Made, MakeError> {
    let importer = batch.principal.user();
    // What an earlier import wrote of it, if it is the same record: nothing to do.
    if object.action == ImportAction::Replace
        && let Some(existing) = &object.existing
        && let Some(origin) = &existing.origin
        && origin.source_sha256.is_some()
        && origin.source_sha256 == object.object.sha256
    {
        note_origin(tx, batch, object, existing.id, Some(object.lies_in), false).await?;
        let mut made = Made::done(object, existing.id);
        made.outcome = ImportOutcome::Unchanged;
        return Ok(made);
    }
    let read = if batch.plan.opened.plain.contains_key(&object.object.id) {
        read_plain(batch, object).await?
    } else {
        read_package(batch, object).await?
    };
    let source_creator = read
        .creator
        .as_ref()
        .and_then(|author| match author {
            orch_api_contract::documents::Author::User(user) => Some(user.id),
            _ => None,
        })
        .or(object.object.creator);
    let credit = batch
        .plan
        .credits
        .author(read.creator.as_ref(), source_creator);
    let updated_credit = batch
        .plan
        .credits
        .author(read.updater.as_ref(), source_creator);
    let document = object.target;
    let settings = crate::tools::edit::settings_at(tx, object.lies_in).await?;
    let mut rules = settings.rules;
    rules.window_secs = crate::document_tree::Options::for_engine(batch.engine).version_window_secs;

    // Where it goes, and under what title.
    let parent = match object.container {
        Some(container) => Directory::Container(container),
        None => Directory::NodeDocs(object.lies_in),
    };
    let mut title = cut_text(&read.title, TITLE_MAX);
    if title.is_empty() {
        title = "Untitled".to_owned();
    }
    let replacing = object.action == ImportAction::Replace;
    if object.action == ImportAction::KeepBoth || (!object.parent_made && !replacing) {
        title = settle_name(
            tx,
            PackageKind::Document,
            object.lies_in,
            object.container,
            &title,
            TITLE_MAX,
        )
        .await?;
    }
    if replacing {
        // The document that is there keeps its identifier, its place and its sharing; what it said is kept as a version.
        let Some(row) = rows::lock(tx, document).await? else {
            return refuse("the document that was there is gone");
        };
        if row.deleted_at.is_some() {
            return refuse("the document that was there is gone");
        }
        let holds =
            orch_authz::documents::listed(tx, batch.principal, &[document], Policy::default())
                .await?
                .iter()
                .any(|listed| listed.holds(Permission::DocEdit));
        if !holds {
            return refuse("you may not change the document that is there");
        }
        history::cut(tx, &row, VersionCause::Rewrite, batch.now).await?;
        if row.title != title {
            // What the source calls it now is what it is called here, and the version just cut keeps the name it had.
            rows::rename(
                tx,
                document,
                &title,
                &Writer::User(updated_credit.user),
                batch.now,
            )
            .await?;
        }
        let current = rows::live_blocks(tx, document).await?;
        let tops: Vec<DocumentOp> = current
            .iter()
            .filter(|block| block.parent_id.is_none())
            .map(|block| DocumentOp::Remove { id: block.id })
            .collect();
        let writer = Writer::User(importer);
        for chunk in tops.chunks(
            usize::try_from(DOCUMENT_OPS_MAX)
                .unwrap_or(usize::MAX)
                .max(1),
        ) {
            apply(tx, document, &writer, chunk, &settings, rules, batch).await?;
        }
        // Its files go with what said them: the new ones are put in below.
        let held = rows::files(tx, document).await?;
        for file in held {
            rows::remove_file(tx, document, file.id).await?;
        }
    } else {
        let created = crate::document_tree::create_text_in(
            tx,
            Actor::Person(batch.principal),
            parent,
            document,
            &title,
            None,
            Time::new(batch.now),
        )
        .await?;
        if !created.created {
            // A retry of what was made: it stands.
            note_origin(tx, batch, object, document, Some(object.lies_in), true).await?;
            return Ok(Made::done(object, document));
        }
    }

    // What it names is read through one map, once.
    let mut made = Made::done(object, document);
    for (code, message) in &read.warnings {
        made.warn(code, message.clone());
    }
    let files = put_files(tx, batch, document, &read.files, &mut made).await?;
    put_embedded(tx, batch, document, &read.embedded, &mut made).await?;
    let mut found = Found::default();
    let mut combined: HashMap<Id, Id> = batch.maps.files.clone();
    combined.extend(files);
    let maps = Maps {
        import: batch.import,
        targets: &batch.maps.targets,
        chats: &batch.maps.chats,
        files: &combined,
        gone: &batch.maps.gone,
        held: &batch.maps.held,
        credits: &batch.plan.credits,
    };
    let blocks = match &read.blocks {
        Body::Records(records) => tree_of(records, document, &maps, &mut found)?,
        Body::Fresh(fresh) => fresh.clone(),
    };
    let writer = Writer::User(credit.user);
    let ops = creation_ops(blocks);
    for chunk in ops.chunks(
        usize::try_from(DOCUMENT_OPS_MAX)
            .unwrap_or(usize::MAX)
            .max(1),
    ) {
        apply(tx, document, &writer, chunk, &settings, rules, batch).await?;
    }
    if let Body::Records(records) = &read.blocks {
        credit_blocks(
            tx,
            batch,
            document,
            records,
            &maps,
            credit.user,
            source_creator,
        )
        .await?;
    }
    if found.degraded > 0 {
        made.warn(
            "references_outside",
            format!(
                "{} references to things that did not come are left as words",
                found.degraded
            ),
        );
    }
    let description = cut_text(&read.description, DESCRIPTION_MAX);
    if !description.is_empty() {
        rows::describe(tx, document, &description, &writer, batch.now).await?;
    }
    // The times it had there are the times it has here.
    sqlx::query(
        "UPDATE documents SET created_at = COALESCE($2, created_at), updated_at = COALESCE($3, updated_at),
                created_by = CASE WHEN $4 THEN created_by ELSE $5 END, updated_by = $6 WHERE id = $1",
    )
    .bind(document)
    .bind(
        (!replacing)
            .then_some(object.object.created_at)
            .flatten()
            .map(|at| at.get()),
    )
    .bind(object.object.updated_at.map(|at| at.get()))
    .bind(replacing)
    .bind(credit.user)
    .bind(updated_credit.user)
    .execute(&mut **tx)
    .await
    .map_err(DbError::from)?;
    let mut audit = AuditEntry::new(
        AuditActor::User(importer),
        orch_db::audit::action!(AuditAction::DocumentCreated).as_str(),
        "document",
        Some(document),
        "A document was made by an import.",
    );
    audit.node_id = Some(object.lies_in);
    audit.org_id = Some(batch.plan.destination.org);
    audit.details = json!({ "title": title, "import": batch.import, "replaced": replacing });
    append(tx, &audit, &orch_db::audit::Origin::default(), batch.now).await?;
    note_origin(
        tx,
        batch,
        object,
        document,
        Some(object.lies_in),
        !replacing,
    )
    .await?;
    let mut originals: BTreeSet<String> = credit
        .original
        .into_iter()
        .chain(updated_credit.original)
        .collect();
    if let Body::Records(records) = &read.blocks {
        originals.extend(records.iter().filter_map(|block| {
            batch
                .plan
                .credits
                .author(block.updated_by.as_ref(), source_creator)
                .original
        }));
    }
    made.author =
        (!originals.is_empty()).then(|| originals.into_iter().collect::<Vec<_>>().join("; "));
    // Its discussions and its versions follow, where they are asked for and the package has them. A document that was there
    // keeps its own history, to which the cut of what it said before has been added.
    if batch.plan.wants.comments && !read.discussions.is_empty() {
        super::comments::bring(
            tx,
            batch,
            object,
            document,
            &read.discussions,
            &maps,
            &mut made,
        )
        .await?;
    }
    if !replacing {
        super::versions::bring(tx, batch, object, document, &maps, rules, &mut made).await?;
        super::access::restore(tx, batch, object, document, &mut made).await?;
    }
    Ok(made)
}

/// Restores per-block attribution in bounded set-based pages after ordinary authorized block creation.
async fn credit_blocks(
    tx: &mut Tx,
    batch: &Batch<'_>,
    document: Id,
    records: &[DocumentBlock],
    maps: &Maps<'_>,
    creator: Id,
    source_creator: Option<Id>,
) -> Result<(), MakeError> {
    let mut authors = Vec::new();
    let mut seen = HashSet::new();
    for block in records.iter().filter(|block| !block.removed) {
        let writer = Writer::User(
            batch
                .plan
                .credits
                .author(block.updated_by.as_ref(), source_creator)
                .user,
        );
        if seen.insert(writer.clone()) {
            authors.push(writer);
        }
    }
    if let Some(row) = rows::get(tx, document).await? {
        let mut span = row.span();
        let mut held: HashSet<Writer> = span.authors.iter().cloned().collect();
        for writer in authors {
            if held.insert(writer.clone()) {
                span.authors.push(writer);
            }
        }
        rows::keep_span(tx, document, &span).await?;
    }
    for page in records.chunks(usize::try_from(DOCUMENT_OPS_MAX).unwrap_or(500).max(1)) {
        let credits: Vec<Json> = page.iter().filter(|block| !block.removed).map(|block| {
            let credit = batch.plan.credits.author(block.updated_by.as_ref(), source_creator);
            json!({"id": identity::block(maps.import, document, block.id), "user_id": credit.user, "at": block.updated_at.get()})
        }).collect();
        sqlx::query("UPDATE document_blocks b SET created_by=$3, updated_by=c.user_id, updated_by_thread=NULL, updated_by_agent=NULL, updated_at=c.at
            FROM jsonb_to_recordset($2) AS c(id uuid,user_id uuid,at timestamptz)
            WHERE b.document_id=$1 AND b.id=c.id")
            .bind(document).bind(json!(credits)).bind(creator).execute(&mut **tx).await.map_err(DbError::from)?;
    }
    Ok(())
}

/// Applies a chunk of operations under its mapped author, and refuses a document that would pass what it may hold.
async fn apply(
    tx: &mut Tx,
    document: Id,
    writer: &Writer,
    ops: &[DocumentOp],
    settings: &crate::tools::edit::Settings,
    rules: batch::Rules,
    batch_: &Batch<'_>,
) -> Result<(), MakeError> {
    match batch::apply(
        tx,
        document,
        writer,
        None,
        ops,
        settings.limits,
        rules,
        batch_.now,
    )
    .await?
    {
        batch::Outcome::Applied(_) | batch::Outcome::Repeated { .. } => Ok(()),
        _ => refuse("it would pass what a document may hold here"),
    }
}
