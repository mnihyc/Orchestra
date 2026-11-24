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
    cursor: Cursor,
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
    page_from_ids(conn, thread, cursor, &ids, limit, remaining, admit_first).await
}

async fn page_from_ids(
    conn: &mut Conn,
    thread: Id,
    mut cursor: Cursor,
    ids: &[Id],
    limit: u32,
    remaining: &mut usize,
    admit_first: bool,
) -> Result<Page, ReadError> {
    let mut result = Page {
        inputs: Vec::new(),
        next: Vec::new(),
    };
    for &id in ids {
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
    items: &[orch_api_contract::threads::Item],
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
    let anchors: Vec<_> = before_all
        .then_some((None, None))
        .into_iter()
        .chain(
            items
                .iter()
                .filter(|item| item.stream == "main")
                .map(|item| (Some(item.id), Some(item.seq_created))),
        )
        .collect();
    if anchors.is_empty() {
        return Ok(result);
    }
    // Share the usual key page across the visible anchors. A full group gets its own
    // indexed page on demand, so a wide screen never preloads limit squared keys.
    let prefix = limit.div_ceil(u32::try_from(anchors.len()).unwrap_or(u32::MAX));
    let mut lower_bounds: Vec<_> = anchors.iter().map(|(_, seq)| *seq).collect();
    lower_bounds.sort_unstable();
    lower_bounds.dedup();
    let view = projection.viewed_continuation_id;
    let groups =
        rows::window_keys(conn, thread, view, at, &lower_bounds, i64::from(prefix) + 1).await?;
    for (anchor, lower) in anchors {
        let Some(group) = groups.get(&lower) else {
            continue;
        };
        let admit = admit_first && result.inputs.is_empty();
        let full;
        let ids = if prefix < limit
            && group.ids.len() > usize::try_from(prefix).unwrap_or(usize::MAX)
            && (*remaining > 0 || admit)
        {
            full = rows::group_ids(
                conn,
                thread,
                view,
                lower,
                group.upper,
                None,
                at,
                i64::from(limit) + 1,
            )
            .await?;
            &full
        } else {
            &group.ids
        };
        let page = page_from_ids(
            conn,
            thread,
            Cursor {
                projection: projection.clone(),
                at,
                anchor,
                after: None,
            },
            ids,
            limit,
            remaining,
            admit,
        )
        .await?;
        result.inputs.extend(page.inputs);
        result.next.extend(page.next);
    }
    Ok(result)
}
