//! Discussions beside a document and the comments in them: opened on a block, answered, reacted to, resolved, edited and deleted.
//!
//! Nothing here is a block: the text of the document, its versions and the
//! stamps of its blocks are left alone. A discussion and its comments are
//! stamped from the document's sequence all the same, each change taking the
//! next number and stamping the discussion it is in, so that whoever has the
//! document open sees a comment arrive as they see a block change.
//!
//! The first comment of a discussion and every answer carry an identifier
//! chosen by the client; a request sent twice finds the comment that has it and
//! answers as the first did.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use orch_api_contract::chats::Reaction;
use orch_documents::comments::{self, Reactions};
use orch_types::{Id, Json, Seq};

use crate::{Conn, DbError};

const DISCUSSION_COLUMNS: &str =
    "x.id, x.document_id, x.block_id, x.quote, x.resolved_at, x.resolved_by,
    x.created_by, x.created_at, x.seq_updated";
const COMMENT_COLUMNS: &str =
    "c.id, c.discussion_id, c.document_id, c.author_id, c.content, c.reactions,
    c.edited_at, c.deleted_at, c.created_at, c.seq_updated";

/// A discussion, as stored.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct DiscussionRow {
    /// Its identifier.
    pub id: Id,
    /// The document.
    pub document_id: Id,
    /// The block it is about; none once that block was purged.
    pub block_id: Option<Id>,
    /// The words it was made on; empty for the whole block.
    pub quote: String,
    /// When it was resolved.
    pub resolved_at: Option<DateTime<Utc>>,
    /// Who resolved it.
    pub resolved_by: Option<Id>,
    /// Who started it.
    pub created_by: Id,
    /// When it was started.
    pub created_at: DateTime<Utc>,
    /// The document sequence at which it last changed.
    pub seq_updated: Seq,
}

/// A comment, as stored.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, sqlx::FromRow)]
pub struct CommentRow {
    /// Its identifier.
    pub id: Id,
    /// The discussion it is in.
    pub discussion_id: Id,
    /// The document.
    pub document_id: Id,
    /// Who wrote it.
    pub author_id: Id,
    /// What it says: inline content; empty once deleted.
    pub content: Json,
    /// Emoji and the people who put each, in the order they did.
    pub reactions: Json,
    /// When it was last edited.
    pub edited_at: Option<DateTime<Utc>>,
    /// When it was deleted.
    pub deleted_at: Option<DateTime<Utc>>,
    /// When it was written.
    pub created_at: DateTime<Utc>,
    /// The document sequence at which it last changed.
    pub seq_updated: Seq,
}

impl CommentRow {
    /// The reactions, in the order they were put on.
    #[must_use]
    pub fn reaction_list(&self) -> Vec<Reaction> {
        let Some(map) = self.reactions.as_object() else {
            return Vec::new();
        };
        map.iter()
            .map(|(emoji, users)| Reaction {
                emoji: emoji.clone(),
                users: users
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|user| user.as_str()?.parse().ok())
                    .collect(),
            })
            .collect()
    }
}

/// A discussion with the comments requested by its read path.
#[derive(Debug, Clone, PartialEq)]
pub struct Full {
    /// The discussion.
    pub discussion: DiscussionRow,
    /// Its comments, in the order they were written.
    pub comments: Vec<CommentRow>,
}

/// The comments of the discussions `ids`, in the order they were written.
pub async fn comments_of(conn: &mut Conn, ids: &[Id]) -> Result<Vec<CommentRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COMMENT_COLUMNS} FROM document_comments c WHERE c.discussion_id = ANY($1)
          ORDER BY c.created_at, c.id"
    )))
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

async fn with_comments(
    conn: &mut Conn,
    discussions: Vec<DiscussionRow>,
) -> Result<Vec<Full>, DbError> {
    let ids: Vec<Id> = discussions.iter().map(|each| each.id).collect();
    let comments: Vec<CommentRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COMMENT_COLUMNS} FROM unnest($1::uuid[]) AS wanted(id)
         CROSS JOIN LATERAL (SELECT * FROM document_comments
            WHERE discussion_id=wanted.id ORDER BY created_at DESC,id DESC LIMIT 50) c
         ORDER BY c.discussion_id,c.created_at,c.id"
    )))
    .bind(&ids)
    .fetch_all(conn)
    .await?;
    Ok(group_comments(discussions, comments))
}

fn group_comments(discussions: Vec<DiscussionRow>, comments: Vec<CommentRow>) -> Vec<Full> {
    let mut grouped: HashMap<Id, Vec<CommentRow>> = HashMap::new();
    for comment in comments {
        grouped
            .entry(comment.discussion_id)
            .or_default()
            .push(comment);
    }
    discussions
        .into_iter()
        .map(|discussion| Full {
            comments: grouped.remove(&discussion.id).unwrap_or_default(),
            discussion,
        })
        .collect()
}

/// One stable comment window. A request holds at most the requested page and sentinel.
pub async fn comments_page(
    conn: &mut Conn,
    discussion: Id,
    after: Option<Cursor>,
    limit: i64,
) -> Result<Vec<CommentRow>, DbError> {
    let condition = if after.is_some() {
        "AND (c.created_at,c.id)<($3,$4)"
    } else {
        ""
    };
    let query = format!(
        "SELECT {COMMENT_COLUMNS} FROM document_comments c
        WHERE c.discussion_id=$1 {condition} ORDER BY c.created_at DESC,c.id DESC LIMIT $2"
    );
    let mut query = sqlx::query_as(sqlx::AssertSqlSafe(query))
        .bind(discussion)
        .bind(limit);
    if let Some(after) = after {
        query = query.bind(after.at).bind(after.id);
    }
    Ok(query.fetch_all(conn).await?)
}

/// Bounded changed discussions for sync and live frames; complete histories use the export path.
pub async fn changed_previews(
    conn: &mut Conn,
    document: Id,
    since: Seq,
    upto: Seq,
    limit: i64,
) -> Result<Vec<Full>, DbError> {
    let rows = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {DISCUSSION_COLUMNS} FROM document_discussions x
         WHERE x.document_id=$1 AND x.seq_updated>$2 AND x.seq_updated<=$3
         ORDER BY x.seq_updated,x.id LIMIT $4"
    )))
    .bind(document)
    .bind(since)
    .bind(upto)
    .bind(limit)
    .fetch_all(&mut *conn)
    .await?;
    with_comments(conn, rows).await
}

/// One discussion of a document, with its latest fifty comments.
pub async fn get(conn: &mut Conn, document: Id, discussion: Id) -> Result<Option<Full>, DbError> {
    let rows: Vec<DiscussionRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {DISCUSSION_COLUMNS} FROM document_discussions x WHERE x.document_id = $1 AND x.id = $2"
    )))
    .bind(document)
    .bind(discussion)
    .fetch_all(&mut *conn)
    .await?;
    Ok(with_comments(conn, rows).await?.into_iter().next())
}

/// The discussions in which something was stamped above `since` and up to
/// `upto`, each with all its comments.
pub async fn changed(
    conn: &mut Conn,
    document: Id,
    since: Seq,
    upto: Seq,
) -> Result<Vec<Full>, DbError> {
    let rows: Vec<DiscussionRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {DISCUSSION_COLUMNS} FROM document_discussions x
          WHERE x.document_id = $1 AND x.seq_updated > $2 AND x.seq_updated <= $3
          ORDER BY x.seq_updated, x.id"
    )))
    .bind(document)
    .bind(since)
    .bind(upto)
    .fetch_all(&mut *conn)
    .await?;
    let ids: Vec<Id> = rows.iter().map(|row| row.id).collect();
    let comments = comments_of(conn, &ids).await?;
    Ok(group_comments(rows, comments))
}

/// How many discussions are stamped above `since`.
pub async fn count_changed(conn: &mut Conn, document: Id, since: Seq) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM document_discussions WHERE document_id = $1 AND seq_updated > $2",
    )
    .bind(document)
    .bind(since)
    .fetch_one(conn)
    .await?)
}

/// Which discussions a list holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Which {
    /// Those nobody resolved.
    Open,
    /// All of them.
    All,
    /// Only the resolved ones.
    Resolved,
}

/// Where a list resumes: the moment of the newest answer of the last
/// discussion shown, and its identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    /// The moment.
    pub at: DateTime<Utc>,
    /// The identifier.
    pub id: Id,
}

/// The discussions beside a document, the newest answer first, each with its comments.
pub async fn list(
    conn: &mut Conn,
    document: Id,
    which: Which,
    after: Option<Cursor>,
    limit: i64,
) -> Result<Vec<(Full, DateTime<Utc>)>, DbError> {
    let filter = match which {
        Which::Open => "AND x.resolved_at IS NULL",
        Which::All => "",
        Which::Resolved => "AND x.resolved_at IS NOT NULL",
    };
    let cursor = if after.is_some() {
        "AND (x.last_activity_at,x.id)<($3,$4)"
    } else {
        ""
    };
    let mut query = sqlx::query_as::<_, (Id, DateTime<Utc>)>(sqlx::AssertSqlSafe(format!(
        "SELECT x.id,x.last_activity_at FROM document_discussions x
         WHERE x.document_id=$1 {filter} {cursor}
         ORDER BY x.last_activity_at DESC,x.id DESC LIMIT $2"
    )))
    .bind(document)
    .bind(limit);
    if let Some(after) = after {
        query = query.bind(after.at).bind(after.id);
    }
    let found = query.fetch_all(&mut *conn).await?;
    let ids: Vec<Id> = found.iter().map(|(id, _)| *id).collect();
    let rows: Vec<DiscussionRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {DISCUSSION_COLUMNS} FROM document_discussions x WHERE x.id=ANY($1)"
    )))
    .bind(&ids)
    .fetch_all(&mut *conn)
    .await?;
    let mut rows: HashMap<Id, DiscussionRow> = rows.into_iter().map(|row| (row.id, row)).collect();
    let rows: Vec<_> = found
        .into_iter()
        .filter_map(|(id, active)| rows.remove(&id).map(|row| (row, active)))
        .collect();
    let actives: Vec<DateTime<Utc>> = rows.iter().map(|(_, active)| *active).collect();
    let full = with_comments(conn, rows.into_iter().map(|(each, _)| each).collect()).await?;
    Ok(full.into_iter().zip(actives).collect())
}

/// One comment of a document.
pub async fn comment(
    conn: &mut Conn,
    document: Id,
    comment: Id,
) -> Result<Option<CommentRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COMMENT_COLUMNS} FROM document_comments c WHERE c.document_id = $1 AND c.id = $2"
    )))
    .bind(document)
    .bind(comment)
    .fetch_optional(conn)
    .await?)
}

/// Whether `user` wrote in the discussion.
pub async fn wrote_in(conn: &mut Conn, discussion: Id, user: Id) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM document_comments WHERE discussion_id = $1 AND author_id = $2)",
    )
    .bind(discussion)
    .bind(user)
    .fetch_one(conn)
    .await?)
}

/// Whoever is to be told of an answer in a discussion: who started it, who
/// wrote in it, once each.
#[cfg(test)]
pub async fn participants(conn: &mut Conn, discussion: Id) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT user_id FROM document_discussion_participants WHERE discussion_id=$1
         ORDER BY CASE WHEN user_id=(SELECT created_by FROM document_discussions WHERE id=$1) THEN 0 ELSE 1 END,user_id",
    )
    .bind(discussion)
    .fetch_all(conn)
    .await?)
}

/// What a comment to be written is.
#[derive(Debug, Clone)]
pub struct NewComment<'a> {
    /// Its identifier, which the client chose.
    pub id: Id,
    /// What it says, checked.
    pub content: &'a Json,
}

/// How opening a discussion ended.
#[derive(Debug, Clone, PartialEq)]
pub enum Opened {
    /// There is no such live block in the document, or the document is gone.
    NoSuchBlock,
    /// The identifier of the first comment is another person's.
    Taken,
    /// The request was made before: the discussion it made.
    Repeated(Box<Full>),
    /// The discussion was opened.
    Created(Box<Full>),
}

/// Opens a discussion on a live block, with its first comment.
pub async fn open(
    conn: &mut Conn,
    document: Id,
    block: Id,
    quote: &str,
    author: Id,
    first: &NewComment<'_>,
    now: DateTime<Utc>,
) -> Result<Opened, DbError> {
    if let Some(known) = comment(conn, document, first.id).await? {
        return Ok(if known.author_id == author {
            match get(conn, document, known.discussion_id).await? {
                Some(full) => Opened::Repeated(Box::new(full)),
                None => Opened::Taken,
            }
        } else {
            Opened::Taken
        });
    }
    let live: Option<Id> = sqlx::query_scalar(
        "SELECT id FROM document_blocks WHERE document_id = $1 AND id = $2 AND removed_at IS NULL",
    )
    .bind(document)
    .bind(block)
    .fetch_optional(&mut *conn)
    .await?;
    if live.is_none() {
        return Ok(Opened::NoSuchBlock);
    }
    let Some(seq) = super::next_seq(conn, document).await? else {
        return Ok(Opened::NoSuchBlock);
    };
    let discussion = orch_types::new_id();
    sqlx::query(
        "INSERT INTO document_discussions (id, document_id, block_id, quote, created_by, created_at, seq_updated)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(discussion)
    .bind(document)
    .bind(block)
    .bind(quote)
    .bind(author)
    .bind(now)
    .bind(seq)
    .execute(&mut *conn)
    .await?;
    insert_comment(conn, document, discussion, author, first, now, seq).await?;
    super::notify(conn, document, seq).await?;
    let full = get(conn, document, discussion).await?;
    Ok(full.map_or(Opened::NoSuchBlock, |full| Opened::Created(Box::new(full))))
}

async fn insert_comment(
    conn: &mut Conn,
    document: Id,
    discussion: Id,
    author: Id,
    new: &NewComment<'_>,
    now: DateTime<Utc>,
    seq: Seq,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO document_comments (id, discussion_id, document_id, author_id, content, created_at, seq_updated)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(new.id)
    .bind(discussion)
    .bind(document)
    .bind(author)
    .bind(new.content)
    .bind(now)
    .bind(seq)
    .execute(conn)
    .await?;
    Ok(())
}

/// How answering ended.
#[derive(Debug, Clone, PartialEq)]
pub enum Answered {
    /// There is no such discussion in the document.
    NoSuchDiscussion,
    /// The identifier of the comment is another person's.
    Taken,
    /// The request was made before: the comment it made.
    Repeated(Box<CommentRow>),
    /// The comment was written.
    Created {
        /// The comment.
        comment: Box<CommentRow>,
        /// The document's sequence after it.
        seq: Seq,
    },
}

/// Answers in a discussion.
pub async fn answer(
    conn: &mut Conn,
    document: Id,
    discussion: Id,
    author: Id,
    new: &NewComment<'_>,
    now: DateTime<Utc>,
) -> Result<Answered, DbError> {
    if let Some(known) = comment(conn, document, new.id).await? {
        return Ok(
            if known.author_id == author && known.discussion_id == discussion {
                Answered::Repeated(Box::new(known))
            } else {
                Answered::Taken
            },
        );
    }
    let exists: Option<Id> = sqlx::query_scalar(
        "SELECT id FROM document_discussions WHERE document_id = $1 AND id = $2",
    )
    .bind(document)
    .bind(discussion)
    .fetch_optional(&mut *conn)
    .await?;
    if exists.is_none() {
        return Ok(Answered::NoSuchDiscussion);
    }
    let Some(seq) = super::next_seq(conn, document).await? else {
        return Ok(Answered::NoSuchDiscussion);
    };
    insert_comment(conn, document, discussion, author, new, now, seq).await?;
    stamp_discussion(conn, discussion, seq).await?;
    super::notify(conn, document, seq).await?;
    let written = comment(conn, document, new.id).await?;
    Ok(
        written.map_or(Answered::NoSuchDiscussion, |comment| Answered::Created {
            comment: Box::new(comment),
            seq,
        }),
    )
}

async fn stamp_discussion(conn: &mut Conn, discussion: Id, seq: Seq) -> Result<(), DbError> {
    sqlx::query("UPDATE document_discussions SET seq_updated = $2 WHERE id = $1")
        .bind(discussion)
        .bind(seq)
        .execute(conn)
        .await?;
    Ok(())
}

/// Resolves a discussion, or opens it again. The sequence it was stamped with,
/// or none where there is no such discussion.
pub async fn resolve(
    conn: &mut Conn,
    document: Id,
    discussion: Id,
    resolved: bool,
    by: Id,
    now: DateTime<Utc>,
) -> Result<Option<Seq>, DbError> {
    let exists: Option<Id> = sqlx::query_scalar(
        "SELECT id FROM document_discussions WHERE document_id = $1 AND id = $2",
    )
    .bind(document)
    .bind(discussion)
    .fetch_optional(&mut *conn)
    .await?;
    if exists.is_none() {
        return Ok(None);
    }
    let Some(seq) = super::next_seq(conn, document).await? else {
        return Ok(None);
    };
    sqlx::query(
        "UPDATE document_discussions SET resolved_at = $3, resolved_by = $4, seq_updated = $5 WHERE id = $1 AND document_id = $2",
    )
    .bind(discussion)
    .bind(document)
    .bind(resolved.then_some(now))
    .bind(resolved.then_some(by))
    .bind(seq)
    .execute(&mut *conn)
    .await?;
    super::notify(conn, document, seq).await?;
    Ok(Some(seq))
}

async fn restamp(
    conn: &mut Conn,
    document: Id,
    comment_id: Id,
) -> Result<Option<(Seq, Id)>, DbError> {
    let Some(known) = comment(conn, document, comment_id).await? else {
        return Ok(None);
    };
    let Some(seq) = super::next_seq(conn, document).await? else {
        return Ok(None);
    };
    stamp_discussion(conn, known.discussion_id, seq).await?;
    Ok(Some((seq, known.discussion_id)))
}

/// Edits a comment that is not deleted.
pub async fn edit(
    conn: &mut Conn,
    document: Id,
    comment_id: Id,
    content: &Json,
    now: DateTime<Utc>,
) -> Result<Option<CommentRow>, DbError> {
    let Some(known) = comment(conn, document, comment_id).await? else {
        return Ok(None);
    };
    if known.deleted_at.is_some() {
        return Ok(None);
    }
    let Some((seq, _)) = restamp(conn, document, comment_id).await? else {
        return Ok(None);
    };
    sqlx::query(
        "UPDATE document_comments SET content = $2, edited_at = $3, seq_updated = $4 WHERE id = $1",
    )
    .bind(comment_id)
    .bind(content)
    .bind(now)
    .bind(seq)
    .execute(&mut *conn)
    .await?;
    super::notify(conn, document, seq).await?;
    comment(conn, document, comment_id).await
}

/// Deletes a comment: the row stays as an emptied line.
pub async fn delete(
    conn: &mut Conn,
    document: Id,
    comment_id: Id,
    now: DateTime<Utc>,
) -> Result<Option<CommentRow>, DbError> {
    let Some(known) = comment(conn, document, comment_id).await? else {
        return Ok(None);
    };
    if known.deleted_at.is_some() {
        return Ok(Some(known));
    }
    let Some((seq, _)) = restamp(conn, document, comment_id).await? else {
        return Ok(None);
    };
    sqlx::query(
        "UPDATE document_comments SET content = '[]'::jsonb, deleted_at = $2, seq_updated = $3 WHERE id = $1",
    )
    .bind(comment_id)
    .bind(now)
    .bind(seq)
    .execute(&mut *conn)
    .await?;
    super::notify(conn, document, seq).await?;
    comment(conn, document, comment_id).await
}

/// Puts a person's reaction to a comment on, or takes it off. Nothing is
/// stamped where it changes nothing.
pub async fn react(
    conn: &mut Conn,
    document: Id,
    comment_id: Id,
    emoji: &str,
    user: Id,
    on: bool,
) -> Result<Option<CommentRow>, DbError> {
    let Some(known) = comment(conn, document, comment_id).await? else {
        return Ok(None);
    };
    if known.deleted_at.is_some() {
        return Ok(None);
    }
    let mut reactions: Reactions = known.reaction_list();
    if !comments::react(&mut reactions, emoji, user, on) {
        return Ok(Some(known));
    }
    let stored: serde_json::Map<String, Json> = reactions
        .iter()
        .map(|each| (each.emoji.clone(), serde_json::json!(each.users)))
        .collect();
    let Some((seq, _)) = restamp(conn, document, comment_id).await? else {
        return Ok(None);
    };
    sqlx::query("UPDATE document_comments SET reactions = $2, seq_updated = $3 WHERE id = $1")
        .bind(comment_id)
        .bind(Json::Object(stored))
        .bind(seq)
        .execute(&mut *conn)
        .await?;
    super::notify(conn, document, seq).await?;
    comment(conn, document, comment_id).await
}

#[cfg(test)]
mod tests {
    use super::super::testing::place;
    use super::*;
    use orch_testkit::{clock, database::TestDb};
    use orch_types::new_id;
    use serde_json::json;

    fn said(words: &str) -> Json {
        json!([{ "type": "text", "text": words }])
    }

    async fn a_block(conn: &mut Conn, document: Id) -> Id {
        let id = new_id();
        sqlx::query(
            "INSERT INTO document_blocks (id, document_id, position, type, props, content, text, seq_created, seq_updated)
             VALUES ($1, $2, 1, 'paragraph', '{}', '[]', 'The second paragraph', 0, 0)",
        )
        .bind(id)
        .bind(document)
        .execute(&mut *conn)
        .await
        .unwrap();
        id
    }

    #[tokio::test]
    async fn a_discussion_is_opened_on_a_block_answered_and_resolved_and_each_change_takes_a_number()
     {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let now = clock::start();
        let here = place(&mut conn, now).await;
        let block = a_block(&mut conn, here.document).await;
        let first = said("Why this?");
        let opened = open(
            &mut conn,
            here.document,
            block,
            "second paragraph",
            here.bob,
            &NewComment {
                id: new_id(),
                content: &first,
            },
            now,
        )
        .await
        .unwrap();
        let Opened::Created(full) = opened else {
            panic!("created")
        };
        assert_eq!(full.discussion.seq_updated, 1);
        assert_eq!(full.comments.len(), 1);
        assert_eq!(full.discussion.quote, "second paragraph");
        let reply = said("Because.");
        let Answered::Created {
            comment: second,
            seq,
        } = answer(
            &mut conn,
            here.document,
            full.discussion.id,
            here.ada,
            &NewComment {
                id: new_id(),
                content: &reply,
            },
            now + chrono::Duration::minutes(1),
        )
        .await
        .unwrap()
        else {
            panic!("created")
        };
        assert_eq!(seq, 2);
        assert_eq!(second.seq_updated, 2);
        assert_eq!(
            get(&mut conn, here.document, full.discussion.id)
                .await
                .unwrap()
                .unwrap()
                .discussion
                .seq_updated,
            2,
            "the discussion is stamped by its newest comment"
        );
        assert_eq!(
            resolve(
                &mut conn,
                here.document,
                full.discussion.id,
                true,
                here.ada,
                now
            )
            .await
            .unwrap(),
            Some(3)
        );
        let resolved = get(&mut conn, here.document, full.discussion.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(resolved.discussion.resolved_by, Some(here.ada));
        assert_eq!(
            resolve(
                &mut conn,
                here.document,
                full.discussion.id,
                false,
                here.bob,
                now
            )
            .await
            .unwrap(),
            Some(4)
        );
        let open_again = get(&mut conn, here.document, full.discussion.id)
            .await
            .unwrap()
            .unwrap();
        assert!(open_again.discussion.resolved_at.is_none());
        let doc = super::super::get(&mut conn, here.document)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            doc.doc_seq, 4,
            "the sequence of the document moved with every one"
        );
        assert_eq!(
            doc.updated_by,
            Some(here.ada),
            "but the text, and who last wrote it, did not: nothing of the kind changed"
        );
        assert_eq!(doc.version, 0);
        assert!(doc.span().is_empty());
    }

    #[tokio::test]
    async fn a_request_sent_twice_answers_as_the_first_did_and_another_persons_identifier_is_refused()
     {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let now = clock::start();
        let here = place(&mut conn, now).await;
        let block = a_block(&mut conn, here.document).await;
        let content = said("Once");
        let id = new_id();
        let first = open(
            &mut conn,
            here.document,
            block,
            "",
            here.bob,
            &NewComment {
                id,
                content: &content,
            },
            now,
        )
        .await
        .unwrap();
        let Opened::Created(made) = first else {
            panic!()
        };
        let again = open(
            &mut conn,
            here.document,
            block,
            "",
            here.bob,
            &NewComment {
                id,
                content: &content,
            },
            now,
        )
        .await
        .unwrap();
        let Opened::Repeated(same) = again else {
            panic!("repeated")
        };
        assert_eq!(same.discussion.id, made.discussion.id);
        assert_eq!(
            super::super::get(&mut conn, here.document)
                .await
                .unwrap()
                .unwrap()
                .doc_seq,
            1,
            "nothing moved"
        );
        let stolen = open(
            &mut conn,
            here.document,
            block,
            "",
            here.ada,
            &NewComment {
                id,
                content: &content,
            },
            now,
        )
        .await
        .unwrap();
        assert_eq!(stolen, Opened::Taken);
        let reply_id = new_id();
        let a = answer(
            &mut conn,
            here.document,
            made.discussion.id,
            here.ada,
            &NewComment {
                id: reply_id,
                content: &content,
            },
            now,
        )
        .await
        .unwrap();
        assert!(matches!(a, Answered::Created { .. }));
        let b = answer(
            &mut conn,
            here.document,
            made.discussion.id,
            here.ada,
            &NewComment {
                id: reply_id,
                content: &content,
            },
            now,
        )
        .await
        .unwrap();
        assert!(matches!(b, Answered::Repeated(_)));
        let c = answer(
            &mut conn,
            here.document,
            made.discussion.id,
            here.bob,
            &NewComment {
                id: reply_id,
                content: &content,
            },
            now,
        )
        .await
        .unwrap();
        assert_eq!(c, Answered::Taken);
    }

    #[tokio::test]
    async fn nothing_is_opened_on_a_block_that_is_not_there_and_nothing_is_answered_that_is_not() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let now = clock::start();
        let here = place(&mut conn, now).await;
        let content = said("Hi");
        let gone = open(
            &mut conn,
            here.document,
            new_id(),
            "",
            here.bob,
            &NewComment {
                id: new_id(),
                content: &content,
            },
            now,
        )
        .await
        .unwrap();
        assert_eq!(gone, Opened::NoSuchBlock);
        let block = a_block(&mut conn, here.document).await;
        sqlx::query("UPDATE document_blocks SET removed_at = $2 WHERE id = $1")
            .bind(block)
            .bind(now)
            .execute(&mut *conn)
            .await
            .unwrap();
        let removed = open(
            &mut conn,
            here.document,
            block,
            "",
            here.bob,
            &NewComment {
                id: new_id(),
                content: &content,
            },
            now,
        )
        .await
        .unwrap();
        assert_eq!(
            removed,
            Opened::NoSuchBlock,
            "a block that is gone is not discussed"
        );
        let none = answer(
            &mut conn,
            here.document,
            new_id(),
            here.bob,
            &NewComment {
                id: new_id(),
                content: &content,
            },
            now,
        )
        .await
        .unwrap();
        assert_eq!(none, Answered::NoSuchDiscussion);
        assert_eq!(
            resolve(&mut conn, here.document, new_id(), true, here.bob, now)
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn a_comment_is_edited_reacted_to_and_deleted_and_stays_as_an_emptied_line() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let now = clock::start();
        let here = place(&mut conn, now).await;
        let block = a_block(&mut conn, here.document).await;
        let content = said("Draft");
        let comment_id = new_id();
        let Opened::Created(made) = open(
            &mut conn,
            here.document,
            block,
            "",
            here.bob,
            &NewComment {
                id: comment_id,
                content: &content,
            },
            now,
        )
        .await
        .unwrap() else {
            panic!()
        };
        let better = said("Better");
        let edited = edit(&mut conn, here.document, comment_id, &better, now)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(edited.content, better);
        assert!(edited.edited_at.is_some());
        let reacted = react(&mut conn, here.document, comment_id, "👍", here.ada, true)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reacted.reaction_list()[0].users, [here.ada]);
        let seq_before = super::super::get(&mut conn, here.document)
            .await
            .unwrap()
            .unwrap()
            .doc_seq;
        react(&mut conn, here.document, comment_id, "👍", here.ada, true)
            .await
            .unwrap();
        assert_eq!(
            super::super::get(&mut conn, here.document)
                .await
                .unwrap()
                .unwrap()
                .doc_seq,
            seq_before,
            "putting on what is on changes nothing"
        );
        let gone = delete(&mut conn, here.document, comment_id, now)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(gone.content, json!([]));
        assert!(gone.deleted_at.is_some());
        assert!(
            edit(&mut conn, here.document, comment_id, &better, now)
                .await
                .unwrap()
                .is_none(),
            "no editing what is deleted"
        );
        let whole = get(&mut conn, here.document, made.discussion.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(whole.comments.len(), 1, "it stays");
    }

    #[tokio::test]
    async fn the_discussions_are_listed_the_newest_answer_first_and_by_whether_they_are_resolved() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let start = clock::start();
        let here = place(&mut conn, start).await;
        let block = a_block(&mut conn, here.document).await;
        let content = said("Q");
        let mut made = Vec::new();
        for minute in 0..3 {
            let Opened::Created(full) = open(
                &mut conn,
                here.document,
                block,
                "",
                here.bob,
                &NewComment {
                    id: new_id(),
                    content: &content,
                },
                start + chrono::Duration::minutes(minute),
            )
            .await
            .unwrap() else {
                panic!()
            };
            made.push(full.discussion.id);
        }
        // The oldest discussion gets the newest answer.
        answer(
            &mut conn,
            here.document,
            made[0],
            here.ada,
            &NewComment {
                id: new_id(),
                content: &content,
            },
            start + chrono::Duration::minutes(10),
        )
        .await
        .unwrap();
        resolve(&mut conn, here.document, made[1], true, here.ada, start)
            .await
            .unwrap();
        let ids = |rows: Vec<(Full, DateTime<Utc>)>| {
            rows.into_iter()
                .map(|(full, _)| full.discussion.id)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            ids(list(&mut conn, here.document, Which::Open, None, 10)
                .await
                .unwrap()),
            [made[0], made[2]]
        );
        assert_eq!(
            ids(list(&mut conn, here.document, Which::Resolved, None, 10)
                .await
                .unwrap()),
            [made[1]]
        );
        assert_eq!(
            ids(list(&mut conn, here.document, Which::All, None, 10)
                .await
                .unwrap()),
            [made[0], made[2], made[1]]
        );
        let page = list(&mut conn, here.document, Which::All, None, 1)
            .await
            .unwrap();
        let (first, active) = &page[0];
        let next = list(
            &mut conn,
            here.document,
            Which::All,
            Some(Cursor {
                at: *active,
                id: first.discussion.id,
            }),
            10,
        )
        .await
        .unwrap();
        assert_eq!(ids(next), [made[2], made[1]]);
        assert_eq!(page[0].0.comments.len(), 2);
    }

    #[tokio::test]
    async fn those_changed_since_a_number_are_found_whole_and_the_people_to_tell_are_found_too() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let now = clock::start();
        let here = place(&mut conn, now).await;
        let block = a_block(&mut conn, here.document).await;
        let first = said("Q");
        let Opened::Created(made) = open(
            &mut conn,
            here.document,
            block,
            "",
            here.bob,
            &NewComment {
                id: new_id(),
                content: &first,
            },
            now,
        )
        .await
        .unwrap() else {
            panic!()
        };
        let named = json!([{ "type": "mention", "user_id": here.ada }]);
        answer(
            &mut conn,
            here.document,
            made.discussion.id,
            here.ada,
            &NewComment {
                id: new_id(),
                content: &named,
            },
            now,
        )
        .await
        .unwrap();
        assert_eq!(count_changed(&mut conn, here.document, 0).await.unwrap(), 1);
        let since_one = changed(&mut conn, here.document, 1, 9).await.unwrap();
        assert_eq!(
            since_one.len(),
            1,
            "the answer moved the discussion above 1"
        );
        assert_eq!(
            since_one[0].comments.len(),
            2,
            "and it comes with all it holds"
        );
        assert!(
            changed(&mut conn, here.document, 2, 9)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            participants(&mut conn, made.discussion.id).await.unwrap(),
            [here.bob, here.ada]
        );
        assert!(
            wrote_in(&mut conn, made.discussion.id, here.ada)
                .await
                .unwrap()
        );
        assert!(
            !wrote_in(&mut conn, made.discussion.id, new_id())
                .await
                .unwrap()
        );
    }
}
