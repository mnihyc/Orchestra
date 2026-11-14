//! Operation-owned full-history cuts. No caller is authorized by knowing a cut ID.
//! The engine checks current/frozen source authority before using these suppliers.

use crate::queries::{
    thread_continuations::reading::OWNERS,
    thread_history,
    threads::{HistorySegmentRow, ItemRow, TurnRow},
};
use crate::{Conn, DbError};
use orch_types::{Hash, Id, Json, Seq};

pub mod native;

/// The normal largest history decode batch, including explicit full operations.
pub const PAGE: i64 = thread_history::PAGE;

/// A cut is metadata owned by exactly one retained job or side thread.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Cut {
    pub id: Id,
    pub thread_id: Id,
    pub continuation_id: Id,
    pub job_id: Option<Id>,
    pub owner_thread_id: Option<Id>,
    pub source_segment_id: Option<Id>,
    pub source_native_ref: Option<String>,
    pub execution_revision: i64,
    pub first_turn: i32,
    pub last_turn: i32,
    pub captured_seq: Seq,
    pub captured_at: chrono::DateTime<chrono::Utc>,
    pub head: Json,
    pub artifacts: Json,
}

pub async fn cut(conn: &mut Conn, id: Id) -> Result<Option<Cut>, DbError> {
    Ok(
        sqlx::query_as("SELECT * FROM thread_history_cuts WHERE id=$1")
            .bind(id)
            .fetch_optional(conn)
            .await?,
    )
}

/// Indexed boundary of this actual read history, excluding inactive suffixes.
pub async fn last_completed(conn: &mut Conn, thread: Id, view: Id) -> Result<Option<i32>, DbError> {
    last_ended(conn, thread, view, false).await
}

/// Latest finished turn in this actual read view, including failed or interrupted answers.
/// Keep carries their recorded words as well as successfully completed turns.
pub async fn last_finished(conn: &mut Conn, thread: Id, view: Id) -> Result<Option<i32>, DbError> {
    last_ended(conn, thread, view, true).await
}

async fn last_ended(
    conn: &mut Conn,
    thread: Id,
    view: Id,
    all_finished: bool,
) -> Result<Option<i32>, DbError> {
    let sql = format!(
        "{OWNERS} SELECT max(t.ordinal) FROM owners o JOIN LATERAL(
      SELECT t.ordinal FROM turns t WHERE t.thread_id=$1 AND t.continuation_id=o.owner
        AND t.ordinal BETWEEN o.first_turn AND o.last_turn
        AND (t.state='completed' OR ($3 AND t.state IN ('failed','interrupted')))
      ORDER BY t.ordinal DESC LIMIT 1) t ON true"
    );
    Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .bind(view)
        .bind(all_finished)
        .fetch_one(conn)
        .await?)
}

/// A queued input whose earlier delivery is not confirmed cannot become copied
/// conversation by following an old turn/user-item reference. New untouched
/// pending words outside this cut remain allowed and keep their original queue.
/// An absent range checks every turn. A caller selecting no turns supplies an explicit empty range.
pub async fn unconfirmed_inputs(
    conn: &mut Conn,
    thread: Id,
    view: Id,
    range: Option<(i32, i32)>,
) -> Result<bool, DbError> {
    let (first, last) = range.map_or((None, None), |(first, last)| (Some(first), Some(last)));
    Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(unconfirmed_sql()))
        .bind(thread)
        .bind(view)
        .bind(first)
        .bind(last)
        .fetch_one(conn)
        .await?)
}

fn unconfirmed_sql() -> String {
    // Start with uncertain inputs, not the completed history. Both ways an input can occur in a
    // selected turn have an index. OFFSET 0 keeps these input-specific seeks correlated instead of
    // letting PostgreSQL hash the entire selected history as an alternative EXISTS plan.
    format!(
        "{OWNERS} SELECT EXISTS(SELECT 1 FROM thread_inputs i
          WHERE i.thread_id=$1 AND i.state<>'delivered' AND (
            EXISTS(SELECT 1 FROM turns t JOIN owners o ON o.owner=t.continuation_id
              WHERE t.thread_id=$1 AND t.input_id=i.id
                AND t.ordinal BETWEEN o.first_turn AND o.last_turn
                AND ($3::integer IS NULL OR t.ordinal BETWEEN $3 AND $4) OFFSET 0)
            OR EXISTS(SELECT 1 FROM items x JOIN turns t ON t.id=x.turn_id
              JOIN owners o ON o.owner=t.continuation_id
              WHERE x.thread_id=$1 AND x.kind='user_message' AND x.payload->>'input_id'=i.id::text
                AND t.thread_id=$1 AND t.ordinal BETWEEN o.first_turn AND o.last_turn
                AND ($3::integer IS NULL OR t.ordinal BETWEEN $3 AND $4) OFFSET 0)))"
    )
}

/// Whether the selected history contains a before-Orch item, without decoding its bodies.
pub async fn has_before_items(conn: &mut Conn, thread: Id, view: Id) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(has_before_sql()))
        .bind(thread)
        .bind(view)
        .fetch_one(conn)
        .await?)
}

fn has_before_sql() -> String {
    format!(
        "{OWNERS} SELECT EXISTS(SELECT 1 FROM owners o JOIN LATERAL (
          SELECT 1 FROM items i WHERE i.thread_id=$1 AND i.continuation_id=o.owner
            AND i.before_orch AND i.turn_id IS NULL AND i.stream='main'
            AND i.seq_created BETWEEN o.first_seq AND o.last_seq LIMIT 1) i ON true)"
    )
}

/// Before-import rows retain their actual native identities and have no synthetic turn.
/// Each read holds one page of the same selected/frozen decoder used by other history readers.
pub struct BeforeItems {
    thread: Id,
    view: Id,
    after: Seq,
    done: bool,
}
impl BeforeItems {
    pub fn new(thread: Id, view: Id) -> Self {
        Self {
            thread,
            view,
            after: 0,
            done: false,
        }
    }
    pub async fn next(&mut self, conn: &mut Conn) -> Result<Vec<ItemRow>, DbError> {
        if self.done {
            return Ok(Vec::new());
        }
        let page = crate::queries::thread_continuations::reading::before_items_page(
            conn,
            self.thread,
            self.view,
            self.after,
        )
        .await?;
        self.done = page.is_empty();
        if let Some(last) = page.last() {
            self.after = last.seq_created;
        }
        Ok(page)
    }
}

/// The caller holds the source thread lock and the owner's creation/runner claim.
pub struct Capture {
    pub id: Id,
    pub thread: Id,
    pub view: Id,
    pub job: Option<Id>,
    pub owner_thread: Option<Id>,
    pub segment: Option<Id>,
    pub execution_revision: i64,
    pub range: Option<(i32, i32)>,
    pub at: chrono::DateTime<chrono::Utc>,
}

/// Freeze membership and small metadata atomically; large payloads remain shared
/// until their first subsequent write. All pages decode at most PAGE bodies.
pub async fn capture(conn: &mut Conn, asked: &Capture) -> Result<Cut, DbError> {
    let started = std::time::Instant::now();
    let (first, last) = asked.range.unwrap_or((1, 0));
    if unconfirmed_inputs(conn, asked.thread, asked.view, Some((first, last))).await? {
        return Err(DbError::Rule {
            constraint: "history_source_unconfirmed_input".into(),
        });
    }
    let side_of: Option<Id> = sqlx::query_scalar("SELECT side_of FROM threads WHERE node_id=$1")
        .bind(asked.thread)
        .fetch_one(&mut *conn)
        .await?;
    let parent = if let Some(parent) = side_of {
        let cut = cut(conn, asked.thread)
            .await?
            .filter(|cut| cut.thread_id == parent && cut.owner_thread_id == Some(asked.thread))
            .ok_or_else(thread_history::damaged)?;
        if sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM thread_history_cut_spans WHERE cut_id=$1 AND (seq_offset>0 OR thread_id<>$2))")
            .bind(cut.id).bind(cut.thread_id).fetch_one(&mut *conn).await? {return Err(thread_history::damaged());}
        Some(cut)
    } else {
        None
    };
    let prefix_turns = parent
        .as_ref()
        .filter(|_| first == 1)
        .map_or(0, |parent| parent.last_turn);
    let prefix_seq = parent
        .as_ref()
        .filter(|_| first == 1)
        .map_or(0, |parent| parent.captured_seq);
    let logical_first = if first == 1 {
        1
    } else {
        first
            .checked_add(prefix_turns)
            .ok_or_else(thread_history::damaged)?
    };
    let logical_last = last
        .checked_add(prefix_turns)
        .ok_or_else(thread_history::damaged)?;
    if let Some(old) = cut(conn, asked.id).await? {
        if (
            old.thread_id,
            old.continuation_id,
            old.job_id,
            old.owner_thread_id,
            old.source_segment_id,
            old.execution_revision,
            old.first_turn,
            old.last_turn,
        ) != (
            asked.thread,
            asked.view,
            asked.job,
            asked.owner_thread,
            asked.segment,
            asked.execution_revision,
            logical_first,
            logical_last,
        ) {
            return Err(thread_history::damaged());
        }
        return Ok(old);
    }
    let changed=sqlx::query("INSERT INTO thread_history_cuts(id,thread_id,continuation_id,job_id,owner_thread_id,
        source_segment_id,source_native_ref,execution_revision,first_turn,last_turn,captured_seq,captured_at,head)
      SELECT $3,t.node_id,c.id,$4,$5,s.id,s.native_session_ref,$6,$7,$8,t.next_seq-1+$11,$9,
        jsonb_build_object('name',n.name,'state',t.state,'agent_id',t.agent_id,
          'agent_name',a.name,'environment_id',t.environment_id)
      FROM threads t JOIN nodes n ON n.id=t.node_id
        JOIN thread_continuations c ON c.thread_id=t.node_id AND c.id=$2
        LEFT JOIN segments s ON s.thread_id=t.node_id AND s.id=$10
        LEFT JOIN agents a ON a.id=t.agent_id
      WHERE t.node_id=$1 AND ($10::uuid IS NULL OR s.id IS NOT NULL)")
        .bind(asked.thread).bind(asked.view).bind(asked.id).bind(asked.job).bind(asked.owner_thread)
        .bind(asked.execution_revision).bind(logical_first).bind(logical_last).bind(asked.at).bind(asked.segment).bind(prefix_seq)
        .execute(&mut *conn).await?.rows_affected();
    if changed != 1 {
        return Err(thread_history::damaged());
    }
    // Capture uses the same bounded page as decoding/copying. A single INSERT for an
    // entire history otherwise spends the request's statement deadline on FK checks.
    // Keyset progress follows actual turns, so sparse ordinal ranges add no empty pages.
    let sql = format!(
        "{OWNERS},capture_page AS (
      SELECT t.id,t.ordinal,to_jsonb(t) AS body FROM turns t WHERE t.thread_id=$1
        AND t.ordinal BETWEEN $4 AND $5 AND t.ordinal>$6
        AND t.state NOT IN ('starting','running')
        AND EXISTS(SELECT 1 FROM owners o WHERE o.owner=t.continuation_id
          AND t.ordinal BETWEEN o.first_turn AND o.last_turn)
        ORDER BY t.ordinal LIMIT $7
      ),captured_page AS (
        INSERT INTO thread_history_cut_rows(cut_id,kind,row_id,ordinal,body)
        SELECT $3,'turn',id,ordinal,body FROM capture_page RETURNING ordinal
      ) SELECT max(ordinal) FROM captured_page"
    );
    let mut after = first.saturating_sub(1);
    loop {
        let next: Option<i32> = sqlx::query_scalar(sqlx::AssertSqlSafe(sql.clone()))
            .bind(asked.thread)
            .bind(asked.view)
            .bind(asked.id)
            .bind(first)
            .bind(last)
            .bind(after)
            .bind(PAGE)
            .fetch_one(&mut *conn)
            .await?;
        let Some(next) = next else { break };
        after = next;
    }
    tracing::debug!(phase = "turns", elapsed = ?started.elapsed(), "Captured history phase");
    if last > 0
        && !sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM thread_history_cut_rows
        WHERE cut_id=$1 AND kind='turn' AND ordinal=$2)",
        )
        .bind(asked.id)
        .bind(last)
        .fetch_one(&mut *conn)
        .await?
    {
        return Err(thread_history::damaged());
    }
    // The cut holds its items by spans, one for each continuation the view is made of: the positions it reads from
    // that continuation up to the moment of the cut, the turns it holds (its turn rows), and which loose lines go with
    // them. Nothing is copied for an item until it changes or goes (`thread_history_cut_freeze`).
    let captured: i64 =
        sqlx::query_scalar("SELECT captured_seq FROM thread_history_cuts WHERE id=$1")
            .bind(asked.id)
            .fetch_one(&mut *conn)
            .await?;
    let sql = format!("{OWNERS} INSERT INTO thread_history_cut_spans(cut_id,thread_id,owner_continuation_id,first_seq,
        last_seq,frozen_at,seq_offset,loose_before_orch,loose_low,loose_high)
      SELECT $3,$1,o.owner,o.first_seq,LEAST(o.last_seq,$7),COALESCE(o.frozen_at,$7),$6,$4=1,
        CASE WHEN $4=1 THEN 0 ELSE COALESCE((SELECT min(j.seq_created) FROM items j
          WHERE j.thread_id=$1 AND j.turn_id IN(SELECT row_id FROM thread_history_cut_rows
            WHERE cut_id=$3 AND kind='turn' AND ordinal=$4)),0) END,
        CASE WHEN $5>0 THEN COALESCE((SELECT max(j.seq_created) FROM items j
          WHERE j.thread_id=$1 AND j.turn_id IN(SELECT row_id FROM thread_history_cut_rows
            WHERE cut_id=$3 AND kind='turn')),0) ELSE -1 END
      FROM owners o WHERE o.first_seq<=LEAST(o.last_seq,$7)");
    sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(asked.thread)
        .bind(asked.view)
        .bind(asked.id)
        .bind(first)
        .bind(last)
        .bind(prefix_seq)
        .bind(captured - prefix_seq)
        .execute(&mut *conn)
        .await?;
    // A missing inherited revision is damage, never permission to read a newer body.
    if sqlx::query_scalar::<_, bool>(missing_revision_sql())
        .bind(asked.id)
        .fetch_one(&mut *conn)
        .await?
    {
        return Err(thread_history::damaged());
    }
    tracing::debug!(phase = "spans", elapsed = ?started.elapsed(), "Captured history phase");
    if let Some(parent) = parent {
        if first == 1 {
            // The prefix is held as the prefix cut holds it: its spans, and the copies it keeps, pinned for this cut too.
            sqlx::query("INSERT INTO thread_history_cut_spans(cut_id,thread_id,owner_continuation_id,first_seq,last_seq,
                frozen_at,seq_offset,loose_before_orch,loose_low,loose_high)
                SELECT $1,thread_id,owner_continuation_id,first_seq,last_seq,frozen_at,seq_offset,loose_before_orch,
                  loose_low,loose_high FROM thread_history_cut_spans WHERE cut_id=$2")
                .bind(asked.id).bind(parent.id).execute(&mut *conn).await?;
            sqlx::query("INSERT INTO thread_history_cut_items(cut_id,item_id,seq_created,turn_id,stream,kind,revision_seq,native_alias,body)
                SELECT $1,item_id,seq_created,turn_id,stream,kind,revision_seq,native_alias,body
                FROM thread_history_cut_items WHERE cut_id=$2")
                .bind(asked.id).bind(parent.id).execute(&mut *conn).await?;
            sqlx::query("INSERT INTO thread_history_cut_blobs(cut_id,blob)
                SELECT $1,b.hash FROM thread_history_cut_items x CROSS JOIN LATERAL thread_payload_blobs(x.body->'payload') p
                  JOIN blobs b ON b.hash=p.blob WHERE x.cut_id=$1 AND x.body IS NOT NULL ON CONFLICT DO NOTHING")
                .bind(asked.id).execute(&mut *conn).await?;
            sqlx::query("UPDATE thread_history_cuts SET artifacts=jsonb_set(artifacts,'{side_prefix}',
                jsonb_build_object('cut_id',$2::uuid,'through',$3::int,'seq_offset',$4::bigint)) WHERE id=$1")
                .bind(asked.id).bind(parent.id).bind(prefix_turns).bind(prefix_seq).execute(&mut *conn).await?;
        }
        // A range starting after the first side turn excludes its inherited prefix.
        // Private ordering is offset; original row identities/native IDs remain in body.
        shift_metadata(conn, asked.id, "turn", prefix_turns).await?;
        if first == 1 {
            inherit_metadata(conn, asked.id, parent.id, &["turn"], false).await?;
        }
    }
    let sql = source_metadata_sql("segments", "s", 4,
        "INSERT INTO thread_history_cut_rows(cut_id,kind,row_id,ordinal,body)
          SELECT $1,'segment',s.id,s.ordinal,to_jsonb(s)||jsonb_build_object('captured_agent_name',a.name)
          FROM segments s LEFT JOIN agents a ON a.id=s.agent_id WHERE s.thread_id=$2 AND
            (s.id=$3 OR EXISTS(SELECT 1 FROM thread_history_cut_rows t
              WHERE t.cut_id=$1 AND t.kind='turn' AND t.body->>'segment_id'=s.id::text))");
    let mut after: Option<Id> = None;
    loop {
        let next: Option<Id> = sqlx::query_scalar(sqlx::AssertSqlSafe(sql.clone()))
            .bind(asked.id)
            .bind(asked.thread)
            .bind(asked.segment)
            .bind(after)
            .bind(PAGE)
            .fetch_optional(&mut *conn)
            .await?;
        let Some(next) = next else { break };
        after = Some(next);
    }
    tracing::debug!(phase = "segments", elapsed = ?started.elapsed(), "Captured history phase");
    capture_item_metadata(conn, asked.id, asked.thread, true).await?;
    tracing::debug!(phase = "item-segments", elapsed = ?started.elapsed(), "Captured history phase");
    if let Some(parent) = side_of.filter(|_| first == 1) {
        let parent_cut = cut(conn, asked.thread)
            .await?
            .filter(|cut| cut.thread_id == parent)
            .ok_or_else(thread_history::damaged)?;
        let max_segment:i32=sqlx::query_scalar("SELECT COALESCE(max(ordinal),0) FROM thread_history_cut_rows WHERE cut_id=$1 AND kind='segment'")
            .bind(parent_cut.id).fetch_one(&mut *conn).await?;
        shift_metadata(conn, asked.id, "segment", max_segment).await?;
        inherit_metadata(
            conn,
            asked.id,
            parent_cut.id,
            &["segment", "input", "approval"],
            true,
        )
        .await?;
    }
    let sql = source_metadata_sql("thread_inputs", "i", 3,
        "INSERT INTO thread_history_cut_rows(cut_id,kind,row_id,body)
          SELECT $1,'input',i.id,to_jsonb(i)||jsonb_build_object('captured_blobs',COALESCE((
            SELECT jsonb_agg(encode(r.blob,'hex')) FROM thread_input_blob_refs r WHERE r.input_id=i.id),'[]'::jsonb))
          FROM thread_inputs i WHERE i.thread_id=$2 AND
            (EXISTS(SELECT 1 FROM thread_history_cut_rows t WHERE t.cut_id=$1 AND t.kind='turn' AND t.row_id=i.turn_id)
             OR EXISTS(SELECT 1 FROM thread_history_cut_rows t WHERE t.cut_id=$1 AND t.kind='turn' AND t.body->>'input_id'=i.id::text))");
    let mut after: Option<Id> = None;
    loop {
        let next: Option<Id> = sqlx::query_scalar(sqlx::AssertSqlSafe(sql.clone()))
            .bind(asked.id)
            .bind(asked.thread)
            .bind(after)
            .bind(PAGE)
            .fetch_optional(&mut *conn)
            .await?;
        let Some(next) = next else { break };
        after = Some(next);
    }
    tracing::debug!(phase = "inputs", elapsed = ?started.elapsed(), "Captured history phase");
    capture_item_metadata(conn, asked.id, asked.thread, false).await?;
    tracing::debug!(phase = "item-inputs", elapsed = ?started.elapsed(), "Captured history phase");
    let mut after: Option<Id> = None;
    loop {
        let next: Option<Id> = sqlx::query_scalar("WITH page AS MATERIALIZED (
          SELECT row_id,body FROM thread_history_cut_rows WHERE cut_id=$1 AND kind='input'
            AND row_id>=COALESCE($2::uuid,'00000000-0000-0000-0000-000000000000'::uuid) AND ($2::uuid IS NULL OR row_id>$2) ORDER BY row_id LIMIT $3
        ),pinned AS (INSERT INTO thread_history_cut_blobs(cut_id,blob)
          SELECT $1,b.hash FROM page i CROSS JOIN LATERAL jsonb_array_elements_text(i.body->'captured_blobs') r(blob)
          JOIN blobs b ON b.hash=decode(r.blob,'hex') ON CONFLICT DO NOTHING)
          SELECT row_id FROM page ORDER BY row_id DESC LIMIT 1")
            .bind(asked.id).bind(after).bind(PAGE).fetch_optional(&mut *conn).await?;
        let Some(next) = next else { break };
        after = Some(next);
    }
    tracing::debug!(phase = "pins", elapsed = ?started.elapsed(), "Captured history phase");
    // Item totals are accumulated with their payload pages. Metadata totals use
    // the compact ordering index and retain no bodies while counting.
    sqlx::query(
        "INSERT INTO thread_history_cut_row_counts(cut_id,kind,row_count,max_ordinal)
      SELECT $1,kind,count(*)::bigint,max(ordinal) FROM thread_history_cut_rows WHERE cut_id=$1
      GROUP BY kind ON CONFLICT DO NOTHING",
    )
    .bind(asked.id)
    .execute(&mut *conn)
    .await?;
    tracing::debug!(phase = "counts", elapsed = ?started.elapsed(), "Captured history phase");
    cut(conn, asked.id)
        .await?
        .ok_or_else(thread_history::damaged)
}

/// Captured metadata is immutable after publication, so its primary-key pages
/// give a stable inherited snapshot without a single large INSERT.
async fn inherit_metadata(
    conn: &mut Conn,
    target: Id,
    source: Id,
    kinds: &[&str],
    keep_existing: bool,
) -> Result<(), DbError> {
    let conflict = if keep_existing {
        "ON CONFLICT DO NOTHING"
    } else {
        ""
    };
    let sql = format!(
        "WITH page AS MATERIALIZED (
      SELECT * FROM thread_history_cut_rows WHERE cut_id=$2 AND kind=ANY($3)
        AND (kind,row_id)>=(COALESCE($4::text,''),COALESCE($5::uuid,'00000000-0000-0000-0000-000000000000'::uuid))
        AND ($4::text IS NULL OR (kind,row_id)>($4,$5::uuid)) ORDER BY kind,row_id LIMIT $6
    ),copied AS (INSERT INTO thread_history_cut_rows(cut_id,kind,row_id,ordinal,body)
      SELECT $1,kind,row_id,ordinal,body FROM page {conflict})
    SELECT kind,row_id FROM page ORDER BY kind DESC,row_id DESC LIMIT 1"
    );
    let mut after: Option<(String, Id)> = None;
    loop {
        let next: Option<(String, Id)> = sqlx::query_as(sqlx::AssertSqlSafe(sql.clone()))
            .bind(target)
            .bind(source)
            .bind(kinds)
            .bind(after.as_ref().map(|(kind, _)| kind.as_str()))
            .bind(after.as_ref().map(|(_, row)| *row))
            .bind(PAGE)
            .fetch_optional(&mut *conn)
            .await?;
        let Some(next) = next else { break };
        after = Some(next);
    }
    Ok(())
}

/// A row-id cursor is unchanged by an ordinal shift, so no shifted row can be
/// visited twice while a large inherited prefix is put before the local turns.
async fn shift_metadata(conn: &mut Conn, id: Id, kind: &str, offset: i32) -> Result<(), DbError> {
    if offset == 0 {
        return Ok(());
    }
    let mut after: Option<Id> = None;
    loop {
        let next: Option<Id> = sqlx::query_scalar(
            "WITH page AS MATERIALIZED (
          SELECT row_id FROM thread_history_cut_rows WHERE cut_id=$1 AND kind=$2
            AND row_id>=COALESCE($4::uuid,'00000000-0000-0000-0000-000000000000'::uuid) AND ($4::uuid IS NULL OR row_id>$4) ORDER BY row_id LIMIT $5
        ),shifted AS (UPDATE thread_history_cut_rows r SET ordinal=r.ordinal+$3 FROM page p
          WHERE r.cut_id=$1 AND r.kind=$2 AND r.row_id=p.row_id)
        SELECT row_id FROM page ORDER BY row_id DESC LIMIT 1",
        )
        .bind(id)
        .bind(kind)
        .bind(offset)
        .bind(after)
        .bind(PAGE)
        .fetch_optional(&mut *conn)
        .await?;
        let Some(next) = next else { break };
        after = Some(next);
    }
    Ok(())
}

/// Source metadata scans bounded candidate pages before applying the captured
/// membership checks. The explicit nonnull lower bound remains an index seek
/// under generic prepared plans; the residual condition skips the cursor itself.
/// Fixed fragments below never interpolate caller input into SQL.
fn source_metadata_sql(table: &str, alias: &str, cursor: usize, statement: &str) -> String {
    let limit = cursor + 1;
    let statement = statement.replacen(
        &format!("FROM {table} {alias}"),
        &format!("FROM source_page {alias}"),
        1,
    );
    format!(
        "WITH source_page AS MATERIALIZED (
      SELECT * FROM {table} WHERE thread_id=$2 AND id>=COALESCE(${cursor}::uuid,'00000000-0000-0000-0000-000000000000'::uuid)
        AND (${cursor}::uuid IS NULL OR id>${cursor})
      ORDER BY id LIMIT ${limit}
    ),captured AS ({statement}) SELECT id FROM source_page ORDER BY id DESC LIMIT 1"
    )
}

/// Loose/adopted items can name metadata not present in any turn. Keep those
/// frozen references, and their exact totals, on the same bounded body pages.
async fn capture_item_metadata(
    conn: &mut Conn,
    id: Id,
    thread: Id,
    segments: bool,
) -> Result<(), DbError> {
    let operation = if segments {
        "captured_segments AS (INSERT INTO thread_history_cut_rows(cut_id,kind,row_id,ordinal,body)
          SELECT $1,'segment',s.id,s.ordinal,to_jsonb(s)||jsonb_build_object('captured_agent_name',a.name)
          FROM segments s LEFT JOIN agents a ON a.id=s.agent_id
          WHERE NOT (SELECT damaged FROM checked) AND s.thread_id=$6
            AND s.id::text IN(SELECT body->>'segment_id' FROM bodies) ON CONFLICT DO NOTHING)"
    } else {
        "captured_inputs AS (INSERT INTO thread_history_cut_rows(cut_id,kind,row_id,body)
          SELECT $1,'input',i.id,to_jsonb(i)||jsonb_build_object('captured_blobs',COALESCE((
            SELECT jsonb_agg(encode(r.blob,'hex')) FROM thread_input_blob_refs r WHERE r.input_id=i.id),'[]'::jsonb))
          FROM thread_inputs i WHERE NOT (SELECT damaged FROM checked) AND i.thread_id=$6 AND i.state='delivered'
            AND i.id::text IN(SELECT body#>>'{payload,input_id}' FROM bodies WHERE body->>'kind'='user_message')
          ON CONFLICT DO NOTHING),
        captured_approvals AS (INSERT INTO thread_history_cut_rows(cut_id,kind,row_id,body)
          SELECT $1,'approval',a.id,to_jsonb(a)||jsonb_build_object('captured_decided_by',u.display_name||' (@'||u.username||')')
          FROM approvals a LEFT JOIN users u ON u.id=a.decided_by WHERE NOT (SELECT damaged FROM checked)
            AND a.id::text IN(SELECT body#>>'{payload,approval_id}' FROM bodies WHERE body->>'kind'='approval')
          ON CONFLICT DO NOTHING),
        counted AS (INSERT INTO thread_history_cut_counts(cut_id,stream,kind,content,item_count,payload_bytes)
          SELECT $1,b.stream,b.kind,b.turn_id IS NOT NULL OR COALESCE((b.body->>'before_orch')::boolean,false),
            count(*)::bigint,sum(COALESCE((b.body->>'payload_bytes')::bigint,octet_length((b.body->'payload')::text)))::bigint
          FROM bodies b WHERE NOT (SELECT damaged FROM checked) GROUP BY b.stream,b.kind,4
          ON CONFLICT(cut_id,stream,kind,content) DO UPDATE SET
            item_count=thread_history_cut_counts.item_count+EXCLUDED.item_count,
            payload_bytes=thread_history_cut_counts.payload_bytes+EXCLUDED.payload_bytes)"
    };
    let sql = format!(
        "WITH {},{PAGE_BODIES},checked AS MATERIALIZED (
      SELECT max(seq_created) AS last,COALESCE(bool_or(body IS NULL),false) AS damaged FROM bodies
    ),{operation} SELECT last,damaged FROM checked",
        members_page(false)
    );
    let mut after: Seq = -1;
    loop {
        let (next, damaged): (Option<Seq>, bool) = sqlx::query_as(sqlx::AssertSqlSafe(sql.clone()))
            .bind(id)
            .bind(after)
            .bind(i64::MAX)
            .bind(None::<&str>)
            .bind(PAGE)
            .bind(thread)
            .fetch_one(&mut *conn)
            .await?;
        if damaged {
            return Err(thread_history::damaged());
        }
        let Some(next) = next else { break };
        after = next;
    }
    Ok(())
}

/// Only items updated after a span froze can lack its required revision.
/// Probe that update-index range before checking membership; expanding every
/// member and joining it back to items makes a fresh long cut do whole-history
/// work even though none of its items changed. The lateral limit preserves an
/// indexed existence check for each actual span under generic prepared plans.
fn missing_revision_sql() -> &'static str {
    "SELECT EXISTS(SELECT 1 FROM thread_history_cut_spans s CROSS JOIN LATERAL (
      SELECT 1 FROM items i
      WHERE i.thread_id=s.thread_id AND i.continuation_id=s.owner_continuation_id
        AND i.seq_updated>s.frozen_at AND i.seq_created BETWEEN s.first_seq AND s.last_seq
        AND NOT EXISTS(SELECT 1 FROM thread_history_cut_items x
          WHERE x.cut_id=s.cut_id AND x.item_id=i.id AND x.body IS NOT NULL)
        AND ((i.turn_id IS NOT NULL AND EXISTS(SELECT 1 FROM thread_history_cut_rows t
          WHERE t.cut_id=s.cut_id AND t.kind='turn' AND t.row_id=i.turn_id))
          OR (i.turn_id IS NULL AND ((s.loose_before_orch AND i.before_orch)
            OR i.seq_created BETWEEN s.loose_low AND s.loose_high)))
        AND NOT EXISTS(SELECT 1 FROM thread_item_revisions r
          WHERE r.item_id=i.id AND r.seq_updated<=s.frozen_at)
      ORDER BY i.seq_updated LIMIT 1
    ) missing WHERE s.cut_id=$1)"
}

const BODIES:&str="WITH bodies AS (
  SELECT c.item_id,c.seq_created,c.turn_id,c.stream,
    (CASE WHEN c.body IS NOT NULL THEN c.body WHEN c.revision_seq IS NOT NULL THEN r.body ELSE to_jsonb(i) END)
      ||jsonb_build_object('seq_created',c.seq_created,'history_native_alias',c.native_alias) AS body
  FROM thread_history_cut_members($1) c LEFT JOIN items i ON i.id=c.item_id
    LEFT JOIN thread_item_revisions r ON r.item_id=c.item_id AND r.seq_updated=c.revision_seq)";

/// Frozen bodies for the members a CTE named `page` has already selected.
/// Materializing the indexed membership page first keeps joins and JSON body
/// construction bounded, including capture metadata and maintained totals.
const PAGE_BODIES: &str = "bodies AS (
  SELECT c.item_id,c.seq_created,c.turn_id,c.stream,c.kind,
    (CASE WHEN c.body IS NOT NULL THEN c.body WHEN c.revision_seq IS NOT NULL THEN r.body ELSE i.body END)
      ||jsonb_build_object('seq_created',c.seq_created,'history_native_alias',c.native_alias) AS body
  FROM page c LEFT JOIN LATERAL (
    SELECT to_jsonb(i) AS body FROM items i WHERE i.id=c.item_id
      AND c.body IS NULL AND c.revision_seq IS NULL LIMIT 1
  ) i ON true LEFT JOIN LATERAL (
    SELECT r.body FROM thread_item_revisions r
      WHERE r.item_id=c.item_id AND r.seq_updated=c.revision_seq AND c.body IS NULL LIMIT 1
  ) r ON true)";

/// A page of the members of the cut `$1`, as [`thread_history_cut_members`] gives them, at positions after `$2` and
/// through `$3`, of the stream `$4` where one is given, at most `$5`: oldest first, or newest first with `descending`.
/// Each span reads its own page from the index of the items, and the kept copies theirs from the cut's own rows, so a
/// page costs a page however long the cut is.
fn members_page(descending: bool) -> String {
    member_page_sql(descending, false)
}

fn copy_members_page() -> String {
    member_page_sql(false, true)
}

fn member_page_sql(descending: bool, copying: bool) -> String {
    let order = if descending { "DESC" } else { "" };
    let live_kind = if copying { "AND i.kind<>'marker'" } else { "" };
    let frozen_kind = if copying { "AND x.kind<>'marker'" } else { "" };
    format!(
        "page AS MATERIALIZED (SELECT * FROM (
    SELECT m.* FROM thread_history_cut_spans s CROSS JOIN LATERAL (
      SELECT s.cut_id,k.item_id,k.seq_created+s.seq_offset AS seq_created,k.turn_id,k.stream,k.kind,
        NULL::uuid AS parent_cut_id,
        CASE WHEN k.seq_updated>s.frozen_at THEN (SELECT r.seq_updated FROM thread_item_revisions r
          WHERE r.item_id=k.item_id AND r.seq_updated<=s.frozen_at ORDER BY r.seq_updated DESC LIMIT 1) END AS revision_seq,
        COALESCE(x.native_alias,a.native_ref) AS native_alias,NULL::jsonb AS body
      FROM (
        SELECT i.id AS item_id,i.seq_created,i.seq_updated,i.turn_id,i.stream,i.kind FROM items i
        WHERE i.thread_id=s.thread_id AND i.continuation_id=s.owner_continuation_id
          AND i.seq_created BETWEEN GREATEST(s.first_seq,$2-s.seq_offset+1) AND LEAST(s.last_seq,$3-s.seq_offset)
          AND ($4::text IS NULL OR i.stream=$4) {live_kind}
          AND COALESCE((SELECT x.body IS NULL FROM thread_history_cut_items x
            WHERE x.cut_id=s.cut_id AND x.item_id=i.id),true)
          AND ((i.turn_id IS NOT NULL AND COALESCE((SELECT true FROM thread_history_cut_rows t
                  WHERE t.cut_id=s.cut_id AND t.kind='turn' AND t.row_id=i.turn_id LIMIT 1),false))
            OR (i.turn_id IS NULL AND ((s.loose_before_orch AND i.before_orch)
                  OR i.seq_created BETWEEN s.loose_low AND s.loose_high)))
        ORDER BY i.seq_created {order} LIMIT $5
      ) k
        LEFT JOIN thread_history_cut_items x ON x.cut_id=s.cut_id AND x.item_id=k.item_id
        LEFT JOIN thread_native_item_aliases a ON a.item_id=k.item_id
    ) m WHERE s.cut_id=$1
    UNION ALL
    (SELECT x.cut_id,x.item_id,x.seq_created,x.turn_id,x.stream,x.kind,x.parent_cut_id,x.revision_seq,x.native_alias,x.body
      FROM thread_history_cut_items x WHERE x.cut_id=$1 AND x.body IS NOT NULL {frozen_kind}
        AND x.seq_created>$2 AND x.seq_created<=$3 AND ($4::text IS NULL OR x.stream=$4)
      ORDER BY x.seq_created {order} LIMIT $5)
  ) held ORDER BY seq_created {order} LIMIT $5)"
    )
}

pub async fn turns(conn: &mut Conn, id: Id, from: i64, to: i64) -> Result<Vec<TurnRow>, DbError> {
    Ok(sqlx::query_as("SELECT p.* FROM thread_history_cut_rows r
      CROSS JOIN LATERAL jsonb_populate_record(NULL::turns,r.body||jsonb_build_object('ordinal',r.ordinal)) p
      WHERE r.cut_id=$1 AND r.kind='turn' AND r.ordinal::bigint BETWEEN $2 AND $3 ORDER BY r.ordinal")
        .bind(id).bind(from).bind(to).fetch_all(conn).await?)
}

pub async fn segments(conn: &mut Conn, id: Id) -> Result<Vec<HistorySegmentRow>, DbError> {
    Ok(sqlx::query_as("SELECT p.* FROM thread_history_cut_rows r
      CROSS JOIN LATERAL jsonb_populate_record(NULL::segments,r.body||jsonb_build_object('ordinal',r.ordinal)) p
      WHERE r.cut_id=$1 AND r.kind='segment' ORDER BY r.ordinal").bind(id).fetch_all(conn).await?)
}

pub async fn metadata(conn: &mut Conn, id: Id, kind: &str) -> Result<Vec<Json>, DbError> {
    Ok(sqlx::query_scalar("SELECT CASE WHEN kind IN ('turn','segment') THEN body||jsonb_build_object('ordinal',ordinal) ELSE body END
        FROM thread_history_cut_rows WHERE cut_id=$1 AND kind=$2 ORDER BY ordinal,row_id")
        .bind(id).bind(kind).fetch_all(conn).await?)
}

pub async fn items_page(
    conn: &mut Conn,
    id: Id,
    after: Seq,
    through: Seq,
    stream: Option<&str>,
) -> Result<Vec<ItemRow>, DbError> {
    let sql = format!(
        "WITH {}, {PAGE_BODIES} SELECT p.* FROM bodies CROSS JOIN LATERAL jsonb_populate_record(NULL::items,bodies.body) p
          ORDER BY p.seq_created",
        members_page(false)
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .bind(after)
        .bind(through)
        .bind(stream)
        .bind(PAGE)
        .fetch_all(conn)
        .await?)
}

/// The ordinary frozen selector with a byte-buffer target before decoding returned rows.
pub async fn items_page_bounded(
    conn: &mut Conn,
    id: Id,
    after: Seq,
    through: Seq,
    stream: Option<&str>,
    descending: bool,
) -> Result<Vec<ItemRow>, DbError> {
    let order = if descending { "DESC" } else { "ASC" };
    let members =
        members_page(descending).replacen("page AS MATERIALIZED", "raw_page AS MATERIALIZED", 1);
    let sql=format!("WITH {members},sized AS MATERIALIZED (
      SELECT c.*,COALESCE(CASE WHEN c.body IS NOT NULL THEN COALESCE((c.body->>'payload_bytes')::bigint,octet_length((c.body->'payload')::text))
        WHEN c.revision_seq IS NOT NULL THEN COALESCE((r.body->>'payload_bytes')::bigint,octet_length((r.body->'payload')::text))
        ELSE i.payload_bytes END,0) AS decode_size
      FROM raw_page c LEFT JOIN items i ON i.id=c.item_id AND c.body IS NULL
      LEFT JOIN thread_item_revisions r ON r.item_id=c.item_id AND r.seq_updated=c.revision_seq),
      page AS MATERIALIZED(SELECT chosen.* FROM (SELECT sized.*,sum(decode_size) OVER(ORDER BY seq_created {order} ROWS UNBOUNDED PRECEDING) AS used FROM sized) chosen
        WHERE used-decode_size<$6::bigint),{PAGE_BODIES}
      SELECT p.* FROM bodies CROSS JOIN LATERAL jsonb_populate_record(NULL::items,bodies.body) p ORDER BY p.seq_created");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .bind(after)
        .bind(through)
        .bind(stream)
        .bind(PAGE)
        .bind(thread_history::PAGE_BYTES)
        .fetch_all(conn)
        .await?)
}

/// What a cut holds, counted in SQL: the number of its turns, and its main-stream items by kind (those made in a turn,
/// and the loose lines from before Orch).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CutCounts {
    /// Its turns.
    pub turns: i64,
    /// Its main-stream items by kind.
    pub items: std::collections::BTreeMap<String, i64>,
}

/// Counts what the cut `id` holds. Nothing of it is read into memory.
pub async fn counts(conn: &mut Conn, id: Id) -> Result<CutCounts, DbError> {
    let turns: i64 = sqlx::query_scalar(
        "SELECT COALESCE((SELECT row_count FROM thread_history_cut_row_counts WHERE cut_id=$1 AND kind='turn'),0)",
    )
    .bind(id)
    .fetch_one(&mut *conn)
    .await?;
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT kind,sum(item_count)::bigint FROM thread_history_cut_counts
      WHERE cut_id=$1 AND stream='main' AND content GROUP BY kind",
    )
    .bind(id)
    .fetch_all(&mut *conn)
    .await?;
    Ok(CutCounts {
        turns,
        items: rows.into_iter().collect(),
    })
}

/// Exact decoded payload bytes of the main stream, measured once at its frozen capture.
pub async fn payload_bytes(conn: &mut Conn, id: Id) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar("SELECT COALESCE(sum(payload_bytes),0)::bigint FROM thread_history_cut_counts WHERE cut_id=$1 AND stream='main'")
        .bind(id).fetch_one(conn).await?)
}

/// The turns of the cut `id` with these identifiers, oldest first.
pub async fn turns_among(conn: &mut Conn, id: Id, ids: &[Id]) -> Result<Vec<TurnRow>, DbError> {
    Ok(sqlx::query_as("SELECT p.* FROM thread_history_cut_rows r
      CROSS JOIN LATERAL jsonb_populate_record(NULL::turns,r.body||jsonb_build_object('ordinal',r.ordinal)) p
      WHERE r.cut_id=$1 AND r.kind='turn' AND r.row_id=ANY($2) ORDER BY r.ordinal")
        .bind(id).bind(ids).fetch_all(conn).await?)
}

/// A page of the turns of the cut `id` numbered above `after` and up to `high`, oldest first.
pub async fn turns_after_page(
    conn: &mut Conn,
    id: Id,
    after: i64,
    high: i64,
    limit: i64,
) -> Result<Vec<TurnRow>, DbError> {
    Ok(sqlx::query_as("SELECT p.* FROM thread_history_cut_rows r
      CROSS JOIN LATERAL jsonb_populate_record(NULL::turns,r.body||jsonb_build_object('ordinal',r.ordinal)) p
      WHERE r.cut_id=$1 AND r.kind='turn' AND r.ordinal>$2 AND r.ordinal<=$3 ORDER BY r.ordinal LIMIT $4")
        .bind(id).bind(after).bind(high).bind(limit).fetch_all(conn).await?)
}

/// A page of the turns of the cut `id` numbered below `before` and from `low`, the latest ones, oldest first.
pub async fn turns_before_page(
    conn: &mut Conn,
    id: Id,
    before: i64,
    low: i64,
    limit: i64,
) -> Result<Vec<TurnRow>, DbError> {
    Ok(sqlx::query_as("SELECT p.* FROM (
        SELECT r.body,r.ordinal FROM thread_history_cut_rows r
      WHERE r.cut_id=$1 AND r.kind='turn' AND r.ordinal<$2 AND r.ordinal>=$3 ORDER BY r.ordinal DESC LIMIT $4) page
      CROSS JOIN LATERAL jsonb_populate_record(NULL::turns,page.body||jsonb_build_object('ordinal',page.ordinal)) p
      ORDER BY page.ordinal")
        .bind(id).bind(before).bind(low).bind(limit).fetch_all(conn).await?)
}

/// The rows of kind `kind` (`segment`, `input` or `approval`) of the cut `id` with these identifiers.
pub async fn metadata_among(
    conn: &mut Conn,
    id: Id,
    kind: &str,
    ids: &[Id],
) -> Result<Vec<Json>, DbError> {
    Ok(sqlx::query_scalar("SELECT CASE WHEN kind IN ('turn','segment') THEN body||jsonb_build_object('ordinal',ordinal) ELSE body END
        FROM thread_history_cut_rows WHERE cut_id=$1 AND kind=$2 AND row_id=ANY($3) ORDER BY ordinal,row_id")
        .bind(id).bind(kind).bind(ids).fetch_all(conn).await?)
}

/// The main-stream items of the cut `id` before the position `before` (the last ones when there is none), at most a
/// page of them, oldest first: the page that lies just before a position, for reading a cut from its end.
pub async fn items_page_back(
    conn: &mut Conn,
    id: Id,
    before: Option<Seq>,
) -> Result<Vec<ItemRow>, DbError> {
    let sql = format!(
        "WITH {}, {PAGE_BODIES} SELECT p.* FROM bodies CROSS JOIN LATERAL jsonb_populate_record(NULL::items,bodies.body) p
          ORDER BY p.seq_created",
        members_page(true)
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .bind(-1_i64)
        .bind(before.map_or(i64::MAX, |before| before.saturating_sub(1)))
        .bind("main")
        .bind(PAGE)
        .fetch_all(conn)
        .await?)
}

/// The last thing each of these sub-agent streams said in these turns of the cut `id`, by stream.
pub async fn last_sub_messages(
    conn: &mut Conn,
    id: Id,
    turns: &[Id],
    streams: &[String],
) -> Result<std::collections::HashMap<String, String>, DbError> {
    let sql = format!(
        "WITH page AS MATERIALIZED (SELECT * FROM thread_history_cut_members($1) c WHERE c.turn_id=ANY($2)
      AND c.stream=ANY($3) AND c.kind='agent_message'),
      {PAGE_BODIES} SELECT DISTINCT ON (stream) stream,body#>>'{{payload,text}}' FROM bodies
      ORDER BY stream,seq_created DESC"
    );
    let rows: Vec<(String, Option<String>)> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .bind(turns)
        .bind(streams)
        .fetch_all(conn)
        .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(stream, text)| text.map(|text| (stream, text)))
        .collect())
}

/// A bounded frozen payload page, retaining markers for ordinary history readers.
pub async fn item_bodies_page(conn: &mut Conn, id: Id, after: Seq) -> Result<Vec<Json>, DbError> {
    item_bodies_from(conn, id, after, members_page(false)).await
}

/// The copied conversation omits markers; they do not consume payload or parent pages.
pub async fn copy_item_bodies_page(
    conn: &mut Conn,
    id: Id,
    after: Seq,
) -> Result<Vec<Json>, DbError> {
    item_bodies_from(conn, id, after, copy_members_page()).await
}

async fn item_bodies_from(
    conn: &mut Conn,
    id: Id,
    after: Seq,
    members: String,
) -> Result<Vec<Json>, DbError> {
    let sql = format!("WITH {members}, {PAGE_BODIES} SELECT body FROM bodies ORDER BY seq_created");
    let rows: Vec<Option<Json>> = sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .bind(after)
        .bind(i64::MAX)
        .bind(None::<&str>)
        .bind(PAGE)
        .fetch_all(conn)
        .await?;
    rows.into_iter()
        .map(|body| body.ok_or_else(thread_history::damaged))
        .collect()
}

/// Copies one keyset page of frozen turns. The cursor is the last complete metadata key,
/// including turns without items; every row keeps the atomic copy's existing identity mapping.
pub async fn copy_turns_page(
    conn: &mut Conn,
    id: Id,
    target: Id,
    owner: Id,
    first: Seq,
    after: Option<(i32, Id)>,
) -> Result<Option<(i32, Id)>, DbError> {
    let (position, limit) = if after.is_some() {
        ("AND (r.ordinal,r.row_id)>($5::integer,$6::uuid)", "$7")
    } else {
        ("", "$5")
    };
    let sql = format!(
        "WITH page AS MATERIALIZED (
          SELECT r.row_id,r.ordinal,r.body FROM thread_history_cut_rows r
          WHERE r.cut_id=$2 AND r.kind='turn' {position}
          ORDER BY r.ordinal,r.row_id LIMIT {limit}
        ),copied AS (
          INSERT INTO turns SELECT p.* FROM page r
      CROSS JOIN LATERAL jsonb_populate_record(NULL::turns,r.body||jsonb_build_object(
      'id',md5($1::text||r.row_id::text)::uuid,'thread_id',$1::uuid,'continuation_id',$3::uuid,'ordinal',r.ordinal,
      'segment_id',md5($1::text||(r.body->>'segment_id'))::uuid,'input_id',md5($1::text||(r.body->>'input_id'))::uuid,
      'lease_id',NULL,'native_turn_ref','fork-copy:'||r.row_id::text,'native_end_confirmed',true,
      'before_snapshot_id',NULL,'end_manifest_blob',NULL,'files_end_manifest',NULL,'end_work',NULL,
      'files_work_pending',false,'files_end_due',false,'cut_notice_sent',true,'phase',NULL,'phase_at',NULL,'phase_attempt',NULL,
      'waiting_since',NULL,'seq_updated',$4::bigint,'completed_seq',$4::bigint)) p
          RETURNING id
        )
        SELECT r.ordinal,r.row_id FROM page r CROSS JOIN (SELECT count(*) FROM copied) done
        ORDER BY r.ordinal DESC,r.row_id DESC LIMIT 1"
    );
    let mut query = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(target)
        .bind(id)
        .bind(owner)
        .bind(first);
    if let Some((ordinal, row)) = after {
        query = query.bind(ordinal).bind(row);
    }
    Ok(query.bind(PAGE).fetch_optional(conn).await?)
}

/// Restores a page of copied parent links after all copied items exist. Reading the frozen
/// bodies and updating their mapped parents share one statement, including pages without parents.
pub async fn restore_copy_parents_page(
    conn: &mut Conn,
    id: Id,
    target: Id,
    after: Seq,
) -> Result<Option<Seq>, DbError> {
    let sql = format!(
        "WITH {}, {PAGE_BODIES}, checked AS MATERIALIZED (
          SELECT max(seq_created) AS last,COALESCE(bool_or(body IS NULL),false) AS damaged FROM bodies
        ),restored AS (
          UPDATE items i SET parent_item_id=md5($6::text||(b.body->>'parent_item_id'))::uuid
          FROM bodies b,checked c WHERE NOT c.damaged AND b.body->>'parent_item_id' IS NOT NULL
            AND i.id=md5($6::text||(b.body->>'id'))::uuid
            AND EXISTS(SELECT 1 FROM thread_history_cut_members($1) p
              WHERE p.item_id=(b.body->>'parent_item_id')::uuid AND p.kind<>'marker')
          RETURNING i.id
        )
        SELECT c.last,c.damaged FROM checked c CROSS JOIN (SELECT count(*) FROM restored) done",
        copy_members_page()
    );
    let (last, damaged): (Option<Seq>, bool) = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .bind(after)
        .bind(i64::MAX)
        .bind(None::<&str>)
        .bind(PAGE)
        .bind(target)
        .fetch_one(conn)
        .await?;
    if damaged {
        return Err(thread_history::damaged());
    }
    Ok(last)
}

pub async fn item_ids(conn: &mut Conn, id: Id) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar("SELECT item_id FROM thread_history_cut_members($1) WHERE kind<>'marker' ORDER BY seq_created")
        .bind(id).fetch_all(conn).await?)
}

/// How many rows of a kind (`segment`, `input`, `turn` or `approval`) the cut `id` holds.
pub async fn row_count(conn: &mut Conn, id: Id, kind: &str) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT COALESCE((SELECT row_count FROM thread_history_cut_row_counts WHERE cut_id=$1 AND kind=$2),0)",
    )
    .bind(id)
    .bind(kind)
    .fetch_one(conn)
    .await?)
}

/// The highest number among the rows of a kind that the cut `id` holds; nought where it holds none.
pub async fn max_ordinal(conn: &mut Conn, id: Id, kind: &str) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT COALESCE((SELECT max_ordinal::bigint FROM thread_history_cut_row_counts WHERE cut_id=$1 AND kind=$2),0)",
    )
    .bind(id)
    .bind(kind)
    .fetch_one(conn)
    .await?)
}

/// Numbering reserves every captured position, including omitted marker rows.
pub async fn item_count(conn: &mut Conn, id: Id) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT COALESCE(sum(item_count),0)::bigint FROM thread_history_cut_counts WHERE cut_id=$1",
    )
    .bind(id)
    .fetch_one(conn)
    .await?)
}

pub async fn native_aliases(conn: &mut Conn, id: Id) -> Result<Vec<(Id, String)>, DbError> {
    Ok(sqlx::query_as("SELECT item_id,native_alias FROM thread_history_cut_members($1) WHERE native_alias IS NOT NULL")
        .bind(id).fetch_all(conn).await?)
}

/// Publish only identities actually matched in the native record. Existing
/// proved aliases cannot be overwritten by another read or an ordinal guess.
pub async fn keep_native_aliases(conn: &mut Conn, id: Id, aliases: &Json) -> Result<(), DbError> {
    let changed=sqlx::query("INSERT INTO thread_history_cut_items AS c (cut_id,item_id,seq_created,turn_id,stream,kind,native_alias)
        SELECT $1,m.item_id,m.seq_created,m.turn_id,m.stream,m.kind,a.value FROM jsonb_each_text($2::jsonb) a
          JOIN thread_history_cut_members($1) m ON m.item_id::text=a.key
        WHERE m.native_alias IS NULL OR m.native_alias=a.value
        ON CONFLICT (cut_id,item_id) DO UPDATE SET native_alias=EXCLUDED.native_alias
          WHERE c.native_alias IS NULL OR c.native_alias=EXCLUDED.native_alias")
        .bind(id).bind(aliases).execute(conn).await?.rows_affected();
    if changed
        != aliases
            .as_object()
            .ok_or_else(thread_history::damaged)?
            .len() as u64
    {
        return Err(thread_history::damaged());
    }
    Ok(())
}

pub async fn items_between(
    conn: &mut Conn,
    id: Id,
    after: Seq,
    through: Seq,
    stream: Option<&str>,
) -> Result<Vec<ItemRow>, DbError> {
    let mut rows = Vec::new();
    let mut cursor = after;
    loop {
        let page = items_page(conn, id, cursor, through, stream).await?;
        let Some(last) = page.last() else { break };
        cursor = last.seq_created;
        let done = page.len() < PAGE as usize;
        rows.extend(page);
        if done {
            break;
        }
    }
    Ok(rows)
}

pub async fn items_of_turn(conn: &mut Conn, id: Id, turn: Id) -> Result<Vec<ItemRow>, DbError> {
    let sql = format!(
        "{BODIES} SELECT p.* FROM bodies CROSS JOIN LATERAL jsonb_populate_record(NULL::items,bodies.body) p
      WHERE bodies.turn_id=$2 AND bodies.seq_created>$3 ORDER BY bodies.seq_created LIMIT $4"
    );
    let mut all = Vec::new();
    let mut after = 0;
    loop {
        let page: Vec<ItemRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql.clone()))
            .bind(id)
            .bind(turn)
            .bind(after)
            .bind(PAGE)
            .fetch_all(&mut *conn)
            .await?;
        let Some(last) = page.last() else { break };
        after = last.seq_created;
        let done = page.len() < PAGE as usize;
        all.extend(page);
        if done {
            break;
        }
    }
    Ok(all)
}

pub async fn turn_high(conn: &mut Conn, id: Id, ordinal: i64) -> Result<Option<Seq>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT max(i.seq_created) FROM thread_history_cut_members($1) i
      JOIN thread_history_cut_rows t ON t.cut_id=$1 AND t.kind='turn' AND t.row_id=i.turn_id
      WHERE t.ordinal::bigint<=$2 AND i.stream='main'",
    )
    .bind(id)
    .bind(ordinal)
    .fetch_one(conn)
    .await?)
}

/// First publisher wins; retries must use the persisted artifact, not render again.
pub async fn keep_artifact(
    conn: &mut Conn,
    id: Id,
    name: &str,
    value: &Json,
    blobs: &[Hash],
) -> Result<bool, DbError> {
    let changed = sqlx::query(
        "UPDATE thread_history_cuts SET artifacts=jsonb_set(artifacts,ARRAY[$2::text],$3)
      WHERE id=$1 AND (NOT artifacts ? $2 OR artifacts->$2=$3)",
    )
    .bind(id)
    .bind(name)
    .bind(value)
    .execute(&mut *conn)
    .await?
    .rows_affected()
        == 1;
    if changed {
        for blob in blobs {
            sqlx::query("INSERT INTO thread_history_cut_blobs(cut_id,blob) VALUES($1,$2) ON CONFLICT DO NOTHING")
        .bind(id).bind(blob.as_bytes().as_slice()).execute(&mut *conn).await?;
        }
    }
    Ok(changed)
}

/// Same admission transaction as the command and its retained outgoing body.
pub async fn hold_command(
    conn: &mut Conn,
    cut: Id,
    command: Id,
    machine: Id,
) -> Result<(), DbError> {
    sqlx::query("INSERT INTO thread_history_cut_commands(cut_id,command_id,runtime_id,retirement_machine_id)
      SELECT $1,$2,r.id,h.machine_id FROM (VALUES(1)) seed(n) LEFT JOIN runtimes r
        ON r.machine_id=$3 AND EXISTS(SELECT 1 FROM machines m WHERE m.id=$3 AND m.runtime_id=r.id)
        LEFT JOIN hosts h ON h.id=r.host_id ON CONFLICT(command_id) DO NOTHING")
        .bind(cut).bind(command).bind(machine).execute(&mut *conn).await?;
    let owner: Option<Id> =
        sqlx::query_scalar("SELECT cut_id FROM thread_history_cut_commands WHERE command_id=$1")
            .bind(command)
            .fetch_optional(conn)
            .await?;
    if owner != Some(cut) {
        return Err(thread_history::damaged());
    }
    Ok(())
}

pub async fn settle_command(conn: &mut Conn, cut: Id, command: Id) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE thread_history_cut_commands SET settled=true WHERE cut_id=$1 AND command_id=$2",
    )
    .bind(cut)
    .bind(command)
    .execute(conn)
    .await?;
    Ok(())
}

/// Safe only after all of its native consumers have genuinely settled. A failed
/// retained job keeps its cut for an explicit same-identity retry until trimming.
pub async fn release(conn: &mut Conn, cut: Id) -> Result<(), DbError> {
    sqlx::query("DELETE FROM thread_history_cuts WHERE id=$1")
        .bind(cut)
        .execute(conn)
        .await?;
    Ok(())
}

#[cfg(test)]
#[path = "thread_history_cuts/scale_tests.rs"]
mod scale_tests;
