//! Cancelled authored inputs keep their native-independent identity and page by their item anchor.

use super::{EngineError, People, ReadError, input_view};
use orch_api_contract::threads::{
    RetainedInput, RetainedInputContinuation, ThreadContinuationProjection,
};
use orch_db::queries::thread_continuations::retained as rows;
use orch_db::{Conn, queries::threads};
use orch_types::{Id, Seq};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Cursor {
    pub projection: ThreadContinuationProjection,
    pub at: Seq,
    pub anchor: Option<Id>,
    pub after: Option<Id>,
}

pub(super) struct Page {
    pub inputs: Vec<RetainedInput>,
    pub next: Vec<RetainedInputContinuation>,
}

pub(super) fn bytes(value: &impl Serialize) -> Result<usize, ReadError> {
    Ok(serde_json::to_vec(value).map_err(EngineError::from)?.len())
}

pub(super) fn byte_target() -> usize {
    usize::try_from(orch_db::queries::thread_history::PAGE_BYTES).unwrap_or(usize::MAX)
}

pub(super) fn decode(text: &str) -> Result<Cursor, ReadError> {
    serde_json::from_str(text)
        .map_err(|_| ReadError::BadRequest("the retained-input cursor is invalid"))
}

fn continuation(cursor: &Cursor) -> Result<RetainedInputContinuation, ReadError> {
    Ok(RetainedInputContinuation {
        after_item: cursor.anchor,
        cursor: serde_json::to_string(cursor).map_err(EngineError::from)?,
    })
}

pub(super) async fn project(
    conn: &mut Conn,
    input: &threads::InputRow,
    anchor: Option<Id>,
) -> Result<RetainedInput, ReadError> {
    let people = People::load(conn, input.author_id.into_iter().chain(input.cancelled_by)).await?;
    let row = input_view(input, &people)?;
    Ok(RetainedInput {
        id: row.id,
        continuation_id: input.continuation_id,
        author: row.author,
        parts: row.content,
        native_action: row.native_action,
        submitted_at: row.created_at,
        outcome: row.cancellation,
        after_item: anchor,
        seq_created: input.seq_created,
        seq_updated: input.seq_updated,
    })
}

/// The same whole-record buffer policy as native item pages: the first record may exceed the target.
pub(super) async fn group(
    conn: &mut Conn,
    thread: Id,
    mut cursor: Cursor,
    limit: u32,
    remaining: &mut usize,
    admit_first: bool,
) -> Result<Page, ReadError> {
    let view = cursor.projection.viewed_continuation_id;
    let lower = match cursor.anchor {
        Some(anchor) => Some(
            rows::item_seq(conn, thread, view, anchor)
                .await?
                .ok_or(ReadError::NotFound)?,
        ),
        None => None,
    };
    let upper = rows::next_item_seq(conn, thread, view, lower).await?;
    if let Some(after) = cursor.after {
        let input = rows::input(conn, thread, after)
            .await?
            .ok_or(ReadError::NotFound)?;
        if input.seq_updated > cursor.at
            || rows::anchor(conn, thread, view, after).await? != cursor.anchor
        {
            return Err(ReadError::BadRequest(
                "the retained-input cursor belongs to another anchor",
            ));
        }
    }
    let ids = rows::group_ids(
        conn,
        thread,
        view,
        lower,
        upper,
        cursor.after,
        cursor.at,
        i64::from(limit) + 1,
    )
    .await?;
    let mut result = Page {
        inputs: Vec::new(),
        next: Vec::new(),
    };
    for id in ids {
        if result.inputs.len() >= usize::try_from(limit).unwrap_or(usize::MAX)
            || (*remaining == 0 && !(admit_first && result.inputs.is_empty()))
        {
            result.next.push(continuation(&cursor)?);
            break;
        }
        let input = rows::input(conn, thread, id)
            .await?
            .ok_or(ReadError::NotFound)?;
        let input = project(conn, &input, cursor.anchor).await?;
        // Charge the record and its JSON separator as part of the shared array allowance.
        let bytes = serde_json::to_vec(&input)
            .map_err(EngineError::from)?
            .len()
            .saturating_add(1);
        if bytes > *remaining && !(admit_first && result.inputs.is_empty()) {
            result.next.push(continuation(&cursor)?);
            break;
        }
        *remaining = remaining.saturating_sub(bytes);
        cursor.after = Some(input.id);
        result.inputs.push(input);
    }
    Ok(result)
}

/// A page walks only already-authorized anchors; each group's continuation remains with that same anchor.
pub(super) struct Window<'a> {
    pub thread: Id,
    pub projection: &'a ThreadContinuationProjection,
    pub at: Seq,
    pub before_all: bool,
    pub limit: u32,
    pub admit_first: bool,
}

pub(super) async fn window(
    conn: &mut Conn,
    window: Window<'_>,
    anchors: Vec<Id>,
    remaining: &mut usize,
) -> Result<Page, ReadError> {
    let Window {
        thread,
        projection,
        at,
        before_all,
        limit,
        admit_first,
    } = window;
    let mut result = Page {
        inputs: Vec::new(),
        next: Vec::new(),
    };
    // Most history pages have no cancelled authored inputs. Use the same visible/frozen
    // stream and sequence bounds as a group before inspecting individual item anchors.
    if (!before_all && anchors.is_empty())
        || rows::group_ids(
            conn,
            thread,
            projection.viewed_continuation_id,
            None,
            None,
            None,
            at,
            1,
        )
        .await?
        .is_empty()
    {
        return Ok(result);
    }
    let anchors = before_all
        .then_some(None)
        .into_iter()
        .chain(anchors.into_iter().map(Some));
    for anchor in anchors {
        if let Some(anchor) = anchor {
            // Child streams do not own main-conversation authored inputs.
            if rows::item_seq(conn, thread, projection.viewed_continuation_id, anchor)
                .await?
                .is_none()
            {
                continue;
            }
        }
        let page = group(
            conn,
            thread,
            Cursor {
                projection: projection.clone(),
                at,
                anchor,
                after: None,
            },
            limit,
            remaining,
            admit_first && result.inputs.is_empty(),
        )
        .await?;
        result.inputs.extend(page.inputs);
        result.next.extend(page.next);
    }
    Ok(result)
}
