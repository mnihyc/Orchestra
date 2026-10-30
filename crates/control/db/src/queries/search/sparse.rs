//! Conservative indexed candidate identities for a filtered nearest search, bounded before choosing an exact scan.

use crate::{Conn, DbError};
use orch_types::{Id, Json};
use std::collections::HashSet;

#[derive(sqlx::FromRow)]
struct Place {
    id: Id,
    path: Vec<Id>,
    owner_id: Option<Id>,
}

/// Native entrances for current searchable sources, never a final permission decision.
#[derive(Debug, Default)]
pub struct Scope {
    /// Whether source figures favor a scoped scan rather than a global index.
    pub small: bool,
    /// Places whose ordinary documents are candidates.
    pub document_nodes: Vec<Id>,
    /// Exceptional document sources outside those places.
    pub documents: Vec<Id>,
    /// Conversation sources covered by the candidate prefilter.
    pub threads: Vec<Id>,
    /// Chat sources covered by the candidate prefilter.
    pub chats: Vec<Id>,
    /// Places whose own fields are candidates.
    pub names: Vec<Id>,
}

fn ids(value: &Json, key: &str, fallback: Option<&str>) -> Option<Vec<Id>> {
    let Some(value) = value
        .get(key)
        .or_else(|| fallback.and_then(|key| value.get(key)))
    else {
        return Some(Vec::new());
    };
    value
        .as_array()?
        .iter()
        .map(|value| value.as_str()?.parse().ok())
        .collect()
}

impl Scope {
    /// Use only a selected source and its kept chunk figure, without expanding the reader's other places.
    pub async fn focused(
        conn: &mut Conn,
        document: Option<Id>,
        chat: Option<Id>,
        budget: i64,
        generation: Option<Id>,
    ) -> Result<Option<Self>, DbError> {
        if budget <= 0 || (document.is_none() && chat.is_none()) {
            return Ok(None);
        }
        let mut scope = Self {
            documents: document.into_iter().collect(),
            chats: chat.into_iter().collect(),
            ..Self::default()
        };
        scope.small = if let Some(generation) = generation {
            let chunks: i64 = sqlx::query_scalar(
                "SELECT COALESCE(sum(chunks),0)::bigint FROM search_sources WHERE generation_id=$1
                  AND ((kind='document' AND source_id=$2) OR (kind='chat' AND source_id=$3))",
            )
            .persistent(false)
            .bind(generation)
            .bind(document)
            .bind(chat)
            .fetch_one(conn)
            .await?;
            chunks <= budget
        } else {
            // One lexical source always benefits from its own leading document/chat index,
            // even when it holds many parts: native windows still apply their configured limits.
            true
        };
        Ok(Some(scope))
    }

    /// Read bounded place/source metadata and use PostgreSQL's existing statistics to choose a scoped scan.
    pub async fn load(
        conn: &mut Conn,
        readable: &Json,
        within: &[Id],
        budget: i64,
        generation: Option<Id>,
    ) -> Result<Option<Self>, DbError> {
        if budget <= 0
            || !readable.is_object()
            || readable.get("unrestricted") == Some(&Json::Bool(true))
        {
            return Ok(None);
        }
        let parsed = (|| {
            Some((
                ids(readable, "scopes", None)?,
                ids(readable, "doc_scopes", Some("scopes"))?,
                ids(readable, "thread_scopes", Some("scopes"))?,
                ids(readable, "chat_scopes", Some("scopes"))?,
                ids(readable, "names", None)?,
                ids(readable, "chat_nodes", None)?,
                ids(readable, "documents", None)?,
                ids(readable, "chats", None)?,
                ids(readable, "groups", None)?,
            ))
        })();
        let Some((
            general,
            doc_paths,
            thread_paths,
            chat_paths,
            named,
            chat_nodes,
            explicit_docs,
            explicit_chats,
            groups,
        )) = parsed
        else {
            return Ok(None);
        };
        let person = match readable.get("person") {
            Some(Json::String(value)) => match value.parse::<Id>() {
                Ok(id) => Some(id),
                Err(_) => return Ok(None),
            },
            None => None,
            _ => return Ok(None),
        };
        // A broad org needs no descendant inventory. Root predicates have native statistics,
        // unlike path-join estimates, so keep selective topic scopes on the bound-identity path.
        let roots: Vec<Id> = sqlx::query_scalar(
            "SELECT id FROM nodes WHERE id=ANY($1) AND kind='org' AND parent_id IS NULL
              AND (cardinality($2::uuid[])=0 OR $2::uuid[] <@ ARRAY[id])",
        )
        .persistent(false)
        .bind(&doc_paths)
        .bind(within)
        .fetch_all(&mut *conn)
        .await?;
        if !roots.is_empty() {
            let native = if let Some(generation) = generation {
                format!(
                    "SELECT id FROM search_chunks WHERE generation_id='{generation}' AND org_id=ANY($1)"
                )
            } else {
                "SELECT id FROM documents WHERE org_id=ANY($1)
                  UNION ALL SELECT i.id FROM items i JOIN nodes n ON n.id=i.thread_id WHERE n.org_id=ANY($1)
                  UNION ALL SELECT m.id FROM chat_messages m JOIN chats c ON c.id=m.chat_id WHERE c.org_id=ANY($1)".to_owned()
            };
            let plan: Json = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
                "EXPLAIN (FORMAT JSON) {native}"
            )))
            .persistent(false)
            .bind(&roots)
            .fetch_one(&mut *conn)
            .await?;
            if plan
                .get(0)
                .and_then(|root| root.get("Plan"))
                .and_then(|plan| plan.get("Plan Rows"))
                .and_then(Json::as_i64)
                .is_some_and(|rows| rows > budget)
            {
                return Ok(None);
            }
        }
        let paths: Vec<_> = general
            .iter()
            .chain(&doc_paths)
            .chain(&thread_paths)
            .chain(&chat_paths)
            .copied()
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        let exact_nodes: Vec<_> = named.iter().chain(&chat_nodes).copied().collect();
        let places: Vec<Place> = sqlx::query_as(
            "SELECT id,path,owner_id FROM nodes
          WHERE path @> $5::uuid[] AND (id=ANY($1) OR path && $2::uuid[] OR owner_id=$3)
          LIMIT $4",
        )
        .persistent(false)
        .bind(&exact_nodes)
        .bind(&paths)
        .bind(person)
        .bind(budget.saturating_add(1))
        .bind(within)
        .fetch_all(&mut *conn)
        .await?;
        if i64::try_from(places.len()).unwrap_or(i64::MAX) > budget {
            return Ok(None);
        }
        let intersects =
            |place: &Place, paths: &[Id]| place.path.iter().any(|id| paths.contains(id));
        let mut scope = Self::default();
        for place in places {
            let owned = person.is_some() && place.owner_id == person;
            if owned || intersects(&place, &doc_paths) {
                scope.document_nodes.push(place.id);
            }
            if owned || intersects(&place, &thread_paths) {
                scope.threads.push(place.id);
            }
            if owned
                || named.contains(&place.id)
                || (readable.get("names").is_none() && intersects(&place, &general))
            {
                scope.names.push(place.id);
            }
        }
        scope.chats = sqlx::query_scalar("SELECT c.id FROM chats c
          JOIN nodes n ON n.id=COALESCE(c.node_id,c.org_id)
          WHERE n.path @> $6::uuid[] AND (c.id=ANY($1) OR n.owner_id=$2
            OR (c.node_id IS NOT NULL AND (c.node_id=ANY($3) OR n.path && $4::uuid[]))
            OR EXISTS(SELECT 1 FROM chat_members m WHERE m.chat_id=c.id AND m.user_id=$2 AND m.left_at IS NULL)
            OR EXISTS(SELECT 1 FROM permission_entries e WHERE e.object_kind='chat' AND e.object_id=c.id AND e.removed_at IS NULL))
          LIMIT $5")
            .persistent(false).bind(&explicit_chats).bind(person).bind(&chat_nodes).bind(&chat_paths)
            .bind(budget.saturating_add(1)).bind(within).fetch_all(&mut *conn).await?;
        if i64::try_from(scope.chats.len()).unwrap_or(i64::MAX) > budget {
            return Ok(None);
        }
        scope.small = scope
            .estimate(conn, generation)
            .await?
            .is_some_and(|rows| rows <= budget);
        let uncovered: Vec<Id> = sqlx::query_scalar(
            "SELECT id FROM nodes WHERE path @> $1::uuid[] AND NOT(id=ANY($2)) LIMIT $3",
        )
        .persistent(false)
        .bind(within)
        .bind(&scope.document_nodes)
        .bind(budget.saturating_add(1))
        .fetch_all(&mut *conn)
        .await?;
        if i64::try_from(uncovered.len()).unwrap_or(i64::MAX) > budget {
            return Ok(None);
        }
        if uncovered.is_empty() {
            return Ok(Some(scope));
        }
        // Keep ownership, container pages and explicit pages as separate indexed entrances.
        // The ordinary live document indexes also serve the legacy nearest inventory's entrances.
        scope.documents = sqlx::query_scalar("SELECT id FROM (
          SELECT d.id FROM documents d
            WHERE d.owner_id=$3 AND d.deleted_at IS NULL AND d.node_id=ANY($1)
          UNION ALL SELECT d.id FROM documents d
            WHERE d.parent_container_id IS NOT NULL AND d.deleted_at IS NULL AND d.node_id=ANY($1)
          UNION ALL SELECT unnest($2::uuid[])
          UNION ALL SELECT e.object_id FROM permission_entries e WHERE e.object_kind IN ('document','document_container')
            AND e.removed_at IS NULL AND (e.permission<>'access.own' OR (e.holder_kind='user' AND e.holder_id=$3)
              OR (e.holder_kind='group' AND e.holder_id=ANY($4)))
          ) sources LIMIT $5")
            .persistent(false).bind(&uncovered).bind(&explicit_docs).bind(person).bind(&groups)
            .bind(budget.saturating_add(1)).fetch_all(&mut *conn).await?;
        if i64::try_from(scope.documents.len()).unwrap_or(i64::MAX) > budget {
            return Ok(None);
        }
        scope.documents.sort_unstable();
        scope.documents.dedup();
        if !scope.documents.is_empty() {
            scope.small = scope
                .estimate(conn, generation)
                .await?
                .is_some_and(|rows| rows <= budget);
        }
        Ok(Some(scope))
    }

    /// The arrays bound after the caller's ordinary parameters, in a fixed order.
    #[must_use]
    pub fn parameters(&self) -> [&[Id]; 5] {
        [
            &self.document_nodes,
            &self.documents,
            &self.threads,
            &self.chats,
            &self.names,
        ]
    }

    /// Declare every typed parameter even for a field that uses only one array.
    #[must_use]
    pub fn declarations(offset: usize) -> String {
        format!(
            "scope_parameters AS (SELECT ${}::uuid[] document_nodes,${}::uuid[] documents,
          ${}::uuid[] threads,${}::uuid[] chats,${}::uuid[] names)",
            offset + 1,
            offset + 2,
            offset + 3,
            offset + 4,
            offset + 5
        )
    }

    /// The same candidate prefilter expressed through already resolved native identities.
    #[must_use]
    pub fn predicate(&self, kind: &str, node: &str, source: &str, offset: usize) -> String {
        match kind {
            "document" => {
                let nodes = format!("{node}=ANY(${}::uuid[])", offset + 1);
                if self.documents.is_empty() {
                    nodes
                } else {
                    format!("({nodes} OR {source}=ANY(${}::uuid[]))", offset + 2)
                }
            }
            "item" | "conversation" => format!("{source}=ANY(${}::uuid[])", offset + 3),
            "chat" | "chat_message" => format!("{source}=ANY(${}::uuid[])", offset + 4),
            "name" => format!("{source}=ANY(${}::uuid[])", offset + 5),
            _ => "false".to_owned(),
        }
    }

    /// Native entrances for the complete scoped chunk set, before distance is computed.
    #[must_use]
    pub fn chunks(&self, generation: Id, offset: usize) -> String {
        // Each source branch is indexed before its union; exceptional sources already covered by
        // a place are excluded so a document's ordinary pieces are not fetched twice.
        format!("SELECT part.* FROM nodes n CROSS JOIN LATERAL (
            SELECT * FROM search_chunks part WHERE part.node_id=n.id AND part.generation_id='{generation}'
              AND part.kind='document' OFFSET 0) part WHERE n.id=ANY(${}::uuid[])
          UNION ALL SELECT part.* FROM unnest(${}::uuid[]) source(id) CROSS JOIN LATERAL (
            SELECT * FROM search_chunks part WHERE part.source_id=source.id AND part.generation_id='{generation}'
              AND part.kind='document' OFFSET 0) part WHERE NOT(part.node_id=ANY(${}::uuid[]))
          UNION ALL SELECT part.* FROM unnest(${}::uuid[]) source(id) CROSS JOIN LATERAL (
            SELECT * FROM search_chunks part WHERE part.source_id=source.id AND part.generation_id='{generation}'
              AND part.kind='conversation' OFFSET 0) part
          UNION ALL SELECT part.* FROM unnest(${}::uuid[]) source(id) CROSS JOIN LATERAL (
            SELECT * FROM search_chunks part WHERE part.source_id=source.id AND part.generation_id='{generation}'
              AND part.kind='chat' OFFSET 0) part",offset+1,offset+2,offset+1,offset+3,offset+4)
    }

    async fn estimate(
        &self,
        conn: &mut Conn,
        generation: Option<Id>,
    ) -> Result<Option<i64>, DbError> {
        // EXPLAIN without ANALYZE consults native planner statistics; it executes none of these reads.
        // The estimate only chooses an entrance. It never bounds results or substitutes for access checks.
        let query = if let Some(generation) = generation {
            format!("SELECT id FROM search_chunks WHERE generation_id='{generation}' AND kind='document'
                AND (node_id=ANY($1) OR source_id=ANY($2))
              UNION ALL SELECT id FROM search_chunks WHERE generation_id='{generation}' AND kind='conversation' AND source_id=ANY($3)
              UNION ALL SELECT id FROM search_chunks WHERE generation_id='{generation}' AND kind='chat' AND source_id=ANY($4)")
        } else {
            "SELECT id FROM documents WHERE node_id=ANY($1) OR id=ANY($2)
              UNION ALL SELECT b.id FROM document_blocks b JOIN documents d ON d.id=b.document_id
                WHERE d.node_id=ANY($1) OR d.id=ANY($2)
              UNION ALL SELECT id FROM items WHERE thread_id=ANY($3)
              UNION ALL SELECT id FROM chat_messages WHERE chat_id=ANY($4)"
                .to_owned()
        };
        let sql = format!(
            "EXPLAIN (FORMAT JSON) WITH {} {query}",
            Self::declarations(0)
        );
        let mut statement =
            sqlx::query_scalar::<_, Json>(sqlx::AssertSqlSafe(sql)).persistent(false);
        for ids in self.parameters() {
            statement = statement.bind(ids);
        }
        let plan = statement.fetch_one(conn).await?;
        Ok(plan
            .get(0)
            .and_then(|root| root.get("Plan"))
            .and_then(|plan| plan.get("Plan Rows"))
            .and_then(Json::as_i64))
    }
}
