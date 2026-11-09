//! Derived native-prefix witnesses belong to an immutable cut, never to a caller's authority.
use super::{Conn, DbError, Hash, Id, Json, PAGE};

/// One completed streamed comparison, with no native or conversation bodies.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Witness {
    pub cut_id: Id,
    pub source_identity: Vec<u8>,
    pub lineage: Vec<u8>,
    pub boundary: Json,
    pub prefix_digest: Vec<u8>,
    pub prefix_entries: i64,
}

pub async fn witness(conn: &mut Conn, cut: Id) -> Result<Option<Witness>, DbError> {
    Ok(sqlx::query_as("SELECT cut_id,source_identity,lineage,boundary,prefix_digest,prefix_entries FROM thread_history_native_witnesses WHERE cut_id=$1")
        .bind(cut).fetch_optional(conn).await?)
}

/// First publication wins. Another native source or comparison cannot replace it.
pub async fn keep(conn: &mut Conn, proof: &Witness) -> Result<bool, DbError> {
    sqlx::query("INSERT INTO thread_history_native_witnesses(cut_id,source_identity,lineage,boundary,prefix_digest,prefix_entries) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(cut_id) DO NOTHING")
        .bind(proof.cut_id).bind(&proof.source_identity).bind(&proof.lineage).bind(&proof.boundary).bind(&proof.prefix_digest).bind(proof.prefix_entries)
        .execute(&mut *conn).await?;
    Ok(witness(conn, proof.cut_id).await?.as_ref() == Some(proof))
}

/// Source identity and frozen lineage, in bounded metadata pages. Live item writes
/// preserve that lineage through existing frozen revisions and do not change this hash.
pub async fn lineage(conn: &mut Conn, cut: Id) -> Result<Hash, DbError> {
    let header = header(conn, cut).await?;
    let mut hash = Hash::hasher();
    hash.update(b"orch.native-proof-lineage.v1\0");
    hash.update(&(header.len() as u64).to_be_bytes());
    hash.update(header.as_bytes());
    let mut after: Option<(Id, Id)> = None;
    loop {
        let page: Vec<(Id, Id, String)> = sqlx::query_as("SELECT thread_id,owner_continuation_id,jsonb_build_array(thread_id,owner_continuation_id,first_seq,last_seq,frozen_at,seq_offset,loose_before_orch,loose_low,loose_high)::text FROM thread_history_cut_spans WHERE cut_id=$1 AND ($2::uuid IS NULL OR (thread_id,owner_continuation_id)>($2,$3)) ORDER BY thread_id,owner_continuation_id LIMIT $4")
            .bind(cut).bind(after.map(|key| key.0)).bind(after.map(|key| key.1)).bind(PAGE).fetch_all(&mut *conn).await?;
        let Some(last) = page.last() else {
            break;
        };
        after = Some((last.0, last.1));
        for (_, _, body) in page {
            hash.update(b"span\0");
            hash.update(&(body.len() as u64).to_be_bytes());
            hash.update(body.as_bytes());
        }
    }
    let mut after = (String::new(), 0_i32, Id::nil());
    loop {
        let page: Vec<(String, i32, Id, String)> = sqlx::query_as("SELECT kind,ordinal,row_id,jsonb_build_array(kind,row_id,ordinal,body)::text FROM thread_history_cut_rows WHERE cut_id=$1 AND (kind,ordinal,row_id)>($2,$3,$4) ORDER BY kind,ordinal,row_id LIMIT $5")
            .bind(cut).bind(&after.0).bind(after.1).bind(after.2).bind(PAGE).fetch_all(&mut *conn).await?;
        let Some(last) = page.last() else {
            break;
        };
        after = (last.0.clone(), last.1, last.2);
        for (_, _, _, body) in page {
            hash.update(b"row\0");
            hash.update(&(body.len() as u64).to_be_bytes());
            hash.update(body.as_bytes());
        }
    }
    Ok(Hash::finish(&hash))
}

async fn header(conn: &mut Conn, cut: Id) -> Result<String, DbError> {
    Ok(sqlx::query_scalar("SELECT jsonb_build_array(id,thread_id,continuation_id,job_id,owner_thread_id,source_segment_id,source_native_ref,execution_revision,first_turn,last_turn,captured_seq,captured_at AT TIME ZONE 'UTC',head)::text FROM thread_history_cuts WHERE id=$1")
        .bind(cut).fetch_one(conn).await?)
}

/// A cached witness checks only this immutable cut header. Once published, the
/// database fence freezes its span/metadata membership without rescanning bodies.
pub async fn identity(conn: &mut Conn, cut: Id) -> Result<Hash, DbError> {
    Ok(Hash::of(header(conn, cut).await?.as_bytes()))
}

/// The same parent lock as lineage writes, before the final comparison and publication.
pub async fn lock(conn: &mut Conn, cut: Id) -> Result<(), DbError> {
    sqlx::query("SELECT id FROM thread_history_cuts WHERE id=$1 FOR UPDATE")
        .bind(cut)
        .fetch_one(conn)
        .await?;
    Ok(())
}

/// Native matching needs only each frozen turn's identity and ordering.
pub async fn turns(conn: &mut Conn, cut: Id) -> Result<Vec<(Id, i32)>, DbError> {
    Ok(sqlx::query_as("SELECT row_id,ordinal FROM thread_history_cut_rows WHERE cut_id=$1 AND kind='turn' ORDER BY ordinal")
        .bind(cut).fetch_all(conn).await?)
}
