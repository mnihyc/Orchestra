//! Indexed hierarchy pages whose visibility is supplied by the shared access engine, with stable keyset cursors.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Id, Json};
use serde::{Deserialize, Serialize};

/// The access engine's decision about a real page or an otherwise uncustomized thread.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Grant {
    /// The same native-only View provenance as the opened object's frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admin_access: Option<orch_access_contract::roles::AdminReach>,
    /// Explicit personal/group participation or a follow, independently of ordinary View.
    #[serde(default)]
    pub participating: bool,
    /// Whether the content may be read.
    pub view: bool,
    /// Whether the ordinary listings may reveal its identity, including a private resource's name.
    #[serde(default)]
    pub listed: bool,
    /// Whether the finder may name this path step.
    pub named: bool,
    /// The reader's own level, in the words of the wire contract.
    pub access: String,
    /// Whether the reader may share this page.
    pub share: bool,
    /// Whether the reader may open its settings.
    pub settings: bool,
    /// A node's chat is authorized by that node's chat area, including when its main area is private.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat: Option<Box<Grant>>,
}

/// The identifiers needed to prepare a request's access projection, without reading ordinary threads.
#[derive(Debug, sqlx::FromRow)]
pub struct AccessSeed {
    /// A node, document, resource or chat.
    pub kind: String,
    /// Its identifier.
    pub id: Id,
}

/// Read places and exceptional thread pages, plus the much smaller resource and document inventories.
/// Ordinary ownership entries are resolved by the same live Owners rows in the final statement.
pub async fn access_seeds(conn: &mut Conn, parent: Option<Id>) -> Result<Vec<AccessSeed>, DbError> {
    Ok(sqlx::query_as(include_str!("everything_seeds.sql"))
        .bind(parent)
        .fetch_all(conn)
        .await?)
}

/// Only the selected content inventories, with their node and environment dependencies and the hierarchy's access seeds.
pub async fn search_seeds(
    conn: &mut Conn,
    parent: Option<Id>,
    documents: &[Id],
    chats: &[Id],
    nodes: &[Id],
) -> Result<Vec<AccessSeed>, DbError> {
    search_seeds_window(conn, parent, documents, chats, nodes, None).await
}

/// A bounded discovery page; callers discard an incomplete page before deriving candidate hints.
pub async fn search_seeds_window(
    conn: &mut Conn,
    parent: Option<Id>,
    documents: &[Id],
    chats: &[Id],
    nodes: &[Id],
    limit: Option<i64>,
) -> Result<Vec<AccessSeed>, DbError> {
    Ok(sqlx::query_as(include_str!("search_access_seeds.sql"))
        .bind(parent)
        .bind(documents)
        .bind(chats)
        .bind(nodes)
        .bind(limit)
        .fetch_all(conn)
        .await?)
}

/// A cursor includes the nullable sort key and stable tie breakers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cursor {
    /// Shared name relevance; zero outside a name search.
    #[serde(default)]
    pub relevance: i32,
    /// The visit clock stays fixed across pages; access still uses the current request's clock.
    #[serde(default)]
    pub score_at: Option<DateTime<Utc>>,
    /// The shared decaying personal visit score.
    #[serde(default)]
    pub visit_score: f64,
    /// Activity is the next search tie-break, before the chosen column.
    #[serde(default)]
    pub search_activity: Option<DateTime<Utc>>,
    /// The key as text; numeric and time keys are fixed-width sortable representations.
    pub key: Option<String>,
    /// The last kind, to disambiguate identifiers belonging to different tables.
    pub kind: String,
    /// The last identifier, including a navigational group's identifier.
    pub id: String,
}

/// What a page of the inventory supplies before public references are assembled.
#[derive(Debug, sqlx::FromRow)]
pub struct Row {
    /// Shared name relevance, kept in the keyset position.
    pub relevance: i32,
    /// The clock and keys of this search page.
    pub score_at: DateTime<Utc>,
    /// The shared decaying personal visit score.
    pub visit_score: f64,
    /// Activity only while searching; null while browsing a branch.
    pub search_activity: Option<DateTime<Utc>>,
    /// Its kind.
    pub kind: String,
    /// Its identifier.
    pub id: String,
    /// Its actual name, withheld by the route for a redacted path step.
    pub name: String,
    /// Its direct parent.
    pub parent_id: Option<String>,
    /// The nodes on the way to it.
    pub path: Vec<Id>,
    /// Its root org, where it has one.
    pub org_id: Option<Id>,
    /// Its recorded maker.
    pub created_by: Option<Id>,
    /// Its first owner.
    pub owner_id: Option<Id>,
    /// When it was made.
    pub created_at: Option<DateTime<Utc>>,
    /// Last recorded activity.
    pub last_activity: Option<DateTime<Utc>>,
    /// This reader's last recorded visit.
    pub last_opened: Option<DateTime<Utc>>,
    /// The public fields of the last human document editor.
    pub activity_by_brief: Option<Json>,
    /// Whether a readable document changed since this reader last saw it.
    pub document_unread: Option<bool>,
    /// Thread or environment state.
    pub state: Option<String>,
    /// Known stored bytes.
    pub size_bytes: Option<i64>,
    /// Recorded sizing fields passed to the existing environment needs/limits resolver.
    pub takes: Option<Json>,
    /// Tag definitions whose place can be read.
    pub tags: Json,
    /// Whether any readable child, including a path step, exists.
    pub has_children: bool,
    /// Its access engine decision.
    pub grant: Json,
    /// The key actually used for this row.
    pub sort_key: Option<String>,
    /// Only the ordinary public-reference fields of the first owner.
    pub owner_brief: Option<Json>,
    /// Only the ordinary public-reference fields of the maker.
    pub creator_brief: Option<Json>,
    /// Sharing and sizing settings on this path, hydrated into the existing settings resolver.
    pub sharing_rows: Json,
    /// A root org's address tells repeated root names apart.
    pub root_address: Option<String>,
    /// A node chat's ordinary-thread path step, authorized in SQL without seeding every thread.
    pub path_tail: Option<Json>,
}

impl Row {
    /// Where another page continues after this row.
    #[must_use]
    pub fn cursor(&self) -> Cursor {
        Cursor {
            relevance: self.relevance,
            score_at: Some(self.score_at),
            visit_score: self.visit_score,
            search_activity: self.search_activity,
            key: self.sort_key.clone(),
            kind: self.kind.clone(),
            id: self.id.clone(),
        }
    }
}

/// Resource predicates applied by the common statement before keyset paging.
#[derive(Debug, Clone, Copy, Default)]
pub struct ResourceFilter<'a> {
    /// The resource's state.
    pub state: Option<&'a str>,
    /// An agent's harness.
    pub harness: Option<&'a str>,
    /// Template kind or environment origin.
    pub kind: Option<&'a str>,
    /// Apply the existing provider-scope policy projected into the grant.
    pub provider_scope: bool,
    /// The resource's home must be at or below this node.
    pub home: Option<Id>,
    /// Resource-list content rows only; name-only entries have no rich view.
    pub details: bool,
}

/// The reader's request and its access engine projection, all in the same snapshot.
pub struct Query<'a> {
    /// Flat listing and Navigation's visibility choices.
    pub navigation: NavigationFilter<'a>,
    /// The resource list's filters, applied before keyset paging.
    pub resource: ResourceFilter<'a>,
    /// Match the access engine's native-only View marker, retaining real paths before paging.
    pub admin_access: Option<bool>,
    /// Keep ordinary participation, before sorting and paging.
    pub participating: bool,
    /// The reader.
    pub user: Id,
    /// Their current effective groups.
    pub groups: &'a [Id],
    /// The roots in which ownership counts for this reader.
    pub member_roots: &'a [Id],
    /// Access decisions, including inherited thread decisions, prepared by the access engine.
    pub grants: &'a Json,
    /// A parent UUID or one of the two group names; none is the top.
    pub parent: Option<&'a str>,
    /// The whole system was explicitly requested by its administrator.
    pub system: bool,
    /// The shared literal query; resource lists also match visible owner/home labels. Empty lists the level.
    pub text: &'a str,
    /// The kinds, or an empty array for all kinds.
    pub kinds: &'a [String],
    /// First owner filter.
    pub owner: Option<Id>,
    /// Inclusive creation bounds.
    pub created_from: Option<DateTime<Utc>>,
    /// Inclusive creation bounds.
    pub created_to: Option<DateTime<Utc>>,
    /// Inclusive own/rolled-up activity bound.
    pub active_from: Option<DateTime<Utc>>,
    /// A column from the contract's closed catalogue.
    pub sort: &'a str,
    /// Descending rather than ascending.
    pub descending: bool,
    /// The last row of the previous page.
    pub after: Option<&'a Cursor>,
    /// One more than the requested size, to detect another page.
    pub limit: i64,
    /// The sampled time used by the access engine too.
    pub at: DateTime<Utc>,
}

/// Navigation predicates, applied before ordering and paging rather than to a browser slice.
#[derive(Debug, Clone, Copy)]
pub struct NavigationFilter<'a> {
    /// Opened or active selects a flat listing; none retains ordinary tree/search behavior.
    pub recent: Option<&'a str>,
    /// Keep only navigation kinds and respect the Threads switch for descendants.
    pub enabled: bool,
    /// Whether archived ancestors are included.
    pub show_archived: bool,
    /// Whether quiet done topics are omitted.
    pub hide_done: bool,
    /// The current thing whose path stays visible when done.
    pub keep: Option<Id>,
}

impl Default for NavigationFilter<'_> {
    fn default() -> Self {
        Self {
            recent: None,
            enabled: false,
            show_archived: true,
            hide_done: false,
            keep: None,
        }
    }
}

/// Reads a level in one statement; visibility, filters, ordering and keyset paging all precede LIMIT.
pub async fn level(conn: &mut Conn, ask: &Query<'_>) -> Result<Vec<Row>, DbError> {
    let words = super::search::Query::parse(ask.text);
    // Each level is planned with its own parent and filters because branch sizes differ widely.
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(statement()))
        .persistent(false)
        .bind(ask.user)
        .bind(ask.groups)
        .bind(ask.member_roots)
        .bind(ask.grants)
        .bind(ask.parent)
        .bind(ask.system)
        .bind(words.like_patterns())
        .bind(ask.kinds)
        .bind(ask.owner)
        .bind(ask.created_from)
        .bind(ask.created_to)
        .bind(ask.sort)
        .bind(ask.descending)
        .bind(ask.after.is_some())
        .bind(ask.after.and_then(|cursor| cursor.key.as_deref()))
        .bind(ask.after.map(|cursor| cursor.kind.as_str()))
        .bind(ask.after.map(|cursor| cursor.id.as_str()))
        .bind(ask.limit)
        .bind(ask.at)
        .bind(words.terms().is_empty() && ask.navigation.recent.is_none())
        .bind(
            !ask.kinds.is_empty()
                || ask.owner.is_some()
                || ask.created_from.is_some()
                || ask.created_to.is_some()
                || ask.active_from.is_some(),
        )
        .bind(ask.participating)
        .bind(ask.admin_access)
        .bind(words.phrase())
        .bind(ask.after.map_or(0, |cursor| cursor.relevance))
        .bind(
            ask.after
                .and_then(|cursor| cursor.score_at)
                .unwrap_or(ask.at),
        )
        .bind(ask.after.map_or(0.0, |cursor| cursor.visit_score))
        .bind(ask.after.and_then(|cursor| cursor.search_activity))
        .bind(ask.active_from)
        .bind(ask.resource.state)
        .bind(ask.resource.harness)
        .bind(ask.resource.kind)
        .bind(ask.resource.provider_scope)
        .bind(ask.resource.home)
        .bind(ask.resource.details)
        .bind(ask.navigation.recent)
        .bind(ask.navigation.enabled)
        .bind(ask.navigation.show_archived)
        .bind(ask.navigation.hide_done)
        .bind(ask.navigation.keep)
        .fetch_all(conn)
        .await?)
}

/// The listing template; shared literal matching and ranking are inserted from source constants.
pub const SQL: &str = include_str!("everything.sql");

/// Discoverable resource counts use the inventory's exact native grants, without rich row readers.
pub async fn resource_counts(
    conn: &mut Conn,
    grants: &Json,
) -> Result<Vec<(String, i64)>, DbError> {
    Ok(sqlx::query_as(include_str!("resource_counts.sql"))
        .bind(grants)
        .fetch_all(conn)
        .await?)
}

/// The exact node/thread/chat-area projection shared by inventory and content search.
/// Arguments are only source-owned SQL parameters, never caller text.
#[must_use]
pub fn node_visibility_sql(
    user: &str,
    groups: &str,
    member_roots: &str,
    grants: &str,
    at: &str,
) -> String {
    include_str!("node_visibility.sql")
        .replace("__USER__", user)
        .replace("__GROUPS__", groups)
        .replace("__ROOTS__", member_roots)
        .replace("__GRANTS__", grants)
        .replace("__AT__", at)
}

fn statement() -> String {
    const GRANTS: &str = "(SELECT grants FROM reader_projection)";
    // Resource lists also search their visible owner/home labels. Normal hierarchy search remains name-only.
    const NAME: &str = "CASE WHEN $35::boolean THEN concat_ws(' ',i.name,
      (SELECT concat_ws(' ',u.display_name,u.username) FROM users u WHERE u.id=i.owner_id),
      (SELECT string_agg(__GRANTS__->'resource_names'->>n.id::text,' ') FROM unnest(i.path) n(id))) ELSE i.name END";
    SQL.replace(
        "__NODE_VISIBILITY__",
        &node_visibility_sql("$1", "$2", "$3", GRANTS, "$19"),
    )
    .replace("__MATCH_NAME__", &super::search::matching_sql(NAME, "$7"))
    .replace(
        "__CHAT_FOLLOW__",
        &super::chats::node_follow_level_sql("cs", "prefs"),
    )
    .replace(
        "__RANK_NAME__",
        &super::search::ranking_sql("i.name", "$24", "$7"),
    )
    .replace(
        "__VISIT_SCORE__",
        &super::hierarchy::visit_score_sql("r.visit_count", "r.visited_at", "$26"),
    )
    .replace("__GRANTS__", GRANTS)
}

/// The plan of the production statement, for the large-inventory regression check.
pub async fn explain(conn: &mut Conn, ask: &Query<'_>) -> Result<Json, DbError> {
    let words = super::search::Query::parse(ask.text);
    Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) {}",
        statement()
    )))
    .persistent(false)
    .bind(ask.user)
    .bind(ask.groups)
    .bind(ask.member_roots)
    .bind(ask.grants)
    .bind(ask.parent)
    .bind(ask.system)
    .bind(words.like_patterns())
    .bind(ask.kinds)
    .bind(ask.owner)
    .bind(ask.created_from)
    .bind(ask.created_to)
    .bind(ask.sort)
    .bind(ask.descending)
    .bind(ask.after.is_some())
    .bind(ask.after.and_then(|cursor| cursor.key.as_deref()))
    .bind(ask.after.map(|cursor| cursor.kind.as_str()))
    .bind(ask.after.map(|cursor| cursor.id.as_str()))
    .bind(ask.limit)
    .bind(ask.at)
    .bind(words.terms().is_empty() && ask.navigation.recent.is_none())
    .bind(
        !ask.kinds.is_empty()
            || ask.owner.is_some()
            || ask.created_from.is_some()
            || ask.created_to.is_some()
            || ask.active_from.is_some(),
    )
    .bind(ask.participating)
    .bind(ask.admin_access)
    .bind(words.phrase())
    .bind(ask.after.map_or(0, |cursor| cursor.relevance))
    .bind(
        ask.after
            .and_then(|cursor| cursor.score_at)
            .unwrap_or(ask.at),
    )
    .bind(ask.after.map_or(0.0, |cursor| cursor.visit_score))
    .bind(ask.after.and_then(|cursor| cursor.search_activity))
    .bind(ask.active_from)
    .bind(ask.resource.state)
    .bind(ask.resource.harness)
    .bind(ask.resource.kind)
    .bind(ask.resource.provider_scope)
    .bind(ask.resource.home)
    .bind(ask.resource.details)
    .bind(ask.navigation.recent)
    .bind(ask.navigation.enabled)
    .bind(ask.navigation.show_archived)
    .bind(ask.navigation.hide_done)
    .bind(ask.navigation.keep)
    .fetch_one(conn)
    .await?)
}
