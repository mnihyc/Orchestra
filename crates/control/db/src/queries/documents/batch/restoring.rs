//! Native version material is prepared before the original history/block writer starts.

use super::*;
use crate::queries::documents::{self as rows, DocumentRow};

/// Owned native restore operands. The caller keeps the document lock until application ends.
/// This contains material, not an authorization grant or a replacement transaction guard.
pub struct Restoration {
    state: RestorationState,
}

enum RestorationState {
    Unchanged(Restored),
    Change(Box<Change>),
}
struct Change {
    doc: DocumentRow,
    writer: Writer,
    gone: Vec<Id>,
    steps: Vec<BlockRow>,
    existing: HashSet<Id>,
    touched: Vec<Id>,
    block_changes: BlockChanges,
    tally: Tally,
    limits: Limits,
    rules: Rules,
}

/// Select the original version/current material and calculate its exact changes without writing.
pub async fn prepare_restoration(
    conn: &mut Conn,
    document: Id,
    version: i32,
    writer: &Writer,
    limits: Limits,
    rules: Rules,
) -> Result<Restoration, DbError> {
    let Some(doc) = rows::lock(conn, document)
        .await?
        .filter(|doc| doc.deleted_at.is_none())
    else {
        return Ok(Restoration {
            state: RestorationState::Unchanged(Restored::Gone),
        });
    };
    let Some(wanted) = history::snapshot(conn, document, version).await? else {
        return Ok(Restoration {
            state: RestorationState::Unchanged(Restored::NoSuchVersion),
        });
    };
    let live = rows::live_blocks(conn, document).await?;
    let wanted_ids: Vec<Id> = wanted.iter().map(|row| row.id).collect();
    let existing: HashMap<Id, BlockRow> = rows::blocks_by_ids(conn, document, &wanted_ids)
        .await?
        .into_iter()
        .map(|row| (row.id, row))
        .collect();
    let wanted_set: HashSet<Id> = wanted_ids.iter().copied().collect();
    let mut block_changes = BlockChanges::default();
    let mut touched: Vec<Id> = Vec::new();

    // What would change, worked out before anything is: a version put back that
    // changes nothing keeps nothing and cuts nothing.
    let gone: Vec<Id> = live
        .iter()
        .filter(|row| !wanted_set.contains(&row.id))
        .map(|row| row.id)
        .collect();
    block_changes.removed.extend(&gone);
    touched.extend(&gone);
    // Parents before the blocks under them, so that every row finds its parent.
    let by_id: HashMap<Id, &BlockRow> = wanted.iter().map(|row| (row.id, row)).collect();
    let mut ordered: Vec<&BlockRow> = wanted.iter().collect();
    ordered.sort_by_key(|row| depth_in(&by_id, row.id));
    let mut steps: Vec<BlockRow> = Vec::new();
    for row in ordered {
        match existing.get(&row.id) {
            Some(now_row) if now_row.removed_at.is_none() => {
                let moved = !same_place(now_row, row);
                let changed = !same_content(now_row, row);
                if !moved && !changed {
                    continue;
                }
                if moved {
                    block_changes.moved.insert(row.id);
                }
                if changed {
                    block_changes.updated.insert(row.id);
                }
            }
            Some(_) | None => {
                block_changes.inserted.insert(row.id);
            }
        }
        touched.push(row.id);
        steps.push(row.clone());
    }
    let tally = block_changes.tally();
    if touched.is_empty() {
        return Ok(Restoration {
            state: RestorationState::Unchanged(Restored::Done(Applied {
                seq: doc.doc_seq,
                touched: Vec::new(),
                named: Vec::new(),
                tally: Tally::default(),
                cut: None,
                changed: false,
            })),
        });
    }

    Ok(Restoration {
        state: RestorationState::Change(Box::new(Change {
            doc,
            writer: writer.clone(),
            gone,
            steps,
            existing: existing.into_keys().collect(),
            touched,
            block_changes,
            tally,
            limits,
            rules,
        })),
    })
}

/// Apply the same version writer, checking immediately before its first history/block mutation.
/// Genuine no-change/absent outcomes invoke no first-effect callback.
pub async fn apply_restoration<E, F>(
    conn: &mut Conn,
    prepared: Restoration,
    now: DateTime<Utc>,
    before: F,
) -> Result<Result<Restored, DbError>, E>
where
    F: FnOnce() -> Result<(), E>,
{
    let change = match prepared.state {
        RestorationState::Change(change) => change,
        RestorationState::Unchanged(outcome) => return Ok(Ok(outcome)),
    };
    before()?;
    Ok(write_restoration(conn, *change, now).await)
}

async fn write_restoration(
    conn: &mut Conn,
    change: Change,
    now: DateTime<Utc>,
) -> Result<Restored, DbError> {
    let Change {
        doc,
        writer,
        gone,
        steps,
        existing,
        touched,
        block_changes,
        tally,
        limits,
        rules,
    } = change;
    let document = doc.id;
    let seq = doc.doc_seq + 1;
    let (user, thread, agent) = stamped_by(&writer);
    let mut span = doc.span();
    let mut cut = None;
    if !span.is_empty() {
        history::cut(conn, &doc, VersionCause::Restore, now).await?;
        history::prune(conn, document, rules.kept, rules.history_max_bytes).await?;
        span = Span::default();
        cut = Some(VersionCause::Restore);
    }
    if !gone.is_empty() {
        sqlx::query(
            "UPDATE document_blocks SET removed_at = $3, seq_updated = $4, updated_by = $5,
                    updated_by_thread = $6, updated_by_agent = $7, updated_at = $3
              WHERE document_id = $1 AND id = ANY($2) AND removed_at IS NULL",
        )
        .bind(document)
        .bind(&gone)
        .bind(now)
        .bind(seq)
        .bind(user)
        .bind(thread)
        .bind(agent)
        .execute(&mut *conn)
        .await?;
    }
    for row in steps {
        if existing.contains(&row.id) {
            sqlx::query(
                "UPDATE document_blocks SET parent_id = $2, position = $3::text::numeric, type = $4, props = $5,
                        content = $6, text = $7, removed_at = NULL, seq_updated = $8, updated_by = $9,
                        updated_by_thread = $10, updated_by_agent = $11, updated_at = $12
                  WHERE id = $1",
            )
            .bind(row.id)
            .bind(row.parent_id)
            .bind(&row.position)
            .bind(&row.block_type)
            .bind(&row.props)
            .bind(&row.content)
            .bind(&row.text)
            .bind(seq)
            .bind(user)
            .bind(thread)
            .bind(agent)
            .bind(now)
            .execute(&mut *conn)
            .await?;
        } else {
            sqlx::query(
                "INSERT INTO document_blocks
                    (id, document_id, parent_id, position, type, props, content, text, seq_created, seq_updated,
                     created_by, updated_by, updated_by_thread, updated_by_agent, created_at, updated_at)
                 VALUES ($1, $2, $3, $4::text::numeric, $5, $6, $7, $8, $9, $9, $10, $10, $11, $12, $13, $13)",
            )
            .bind(row.id)
            .bind(document)
            .bind(row.parent_id)
            .bind(&row.position)
            .bind(&row.block_type)
            .bind(&row.props)
            .bind(&row.content)
            .bind(&row.text)
            .bind(seq)
            .bind(user)
            .bind(thread)
            .bind(agent)
            .bind(now)
            .execute(&mut *conn)
            .await?;
        }
    }
    span.record(&writer, &block_changes, orch_types::Time::new(now));
    sqlx::query(
        "UPDATE documents SET doc_seq = $2, seq_updated = $2, updated_at = $3, updated_by = $4, span = $5
          WHERE id = $1",
    )
    .bind(document)
    .bind(seq)
    .bind(now)
    .bind(user)
    .bind(span.to_json())
    .execute(&mut *conn)
    .await?;
    let (blocks, bytes) = rows::recount(conn, document).await?;
    if blocks > limits.max_blocks {
        return Ok(Restored::TooManyBlocks {
            limit: limits.max_blocks,
        });
    }
    if bytes > limits.max_bytes {
        return Ok(Restored::TooLarge {
            limit: limits.max_bytes,
        });
    }
    crate::queries::hierarchy::touch_activity(conn, doc.node_id, now).await?;
    rows::notify(conn, document, seq).await?;
    let touched = rows::blocks_by_ids(conn, document, &touched).await?;
    Ok(Restored::Done(Applied {
        seq,
        touched,
        named: Vec::new(),
        tally,
        cut,
        changed: true,
    }))
}
