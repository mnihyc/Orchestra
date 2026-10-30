//! Current document material copied through ordinary fresh blocks and independently pinned files.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_api_contract::documents::{BlockContent, BlockProps, BlockType, NewBlock};
use orch_types::{Hash, Id, new_id};
use serde_json::json;
use std::collections::HashMap;

/// Why current document material could not be copied.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Its stored material could not be read or written.
    #[error(transparent)]
    Db(#[from] DbError),
    /// Stored block material does not match its normal wire types.
    #[error(transparent)]
    Content(#[from] serde_json::Error),
    /// A referenced file's bytes disappeared before they could be pinned.
    #[error("a copied file is no longer available")]
    MissingFile,
    /// The source disappeared before its current snapshot could be locked.
    #[error("the source document is no longer available")]
    MissingSource,
}

/// Current text and files, with fresh block and file identities and no access, author or history copy.
#[derive(Debug)]
pub struct Plan {
    /// Normal creation blocks.
    pub blocks: Vec<NewBlock>,
    /// Files with their new identifiers and the same current bytes and names.
    pub files: Vec<super::HeldFile>,
    /// Original-to-fresh block identities for a whole-tree copy.
    pub block_ids: HashMap<Id, Id>,
    /// Original-to-fresh attached-file identities.
    pub file_ids: HashMap<Id, Id>,
}

/// A stable per-person creation identity, domain-separated and length-framed before hashing.
#[must_use]
pub fn identity(user: Id, destination: Id, client: &str, entry: &str) -> Id {
    let mut hash = Hash::hasher();
    for part in [
        b"document-material-v1".as_slice(),
        user.as_bytes(),
        destination.as_bytes(),
        client.as_bytes(),
        entry.as_bytes(),
    ] {
        hash.update(&u64::try_from(part.len()).unwrap_or(u64::MAX).to_le_bytes());
        hash.update(part);
    }
    let digest = Hash::finish(&hash);
    // An Id is 128 bits; the collision resistance of its derived identity is the same as a UUID.
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest.as_bytes()[..16]);
    Id::from_bytes(bytes)
}

/// Reads one current source; the caller holds its ordinary View and destination creation guard.
pub async fn plan(conn: &mut Conn, source: Id, destination: Id) -> Result<Plan, Error> {
    let mut plan = plan_raw(conn, source).await?;
    let mut value = serde_json::to_value(&plan.blocks)?;
    rewrite_anchors(&mut value, &plan.block_ids, source, destination);
    plan.blocks = serde_json::from_value(value)?;
    Ok(plan)
}

async fn plan_raw(conn: &mut Conn, source: Id) -> Result<Plan, Error> {
    if super::lock(conn, source)
        .await?
        .is_none_or(|document| document.deleted_at.is_some())
    {
        return Err(Error::MissingSource);
    }
    let blocks = super::live_blocks(conn, source).await?;
    let raw: Vec<super::RawHeld> = sqlx::query_as(
        "SELECT id,name,media_type,size,is_text,added_by,added_by_thread,added_at,blob FROM document_files WHERE document_id=$1 ORDER BY name,id",
    ).bind(source).fetch_all(conn).await.map_err(DbError::from)?;
    let mut files: Vec<super::HeldFile> = raw
        .into_iter()
        .map(super::HeldFile::try_from)
        .collect::<Result<_, _>>()?;
    let mut file_ids = HashMap::new();
    for file in &mut files {
        let id = new_id();
        file_ids.insert(file.file.id, id);
        file.file.id = id;
    }
    let block_ids: HashMap<Id, Id> = blocks.iter().map(|block| (block.id, new_id())).collect();
    let mut children: HashMap<Option<Id>, Vec<usize>> = HashMap::new();
    for (index, block) in blocks.iter().enumerate() {
        children.entry(block.parent_id).or_default().push(index);
    }
    fn fresh(
        rows: &[super::BlockRow],
        children: &HashMap<Option<Id>, Vec<usize>>,
        files: &HashMap<Id, Id>,
        ids: &HashMap<Id, Id>,
        parent: Option<Id>,
    ) -> Result<Vec<NewBlock>, serde_json::Error> {
        children
            .get(&parent)
            .into_iter()
            .flatten()
            .map(|index| {
                let row = &rows[*index];
                let block_type: BlockType = serde_json::from_value(json!(row.block_type))?;
                let mut props = row.props.clone();
                if let Some(file) = props
                    .get("file_id")
                    .and_then(|id| id.as_str())
                    .and_then(|id| id.parse::<Id>().ok())
                    .and_then(|id| files.get(&id))
                {
                    props["file_id"] = json!(file);
                }
                let props: BlockProps = serde_json::from_value(props)?;
                let content: BlockContent = serde_json::from_value(row.content.clone())?;
                Ok(NewBlock {
                    id: ids[&row.id],
                    block_type,
                    props: Some(props),
                    content: Some(content),
                    children: Some(fresh(rows, children, files, ids, Some(row.id))?),
                })
            })
            .collect()
    }
    let blocks = fresh(&blocks, &children, &file_ids, &block_ids, None)?;
    Ok(Plan {
        blocks,
        files,
        block_ids,
        file_ids,
    })
}

/// Copies current material with one identity map shared by every selected document.
pub async fn plan_tree(conn: &mut Conn, pairs: &[(Id, Id)]) -> Result<HashMap<Id, Plan>, Error> {
    let documents: HashMap<_, _> = pairs.iter().copied().collect();
    let mut plans = HashMap::new();
    let mut blocks = HashMap::new();
    let mut files = HashMap::new();
    let mut ordered = pairs.to_vec();
    ordered.sort_unstable();
    for (source, _) in ordered {
        let material = plan_raw(conn, source).await?;
        blocks.extend(material.block_ids.iter().map(|(old, new)| (*old, *new)));
        files.extend(material.file_ids.iter().map(|(old, new)| (*old, *new)));
        plans.insert(source, material);
    }
    for (source, material) in &mut plans {
        let mut value = serde_json::to_value(&material.blocks)?;
        rewrite_tree_refs(&mut value, &documents, &blocks, &files);
        if let Some(destination) = documents.get(source) {
            rewrite_anchors(&mut value, &material.block_ids, *source, *destination);
        }
        material.blocks = serde_json::from_value(value)?;
    }
    Ok(plans)
}

fn rewrite_tree_refs(
    value: &mut orch_types::Json,
    documents: &HashMap<Id, Id>,
    blocks: &HashMap<Id, Id>,
    files: &HashMap<Id, Id>,
) {
    match value {
        orch_types::Json::Array(values) => {
            for value in values {
                rewrite_tree_refs(value, documents, blocks, files)
            }
        }
        orch_types::Json::Object(object) => {
            if object.get("type").and_then(|value| value.as_str()) == Some("ref")
                && object.get("kind").and_then(|value| value.as_str()) == Some("document")
                && let Some(id) = object
                    .get("id")
                    .and_then(|value| value.as_str())
                    .and_then(|id| id.parse::<Id>().ok())
                    .and_then(|id| documents.get(&id))
            {
                object.insert("id".into(), json!(id));
                // The block a reference names was copied with its document: it names the copy's block.
                if let Some(block) = block_of_part(object).and_then(|block| blocks.get(&block)) {
                    object.insert("part".into(), json!({ "block": block }));
                }
            }
            if let Some(id) = object
                .get("file_id")
                .and_then(|value| value.as_str())
                .and_then(|id| id.parse::<Id>().ok())
                .and_then(|id| files.get(&id))
            {
                object.insert("file_id".into(), json!(id));
            }
            if let Some(orch_types::Json::String(href)) = object.get_mut("href") {
                for (source, destination) in documents {
                    for prefix in ["/doc/", "/documents/"] {
                        let old = format!("{prefix}{source}");
                        if href.strip_prefix(&old).is_some_and(|tail| {
                            tail.is_empty() || tail.starts_with(['?', '#', '/'])
                        }) {
                            href.replace_range(..old.len(), &format!("{prefix}{destination}"));
                            for (old, new) in files {
                                let path = format!("/files/{old}");
                                if let Some(at) = href.find(&path) {
                                    let end = at + path.len();
                                    if href.get(end..).is_some_and(|tail| {
                                        tail.is_empty() || tail.starts_with(['?', '#', '/'])
                                    }) {
                                        href.replace_range(at..end, &format!("/files/{new}"));
                                    }
                                }
                            }
                            for marker in ["?block=", "&block=", "#block="] {
                                if let Some(at) = href.find(marker) {
                                    let start = at + marker.len();
                                    let end = href[start..]
                                        .find(['&', '#'])
                                        .map_or(href.len(), |n| start + n);
                                    if let Some(id) = href[start..end]
                                        .parse::<Id>()
                                        .ok()
                                        .and_then(|id| blocks.get(&id))
                                    {
                                        href.replace_range(start..end, &id.to_string());
                                    }
                                }
                            }
                        }
                    }
                }
            }
            for value in object.values_mut() {
                rewrite_tree_refs(value, documents, blocks, files);
            }
        }
        _ => {}
    }
}

/// The block a stored reference names as its part, where it names one.
fn block_of_part(object: &serde_json::Map<String, orch_types::Json>) -> Option<Id> {
    object
        .get("part")
        .and_then(|part| part.get("block"))
        .and_then(|value| value.as_str())
        .and_then(|block| block.parse::<Id>().ok())
}

// Only block anchors are internal identities. External node/document references retain their
// ordinary ids and are still filtered by the normal reader; arbitrary authored text is not rewritten.
fn rewrite_anchors(
    value: &mut orch_types::Json,
    ids: &HashMap<Id, Id>,
    source: Id,
    destination: Id,
) {
    match value {
        orch_types::Json::Array(items) => {
            for item in items {
                rewrite_anchors(item, ids, source, destination);
            }
        }
        orch_types::Json::Object(object) => {
            if object.get("type").and_then(|value| value.as_str()) == Some("ref")
                && object.get("kind").and_then(|value| value.as_str()) == Some("document")
                && object
                    .get("id")
                    .and_then(|value| value.as_str())
                    .and_then(|value| value.parse::<Id>().ok())
                    == Some(source)
            {
                object.insert("id".to_owned(), json!(destination));
                if let Some(block) = block_of_part(object).and_then(|block| ids.get(&block)) {
                    object.insert("part".to_owned(), json!({ "block": block }));
                }
            }
            if let Some(orch_types::Json::String(href)) = object.get_mut("href") {
                let self_path = format!("/doc/{source}");
                let own_page = href.split(['?', '#']).next() == Some(self_path.as_str());
                if own_page || href.starts_with(['?', '#']) {
                    for prefix in ["?block=", "&block=", "#block="] {
                        if let Some(at) = href.find(prefix) {
                            let begin = at + prefix.len();
                            let end = href[begin..]
                                .find(['&', '#'])
                                .map_or(href.len(), |length| begin + length);
                            if let Some(fresh) = href[begin..end]
                                .parse::<Id>()
                                .ok()
                                .and_then(|old| ids.get(&old))
                            {
                                href.replace_range(begin..end, &fresh.to_string());
                            }
                        }
                    }
                    if own_page {
                        href.replace_range(..self_path.len(), &format!("/doc/{destination}"));
                    }
                }
            }
            for item in object.values_mut() {
                rewrite_anchors(item, ids, source, destination);
            }
        }
        _ => {}
    }
}

/// Installs a plan's files through the normal file producer, with independent pins and fresh writers.
pub async fn install_files(
    conn: &mut Conn,
    destination: Id,
    files: &[super::HeldFile],
    user: Option<Id>,
    thread: Option<Id>,
    at: DateTime<Utc>,
) -> Result<(), Error> {
    for held in files {
        if crate::queries::storage::pin_blobs(conn, &[held.blob]).await? != 1 {
            return Err(Error::MissingFile);
        }
        let file = &held.file;
        super::add_file(
            conn,
            &super::NewFile {
                id: file.id,
                document: destination,
                blob: held.blob,
                name: &file.name,
                media_type: &file.media_type,
                size: file.size,
                is_text: file.is_text,
                added_by: user,
                added_by_thread: thread,
                at,
            },
        )
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_internal_anchors_and_self_references_are_rewritten() {
        let source = new_id();
        let destination = new_id();
        let old = new_id();
        let fresh = new_id();
        let mut content = json!([
            {"type":"text","text":old.to_string(),"href":format!("/doc/{source}?block={old}")},
            {"type":"text","text":"external","href":format!("https://elsewhere.test/doc/{source}?block={old}")},
            {"type":"ref","kind":"document","id":source},
        ]);
        rewrite_anchors(
            &mut content,
            &HashMap::from([(old, fresh)]),
            source,
            destination,
        );
        assert_eq!(content[0]["text"], old.to_string());
        assert_eq!(
            content[0]["href"],
            format!("/doc/{destination}?block={fresh}")
        );
        assert_eq!(
            content[1]["href"],
            format!("https://elsewhere.test/doc/{source}?block={old}")
        );
        assert_eq!(content[2]["id"], json!(destination));
    }

    #[test]
    fn a_whole_copy_remaps_cross_document_blocks_and_files_without_touching_external_material() {
        let (a, b, new_a, new_b, block, new_block, file, new_file) = (
            new_id(),
            new_id(),
            new_id(),
            new_id(),
            new_id(),
            new_id(),
            new_id(),
            new_id(),
        );
        let outside = new_id();
        let mut content = json!([
            {"type":"ref","kind":"document","id":b},
            {"type":"text","text":b.to_string(),"href":format!("/doc/{b}?block={block}")},
            {"type":"text","text":"self file","href":format!("/documents/{a}/files/{file}")},
            {"type":"text","text":"external","href":format!("https://example.test/doc/{b}?block={block}")},
            {"type":"ref","kind":"document","id":outside},
            {"type":"text","text":"relative","href":format!("?block={block}")},
        ]);
        rewrite_tree_refs(
            &mut content,
            &HashMap::from([(a, new_a), (b, new_b)]),
            &HashMap::from([(block, new_block)]),
            &HashMap::from([(file, new_file)]),
        );
        rewrite_anchors(&mut content, &HashMap::from([(block, new_block)]), a, new_a);
        assert_eq!(content[0]["id"], json!(new_b));
        assert_eq!(content[1]["text"], b.to_string());
        assert_eq!(
            content[1]["href"],
            format!("/doc/{new_b}?block={new_block}")
        );
        assert_eq!(
            content[2]["href"],
            format!("/documents/{new_a}/files/{new_file}")
        );
        assert_eq!(
            content[3]["href"],
            format!("https://example.test/doc/{b}?block={block}")
        );
        assert_eq!(content[4]["id"], json!(outside));
        assert_eq!(content[5]["href"], format!("?block={new_block}"));
    }
}
