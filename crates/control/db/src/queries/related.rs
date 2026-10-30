//! Bounded related-item candidates from live places, stored typed references and generation-owned item vectors.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Id, Json};
use std::collections::HashSet;

/// Point lookups keep even a million duplicate titles to two identities per entrance.
pub const SOURCE_DOCUMENTS_SQL: &str = r#"
 WITH matches AS (
   (SELECT id FROM documents WHERE node_id=$1 AND __PARENT__ AND deleted_at IS NULL
    AND orch_related_name_key(translate(basename,'ABCDEFGHIJKLMNOPQRSTUVWXYZ','abcdefghijklmnopqrstuvwxyz'))=orch_related_name_key(translate($3,'ABCDEFGHIJKLMNOPQRSTUVWXYZ','abcdefghijklmnopqrstuvwxyz'))
    AND translate(basename,'ABCDEFGHIJKLMNOPQRSTUVWXYZ','abcdefghijklmnopqrstuvwxyz')=translate($3,'ABCDEFGHIJKLMNOPQRSTUVWXYZ','abcdefghijklmnopqrstuvwxyz') LIMIT 2)
   UNION ALL
   (SELECT id FROM documents WHERE node_id=$1 AND __PARENT__ AND deleted_at IS NULL
    AND orch_related_name_key(translate(title,'ABCDEFGHIJKLMNOPQRSTUVWXYZ','abcdefghijklmnopqrstuvwxyz'))=orch_related_name_key(translate($3,'ABCDEFGHIJKLMNOPQRSTUVWXYZ','abcdefghijklmnopqrstuvwxyz'))
    AND translate(title,'ABCDEFGHIJKLMNOPQRSTUVWXYZ','abcdefghijklmnopqrstuvwxyz')=translate($3,'ABCDEFGHIJKLMNOPQRSTUVWXYZ','abcdefghijklmnopqrstuvwxyz') LIMIT 2)
   UNION ALL
   (SELECT id FROM documents WHERE node_id=$1 AND __PARENT__ AND deleted_at IS NULL
    AND orch_related_name_key(translate(title,'ABCDEFGHIJKLMNOPQRSTUVWXYZ','abcdefghijklmnopqrstuvwxyz'))=orch_related_name_key(translate($4,'ABCDEFGHIJKLMNOPQRSTUVWXYZ','abcdefghijklmnopqrstuvwxyz'))
    AND translate(title,'ABCDEFGHIJKLMNOPQRSTUVWXYZ','abcdefghijklmnopqrstuvwxyz')=translate($4,'ABCDEFGHIJKLMNOPQRSTUVWXYZ','abcdefghijklmnopqrstuvwxyz') LIMIT 2)
 ) SELECT DISTINCT id FROM matches LIMIT 2
"#;

/// Selects the appropriate indexed root/folder entrance without a nullable OR condition.
#[must_use]
pub fn source_documents_sql(folder: bool) -> String {
    SOURCE_DOCUMENTS_SQL.replace(
        "__PARENT__",
        if folder {
            "parent_container_id=$2"
        } else {
            "parent_container_id IS NULL AND $2::uuid IS NULL"
        },
    )
}

/// Potential agent scope entrances are bounded before decoding; exact Reach remains the decision.
pub async fn agent_entrances(
    conn: &mut Conn,
    path: &[Id],
    at: chrono::DateTime<Utc>,
    budget: i64,
) -> Result<(Vec<(String, Id)>, bool), DbError> {
    let mut rows = sqlx::query_as(r#"
      WITH entrances AS (
        (SELECT object_kind,object_id FROM permission_entries
         WHERE holder_kind='agents' AND holder_id=ANY($1) AND removed_at IS NULL AND expires_at IS NULL
           AND permission=ANY(ARRAY['doc.view','doc.edit','doc.manage','thread.view','thread.interact','chat.view','chat.interact'])
         ORDER BY holder_id,expires_at,object_kind,object_id,permission LIMIT $3)
        UNION ALL
        (SELECT object_kind,object_id FROM permission_entries
         WHERE holder_kind='agents' AND holder_id=ANY($1) AND removed_at IS NULL AND expires_at>$2
           AND permission=ANY(ARRAY['doc.view','doc.edit','doc.manage','thread.view','thread.interact','chat.view','chat.interact'])
         ORDER BY holder_id,expires_at,object_kind,object_id,permission LIMIT $3)
      ) SELECT object_kind,object_id FROM entrances LIMIT $3
    "#).bind(path).bind(at).bind(budget.saturating_add(1)).fetch_all(conn).await?;
    let complete = i64::try_from(rows.len()).unwrap_or(i64::MAX) <= budget;
    rows.truncate(usize::try_from(budget).unwrap_or(0));
    Ok((rows, complete))
}

/// A current item with the hierarchy identities needed to compare its place.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Item {
    /// `document`, `chat` or `conversation`.
    pub kind: String,
    /// Its stable identity.
    pub source_id: Id,
    /// The node beside which it lives.
    pub node_id: Id,
    /// Its root org.
    pub org_id: Id,
    /// A document-only folder, where present.
    pub parent_container_id: Option<Id>,
    /// Its current name, not yet authorized for delivery.
    pub name: String,
    /// Its latest native change.
    pub changed_at: DateTime<Utc>,
    /// Its current native content sequence.
    pub marker: i64,
    /// Its current place path.
    pub path: Vec<Id>,
    /// Its nearest topic, if any.
    pub topic_id: Option<Id>,
    /// Its nearest project, if any.
    pub project_id: Option<Id>,
    /// Project ancestors, from the outer project down to the containing folder.
    pub projects: Vec<Id>,
    /// Its nearest project when that project is a folder inside another project.
    pub folder_id: Option<Id>,
}

const ITEM_COLUMNS: &str = "entry.*,n.path,
 (SELECT a.id FROM nodes a WHERE a.id=ANY(n.path) AND a.kind='topic'
    ORDER BY array_position(n.path,a.id) DESC LIMIT 1) topic_id,
 (SELECT a.id FROM nodes a WHERE a.id=ANY(n.path) AND a.kind='project'
    ORDER BY array_position(n.path,a.id) DESC LIMIT 1) project_id,
 ARRAY(SELECT a.id FROM nodes a WHERE a.id=ANY(n.path) AND a.kind='project'
    ORDER BY array_position(n.path,a.id)) projects,
 (SELECT a.id FROM nodes a JOIN nodes p ON p.id=a.parent_id AND p.kind='project'
    WHERE a.id=ANY(n.path) AND a.kind='project'
    ORDER BY array_position(n.path,a.id) DESC LIMIT 1) folder_id";

fn access(readable: &str) -> String {
    access_for(readable, "entry.kind", "entry.source_id")
}

fn access_for(readable: &str, kind: &str, source: &str) -> String {
    let ordinary = super::search::CANDIDATE_ACCESS
        .replace("__ACCESS__", readable)
        .replace("__PATH__", "n.path")
        .replace("__OWNER__", "n.owner_id")
        .replace("__KIND__", kind)
        .replace("__SOURCE__", source);
    // These are indexed potential grants, not decisions: the ordinary Reach frame still checks every hit.
    let holder = format!(
        "e.holder_kind='agents' AND e.holder_id=ANY(ARRAY(SELECT jsonb_array_elements_text(COALESCE({readable}->'agent_path','[]'::jsonb))::uuid)) AND e.removed_at IS NULL AND (e.expires_at IS NULL OR e.expires_at>({readable}->>'at')::timestamptz)"
    );
    let area = format!(
        "e.permission=ANY(CASE {kind} WHEN 'conversation' THEN ARRAY['thread.view','thread.interact'] WHEN 'chat' THEN ARRAY['chat.view','chat.interact'] ELSE ARRAY['doc.view','doc.edit','doc.manage'] END)"
    );
    format!("(({ordinary}) OR (COALESCE(jsonb_array_length({readable}->'agent_path'),0)>0 AND (
      EXISTS(SELECT 1 FROM permission_entries e WHERE {holder} AND {area}
        AND e.object_kind=CASE {kind} WHEN 'conversation' THEN 'node' ELSE {kind} END AND e.object_id={source})
      OR EXISTS(SELECT 1 FROM permission_entries e WHERE {holder} AND {area}
        AND e.object_kind='node' AND e.object_id=ANY(n.path))
      OR ({kind}='document' AND EXISTS(
        WITH RECURSIVE parents AS (
          SELECT c.id,c.parent_container_id FROM document_containers c
          WHERE c.id=(SELECT d.parent_container_id FROM documents d WHERE d.id={source})
          UNION ALL SELECT c.id,c.parent_container_id FROM parents p JOIN document_containers c ON c.id=p.parent_container_id
        ) SELECT 1 FROM parents p JOIN permission_entries e ON e.object_kind='document_container' AND e.object_id=p.id
          WHERE {holder} AND {area}
      )))))")
}

/// Reads only selected current identities; the caller still decides which may be delivered.
pub async fn items(conn: &mut Conn, ids: &[Id]) -> Result<Vec<Item>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {ITEM_COLUMNS} FROM related_items entry JOIN nodes n ON n.id=entry.node_id
         WHERE entry.source_id=ANY($1)"
    )))
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

/// Ordered metadata stays outside hash joins so a recent window can stop before reading the installation.
pub const PLACE_CANDIDATES_SQL: &str = r#"
 WITH places AS MATERIALIZED (
   (SELECT id FROM nodes WHERE id=$1 AND deleted_at IS NULL)
   UNION ALL
   (SELECT id FROM nodes WHERE path @> ARRAY[$1::uuid] AND id<>$1 AND deleted_at IS NULL
    ORDER BY GREATEST(updated_at,activity_at) DESC,id LIMIT $2)
 ), raw AS MATERIALIZED (
   SELECT entry.* FROM places p CROSS JOIN LATERAL (
     SELECT * FROM related_items entry WHERE entry.kind='document' AND entry.node_id=p.id
       AND __FOLDER__ AND entry.source_id<>$5
     ORDER BY changed_at DESC,source_id DESC LIMIT CASE WHEN p.id=$1 THEN $2 ELSE 1 END
   ) entry
   UNION ALL
   SELECT entry.* FROM places p CROSS JOIN LATERAL (
     SELECT * FROM related_items entry WHERE entry.kind='conversation' AND entry.source_id=p.id LIMIT 1
   ) entry WHERE $4::uuid IS NULL
   UNION ALL
   SELECT entry.* FROM places p CROSS JOIN LATERAL (
     SELECT * FROM related_items entry WHERE entry.kind='chat' AND entry.source_id IN
       (SELECT id FROM chats WHERE node_id=p.id) LIMIT 1
   ) entry WHERE $4::uuid IS NULL
   UNION ALL
   (SELECT * FROM related_items entry WHERE entry.kind='document' AND entry.source_id=ANY(__DOCUMENTS__)
      AND __FOLDER__ AND entry.source_id<>$5 ORDER BY changed_at DESC,source_id DESC LIMIT $2)
   UNION ALL
   (SELECT * FROM related_items entry WHERE entry.kind='chat' AND entry.source_id=ANY(__CHATS__)
      AND $4::uuid IS NULL ORDER BY changed_at DESC,source_id DESC LIMIT $2)
 ) SELECT source_id FROM (
   SELECT DISTINCT entry.source_id,entry.changed_at,(entry.node_id=$1) closer
   FROM raw entry CROSS JOIN LATERAL (
     SELECT n.path,n.owner_id FROM nodes n WHERE n.id=entry.node_id AND n.path @> ARRAY[$1::uuid] OFFSET 0
   ) n WHERE entry.source_id<>$5 AND __ACCESS__
 ) ranked ORDER BY closer DESC,changed_at DESC,source_id DESC LIMIT $2
"#;

/// A bounded native-node neighborhood, with direct source and exceptional document/chat entrances.
pub async fn by_place(
    conn: &mut Conn,
    source: &Item,
    readable: &Json,
    count: i64,
) -> Result<Vec<Id>, DbError> {
    let scope = if readable.get("agent_grants_complete") != Some(&Json::Bool(false)) {
        super::search::sparse::Scope::load(
            conn,
            readable,
            &[source.org_id],
            count.saturating_mul(count),
            None,
        )
        .await?
    } else {
        None
    };
    let mut found = Vec::new();
    let mut seen = HashSet::from([source.source_id]);
    let mut places = vec![(source.node_id, source.parent_container_id)];
    places.extend(
        source
            .topic_id
            .into_iter()
            .chain(source.projects.iter().rev().copied())
            .chain([source.org_id])
            .map(|id| (id, None)),
    );
    places.dedup();
    for (place, folder) in places {
        let mut sql = PLACE_CANDIDATES_SQL
            .replace(
                "__FOLDER__",
                if folder.is_some() {
                    "parent_container_id=$4"
                } else {
                    "$4::uuid IS NULL"
                },
            )
            .replace(
                "__DOCUMENTS__",
                if scope.is_some() {
                    "$7::uuid[]"
                } else {
                    "ARRAY(SELECT jsonb_array_elements_text($3->'documents')::uuid)"
                },
            )
            .replace(
                "__CHATS__",
                if scope.is_some() {
                    "$9::uuid[]"
                } else {
                    "ARRAY(SELECT jsonb_array_elements_text($3->'chats')::uuid)"
                },
            )
            .replace("__ACCESS__", &access("$3"));
        if scope.is_some() {
            sql = sql.replacen(
                "WITH places",
                &format!(
                    "WITH {}, places",
                    super::search::sparse::Scope::declarations(5)
                ),
                1,
            );
        }
        let mut query = sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
            .persistent(false)
            .bind(place)
            .bind(count)
            .bind(readable)
            .bind(folder)
            .bind(source.source_id);
        if let Some(scope) = &scope {
            for ids in scope.parameters() {
                query = query.bind(ids);
            }
        }
        let ids: Vec<Id> = query.fetch_all(&mut *conn).await?;
        for id in ids {
            if seen.insert(id) {
                found.push(id);
            }
            if i64::try_from(found.len()).unwrap_or(i64::MAX) >= count {
                return Ok(found);
            }
        }
    }
    Ok(found)
}

/// The indexed distinct-target neighborhood, shared with the native query-plan proof.
pub const REFERENCE_CANDIDATES_SQL: &str = r#"
  WITH targets AS MATERIALIZED (
    SELECT target_kind,target_id,target_path_key FROM related_reference_targets
    WHERE source_kind=$1 AND source_id=$2 ORDER BY target_kind,target_id,target_path_key LIMIT $3
  ), direct AS (
    SELECT target_id id FROM targets WHERE target_kind IN ('document','node','chat')
    UNION ALL
    (SELECT source_id FROM related_reference_targets WHERE target_kind='document' AND target_id=$2 AND target_path_key=''::bytea
      ORDER BY source_id,source_kind LIMIT $3)
    UNION ALL
    SELECT near.source_id FROM targets t CROSS JOIN LATERAL (
      SELECT r.source_id FROM related_reference_targets r
      WHERE r.target_kind=t.target_kind AND r.target_id=t.target_id AND r.target_path_key=t.target_path_key AND r.source_id<>$2
      ORDER BY r.source_id,r.source_kind LIMIT $3
    ) near
  ) SELECT DISTINCT id FROM direct WHERE id<>$2 ORDER BY id LIMIT $3
"#;

/// A bounded direct, reverse and shared-target reference neighborhood.
pub async fn by_references(conn: &mut Conn, source: &Item, count: i64) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(REFERENCE_CANDIDATES_SQL)
        .bind(&source.kind)
        .bind(source.source_id)
        .bind(count)
        .fetch_all(conn)
        .await?)
}

/// An item's reference relationship to the source, without any referenced target's name.
#[derive(Debug, sqlx::FromRow)]
pub struct ReferenceScore {
    /// The related candidate.
    pub source_id: Id,
    /// The source directly references this candidate.
    pub outgoing: bool,
    /// This candidate directly references the source.
    pub incoming: bool,
    /// Jaccard overlap of their distinct outgoing reference targets.
    pub overlap: f64,
}

/// Bounded distinct-target overlap; repeated historical links never enlarge the window or its score.
pub const REFERENCE_SCORES_SQL: &str = r#"
 WITH targets AS MATERIALIZED (
   SELECT target_kind,target_id,target_path_key FROM related_reference_targets
   WHERE source_kind=$1 AND source_id=$2 ORDER BY target_kind,target_id,target_path_key LIMIT $4
 ) SELECT entry.source_id,
   EXISTS(SELECT 1 FROM related_reference_targets WHERE source_kind=$1 AND source_id=$2
     AND target_id=entry.source_id AND target_path_key=''::bytea AND target_kind=
       CASE entry.kind WHEN 'conversation' THEN 'node' ELSE entry.kind END OFFSET 0) outgoing,
   EXISTS(SELECT 1 FROM related_reference_targets WHERE source_kind=entry.kind AND source_id=entry.source_id
     AND target_kind='document' AND target_id=$2 AND target_path_key=''::bytea OFFSET 0) incoming,
   overlap.score AS overlap
 FROM related_items entry CROSS JOIN LATERAL (
   WITH theirs AS MATERIALIZED (
     SELECT target_kind,target_id,target_path_key FROM related_reference_targets
     WHERE source_kind=entry.kind AND source_id=entry.source_id
     ORDER BY target_kind,target_id,target_path_key LIMIT $4
   ) SELECT COALESCE(
     (SELECT count(*)::float8 FROM targets JOIN theirs USING(target_kind,target_id,target_path_key)) /
     NULLIF((SELECT count(*)::float8 FROM (SELECT * FROM targets UNION SELECT * FROM theirs) combined),0),0)::float8 score
 ) overlap WHERE entry.source_id=ANY($3)
"#;

/// Scores only selected candidate identities with one configured target window per item.
pub async fn reference_scores(
    conn: &mut Conn,
    source: &Item,
    ids: &[Id],
    count: i64,
) -> Result<Vec<ReferenceScore>, DbError> {
    Ok(sqlx::query_as(REFERENCE_SCORES_SQL)
        .bind(&source.kind)
        .bind(source.source_id)
        .bind(ids)
        .bind(count)
        .fetch_all(conn)
        .await?)
}

/// Nearest whole items in one generation; caller-supplied scopes narrow the graph before the candidate cutoff.
pub async fn by_meaning(
    conn: &mut Conn,
    source: &Item,
    generation: Id,
    dimensions: i32,
    readable: &Json,
    count: i64,
    exact_limit: i64,
) -> Result<Vec<Id>, DbError> {
    sqlx::query("SELECT set_config('hnsw.ef_search',$1,true)")
        .bind(count.clamp(1, super::semantic::EF_SEARCH_MAX).to_string())
        .execute(&mut *conn)
        .await?;
    // Identifiers and positive vector lengths come only from the generation row.
    let distance = format!(
        "subvector(v.embedding,1,{dimensions})::halfvec({dimensions}) <=>
         (SELECT subvector(embedding,1,{dimensions})::halfvec({dimensions}) FROM search_item_vectors
          WHERE generation_id='{generation}' AND kind=$1 AND source_id=$2 AND marker=$3)"
    );
    let joins = "CROSS JOIN LATERAL (
         SELECT entry.* FROM related_items entry WHERE entry.kind=v.kind AND entry.source_id=v.source_id AND entry.marker=v.marker OFFSET 0
       ) entry CROSS JOIN LATERAL (
         SELECT 1 FROM search_sources s WHERE s.generation_id=v.generation_id AND s.kind=v.kind
           AND s.source_id=v.source_id AND s.marker=v.marker AND s.title=entry.name OFFSET 0
       ) current CROSS JOIN LATERAL (
         SELECT n.path,n.owner_id FROM nodes n WHERE n.id=entry.node_id OFFSET 0
       ) n";
    let org = source.org_id.to_string();
    let full_root = ["doc_scopes", "thread_scopes", "chat_scopes"]
        .iter()
        .all(|kind| {
            readable
                .get(*kind)
                .or_else(|| readable.get("scopes"))
                .and_then(Json::as_array)
                .is_some_and(|scopes| scopes.iter().any(|id| id.as_str() == Some(org.as_str())))
        });
    let graph = format!(
        "SELECT v.source_id,({distance})::float8 distance FROM (
           SELECT * FROM search_item_vectors v WHERE v.generation_id='{generation}' AND v.source_id<>$2
           ORDER BY {distance} OFFSET 0
         ) v {joins} WHERE {}",
        access("$5")
    );
    let scope = if exact_limit > 0
        && !full_root
        && readable.get("agent_grants_complete") != Some(&Json::Bool(false))
    {
        super::search::sparse::Scope::load(
            conn,
            readable,
            &[source.org_id],
            exact_limit,
            Some(generation),
        )
        .await?
        .filter(|scope| scope.small)
    } else {
        None
    };
    let query = if let Some(scope) = &scope {
        // Reuse search's indexed scope/source entrances and its configurable bound. A complete small
        // inventory is cheaper than walking the global graph for a one-percent reader; overlap chooses
        // the graph conservatively. Deduplicate whole items only after bounding the chunk inventory.
        let predicate = access_for("$5", "part.kind", "part.source_id");
        let inventory = format!(
            "SELECT part.kind,part.source_id FROM ({}) part
             JOIN nodes n ON n.id=part.node_id WHERE {predicate} LIMIT {}",
            scope.chunks(generation, 5),
            exact_limit.saturating_add(1)
        );
        format!("WITH {}, candidate_rows AS MATERIALIZED ({inventory}),
          use_graph AS MATERIALIZED (SELECT count(*)>{exact_limit} yes FROM candidate_rows),
          eligible AS (SELECT DISTINCT kind,source_id FROM candidate_rows)
          SELECT source_id FROM (
            (SELECT v.source_id,({distance})::float8 distance FROM eligible p
             CROSS JOIN LATERAL (SELECT v.* FROM search_item_vectors v
               WHERE v.generation_id='{generation}' AND v.kind=p.kind AND v.source_id=p.source_id OFFSET 0) v
             {joins} WHERE NOT (SELECT yes FROM use_graph) AND v.source_id<>$2 AND {}
             ORDER BY ({distance})+0 LIMIT $4)
            UNION ALL ({graph} AND (SELECT yes FROM use_graph) ORDER BY {distance} LIMIT $4)
          ) near ORDER BY distance,source_id LIMIT $4",super::search::sparse::Scope::declarations(5),access("$5"))
    } else {
        format!("{graph} ORDER BY {distance} LIMIT $4")
    };
    let mut statement = sqlx::query_scalar(sqlx::AssertSqlSafe(query))
        .persistent(false)
        .bind(&source.kind)
        .bind(source.source_id)
        .bind(source.marker)
        .bind(count)
        .bind(readable);
    if let Some(scope) = &scope {
        for ids in scope.parameters() {
            statement = statement.bind(ids);
        }
    }
    Ok(statement.fetch_all(conn).await?)
}

/// Cosine similarity of selected current items to a current document, within one generation alone.
pub async fn meaning_scores(
    conn: &mut Conn,
    source: &Item,
    generation: Id,
    ids: &[Id],
) -> Result<Vec<(Id, f64)>, DbError> {
    Ok(sqlx::query_as(
        "WITH selected AS MATERIALIZED (
           SELECT v.* FROM search_item_vectors v WHERE v.generation_id=$1 AND v.source_id=ANY($5)
         ) SELECT v.source_id,(1-(v.embedding <=> anchor.embedding))::float8
         FROM selected v CROSS JOIN LATERAL (
           SELECT entry.name FROM related_items entry WHERE entry.kind=v.kind AND entry.source_id=v.source_id
             AND entry.marker=v.marker OFFSET 0
         ) live CROSS JOIN LATERAL (
           SELECT 1 FROM search_sources s WHERE s.generation_id=v.generation_id AND s.kind=v.kind
             AND s.source_id=v.source_id AND s.marker=v.marker AND s.title=live.name OFFSET 0
         ) current CROSS JOIN search_item_vectors anchor
         WHERE anchor.generation_id=$1 AND anchor.kind=$2 AND anchor.source_id=$3 AND anchor.marker=$4",
    )
    .bind(generation)
    .bind(&source.kind)
    .bind(source.source_id)
    .bind(source.marker)
    .bind(ids)
    .fetch_all(conn)
    .await?)
}
