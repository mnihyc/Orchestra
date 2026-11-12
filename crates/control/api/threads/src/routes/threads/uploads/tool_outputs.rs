//! Declared tool output references use the same authorized conversation blob reader as images.

use crate::middleware::requests::ApiResult;

use futures::TryStreamExt;

use futures::future::BoxFuture;

use orch_api_contract::threads::ToolCallPayload;

use orch_api_contract::threads::ToolPayload;

use orch_db::Conn;

use orch_types::Hash;

use orch_types::Id;

use orch_types::Json;
type Description = (String, Option<String>);

fn described(payload: &Json, hash: &Hash) -> Option<Description> {
    let tool = payload.get("tool")?;
    // Preserve the existing image description, including older optional media/name metadata.
    if tool.get("kind").and_then(Json::as_str) == Some("image")
        && tool.get("blob").and_then(Json::as_str) == Some(hash.to_string().as_str())
    {
        return Some((
            tool.get("name")
                .and_then(Json::as_str)
                .unwrap_or("image")
                .to_owned(),
            tool.get("media_type")
                .and_then(Json::as_str)
                .map(str::to_owned),
        ));
    }
    let call: ToolCallPayload = serde_json::from_value(payload.clone()).ok()?;
    let name = if call.raw.input.blob.as_ref() == Some(hash) {
        "tool-input.txt"
    } else if call
        .raw
        .output
        .as_ref()
        .and_then(|output| output.blob.as_ref())
        == Some(hash)
    {
        "tool-output.txt"
    } else {
        let output = match &call.tool {
            ToolPayload::Command { output, .. } => output,
            ToolPayload::FileRead { content, .. } | ToolPayload::WebFetch { content, .. } => {
                content
            }
            _ => return None,
        };
        if output.blob.as_ref() != Some(hash) {
            return None;
        }
        "tool-output.txt"
    };
    Some((name.to_owned(), Some("text/plain".to_owned())))
}

pub(super) fn in_thread(
    conn: &mut Conn,
    thread: Id,
    hash: Hash,
) -> BoxFuture<'_, ApiResult<Option<Description>>> {
    Box::pin(async move {
        // Keep the existing image path independent of when its bytes first arrived in the store.
        let image: Option<Json> = sqlx::query_scalar(
            "SELECT payload FROM items WHERE thread_id=$1 AND kind='tool_call'
             AND payload->'tool'->>'kind'='image' AND payload->'tool'->>'blob'=$2 LIMIT 1",
        )
        .bind(thread)
        .bind(hash.to_string())
        .fetch_optional(&mut *conn)
        .await
        .map_err(orch_db::DbError::from)?;
        if let Some(image) = image
            && let Some(description) = described(&image, &hash)
        {
            return Ok(Some(description));
        }
        // The existing reference index bounds candidates without scanning a thread's JSON history.
        // It includes opaque JSON too, so validate a typed reference before granting read authority.
        let mut candidates = sqlx::query_scalar::<_, Json>(
            "SELECT i.payload FROM thread_item_blob_refs r JOIN items i ON i.id=r.item_id
             WHERE i.thread_id=$1 AND i.kind='tool_call' AND r.blob=$2",
        )
        .bind(thread)
        .bind(hash.as_bytes().as_slice())
        .fetch(&mut *conn);
        while let Some(payload) = candidates
            .try_next()
            .await
            .map_err(orch_db::DbError::from)?
        {
            if let Some(description) = described(&payload, &hash) {
                return Ok(Some(description));
            }
        }
        Ok(None)
    })
}
