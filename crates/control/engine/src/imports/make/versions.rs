//! The earlier versions of a document: each read from its record in the package and written as a version of the document that was made, under the identifiers its blocks have here, so that putting one back changes the blocks that are there and not a second set of them.

use super::documents::RECORD_MAX_BYTES;
use super::{Batch, Made};
use crate::imports::identity;
use crate::imports::model::Object;
use crate::imports::remap::{Found, Maps, remap};
use chrono::{DateTime, Utc};
use orch_api_contract::documents::{BlockContent, BlockType, DocumentBlock};
use orch_db::Tx;
use orch_db::queries::documents::{batch as batch_rows, history};
use orch_db::queries::imports::{self as rows, NewVersion};
use orch_documents::blocks::{content_from_json, plain_text};
use orch_documents::versions::Writer;
use orch_types::{Id, Json};
use serde_json::json;
use std::collections::{BTreeSet, HashSet};

/// What a version of the package says of itself.
struct Record {
    title: String,
    cut_at: Option<DateTime<Utc>>,
    blocks: Vec<DocumentBlock>,
    authors: Vec<Writer>,
}

fn read(bytes: &[u8]) -> Result<Record, String> {
    let value: Json = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    let blocks: Vec<DocumentBlock> = serde_json::from_value(
        value
            .get("blocks")
            .cloned()
            .unwrap_or_else(|| Json::Array(Vec::new())),
    )
    .map_err(|error| error.to_string())?;
    Ok(Record {
        title: value
            .get("title")
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_owned(),
        cut_at: value
            .get("cut_at")
            .and_then(|at| serde_json::from_value::<orch_types::Time>(at.clone()).ok())
            .map(orch_types::Time::get),
        blocks,
        authors: value
            .get("authors")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
            .filter_map(Writer::from_json)
            .collect(),
    })
}

/// The snapshot of a version as the rows of its blocks: what [`history::snapshot`] reads back.
fn snapshot_of(
    blocks: &[DocumentBlock],
    document: Id,
    at: DateTime<Utc>,
    maps: &Maps<'_>,
    found: &mut Found,
    creator: Option<Id>,
) -> (Json, i64) {
    let live: Vec<&DocumentBlock> = blocks.iter().filter(|block| !block.removed).collect();
    let known: HashSet<Id> = live.iter().map(|block| block.id).collect();
    let mut bytes = 0_i64;
    let rows: Vec<Json> = live
        .iter()
        .map(|block| {
            let mut props = serde_json::to_value(&block.props).unwrap_or(Json::Null);
            let mut content = serde_json::to_value(&block.content).unwrap_or(Json::Null);
            remap(&mut props, maps, found);
            remap(&mut content, maps, found);
            let kind = serde_json::to_value(block.block_type)
                .ok()
                .and_then(|word| word.as_str().map(str::to_owned))
                .unwrap_or_else(|| "paragraph".to_owned());
            let text = plain_text(block_type_of(&kind), &props, &content_of(&content));
            bytes += i64::try_from(text.len()).unwrap_or(i64::MAX);
            let credit = maps.credits.author(block.updated_by.as_ref(), creator);
            json!({
                "id": identity::block(maps.import, document, block.id),
                "document_id": document,
                "parent_id": block
                    .parent_id
                    .filter(|parent| known.contains(parent))
                    .map(|parent| identity::block(maps.import, document, parent)),
                "position": block.position,
                "block_type": kind,
                "props": props,
                "content": content,
                "text": text,
                "seq_created": 1,
                "seq_updated": 1,
                "created_by": maps.credits.of(creator).user,
                "updated_by": credit.user,
                "updated_by_thread": null,
                "updated_by_agent": null,
                "created_at": at,
                "updated_at": block.updated_at.get(),
                "removed_at": null,
            })
        })
        .collect();
    (Json::Array(rows), bytes)
}

fn block_type_of(word: &str) -> BlockType {
    serde_json::from_value(Json::String(word.to_owned())).unwrap_or(BlockType::Paragraph)
}

fn content_of(content: &Json) -> BlockContent {
    content_from_json(content)
}

/// Version writers are metadata, never an authority used for the import's writes.
fn mapped_authors(
    writers: &[Writer],
    credits: &crate::imports::people::Credits,
    fallback: Option<Id>,
) -> Json {
    let mut mapped = Vec::new();
    let mut seen = HashSet::new();
    for writer in writers {
        let user = match writer {
            Writer::User(user) => credits.of(Some(*user)).user,
            Writer::Agent { .. } => credits.of(None).user,
        };
        if seen.insert(user) {
            mapped.push(Writer::User(user).to_json());
        }
    }
    if mapped.is_empty() {
        mapped.push(Writer::User(credits.of(fallback).user).to_json());
    }
    Json::Array(mapped)
}

/// Writes the versions the package keeps of a document that was made. A version whose record is damaged or unreadable is left out
/// with a warning, and the others are kept. Returns how many were written.
pub(super) async fn bring(
    tx: &mut Tx,
    batch: &Batch<'_>,
    object: &Object,
    document: Id,
    maps: &Maps<'_>,
    rules: batch_rows::Rules,
    made: &mut Made,
) -> Result<usize, super::MakeError> {
    if !batch.plan.wants.versions || object.object.versions.is_empty() {
        return Ok(0);
    }
    let mut found = Found::default();
    let mut written = 0_usize;
    let mut highest = 0_i32;
    let mut highest_at = batch.now;
    let mut versions: Vec<_> = object.object.versions.iter().collect();
    versions.sort_by_key(|version| version.number);
    let mut originals: BTreeSet<String> = made.author.iter().cloned().collect();
    for version in versions {
        let Ok(number) = i32::try_from(version.number) else {
            continue;
        };
        if number < 1 {
            continue;
        }
        let bytes = match batch
            .plan
            .opened
            .entries
            .read(&version.path, RECORD_MAX_BYTES)
            .await
        {
            Ok(bytes) => bytes,
            Err(error) => {
                made.warn(
                    "missing_file",
                    format!("version {number} could not be read from the package: {error}"),
                );
                continue;
            }
        };
        if hex::encode(<sha2::Sha256 as sha2::Digest>::digest(&bytes)) != version.sha256 {
            made.warn(
                "missing_file",
                format!("version {number} does not match its checksum and was left out"),
            );
            continue;
        }
        let record = match read(&bytes) {
            Ok(record) => record,
            Err(error) => {
                made.warn(
                    "unsupported_block",
                    format!("version {number} cannot be read and was left out: {error}"),
                );
                continue;
            }
        };
        originals.extend(record.authors.iter().filter_map(|writer| match writer {
            Writer::User(user) => batch.plan.credits.of(Some(*user)).original,
            Writer::Agent { agent, .. } => Some(agent.clone()),
        }));
        originals.extend(record.blocks.iter().filter_map(|block| {
            batch
                .plan
                .credits
                .author(block.updated_by.as_ref(), object.object.creator)
                .original
        }));
        let at = record.cut_at.unwrap_or(version.cut_at.get());
        let (snapshot, size) = snapshot_of(
            &record.blocks,
            document,
            at,
            maps,
            &mut found,
            object.object.creator,
        );
        let authors = mapped_authors(&record.authors, &batch.plan.credits, object.object.creator);
        let title: String = record.title.chars().take(512).collect();
        let put = rows::put_version(
            tx,
            &NewVersion {
                document,
                version: number,
                title: &title,
                snapshot: &snapshot,
                bytes: size,
                authors: &authors,
                at,
            },
        )
        .await?;
        if put {
            written += 1;
            if number > highest {
                highest = number;
                highest_at = at;
            }
        }
    }
    made.author =
        (!originals.is_empty()).then(|| originals.into_iter().collect::<Vec<_>>().join("; "));
    if highest > 0 {
        rows::raise_version_counter(tx, document, highest, highest_at).await?;
        history::prune(tx, document, rules.kept, rules.history_max_bytes).await?;
    }
    if found.degraded > 0 {
        made.warn(
            "references_outside",
            format!(
                "{} references in earlier versions to things that did not come are left as words",
                found.degraded
            ),
        );
    }
    Ok(written)
}
