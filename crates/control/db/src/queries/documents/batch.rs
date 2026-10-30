//! A batch of changes applied to a document's rows, and a version put back: whole, under the lock on the document's row, or not at all.
//!
//! The batch is applied to the tree of the document in memory by the rules of
//! `orch-documents`, and when every operation has been, its effects are
//! written: one new sequence for the document, the blocks it touched and the
//! tally of what was changed and by whom, with a version cut first when the
//! clock or the batch says so. Every outcome but [`Outcome::Applied`] and
//! [`Outcome::Repeated`] leaves the caller's transaction with nothing worth
//! committing: it is to be dropped.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use orch_api_contract::documents::{BlockType, DOCUMENT_OPS_MAX, DocumentOp, VersionCause};
use orch_documents::blocks::{self, Checked, Fault};
use orch_documents::operations::{self, Effects, Node, Refusal, Tree};
use orch_documents::position::Position;
use orch_documents::versions::{self, BlockChanges, Span, Tally, Writer};
use orch_types::{Id, Seq};

use super::{BlockRow, history, stamped_by};
use crate::{Conn, DbError};

/// What a document may hold, as resolved at its node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// The most bytes of all its blocks together.
    pub max_bytes: i64,
    /// The most live blocks.
    pub max_blocks: i64,
}

/// When versions are cut, and how many are kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rules {
    /// A version is cut before a batch when the last was cut longer ago than this.
    pub window_secs: i64,
    /// The most versions kept.
    pub kept: i64,
    /// The most bytes the kept versions come to.
    pub history_max_bytes: i64,
}

/// What a batch did.
#[derive(Debug, Clone)]
pub struct Applied {
    /// The document's sequence after it: the one before when nothing changed.
    pub seq: Seq,
    /// The blocks it touched, as they stand.
    pub touched: Vec<BlockRow>,
    /// The people named in what it wrote, once each.
    pub named: Vec<Id>,
    /// How many blocks of each kind.
    pub tally: Tally,
    /// The version that was cut before it, and why.
    pub cut: Option<VersionCause>,
    /// Whether anything changed.
    pub changed: bool,
}

/// How a batch ended.
#[derive(Debug, Clone)]
pub enum Outcome {
    /// There is no such document, or it was deleted.
    Gone,
    /// The batch was applied before: this is the sequence the first time led to.
    Repeated {
        /// The sequence.
        seq: Seq,
    },
    /// The batch was applied.
    Applied(Applied),
    /// Blocks it builds on were changed or removed since: those blocks, as they stand.
    Stale {
        /// The blocks.
        current: Vec<BlockRow>,
    },
    /// A block it makes has an identifier in use.
    Exists {
        /// The identifier.
        id: Id,
    },
    /// Something in it is wrong.
    Invalid(Vec<Fault>),
    /// One block would be too large.
    BlockTooLarge {
        /// Where.
        path: String,
        /// How large.
        bytes: i64,
    },
    /// A block would be nested deeper than a block may be.
    TooDeep {
        /// Where.
        path: String,
    },
    /// The document would have more blocks than it may.
    TooManyBlocks {
        /// What it may.
        limit: i64,
    },
    /// The document's text would be larger than it may be.
    TooLarge {
        /// What it may.
        limit: i64,
    },
}

impl Outcome {
    /// Whether the transaction holds a change that is worth committing.
    #[must_use]
    pub fn is_kept(&self) -> bool {
        matches!(self, Self::Applied(_) | Self::Repeated { .. })
    }
}

fn parse_type(word: &str) -> Option<BlockType> {
    serde_json::from_value(serde_json::Value::String(word.to_owned())).ok()
}

/// A stored block as the rules of a batch see it, with its props and content.
fn checked_of(row: &BlockRow) -> Option<Checked> {
    let block_type = parse_type(&row.block_type)?;
    let content = blocks::content_from_json(&row.content);
    let bytes = serde_json::to_string(&row.props).map_or(0, |text| text.len())
        + serde_json::to_string(&row.content).map_or(0, |text| text.len());
    Some(Checked {
        block_type,
        props: row.props.clone(),
        content,
        text: row.text.clone(),
        bytes: i64::try_from(bytes).unwrap_or(i64::MAX),
    })
}

fn node_of(row: &BlockRow, with_data: bool) -> Option<Node> {
    Some(Node {
        id: row.id,
        parent: row.parent_id,
        position: Position::parse(&row.position)?,
        block_type: parse_type(&row.block_type)?,
        seq_updated: row.seq_updated,
        removed: row.removed_at.is_some(),
        checked: if with_data { checked_of(row) } else { None },
    })
}

/// The tree a batch is applied to: the document's live blocks, and the rows
/// the batch names, removed ones among them, with the data of those it updates.
async fn load_tree(
    conn: &mut Conn,
    document: Id,
    wanted: &operations::References,
) -> Result<Tree, DbError> {
    let skeleton: Vec<(Id, Option<Id>, String, String, i64)> = sqlx::query_as(
        "SELECT id, parent_id, position::text, type, seq_updated FROM document_blocks
          WHERE document_id = $1 AND removed_at IS NULL",
    )
    .bind(document)
    .fetch_all(&mut *conn)
    .await?;
    let mut nodes: HashMap<Id, Node> = HashMap::new();
    for (id, parent_id, position, block_type, seq_updated) in skeleton {
        if let (Some(position), Some(block_type)) =
            (Position::parse(&position), parse_type(&block_type))
        {
            nodes.insert(
                id,
                Node {
                    id,
                    parent: parent_id,
                    position,
                    block_type,
                    seq_updated,
                    removed: false,
                    checked: None,
                },
            );
        }
    }
    let mut tree = Tree::new(nodes.into_values().collect());
    supply_named(conn, document, wanted, &mut tree).await?;
    Ok(tree)
}

/// Read only this chunk's named rows; the preceding chunks already updated the retained skeleton.
async fn supply_named(
    conn: &mut Conn,
    document: Id,
    wanted: &operations::References,
    tree: &mut Tree,
) -> Result<(), DbError> {
    let named = super::blocks_by_ids(conn, document, &wanted.named).await?;
    let to_update: HashSet<Id> = wanted.updated.iter().copied().collect();
    for row in named {
        if row.removed_at.is_some() {
            if let Some(node) = node_of(&row, false) {
                tree.supply(node);
            }
        } else if to_update.contains(&row.id)
            && let Some(node) = node_of(&row, true)
        {
            tree.supply(node);
        }
    }
    Ok(())
}

/// Cached only within one call: a retained tree cannot outlive the edit's exclusive connection borrow.
struct EditTree {
    tree: Tree,
    counted: bool,
}

/// Totals for precisely the content-changing identities, including removed descendants.
async fn changed_totals(conn: &mut Conn, document: Id, ids: &[Id]) -> Result<(i64, i64), DbError> {
    Ok(sqlx::query_as(
        "SELECT count(*)::bigint,COALESCE(sum(octet_length(props::text)+octet_length(content::text)),0)::bigint
           FROM document_blocks WHERE id=ANY($2) AND document_id=$1 AND removed_at IS NULL",
    )
    .bind(document)
    .bind(ids)
    .fetch_one(conn)
    .await?)
}

async fn write_effects(
    conn: &mut Conn,
    document: Id,
    writer: &Writer,
    effects: &Effects,
    seq: Seq,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    let (user, thread, agent) = stamped_by(writer);
    if !effects.inserted.is_empty() {
        let ids: Vec<Id> = effects.inserted.iter().map(|w| w.id).collect();
        let parents: Vec<Option<Id>> = effects.inserted.iter().map(|w| w.parent).collect();
        let positions: Vec<String> = effects
            .inserted
            .iter()
            .map(|w| w.position.to_string())
            .collect();
        let types: Vec<String> = effects
            .inserted
            .iter()
            .map(|w| type_word(w.checked.block_type))
            .collect();
        let props: Vec<orch_types::Json> = effects
            .inserted
            .iter()
            .map(|w| w.checked.props.clone())
            .collect();
        let contents: Vec<orch_types::Json> = effects
            .inserted
            .iter()
            .map(|w| w.checked.content_json())
            .collect();
        let texts: Vec<String> = effects
            .inserted
            .iter()
            .map(|w| w.checked.text.clone())
            .collect();
        sqlx::query(
            "INSERT INTO document_blocks
                (id, document_id, parent_id, position, type, props, content, text, seq_created, seq_updated,
                 created_by, updated_by, updated_by_thread, updated_by_agent, created_at, updated_at)
             SELECT t.id, $2, t.parent, t.position::text::numeric, t.type, t.props, t.content, t.text, $3, $3,
                    $4, $4, $5, $6, $7, $7
               FROM UNNEST($1::uuid[], $8::uuid[], $9::text[], $10::text[], $11::jsonb[], $12::jsonb[], $13::text[])
                    AS t(id, parent, position, type, props, content, text)",
        )
        .bind(&ids)
        .bind(document)
        .bind(seq)
        .bind(user)
        .bind(thread)
        .bind(agent)
        .bind(now)
        .bind(&parents)
        .bind(&positions)
        .bind(&types)
        .bind(&props)
        .bind(&contents)
        .bind(&texts)
        .execute(&mut *conn)
        .await?;
    }
    for each in &effects.updated {
        sqlx::query(
            "UPDATE document_blocks SET type = $2, props = $3, content = $4, text = $5, seq_updated = $6,
                    updated_by = $7, updated_by_thread = $8, updated_by_agent = $9, updated_at = $10
              WHERE id = $1",
        )
        .bind(each.id)
        .bind(type_word(each.checked.block_type))
        .bind(&each.checked.props)
        .bind(each.checked.content_json())
        .bind(&each.checked.text)
        .bind(seq)
        .bind(user)
        .bind(thread)
        .bind(agent)
        .bind(now)
        .execute(&mut *conn)
        .await?;
    }
    for each in &effects.moved {
        sqlx::query(
            "UPDATE document_blocks SET parent_id = $2, position = $3::text::numeric, seq_updated = $4,
                    updated_by = $5, updated_by_thread = $6, updated_by_agent = $7, updated_at = $8
              WHERE id = $1",
        )
        .bind(each.id)
        .bind(each.parent)
        .bind(each.position.to_string())
        .bind(seq)
        .bind(user)
        .bind(thread)
        .bind(agent)
        .bind(now)
        .execute(&mut *conn)
        .await?;
    }
    if !effects.removed.is_empty() {
        sqlx::query(
            "UPDATE document_blocks SET removed_at = $3, seq_updated = $4, updated_by = $5,
                    updated_by_thread = $6, updated_by_agent = $7, updated_at = $3
              WHERE document_id = $1 AND id = ANY($2) AND removed_at IS NULL",
        )
        .bind(document)
        .bind(&effects.removed)
        .bind(now)
        .bind(seq)
        .bind(user)
        .bind(thread)
        .bind(agent)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

fn type_word(block_type: BlockType) -> String {
    serde_json::to_value(block_type)
        .ok()
        .and_then(|word| word.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// Applies a batch of operations to a document, as `writer`. `client_id` makes
/// a repeat answer as the first did, for a person's batch. The caller holds a
/// transaction and commits it only for an outcome that [`Outcome::is_kept`].
#[expect(
    clippy::too_many_arguments,
    reason = "a batch is applied under the document's lock with who wrote it, the limits and the clock, which are all different things"
)]
pub async fn apply(
    conn: &mut Conn,
    document: Id,
    writer: &Writer,
    client_id: Option<Id>,
    ops: &[DocumentOp],
    limits: Limits,
    rules: Rules,
    now: DateTime<Utc>,
) -> Result<Outcome, DbError> {
    apply_fenced(
        conn, document, writer, client_id, ops, limits, rules, now, None,
    )
    .await
}

/// Applies ordinary operations against an optional live snapshot, under the same lock as the
/// change. A repeated accepted client identity wins before the fence, including after later edits.
#[expect(
    clippy::too_many_arguments,
    reason = "the existing batch producer plus its optional observed sequence"
)]
pub async fn apply_fenced(
    conn: &mut Conn,
    document: Id,
    writer: &Writer,
    client_id: Option<Id>,
    ops: &[DocumentOp],
    limits: Limits,
    rules: Rules,
    now: DateTime<Utc>,
    expected_seq: Option<Seq>,
) -> Result<Outcome, DbError> {
    let mut cached = None;
    apply_with_tree(
        conn,
        document,
        writer,
        client_id,
        ops,
        limits,
        rules,
        now,
        expected_seq,
        &mut cached,
    )
    .await
}

/// Applies every ordinary-sized chunk in one caller-owned transaction, reading its tree once.
/// Refused outcomes still require rolling back the entire edit, including earlier accepted chunks.
pub async fn apply_chunks(
    conn: &mut Conn,
    document: Id,
    writer: &Writer,
    ops: &[DocumentOp],
    limits: Limits,
    rules: Rules,
    now: DateTime<Utc>,
) -> Result<Outcome, DbError> {
    let mut cached = None;
    let mut first: Option<Applied> = None;
    let most = usize::try_from(DOCUMENT_OPS_MAX).unwrap_or(usize::MAX);
    for chunk in ops.chunks(most) {
        let outcome = apply_with_tree(
            conn,
            document,
            writer,
            None,
            chunk,
            limits,
            rules,
            now,
            None,
            &mut cached,
        )
        .await?;
        match outcome {
            Outcome::Applied(applied) => match first.as_mut() {
                None => first = Some(applied),
                Some(total) => {
                    total.seq = applied.seq;
                    total.touched.extend(applied.touched);
                    total.named.extend(applied.named);
                    total.changed |= applied.changed;
                }
            },
            other => return Ok(other),
        }
    }
    Ok(first.map_or_else(
        || Outcome::Invalid(vec![Fault::at("ops", "There is nothing to change.")]),
        Outcome::Applied,
    ))
}

#[expect(
    clippy::too_many_arguments,
    reason = "one batch's existing context plus its edit-local retained tree"
)]
async fn apply_with_tree(
    conn: &mut Conn,
    document: Id,
    writer: &Writer,
    client_id: Option<Id>,
    ops: &[DocumentOp],
    limits: Limits,
    rules: Rules,
    now: DateTime<Utc>,
    expected_seq: Option<Seq>,
    cached: &mut Option<EditTree>,
) -> Result<Outcome, DbError> {
    let Some(doc) = super::lock(conn, document)
        .await?
        .filter(|doc| doc.deleted_at.is_none())
    else {
        return Ok(Outcome::Gone);
    };
    if let (Writer::User(author), Some(client)) = (writer, client_id) {
        let seen: Option<i64> = sqlx::query_scalar(
            "SELECT seq FROM document_ops WHERE document_id = $1 AND author_id = $2 AND client_id = $3",
        )
        .bind(document)
        .bind(author)
        .bind(client)
        .fetch_optional(&mut *conn)
        .await?;
        if let Some(seq) = seen {
            return Ok(Outcome::Repeated { seq });
        }
    }
    let wanted = operations::references(ops);
    if expected_seq.is_some_and(|observed| observed != doc.doc_seq) {
        return Ok(Outcome::Stale {
            current: super::blocks_by_ids(conn, document, &wanted.named).await?,
        });
    }
    if let Some(taken) = super::ids_in_use(conn, &wanted.made).await?.first() {
        return Ok(Outcome::Exists { id: *taken });
    }
    let mut edit = if let Some(mut edit) = cached.take() {
        supply_named(conn, document, &wanted, &mut edit.tree).await?;
        edit
    } else {
        EditTree {
            tree: load_tree(conn, document, &wanted).await?,
            counted: false,
        }
    };
    let seq = doc.doc_seq + 1;
    let effects = match operations::apply(&mut edit.tree, ops, seq, limits.max_blocks) {
        Ok(effects) => effects,
        Err(refusal) => return refused(conn, document, refusal).await,
    };
    // A file a block names is one the document holds.
    if !effects.files.is_empty() {
        let named: Vec<Id> = effects.files.iter().map(|(_, file)| *file).collect();
        let held = super::files_among(conn, document, &named).await?;
        let missing: Vec<Fault> = effects
            .files
            .iter()
            .filter(|(_, file)| !held.contains(file))
            .map(|(block, file)| {
                Fault::at(
                    "ops",
                    format!(
                        "the block {block} names the file {file}, which the document does not hold"
                    ),
                )
            })
            .collect();
        if !missing.is_empty() {
            return Ok(Outcome::Invalid(missing));
        }
    }
    let replay = match (writer, client_id) {
        (Writer::User(author), Some(client)) => Some((*author, client)),
        _ => None,
    };
    if effects.touched.is_empty() {
        record_batch(conn, document, replay, doc.doc_seq, now).await?;
        edit.tree.discard_data(&wanted.updated);
        *cached = Some(edit);
        return Ok(Outcome::Applied(Applied {
            seq: doc.doc_seq,
            touched: Vec::new(),
            named: Vec::new(),
            tally: Tally::default(),
            cut: None,
            changed: false,
        }));
    }
    let mut span = doc.span();
    let cut = versions::cut_before_a_batch(
        &span,
        doc.version_at.map(orch_types::Time::new),
        orch_types::Time::new(doc.created_at),
        orch_types::Time::new(now),
        rules.window_secs,
        effects.replaces_everything,
    );
    if let Some(cause) = cut {
        history::cut(conn, &doc, cause, now).await?;
        history::prune(conn, document, rules.kept, rules.history_max_bytes).await?;
        span = Span::default();
    }
    let changed_ids: Vec<Id> = effects
        .inserted
        .iter()
        .map(|block| block.id)
        .chain(effects.updated.iter().map(|block| block.id))
        .chain(effects.removed.iter().copied())
        .collect();
    let previous = if edit.counted {
        Some(changed_totals(conn, document, &changed_ids).await?)
    } else {
        None
    };
    write_effects(conn, document, writer, &effects, seq, now).await?;
    let (user, _, _) = stamped_by(writer);
    span.record(writer, &effects.block_changes(), orch_types::Time::new(now));
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
    let bytes = if let Some((before_count, before_bytes)) = previous {
        let (after_count, after_bytes) = changed_totals(conn, document, &changed_ids).await?;
        sqlx::query_scalar(
            "UPDATE documents SET block_count=(block_count::bigint+$2)::int,bytes=bytes+$3
               WHERE id=$1 RETURNING bytes",
        )
        .bind(document)
        .bind(after_count - before_count)
        .bind(after_bytes - before_bytes)
        .fetch_one(&mut *conn)
        .await?
    } else {
        let (_, bytes) = super::recount(conn, document).await?;
        edit.counted = true;
        bytes
    };
    if bytes > limits.max_bytes {
        return Ok(Outcome::TooLarge {
            limit: limits.max_bytes,
        });
    }
    record_batch(conn, document, replay, seq, now).await?;
    crate::queries::hierarchy::touch_activity(conn, doc.node_id, now).await?;
    super::notify(conn, document, seq).await?;
    let touched = super::blocks_by_ids(conn, document, &effects.touched).await?;
    edit.tree.discard_data(&wanted.updated);
    edit.tree.discard_data(&changed_ids);
    *cached = Some(edit);
    Ok(Outcome::Applied(Applied {
        seq,
        touched,
        named: effects.named,
        tally: effects.tally,
        cut,
        changed: true,
    }))
}

async fn record_batch(
    conn: &mut Conn,
    document: Id,
    replay: Option<(Id, Id)>,
    seq: Seq,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    let Some((author, client)) = replay else {
        return Ok(());
    };
    sqlx::query(
        "INSERT INTO document_ops (document_id, author_id, client_id, seq, created_at)
         VALUES ($1, $2, $3, $4, $5) ON CONFLICT DO NOTHING",
    )
    .bind(document)
    .bind(author)
    .bind(client)
    .bind(seq)
    .bind(now)
    .execute(conn)
    .await?;
    Ok(())
}

async fn refused(conn: &mut Conn, document: Id, refusal: Refusal) -> Result<Outcome, DbError> {
    Ok(match refusal {
        Refusal::Stale(ids) => Outcome::Stale {
            current: super::blocks_by_ids(conn, document, &ids).await?,
        },
        Refusal::Exists(id) => Outcome::Exists { id },
        Refusal::Invalid(faults) => Outcome::Invalid(faults),
        Refusal::BlockTooLarge { path, bytes } => Outcome::BlockTooLarge { path, bytes },
        Refusal::TooDeep { path } => Outcome::TooDeep { path },
        Refusal::TooManyBlocks { limit } => Outcome::TooManyBlocks { limit },
        Refusal::NotLoaded(id) => Outcome::Invalid(vec![Fault::at(
            "ops",
            format!("the block {id} could not be read"),
        )]),
    })
}

/// The blocks stamped with exactly `seq`: what the batch that led to it touched,
/// as far as they still stand as it left them.
pub async fn stamped_at(conn: &mut Conn, document: Id, seq: Seq) -> Result<Vec<BlockRow>, DbError> {
    super::blocks_changed(conn, document, seq - 1, seq, i64::MAX).await
}

// ------------------------------------------------------------------ putting a version back

/// How putting a version back ended.
#[derive(Debug, Clone)]
pub enum Restored {
    /// There is no such document, or it was deleted.
    Gone,
    /// The document has no such version.
    NoSuchVersion,
    /// The blocks of the version stand again. `Applied::cut` says whether the
    /// present state was kept as a version first.
    Done(Applied),
    /// The document would have more blocks than it may.
    TooManyBlocks {
        /// What it may.
        limit: i64,
    },
    /// The document's text would be larger than it may be.
    TooLarge {
        /// What it may.
        limit: i64,
    },
}

fn depth_in(rows: &HashMap<Id, &BlockRow>, id: Id) -> usize {
    let mut depth = 0;
    let mut at = rows.get(&id).and_then(|row| row.parent_id);
    while let Some(parent) = at {
        depth += 1;
        if depth > rows.len() {
            break;
        }
        at = rows.get(&parent).and_then(|row| row.parent_id);
    }
    depth
}

fn same_place(a: &BlockRow, b: &BlockRow) -> bool {
    a.parent_id == b.parent_id && Position::parse(&a.position) == Position::parse(&b.position)
}

fn same_content(a: &BlockRow, b: &BlockRow) -> bool {
    a.block_type == b.block_type && a.props == b.props && a.content == b.content
}

/// Puts a version back: the present state is cut as a version first, unless it
/// is the last one already, and then the live blocks are those of the version,
/// as one change by `writer`. Nothing is merged: what was written since is in
/// the version that was kept.
pub async fn restore(
    conn: &mut Conn,
    document: Id,
    version: i32,
    writer: &Writer,
    limits: Limits,
    rules: Rules,
    now: DateTime<Utc>,
) -> Result<Restored, DbError> {
    let Some(doc) = super::lock(conn, document)
        .await?
        .filter(|doc| doc.deleted_at.is_none())
    else {
        return Ok(Restored::Gone);
    };
    let Some(wanted) = history::snapshot(conn, document, version).await? else {
        return Ok(Restored::NoSuchVersion);
    };
    let live = super::live_blocks(conn, document).await?;
    let wanted_ids: Vec<Id> = wanted.iter().map(|row| row.id).collect();
    let existing: HashMap<Id, BlockRow> = super::blocks_by_ids(conn, document, &wanted_ids)
        .await?
        .into_iter()
        .map(|row| (row.id, row))
        .collect();
    let wanted_set: HashSet<Id> = wanted_ids.iter().copied().collect();
    let seq = doc.doc_seq + 1;
    let (user, thread, agent) = stamped_by(writer);
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
    let mut steps: Vec<&BlockRow> = Vec::new();
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
        steps.push(row);
    }
    let tally = block_changes.tally();
    if touched.is_empty() {
        return Ok(Restored::Done(Applied {
            seq: doc.doc_seq,
            touched: Vec::new(),
            named: Vec::new(),
            tally: Tally::default(),
            cut: None,
            changed: false,
        }));
    }
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
        if existing.contains_key(&row.id) {
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
    span.record(writer, &block_changes, orch_types::Time::new(now));
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
    let (blocks, bytes) = super::recount(conn, document).await?;
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
    super::notify(conn, document, seq).await?;
    let touched = super::blocks_by_ids(conn, document, &touched).await?;
    Ok(Restored::Done(Applied {
        seq,
        touched,
        named: Vec::new(),
        tally,
        cut,
        changed: true,
    }))
}

#[cfg(test)]
mod tests {
    use super::super::testing::{LIMITS, RULES, edit, paragraph, place, put, write};
    use super::*;
    use chrono::Duration;
    use orch_api_contract::documents::{BlockContent, Inline};
    use orch_testkit::{clock, database::TestDb};
    use orch_types::new_id;

    fn text_of(row: &BlockRow) -> String {
        blocks::inline_text(&match blocks::content_from_json(&row.content) {
            BlockContent::Inline(runs) => runs,
            BlockContent::Rows(_) => Vec::new(),
        })
    }

    #[tokio::test]
    async fn two_people_who_write_in_different_blocks_both_succeed() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let now = clock::start();
        let here = place(&mut conn, now).await;
        let (a, b) = (new_id(), new_id());
        let Outcome::Applied(made) = write(
            &mut conn,
            here.document,
            here.ada,
            &[put(a, "a"), put(b, "b")],
            now,
        )
        .await
        else {
            panic!("applied")
        };
        assert_eq!(made.seq, 1);
        let seen_by_both = made.seq;
        let Outcome::Applied(ada) = write(
            &mut conn,
            here.document,
            here.ada,
            &[edit(a, seen_by_both, "a by ada")],
            now,
        )
        .await
        else {
            panic!("ada's batch is applied")
        };
        let Outcome::Applied(bob) = write(
            &mut conn,
            here.document,
            here.bob,
            &[edit(b, seen_by_both, "b by bob")],
            now,
        )
        .await
        else {
            panic!("bob wrote in another block and is not met")
        };
        assert_eq!((ada.seq, bob.seq), (2, 3));
        let live = super::super::live_blocks(&mut conn, here.document)
            .await
            .unwrap();
        let mut words: Vec<String> = live.iter().map(text_of).collect();
        words.sort();
        assert_eq!(words, ["a by ada", "b by bob"]);
    }

    #[tokio::test]
    async fn the_second_of_two_people_who_change_one_block_is_refused_with_the_block_as_it_stands_and_none_of_their_batch_applies()
     {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let now = clock::start();
        let here = place(&mut conn, now).await;
        let (shared, other) = (new_id(), new_id());
        let Outcome::Applied(made) = write(
            &mut conn,
            here.document,
            here.ada,
            &[put(shared, "shared"), put(other, "other")],
            now,
        )
        .await
        else {
            panic!()
        };
        let known = made.seq;
        write(
            &mut conn,
            here.document,
            here.ada,
            &[edit(shared, known, "ada's words")],
            now,
        )
        .await;
        let fresh = new_id();
        let outcome = write(
            &mut conn,
            here.document,
            here.bob,
            &[
                put(fresh, "bob's new block"),
                edit(other, known, "bob changes the other block too"),
                edit(shared, known, "bob's words"),
            ],
            now,
        )
        .await;
        let Outcome::Stale { current } = outcome else {
            panic!("stale")
        };
        assert_eq!(
            current.len(),
            1,
            "only the block somebody changed meanwhile"
        );
        assert_eq!(current[0].id, shared);
        assert_eq!(text_of(&current[0]), "ada's words");
        assert_eq!(current[0].seq_updated, 2);
        assert!(!current[0].seq_updated.eq(&known));
        // Nothing of Bob's batch is there: the caller drops the transaction; here, in the same
        // one, the batch left no row behind that would be committed.
        assert!(!outcome_kept(&Outcome::Stale { current }));
        let doc = super::super::get(&mut conn, here.document)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(doc.doc_seq, 2);
    }

    fn outcome_kept(outcome: &Outcome) -> bool {
        outcome.is_kept()
    }

    #[tokio::test]
    async fn a_refused_batch_that_was_half_written_leaves_nothing_when_its_transaction_is_dropped()
    {
        let test = TestDb::fresh().await;
        let now = clock::start();
        let here = {
            let mut conn = test.pool.acquire().await.unwrap();
            place(&mut conn, now).await
        };
        {
            let mut tx = test.pool.begin().await.unwrap();
            let too_much = LIMITS_BYTES_TINY;
            let outcome = apply(
                &mut tx,
                here.document,
                &Writer::User(here.ada),
                None,
                &[put(new_id(), "this will not fit the document")],
                too_much,
                RULES,
                now,
            )
            .await
            .unwrap();
            assert!(matches!(outcome, Outcome::TooLarge { limit: 10 }));
            assert!(!outcome.is_kept());
        }
        let mut conn = test.pool.acquire().await.unwrap();
        let doc = super::super::get(&mut conn, here.document)
            .await
            .unwrap()
            .unwrap();
        assert_eq!((doc.doc_seq, doc.block_count, doc.bytes), (0, 0, 0));
        assert!(
            super::super::live_blocks(&mut conn, here.document)
                .await
                .unwrap()
                .is_empty()
        );
    }

    const LIMITS_BYTES_TINY: Limits = Limits {
        max_bytes: 10,
        max_blocks: 5_000,
    };

    #[tokio::test]
    async fn a_batch_sent_twice_is_applied_once() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let now = clock::start();
        let here = place(&mut conn, now).await;
        let client = new_id();
        let id = new_id();
        let ops = [put(id, "once")];
        let first = apply(
            &mut conn,
            here.document,
            &Writer::User(here.ada),
            Some(client),
            &ops,
            LIMITS,
            RULES,
            now,
        )
        .await
        .unwrap();
        let Outcome::Applied(first) = first else {
            panic!()
        };
        let again = apply(
            &mut conn,
            here.document,
            &Writer::User(here.ada),
            Some(client),
            &ops,
            LIMITS,
            RULES,
            now,
        )
        .await
        .unwrap();
        let Outcome::Repeated { seq } = again else {
            panic!("repeated")
        };
        assert_eq!(seq, first.seq);
        assert_eq!(
            super::super::live_blocks(&mut conn, here.document)
                .await
                .unwrap()
                .len(),
            1
        );
        let replayed = stamped_at(&mut conn, here.document, seq).await.unwrap();
        assert_eq!(replayed.len(), 1, "what the first time did is found again");
        // The same client identifier from somebody else is another batch.
        let other = apply(
            &mut conn,
            here.document,
            &Writer::User(here.bob),
            Some(client),
            &[put(new_id(), "bob's")],
            LIMITS,
            RULES,
            now,
        )
        .await
        .unwrap();
        assert!(matches!(other, Outcome::Applied(_)));
    }

    #[tokio::test]
    async fn a_block_identifier_that_is_in_use_anywhere_is_refused() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let now = clock::start();
        let here = place(&mut conn, now).await;
        let id = new_id();
        write(&mut conn, here.document, here.ada, &[put(id, "mine")], now).await;
        // Another document of the same place.
        let other = new_id();
        super::super::insert(
            &mut conn,
            &super::super::NewDocument {
                id: other,
                node_id: here.topic,
                org_id: here.org,
                owner_id: here.ada,
                created_by_thread: None,
                created_by_agent: None,
                title: "Other",
                now,
            },
        )
        .await
        .unwrap();
        let outcome = write(&mut conn, other, here.ada, &[put(id, "taken")], now).await;
        assert!(matches!(outcome, Outcome::Exists { id: taken } if taken == id));
    }

    #[tokio::test]
    async fn the_text_of_a_document_has_a_size_and_a_count_and_the_stored_tree_has_its_order() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let now = clock::start();
        let here = place(&mut conn, now).await;
        let ids: Vec<Id> = (0..4).map(|_| new_id()).collect();
        let mut ops = vec![put(ids[0], "first")];
        // Each one after the one before, and the last first among the siblings.
        for pair in ids.windows(2).take(2) {
            ops.push(DocumentOp::Insert {
                block: paragraph(pair[1], "next"),
                parent_id: None,
                after_id: Some(pair[0]),
            });
        }
        ops.push(DocumentOp::Insert {
            block: paragraph(ids[3], "at the top"),
            parent_id: None,
            after_id: None,
        });
        let Outcome::Applied(done) = write(&mut conn, here.document, here.ada, &ops, now).await
        else {
            panic!()
        };
        assert_eq!(done.tally.inserted, 4);
        let order: Vec<Id> = super::super::live_blocks(&mut conn, here.document)
            .await
            .unwrap()
            .iter()
            .map(|b| b.id)
            .collect();
        assert_eq!(order, [ids[3], ids[0], ids[1], ids[2]]);
        let doc = super::super::get(&mut conn, here.document)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(doc.block_count, 4);
        assert!(doc.bytes > 0);
        let counted: i64 = sqlx::query_scalar(
            "SELECT sum(octet_length(props::text) + octet_length(content::text))::bigint FROM document_blocks WHERE document_id = $1",
        )
        .bind(here.document)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        assert_eq!(doc.bytes, counted);
        // Too many blocks.
        let few = Limits {
            max_blocks: 4,
            ..LIMITS
        };
        let outcome = apply(
            &mut conn,
            here.document,
            &Writer::User(here.ada),
            None,
            &[put(new_id(), "fifth")],
            few,
            RULES,
            now,
        )
        .await
        .unwrap();
        assert!(matches!(outcome, Outcome::TooManyBlocks { limit: 4 }));
    }

    #[tokio::test]
    async fn a_batch_that_replaces_the_whole_document_keeps_what_it_said_as_a_version_first() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let now = clock::start();
        let here = place(&mut conn, now).await;
        let (a, b) = (new_id(), new_id());
        write(
            &mut conn,
            here.document,
            here.ada,
            &[put(a, "old one"), put(b, "old two")],
            now,
        )
        .await;
        let soon = now + Duration::seconds(5);
        let replacement = new_id();
        let Outcome::Applied(done) = write(
            &mut conn,
            here.document,
            here.ada,
            &[
                DocumentOp::Remove { id: a },
                DocumentOp::Remove { id: b },
                put(replacement, "the new text"),
            ],
            soon,
        )
        .await
        else {
            panic!()
        };
        assert_eq!(
            done.cut,
            Some(VersionCause::Rewrite),
            "five seconds after the last change, far inside the window"
        );
        let kept = history::snapshot(&mut conn, here.document, 1)
            .await
            .unwrap()
            .unwrap();
        let mut words: Vec<String> = kept.iter().map(text_of).collect();
        words.sort();
        assert_eq!(words, ["old one", "old two"]);
    }

    #[tokio::test]
    async fn a_history_of_small_edits_holds_each_edit_once_and_every_version_reads_back_as_it_was()
    {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let now = clock::start();
        let here = place(&mut conn, now).await;
        let ids: Vec<Id> = (0..40).map(|_| new_id()).collect();
        let long = "a sentence that makes the paragraph long enough to matter ".repeat(4);
        let ops: Vec<DocumentOp> = ids.iter().map(|id| put(*id, &long)).collect();
        write(&mut conn, here.document, here.ada, &ops, now).await;
        let mut expected: Vec<(i32, Vec<BlockRow>)> = Vec::new();
        for round in 0..12 {
            let blocks = super::super::live_blocks(&mut conn, here.document)
                .await
                .unwrap();
            let target = &blocks[round % blocks.len()];
            write(
                &mut conn,
                here.document,
                here.ada,
                &[edit(
                    target.id,
                    target.seq_updated,
                    &format!("edit {round}"),
                )],
                now,
            )
            .await;
            let row = super::super::get(&mut conn, here.document)
                .await
                .unwrap()
                .unwrap();
            let number = history::cut(&mut conn, &row, VersionCause::Window, now)
                .await
                .unwrap();
            let held = super::super::live_blocks(&mut conn, here.document)
                .await
                .unwrap();
            expected.push((number, held));
        }
        for (number, blocks) in &expected {
            let read = history::snapshot(&mut conn, here.document, *number)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(&read, blocks, "version {number} reads back as it was");
        }
        let (changes, stored, whole): (i64, i64, i64) = sqlx::query_as(
            "SELECT count(*) FILTER (WHERE base IS NOT NULL), sum(stored_bytes)::bigint, sum(whole_bytes)::bigint
               FROM document_versions WHERE document_id = $1",
        )
        .bind(here.document)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        assert!(
            changes >= 10,
            "most versions are held as changes: {changes} of 12"
        );
        assert!(
            stored * 4 < whole,
            "the history holds each edit once: {stored} of {whole} bytes"
        );
        let dropped = history::prune(&mut conn, here.document, 5, i64::MAX)
            .await
            .unwrap();
        assert_eq!(dropped.len(), 7);
        let oldest: (i32, Option<i32>) = sqlx::query_as(
            "SELECT version, base FROM document_versions WHERE document_id = $1 ORDER BY version LIMIT 1",
        )
        .bind(here.document)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        assert_eq!(
            oldest.1, None,
            "the oldest kept version is held whole again"
        );
        for (number, blocks) in expected.iter().filter(|(number, _)| *number >= oldest.0) {
            let read = history::snapshot(&mut conn, here.document, *number)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                &read, blocks,
                "version {number} still reads back after the oldest went"
            );
        }
        let fewer = history::prune(&mut conn, here.document, 200, whole / 40)
            .await
            .unwrap();
        assert!(!fewer.is_empty(), "a small budget lets more go");
        let newest = expected.last().unwrap();
        let read = history::snapshot(&mut conn, here.document, newest.0)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(read, newest.1, "the newest never goes and reads back whole");
    }

    #[tokio::test]
    async fn putting_a_version_back_keeps_the_present_state_first_and_makes_the_blocks_of_that_version_stand_again()
     {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let start = clock::start();
        let here = place(&mut conn, start).await;
        let (kept_block, edited, removed_later) = (new_id(), new_id(), new_id());
        let Outcome::Applied(first) = write(
            &mut conn,
            here.document,
            here.ada,
            &[
                put(kept_block, "stays"),
                put(edited, "as it was"),
                put(removed_later, "will go"),
            ],
            start,
        )
        .await
        else {
            panic!()
        };
        // Ten minutes and a second on, so that the next batch cuts a version of this state.
        let t1 = start + Duration::seconds(601);
        let fresh = new_id();
        let Outcome::Applied(second) = write(
            &mut conn,
            here.document,
            here.bob,
            &[
                edit(edited, first.seq, "changed since"),
                DocumentOp::Remove { id: removed_later },
                put(fresh, "written since"),
            ],
            t1,
        )
        .await
        else {
            panic!()
        };
        assert_eq!(second.cut, Some(VersionCause::Window));
        let t2 = t1 + Duration::seconds(30);
        let Restored::Done(done) = restore(
            &mut conn,
            here.document,
            1,
            &Writer::User(here.ada),
            LIMITS,
            RULES,
            t2,
        )
        .await
        .unwrap() else {
            panic!("restored")
        };
        assert_eq!(
            done.cut,
            Some(VersionCause::Restore),
            "the present state is kept first"
        );
        assert!(done.changed);
        let live = super::super::live_blocks(&mut conn, here.document)
            .await
            .unwrap();
        let mut words: Vec<String> = live.iter().map(text_of).collect();
        words.sort();
        assert_eq!(words, ["as it was", "stays", "will go"]);
        assert!(
            live.iter().all(|row| row.id != fresh),
            "what was written since is gone from the page"
        );
        // And it is in the version that was kept just before.
        let before = history::snapshot(&mut conn, here.document, 2)
            .await
            .unwrap()
            .unwrap();
        assert!(before.iter().any(|row| text_of(row) == "written since"));
        let versions = history::list(&mut conn, here.document, None, None, 10)
            .await
            .unwrap();
        assert_eq!(
            versions
                .iter()
                .map(|v| v.cause.as_str())
                .collect::<Vec<_>>(),
            ["restore", "window"]
        );
        // A restore of what already stands changes nothing and cuts nothing.
        let t3 = t2 + Duration::seconds(30);
        let Restored::Done(same) = restore(
            &mut conn,
            here.document,
            1,
            &Writer::User(here.ada),
            LIMITS,
            RULES,
            t3,
        )
        .await
        .unwrap() else {
            panic!()
        };
        assert!(!same.changed);
        assert_eq!(same.cut, None);
        assert!(matches!(
            restore(
                &mut conn,
                here.document,
                99,
                &Writer::User(here.ada),
                LIMITS,
                RULES,
                t3
            )
            .await
            .unwrap(),
            Restored::NoSuchVersion
        ));
    }

    #[tokio::test]
    async fn a_reader_who_would_continue_from_below_what_was_purged_is_told_to_load_again() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let now = clock::start();
        let here = place(&mut conn, now).await;
        let (a, b) = (new_id(), new_id());
        write(
            &mut conn,
            here.document,
            here.ada,
            &[put(a, "a"), put(b, "b")],
            now,
        )
        .await;
        write(
            &mut conn,
            here.document,
            here.ada,
            &[DocumentOp::Remove { id: a }],
            now + Duration::seconds(1),
        )
        .await;
        // The removed row is kept so that a reader learns it went...
        let since_one = super::super::blocks_changed(&mut conn, here.document, 1, 9, 100)
            .await
            .unwrap();
        assert_eq!(since_one.len(), 1);
        assert!(since_one[0].removed_at.is_some());
        // ... until it is purged, and the document says up to where.
        let purged = super::super::purge_removed_blocks(&mut conn, now + Duration::days(40))
            .await
            .unwrap();
        assert_eq!(purged, 1);
        let doc = super::super::get(&mut conn, here.document)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(doc.purged_seq, 2);
        assert!(
            super::super::blocks_changed(&mut conn, here.document, 1, 9, 100)
                .await
                .unwrap()
                .is_empty()
        );
        // A block that is not removed is never purged.
        assert_eq!(
            super::super::live_blocks(&mut conn, here.document)
                .await
                .unwrap()
                .len(),
            1
        );
        // Batch records older than a day go as well.
        assert_eq!(
            super::super::purge_batch_records(&mut conn, now + Duration::days(2))
                .await
                .unwrap(),
            2
        );
    }

    #[tokio::test]
    async fn a_file_that_a_block_names_has_to_be_one_the_document_holds() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let now = clock::start();
        let here = place(&mut conn, now).await;
        let image = NewBlock {
            id: new_id(),
            block_type: BlockType::Image,
            props: serde_json::from_value(serde_json::json!({ "file_id": new_id() })).ok(),
            content: Some(BlockContent::Inline(vec![Inline::Break])),
            children: None,
        };
        let outcome = write(
            &mut conn,
            here.document,
            here.ada,
            &[DocumentOp::Insert {
                block: image,
                parent_id: None,
                after_id: None,
            }],
            now,
        )
        .await;
        assert!(
            matches!(outcome, Outcome::Invalid(faults) if faults[0].message.contains("does not hold"))
        );
    }

    use orch_api_contract::documents::NewBlock;
}
