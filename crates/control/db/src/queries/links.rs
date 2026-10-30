//! What refers to a document, what it refers to and the neighbourhood around it, read from the edge store of typed references.
//!
//! Every statement here answers only about live parts: a block that was removed, a deleted comment or message, and
//! an item of a thread that is not in the view the thread holds now are never sources. Who may read a source is the
//! caller's: the conservative scopes narrow the candidates in the statement, and the caller checks each one exactly
//! before anything is delivered or counted.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Id, Json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

/// The digest the edge store keeps of a workspace path, so that a file of an environment is a target of its own.
#[must_use]
pub fn path_key(path: &str) -> Vec<u8> {
    Sha256::digest(path.as_bytes()).to_vec()
}

/// An item of a thread is part of what the thread shows now: kept in the continuation it follows or inherited into it
/// unchanged since it was frozen, a kind people read as a message, with no text Orch added for the agent.
/// Alias `i` is the item. Correlated probes use `OFFSET 0` so an OR cannot expand them into a scan of all threads.
const LIVE_ITEM: &str = "i.kind IN ('user_message','agent_message')
  AND NOT i.contains_internal_marker
  AND EXISTS (SELECT 1 FROM threads th WHERE th.node_id=i.thread_id AND (
    i.continuation_id=th.active_continuation_id
    OR EXISTS (SELECT 1 FROM thread_continuation_spans s
      WHERE s.continuation_id=th.active_continuation_id AND s.owner_continuation_id=i.continuation_id
        AND i.seq_created BETWEEN s.first_seq AND s.last_seq AND i.seq_updated<=s.frozen_at
        AND (i.turn_id IS NULL OR EXISTS (SELECT 1 FROM turns tu
          WHERE tu.id=i.turn_id AND tu.ordinal BETWEEN s.first_turn AND s.last_turn)))))";

/// A thread that is a side thread of another is a private scratch conversation: no tree lists it and nothing refers
/// to it. `alias` is the `related_items` row.
fn not_side(alias: &str) -> String {
    format!(
        "({alias}.kind<>'conversation' OR NOT EXISTS (SELECT 1 FROM threads side \
         WHERE side.node_id={alias}.source_id AND side.side_of IS NOT NULL))"
    )
}

/// The conservative scopes of a reader over a whole thing of `related_items` under `alias`, whose node is `n`.
fn access(readable: &str, alias: &str) -> String {
    super::search::CANDIDATE_ACCESS
        .replace("__ACCESS__", readable)
        .replace("__PATH__", "n.path")
        .replace("__OWNER__", "n.owner_id")
        .replace("__KIND__", &format!("{alias}.kind"))
        .replace("__SOURCE__", &format!("{alias}.source_id"))
}

/// The sources of the edge store that refer to a document, newest identity first, after a position: only their
/// identities, which say nothing yet of who may read them.
///
/// Each source is found by one probe of the index on the target, however many of its parts refer to the document,
/// so that a chat that cites a document a hundred thousand times costs one step, not a hundred thousand.
pub async fn raw_sources(
    conn: &mut Conn,
    target: Id,
    before: Option<Id>,
    limit: i64,
) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "WITH RECURSIVE s(source_id) AS (
           (SELECT source_id FROM related_references
             WHERE target_kind='document' AND target_id=$1 AND target_path_key=''::bytea
               AND source_id<>$1 AND source_id<$2
             ORDER BY source_id DESC LIMIT 1)
           UNION ALL
           SELECT (SELECT r.source_id FROM related_references r
                    WHERE r.target_kind='document' AND r.target_id=$1 AND r.target_path_key=''::bytea
                      AND r.source_id<>$1 AND r.source_id<s.source_id
                    ORDER BY r.source_id DESC LIMIT 1)
             FROM s WHERE s.source_id IS NOT NULL)
         SELECT source_id FROM s WHERE source_id IS NOT NULL LIMIT $3",
    )
    .bind(target)
    .bind(before.unwrap_or(Id::from_u128(u128::MAX)))
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// A source that a reader's conservative scopes reach and that still holds a live part referring to the target.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct LiveSource {
    /// `document`, `chat` or `conversation`.
    pub kind: String,
    /// Its identity.
    pub source_id: Id,
}

/// Of some sources, those that are alive, have a live mention of the document and that the conservative scopes may
/// reach; the caller still checks each one exactly.
pub async fn live_sources(
    conn: &mut Conn,
    target: Id,
    ids: &[Id],
    readable: &Json,
) -> Result<Vec<LiveSource>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT src.kind,src.source_id FROM related_items src JOIN nodes n ON n.id=src.node_id
          WHERE src.source_id=ANY($2) AND {} AND {}
            AND (src.kind<>'conversation' OR EXISTS (
              SELECT 1 FROM related_references r JOIN items i ON i.id=r.part_id AND i.thread_id=src.source_id
               WHERE r.source_kind='conversation' AND r.source_id=src.source_id AND r.target_kind='document'
                 AND r.target_id=$1 AND r.target_path_key=''::bytea AND {LIVE_ITEM} OFFSET 0))",
        access("$3", "src"),
        not_side("src")
    )))
    .bind(target)
    .bind(ids)
    .bind(readable)
    .fetch_all(conn)
    .await?)
}

/// A live part of a source that mentions the target.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PartRow {
    /// `block`, `comment`, `message` or `item`.
    pub part_kind: String,
    /// The document, chat or thread.
    pub source_id: Id,
    /// The block, comment, message or item.
    pub part_id: Id,
    /// When it was written or last changed.
    #[sqlx(rename = "moment")]
    pub at: DateTime<Utc>,
    /// The person who wrote it, where one did.
    pub by_user: Option<Id>,
    /// The agent's thread, where an agent did.
    pub by_thread: Option<Id>,
    /// That agent's name then.
    pub by_agent: Option<String>,
    /// What it holds: inline content, message parts or an item's payload.
    pub body: Json,
    /// For a comment, its discussion.
    pub discussion_id: Option<Id>,
    /// For a comment, the block its discussion is about.
    pub block_id: Option<Id>,
    /// For an item, the number of the turn it is in.
    pub turn: Option<i32>,
}

/// Where the live parts of one kind of source are found: the edge, the row it points at, and what makes that row alive.
/// `src.id` is the source and `$1` the target. The columns of `select` are the same for every kind.
struct PartKind {
    source_kind: &'static str,
    select: &'static str,
    from: &'static str,
    live: &'static str,
}

/// The complete source index order keeps mention pages on one bounded range even when a target is very popular.
const PART_KEY: &str =
    "r.source_kind,r.source_id,r.target_kind,r.target_id,r.target_path_key,r.part_id";

fn part_kinds(kind: &str) -> Vec<PartKind> {
    match kind {
        "document" => vec![
            PartKind {
                source_kind: "document",
                select:
                    "'block'::text part_kind,r.part_id,b.updated_at moment,b.updated_by by_user,
                         b.updated_by_thread by_thread,b.updated_by_agent by_agent,b.content body,
                         NULL::uuid discussion_id,NULL::uuid block_id,NULL::int turn",
                from: "related_references r JOIN document_blocks b
                    ON b.id=r.part_id AND b.document_id=r.source_id AND b.removed_at IS NULL",
                live: "true",
            },
            PartKind {
                source_kind: "comment",
                select:
                    "'comment'::text part_kind,r.part_id,c.created_at moment,c.author_id by_user,
                         NULL::uuid by_thread,NULL::text by_agent,c.content body,
                         c.discussion_id,d.block_id,NULL::int turn",
                from: "related_references r JOIN document_comments c
                    ON c.id=r.part_id AND c.document_id=r.source_id AND c.deleted_at IS NULL
                  JOIN document_discussions d ON d.id=c.discussion_id",
                live: "true",
            },
        ],
        "chat" => vec![PartKind {
            source_kind: "chat",
            select: "'message'::text part_kind,r.part_id,m.created_at moment,m.author_id by_user,
                     m.author_thread_id by_thread,m.author_agent by_agent,m.content body,
                     NULL::uuid discussion_id,NULL::uuid block_id,NULL::int turn",
            from: "related_references r JOIN chat_messages m
                ON m.id=r.part_id AND m.chat_id=r.source_id AND m.deleted_at IS NULL",
            live: "true",
        }],
        _ => vec![PartKind {
            source_kind: "conversation",
            select: "'item'::text part_kind,r.part_id,i.created_at moment,i.author_id by_user,
                     NULL::uuid by_thread,NULL::text by_agent,i.payload body,
                     NULL::uuid discussion_id,NULL::uuid block_id,tu.ordinal turn",
            from: "related_references r JOIN items i ON i.id=r.part_id AND i.thread_id=r.source_id
                  LEFT JOIN turns tu ON tu.id=i.turn_id",
            live: LIVE_ITEM,
        }],
    }
}

/// The sources among `sources` of one kind, as the identities the statements below take.
fn of_kind(sources: &[LiveSource], kind: &str) -> Vec<Id> {
    sources
        .iter()
        .filter(|source| source.kind == kind)
        .map(|source| source.source_id)
        .collect()
}

/// The kinds of source among `sources`, once each.
fn kinds_in(sources: &[LiveSource]) -> Vec<&'static str> {
    ["document", "chat", "conversation"]
        .into_iter()
        .filter(|kind| sources.iter().any(|source| source.kind == *kind))
        .collect()
}

/// The live mentions of a document in the given sources, in the order of their parts (the order they were written
/// in): at most `per_source` of each, after a part within the sources when asked for.
///
/// Each source is read as one range of the index on the source, which carries the part last: a source that mentions
/// the document a hundred thousand times costs `per_source` steps.
pub async fn parts(
    conn: &mut Conn,
    target: Id,
    sources: &[LiveSource],
    per_source: i64,
    after: Option<Id>,
) -> Result<Vec<PartRow>, DbError> {
    let after = after.unwrap_or(Id::from_u128(0));
    let mut found: Vec<PartRow> = Vec::new();
    for kind in kinds_in(sources) {
        for part in part_kinds(kind) {
            found.extend(
                sqlx::query_as::<_, PartRow>(sqlx::AssertSqlSafe(format!(
                    "SELECT src.id source_id,p.part_kind,p.part_id,p.moment,p.by_user,p.by_thread,p.by_agent,p.body,
                            p.discussion_id,p.block_id,p.turn
                       FROM unnest($2::uuid[]) src(id) CROSS JOIN LATERAL (
                         SELECT {} FROM {} WHERE
                           ({PART_KEY})>($5::text,src.id,'document',$1,''::bytea,$4::uuid)
                           AND ({PART_KEY})<=($5::text,src.id,'document',$1,''::bytea,$6::uuid)
                           AND {} ORDER BY {PART_KEY} LIMIT $3) p",
                    part.select, part.from, part.live
                )))
                .bind(target)
                .bind(of_kind(sources, kind))
                .bind(per_source)
                .bind(after)
                .bind(part.source_kind)
                // The inclusive final UUID closes the same source/target prefix, including its last possible part.
                .bind(Id::from_u128(u128::MAX))
                .fetch_all(&mut *conn)
                .await?,
            );
        }
    }
    found.sort_by(|left, right| {
        right
            .source_id
            .cmp(&left.source_id)
            .then(left.part_id.cmp(&right.part_id))
    });
    // A document reads its blocks and its comments as one: keep the first `per_source` of the two together.
    let mut kept: Vec<PartRow> = Vec::with_capacity(found.len());
    let mut run = (Id::from_u128(0), 0_i64);
    for part in found {
        if run.0 != part.source_id {
            run = (part.source_id, 0);
        }
        run.1 += 1;
        if run.1 <= per_source {
            kept.push(part);
        }
    }
    Ok(kept)
}

/// How many live parts of each source mention the document, counted up to `limit` each: a source that holds more
/// reads as `limit`. The count stops there, so that it costs the same for a source that is cited a hundred
/// thousand times as for one cited `limit` times.
pub async fn mention_counts(
    conn: &mut Conn,
    target: Id,
    sources: &[LiveSource],
    limit: i64,
) -> Result<HashMap<Id, i64>, DbError> {
    let mut counts: HashMap<Id, i64> = HashMap::new();
    for kind in kinds_in(sources) {
        for part in part_kinds(kind) {
            let rows: Vec<(Id, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "SELECT src.id,(SELECT count(*) FROM (SELECT 1 FROM {} WHERE
                    r.source_kind=$4 AND r.source_id=src.id AND r.target_kind='document'
                    AND r.target_id=$1 AND r.target_path_key=''::bytea AND {} LIMIT $3) c)
                   FROM unnest($2::uuid[]) src(id)",
                part.from, part.live
            )))
            .bind(target)
            .bind(of_kind(sources, kind))
            .bind(limit)
            .bind(part.source_kind)
            .fetch_all(&mut *conn)
            .await?;
            for (source, count) in rows {
                *counts.entry(source).or_default() += count;
            }
        }
    }
    for count in counts.values_mut() {
        *count = (*count).min(limit);
    }
    Ok(counts)
}

/// One thing a document refers to, with the first part of the document that does.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct OutEdge {
    /// `document`, `node`, `chat`, `file` or `environment_file`.
    pub target_kind: String,
    /// The thing.
    pub target_id: Id,
    /// The digest of the path of a workspace file; empty for the rest.
    pub target_path_key: Vec<u8>,
    /// How many live parts of the document refer to it.
    pub mentions: i64,
    /// The first of them, by when it was written, then identity.
    pub first_part: Id,
    /// Whether that part is a block or a comment.
    pub first_kind: String,
    /// When that part was written or last changed.
    pub first_at: DateTime<Utc>,
}

/// Where a page of the targets of a document goes on, in the source index's target-key order.
#[derive(Debug, Clone)]
pub struct OutPosition {
    /// Its identity.
    pub target_id: Id,
    /// Its kind.
    pub target_kind: String,
    /// The digest of its path, empty but for a workspace file.
    pub path_key: Vec<u8>,
}

/// What a document's live blocks and comments refer to, once each in target-key order: select a page of counter
/// keys before looking up each first occurrence, however many parts refer to those targets.
// The composite lower and source upper bounds together select exactly one source.
// Keep the source in the order so a deep cursor seeks directly to its target.
pub async fn out_edges(
    conn: &mut Conn,
    document: Id,
    after: Option<&OutPosition>,
    limit: i64,
) -> Result<Vec<OutEdge>, DbError> {
    Ok(sqlx::query_as(
        "WITH candidates AS MATERIALIZED (
           (SELECT target_kind,target_id,target_path_key FROM related_reference_targets
             WHERE source_kind='document' AND source_id<=$1 AND target_id<>$1
               AND (source_id,target_kind,target_id,target_path_key)>($1,$2::text,$3::uuid,$4::bytea)
             ORDER BY source_id,target_kind,target_id,target_path_key LIMIT $5)
           UNION
           (SELECT target_kind,target_id,target_path_key FROM related_reference_targets
             WHERE source_kind='comment' AND source_id<=$1 AND target_id<>$1
               AND (source_id,target_kind,target_id,target_path_key)>($1,$2::text,$3::uuid,$4::bytea)
             ORDER BY source_id,target_kind,target_id,target_path_key LIMIT $5)
         ), page AS MATERIALIZED (
           SELECT * FROM candidates ORDER BY target_kind,target_id,target_path_key LIMIT $5
         )
         SELECT p.target_kind,p.target_id,p.target_path_key,COALESCE(d.links,0)+COALESCE(c.links,0) mentions,
                first.part_id first_part,CASE first.source_kind WHEN 'document' THEN 'block' ELSE 'comment' END first_kind,
                first.source_moment first_at
           FROM page p
           LEFT JOIN related_reference_targets d ON d.source_kind='document' AND d.source_id=$1
             AND (d.target_kind,d.target_id,d.target_path_key)=(p.target_kind,p.target_id,p.target_path_key)
           LEFT JOIN related_reference_targets c ON c.source_kind='comment' AND c.source_id=$1
             AND (c.target_kind,c.target_id,c.target_path_key)=(p.target_kind,p.target_id,p.target_path_key)
           CROSS JOIN LATERAL (
             SELECT r.part_id,r.source_kind,r.source_moment FROM related_references r
               WHERE r.source_kind IN ('document','comment') AND r.source_moment IS NOT NULL
                 AND (r.source_id,r.target_kind,r.target_id,r.target_path_key)=
                     ($1,p.target_kind,p.target_id,p.target_path_key)
               ORDER BY r.source_id,r.target_kind,r.target_id,r.target_path_key,r.source_moment,r.part_id,r.source_kind
               LIMIT 1) first
          ORDER BY p.target_kind,p.target_id,p.target_path_key",
    )
    .bind(document)
    .bind(after.map_or("", |position| position.target_kind.as_str()))
    .bind(after.map_or(Id::nil(), |position| position.target_id))
    .bind(after.map_or(&[][..], |position| position.path_key.as_slice()))
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// What a part holds, to read how a reference in it was written.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PartBody {
    /// The block or comment.
    pub part_id: Id,
    /// Its inline content.
    pub body: Json,
}

/// The inline content of some blocks and comments of one document.
pub async fn part_bodies(
    conn: &mut Conn,
    document: Id,
    ids: &[Id],
) -> Result<Vec<PartBody>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id part_id,content body FROM document_blocks WHERE document_id=$1 AND id=ANY($2) AND removed_at IS NULL
         UNION ALL
         SELECT id,content FROM document_comments WHERE document_id=$1 AND id=ANY($2) AND deleted_at IS NULL",
    )
    .bind(document)
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

/// A reference between two whole things, in the direction it is written.
#[derive(Debug, Clone, PartialEq, Eq, Hash, sqlx::FromRow)]
pub struct Link {
    /// `document`, `chat` or `conversation`: what refers.
    pub from_kind: String,
    /// What refers.
    pub from_id: Id,
    /// `document`, `chat` or `conversation`: what is referred to.
    pub to_kind: String,
    /// What is referred to.
    pub to_id: Id,
}

/// A whole thing by its kind (`document`, `chat` or `conversation`) and identity.
pub type Whole<'a> = (&'a str, Id);

/// Whole things that refer to the given ones or are referred to by them, and that the conservative scopes may reach:
/// their live references only, each pair once, at most `limit` pairs. The caller checks the far ends exactly.
///
/// Each given thing offers at most `per_thing` neighbours in each direction (and kind), found one probe of an index
/// at a time, so that a thing that is cited a hundred thousand times costs `per_thing` probes and the whole costs
/// no more than the things given times `per_thing`.
pub async fn neighbours(
    conn: &mut Conn,
    frontier: &[Whole<'_>],
    readable: &Json,
    per_thing: i64,
    limit: i64,
) -> Result<Vec<Link>, DbError> {
    // The far end is the entry the scopes are applied to; the near end is already known to be readable.
    let far = format!("{} AND {}", access("$3", "far"), not_side("far"));
    let (kinds, ids): (Vec<&str>, Vec<Id>) = frontier.iter().copied().unzip();
    let sql = format!(
        "WITH RECURSIVE
         fr AS (SELECT f.id,f.kind,CASE f.kind WHEN 'conversation' THEN 'node' ELSE f.kind END tkind
                  FROM unnest($1::uuid[],$2::text[]) f(id,kind)),
         inb(fid,fkind,tkind,src,n) AS (
           SELECT fr.id,fr.kind,fr.tkind,first.source_id,1 FROM fr
             CROSS JOIN LATERAL (SELECT r.source_id FROM related_references r
                WHERE r.target_kind=fr.tkind AND r.target_id=fr.id AND r.target_path_key=''::bytea AND r.source_id<>fr.id
                ORDER BY r.source_id DESC LIMIT 1) first
           UNION ALL
           SELECT ib.fid,ib.fkind,ib.tkind,nxt.source_id,ib.n+1 FROM inb ib
             CROSS JOIN LATERAL (SELECT r.source_id FROM related_references r
                WHERE r.target_kind=ib.tkind AND r.target_id=ib.fid AND r.target_path_key=''::bytea
                  AND r.source_id<>ib.fid AND r.source_id<ib.src
                ORDER BY r.source_id DESC LIMIT 1) nxt
            WHERE ib.n<$4),
         outb(fid,fkind,skind,tkind,tgt,n) AS (
           SELECT fr.id,fr.kind,sk.skind,tk.tkind,first.target_id,1 FROM fr
             CROSS JOIN LATERAL (SELECT unnest(CASE fr.kind WHEN 'document' THEN ARRAY['document','comment']
                                                 ELSE ARRAY[fr.kind] END) skind) sk
             CROSS JOIN (VALUES ('document'),('node'),('chat')) tk(tkind)
             CROSS JOIN LATERAL (SELECT r.target_id FROM related_references r
                WHERE r.source_kind=sk.skind AND r.source_id=fr.id AND r.target_kind=tk.tkind
                  AND r.target_path_key=''::bytea AND r.target_id<>fr.id
                ORDER BY r.target_id LIMIT 1) first
           UNION ALL
           SELECT ob.fid,ob.fkind,ob.skind,ob.tkind,nxt.target_id,ob.n+1 FROM outb ob
             CROSS JOIN LATERAL (SELECT r.target_id FROM related_references r
                WHERE r.source_kind=ob.skind AND r.source_id=ob.fid AND r.target_kind=ob.tkind
                  AND r.target_path_key=''::bytea AND r.target_id<>ob.fid AND r.target_id>ob.tgt
                ORDER BY r.target_id LIMIT 1) nxt
            WHERE ob.n<$4)
         SELECT DISTINCT from_kind,from_id,to_kind,to_id FROM (
           SELECT far.kind from_kind,far.source_id from_id,ib.fkind to_kind,ib.fid to_id
             FROM inb ib
             JOIN related_items far ON far.source_id=ib.src AND far.source_id=ANY(ARRAY(SELECT src FROM inb))
             JOIN nodes n ON n.id=far.node_id
            WHERE {far}
              AND (far.kind<>'conversation' OR EXISTS (SELECT 1 FROM related_references r
                    JOIN items i ON i.id=r.part_id AND i.thread_id=r.source_id
                   WHERE r.source_kind='conversation' AND r.source_id=far.source_id AND r.target_kind=ib.tkind
                     AND r.target_id=ib.fid AND r.target_path_key=''::bytea AND {LIVE_ITEM} OFFSET 0))
           UNION
           SELECT ob.fkind,ob.fid,far.kind,far.source_id
             FROM outb ob
             JOIN related_items far ON far.source_id=ob.tgt AND far.source_id=ANY(ARRAY(SELECT tgt FROM outb))
              AND far.kind=CASE ob.tkind WHEN 'node' THEN 'conversation' ELSE ob.tkind END
             JOIN nodes n ON n.id=far.node_id
            WHERE {far}
              AND (ob.skind<>'conversation' OR EXISTS (SELECT 1 FROM related_references r
                    JOIN items i ON i.id=r.part_id AND i.thread_id=r.source_id
                   WHERE r.source_kind='conversation' AND r.source_id=ob.fid AND r.target_kind=ob.tkind
                     AND r.target_id=ob.tgt AND r.target_path_key=''::bytea AND {LIVE_ITEM} OFFSET 0))
         ) pairs ORDER BY from_id,to_id LIMIT $5"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(ids)
        .bind(kinds)
        .bind(readable)
        .bind(per_thing)
        .bind(limit)
        .fetch_all(conn)
        .await?)
}

/// The references among a set of whole things, each pair once, in the direction it is written: one probe of the
/// index on the source for each ordered pair, so that the cost is the square of the number of things and nothing
/// that depends on how often any of them is referred to.
pub async fn among(conn: &mut Conn, members: &[Whole<'_>]) -> Result<Vec<Link>, DbError> {
    let (kinds, ids): (Vec<&str>, Vec<Id>) = members.iter().copied().unzip();
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT DISTINCT a.kind from_kind,a.id from_id,b.kind to_kind,b.id to_id
           FROM unnest($1::uuid[],$2::text[]) a(id,kind)
           CROSS JOIN unnest($1::uuid[],$2::text[]) b(id,kind)
          WHERE a.id<>b.id
            AND (a.kind<>'conversation' OR NOT EXISTS (SELECT 1 FROM threads side WHERE side.node_id=a.id AND side.side_of IS NOT NULL))
            AND (b.kind<>'conversation' OR NOT EXISTS (SELECT 1 FROM threads side WHERE side.node_id=b.id AND side.side_of IS NOT NULL))
            AND EXISTS (SELECT 1 FROM related_references r
                 WHERE r.source_kind=ANY(CASE a.kind WHEN 'document' THEN ARRAY['document','comment'] ELSE ARRAY[a.kind] END)
                   AND r.source_id=a.id AND r.target_kind=CASE b.kind WHEN 'conversation' THEN 'node' ELSE b.kind END
                   AND r.target_id=b.id AND r.target_path_key=''::bytea
                   AND (r.source_kind<>'conversation' OR EXISTS (SELECT 1 FROM items i
                         WHERE i.id=r.part_id AND i.thread_id=r.source_id AND {LIVE_ITEM} OFFSET 0)))
          ORDER BY from_id,to_id"
    )))
    .bind(ids)
    .bind(kinds)
    .fetch_all(conn)
    .await?)
}

/// The tabs a document's owner chose for it, when they did.
pub async fn own_tabs(conn: &mut Conn, document: Id) -> Result<Option<Vec<String>>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT shown FROM document_links_display WHERE document_id=$1")
            .bind(document)
            .fetch_optional(conn)
            .await?,
    )
}

/// Records the tabs a document shows, or takes the choice back with `None`.
pub async fn set_own_tabs(
    conn: &mut Conn,
    document: Id,
    shown: Option<&[String]>,
    by: Id,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    match shown {
        Some(shown) => {
            sqlx::query(
                "INSERT INTO document_links_display (document_id,shown,set_by,set_at) VALUES ($1,$2,$3,$4)
                 ON CONFLICT (document_id) DO UPDATE SET shown=EXCLUDED.shown,set_by=EXCLUDED.set_by,set_at=EXCLUDED.set_at",
            )
            .bind(document)
            .bind(shown)
            .bind(by)
            .bind(at)
            .execute(conn)
            .await?;
        }
        None => {
            sqlx::query("DELETE FROM document_links_display WHERE document_id=$1")
                .bind(document)
                .execute(conn)
                .await?;
        }
    }
    Ok(())
}
