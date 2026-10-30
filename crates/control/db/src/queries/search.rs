//! Search candidates, readable content scopes, stable pages and repair of stored text projections.
//!
//! Candidate names are ranked before access is checked in batches. Content is
//! searched only in the identifiers its caller has already resolved as readable.

mod cancellation;
pub mod candidates;
pub mod items;
pub mod sparse;
pub use cancellation::{CancelRead, cancel_on_drop};

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::Id;

/// A name awaiting the caller's bulk access check.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct NameRow {
    /// A current kind-specific state, where recorded.
    pub state: Option<String>,
    /// Existing activity/update clock; the same value used by the ranking tie-break.
    pub last_activity: Option<DateTime<Utc>>,
    /// This caller's kind/id favorite, independently of every other kind with that identifier.
    pub starred: bool,
    /// What it is.
    pub kind: String,
    /// Its identifier.
    pub id: Id,
    /// Its name or title.
    pub name: String,
    /// Its address, where it has one.
    pub address: Option<String>,
    /// The node it is beside, or itself for a node.
    pub node_id: Option<Id>,
    /// Its root org, where it has one.
    pub org_id: Option<Id>,
    /// Its first owner, for documents and resources.
    pub owner_id: Option<Id>,
    /// The caller's most recent visit to it.
    pub visited_at: Option<DateTime<Utc>>,
    /// Whether it or a place above it is archived.
    pub archived: bool,
}

/// Which names to consider, before their pages are checked.
#[derive(Debug)]
pub struct Names<'a> {
    /// A direct identity lookup, used only by guarded personal-state writes.
    pub identifiers: Option<&'a [Id]>,
    /// Limit to the caller's favorites, including with no query words.
    pub starred: bool,
    /// The request's sampled clock, for decaying personal visit counts.
    pub at: DateTime<Utc>,
    /// The caller.
    pub user: Id,
    /// Trees they belong to, for the directory.
    pub trees: &'a [Id],
    /// Root orgs reached by current membership, grants, ownership or chat participation.
    pub roots: &'a [Id],
    /// Whether system administration is carried by this request.
    pub system_admin: bool,
    /// A literal substring; empty means recents.
    pub text: &'a str,
    /// The kinds requested.
    pub kinds: &'a [String],
    /// A single tree, if requested.
    pub org: Option<Id>,
    /// How many candidates were consumed already.
    pub offset: i64,
    /// How many candidates to read at once.
    pub limit: i64,
}

/// A substring as a LIKE pattern, with SQL's wildcard characters made literal.
#[must_use]
pub fn substring(text: &str) -> String {
    format!(
        "%{}%",
        text.replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    )
}

/// Literal search grammar: words in any order, with double quotes keeping a phrase together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    terms: Vec<String>,
}

impl Query {
    /// One already-parsed literal, preserving any spaces or quotes inside an expression's index hint.
    #[must_use]
    pub fn literal(text: &str) -> Self {
        Self {
            terms: if text.is_empty() {
                Vec::new()
            } else {
                vec![text.to_owned()]
            },
        }
    }

    /// An unfinished quote keeps its remaining words together while a person types.
    pub fn parse(text: &str) -> Self {
        let mut terms = Vec::new();
        let mut term = String::new();
        let mut quoted = false;
        for character in text.chars() {
            if character == '"' {
                quoted = !quoted;
            } else if character.is_whitespace() && !quoted {
                if !term.trim().is_empty() {
                    terms.push(term.trim().to_owned());
                }
                term.clear();
            } else {
                term.push(character);
            }
        }
        if !term.trim().is_empty() {
            terms.push(term.trim().to_owned());
        }
        Self { terms }
    }

    /// Every literal term must occur; none are dropped as question words.
    #[must_use]
    pub fn terms(&self) -> &[String] {
        &self.terms
    }

    /// The unquoted words, for exact-name and whole-query prefix tiers.
    #[must_use]
    pub fn phrase(&self) -> String {
        self.terms.join(" ")
    }

    /// Bound patterns for the shared indexed predicate, with wildcards kept literal.
    #[must_use]
    pub fn like_patterns(&self) -> Vec<String> {
        let mut terms: Vec<_> = self.terms.iter().collect();
        // The longest literal usually narrows a trigram scan most; matching and counts remain order independent.
        terms.sort_by_key(|term| std::cmp::Reverse(term.chars().count()));
        terms.into_iter().map(|term| substring(term)).collect()
    }

    /// Literal matching for callers that already hold authorized text rather than a database column.
    #[must_use]
    pub fn matches(&self, text: &str) -> bool {
        let text = text.to_lowercase();
        self.terms
            .iter()
            .all(|term| text.contains(&term.to_lowercase()))
    }
}

/// Where among a thing's fields the words were found, from the least telling to the most: a title outranks a
/// description, a description a heading, and a heading the rest of the text. People's search ordered by best
/// matches and the agents' search rank with the same fields.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Field {
    /// Ordinary text: a paragraph, a message, a turn.
    Body = 0,
    /// A heading of a document, or the name of a file a document holds.
    Heading = 1,
    /// What a place, a thread or a document says it is about.
    Description = 2,
    /// A name or a title.
    Title = 3,
}

impl Field {
    /// The number the database statements rank by.
    #[must_use]
    pub const fn tier(self) -> i32 {
        self as i32
    }
}

/// How the matches of literal words are ranked among each other, the same for people and agents: by the field
/// they were found in, then by how often the words occur there, then the most recent first; the identifier
/// breaks what remains, so that the same search gives the same order.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct Rank {
    /// The field the best match was in.
    pub field: Field,
    /// How many times the words occur.
    pub count: i64,
    /// When what matched was last changed.
    pub at: DateTime<Utc>,
    /// The matching thing, for a deterministic tie.
    pub id: Id,
}

impl Query {
    /// How many times the words occur in a text, without regard to case; overlapping occurrences of one word
    /// are counted once each where they do not overlap, as a reader would count them.
    #[must_use]
    pub fn occurrences(&self, text: &str) -> i64 {
        let folded = text.to_lowercase();
        self.terms
            .iter()
            .map(|term| term.to_lowercase())
            .filter(|term| !term.is_empty())
            .map(|term| i64::try_from(folded.matches(term.as_str()).count()).unwrap_or(i64::MAX))
            .fold(0_i64, i64::saturating_add)
    }
}

/// How many times the bound words occur in a text, in SQL, without regard to case; both arguments are
/// source-owned SQL expressions, never user text. The same count as [`Query::occurrences`].
#[must_use]
pub fn occurrences_sql(text: &str, terms_parameter: &str) -> String {
    format!(
        "(SELECT COALESCE(sum((char_length(lower({text})) - char_length(replace(lower({text}), lower(term), '')))
            / char_length(term)), 0)::bigint FROM unnest({terms_parameter}::text[]) AS words(term) WHERE char_length(term) > 0)"
    )
}

/// The one indexed match fragment; both arguments are source-owned SQL expressions, never user text.
#[must_use]
pub fn matching_sql(text: &str, patterns_parameter: &str) -> String {
    include_str!("search_match.sql")
        .replace("{text}", text)
        .replace("{patterns}", patterns_parameter)
}

/// Name relevance, before personal visits and activity; arguments are source-owned SQL expressions.
#[must_use]
pub fn ranking_sql(name: &str, query_parameter: &str, patterns_parameter: &str) -> String {
    // Word boundaries are tested on short names, never on content bodies. Keeping matching separate
    // leaves the ordinary column/pattern predicate available to the existing trigram index.
    format!(
        r"CASE WHEN {query_parameter}::text='' THEN 0
      WHEN NOT ({name} ILIKE ALL({patterns_parameter}::text[])) THEN 0
      WHEN lower({name})=lower({query_parameter}) THEN 4
      WHEN starts_with(lower({name}),lower({query_parameter})) THEN 3
      WHEN NOT EXISTS (SELECT 1 FROM unnest({patterns_parameter}::text[]) part(pattern)
        WHERE NOT EXISTS (SELECT 1 FROM generate_series(1,char_length({name})) pos(at)
          WHERE (pos.at=1 OR substr({name},pos.at-1,1) !~ '[[:alnum:]]')
            AND substr({name},pos.at) ILIKE substr(part.pattern,2))) THEN 2
      ELSE 1 END"
    )
}

/// The tier [`ranking_sql`] gives a name that matches every word, without walking its characters: equal to
/// the phrase, starting with it, every word at a word's start, or only inside a word. Arguments are SQL.
#[must_use]
pub fn name_tier_sql(name: &str, query_parameter: &str, terms_parameter: &str) -> String {
    format!(
        r"CASE WHEN lower({name})=lower({query_parameter}) THEN 4
      WHEN starts_with(lower({name}),lower({query_parameter})) THEN 3
      WHEN NOT EXISTS (SELECT 1 FROM unnest({terms_parameter}::text[]) part(term)
        WHERE {name} !~* ('(^|[^[:alnum:]])' || regexp_replace(part.term,'([^[:alnum:]])','\\\1','g'))) THEN 2
      ELSE 1 END"
    )
}

/// Begins the read snapshot before any access or content is looked up.
pub async fn snapshot(conn: &mut Conn) -> Result<(), DbError> {
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(conn)
        .await?;
    Ok(())
}

/// A current chat participant, including whether they joined as a guest.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ParticipantChat {
    /// The end of a timed membership, checked against the caller's clock.
    #[sqlx(default)]
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    /// The chat.
    pub id: Id,
    /// The root org the chat belongs to.
    pub org_id: Id,
    /// Whether this person is a guest.
    pub guest: bool,
}

/// Current participation in a set of chats; inspection rights are never used.
pub async fn participant_chats(
    conn: &mut Conn,
    user: Id,
    ids: &[Id],
) -> Result<Vec<ParticipantChat>, DbError> {
    Ok(sqlx::query_as("SELECT c.id, c.org_id, cm.guest, cm.expires_at FROM chats c JOIN chat_members cm ON cm.chat_id = c.id JOIN nodes root ON root.id = c.org_id WHERE cm.user_id = $1 AND cm.left_at IS NULL AND c.id = ANY($2) AND NOT EXISTS (SELECT 1 FROM nodes a WHERE a.id = ANY(root.path) AND a.deleted_at IS NOT NULL)")
        .bind(user).bind(ids).fetch_all(conn).await?)
}

/// Candidate roots only: current bulk authorization still decides every name before it is returned.
/// `held`, where given, is every identity the person holds at `at` without their flag of system admin, as
/// `access_holders` finds them (the access engine keeps them between requests); without it they are worked out here.
pub async fn name_roots(
    conn: &mut Conn,
    user: Id,
    system_admin: bool,
    at: DateTime<Utc>,
    held: Option<&super::access::kept::Held>,
) -> Result<Vec<Id>, DbError> {
    if system_admin {
        return Ok(sqlx::query_scalar(
            "SELECT id FROM nodes WHERE kind='org' AND parent_id IS NULL AND deleted_at IS NULL",
        )
        .fetch_all(conn)
        .await?);
    }
    Ok(sqlx::query_scalar(r"WITH held AS MATERIALIZED (
        SELECT * FROM unnest($3::text[],$4::uuid[],$5::text[]) AS given(kind,id,role) WHERE $6
        UNION ALL SELECT * FROM access_holders($1,$2,false) WHERE NOT $6
      ), objects AS (
        SELECT 'node'::text kind,id,id place,org_id,owner_id FROM nodes WHERE deleted_at IS NULL
        UNION ALL SELECT 'document',id,node_id,org_id,owner_id FROM documents WHERE deleted_at IS NULL
        UNION ALL SELECT 'document_container',id,node_id,org_id,owner_id FROM document_containers WHERE deleted_at IS NULL
        UNION ALL SELECT 'chat',id,COALESCE(node_id,org_id),org_id,NULL FROM chats
        UNION ALL SELECT 'provider',id,home_id,org_id,owner_id FROM providers WHERE deleted_at IS NULL
        UNION ALL SELECT 'agent',id,home_id,org_id,owner_id FROM agents WHERE deleted_at IS NULL
        UNION ALL SELECT 'environment',id,home_node_id,org_id,owner_id FROM environments WHERE deleted_at IS NULL
        UNION ALL SELECT 'host',h.id,h.home_id,h.org_id,h.owner_id FROM hosts h JOIN machines m ON m.id=h.machine_id WHERE m.deleted_at IS NULL
        UNION ALL SELECT 'template',id,home_id,org_id,owner_id FROM env_templates WHERE deleted_at IS NULL
        UNION ALL SELECT 'mcp_server',id,home_id,org_id,owner_id FROM mcp_servers WHERE deleted_at IS NULL
        UNION ALL SELECT 'mount',id,home_id,org_id,owner_id FROM mounts WHERE deleted_at IS NULL
        UNION ALL SELECT 'skill',id,home_id,org_id,owner_id FROM skills WHERE deleted_at IS NULL
        UNION ALL SELECT 'machine',id,home_id,org_id,owner_id FROM machines WHERE deleted_at IS NULL
      ), seeds AS (
        SELECT m.org_id place FROM org_members m WHERE m.user_id=$1
        UNION SELECT n.id FROM held h JOIN nodes n ON n.id=h.id WHERE h.kind IN ('role','visible')
        UNION SELECT COALESCE(o.place,o.org_id) FROM objects o WHERE o.owner_id=$1
        UNION SELECT COALESCE(o.place,o.org_id) FROM permission_entries e JOIN objects o ON o.kind=e.object_kind AND o.id=e.object_id
          LEFT JOIN nodes n ON n.id=COALESCE(o.place,o.org_id)
          WHERE e.removed_at IS NULL AND (e.expires_at IS NULL OR e.expires_at>$2)
            AND (EXISTS (SELECT 1 FROM held h WHERE
                 (e.holder_kind=h.kind AND e.holder_id=h.id AND e.holder_kind IN ('user','group','members'))
                 OR (e.holder_kind='role' AND h.kind IN ('role','visible') AND h.id=COALESCE(e.holder_id,o.place)
                     AND array_position(ARRAY['viewer','interactor','developer','admin'],h.role)>=array_position(ARRAY['viewer','interactor','developer','admin'],e.holder_role)))
                 OR (e.holder_kind='everyone' AND EXISTS (SELECT 1 FROM held h WHERE h.kind='visible' AND h.id=ANY(n.path))))
        UNION SELECT c.org_id FROM chat_members cm JOIN chats c ON c.id=cm.chat_id WHERE cm.user_id=$1 AND cm.left_at IS NULL
      ), roots AS (
        SELECT DISTINCT n.path[1] root FROM seeds s JOIN nodes n ON n.id=s.place WHERE n.deleted_at IS NULL
      )
      SELECT DISTINCT n.id FROM nodes n JOIN roots r ON n.path[1]=r.root
        WHERE n.kind='org' AND n.deleted_at IS NULL")
        .bind(user).bind(at)
        .bind(held.map(|held| held.kinds.as_slice()).unwrap_or_default())
        .bind(held.map(|held| held.ids.as_slice()).unwrap_or_default())
        .bind(held.map(|held| held.roles.as_slice()).unwrap_or_default())
        .bind(held.is_some())
        .fetch_all(conn).await?)
}

/// Ranked name candidates inside reachable roots; the route bounds its bulk visibility checks.
pub async fn names(conn: &mut Conn, ask: &Names<'_>) -> Result<Vec<NameRow>, DbError> {
    let query = Query::parse(ask.text);
    let pattern = query.like_patterns();
    let phrase = query.phrase();
    let prefix = format!(
        "{}%",
        phrase
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    );
    let sql = r"WITH document_hits AS MATERIALIZED (
            -- A title that is only a title is matched here, by its own trigram index, under the same kind, org
            -- and root tests the batch applies below, before any place is read.
            SELECT d.id, d.title, d.node_id, d.org_id, d.owner_id, d.updated_at FROM documents d
             WHERE d.deleted_at IS NULL AND __DOCUMENT_NAME__ AND 'document' = ANY($6) AND ($7::uuid IS NULL OR d.org_id = $7)
               AND ($3 OR $12::uuid[] IS NOT NULL OR d.org_id IS NULL OR d.org_id = ANY($11::uuid[]) OR d.owner_id = $1)
        ), document_places AS MATERIALIZED (
            -- Each live place of those documents is read once, not once for every document in it.
            SELECT n.id, EXISTS (SELECT 1 FROM nodes a WHERE a.id = ANY(n.path) AND a.archived_at IS NOT NULL) archived
              FROM nodes n WHERE n.id IN (SELECT node_id FROM document_hits)
               AND NOT EXISTS (SELECT 1 FROM nodes a WHERE a.id = ANY(n.path) AND a.deleted_at IS NOT NULL)
        ), document_pool AS MATERIALIZED (
            -- The documents this batch can show: the best and newest matches in the order the batch is ranked
            -- by, and every match the reader visited (which ranks by its visits), starred or named. A word in a
            -- hundred thousand titles ranks no more of them than the batch needs.
            (SELECT h.* FROM document_hits h
              WHERE $4 <> '' AND $12::uuid[] IS NULL AND NOT $13 AND h.node_id IN (SELECT id FROM document_places)
              ORDER BY __DOCUMENT_TIER__ DESC, h.updated_at DESC, h.id LIMIT $9 + $10)
            UNION
            SELECT h.* FROM document_hits h
             WHERE h.node_id IN (SELECT id FROM document_places)
               AND (h.id = ANY($12::uuid[])
                 OR ($13 AND h.id IN (SELECT f.id FROM favorites f WHERE f.user_id = $1 AND f.kind = 'document'))
                 OR (NOT $13 AND $12::uuid[] IS NULL
                     AND h.id IN (SELECT r.id FROM recents r WHERE r.user_id = $1 AND r.kind = 'document')))
        ), resource_names AS (
            SELECT 'provider'::text kind, id, name, owner_id, home_id node_id, org_id, updated_at FROM providers WHERE deleted_at IS NULL
            UNION ALL SELECT 'agent', id, name, owner_id, home_id, org_id, updated_at FROM agents WHERE deleted_at IS NULL
            UNION ALL SELECT 'environment', id, name, owner_id, home_node_id, org_id, GREATEST(last_activity_at,updated_at) FROM environments WHERE deleted_at IS NULL
            UNION ALL SELECT 'host', h.id, h.name, h.owner_id, h.home_id, h.org_id, h.updated_at FROM hosts h JOIN machines m ON m.id = h.machine_id WHERE m.deleted_at IS NULL
            UNION ALL SELECT 'template', id, name, owner_id, home_id, org_id, updated_at FROM env_templates WHERE deleted_at IS NULL
            UNION ALL SELECT 'mcp_server', id, name, owner_id, home_id, org_id, updated_at FROM mcp_servers WHERE deleted_at IS NULL AND environment_id IS NULL
            UNION ALL SELECT 'mount', id, name, owner_id, home_id, org_id, updated_at FROM mounts WHERE deleted_at IS NULL
            UNION ALL SELECT 'skill', id, name, owner_id, home_id, org_id, updated_at FROM skills WHERE deleted_at IS NULL
        ), all_names AS (
            SELECT n.kind, n.id, n.name,
                   CASE WHEN n.kind = 'org' THEN (SELECT string_agg(o.slug, '.' ORDER BY p.ord) FROM unnest(n.path) WITH ORDINALITY p(id, ord) JOIN orgs o ON o.node_id = p.id) END address,
                   n.id node_id, n.org_id, NULL::uuid owner_id, GREATEST(n.activity_at,n.updated_at,n.created_at,th.last_activity_at) AS updated_at, NULL::timestamptz old_visited_at,
                   EXISTS (SELECT 1 FROM nodes a WHERE a.id = ANY(n.path) AND a.archived_at IS NOT NULL) archived,
                   NULL::text username
              FROM nodes n
              LEFT JOIN threads th ON th.node_id = n.id
             WHERE n.deleted_at IS NULL AND th.side_of IS NULL AND (n.kind = 'org' OR __NODE_NAME__)
               AND NOT EXISTS (SELECT 1 FROM nodes a WHERE a.id = ANY(n.path) AND a.deleted_at IS NOT NULL)
            UNION ALL
            SELECT 'document', d.id, d.title, NULL, d.node_id, d.org_id, d.owner_id, d.updated_at, NULL, p.archived, NULL
              FROM document_pool d JOIN document_places p ON p.id = d.node_id
            UNION ALL
            SELECT 'document_container', c.id, c.name, NULL, c.node_id, c.org_id, c.owner_id, c.updated_at, NULL,
                   EXISTS (SELECT 1 FROM nodes a WHERE a.id = ANY(n.path) AND a.archived_at IS NOT NULL), NULL
              FROM document_containers c JOIN nodes n ON n.id=c.node_id
             WHERE c.deleted_at IS NULL AND __CONTAINER_NAME__ AND NOT EXISTS (SELECT 1 FROM nodes a WHERE a.id=ANY(n.path) AND a.deleted_at IS NOT NULL)
            UNION ALL
            SELECT 'chat', c.id,
                   COALESCE(c.name, n.name, (SELECT string_agg(u.display_name, ', ' ORDER BY u.username) FROM chat_members cm JOIN users u ON u.id = cm.user_id WHERE cm.chat_id = c.id AND cm.left_at IS NULL)),
                   NULL, c.node_id, c.org_id, NULL, COALESCE(c.last_message_at, c.created_at), NULL,
                   EXISTS (SELECT 1 FROM nodes a WHERE a.id = ANY(COALESCE(n.path, root.path)) AND a.archived_at IS NOT NULL), NULL
              FROM chats c LEFT JOIN nodes n ON n.id = c.node_id JOIN nodes root ON root.id = c.org_id
             WHERE (n.id IS NULL OR NOT EXISTS (SELECT 1 FROM nodes a WHERE a.id = ANY(n.path) AND a.deleted_at IS NOT NULL))
               -- A search finds a direct or group chat by the people in it (inspecting one is an audited
               -- opening, never a hit); an exact lookup of one is asked as the reader may open it.
               AND (c.node_id IS NOT NULL OR EXISTS (SELECT 1 FROM chat_members cm WHERE cm.chat_id = c.id AND cm.user_id = $1 AND cm.left_at IS NULL)
                    OR ($12::uuid[] IS NOT NULL AND ($3 OR c.org_id=ANY($2::uuid[]))))
               AND (c.node_id IS NULL OR $12::uuid[] IS NOT NULL
                    OR EXISTS (SELECT 1 FROM recents seen WHERE seen.user_id=$1 AND seen.kind='chat' AND seen.id=c.id)
                    OR EXISTS (SELECT 1 FROM favorites saved WHERE saved.user_id=$1 AND saved.kind='chat' AND saved.id=c.id)
                    OR EXISTS (SELECT 1 FROM chat_messages m
                    WHERE m.chat_id = c.id AND m.kind = 'message' AND m.deleted_at IS NULL))
               AND NOT EXISTS (SELECT 1 FROM nodes a WHERE a.id = ANY(root.path) AND a.deleted_at IS NOT NULL)
            UNION ALL
            SELECT 'tag', t.id, t.name, NULL, n.id, n.org_id, NULL, t.created_at, NULL,
                   EXISTS (SELECT 1 FROM nodes a WHERE a.id = ANY(n.path) AND a.archived_at IS NOT NULL), NULL
              FROM tags t JOIN nodes n ON n.id = t.node_id
             WHERE __TAG_NAME__ AND NOT EXISTS (SELECT 1 FROM nodes a WHERE a.id = ANY(n.path) AND a.deleted_at IS NOT NULL)
            UNION ALL
            SELECT 'user', u.id, u.display_name,
                   (SELECT string_agg(o.slug, '.' ORDER BY p.ord) FROM unnest(n.path) WITH ORDINALITY p(id, ord) JOIN orgs o ON o.node_id = p.id) || '.' || u.username,
                   n.id, n.org_id, NULL, u.updated_at, NULL, false, u.username
              FROM users u JOIN org_members m ON m.user_id = u.id JOIN nodes n ON n.id = m.domain_id
             WHERE m.org_id = ANY($2) AND NOT EXISTS (SELECT 1 FROM nodes a WHERE a.id = ANY(n.path) AND a.deleted_at IS NOT NULL)
            UNION ALL
            SELECT 'user', u.id, u.display_name,
                   (SELECT string_agg(o.slug, '.' ORDER BY p.ord) FROM unnest(n.path) WITH ORDINALITY p(id, ord) JOIN orgs o ON o.node_id = p.id) || '.' || u.username,
                   n.id, n.org_id, NULL, u.updated_at, NULL, false, u.username
              FROM users u JOIN nodes n ON n.id = u.service_org_id
             WHERE n.org_id = ANY($2) AND u.kind = 'service' AND u.status = 'active' AND NOT EXISTS (SELECT 1 FROM nodes a WHERE a.id = ANY(n.path) AND a.deleted_at IS NOT NULL)
            UNION ALL
            SELECT 'group', g.id, g.name, NULL, n.id, n.org_id, NULL, g.updated_at, NULL, false, NULL
              FROM groups g JOIN nodes n ON n.id = g.org_id
             WHERE n.org_id = ANY($2) AND NOT EXISTS (SELECT 1 FROM nodes a WHERE a.id = ANY(n.path) AND a.deleted_at IS NOT NULL)
            UNION ALL
            SELECT r.kind, r.id, r.name,
                   CASE WHEN r.node_id IS NULL THEN 'system.' || CASE WHEN r.kind='environment' THEN __ENVIRONMENT_SEGMENT__ ELSE r.name END ELSE
                     (SELECT string_agg(o.slug, '.' ORDER BY p.ord) FROM unnest(n.path) WITH ORDINALITY p(id, ord) JOIN orgs o ON o.node_id = p.id) || '.' || u.username || '.' || CASE WHEN r.kind='environment' THEN __ENVIRONMENT_SEGMENT__ ELSE r.name END END,
                   r.node_id, r.org_id, r.owner_id, r.updated_at, NULL, false, NULL
              FROM resource_names r LEFT JOIN nodes n ON n.id = r.node_id LEFT JOIN users u ON u.id = r.owner_id
             WHERE n.id IS NULL OR NOT EXISTS (SELECT 1 FROM nodes a WHERE a.id = ANY(n.path) AND a.deleted_at IS NOT NULL)
            UNION ALL
            SELECT 'machine', m.id, m.name, NULL,
                   CASE WHEN h.id IS NULL THEN m.home_id ELSE h.home_id END,
                   CASE WHEN h.id IS NULL THEN m.org_id ELSE h.org_id END,
                   m.owner_id, m.updated_at, NULL, false, NULL
              FROM machines m LEFT JOIN hosts h ON h.machine_id=m.id
             WHERE m.runtime_id IS NULL AND m.deleted_at IS NULL
               AND (m.owner_id=$1 OR $3 OR (CASE WHEN h.id IS NULL THEN m.org_id ELSE h.org_id END)=ANY($2::uuid[]))
        ), matched AS (
            SELECT a.*, r.visited_at, f.position favorite_position, (f.id IS NOT NULL) starred,
                   name ILIKE $5 ESCAPE '\' starts_name, __NAME_RANK__ match_rank, __VISIT_SCORE__ visit_score
              FROM all_names a LEFT JOIN recents r ON r.user_id=$1 AND r.kind=a.kind AND r.id=a.id
              LEFT JOIN favorites f ON f.user_id=$1 AND f.kind=a.kind AND f.id=a.id
             WHERE a.kind = ANY($6) AND ($7::uuid IS NULL OR a.org_id = $7)
               AND ($3 OR $12::uuid[] IS NOT NULL OR a.org_id IS NULL OR a.org_id = ANY($11::uuid[]) OR a.owner_id = $1)
               AND ($12::uuid[] IS NULL OR a.id=ANY($12)) AND (NOT $13::boolean OR f.id IS NOT NULL)
               AND ($12 IS NOT NULL OR ($4 = '' AND (r.visited_at IS NOT NULL OR $13)) OR ($4 <> '' AND (__NAME_MATCH__)))
        ), unique_names AS (
            SELECT DISTINCT ON (kind, id) * FROM matched ORDER BY kind, id, match_rank DESC, address
        )
        SELECT kind, id, name, address, node_id, org_id, owner_id, visited_at, archived, starred,
               updated_at AS last_activity,
               CASE WHEN archived THEN 'archived' ELSE CASE kind
                 WHEN 'thread' THEN (SELECT state FROM threads WHERE node_id=unique_names.id)
                 WHEN 'topic' THEN (SELECT status FROM topics WHERE node_id=unique_names.id)
                 WHEN 'environment' THEN (SELECT status FROM environments WHERE id=unique_names.id)
                 WHEN 'provider' THEN (SELECT status FROM providers WHERE id=unique_names.id)
                 WHEN 'agent' THEN (SELECT CASE WHEN disabled THEN 'disabled' END FROM agents WHERE id=unique_names.id)
                 WHEN 'machine' THEN (SELECT CASE WHEN online THEN 'online' ELSE 'offline' END FROM machines WHERE id=unique_names.id)
                 WHEN 'host' THEN (SELECT CASE WHEN m.online THEN 'online' ELSE 'offline' END FROM hosts h JOIN machines m ON m.id=h.machine_id WHERE h.id=unique_names.id)
               END END AS state
          FROM unique_names
         ORDER BY match_rank DESC,
                  CASE WHEN $4 = '' AND $13 THEN favorite_position END ASC NULLS LAST,
                  CASE WHEN $4 = '' AND NOT $13 THEN visited_at END DESC NULLS LAST,
                  visit_score DESC, updated_at DESC, kind, id
         OFFSET $9 LIMIT $10";
    // Names that are only a name (a document's title, a folder's, a tag's, any place but an org) are matched
    // in their own branch, where their trigram index finds them; the full match below still decides.
    let named = |column: &str| {
        format!(
            "($4 = '' OR $12::uuid[] IS NOT NULL OR {})",
            matching_sql(column, "$8")
        )
    };
    let sql = sql
        .replace("__NODE_NAME__", &named("n.name"))
        .replace("__DOCUMENT_NAME__", &named("d.title"))
        .replace("__DOCUMENT_TIER__", &name_tier_sql("h.title", "$4", "$15"))
        .replace("__CONTAINER_NAME__", &named("c.name"))
        .replace("__TAG_NAME__", &named("t.name"))
        .replace(
            "__ENVIRONMENT_SEGMENT__",
            super::addresses::ENVIRONMENT_SEGMENT_SQL,
        )
        .replace(
            "__NAME_MATCH__",
            &matching_sql("concat_ws(E'\\n',name,username,address)", "$8"),
        )
        .replace("__NAME_RANK__", &ranking_sql("name", "$4", "$8"))
        .replace(
            "__VISIT_SCORE__",
            &super::hierarchy::visit_score_sql("r.visit_count", "r.visited_at", "$14"),
        );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(ask.user)
        .bind(ask.trees)
        .bind(ask.system_admin)
        .bind(phrase)
        .bind(prefix)
        .bind(ask.kinds)
        .bind(ask.org)
        .bind(pattern)
        .bind(ask.offset)
        .bind(ask.limit)
        .bind(ask.roots)
        .bind(ask.identifiers)
        .bind(ask.starred)
        .bind(ask.at)
        .bind(query.terms())
        .fetch_all(conn)
        .await?)
}

/// An ordinary tag list that supplies a definition.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct TagScope {
    /// The tag inherited by this ordinary tag-list scope.
    pub tag_id: Id,
    /// The org, project or topic whose tag list supplies it.
    pub node_id: Id,
}

/// Scopes whose ordinary tag lists include these definitions; access is resolved in one bulk check.
pub async fn tag_scopes(conn: &mut Conn, tags: &[Id]) -> Result<Vec<TagScope>, DbError> {
    Ok(sqlx::query_as("SELECT t.id AS tag_id, n.id AS node_id FROM tags t
        JOIN nodes n ON t.node_id = ANY(n.path)
        WHERE t.id = ANY($1) AND n.kind IN ('org','project','topic')
          AND NOT EXISTS (SELECT 1 FROM nodes a WHERE a.id = ANY(n.path) AND a.deleted_at IS NOT NULL)")
        .bind(tags).fetch_all(conn).await?)
}

/// A live node inside the scope, before its access is resolved.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PlaceRow {
    /// The node.
    pub id: Id,
    /// Its kind.
    pub kind: String,
    /// Its parent, absent only for a root org.
    pub parent_id: Option<Id>,
}

/// Nodes within the scope, excluding deleted places and side threads.
pub async fn places(conn: &mut Conn, scope: Option<Id>) -> Result<Vec<PlaceRow>, DbError> {
    Ok(sqlx::query_as("SELECT n.id, n.kind, n.parent_id FROM nodes n LEFT JOIN threads th ON th.node_id = n.id WHERE ($1::uuid IS NULL OR $1 = ANY(n.path)) AND th.side_of IS NULL AND NOT EXISTS (SELECT 1 FROM nodes a WHERE a.id = ANY(n.path) AND a.deleted_at IS NOT NULL)")
        .bind(scope).fetch_all(conn).await?)
}

/// One selected scope, without enumerating its descendants.
pub async fn place(conn: &mut Conn, id: Id) -> Result<Option<PlaceRow>, DbError> {
    Ok(sqlx::query_as("SELECT n.id,n.kind,n.parent_id FROM nodes n LEFT JOIN threads t ON t.node_id=n.id
        WHERE n.id=$1 AND t.side_of IS NULL AND NOT EXISTS (SELECT 1 FROM nodes gone WHERE gone.id=ANY(n.path) AND gone.deleted_at IS NOT NULL)")
        .bind(id).fetch_optional(conn).await?)
}

/// Whether an exact selected document/chat home is live and inside the requested scope.
pub async fn within(conn: &mut Conn, node: Id, scope: Option<Id>) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM nodes n WHERE n.id=$1 AND ($2::uuid IS NULL OR n.path @> ARRAY[$2::uuid])
        AND NOT EXISTS(SELECT 1 FROM nodes gone WHERE gone.id=ANY(n.path) AND gone.deleted_at IS NOT NULL))")
        .bind(node).bind(scope).fetch_one(conn).await?)
}

/// Chats within the scope: node chats and current participation only.
pub async fn chats(
    conn: &mut Conn,
    user: Id,
    nodes: &[Id],
    scope: Option<Id>,
    include_directs: bool,
) -> Result<Vec<super::chats::ChatRow>, DbError> {
    chats_in_admin_mode(conn, user, nodes, scope, include_directs, false, &[]).await
}

/// Candidate chats for a search; native candidates are still filtered by their exact shared domains.
pub async fn chats_in_admin_mode(
    conn: &mut Conn,
    user: Id,
    nodes: &[Id],
    scope: Option<Id>,
    include_directs: bool,
    system_admin: bool,
    trees: &[Id],
) -> Result<Vec<super::chats::ChatRow>, DbError> {
    Ok(sqlx::query_as("SELECT c.* FROM chats c JOIN nodes root ON root.id = c.org_id WHERE (c.node_id = ANY($2) OR ($4 AND ($3::uuid IS NULL OR c.org_id = $3) AND ($5 OR c.org_id=ANY($6::uuid[]) OR EXISTS (SELECT 1 FROM chat_members cm WHERE cm.chat_id = c.id AND cm.user_id = $1 AND cm.left_at IS NULL)))) AND NOT EXISTS (SELECT 1 FROM nodes a WHERE a.id = ANY(root.path) AND a.deleted_at IS NOT NULL)")
        .bind(user).bind(nodes).bind(scope).bind(include_directs).bind(system_admin).bind(trees)
        .fetch_all(conn).await?)
}

/// Live documents beside the scoped nodes, before their own pages are checked.
pub async fn document_ids(conn: &mut Conn, nodes: &[Id]) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM documents WHERE node_id = ANY($1) AND deleted_at IS NULL",
    )
    .bind(nodes)
    .fetch_all(conn)
    .await?)
}

/// A stable position shared by all kinds of content result.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Cursor {
    /// How often the words occur in the hit; zero in chronological searches.
    pub matches: i64,
    /// The field tier of [`Field`]; zero in chronological searches.
    pub relevance: i32,
    /// The moment of the hit.
    pub at: DateTime<Utc>,
    /// Its globally unique item, message, document or block identifier.
    pub id: Id,
    /// Its result kind.
    pub kind: String,
    /// Distinguishes a document block from a header even if their ids coincide.
    pub block: bool,
}

/// A match in already authorized content, with the source text for its excerpt.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct HitRow {
    /// How often the words occur in the text that matched; zero for chronological results.
    pub matches: i64,
    /// The field tier carried across relevance pages ([`Field::tier`]); zero for chronological results.
    pub relevance: i32,
    /// Item, chat_message or document.
    pub kind: String,
    /// The thread, chat or document it belongs to.
    pub object_id: Id,
    /// Its stable sorting identifier.
    pub id: Id,
    /// A matched document block; absent for a title or file name.
    pub block_id: Option<Id>,
    /// The actual matched block/file/header attribution, retained through paging.
    pub author_id: Option<Id>,
    /// An agent writer's origin.
    pub author_thread_id: Option<Id>,
    /// The agent's name when that block was changed.
    pub author_agent: Option<String>,
    /// The text that matched.
    pub text: String,
    /// The text folded by the database's own collation for highlighting.
    pub folded_text: String,
    /// The query folded by the same collation as matching.
    pub folded_query: String,
    /// The moment used to order it.
    pub at: DateTime<Utc>,
}

impl HitRow {
    /// The position at which the next page begins.
    #[must_use]
    pub fn cursor(&self) -> Cursor {
        Cursor {
            matches: self.matches,
            relevance: self.relevance,
            at: self.at,
            id: self.id,
            kind: self.kind.clone(),
            block: self.block_id.is_some(),
        }
    }
}

impl CandidateSet {
    /// The sources and individual parts of one ordered content page.
    #[must_use]
    pub fn from_content(hits: &[HitRow]) -> Self {
        let mut set = Self::default();
        for hit in hits {
            match hit.kind.as_str() {
                "document" => {
                    set.documents.push(hit.object_id);
                    if let Some(block) = hit.block_id {
                        set.blocks.push(block);
                    } else {
                        set.headers.push(hit.object_id);
                    }
                }
                "chat_message" => {
                    set.chats.push(hit.object_id);
                    set.messages.push(hit.id);
                }
                "name" => {
                    set.nodes.push(hit.object_id);
                    set.names.push(hit.id);
                }
                _ => {
                    set.nodes.push(hit.object_id);
                    set.items.push(hit.id);
                }
            }
        }
        for ids in [
            &mut set.nodes,
            &mut set.names,
            &mut set.documents,
            &mut set.blocks,
            &mut set.headers,
            &mut set.chats,
            &mut set.messages,
            &mut set.items,
        ] {
            ids.sort_unstable();
            ids.dedup();
        }
        set
    }
}

/// Indexed part identities to authorize and rank without walking unrelated inventories.
#[derive(Debug, Default, Clone)]
pub struct CandidateSet {
    /// Resolve only selected metadata and ancestry; omit file/history discovery and totals.
    pub headers_only: bool,
    /// Places needed by the selected part frames.
    pub nodes: Vec<Id>,
    /// Direct or node chats needed by those frames.
    pub chats: Vec<Id>,
    /// Documents needed for current permission frames.
    pub documents: Vec<Id>,
    /// Conversation item identities.
    pub items: Vec<Id>,
    /// Chat message identities.
    pub messages: Vec<Id>,
    /// Document block identities.
    pub blocks: Vec<Id>,
    /// Documents whose own fields matched.
    pub headers: Vec<Id>,
    /// Places whose own fields matched.
    pub names: Vec<Id>,
}

/// A content search applying the existing access projection in SQL before matching and paging.
#[derive(Debug)]
pub struct Content<'a> {
    /// Explicit history only for one thread scope; absence selects each readable thread's active history.
    pub continuation: Option<Id>,
    /// Current request identity, holder groups and member roots used by the common node projection.
    pub user: Id,
    /// The current holder groups.
    pub groups: &'a [Id],
    /// Roots in which the reader's live Owners entries count.
    pub member_roots: &'a [Id],
    /// Everything's exact access decisions, including the four inherited thread profiles.
    pub grants: &'a orch_types::Json,
    /// The sampled access clock.
    pub at: DateTime<Utc>,
    /// Optional node subtree, exact chat or exact document.
    pub scope: Option<Id>,
    /// An exact chat selection.
    pub chat: Option<Id>,
    /// An exact document selection.
    pub document: Option<Id>,
    /// The ordinary participation focus, applied before LIMIT.
    pub participating: bool,
    /// Rank by [`Field`], then by how often the words occur, then the newest first; otherwise newest first.
    pub relevance: bool,
    /// Offer place names only for the unfiltered relevance search.
    pub names: bool,
    /// Keep entries with real current attachments/files.
    pub with_file: bool,
    /// The caller to require in current stored mentions.
    pub mentions_me: Option<Id>,
    /// Literal words and quoted phrases, using the one query grammar.
    pub text: &'a str,
    /// Item kinds requested.
    pub item_kinds: &'a [String],
    /// Whether chat messages were requested.
    pub chat_messages: bool,
    /// Whether documents were requested.
    pub document_content: bool,
    /// Only what this person wrote.
    pub author: Option<Id>,
    /// Inclusive lower bound.
    pub from: Option<DateTime<Utc>>,
    /// Inclusive upper bound.
    pub to: Option<DateTime<Utc>>,
    /// A position from the previous page.
    pub after: Option<&'a Cursor>,
    /// The page size, with one extra for whether another page exists.
    pub limit: i64,
    /// An indexed candidate window; none keeps the ordinary exact scope reader.
    pub selected: Option<&'a CandidateSet>,
    /// The conversation items already chosen by [`items::newest`]; none reads every item of each history.
    pub items: Option<&'a [Id]>,
}

/// Filter the containing source before a candidate window or result page is bounded.
pub(super) fn advanced_source_sql(selection: &str, kind: &str, source: &str, part: &str) -> String {
    let fact = |document: &str, node: &str, chat: &str| {
        format!(
            "CASE {kind} WHEN 'document' THEN (SELECT {document} FROM documents adv_d WHERE adv_d.id={source}) \
             WHEN 'item' THEN (SELECT {node} FROM nodes adv_n WHERE adv_n.id={source}) \
             WHEN 'name' THEN (SELECT {node} FROM nodes adv_n WHERE adv_n.id={source}) \
             WHEN 'chat_message' THEN (SELECT {chat} FROM chats adv_c WHERE adv_c.id={source}) END"
        )
    };
    let owner = fact("adv_d.owner_id", "adv_n.owner_id", "NULL::uuid");
    let creator = fact("adv_d.created_by", "adv_n.created_by", "adv_c.created_by");
    let chat_editor =
        format!("(SELECT adv_m.author_id FROM chat_messages adv_m WHERE adv_m.id={part})");
    let editor = fact("adv_d.updated_by", "NULL::uuid", &chat_editor);
    let chat_created =
        format!("(SELECT adv_m.created_at FROM chat_messages adv_m WHERE adv_m.id={part})");
    let created = fact("adv_d.created_at", "adv_n.created_at", &chat_created);
    let chat_edited = format!(
        "(SELECT COALESCE(adv_m.edited_at,adv_m.created_at) FROM chat_messages adv_m WHERE adv_m.id={part})"
    );
    let edited = fact("adv_d.updated_at", "adv_n.updated_at", &chat_edited);
    advanced_facts_sql(selection, &owner, &creator, &editor, &created, &edited)
}

/// Compare the recorded source facts without changing the ordering of candidate streams.
pub(super) fn advanced_facts_sql(
    selection: &str,
    owner: &str,
    creator: &str,
    editor: &str,
    created: &str,
    edited: &str,
) -> String {
    include_str!("search_source_facts.sql")
        .replace("__SELECTION__", selection)
        .replace("__OWNER__", owner)
        .replace("__CREATOR__", creator)
        .replace("__EDITOR__", editor)
        .replace("__CREATED__", created)
        .replace("__EDITED__", edited)
}

/// Avoid metadata joins when the date choice has no bounds and no person filter is active.
pub(super) fn advanced_source_needed(selection: Option<&orch_types::Json>) -> bool {
    let present = |key: &str| {
        selection
            .and_then(|value| value.get(key))
            .is_some_and(|value| !value.is_null())
    };
    ["owner_id", "created_by_id", "edited_by_id"]
        .iter()
        .any(|key| present(key))
        || present("date_field") && (present("from") || present("to"))
}

/// Whether a selection asks for a current file type.
pub(super) fn advanced_file_needed(selection: Option<&orch_types::Json>) -> bool {
    selection
        .and_then(|value| value.get("file_type"))
        .is_some_and(|value| !value.is_null())
}

/// Whether a selection asks for the caller's shared items.
pub(super) fn advanced_shared_needed(selection: Option<&orch_types::Json>) -> bool {
    selection
        .and_then(|value| value.get("shared_with_me"))
        .and_then(orch_types::Json::as_bool)
        == Some(true)
}

/// Match an exact MIME type or all types in one MIME family.
pub(super) fn file_type_sql(selection: &str, media: &str) -> String {
    include_str!("search_file_type.sql")
        .replace("__SELECTION__", selection)
        .replace("__MEDIA_TYPE__", media)
}

/// Test the media types carried in a current item or message payload.
pub(super) fn file_json_sql(selection: &str, payload: &str) -> String {
    include_str!("search_payload_file.sql")
        .replace("__SELECTION__", selection)
        .replace("__PAYLOAD__", payload)
}

/// A candidate must carry the requested file type before it can fill a bounded window.
pub(super) fn advanced_file_sql(
    selection: &str,
    kind: &str,
    source: &str,
    part: &str,
    revisions: bool,
) -> String {
    let document = file_type_sql(selection, "adv_f.media_type");
    let item = file_json_sql(selection, "adv_i.payload");
    let revision = if revisions {
        format!(
            "OR EXISTS (SELECT 1 FROM thread_item_revisions adv_r WHERE adv_r.item_id={part} AND {})",
            file_json_sql(selection, "adv_r.body->'payload'")
        )
    } else {
        String::new()
    };
    let chat = file_json_sql(selection, "adv_m.content");
    format!(
        "({selection}->>'file_type' IS NULL OR CASE {kind} \
          WHEN 'document' THEN (EXISTS(SELECT 1 FROM documents adv_d WHERE adv_d.id={source} AND adv_d.search_file_count>0) AND EXISTS(SELECT 1 FROM document_files adv_f WHERE adv_f.document_id={source} AND {document})) \
          WHEN 'item' THEN (EXISTS(SELECT 1 FROM items adv_i WHERE adv_i.id={part} AND {item}) {revision}) \
          WHEN 'chat_message' THEN EXISTS(SELECT 1 FROM chat_messages adv_m WHERE adv_m.id={part} AND {chat}) \
          ELSE false END)"
    )
}

/// Use the same named holders as the shared-items view, followed by the ordinary current Read decision.
pub(super) fn advanced_shared_sql(selection: &str, kind: &str, source: &str) -> String {
    let node = format!(
        "CASE {kind} WHEN 'document' THEN (SELECT node_id FROM documents WHERE id={source}) \
        WHEN 'item' THEN {source} WHEN 'name' THEN {source} \
        WHEN 'chat_message' THEN (SELECT node_id FROM chats WHERE id={source}) END"
    );
    let path = format!("(SELECT path FROM nodes WHERE id=({node}))");
    let holders = format!(
        "jsonb_to_recordset(COALESCE({selection}->'search_holders','[]'::jsonb)) AS adv_h(kind text,id uuid,role text)"
    );
    let holder = format!(
        "EXISTS (SELECT 1 FROM {holders} WHERE \
        (adv_e.holder_kind=adv_h.kind AND adv_e.holder_id=adv_h.id AND adv_e.holder_kind IN ('user','group','members')) OR \
        (adv_e.holder_kind='role' AND adv_h.kind IN ('role','visible') AND adv_h.id=COALESCE(adv_e.holder_id,({node})) \
          AND array_position(ARRAY['viewer','interactor','developer','admin'],adv_h.role)>=array_position(ARRAY['viewer','interactor','developer','admin'],adv_e.holder_role)) OR \
        (adv_e.holder_kind='everyone' AND adv_h.kind='visible' AND adv_h.id=ANY({path})))"
    );
    let folder = format!(
        "WITH RECURSIVE parents AS ( \
        SELECT id,parent_container_id FROM document_containers WHERE id=(SELECT parent_container_id FROM documents WHERE id={source}) \
        UNION SELECT p.id,p.parent_container_id FROM document_containers p JOIN parents c ON p.id=c.parent_container_id) \
        SELECT id FROM parents"
    );
    format!(
        "(NOT COALESCE(({selection}->>'shared_with_me')::boolean,false) OR \
      ({kind}='document' AND EXISTS(SELECT 1 FROM documents adv_d WHERE adv_d.id={source} AND adv_d.owner_id=({selection}->>'search_user')::uuid)) OR \
      EXISTS(SELECT 1 FROM permission_entries adv_e WHERE adv_e.page='live' AND adv_e.removed_at IS NULL \
        AND (adv_e.expires_at IS NULL OR adv_e.expires_at>({selection}->>'search_at')::timestamptz) AND {holder} AND \
        (({kind}='document' AND adv_e.object_kind='document' AND adv_e.object_id={source}) OR \
         ({kind}='document' AND adv_e.object_kind='document_container' AND adv_e.object_id IN ({folder})) OR \
         (adv_e.object_kind='node' AND adv_e.object_id=({node}) AND adv_e.permission<>'access.own'))) OR \
      EXISTS(SELECT 1 FROM node_roles adv_r JOIN {holders} ON adv_r.subject_kind=adv_h.kind AND adv_r.subject_id=adv_h.id \
        WHERE adv_r.node_id=ANY({path}) AND adv_r.removed_at IS NULL \
        AND (adv_r.expires_at IS NULL OR adv_r.expires_at>({selection}->>'search_at')::timestamptz) \
        AND (adv_r.subject_kind<>'role' OR array_position(ARRAY['viewer','interactor','developer','admin'],adv_h.role)>=array_position(ARRAY['viewer','interactor','developer','admin'],adv_r.subject_role))))"
    )
}

/// One indexed content statement; inherited access, focus, filters and ordering all precede LIMIT.
pub async fn content(conn: &mut Conn, ask: &Content<'_>) -> Result<Vec<HitRow>, DbError> {
    content_rows(conn, ask, false).await
}

/// One ordered raw page, used only to choose the identities for a subsequent exact access frame.
/// Its text and identities must never be returned before `content` checks the selected sources.
pub async fn content_candidates(
    conn: &mut Conn,
    ask: &Content<'_>,
) -> Result<Vec<HitRow>, DbError> {
    content_rows(conn, ask, true).await
}

/// The words of a content search: literal for an expression's own literal, otherwise the one query grammar.
fn query_of(ask: &Content<'_>) -> Query {
    if ask
        .grants
        .get("search_regex")
        .and_then(orch_types::Json::as_bool)
        == Some(true)
    {
        Query::literal(ask.text)
    } else {
        Query::parse(ask.text)
    }
}

/// Whether an item `i` of the history owner `o` matches the search, as one SQL condition.
fn item_filter(ask: &Content<'_>) -> String {
    let file_type = if advanced_file_needed(ask.grants.get("search_advanced")) {
        file_json_sql("($5::jsonb->'search_advanced')", "i.payload")
    } else {
        "true".to_owned()
    };
    include_str!("search_item_filter.sql")
        .replace("__ITEM_MATCH__", &matching_sql("i.search_text", "$1"))
        .replace("__ITEM_FILE_TYPE__", &file_type)
}

/// Bind a content statement's thirty-six parameters, in the order its SQL numbers them.
fn bind_content<'q, O>(
    statement: sqlx::query::QueryAs<'q, sqlx::Postgres, O, sqlx::postgres::PgArguments>,
    ask: &'q Content<'q>,
    query: &'q Query,
) -> sqlx::query::QueryAs<'q, sqlx::Postgres, O, sqlx::postgres::PgArguments> {
    statement
        .bind(query.like_patterns())
        .bind(ask.user)
        .bind(ask.groups)
        .bind(ask.member_roots)
        .bind(ask.grants)
        .bind(ask.scope)
        .bind(ask.chat)
        .bind(ask.document)
        .bind(ask.item_kinds)
        .bind(ask.chat_messages)
        .bind(ask.document_content)
        .bind(ask.author)
        .bind(ask.from)
        .bind(ask.to)
        .bind(ask.after.map(|cursor| cursor.at))
        .bind(ask.after.map(|cursor| cursor.id))
        .bind(ask.after.map(|cursor| cursor.kind.as_str()))
        .bind(ask.after.map(|cursor| cursor.block))
        .bind(ask.limit)
        .bind(ask.text)
        .bind(ask.with_file)
        .bind(ask.mentions_me)
        .bind(ask.participating)
        .bind(ask.at)
        .bind(ask.relevance)
        .bind(ask.after.map_or(0, |cursor| cursor.relevance))
        .bind(ask.names)
        .bind(query.terms())
        .bind(ask.after.map_or(0, |cursor| cursor.matches))
        .bind(ask.continuation)
        .bind(ask.selected.map(|set| set.documents.as_slice()))
        .bind(ask.selected.map(|set| set.items.as_slice()))
        .bind(ask.selected.map(|set| set.messages.as_slice()))
        .bind(ask.selected.map(|set| set.blocks.as_slice()))
        .bind(ask.selected.map(|set| set.headers.as_slice()))
        .bind(ask.selected.map(|set| set.names.as_slice()))
}

async fn content_rows(
    conn: &mut Conn,
    ask: &Content<'_>,
    candidates: bool,
) -> Result<Vec<HitRow>, DbError> {
    let query = query_of(ask);
    let visibility = if candidates {
        // Keep history selection and liveness identical; only the access decision is deferred.
        include_str!("search_candidate_visibility.sql").to_owned()
    } else {
        super::everything::node_visibility_sql("$2", "$3", "$4", "$5", "$24")
    };
    let mut sql = include_str!("search_content.sql")
        .replace("__NODE_VISIBILITY__", &visibility)
        .replace("__HISTORY__", include_str!("search_history.sql"))
        .replace("__ITEM_FILTER__", &item_filter(ask))
        .replace(
            "__CONTENT_CANDIDATES__",
            if candidates { "true" } else { "false" },
        )
        .replace("__HEADERS_MATCH__", &matching_sql("text", "$1"))
        .replace(
            "__OLD_ITEM_MATCH__",
            &matching_sql("r.body->>'search_text'", "$1"),
        )
        .replace("__CHAT_MATCH__", &matching_sql("m.search_text", "$1"))
        .replace("__BLOCK_MATCH__", &matching_sql("b.text", "$1"))
        .replace("__NODE_MATCH__", &matching_sql("x.text", "$1"))
        .replace("__OCCURRENCES__", &occurrences_sql("m.text", "$28"))
        .replace("__TITLE__", &Field::Title.tier().to_string())
        .replace("__DESCRIPTION__", &Field::Description.tier().to_string())
        .replace("__HEADING__", &Field::Heading.tier().to_string());
    // Rank in SQL, but carry only identities and the existing cursor into the access phase.
    sql = sql.replace(
        "__CONTENT_COLUMNS__",
        if candidates {
            include_str!("search_candidate_columns.sql")
        } else {
            "*,lower(text) folded_text,lower($20::text) folded_query"
        },
    );
    let selection = ask.grants.get("search_advanced");
    let advanced_source = if advanced_source_needed(selection) {
        advanced_source_sql(
            "($5::jsonb->'search_advanced')",
            "m.kind",
            "m.object_id",
            "m.id",
        )
    } else {
        "true".to_owned()
    };
    let advanced_shared = if advanced_shared_needed(selection) {
        advanced_shared_sql("($5::jsonb->'search_advanced')", "m.kind", "m.object_id")
    } else {
        "true".to_owned()
    };
    sql = sql
        .replace("__ADVANCED_SOURCE__", &advanced_source)
        .replace("__ADVANCED_SHARED__", &advanced_shared);
    let advanced = "($5::jsonb->'search_advanced')";
    let document_file = if advanced_file_needed(selection) {
        format!(
            "d.search_file_count>0 AND EXISTS(SELECT 1 FROM document_files adv_f WHERE adv_f.document_id=d.id AND {})",
            file_type_sql(advanced, "adv_f.media_type")
        )
    } else {
        "true".to_owned()
    };
    let header_file = if advanced_file_needed(selection) {
        file_type_sql(advanced, "f.media_type")
    } else {
        "true".to_owned()
    };
    let json_file = |payload| {
        if advanced_file_needed(selection) {
            file_json_sql(advanced, payload)
        } else {
            "true".to_owned()
        }
    };
    sql = sql
        .replace("__DOCUMENT_FILE_TYPE__", &document_file)
        .replace("__HEADER_FILE_TYPE__", &header_file)
        .replace("__REVISION_FILE_TYPE__", &json_file("r.body->'payload'"))
        .replace("__CHAT_FILE_TYPE__", &json_file("m.content"));
    if ask.selected.is_some() {
        // The text indexes can underestimate short/common matches. Hydrate only the already bounded
        // identities before matching, so that a broad index is never scanned for a few selected parts.
        sql = sql
            .replace(
                "FROM items i JOIN conversation_owners",
                "FROM selected_items i JOIN conversation_owners",
            )
            .replace(
                "JOIN items i ON i.id=r.item_id",
                "JOIN selected_items i ON i.id=r.item_id",
            )
            .replace(
                "FROM chat_messages m JOIN readable_chats",
                "FROM selected_messages m JOIN readable_chats",
            )
            .replace(
                "FROM document_blocks b JOIN readable_documents",
                "FROM selected_blocks b JOIN readable_documents",
            );
        sql = sql.replacen(
            "WITH ",
            "WITH selected_items AS MATERIALIZED (SELECT * FROM items WHERE id=ANY($32)),
            selected_messages AS MATERIALIZED (SELECT * FROM chat_messages WHERE id=ANY($33)),
            selected_blocks AS MATERIALIZED (SELECT * FROM document_blocks WHERE id=ANY($34)), ",
            1,
        );
    } else if ask.items.is_some() {
        // The conversation items were chosen beforehand, newest first and without reading a history whole;
        // every other source and the one ordering stay as they are.
        sql = sql.replace(
            "FROM items i JOIN conversation_owners",
            "FROM bounded_items i JOIN conversation_owners",
        );
        sql = sql.replacen(
            "WITH ",
            "WITH bounded_items AS MATERIALIZED (SELECT * FROM items WHERE id=ANY($37)), ",
            1,
        );
    }
    Ok(bind_content(
        sqlx::query_as(sqlx::AssertSqlSafe(sql)).persistent(false),
        ask,
        &query,
    )
    .bind(ask.items)
    .fetch_all(conn)
    .await?)
}

/// One file chosen for a document already authorized by the search caller.
#[derive(Debug, sqlx::FromRow)]
pub struct MatchedFile {
    /// The document represented by this result.
    pub document_id: Id,
    /// Its matching current file.
    #[sqlx(flatten)]
    pub file: super::documents::FileRow,
}

/// Choose at most one matching filename or typed fallback per selected document, without sorting its files.
pub async fn files_for_hits(
    conn: &mut Conn,
    documents: &[Id],
    names: &[&str],
    focused: bool,
    media_type: Option<&str>,
) -> Result<Vec<MatchedFile>, DbError> {
    let (predicate, media) = match media_type {
        None | Some("*/*") => ("$4::text IS NULL", None),
        Some(media) if media.ends_with("/*") => (
            "position('/' in f.media_type)>0 AND split_part(lower(f.media_type),'/',1)=$4",
            Some(media[..media.len() - 2].to_ascii_lowercase()),
        ),
        Some(media) => ("lower(f.media_type)=$4", Some(media.to_ascii_lowercase())),
    };
    let sql = include_str!("search_files.sql").replace("__FILE_TYPE__", predicate);
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .persistent(false)
        .bind(documents)
        .bind(names)
        .bind(focused)
        .bind(media)
        .fetch_all(conn)
        .await?)
}

/// Items of a search page, read together after their thread access was resolved.
pub async fn items_by_ids(
    conn: &mut Conn,
    ids: &[Id],
) -> Result<Vec<super::threads::ItemRow>, DbError> {
    Ok(
        sqlx::query_as("SELECT i.* FROM items i WHERE i.id = ANY($1)")
            .bind(ids)
            .fetch_all(conn)
            .await?,
    )
}

/// Items to repair, in bounded batches ordered by their identifiers.
pub async fn projection_items(
    conn: &mut Conn,
    after: Option<Id>,
    limit: i64,
) -> Result<Vec<super::threads::ItemRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT i.* FROM items i WHERE ($1::uuid IS NULL OR i.id > $1) ORDER BY i.id LIMIT $2",
    )
    .bind(after)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// Legacy context-bearing projections, found through the same partial trigram index, for bounded repair.
pub async fn context_projection_items(
    conn: &mut Conn,
    after: Option<Id>,
    limit: i64,
) -> Result<Vec<super::threads::ItemRow>, DbError> {
    Ok(sqlx::query_as("SELECT i.* FROM items i WHERE i.kind <> 'marker' AND i.search_text IS NOT NULL
            AND i.search_text LIKE '%<orch>%' AND ($1::uuid IS NULL OR i.id > $1) ORDER BY i.id LIMIT $2")
        .bind(after).bind(limit).fetch_all(conn).await?)
}

/// Repairs a batch only while the source payload and status still match.
pub async fn put_item_texts(conn: &mut Conn, updates: &orch_types::Json) -> Result<u64, DbError> {
    Ok(sqlx::query("UPDATE items i SET search_text = x.text FROM jsonb_to_recordset($1) AS x(id uuid, payload jsonb, status text, text text) WHERE i.id = x.id AND i.payload = COALESCE(x.payload, 'null'::jsonb) AND i.status = x.status AND i.search_text IS DISTINCT FROM x.text")
        .bind(updates).execute(conn).await?.rows_affected())
}

/// Messages to repair, including deleted rows whose old text must be cleared.
pub async fn projection_messages(
    conn: &mut Conn,
    after: Option<Id>,
    limit: i64,
) -> Result<Vec<super::chats::MessageRow>, DbError> {
    Ok(sqlx::query_as("SELECT m.* FROM chat_messages m WHERE ($1::uuid IS NULL OR m.id > $1) ORDER BY m.id LIMIT $2")
        .bind(after).bind(limit).fetch_all(conn).await?)
}

/// Repairs a batch only while each message's source and deletion state match.
pub async fn put_message_texts(
    conn: &mut Conn,
    updates: &orch_types::Json,
) -> Result<u64, DbError> {
    Ok(sqlx::query("UPDATE chat_messages m SET search_text = x.text FROM jsonb_to_recordset($1) AS x(id uuid, content jsonb, deleted_at timestamptz, text text) WHERE m.id = x.id AND m.content = COALESCE(x.content, 'null'::jsonb) AND m.deleted_at IS NOT DISTINCT FROM x.deleted_at AND m.search_text IS DISTINCT FROM x.text")
        .bind(updates).execute(conn).await?.rows_affected())
}

/// A block's source fields, sufficient to reproduce its plain-text projection.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ProjectionBlock {
    /// The block.
    pub id: Id,
    /// Its type.
    pub block_type: String,
    /// Its properties.
    pub props: orch_types::Json,
    /// Its inline text or table cells.
    pub content: orch_types::Json,
    /// Whether it was removed.
    pub removed_at: Option<DateTime<Utc>>,
}

/// Blocks to repair; removed blocks are read so their old projection is cleared.
pub async fn projection_blocks(
    conn: &mut Conn,
    after: Option<Id>,
    limit: i64,
) -> Result<Vec<ProjectionBlock>, DbError> {
    Ok(sqlx::query_as("SELECT id, type AS block_type, props, content, removed_at FROM document_blocks WHERE ($1::uuid IS NULL OR id > $1) ORDER BY id LIMIT $2")
        .bind(after).bind(limit).fetch_all(conn).await?)
}

/// Repairs a batch only while its properties, content, type and removal state match.
pub async fn put_block_texts(conn: &mut Conn, updates: &orch_types::Json) -> Result<u64, DbError> {
    Ok(sqlx::query("UPDATE document_blocks b SET text = x.text FROM jsonb_to_recordset($1) AS x(id uuid, props jsonb, content jsonb, block_type text, removed_at timestamptz, text text) WHERE b.id = x.id AND b.props = x.props AND b.content = x.content AND b.type = x.block_type AND b.removed_at IS NOT DISTINCT FROM x.removed_at AND b.text IS DISTINCT FROM x.text")
        .bind(updates).execute(conn).await?.rows_affected())
}

/// The title of an approval, only in the thread whose item names it.
pub async fn approval_title(
    conn: &mut Conn,
    thread: Id,
    id: Id,
) -> Result<Option<String>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT title FROM approvals WHERE id = $1 AND thread_id = $2")
            .bind(id)
            .bind(thread)
            .fetch_optional(conn)
            .await?,
    )
}

/// The parts that pieces found by meaning begin with, to be checked against what a reader may read now.
#[derive(Debug)]
pub struct MeaningAsk<'a> {
    /// Current request identity, holder groups and member roots used by the common node projection.
    pub user: Id,
    /// The current holder groups.
    pub groups: &'a [Id],
    /// Roots in which the reader's live Owners entries count.
    pub member_roots: &'a [Id],
    /// Everything's exact access decisions.
    pub grants: &'a orch_types::Json,
    /// The sampled access clock.
    pub at: DateTime<Utc>,
    /// Optional node subtree.
    pub scope: Option<Id>,
    /// The ordinary participation focus.
    pub participating: bool,
    /// Items of conversations.
    pub items: &'a [Id],
    /// Messages of chats.
    pub messages: &'a [Id],
    /// Blocks of documents.
    pub blocks: &'a [Id],
    /// The query, folded for highlighting as content search does.
    pub text: &'a str,
    /// Item kinds requested.
    pub item_kinds: &'a [String],
    /// Whether chat messages were requested.
    pub chat_messages: bool,
    /// Whether documents were requested.
    pub document_content: bool,
    /// Only parts created at or after this time.
    pub from: Option<DateTime<Utc>>,
    /// Only parts created at or before this time.
    pub to: Option<DateTime<Utc>>,
    /// An exact chat, document, author, attachment or mention filter, shared with lexical retrieval.
    pub chat: Option<Id>,
    /// An exact document.
    pub document: Option<Id>,
    /// An exact author.
    pub author: Option<Id>,
    /// Whether a current file is required.
    pub with_file: bool,
    /// The person who must be mentioned.
    pub mentions_me: Option<Id>,
}

/// Of the parts found by meaning, those the reader may read now, as hits of content search (no field tier and no
/// word count: nothing of them matched the words).
pub async fn meaning_hits(conn: &mut Conn, ask: &MeaningAsk<'_>) -> Result<Vec<HitRow>, DbError> {
    if ask.items.is_empty() && ask.messages.is_empty() && ask.blocks.is_empty() {
        return Ok(Vec::new());
    }
    let selection = ask.grants.get("search_advanced");
    let advanced = "($4::jsonb->'search_advanced')";
    let source = if advanced_source_needed(selection) {
        advanced_source_sql(advanced, "found.kind", "found.object_id", "found.id")
    } else {
        "true".to_owned()
    };
    let file = if advanced_file_needed(selection) {
        advanced_file_sql(advanced, "found.kind", "found.object_id", "found.id", false)
    } else {
        "true".to_owned()
    };
    let shared = if advanced_shared_needed(selection) {
        advanced_shared_sql(advanced, "found.kind", "found.object_id")
    } else {
        "true".to_owned()
    };
    let sql = include_str!("search_meaning.sql")
        .replace(
            "__NODE_VISIBILITY__",
            &super::everything::node_visibility_sql("$1", "$2", "$3", "$4", "$5"),
        )
        .replace("__ADVANCED_SOURCE__", &source)
        .replace("__ADVANCED_FILE__", &file)
        .replace("__ADVANCED_SHARED__", &shared);
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .persistent(false)
        .bind(ask.user)
        .bind(ask.groups)
        .bind(ask.member_roots)
        .bind(ask.grants)
        .bind(ask.at)
        .bind(ask.scope)
        .bind(ask.participating)
        .bind(ask.items)
        .bind(ask.messages)
        .bind(ask.blocks)
        .bind(ask.text)
        .bind(ask.item_kinds)
        .bind(ask.chat_messages)
        .bind(ask.document_content)
        .bind(ask.from)
        .bind(ask.to)
        .bind(ask.chat)
        .bind(ask.document)
        .bind(ask.author)
        .bind(ask.with_file)
        .bind(ask.mentions_me)
        .fetch_all(conn)
        .await?)
}

/// Conservative scopes before candidate ranking; each returned part still requires its exact access frame.
/// Exceptional document pages remain candidates even when their containing place is not readable.
pub const CANDIDATE_ACCESS: &str = r#"(__ACCESS__::jsonb IS NULL
 OR __ACCESS__->>'unrestricted'='true'
 OR (__KIND__='document' AND __SOURCE__ IN(SELECT jsonb_array_elements_text(__ACCESS__->'documents')::uuid))
 OR (__KIND__ IN ('chat','chat_message') AND __SOURCE__ IN(SELECT jsonb_array_elements_text(__ACCESS__->'chats')::uuid))
 OR (__KIND__='name' AND (__SOURCE__ IN(SELECT jsonb_array_elements_text(__ACCESS__->'names')::uuid)
    OR (__ACCESS__->'names' IS NULL AND __PATH__ && ARRAY(SELECT jsonb_array_elements_text(__ACCESS__->'scopes')::uuid))))
 OR (__KIND__='document' AND __PATH__ && ARRAY(SELECT jsonb_array_elements_text(COALESCE(__ACCESS__->'doc_scopes',__ACCESS__->'scopes'))::uuid))
 OR (__KIND__ IN ('item','conversation') AND __PATH__ && ARRAY(SELECT jsonb_array_elements_text(COALESCE(__ACCESS__->'thread_scopes',__ACCESS__->'scopes'))::uuid))
 OR (__KIND__ IN ('chat','chat_message') AND EXISTS(SELECT 1 FROM chats visible_chat WHERE visible_chat.id=__SOURCE__ AND visible_chat.node_id IS NOT NULL
    AND (visible_chat.node_id IN(SELECT jsonb_array_elements_text(__ACCESS__->'chat_nodes')::uuid)
      OR __PATH__ && ARRAY(SELECT jsonb_array_elements_text(COALESCE(__ACCESS__->'chat_scopes',__ACCESS__->'scopes'))::uuid)) OFFSET 0))
 OR __OWNER__=(__ACCESS__->>'person')::uuid
 OR (__KIND__='document' AND (EXISTS(SELECT 1 FROM documents own WHERE own.id=__SOURCE__ AND (own.owner_id=(__ACCESS__->>'person')::uuid OR own.parent_container_id IS NOT NULL) OFFSET 0)
   OR EXISTS(SELECT 1 FROM permission_entries pe1 WHERE pe1.object_kind IN ('document','document_container') AND pe1.object_id=__SOURCE__
      AND pe1.removed_at IS NULL AND (pe1.permission<>'access.own' OR (pe1.holder_kind='user' AND pe1.holder_id=(__ACCESS__->>'person')::uuid)
        OR (pe1.holder_kind='group' AND pe1.holder_id IN(SELECT jsonb_array_elements_text(__ACCESS__->'groups')::uuid))) OFFSET 0)))
 OR (__KIND__ IN ('chat','chat_message') AND (EXISTS(SELECT 1 FROM chat_members member WHERE member.chat_id=__SOURCE__
      AND member.user_id=(__ACCESS__->>'person')::uuid AND member.left_at IS NULL OFFSET 0)
   OR EXISTS(SELECT 1 FROM permission_entries pe2 WHERE pe2.object_kind='chat' AND pe2.object_id=__SOURCE__ AND pe2.removed_at IS NULL OFFSET 0))))"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> DateTime<Utc> {
        text.parse().unwrap()
    }

    #[test]
    fn words_are_counted_without_regard_to_case_in_any_script() {
        let english = Query::parse("Cookie session");
        assert_eq!(
            english.occurrences("The cookie and the COOKIE jar; a session."),
            3
        );
        let chinese = Query::parse("登录 失败");
        assert_eq!(chinese.occurrences("登录失败：再次登录时也失败了"), 4);
        assert_eq!(
            Query::parse("\"会话 cookie\"").occurrences("会话 cookie 与 会话 COOKIE"),
            2
        );
        assert_eq!(english.occurrences("nothing here"), 0);
    }

    #[test]
    fn a_title_outranks_a_description_a_heading_and_a_body_before_counts_and_dates_are_compared() {
        let rank = |field, count, when: &str, id: u128| Rank {
            field,
            count,
            at: at(when),
            id: Id::from_u128(id),
        };
        let mut ranks = [
            rank(Field::Body, 9, "2026-10-20T00:00:00Z", 1),
            rank(Field::Heading, 1, "2026-10-01T00:00:00Z", 2),
            rank(Field::Title, 1, "2026-01-01T00:00:00Z", 3),
            rank(Field::Description, 5, "2026-10-21T00:00:00Z", 4),
            rank(Field::Body, 9, "2026-10-22T00:00:00Z", 5),
            rank(Field::Body, 9, "2026-10-22T00:00:00Z", 6),
            rank(Field::Body, 10, "2026-01-01T00:00:00Z", 7),
        ];
        ranks.sort_by_key(|rank| std::cmp::Reverse(rank.clone()));
        let order: Vec<u128> = ranks.iter().map(|rank| rank.id.as_u128()).collect();
        assert_eq!(order, vec![3, 4, 2, 7, 6, 5, 1]);
        assert!(Field::Title.tier() > Field::Description.tier());
        assert!(Field::Heading.tier() > Field::Body.tier());
    }
}
