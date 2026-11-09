//! Retained agent outputs use the host's standing blob transfer before their metadata is published.

use crate::content::Content;
use orch_agent_contract::discovery::{RECORD_MAX_BYTES, RecordedEntry};
use orch_agent_contract::events::ToolCallPayload;
use orch_envhost::blobs::BlobSink as _;
use orch_link_contract::commands::{CommandError, ErrorCode};
use orch_link_contract::events::Event;
use orch_link_contract::foreign::ItemPayload;
use orch_types::Hash;
use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

fn tool_hashes(call: &ToolCallPayload, hashes: &mut BTreeSet<Hash>) {
    hashes.extend(call.output_blobs());
}

/// Only declared byte references count; opaque tool arguments and results grant no file access.
pub(super) fn referenced(event: &Event) -> BTreeSet<Hash> {
    let mut hashes = BTreeSet::new();
    match event {
        Event::ItemStarted(item) => {
            if let ItemPayload::ToolCall(call) = &item.payload {
                tool_hashes(call, &mut hashes);
            }
        }
        Event::ItemCompleted(item) => {
            if let ItemPayload::ToolCall(call) = &item.payload {
                tool_hashes(call, &mut hashes);
            }
            hashes.extend(item.output_blob.as_ref().map(|blob| blob.hash));
        }
        _ => {}
    }
    hashes
}

/// Uses the same resumable, hash-checked transfer as standing shell and tree outputs. Cache
/// files are retained; they are not temporary export archives owned by the upload operation.
pub(crate) async fn keep(
    content: &std::sync::Arc<Content>,
    directory: Option<&Path>,
    event: &Event,
) -> Result<(), CommandError> {
    keep_hashes(content, directory, referenced(event)).await
}

/// The same output path for typed dependencies in a native adoption record, before its metadata is published.
pub(crate) async fn keep_record(
    content: &std::sync::Arc<Content>,
    directory: Option<&Path>,
    record: &Path,
) -> Result<(), CommandError> {
    let record = record.to_owned();
    let hashes = tokio::task::spawn_blocking(move || record_hashes(&record))
        .await
        .map_err(|_| {
            CommandError::new(ErrorCode::Internal, "the native record could not be read")
        })??;
    // The common transfer client splits this deduplicated set into its declared wire batches.
    keep_hashes(content, directory, hashes).await
}

fn record_hashes(record: &Path) -> Result<BTreeSet<Hash>, CommandError> {
    let file = std::fs::File::open(record).map_err(|error| {
        CommandError::new(
            ErrorCode::Internal,
            format!("the native record could not be read: {error}"),
        )
    })?;
    let maximum = u64::try_from(RECORD_MAX_BYTES)
        .map_err(|error| CommandError::new(ErrorCode::Internal, error.to_string()))?;
    if file
        .metadata()
        .map_err(|error| CommandError::new(ErrorCode::Internal, error.to_string()))?
        .len()
        > maximum
    {
        return Err(CommandError::new(
            ErrorCode::Internal,
            "the native record exceeds its declared format budget",
        ));
    }
    let mut hashes = BTreeSet::new();
    for line in BufReader::new(file.take(maximum)).lines() {
        let line =
            line.map_err(|error| CommandError::new(ErrorCode::Internal, error.to_string()))?;
        let Ok(RecordedEntry::Item(item)) = serde_json::from_str::<RecordedEntry>(&line) else {
            continue;
        };
        if let orch_agent_contract::events::ItemContent::ToolCall(call) = item.content {
            hashes.extend(call.output_blobs());
        }
    }
    Ok(hashes)
}

async fn keep_hashes(
    content: &std::sync::Arc<Content>,
    directory: Option<&Path>,
    hashes: BTreeSet<Hash>,
) -> Result<(), CommandError> {
    let hashes: Vec<_> = hashes.into_iter().collect();
    if hashes.is_empty() {
        return Ok(());
    }
    let missing = content
        .standing_blobs()
        .missing(&hashes)
        .await
        .map_err(|error| {
            CommandError::new(
                ErrorCode::Internal,
                format!("retained outputs could not be checked: {error}"),
            )
        })?;
    if missing.is_empty() {
        return Ok(());
    }
    let directory = directory.ok_or_else(|| {
        CommandError::new(
            ErrorCode::Internal,
            "the adapter has not supplied its retained output cache",
        )
    })?;
    for hash in missing {
        content
            .put_expected(
                crate::content::STANDING_COMMAND,
                &directory.join(hash.to_string()),
                hash,
            )
            .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn opaque_mcp_arguments_cannot_name_a_file_for_upload() -> Result<(), Box<dyn std::error::Error>>
    {
        let hidden = Hash::of(b"not an output");
        let actual = Hash::of(b"the full raw output");
        let event = Event::decode(
            "item.completed",
            json!({
                "native_ref":"mcp", "status":"completed", "payload":{
                    "tool":{"kind":"mcp","server":"fixture","tool":"read","arguments":{"blob":hidden},"result":{"blob":hidden},"error":null},
                    "title":"Read","started_at":"2026-01-01T00:00:00Z","duration_ms":1,
                    "raw":{"name":"mcp","input":{"inline":"{}","total_len":2,"truncated":false,"blob":null},
                        "output":{"inline":"tail","total_len":100,"truncated":true,"blob":actual},"is_error":false}
                }
            }),
        )?;
        assert_eq!(referenced(&event), BTreeSet::from([actual]));
        Ok(())
    }
}
