//! Members with their domain, roles held in places, permission rows and entries with the Owners row, suspensions in an org, groups, access requests and the authorization epoch.
//!
//! This part is what the access engine reads and writes: what is known of a
//! person (their groups, where they are members, where they are suspended),
//! the roles given in places, the rows and entries of the pages that are
//! stored, and the epoch that every change of these moves. Kinds, holders and
//! roles come back as the words they are stored with; the engine reads them.
//!
//! Nothing here decides anything. A row that is not stored is not returned,
//! and a removed entry is returned by nothing but what asks for the record.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Id, Json};

/// The nil identifier, with which the pages of the system are kept: the
/// system is no row of any table.
pub const SYSTEM_ID: Id = Id::nil();

pub mod kept;
pub mod participation;

/// What is known of a person apart from what pages name them for.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct PrincipalRow {
    /// `active` or `suspended`: the latter is a suspension altogether.
    pub status: String,
    /// Whether they hold Admin at the system.
    pub is_system_admin: bool,
    /// The groups they are in.
    pub groups: Vec<Id>,
    /// The orgs they are suspended in.
    pub suspended_in: Vec<Id>,
}

/// The facts about the user `user`, or nothing where there is no such user.
pub async fn principal(conn: &mut Conn, user: Id) -> Result<Option<PrincipalRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT u.status, u.is_system_admin,
                COALESCE((SELECT array_agg(g.group_id) FROM group_members g WHERE g.user_id = u.id), '{}') AS groups,
                COALESCE((SELECT array_agg(s.org_id) FROM org_suspensions s WHERE s.user_id = u.id), '{}') AS suspended_in
           FROM users u WHERE u.id = $1",
    )
    .bind(user)
    .fetch_optional(conn)
    .await?)
}

/// Groups reached through the current holder graph, including nested groups and roles at places.
pub async fn effective_groups(
    conn: &mut Conn,
    user: Id,
    at: DateTime<Utc>,
    system: bool,
) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM access_holders($1, $2, $3) WHERE kind = 'group' ORDER BY id",
    )
    .bind(user)
    .bind(at)
    .bind(system)
    .fetch_all(conn)
    .await?)
}

/// A person's groups and page-only viewer identities from one current fixed-point expansion.
pub async fn effective_holder_sets(
    conn: &mut Conn,
    user: Id,
    at: DateTime<Utc>,
    system: bool,
) -> Result<Vec<(String, Id)>, DbError> {
    Ok(sqlx::query_as("SELECT kind, id FROM access_holders($1, $2, $3) WHERE kind IN ('group','visible') ORDER BY kind, id")
        .bind(user).bind(at).bind(system).fetch_all(conn).await?)
}

/// A place's identifier and present address, before the caller's visibility cut.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct HolderPlaceRow {
    /// The place.
    pub id: Id,
    /// Its tree.
    pub root: Id,
    /// Org, project or topic.
    pub kind: String,
    /// Identifiers from the root to this place.
    pub path: Vec<Id>,
    /// Its own name.
    pub leaf_name: String,
    /// Names from the tree's root to this place.
    pub name: String,
    /// Short names of orgs, and names of projects/topics, from the root.
    pub address: String,
    /// The nearest org, which keeps its groups.
    pub org: Id,
}

/// Places whose current names may match a locator; permissions are cut before anything is returned.
pub async fn holder_places(conn: &mut Conn) -> Result<Vec<HolderPlaceRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT n.id, n.org_id AS root, n.kind, n.path, n.name AS leaf_name,
                string_agg(p.name, ' > ' ORDER BY a.position) AS name,
                string_agg(COALESCE(o.slug, lower(replace(p.name, ' ', '-'))), '.' ORDER BY a.position) AS address,
                (array_agg(p.id ORDER BY a.position DESC) FILTER (WHERE p.kind = 'org'))[1] AS org
           FROM nodes n CROSS JOIN LATERAL unnest(n.path) WITH ORDINALITY a(id, position)
           JOIN nodes p ON p.id = a.id LEFT JOIN orgs o ON o.node_id = p.id
          WHERE n.kind IN ('org','project','topic') AND n.deleted_at IS NULL
          GROUP BY n.id ORDER BY n.org_id, n.id")
        .fetch_all(conn).await?)
}

/// Current person addresses with their domains; the API cuts domains before exposing their names.
pub async fn holder_user_addresses(
    conn: &mut Conn,
    users: &[Id],
) -> Result<Vec<(Id, Id, Id, String)>, DbError> {
    Ok(sqlx::query_as("SELECT m.user_id, m.org_id, m.domain_id,
        string_agg(o.slug, '.' ORDER BY a.position) || '.' || u.username AS address
        FROM org_members m JOIN users u ON u.id = m.user_id JOIN nodes d ON d.id = m.domain_id
        CROSS JOIN LATERAL unnest(d.path) WITH ORDINALITY a(id, position) JOIN orgs o ON o.node_id = a.id
        WHERE m.user_id = ANY($1) AND d.deleted_at IS NULL GROUP BY m.user_id, m.org_id, m.domain_id, u.username")
        .bind(users).fetch_all(conn).await?)
}

/// A person's membership of one tree of orgs.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct MembershipRow {
    /// The root org of the tree.
    pub org_id: Id,
    /// Their domain.
    pub domain_id: Id,
    /// The orgs from the root org down to the domain, both included: every
    /// org they are a member of in this tree.
    pub domain_path: Vec<Id>,
}

/// Every tree of orgs `user` is a member of.
pub async fn memberships(conn: &mut Conn, user: Id) -> Result<Vec<MembershipRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT m.org_id, m.domain_id, d.path AS domain_path
           FROM org_members m JOIN nodes d ON d.id = m.domain_id
          WHERE m.user_id = $1 AND d.deleted_at IS NULL
            AND NOT EXISTS (SELECT 1 FROM nodes gone WHERE gone.id = ANY(d.path) AND gone.deleted_at IS NOT NULL)
          ORDER BY m.org_id",
    )
    .bind(user)
    .fetch_all(conn)
    .await?)
}

/// A role given in a place.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct RoleRow {
    /// The giving.
    pub id: Id,
    /// The place.
    pub node_id: Id,
    /// `user`, `group` or `members`.
    pub subject_kind: String,
    /// The user, the group, or the org whose members are meant.
    pub subject_id: Id,
    /// The role held at the subject place, for `role`.
    pub subject_role: Option<String>,
    /// `viewer`, `interactor`, `developer` or `admin`.
    pub role: String,
    /// When it ends by itself.
    pub expires_at: Option<DateTime<Utc>>,
    /// Why it was given.
    pub note: String,
    /// Who gave it.
    pub created_by: Id,
    /// When it was given.
    pub created_at: DateTime<Utc>,
    /// When it was taken away.
    pub removed_at: Option<DateTime<Utc>>,
    /// How many times it was changed.
    pub revision: i64,
}

macro_rules! role_columns {
    () => {
        "id, node_id, subject_kind, subject_id, subject_role, role, expires_at, note, created_by, created_at, removed_at, revision"
    };
}

/// The roles given in the places `places` that reach a person: given to
/// them, to a group of theirs (`groups`) or to an org they are a member of
/// (`member_of`), and neither taken away nor ended at `now`.
pub async fn roles_reaching(
    conn: &mut Conn,
    places: &[Id],
    user: Id,
    groups: &[Id],
    member_of: &[Id],
    now: DateTime<Utc>,
) -> Result<Vec<RoleRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {} FROM node_roles r
          WHERE node_id = ANY($1) AND removed_at IS NULL AND (expires_at IS NULL OR expires_at > $5)
            AND {} ORDER BY node_id, created_at, id",
        role_columns!(),
        role_subject_sql("$2", "$3", "$4", "$5"),
    )))
    .bind(places)
    .bind(user)
    .bind(groups)
    .bind(member_of)
    .bind(now)
    .fetch_all(conn)
    .await?)
}

// Source-controlled aliases and placeholders only. Participation reads the
// same ordinary role-subject match as the permission frame, without native reach.
fn role_subject_sql(user: &str, groups: &str, members: &str, at: &str) -> String {
    format!("((r.subject_kind = 'user' AND r.subject_id = {user})
      OR (r.subject_kind = 'group' AND r.subject_id = ANY({groups}))
      OR (r.subject_kind = 'members' AND r.subject_id = ANY({members}))
      OR (r.subject_kind = 'role' AND EXISTS (
        SELECT 1 FROM access_holders({user}, {at}, false) h
         WHERE h.kind IN ('role','visible') AND h.id = r.subject_id
           AND (r.subject_role <> 'viewer' OR h.kind = 'visible')
           AND array_position(ARRAY['viewer','interactor','developer','admin'], h.role)
               >= array_position(ARRAY['viewer','interactor','developer','admin'], r.subject_role))))")
}

/// Every role that counts in the place `place` at `now`, to whomever it is
/// given: the roles of a place as its page lists them.
pub async fn roles_in(
    conn: &mut Conn,
    place: Id,
    now: DateTime<Utc>,
) -> Result<Vec<RoleRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        role_columns!(),
        " FROM node_roles
          WHERE node_id = $1 AND removed_at IS NULL AND (expires_at IS NULL OR expires_at > $2)
          ORDER BY created_at, id"
    ))
    .bind(place)
    .bind(now)
    .fetch_all(conn)
    .await?)
}

/// Every role that counts in any of the places `places` at `now`, to
/// whomever it is given, place by place in the order they were given.
pub async fn roles_in_places(
    conn: &mut Conn,
    places: &[Id],
    now: DateTime<Utc>,
) -> Result<Vec<RoleRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        role_columns!(),
        " FROM node_roles
          WHERE node_id = ANY($1) AND removed_at IS NULL AND (expires_at IS NULL OR expires_at > $2)
          ORDER BY node_id, created_at, id"
    ))
    .bind(places)
    .bind(now)
    .fetch_all(conn)
    .await?)
}

/// One entry of a row, as it is stored.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct EntryRow {
    /// The entry.
    pub id: Id,
    /// `system`, `node`, `provider`, and so on.
    pub object_kind: String,
    /// The object, the nil identifier for the system.
    pub object_id: Id,
    /// `live` or `defaults`.
    pub page: String,
    /// `any`, `managed` or `connected`.
    pub profile: String,
    /// The permission, by its key.
    pub permission: String,
    /// `role`, `group`, `user`, `node`, `agents` or `everyone`.
    pub holder_kind: String,
    /// The group, the person or the place; for a role, where it is held
    /// when that is not the object's own place.
    pub holder_id: Option<Id>,
    /// For a role, the role.
    pub holder_role: Option<String>,
    /// What the holder may do with the permission.
    pub options: Json,
    /// When it ends by itself.
    pub expires_at: Option<DateTime<Utc>>,
    /// Why it was written.
    pub note: String,
    /// `page`, `creation`, `request` or `seed`.
    pub source: String,
    /// Who wrote it; nobody for what was shipped.
    pub created_by: Option<Id>,
    /// When it was written.
    pub created_at: DateTime<Utc>,
    /// When it was removed.
    pub removed_at: Option<DateTime<Utc>>,
    /// Who removed it.
    pub removed_by: Option<Id>,
    /// How many times it was changed.
    pub revision: i64,
}

macro_rules! entry_columns {
    () => {
        "e.id, e.object_kind, e.object_id, e.page, e.profile, e.permission, e.holder_kind, e.holder_id, e.holder_role,
         e.options, e.expires_at, e.note, e.source, e.created_by, e.created_at, e.removed_at, e.removed_by, e.revision"
    };
}

/// A page of an object: where rows and entries are kept.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PageKey {
    /// `system`, `node`, `document`, and so on.
    pub object_kind: String,
    /// The object; the nil identifier for the system.
    pub object_id: Id,
    /// `live` or `defaults`.
    pub page: String,
}

impl PageKey {
    /// The page `page` of the object `object_id` of kind `object_kind`.
    #[must_use]
    pub fn new(object_kind: &str, object_id: Id, page: &str) -> Self {
        Self {
            object_kind: object_kind.to_owned(),
            object_id,
            page: page.to_owned(),
        }
    }
}

fn split(pages: &[PageKey]) -> (Vec<String>, Vec<Id>, Vec<String>) {
    (
        pages.iter().map(|page| page.object_kind.clone()).collect(),
        pages.iter().map(|page| page.object_id).collect(),
        pages.iter().map(|page| page.page.clone()).collect(),
    )
}

/// The entries of the Owners rows of the objects `objects` (`object_kind`,
/// `object_id`) that were not removed: people and groups, in the order they
/// were named.
pub async fn owner_entries(conn: &mut Conn, objects: &[PageKey]) -> Result<Vec<EntryRow>, DbError> {
    let (kinds, ids, _) = split(objects);
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        entry_columns!(),
        " FROM permission_entries e
           JOIN unnest($1::text[], $2::uuid[]) AS w(object_kind, object_id)
             ON e.object_kind = w.object_kind AND e.object_id = w.object_id
          WHERE e.page = 'live' AND e.permission = 'access.own' AND e.removed_at IS NULL
          ORDER BY e.created_at, e.id"
    ))
    .bind(kinds)
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

/// The entries that were not removed in the rows of `permissions` on the
/// pages `pages`, in the order they were written.
pub async fn entries(
    conn: &mut Conn,
    pages: &[PageKey],
    permissions: &[String],
) -> Result<Vec<EntryRow>, DbError> {
    let (kinds, ids, which) = split(pages);
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        entry_columns!(),
        " FROM permission_entries e
           JOIN unnest($1::text[], $2::uuid[], $3::text[]) AS w(object_kind, object_id, page)
             ON e.object_kind = w.object_kind AND e.object_id = w.object_id AND e.page = w.page
          WHERE e.removed_at IS NULL AND e.permission = ANY($4)
          ORDER BY e.created_at, e.id"
    ))
    .bind(kinds)
    .bind(ids)
    .bind(which)
    .bind(permissions)
    .fetch_all(conn)
    .await?)
}

/// A row that is stored: one that somebody changed.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct StoredRow {
    /// `system`, `node`, and so on.
    pub object_kind: String,
    /// The object; the nil identifier for the system.
    pub object_id: Id,
    /// `live` or `defaults`.
    pub page: String,
    /// `any`, `managed` or `connected`.
    pub profile: String,
    /// The permission, by its key.
    pub permission: String,
    /// Whether it follows the default of the place above.
    pub inherit: bool,
    /// On a defaults page: whether the pages inside follow it exactly.
    pub locked: bool,
    /// Who changed it last; nobody for what was shipped.
    pub updated_by: Option<Id>,
    /// When it was changed last.
    pub updated_at: DateTime<Utc>,
    /// How many times it was changed.
    pub revision: i64,
}

/// The stored rows of `permissions` on the pages `pages`.
pub async fn stored_rows(
    conn: &mut Conn,
    pages: &[PageKey],
    permissions: &[String],
) -> Result<Vec<StoredRow>, DbError> {
    let (kinds, ids, which) = split(pages);
    Ok(sqlx::query_as(
        "SELECT r.object_kind, r.object_id, r.page, r.profile, r.permission, r.inherit, r.locked,
                r.updated_by, r.updated_at, r.revision
           FROM permission_rows r
           JOIN unnest($1::text[], $2::uuid[], $3::text[]) AS w(object_kind, object_id, page)
             ON r.object_kind = w.object_kind AND r.object_id = w.object_id AND r.page = w.page
          WHERE r.permission = ANY($4)",
    )
    .bind(kinds)
    .bind(ids)
    .bind(which)
    .bind(permissions)
    .fetch_all(conn)
    .await?)
}

/// The stored rows of the permission `permission` on the pages, own and
/// defaults, of the nodes `nodes`.
pub async fn stored_rows_of_nodes(
    conn: &mut Conn,
    nodes: &[Id],
    permissions: &[String],
) -> Result<Vec<StoredRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT object_kind, object_id, page, profile, permission, inherit, locked, updated_by, updated_at, revision
           FROM permission_rows
          WHERE object_kind = 'node' AND object_id = ANY($1) AND permission = ANY($2)",
    )
    .bind(nodes)
    .bind(permissions)
    .fetch_all(conn)
    .await?)
}

/// Every stored row of the page `page`, whatever its permission.
pub async fn stored_rows_of(conn: &mut Conn, page: &PageKey) -> Result<Vec<StoredRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT object_kind, object_id, page, profile, permission, inherit, locked, updated_by, updated_at, revision
           FROM permission_rows WHERE object_kind = $1 AND object_id = $2 AND page = $3",
    )
    .bind(&page.object_kind)
    .bind(page.object_id)
    .bind(&page.page)
    .fetch_all(conn)
    .await?)
}

/// Every entry that was not removed on the page `page`, the Owners row
/// among them.
pub async fn entries_of(conn: &mut Conn, page: &PageKey) -> Result<Vec<EntryRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        entry_columns!(),
        " FROM permission_entries e
          WHERE e.object_kind = $1 AND e.object_id = $2 AND e.page = $3 AND e.removed_at IS NULL
          ORDER BY e.created_at, e.id"
    ))
    .bind(&page.object_kind)
    .bind(page.object_id)
    .bind(&page.page)
    .fetch_all(conn)
    .await?)
}

/// The permission whose stored row counts the changes of a page. Every
/// kind of thing has it and no page leaves it out, so it can say how many
/// times a page was changed without a counter of its own.
pub const PAGE_COUNTER: &str = "access.manage";

/// How many times the page was changed: the revision of the row that
/// counts them, or nothing before the first change.
pub async fn page_revision(conn: &mut Conn, page: &PageKey) -> Result<i64, DbError> {
    let revision: Option<i64> = sqlx::query_scalar(
        "SELECT revision FROM permission_rows
          WHERE object_kind = $1 AND object_id = $2 AND page = $3 AND profile = 'any' AND permission = $4",
    )
    .bind(&page.object_kind)
    .bind(page.object_id)
    .bind(&page.page)
    .bind(PAGE_COUNTER)
    .fetch_optional(conn)
    .await?;
    Ok(revision.unwrap_or(0))
}

/// Counts one change of the page, by `by` at `at`, in the row that counts
/// them; the row is made, following and naming nobody, when it is not
/// there. Returns the count after.
pub async fn count_page_change(
    conn: &mut Conn,
    page: &PageKey,
    by: Option<Id>,
    at: DateTime<Utc>,
) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "INSERT INTO permission_rows (object_kind, object_id, page, profile, permission, updated_by, updated_at)
         VALUES ($1, $2, $3, 'any', $4, $5, $6)
         ON CONFLICT (object_kind, object_id, page, profile, permission)
         DO UPDATE SET revision = permission_rows.revision + 1, updated_by = EXCLUDED.updated_by,
                       updated_at = EXCLUDED.updated_at
         RETURNING revision",
    )
    .bind(&page.object_kind)
    .bind(page.object_id)
    .bind(&page.page)
    .bind(PAGE_COUNTER)
    .bind(by)
    .bind(at)
    .fetch_one(conn)
    .await?)
}

/// A row as it is to be stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowWrite<'a> {
    /// The page it is on.
    pub page: &'a PageKey,
    /// `any`, `managed` or `connected`.
    pub profile: &'a str,
    /// The permission, by its key.
    pub permission: &'a str,
    /// Whether it follows the default of the place above.
    pub inherit: bool,
    /// On a defaults page: whether the pages inside follow it exactly.
    pub locked: bool,
}

/// Writes the row as it is to be, making it when it is not stored and
/// counting the change when it is.
pub async fn write_row(
    conn: &mut Conn,
    row: &RowWrite<'_>,
    by: Option<Id>,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO permission_rows (object_kind, object_id, page, profile, permission, inherit, locked, updated_by, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
         ON CONFLICT (object_kind, object_id, page, profile, permission)
         DO UPDATE SET inherit = EXCLUDED.inherit, locked = EXCLUDED.locked, updated_by = EXCLUDED.updated_by,
                       updated_at = EXCLUDED.updated_at, revision = permission_rows.revision + 1",
    )
    .bind(&row.page.object_kind)
    .bind(row.page.object_id)
    .bind(&row.page.page)
    .bind(row.profile)
    .bind(row.permission)
    .bind(row.inherit)
    .bind(row.locked)
    .bind(by)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

/// An entry to write.
#[derive(Debug, Clone, PartialEq)]
pub struct NewEntry<'a> {
    /// Its identifier.
    pub id: Id,
    /// The page it stands on.
    pub page: &'a PageKey,
    /// `any`, `managed` or `connected`.
    pub profile: &'a str,
    /// The permission, by its key.
    pub permission: &'a str,
    /// `role`, `group`, `user`, `node`, `agents` or `everyone`.
    pub holder_kind: &'a str,
    /// The group, the person or the place; for a role, where it is held
    /// when that is not the object's own place.
    pub holder_id: Option<Id>,
    /// For a role, the role.
    pub holder_role: Option<&'a str>,
    /// What the holder may do with the permission.
    pub options: &'a Json,
    /// When it ends by itself.
    pub expires_at: Option<DateTime<Utc>>,
    /// Why it is written.
    pub note: &'a str,
    /// `page`, `creation`, `request` or `seed`.
    pub source: &'a str,
    /// Who writes it; nobody for what is shipped.
    pub created_by: Option<Id>,
    /// When.
    pub at: DateTime<Utc>,
}

/// Writes an entry. A holder who is named in the row already is a unique
/// value taken.
pub async fn insert_entry(conn: &mut Conn, entry: &NewEntry<'_>) -> Result<(), DbError> {
    insert_entry_for_share(conn, entry, None).await
}

/// Writes the independently retained reach of an agent share; ordinary entries use None.
pub async fn insert_entry_for_share(
    conn: &mut Conn,
    entry: &NewEntry<'_>,
    share_inside: Option<bool>,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO permission_entries (id, object_kind, object_id, page, profile, permission, holder_kind, holder_id,
                                         holder_role, options, expires_at, note, source, created_by, created_at, share_inside)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)",
    )
    .bind(entry.id)
    .bind(&entry.page.object_kind)
    .bind(entry.page.object_id)
    .bind(&entry.page.page)
    .bind(entry.profile)
    .bind(entry.permission)
    .bind(entry.holder_kind)
    .bind(entry.holder_id)
    .bind(entry.holder_role)
    .bind(entry.options)
    .bind(entry.expires_at)
    .bind(entry.note)
    .bind(entry.source)
    .bind(entry.created_by)
    .bind(entry.at)
    .bind(share_inside)
    .execute(conn)
    .await?;
    Ok(())
}

/// The entry of the row `permission` / `profile` of `page` that names the
/// holder given, where it was not removed: it may have ended.
pub async fn find_entry(
    conn: &mut Conn,
    page: &PageKey,
    profile: &str,
    permission: &str,
    holder_kind: &str,
    holder_id: Option<Id>,
    holder_role: Option<&str>,
) -> Result<Option<EntryRow>, DbError> {
    find_entry_for_share(
        conn,
        page,
        profile,
        permission,
        holder_kind,
        holder_id,
        holder_role,
        None,
    )
    .await
}

/// Looks up only this retained share reach, so a covering reach cannot replace it.
#[allow(
    clippy::too_many_arguments,
    reason = "a permission-row key and the share reach that distinguishes its two entries"
)]
pub async fn find_entry_for_share(
    conn: &mut Conn,
    page: &PageKey,
    profile: &str,
    permission: &str,
    holder_kind: &str,
    holder_id: Option<Id>,
    holder_role: Option<&str>,
    share_inside: Option<bool>,
) -> Result<Option<EntryRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        entry_columns!(),
        " FROM permission_entries e
          WHERE e.object_kind = $1 AND e.object_id = $2 AND e.page = $3 AND e.profile = $4 AND e.permission = $5
            AND e.holder_kind = $6 AND e.holder_id IS NOT DISTINCT FROM $7 AND e.holder_role IS NOT DISTINCT FROM $8
            AND e.share_inside IS NOT DISTINCT FROM $9
            AND e.removed_at IS NULL"
    ))
    .bind(&page.object_kind)
    .bind(page.object_id)
    .bind(&page.page)
    .bind(profile)
    .bind(permission)
    .bind(holder_kind)
    .bind(holder_id)
    .bind(holder_role)
    .bind(share_inside)
    .fetch_optional(conn)
    .await?)
}

/// Changes an entry that was not removed to what it is now to be, and
/// counts the change. The answer says whether there was such an entry.
pub async fn rewrite_entry(
    conn: &mut Conn,
    id: Id,
    options: &Json,
    expires_at: Option<DateTime<Utc>>,
    note: &str,
) -> Result<bool, DbError> {
    let done = sqlx::query(
        "UPDATE permission_entries SET options = $2, expires_at = $3, note = $4, revision = revision + 1
          WHERE id = $1 AND removed_at IS NULL",
    )
    .bind(id)
    .bind(options)
    .bind(expires_at)
    .bind(note)
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Removes an entry: it is kept for the record and counts for nothing from
/// then on. The answer says whether there was such an entry.
pub async fn remove_entry(
    conn: &mut Conn,
    id: Id,
    by: Id,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    let done = sqlx::query(
        "UPDATE permission_entries SET removed_at = $3, removed_by = $2, revision = revision + 1
          WHERE id = $1 AND removed_at IS NULL",
    )
    .bind(id)
    .bind(by)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Puts the page back to following the defaults throughout: every stored
/// row becomes the row that follows and is not locked, and every entry but
/// the Owners row's is removed. Returns the entries removed.
pub async fn reset_page(
    conn: &mut Conn,
    page: &PageKey,
    by: Id,
    at: DateTime<Utc>,
) -> Result<Vec<Id>, DbError> {
    sqlx::query(
        "UPDATE permission_rows SET inherit = true, locked = false, updated_by = $4, updated_at = $5,
                                    revision = revision + 1
          WHERE object_kind = $1 AND object_id = $2 AND page = $3 AND (NOT inherit OR locked)",
    )
    .bind(&page.object_kind)
    .bind(page.object_id)
    .bind(&page.page)
    .bind(by)
    .bind(at)
    .execute(&mut *conn)
    .await?;
    Ok(sqlx::query_scalar(
        "UPDATE permission_entries SET removed_at = $5, removed_by = $4, revision = revision + 1
          WHERE object_kind = $1 AND object_id = $2 AND page = $3 AND removed_at IS NULL AND permission <> 'access.own'
      RETURNING id",
    )
    .bind(&page.object_kind)
    .bind(page.object_id)
    .bind(&page.page)
    .bind(by)
    .bind(at)
    .fetch_all(conn)
    .await?)
}

/// What the authorization epochs of every scope come to together (the system's, a person's standing
/// and each root org's): it moves with every change of who may do what, which may take away what a
/// reader was given, and not with what is made or only given (the epoch below moves with those too).
pub async fn authority_epoch(conn: &mut Conn) -> Result<i64, DbError> {
    Ok(
        sqlx::query_scalar("SELECT COALESCE(sum(epoch), 0)::bigint FROM authz_epochs")
            .fetch_one(conn)
            .await?,
    )
}

/// The authorization epoch: it is moved by every change to who may do what,
/// and what was decided under an older one may be decided again.
pub async fn epoch(conn: &mut Conn) -> Result<i64, DbError> {
    Ok(
        sqlx::query_scalar("SELECT COALESCE((SELECT epoch FROM authz_state WHERE id), 0)")
            .fetch_one(conn)
            .await?,
    )
}

/// Moves the epoch on, and returns it. Called once by a change, in the
/// transaction that makes it. The move, and its announcement to every
/// connection that holds something open on an older decision, happen at that
/// transaction's commit (`orch_commit_work`), whichever writer made it:
/// the counter's row is locked for a commit, never for a request. What is
/// returned is the epoch the change takes unless another one commits first.
pub async fn bump_epoch(conn: &mut Conn) -> Result<i64, DbError> {
    // A change of existing authority also moves the authorization epochs of its scope (its request's root org,
    // or the system's) at commit, where every write that decided under them checks them. Births only add a
    // fresh object's authority and move the cache alone. Personal credentials are checked from their locked
    // rows; they change no cached permission answer and need no global counter write.
    let (epoch, personal): (i64, bool) = sqlx::query_as(
        "SELECT CASE WHEN m.value = 'personal' THEN COALESCE((SELECT epoch FROM authz_state WHERE id), 0)
                     ELSE orch_authz_change(m.value <> 'birth') END,
                m.value = 'personal'
           FROM (SELECT COALESCE(current_setting('orch.guard_epoch', true), '') AS value) m",
    )
    .fetch_one(&mut *conn)
    .await?;
    if personal && let Some(user) = super::transaction_guard::personal_user() {
        crate::sequences::notify_identity(conn, user).await?;
    }
    Ok(epoch)
}

/// Records an existing object's authority change in its current owning root.
/// The caller has pinned that object's identity/home in its transaction. Only
/// this mutation uses the supplied scope; later writes retain the caller's scope.
pub async fn bump_epoch_in_root(conn: &mut Conn, root: Id) -> Result<i64, DbError> {
    if !super::transaction_guard::in_transaction(conn) {
        return Err(DbError::Rule {
            constraint: "scoped_authority_requires_transaction".into(),
        });
    }
    super::transaction_guard::lock_authority(conn).await?;
    let previous: Option<String> =
        sqlx::query_scalar("SELECT current_setting('orch.authz_scope',true)")
            .fetch_one(&mut *conn)
            .await?;
    let _: String = sqlx::query_scalar(
        "SELECT set_config('orch.authz_scope',jsonb_build_array($1::uuid)::text,true)",
    )
    .bind(root)
    .fetch_one(&mut *conn)
    .await?;
    let epoch = bump_epoch(conn).await?;
    let _: String = sqlx::query_scalar("SELECT set_config('orch.authz_scope',$1,true)")
        .bind(previous.unwrap_or_default())
        .fetch_one(conn)
        .await?;
    Ok(epoch)
}

/// Declares the change of the system's defaults page, the one page that has no
/// node: it is a change of authority of the whole system, which moves the
/// system's epoch at its commit, where two such changes are ordered.
pub async fn lock_system_defaults(conn: &mut Conn) -> Result<(), DbError> {
    super::transaction_guard::lock_authority(conn).await
}

/// Names that a line of history shows, found by identifier.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Named {
    /// The identifier.
    pub id: Id,
    /// The name.
    pub name: String,
}

/// The names people are shown by.
pub async fn user_names(conn: &mut Conn, users: &[Id]) -> Result<Vec<Named>, DbError> {
    Ok(
        sqlx::query_as("SELECT id, display_name AS name FROM users WHERE id = ANY($1)")
            .bind(users)
            .fetch_all(conn)
            .await?,
    )
}

/// The names of groups.
pub async fn group_names(conn: &mut Conn, groups: &[Id]) -> Result<Vec<Named>, DbError> {
    Ok(
        sqlx::query_as("SELECT id, name FROM groups WHERE id = ANY($1)")
            .bind(groups)
            .fetch_all(conn)
            .await?,
    )
}

/// A group with the org that keeps it and that org's root.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct GroupRow {
    /// The group.
    pub id: Id,
    /// The org that keeps it.
    pub org_id: Id,
    /// The root org of that org.
    pub root_id: Id,
    /// Its name.
    pub name: String,
}

/// The groups `groups`.
pub async fn groups(conn: &mut Conn, groups: &[Id]) -> Result<Vec<GroupRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT g.id, g.org_id, n.org_id AS root_id, g.name
           FROM groups g JOIN nodes n ON n.id = g.org_id WHERE g.id = ANY($1)",
    )
    .bind(groups)
    .fetch_all(conn)
    .await?)
}

/// Which of `users` are members of the root org `root`.
pub async fn members_among(conn: &mut Conn, root: Id, users: &[Id]) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT user_id FROM org_members WHERE org_id = $1 AND user_id = ANY($2)",
    )
    .bind(root)
    .bind(users)
    .fetch_all(conn)
    .await?)
}

/// Which of `users` are local holders for naming in the root tree: its members and
/// service identities kept by an org inside it, without making those identities members.
pub async fn local_holders_among(
    conn: &mut Conn,
    root: Id,
    users: &[Id],
) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT u.id FROM users u WHERE u.id = ANY($2)
           AND (EXISTS (SELECT 1 FROM org_members m WHERE m.user_id = u.id AND m.org_id = $1)
                OR (u.kind = 'service' AND EXISTS (
                    SELECT 1 FROM nodes n WHERE n.id = u.service_org_id AND n.org_id = $1)))",
    )
    .bind(root)
    .bind(users)
    .fetch_all(conn)
    .await?)
}

/// The root orgs of the places `places`, each with its place.
pub async fn roots_of(conn: &mut Conn, places: &[Id]) -> Result<Vec<(Id, Id)>, DbError> {
    Ok(
        sqlx::query_as("SELECT id, org_id FROM nodes WHERE id = ANY($1)")
            .bind(places)
            .fetch_all(conn)
            .await?,
    )
}

/// The members of the groups `groups`, each with its group: at most
/// `limit` pairs in a fixed order.
pub async fn group_people(
    conn: &mut Conn,
    groups: &[Id],
    limit: i64,
    at: DateTime<Utc>,
) -> Result<Vec<(Id, Id)>, DbError> {
    Ok(sqlx::query_as(
        "WITH requested AS MATERIALIZED (
           SELECT * FROM unnest($1::uuid[]) WITH ORDINALITY AS g(id,idx)
         ) SELECT DISTINCT g.id, h.user_id FROM access_holder_people_many(
             ARRAY(SELECT 'group'::text FROM requested ORDER BY idx), ARRAY(SELECT id FROM requested ORDER BY idx),
             ARRAY(SELECT ''::text FROM requested ORDER BY idx), $3, true) h
          JOIN requested g ON g.idx=h.wanted
          ORDER BY g.id, h.user_id LIMIT $2",
    )
    .bind(groups)
    .bind(limit)
    .bind(at)
    .fetch_all(conn)
    .await?)
}

/// A direct member of a group: exactly one identifier, with a role beside a place identifier.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct GroupHolderRow {
    /// One person.
    pub user_id: Option<Id>,
    /// Another group.
    pub member_group_id: Option<Id>,
    /// A role at this place.
    pub member_node_id: Option<Id>,
    /// Members by domain of this org.
    pub member_org_id: Option<Id>,
    /// The role at the member node.
    pub member_role: Option<String>,
}

/// Direct group members, as identifiers; expansion is the current authorization graph.
pub async fn group_holders(conn: &mut Conn, group: Id) -> Result<Vec<GroupHolderRow>, DbError> {
    Ok(sqlx::query_as("SELECT user_id, member_group_id, member_node_id, member_org_id, member_role FROM group_members WHERE group_id = $1 ORDER BY added_at, id")
        .bind(group).fetch_all(conn).await?)
}

/// Checks the entire nesting path affected by a new edge, under the authority write gate.
pub async fn group_edge_allowed(conn: &mut Conn, outer: Id, inner: Id) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "WITH RECURSIVE edges(outer_id, inner_id) AS (
           SELECT group_id, member_group_id FROM group_members WHERE member_group_id IS NOT NULL
           UNION SELECT $1::uuid, $2::uuid
         ), paths(start, last, depth, seen, cycle) AS (
           SELECT id, id, 1, ARRAY[id], false FROM groups
           UNION ALL SELECT p.start, e.inner_id, p.depth + 1, p.seen || e.inner_id, e.inner_id = ANY(p.seen)
             FROM paths p JOIN edges e ON e.outer_id = p.last WHERE p.depth <= 5 AND NOT p.cycle
         ) SELECT NOT EXISTS (SELECT 1 FROM paths WHERE cycle OR depth > 5)")
        .bind(outer).bind(inner).fetch_one(conn).await?)
}

/// Adds an exact holder identity; a duplicate is harmless.
pub async fn add_group_holder(
    conn: &mut Conn,
    group: Id,
    holder: &GroupHolderRow,
    by: Id,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query("INSERT INTO group_members (group_id, user_id, member_group_id, member_node_id, member_org_id, member_role, added_by, added_at)
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8) ON CONFLICT DO NOTHING")
        .bind(group).bind(holder.user_id).bind(holder.member_group_id).bind(holder.member_node_id).bind(holder.member_org_id)
        .bind(&holder.member_role).bind(by).bind(at).execute(conn).await?;
    Ok(())
}

/// Removes one exact identity, leaving membership through other holders intact.
pub async fn remove_group_holder(
    conn: &mut Conn,
    group: Id,
    holder: &GroupHolderRow,
) -> Result<bool, DbError> {
    Ok(sqlx::query("DELETE FROM group_members WHERE group_id = $1 AND user_id IS NOT DISTINCT FROM $2::uuid
        AND member_group_id IS NOT DISTINCT FROM $3::uuid AND member_node_id IS NOT DISTINCT FROM $4::uuid
        AND member_org_id IS NOT DISTINCT FROM $5::uuid AND member_role IS NOT DISTINCT FROM $6::text")
        .bind(group).bind(holder.user_id).bind(holder.member_group_id).bind(holder.member_node_id).bind(holder.member_org_id)
        .bind(&holder.member_role).execute(conn).await?.rows_affected() > 0)
}

/// One holder source to expand completely for private notification delivery.
#[derive(Debug, Clone, Default)]
pub struct NotificationPeopleSource<'a> {
    /// One directly named person.
    pub user: Option<Id>,
    /// Members of one group.
    pub group: Option<Id>,
    /// Members by domain of this org, distinct from everyone who may view it.
    pub members_org: Option<Id>,
    /// Places where a role can be held.
    pub role_places: &'a [Id],
    /// Roles that meet or exceed the requested role.
    pub roles: &'a [String],
    /// Only an explicit Everyone-at-place holder counts page-only guests.
    pub include_viewers: bool,
    /// Everyone means active human members of this tree.
    pub everyone_root: Option<Id>,
    /// Include active human system admins.
    pub system_admins: bool,
}

/// One complete-expansion page behind a notification holder, with no per-source truncation.
///
/// A source names one kind of holder, and an expansion whose holder is not named is not run:
/// a function's body is planned at each call whatever its arguments, 5-15 ms, so the two that a
/// source leaves empty cost more than the one that it names. The gate is a one-time filter on the
/// parameters; an expansion of a missing holder finds nobody, so the people are the same.
pub async fn notification_people_page(
    conn: &mut Conn,
    source: &NotificationPeopleSource<'_>,
    after: Option<Id>,
    limit: i64,
    at: DateTime<Utc>,
) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "WITH people AS (
           SELECT $2::uuid AS user_id WHERE $2::uuid IS NOT NULL
           UNION SELECT user_id FROM access_holder_people('group', $3, '', $8, true) WHERE $3::uuid IS NOT NULL
           UNION SELECT id FROM users WHERE $4 AND is_system_admin AND status = 'active'
           UNION SELECT user_id FROM org_members WHERE org_id = $5
           UNION SELECT user_id FROM access_role_people($6, $7, $11, $8, true) WHERE cardinality($6::uuid[]) > 0
           UNION SELECT user_id FROM access_holder_people('members', $10, '', $8, false) WHERE $10::uuid IS NOT NULL
         ) SELECT u.id FROM people p JOIN users u ON u.id = p.user_id
          WHERE u.kind = 'human' AND u.status = 'active' AND ($1::uuid IS NULL OR u.id > $1)
          ORDER BY u.id LIMIT $9",
    ).bind(after).bind(source.user).bind(source.group).bind(source.system_admins).bind(source.everyone_root)
        .bind(source.role_places).bind(source.roles).bind(at).bind(limit).bind(source.members_org).bind(source.include_viewers).fetch_all(conn).await?)
}

/// Active identities counted for an Everyone holder's consequences; the preview separately checks which may be named.
pub async fn all_active_users(conn: &mut Conn) -> Result<Vec<Id>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT id FROM users WHERE status='active' ORDER BY id")
            .fetch_all(conn)
            .await?,
    )
}

/// The system admins who are not suspended: at most `limit`, in a fixed order.
pub async fn system_admin_people(conn: &mut Conn, limit: i64) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM users WHERE is_system_admin AND status = 'active' ORDER BY id LIMIT $1",
    )
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// Which of `users` are active in all of `orgs`: suspended neither
/// altogether nor in any of them.
pub async fn active_among(conn: &mut Conn, users: &[Id], orgs: &[Id]) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT u.id FROM users u
          WHERE u.id = ANY($1) AND u.status = 'active'
            AND NOT EXISTS (SELECT 1 FROM org_suspensions s WHERE s.user_id = u.id AND s.org_id = ANY($2))",
    )
    .bind(users)
    .bind(orgs)
    .fetch_all(conn)
    .await?)
}

/// Every person who holds one of `roles` in one of `places`: given it,
/// through a group of theirs, or as a member of an org it is given to, with
/// the place, once for each. Only people who are not suspended altogether
/// are returned, at most `limit` pairs in a fixed order.
pub async fn people_given_role(
    conn: &mut Conn,
    places: &[Id],
    roles: &[String],
    now: DateTime<Utc>,
    limit: i64,
) -> Result<Vec<(Id, Id)>, DbError> {
    // Actual assignments keep their distance from the thing asking. Effective roles would move
    // inherited and system admins to every nearer place and send requests to all of them.
    Ok(sqlx::query_as(
        "SELECT DISTINCT r.node_id, h.user_id FROM node_roles r
          JOIN nodes n ON n.id=r.node_id AND n.deleted_at IS NULL
          CROSS JOIN LATERAL access_holder_people(
            CASE WHEN r.subject_kind='role' AND r.subject_role='viewer' THEN 'visible' ELSE r.subject_kind END,r.subject_id,
            COALESCE(r.subject_role,''),$3,true) h
          WHERE r.node_id=ANY($1) AND r.role=ANY($2) AND r.removed_at IS NULL
            AND (r.expires_at IS NULL OR r.expires_at>$3)
            AND NOT EXISTS (SELECT 1 FROM nodes dead WHERE dead.id=ANY(n.path) AND dead.deleted_at IS NOT NULL)
            AND NOT EXISTS (SELECT 1 FROM org_suspensions s WHERE s.user_id=h.user_id AND s.org_id=ANY(n.path))
          ORDER BY r.node_id,h.user_id LIMIT $4",
    )
    .bind(places)
    .bind(roles)
    .bind(now)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// Implicit shipped role entries use actual roles; explicit Everyone holders include page-only guests.
pub async fn people_in_roles(
    conn: &mut Conn,
    places: &[Id],
    roles: &[String],
    now: DateTime<Utc>,
    limit: i64,
    include_viewers: bool,
) -> Result<Vec<(Id, Id)>, DbError> {
    // Expand indexed holder sources once, then order the complete union. Page
    // boundaries must not truncate one source before deduplication.
    Ok(sqlx::query_as(
        "SELECT DISTINCT place, user_id FROM access_role_people($1, $2, $5, $3, true)
          ORDER BY place, user_id LIMIT $4",
    )
    .bind(places)
    .bind(roles)
    .bind(now)
    .bind(limit)
    .bind(include_viewers)
    .fetch_all(conn)
    .await?)
}

/// The people who are admins of the places `places` by being given the role
/// of Admin there (themselves, through a group or as a member of an org
/// that was), or as owners of them; and, where `system` is set, the admins
/// of the system. Nobody is listed twice.
pub async fn admin_people(
    conn: &mut Conn,
    places: &[Id],
    now: DateTime<Utc>,
    system: bool,
) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT user_id FROM (
           SELECT h.user_id FROM unnest($1::uuid[]) AS p(id)
             CROSS JOIN LATERAL access_holder_people('role', p.id, 'admin', $2, false) h
           UNION SELECT id FROM users WHERE $3 AND is_system_admin AND status = 'active'
         ) people ORDER BY user_id",
    )
    .bind(places)
    .bind(now)
    .bind(system)
    .fetch_all(conn)
    .await?)
}

/// The people whose domain is the org `org` or an org inside it: its
/// members, at most `limit` of them.
pub async fn org_member_people(conn: &mut Conn, org: Id, limit: i64) -> Result<Vec<Id>, DbError> {
    Ok(member_people(conn, &[org], limit)
        .await?
        .into_iter()
        .map(|(_, user)| user)
        .collect())
}

/// Current members by domain for several holder chips, including the active/deleted/suspended cuts.
pub async fn member_people(
    conn: &mut Conn,
    orgs: &[Id],
    limit: i64,
) -> Result<Vec<(Id, Id)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT DISTINCT o.id, m.user_id FROM org_members m JOIN users u ON u.id = m.user_id
          JOIN nodes d ON d.id = m.domain_id JOIN nodes o ON o.id = ANY(d.path)
          WHERE o.id = ANY($1) AND o.kind = 'org' AND u.status = 'active'
            AND NOT EXISTS (SELECT 1 FROM nodes p WHERE p.id = ANY(d.path) AND p.deleted_at IS NOT NULL)
            AND NOT EXISTS (SELECT 1 FROM org_suspensions s WHERE s.user_id = m.user_id AND s.org_id = ANY(d.path))
          ORDER BY o.id, m.user_id LIMIT $2")
        .bind(orgs).bind(limit).fetch_all(conn).await?)
}

/// The entries `ids`, as they are stored; one that was removed is among them.
pub async fn entries_by_id(conn: &mut Conn, ids: &[Id]) -> Result<Vec<EntryRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        entry_columns!(),
        " FROM permission_entries e WHERE e.id = ANY($1) ORDER BY e.created_at, e.id"
    ))
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

/// A group, as the API shows it.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct GroupFull {
    /// The group.
    pub id: Id,
    /// The org that keeps it.
    pub org_id: Id,
    /// The root org of that org.
    pub root_id: Id,
    /// Its name.
    pub name: String,
    /// What it is for.
    pub description: String,
    /// How many people are in it.
    pub member_count: i64,
}

macro_rules! group_columns {
    () => {
        "g.id, g.org_id, n.org_id AS root_id, g.name, g.description,
         (SELECT count(*) FROM group_members m WHERE m.group_id = g.id) AS member_count"
    };
}

/// The groups `ids` with their member counts.
pub async fn groups_full(conn: &mut Conn, ids: &[Id]) -> Result<Vec<GroupFull>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        group_columns!(),
        " FROM groups g JOIN nodes n ON n.id = g.org_id WHERE g.id = ANY($1) ORDER BY g.name, g.id"
    ))
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

/// Custom groups of the visible orgs offered by a holder search.
pub async fn groups_in_orgs(conn: &mut Conn, orgs: &[Id]) -> Result<Vec<GroupFull>, DbError> {
    Ok(sqlx::query_as(concat!("SELECT ", group_columns!(),
        " FROM groups g JOIN nodes n ON n.id = g.org_id WHERE g.org_id = ANY($1) AND n.deleted_at IS NULL ORDER BY g.name, g.id"))
        .bind(orgs).fetch_all(conn).await?)
}

/// The groups that the orgs `orgs` keep, those of the orgs below them too
/// when `under` is set.
pub async fn groups_of_orgs(
    conn: &mut Conn,
    org: Id,
    under: bool,
) -> Result<Vec<GroupFull>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        group_columns!(),
        " FROM groups g JOIN nodes n ON n.id = g.org_id
          WHERE n.deleted_at IS NULL AND (g.org_id = $1 OR ($2 AND $1 = ANY(n.path)))
          ORDER BY g.name, g.id"
    ))
    .bind(org)
    .bind(under)
    .fetch_all(conn)
    .await?)
}

/// What is said of a role beyond the row: whether it is a member's role at
/// their domain, and whether its subject is not of the tree of the root org
/// the place lies in.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct RoleFlags {
    /// The role's identifier.
    pub id: Id,
    /// A user's role at the org that is their domain.
    pub domain: bool,
    /// The subject is neither a local person or service identity, nor a group or an org of the tree.
    pub outside: bool,
}

/// The flags of the roles `ids`, for places that lie in the tree of `root`.
pub async fn role_flags(conn: &mut Conn, ids: &[Id], root: Id) -> Result<Vec<RoleFlags>, DbError> {
    Ok(sqlx::query_as(
        "SELECT r.id,
                (r.subject_kind = 'user' AND EXISTS (SELECT 1 FROM org_members m
                                                      WHERE m.user_id = r.subject_id AND m.domain_id = r.node_id)) AS domain,
                NOT (CASE r.subject_kind
                       WHEN 'user' THEN EXISTS (SELECT 1 FROM org_members m
                                                 WHERE m.user_id = r.subject_id AND m.org_id = $2)
                                        OR EXISTS (SELECT 1 FROM users u JOIN nodes n ON n.id = u.service_org_id
                                                     WHERE u.id = r.subject_id AND u.kind = 'service' AND n.org_id = $2)
                       WHEN 'group' THEN EXISTS (SELECT 1 FROM groups g JOIN nodes gn ON gn.id = g.org_id
                                                  WHERE g.id = r.subject_id AND gn.org_id = $2)
                       ELSE EXISTS (SELECT 1 FROM nodes o WHERE o.id = r.subject_id AND o.org_id = $2)
                     END) AS outside
           FROM node_roles r WHERE r.id = ANY($1)",
    )
    .bind(ids)
    .bind(root)
    .fetch_all(conn)
    .await?)
}

/// The role given by the row `id` that has not been taken away.
pub async fn role_row(conn: &mut Conn, id: Id) -> Result<Option<RoleRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        role_columns!(),
        " FROM node_roles WHERE id = $1 AND removed_at IS NULL"
    ))
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

/// A role to give.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewRole<'a> {
    /// The giving.
    pub id: Id,
    /// The place.
    pub node: Id,
    /// `user`, `group` or `members`.
    pub subject_kind: &'a str,
    /// The user, the group or the org.
    pub subject: Id,
    /// The role held at the subject place, for `role`.
    pub subject_role: Option<&'a str>,
    /// The role.
    pub role: &'a str,
    /// When it ends by itself.
    pub expires_at: Option<DateTime<Utc>>,
    /// Why.
    pub note: &'a str,
    /// Who gives it.
    pub by: Id,
    /// When.
    pub at: DateTime<Utc>,
}

/// Gives a role. A subject holds one lasting role at a place and any others
/// with an end beside it, one grant for each role; the strongest in force
/// applies. A role the subject held there and that ended by itself is taken
/// away first, so that giving a new one is not a clash with a role that no
/// longer counts. Holding the same role there, or a lasting role when this
/// one lasts too, is a taken unique value.
pub async fn put_role(conn: &mut Conn, new: &NewRole<'_>) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE node_roles SET removed_at = $4, removed_by = $5, revision = revision + 1
          WHERE node_id = $1 AND subject_kind = $2 AND subject_id = $3 AND removed_at IS NULL
            AND expires_at IS NOT NULL AND expires_at <= $4
            AND subject_role IS NOT DISTINCT FROM $6::text",
    )
    .bind(new.node)
    .bind(new.subject_kind)
    .bind(new.subject)
    .bind(new.at)
    .bind(new.by)
    .bind(new.subject_role)
    .execute(&mut *conn)
    .await?;
    sqlx::query(
        "INSERT INTO node_roles (id, node_id, subject_kind, subject_id, role, expires_at, note, created_by, created_at, subject_role)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(new.id)
    .bind(new.node)
    .bind(new.subject_kind)
    .bind(new.subject)
    .bind(new.role)
    .bind(new.expires_at)
    .bind(new.note)
    .bind(new.by)
    .bind(new.at)
    .bind(new.subject_role)
    .execute(conn)
    .await?;
    Ok(())
}

/// What changes in a role.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoleChange<'a> {
    /// A new role.
    pub role: Option<&'a str>,
    /// A new end: `Some(None)` takes the end away.
    pub expires_at: Option<Option<DateTime<Utc>>>,
    /// A new note.
    pub note: Option<&'a str>,
}

/// Changes a role if it is still at `expected` revision; the answer is the
/// role as it is now, or none when it moved on.
pub async fn change_role(
    conn: &mut Conn,
    id: Id,
    change: &RoleChange<'_>,
    expected: i64,
) -> Result<Option<RoleRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "UPDATE node_roles SET role = COALESCE($2, role),
                expires_at = CASE WHEN $3 THEN $4 ELSE expires_at END,
                note = COALESCE($5, note), revision = revision + 1
          WHERE id = $1 AND revision = $6 AND removed_at IS NULL
          RETURNING ",
        role_columns!()
    ))
    .bind(id)
    .bind(change.role)
    .bind(change.expires_at.is_some())
    .bind(change.expires_at.flatten())
    .bind(change.note)
    .bind(expected)
    .fetch_optional(conn)
    .await?)
}

/// Takes a role away; the answer says whether it was there.
pub async fn remove_role(
    conn: &mut Conn,
    id: Id,
    by: Id,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    let done = sqlx::query(
        "UPDATE node_roles SET removed_at = $3, removed_by = $2, revision = revision + 1
          WHERE id = $1 AND removed_at IS NULL",
    )
    .bind(id)
    .bind(by)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Makes `user` a member of the root org `root` with `domain` as their
/// domain, invited by `invited_by` where somebody invited them. A person who
/// is a member of that tree already is a taken unique value. The role they
/// hold at the domain and the epoch are the caller's.
pub async fn add_member(
    conn: &mut Conn,
    root: Id,
    user: Id,
    domain: Id,
    invited_by: Option<Id>,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO org_members (org_id, user_id, domain_id, joined_at, invited_by)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(root)
    .bind(user)
    .bind(domain)
    .bind(at)
    .bind(invited_by)
    .execute(conn)
    .await?;
    Ok(())
}

// ---- groups ----

/// Makes a group kept by the org `org`. A name the org has already, without
/// regard to case, is a taken unique value.
pub async fn create_group(
    conn: &mut Conn,
    id: Id,
    org: Id,
    name: &str,
    description: &str,
    by: Id,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO groups (id, org_id, name, description, created_by, created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $6)",
    )
    .bind(id)
    .bind(org)
    .bind(name)
    .bind(description)
    .bind(by)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

/// Renames a group or describes it anew.
pub async fn change_group(
    conn: &mut Conn,
    id: Id,
    name: Option<&str>,
    description: Option<&str>,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE groups SET name = COALESCE($2, name), description = COALESCE($3, description), updated_at = $4
          WHERE id = $1",
    )
    .bind(id)
    .bind(name)
    .bind(description)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

/// Deletes a group with the roles it holds and the entries that name it,
/// which are taken away by `by` at `at`; its members go with it. The answer
/// is how many roles and how many entries went.
pub async fn delete_group(
    conn: &mut Conn,
    id: Id,
    by: Id,
    at: DateTime<Utc>,
) -> Result<(i64, i64), DbError> {
    let roles = sqlx::query(
        "UPDATE node_roles SET removed_at = $3, removed_by = $2, revision = revision + 1
          WHERE subject_kind = 'group' AND subject_id = $1 AND removed_at IS NULL",
    )
    .bind(id)
    .bind(by)
    .bind(at)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    let entries = sqlx::query(
        "UPDATE permission_entries SET removed_at = $3, removed_by = $2, revision = revision + 1
          WHERE holder_kind = 'group' AND holder_id = $1 AND removed_at IS NULL",
    )
    .bind(id)
    .bind(by)
    .bind(at)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    sqlx::query("DELETE FROM groups WHERE id = $1")
        .bind(id)
        .execute(conn)
        .await?;
    Ok((
        i64::try_from(roles).unwrap_or(i64::MAX),
        i64::try_from(entries).unwrap_or(i64::MAX),
    ))
}

/// The people in a group, by name.
pub async fn group_member_ids(conn: &mut Conn, group: Id) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT m.user_id FROM group_members m JOIN users u ON u.id = m.user_id
          WHERE m.group_id = $1 ORDER BY lower(u.display_name), m.user_id",
    )
    .bind(group)
    .fetch_all(conn)
    .await?)
}

/// Puts `user` in a group; the answer says whether they were not in it.
pub async fn add_group_member(
    conn: &mut Conn,
    group: Id,
    user: Id,
    by: Id,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    let done = sqlx::query(
        "INSERT INTO group_members (group_id, user_id, added_by, added_at) VALUES ($1, $2, $3, $4)
         ON CONFLICT DO NOTHING",
    )
    .bind(group)
    .bind(user)
    .bind(by)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Takes `user` out of a group; the answer says whether they were in it.
pub async fn remove_group_member(conn: &mut Conn, group: Id, user: Id) -> Result<bool, DbError> {
    let done = sqlx::query("DELETE FROM group_members WHERE group_id = $1 AND user_id = $2")
        .bind(group)
        .bind(user)
        .execute(conn)
        .await?;
    Ok(done.rows_affected() == 1)
}

/// Whether `user` is named in the Owners row of the node `node`, themselves
/// or through a group.
pub async fn owns_page(conn: &mut Conn, node: Id, user: Id) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM permission_entries e
                         WHERE e.object_kind = 'node' AND e.object_id = $1 AND e.page = 'live'
                           AND e.permission = 'access.own' AND e.removed_at IS NULL
                           AND ((e.holder_kind = 'user' AND e.holder_id = $2)
                                OR (e.holder_kind = 'group' AND EXISTS (
                                      SELECT 1 FROM group_members gm
                                       WHERE gm.group_id = e.holder_id AND gm.user_id = $2))))",
    )
    .bind(node)
    .bind(user)
    .fetch_one(conn)
    .await?)
}

// ---- members of an org ----

/// A member of an org with what a list of members shows of them.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct MemberRow {
    /// The person.
    pub user_id: Id,
    /// Their domain.
    pub domain_id: Id,
    /// The short names down to their domain, joined by dots, and their
    /// username.
    pub address: String,
    /// The role they hold at their domain, when they hold one.
    pub role: Option<String>,
    /// Whether they are one of the owners of the org asked about.
    pub is_owner: bool,
    /// Since when they have been a member.
    pub since: DateTime<Utc>,
    /// How many roles they hold by name in the tree, besides the one at
    /// their domain.
    pub also: i64,
    /// Whether they are suspended in the org asked about.
    pub suspended: bool,
}

/// What a member row is read with and from: `$1` is the org asked about
/// (it decides `is_owner` and `suspended`) and `$2` the moment roles are
/// counted at.
macro_rules! member_select {
    () => {
        "SELECT m.user_id, m.domain_id,
                COALESCE((SELECT string_agg(o.slug, '.' ORDER BY p.ord)
                            FROM unnest(d.path) WITH ORDINALITY AS p(id, ord)
                            JOIN orgs o ON o.node_id = p.id), '') || '.' || u.username AS address,
                (SELECT r.role FROM node_roles r
                  WHERE r.node_id = m.domain_id AND r.subject_kind = 'user' AND r.subject_id = m.user_id
                    AND r.removed_at IS NULL AND (r.expires_at IS NULL OR r.expires_at > $2)
                  ORDER BY CASE r.role WHEN 'admin' THEN 4 WHEN 'developer' THEN 3 WHEN 'interactor' THEN 2 ELSE 1 END DESC
                  LIMIT 1) AS role,
                EXISTS (SELECT 1 FROM permission_entries e
                         WHERE e.object_kind = 'node' AND e.object_id = $1 AND e.page = 'live'
                           AND e.permission = 'access.own' AND e.removed_at IS NULL
                           AND ((e.holder_kind = 'user' AND e.holder_id = m.user_id)
                                OR (e.holder_kind = 'group' AND EXISTS (
                                      SELECT 1 FROM group_members gm WHERE gm.group_id = e.holder_id AND gm.user_id = m.user_id)))) AS is_owner,
                m.joined_at AS since,
                (SELECT count(*) FROM node_roles r JOIN nodes rn ON rn.id = r.node_id
                  WHERE r.subject_kind = 'user' AND r.subject_id = m.user_id AND r.removed_at IS NULL
                    AND r.node_id <> m.domain_id AND rn.org_id = m.org_id
                    AND (r.expires_at IS NULL OR r.expires_at > $2)) AS also,
                EXISTS (SELECT 1 FROM org_suspensions s WHERE s.org_id = $1 AND s.user_id = m.user_id) AS suspended
           FROM org_members m
           JOIN nodes d ON d.id = m.domain_id
           JOIN users u ON u.id = m.user_id"
    };
}

/// The members of the org `org`: everybody whose domain is the org or an
/// org inside it, or only the org itself with `direct`; those whose name or
/// username holds `text` when it is given; by username, after the member
/// `after` (a username and an identifier), at most `limit`.
pub async fn members_of(
    conn: &mut Conn,
    org: Id,
    direct: bool,
    text: Option<&str>,
    after: Option<(&str, Id)>,
    limit: i64,
    now: DateTime<Utc>,
) -> Result<Vec<MemberRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        member_select!(),
        " WHERE ((NOT $3 AND $1 = ANY(d.path)) OR ($3 AND m.domain_id = $1))
            AND ($4::text IS NULL OR strpos(lower(u.username), lower($4)) > 0
                 OR strpos(lower(u.display_name), lower($4)) > 0)
            AND ($5::text IS NULL OR (u.username, m.user_id) > ($5, $6))
          ORDER BY u.username, m.user_id
          LIMIT $7"
    ))
    .bind(org)
    .bind(now)
    .bind(direct)
    .bind(text)
    .bind(after.map(|(name, _)| name.to_owned()))
    .bind(after.map(|(_, id)| id))
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// The member `user` of the root org `root`, read as the list of the members
/// of the root org reads them; nothing where they are no member.
pub async fn member_row(
    conn: &mut Conn,
    root: Id,
    user: Id,
    now: DateTime<Utc>,
) -> Result<Option<MemberRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        member_select!(),
        " WHERE m.org_id = $1 AND m.user_id = $3"
    ))
    .bind(root)
    .bind(now)
    .bind(user)
    .fetch_optional(conn)
    .await?)
}

// ---- suspension in an org ----

/// A person held out of an org.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct SuspensionRow {
    /// The person.
    pub user_id: Id,
    /// Why.
    pub reason: String,
    /// Who suspended them.
    pub suspended_by: Id,
    /// When.
    pub suspended_at: DateTime<Utc>,
    /// Whether they are no member of the tree: somebody let into a place.
    pub outside: bool,
    /// How many things in the org are theirs as first owner.
    pub owns: i64,
}

/// Holds `user` out of the org `org`. Somebody who is held out already is a
/// taken unique value.
pub async fn suspend_member(
    conn: &mut Conn,
    org: Id,
    user: Id,
    reason: &str,
    by: Id,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO org_suspensions (org_id, user_id, reason, suspended_by, suspended_at)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(org)
    .bind(user)
    .bind(reason)
    .bind(by)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

/// Lifts a suspension; the answer says whether there was one.
pub async fn end_suspension(conn: &mut Conn, org: Id, user: Id) -> Result<bool, DbError> {
    let done = sqlx::query("DELETE FROM org_suspensions WHERE org_id = $1 AND user_id = $2")
        .bind(org)
        .bind(user)
        .execute(conn)
        .await?;
    Ok(done.rows_affected() == 1)
}

/// Who is held out of the org `org` now, the latest first, or only `user`.
pub async fn suspensions_in(
    conn: &mut Conn,
    org: Id,
    user: Option<Id>,
) -> Result<Vec<SuspensionRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT s.user_id, s.reason, s.suspended_by, s.suspended_at,
                NOT EXISTS (SELECT 1 FROM org_members m
                              JOIN nodes d ON d.id = m.domain_id
                             WHERE m.user_id = s.user_id AND $1 = ANY(d.path)) AS outside,
                (SELECT count(*) FROM nodes n
                  WHERE $1 = ANY(n.path) AND n.owner_id = s.user_id AND n.deleted_at IS NULL) AS owns
           FROM org_suspensions s
          WHERE s.org_id = $1 AND ($2::uuid IS NULL OR s.user_id = $2)
          ORDER BY s.suspended_at DESC, s.user_id",
    )
    .bind(org)
    .bind(user)
    .fetch_all(conn)
    .await?)
}

/// Whether `user` is a member of the tree whose domain lies in the org
/// `org` or holds a role in it or anywhere below it: somebody the org's
/// admins may hold out.
pub async fn belongs_in(conn: &mut Conn, org: Id, user: Id) -> Result<Option<bool>, DbError> {
    let member: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM org_members m JOIN nodes d ON d.id = m.domain_id
                         WHERE m.user_id = $2 AND $1 = ANY(d.path))",
    )
    .bind(org)
    .bind(user)
    .fetch_one(&mut *conn)
    .await?;
    if member {
        return Ok(Some(false));
    }
    let let_in: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM node_roles r JOIN nodes n ON n.id = r.node_id
                         WHERE r.subject_kind = 'user' AND r.subject_id = $2 AND r.removed_at IS NULL
                           AND $1 = ANY(n.path))
             OR EXISTS (SELECT 1 FROM permission_entries e JOIN nodes n ON n.id = e.object_id
                         WHERE e.object_kind = 'node' AND e.holder_kind = 'user' AND e.holder_id = $2
                           AND e.removed_at IS NULL AND e.permission <> 'access.own' AND $1 = ANY(n.path))",
    )
    .bind(org)
    .bind(user)
    .fetch_one(conn)
    .await?;
    Ok(let_in.then_some(true))
}

// ---- ending and moving a membership ----

/// A node whose Owners row names a person, with what becomes of it when they
/// leave the tree.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct OwnedNode {
    /// The node.
    pub id: Id,
    /// Its kind: `org`, `project`, `topic` or `thread`.
    pub kind: String,
    /// The org it lies in: the nearest org above it. A root org lies in none.
    pub lies_in: Option<Id>,
    /// Whether the person is the only one its Owners row names.
    pub alone: bool,
    /// Whether it is deleted and may still be restored.
    pub deleted: bool,
}

/// The nodes of the tree of the root org `root` that `user` owns: the Owners
/// row of their live page names them, or they are the first owner. The
/// shallowest come first, so that the first owner of an org is settled
/// before what lies in it is passed to them.
pub async fn owned_nodes(conn: &mut Conn, root: Id, user: Id) -> Result<Vec<OwnedNode>, DbError> {
    Ok(sqlx::query_as(
        "SELECT n.id, n.kind,
                (SELECT a.id FROM nodes a
                  WHERE a.id = ANY(n.path) AND a.id <> n.id AND a.kind = 'org'
                  ORDER BY array_position(n.path, a.id) DESC LIMIT 1) AS lies_in,
                NOT EXISTS (SELECT 1 FROM permission_entries o
                             WHERE o.object_kind = 'node' AND o.object_id = n.id AND o.page = 'live'
                               AND o.permission = 'access.own' AND o.removed_at IS NULL
                               AND NOT (o.holder_kind = 'user' AND o.holder_id = $2)) AS alone,
                (n.deleted_at IS NOT NULL) AS deleted
           FROM nodes n
          WHERE n.org_id = $1
            AND (n.owner_id = $2
                 OR EXISTS (SELECT 1 FROM permission_entries e
                             WHERE e.object_kind = 'node' AND e.object_id = n.id AND e.page = 'live'
                               AND e.permission = 'access.own' AND e.removed_at IS NULL
                               AND e.holder_kind = 'user' AND e.holder_id = $2))
          ORDER BY cardinality(n.path), n.id",
    )
    .bind(root)
    .bind(user)
    .fetch_all(conn)
    .await?)
}

/// How many nodes of each kind `user` owns that are not deleted: in the
/// tree of `root`, or only inside the node `inside`; only those the Owners
/// row of which names nobody else with `alone`.
pub async fn owned_counts(
    conn: &mut Conn,
    root: Id,
    user: Id,
    inside: Option<Id>,
    alone: bool,
) -> Result<Vec<(String, i64)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT n.kind, count(*)
           FROM nodes n
          WHERE n.org_id = $1 AND n.deleted_at IS NULL
            AND ($3::uuid IS NULL OR $3 = ANY(n.path))
            AND (n.owner_id = $2
                 OR EXISTS (SELECT 1 FROM permission_entries e
                             WHERE e.object_kind = 'node' AND e.object_id = n.id AND e.page = 'live'
                               AND e.permission = 'access.own' AND e.removed_at IS NULL
                               AND e.holder_kind = 'user' AND e.holder_id = $2))
            AND (NOT $4 OR NOT EXISTS (SELECT 1 FROM permission_entries o
                             WHERE o.object_kind = 'node' AND o.object_id = n.id AND o.page = 'live'
                               AND o.permission = 'access.own' AND o.removed_at IS NULL
                               AND NOT (o.holder_kind = 'user' AND o.holder_id = $2)))
          GROUP BY n.kind",
    )
    .bind(root)
    .bind(user)
    .bind(inside)
    .bind(alone)
    .fetch_all(conn)
    .await?)
}

/// How many documents of the tree of `root` `user` owns, as [`owned_counts`] counts nodes: their
/// first owner or one its Owners row names, inside `inside` where it is given, and with nobody else
/// in that row where `alone` says so. A document beside a deleted node is gone with it, and not
/// counted.
pub async fn owned_document_count(
    conn: &mut Conn,
    root: Id,
    user: Id,
    inside: Option<Id>,
    alone: bool,
) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT count(*)
           FROM documents d
           JOIN nodes n ON n.id = d.node_id
          WHERE d.org_id = $1 AND d.deleted_at IS NULL AND n.deleted_at IS NULL
            AND ($3::uuid IS NULL OR $3 = ANY(n.path))
            AND (d.owner_id = $2
                 OR EXISTS (SELECT 1 FROM permission_entries e
                             WHERE e.object_kind = 'document' AND e.object_id = d.id AND e.page = 'live'
                               AND e.permission = 'access.own' AND e.removed_at IS NULL
                               AND e.holder_kind = 'user' AND e.holder_id = $2))
            AND (NOT $4 OR NOT EXISTS (SELECT 1 FROM permission_entries o
                             WHERE o.object_kind = 'document' AND o.object_id = d.id AND o.page = 'live'
                               AND o.permission = 'access.own' AND o.removed_at IS NULL
                               AND NOT (o.holder_kind = 'user' AND o.holder_id = $2)))",
    )
    .bind(root)
    .bind(user)
    .bind(inside)
    .bind(alone)
    .fetch_one(conn)
    .await?)
}

/// How many roles `user` holds in the tree of `root`, and how many entries
/// of pages name them (not counting the Owners rows, which are counted
/// with what they own).
pub async fn named_counts(conn: &mut Conn, root: Id, user: Id) -> Result<(i64, i64), DbError> {
    Ok(sqlx::query_as(
        "SELECT (SELECT count(*) FROM node_roles r JOIN nodes n ON n.id = r.node_id
                  WHERE n.org_id = $1 AND r.subject_kind = 'user' AND r.subject_id = $2
                    AND r.removed_at IS NULL),
                (SELECT count(*) FROM permission_entries e JOIN nodes n ON n.id = e.object_id
                  WHERE e.object_kind = 'node' AND n.org_id = $1 AND e.holder_kind = 'user'
                    AND e.holder_id = $2 AND e.removed_at IS NULL AND e.permission <> 'access.own')",
    )
    .bind(root)
    .bind(user)
    .fetch_one(conn)
    .await?)
}

/// The roles `user` holds by name in the tree of `root`, except the ones
/// held at the node `except`: the roles a change of domain does not touch.
pub async fn roles_elsewhere(
    conn: &mut Conn,
    root: Id,
    user: Id,
    except: Id,
) -> Result<Vec<RoleRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        role_columns!(),
        " FROM node_roles
          WHERE subject_kind = 'user' AND subject_id = $2 AND removed_at IS NULL AND node_id <> $3
            AND node_id IN (SELECT id FROM nodes WHERE org_id = $1)
          ORDER BY created_at, id"
    ))
    .bind(root)
    .bind(user)
    .bind(except)
    .fetch_all(conn)
    .await?)
}

/// The owners of the node `node` that are people, those who have been in
/// the row longest first.
pub async fn owner_users(conn: &mut Conn, node: Id) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT holder_id FROM permission_entries
          WHERE object_kind = 'node' AND object_id = $1 AND page = 'live' AND permission = 'access.own'
            AND holder_kind = 'user' AND removed_at IS NULL
          ORDER BY created_at, id",
    )
    .bind(node)
    .fetch_all(conn)
    .await?)
}

/// The role `user` holds by name at the node `node`, when they hold one: the strongest of their grants there that
/// has not ended.
pub async fn role_at(conn: &mut Conn, node: Id, user: Id) -> Result<Option<String>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT role FROM node_roles
          WHERE node_id = $1 AND subject_kind = 'user' AND subject_id = $2 AND removed_at IS NULL
            AND (expires_at IS NULL OR expires_at > now())
          ORDER BY CASE role WHEN 'admin' THEN 4 WHEN 'developer' THEN 3 WHEN 'interactor' THEN 2 ELSE 1 END DESC
          LIMIT 1",
    )
    .bind(node)
    .bind(user)
    .fetch_optional(conn)
    .await?)
}

/// How many nodes that are not deleted lie inside `inside` and not inside
/// `outside`, in the tree of `root`, and are reached by `user` through no
/// role, group or entry of theirs besides the role at their domain
/// `domain`: what they would lose with that role, or gain with a role there.
pub async fn reached_only_through_domain(
    conn: &mut Conn,
    root: Id,
    user: Id,
    domain: Id,
    inside: Id,
    outside: Id,
) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM nodes n
          WHERE n.org_id = $1 AND n.deleted_at IS NULL
            AND $4 = ANY(n.path) AND NOT ($5 = ANY(n.path))
            AND NOT EXISTS (SELECT 1 FROM node_roles r
                             WHERE r.subject_kind = 'user' AND r.subject_id = $2 AND r.removed_at IS NULL
                               AND r.node_id <> $3 AND r.node_id = ANY(n.path))
            AND NOT EXISTS (SELECT 1 FROM node_roles r JOIN group_members gm ON gm.group_id = r.subject_id
                             WHERE r.subject_kind = 'group' AND gm.user_id = $2 AND r.removed_at IS NULL
                               AND r.node_id = ANY(n.path))
            AND NOT EXISTS (SELECT 1 FROM permission_entries e
                             WHERE e.object_kind = 'node' AND e.object_id = ANY(n.path) AND e.removed_at IS NULL
                               AND ((e.holder_kind = 'user' AND e.holder_id = $2)
                                    OR (e.holder_kind = 'group' AND e.holder_id IN
                                          (SELECT group_id FROM group_members WHERE user_id = $2))))",
    )
    .bind(root)
    .bind(user)
    .bind(domain)
    .bind(inside)
    .bind(outside)
    .fetch_one(conn)
    .await?)
}

/// What ending a membership did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Ended {
    /// Roles that were taken away.
    pub roles: i64,
    /// Entries naming the person that were taken off pages.
    pub entries: i64,
    /// Groups they were taken out of.
    pub groups: i64,
    /// What they owned alone, with the person it passed to.
    pub passed: Vec<(Id, Id)>,
    /// What they owned together with others, and was left to the others.
    pub left: Vec<Id>,
}

/// Ends the membership of `user` in the tree of the root org `root`, as
/// `by`: they leave every Owners row, and what they owned alone passes to
/// the first owner of the org it lies in (what they owned with others stays
/// with the others, the one longest in the row first); their roles, the
/// entries that name them, their groups, their suspensions in the tree and
/// the membership itself are removed. Nothing is deleted from the record.
pub async fn end_membership(
    conn: &mut Conn,
    root: Id,
    user: Id,
    by: Id,
    at: DateTime<Utc>,
) -> Result<Ended, DbError> {
    let mut ended = Ended::default();
    for node in owned_nodes(conn, root, user).await? {
        sqlx::query(
            "UPDATE permission_entries SET removed_at = $3, removed_by = $2, revision = revision + 1
              WHERE object_kind = 'node' AND object_id = $1 AND page = 'live' AND permission = 'access.own'
                AND holder_kind = 'user' AND holder_id = $4 AND removed_at IS NULL",
        )
        .bind(node.id)
        .bind(by)
        .bind(at)
        .bind(user)
        .execute(&mut *conn)
        .await?;
        let first: Id = sqlx::query_scalar("SELECT owner_id FROM nodes WHERE id = $1")
            .bind(node.id)
            .fetch_one(&mut *conn)
            .await?;
        let successor: Option<Id> = sqlx::query_scalar(
            "SELECT holder_id FROM permission_entries
              WHERE object_kind = 'node' AND object_id = $1 AND page = 'live' AND permission = 'access.own'
                AND holder_kind = 'user' AND removed_at IS NULL
              ORDER BY created_at, id LIMIT 1",
        )
        .bind(node.id)
        .fetch_optional(&mut *conn)
        .await?;
        if first != user {
            ended.left.push(node.id);
            continue;
        }
        if let Some(next) = successor {
            super::hierarchy::set_first_owner(conn, node.id, next, at).await?;
            ended.left.push(node.id);
            continue;
        }
        // Alone, or with groups only: the first owner of the org it lies in
        // becomes its first owner. A root org has no org above it; the one
        // who asks has seen to it that this is not the last owner of one.
        let Some(above) = node.lies_in else { continue };
        let target: Id = sqlx::query_scalar("SELECT owner_id FROM nodes WHERE id = $1")
            .bind(above)
            .fetch_one(&mut *conn)
            .await?;
        sqlx::query(
            "INSERT INTO permission_entries (id, object_kind, object_id, page, permission, holder_kind,
                                             holder_id, source, created_by, created_at)
             SELECT gen_random_uuid(), 'node', $1, 'live', 'access.own', 'user', $2, 'page', $3, $4
              WHERE NOT EXISTS (SELECT 1 FROM permission_entries e
                                 WHERE e.object_kind = 'node' AND e.object_id = $1 AND e.page = 'live'
                                   AND e.permission = 'access.own' AND e.holder_kind = 'user'
                                   AND e.holder_id = $2 AND e.removed_at IS NULL)",
        )
        .bind(node.id)
        .bind(target)
        .bind(by)
        .bind(at)
        .execute(&mut *conn)
        .await?;
        super::hierarchy::set_first_owner(conn, node.id, target, at).await?;
        ended.passed.push((node.id, target));
    }
    ended.roles = count_of(
        sqlx::query(
            "UPDATE node_roles SET removed_at = $3, removed_by = $2, revision = revision + 1
              WHERE subject_kind = 'user' AND subject_id = $4 AND removed_at IS NULL
                AND node_id IN (SELECT id FROM nodes WHERE org_id = $1)",
        )
        .bind(root)
        .bind(by)
        .bind(at)
        .bind(user)
        .execute(&mut *conn)
        .await?
        .rows_affected(),
    );
    ended.entries = count_of(
        sqlx::query(
            "UPDATE permission_entries SET removed_at = $3, removed_by = $2, revision = revision + 1
              WHERE holder_kind = 'user' AND holder_id = $4 AND removed_at IS NULL
                AND permission <> 'access.own' AND object_kind = 'node'
                AND object_id IN (SELECT id FROM nodes WHERE org_id = $1)",
        )
        .bind(root)
        .bind(by)
        .bind(at)
        .bind(user)
        .execute(&mut *conn)
        .await?
        .rows_affected(),
    );
    ended.groups = count_of(
        sqlx::query(
            "DELETE FROM group_members gm USING groups g, nodes gn
              WHERE gm.group_id = g.id AND gn.id = g.org_id AND gn.org_id = $1 AND gm.user_id = $2",
        )
        .bind(root)
        .bind(user)
        .execute(&mut *conn)
        .await?
        .rows_affected(),
    );
    sqlx::query(
        "DELETE FROM org_suspensions
          WHERE user_id = $2 AND org_id IN (SELECT id FROM nodes WHERE org_id = $1)",
    )
    .bind(root)
    .bind(user)
    .execute(&mut *conn)
    .await?;
    sqlx::query("DELETE FROM org_members WHERE org_id = $1 AND user_id = $2")
        .bind(root)
        .bind(user)
        .execute(&mut *conn)
        .await?;
    Ok(ended)
}

/// A count of rows as the signed number the API speaks.
fn count_of(rows: u64) -> i64 {
    i64::try_from(rows).unwrap_or(i64::MAX)
}

/// How far a role reaches: the higher, the more.
fn role_rank(role: &str) -> u8 {
    match role {
        "admin" => 4,
        "developer" => 3,
        "interactor" => 2,
        _ => 1,
    }
}

/// Changes the domain of `user` in the root org `root` from the org `from`
/// to the org `to`, as `by`: the role they held at `from` is taken away and
/// `role` is given at `to`, the higher one staying where they hold a role
/// there already. The answer is the role they hold at `to` afterwards.
#[expect(
    clippy::too_many_arguments,
    reason = "one act of one person between two places, by one person, at one moment"
)]
pub async fn move_member(
    conn: &mut Conn,
    root: Id,
    user: Id,
    from: Id,
    to: Id,
    role: &str,
    by: Id,
    at: DateTime<Utc>,
) -> Result<String, DbError> {
    move_member_with_choice(conn, root, user, from, to, role, true, by, at).await
}

/// Moves a member, preserving all current domain grants when no new role was chosen.
#[expect(
    clippy::too_many_arguments,
    reason = "one act of one person between two places, by one person, at one moment"
)]
pub async fn move_member_with_choice(
    conn: &mut Conn,
    root: Id,
    user: Id,
    from: Id,
    to: Id,
    role: &str,
    replace_role: bool,
    by: Id,
    at: DateTime<Utc>,
) -> Result<String, DbError> {
    // A domain can have a lasting role and shorter grants beside it. Keep the ends of the
    // grants that move, so moving the person does not silently make a temporary role lasting.
    let source: Vec<(String, Option<DateTime<Utc>>, String)> = sqlx::query_as(
        "SELECT role, expires_at, note FROM node_roles
          WHERE node_id = $1 AND subject_kind = 'user' AND subject_id = $2 AND removed_at IS NULL
            AND (expires_at IS NULL OR expires_at > $3)
          ORDER BY CASE role WHEN 'admin' THEN 4 WHEN 'developer' THEN 3 WHEN 'interactor' THEN 2 ELSE 1 END DESC,
                   expires_at DESC NULLS FIRST",
    )
    .bind(from)
    .bind(user)
    .bind(at)
    .fetch_all(&mut *conn)
    .await?;
    let moving = if replace_role || source.is_empty() {
        let chosen = source
            .iter()
            .find(|(held, _, _)| held == role)
            .or_else(|| source.first());
        vec![(
            role.to_owned(),
            chosen.and_then(|(_, end, _)| *end),
            chosen.map_or_else(String::new, |(_, _, note)| note.clone()),
        )]
    } else {
        source
    };
    sqlx::query(
        "UPDATE node_roles SET removed_at = $4, removed_by = $3, revision = revision + 1
          WHERE node_id = $1 AND subject_kind = 'user' AND subject_id = $2 AND removed_at IS NULL",
    )
    .bind(from)
    .bind(user)
    .bind(by)
    .bind(at)
    .execute(&mut *conn)
    .await?;
    for (moving_role, end, note) in moving {
        let held: Vec<(Id, String, Option<DateTime<Utc>>)> = sqlx::query_as(
            "SELECT id, role, expires_at FROM node_roles
              WHERE node_id = $1 AND subject_kind = 'user' AND subject_id = $2 AND removed_at IS NULL",
        )
        .bind(to)
        .bind(user)
        .fetch_all(&mut *conn)
        .await?;
        let lasting = held.iter().find(|(_, _, end)| end.is_none());
        if lasting.is_some_and(|(_, existing, _)| role_rank(existing) >= role_rank(&moving_role)) {
            continue;
        }
        let same = held
            .iter()
            .find(|(_, existing, _)| *existing == moving_role);
        if end.is_none()
            && let Some((lasting_id, _, _)) = lasting
        {
            if let Some((same_id, _, _)) = same {
                sqlx::query("UPDATE node_roles SET removed_at = $2, removed_by = $3, revision = revision + 1 WHERE id = $1")
                    .bind(same_id).bind(at).bind(by).execute(&mut *conn).await?;
            }
            sqlx::query("UPDATE node_roles SET role = $2, revision = revision + 1 WHERE id = $1")
                .bind(lasting_id)
                .bind(&moving_role)
                .execute(&mut *conn)
                .await?;
        } else if let Some((same_id, _, existing_end)) = same {
            let extend = match (existing_end, end) {
                (None, _) => false,
                (Some(_), None) => true,
                (Some(before), Some(after)) => after > *before,
            };
            if extend {
                sqlx::query(
                    "UPDATE node_roles SET expires_at = $2, revision = revision + 1 WHERE id = $1",
                )
                .bind(same_id)
                .bind(end)
                .execute(&mut *conn)
                .await?;
            }
        } else {
            sqlx::query(
                "INSERT INTO node_roles (id, node_id, subject_kind, subject_id, role, expires_at, note, created_by, created_at)
                 VALUES (gen_random_uuid(), $1, 'user', $2, $3, $4, $5, $6, $7)",
            )
            .bind(to)
            .bind(user)
            .bind(&moving_role)
            .bind(end)
            .bind(&note)
            .bind(by)
            .bind(at)
            .execute(&mut *conn)
            .await?;
        }
    }
    let now_held: String = sqlx::query_scalar(
        "SELECT role FROM node_roles
          WHERE node_id = $1 AND subject_kind = 'user' AND subject_id = $2 AND removed_at IS NULL
            AND (expires_at IS NULL OR expires_at > $3)
          ORDER BY CASE role WHEN 'admin' THEN 4 WHEN 'developer' THEN 3 WHEN 'interactor' THEN 2 ELSE 1 END DESC
          LIMIT 1",
    )
    .bind(to)
    .bind(user)
    .bind(at)
    .fetch_one(&mut *conn)
    .await?;
    sqlx::query(
        "UPDATE org_members SET domain_id = $3, moved_at = $4, moved_by = $5
          WHERE org_id = $1 AND user_id = $2",
    )
    .bind(root)
    .bind(user)
    .bind(to)
    .bind(at)
    .bind(by)
    .execute(conn)
    .await?;
    Ok(now_held)
}

/// The short names of the orgs `orgs`: one part of an address each.
pub async fn org_slugs(conn: &mut Conn, orgs: &[Id]) -> Result<Vec<(Id, String)>, DbError> {
    Ok(
        sqlx::query_as("SELECT node_id, slug FROM orgs WHERE node_id = ANY($1)")
            .bind(orgs)
            .fetch_all(conn)
            .await?,
    )
}

/// The usernames of `users`: the part of an address that names a person.
pub async fn usernames(conn: &mut Conn, users: &[Id]) -> Result<Vec<(Id, String)>, DbError> {
    Ok(
        sqlx::query_as("SELECT id, username FROM users WHERE id = ANY($1)")
            .bind(users)
            .fetch_all(conn)
            .await?,
    )
}

/// Copies the stored rows and the entries that count of the live page `from` onto the live page
/// `to`, the Owners row left out, each entry as written by `by` now: what a fork takes over from
/// the thread it was taken from when its maker asks for its page to be the original's. Returns how
/// many rows and entries were copied.
pub async fn copy_page(
    conn: &mut Conn,
    from: &PageKey,
    to: &PageKey,
    by: Id,
    at: DateTime<Utc>,
) -> Result<(u64, u64), DbError> {
    let rows = sqlx::query(
        "INSERT INTO permission_rows (object_kind, object_id, page, profile, permission, inherit, locked,
                                      updated_by, updated_at)
         SELECT $4, $5, page, profile, permission, inherit, false, $6, $7
           FROM permission_rows
          WHERE object_kind = $1 AND object_id = $2 AND page = $3 AND permission <> 'access.manage'
         ON CONFLICT (object_kind, object_id, page, profile, permission)
         DO UPDATE SET inherit = EXCLUDED.inherit, revision = permission_rows.revision + 1",
    )
    .bind(&from.object_kind)
    .bind(from.object_id)
    .bind(&from.page)
    .bind(&to.object_kind)
    .bind(to.object_id)
    .bind(by)
    .bind(at)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    let entries = sqlx::query(
        "INSERT INTO permission_entries (id, object_kind, object_id, page, profile, permission, holder_kind,
                                         holder_id, holder_role, options, expires_at, note, source, created_by,
                                         created_at)
         SELECT gen_random_uuid(), $4, $5, page, profile, permission, holder_kind, holder_id, holder_role,
                options, expires_at, note, 'page', $6, $7
           FROM permission_entries
          WHERE object_kind = $1 AND object_id = $2 AND page = $3 AND removed_at IS NULL
            AND permission <> 'access.own' AND (expires_at IS NULL OR expires_at > $7)
         ON CONFLICT DO NOTHING",
    )
    .bind(&from.object_kind)
    .bind(from.object_id)
    .bind(&from.page)
    .bind(&to.object_kind)
    .bind(to.object_id)
    .bind(by)
    .bind(at)
    .execute(conn)
    .await?
    .rows_affected();
    Ok((rows, entries))
}

/// A role to give a person by name in a place.
#[derive(Debug, Clone, Copy)]
pub struct PersonRole<'a> {
    /// The place: an org, a project or a topic.
    pub place: Id,
    /// The person.
    pub user: Id,
    /// `viewer`, `interactor`, `developer` or `admin`.
    pub role: &'a str,
    /// When it ends by itself.
    pub expires_at: Option<DateTime<Utc>>,
    /// Why it is given.
    pub note: &'a str,
    /// Who gives it.
    pub by: Id,
    /// When.
    pub at: DateTime<Utc>,
}

/// Gives the person a role of their own in the place, and returns the giving.
pub async fn give_role_to_person(conn: &mut Conn, role: &PersonRole<'_>) -> Result<Id, DbError> {
    let id = orch_types::new_id();
    sqlx::query(
        "INSERT INTO node_roles (id, node_id, subject_kind, subject_id, role, expires_at, note, created_by,
                                 created_at)
         VALUES ($1, $2, 'user', $3, $4, $5, $6, $7, $8)",
    )
    .bind(id)
    .bind(role.place)
    .bind(role.user)
    .bind(role.role)
    .bind(role.expires_at)
    .bind(role.note)
    .bind(role.by)
    .bind(role.at)
    .execute(conn)
    .await?;
    Ok(id)
}

/// Takes the giving `role` away, by `by` at `at`, and counts the change.
pub async fn end_role(conn: &mut Conn, role: Id, by: Id, at: DateTime<Utc>) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE node_roles SET removed_at = $3, removed_by = $2, revision = revision + 1
          WHERE id = $1 AND removed_at IS NULL",
    )
    .bind(role)
    .bind(by)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

/// Makes the giving `role` the role `to`, and counts the change.
pub async fn set_role(conn: &mut Conn, role: Id, to: &str) -> Result<(), DbError> {
    sqlx::query("UPDATE node_roles SET role = $2, revision = revision + 1 WHERE id = $1")
        .bind(role)
        .bind(to)
        .execute(conn)
        .await?;
    Ok(())
}

/// Locks the membership of `user` in the tree of the root org `root` until the transaction ends, so
/// that two moves of one person are made one after the other. Returns their domain, or none if
/// they are no member of that tree.
pub async fn lock_membership(conn: &mut Conn, root: Id, user: Id) -> Result<Option<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT domain_id FROM org_members WHERE org_id = $1 AND user_id = $2 FOR UPDATE",
    )
    .bind(root)
    .bind(user)
    .fetch_optional(conn)
    .await?)
}

/// Makes `domain` the domain of `user` in the tree of the root org `root`, moved by `by` at `at`.
/// Returns whether the person is a member of that tree.
pub async fn set_domain(
    conn: &mut Conn,
    root: Id,
    user: Id,
    domain: Id,
    by: Id,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE org_members SET domain_id = $3, moved_at = $5, moved_by = $4
          WHERE org_id = $1 AND user_id = $2",
    )
    .bind(root)
    .bind(user)
    .bind(domain)
    .bind(by)
    .bind(at)
    .execute(conn)
    .await?
    .rows_affected()
        == 1)
}

/// How many places of one kind a move of a member from one domain to another takes away and gives.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct PlaceCount {
    /// `org`, `project`, `topic` or `thread`.
    pub kind: String,
    /// How many lie in the old domain and not in the new one.
    pub lost: i64,
    /// How many lie in the new domain and not in the old one.
    pub gained: i64,
}

/// What lies in the org `old` and not in the org `new`, and the other way round, by kind: the
/// places where a role held at the domain stops to count and starts to. Nothing deleted is counted,
/// and a kind of which nothing differs is left out.
pub async fn places_between(conn: &mut Conn, old: Id, new: Id) -> Result<Vec<PlaceCount>, DbError> {
    Ok(sqlx::query_as(
        "SELECT kind,
                count(*) FILTER (WHERE $1 = ANY(path) AND NOT $2 = ANY(path)) AS lost,
                count(*) FILTER (WHERE $2 = ANY(path) AND NOT $1 = ANY(path)) AS gained
           FROM nodes
          WHERE deleted_at IS NULL AND path && ARRAY[$1, $2]
          GROUP BY kind
         HAVING count(*) FILTER (WHERE ($1 = ANY(path)) <> ($2 = ANY(path))) > 0
          ORDER BY kind",
    )
    .bind(old)
    .bind(new)
    .fetch_all(conn)
    .await?)
}

/// How many roles that count at `now` are given to the members of each of the orgs `orgs`, for the
/// orgs that have any: what a member holds for as long as the org is one of those they belong to.
pub async fn roles_given_to_members(
    conn: &mut Conn,
    orgs: &[Id],
    now: DateTime<Utc>,
) -> Result<Vec<(Id, i64)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT subject_id, count(*) FROM node_roles
          WHERE subject_kind = 'members' AND subject_id = ANY($1) AND removed_at IS NULL
            AND (expires_at IS NULL OR expires_at > $2)
          GROUP BY subject_id",
    )
    .bind(orgs)
    .bind(now)
    .fetch_all(conn)
    .await?)
}

/// A stretch of work that is undone however it ends: inside a transaction a savepoint, otherwise
/// a transaction of its own. What is written through it is read back through it, and is gone when
/// it is [`end_trial`]led.
pub type Trial<'c> = sqlx::Transaction<'c, sqlx::Postgres>;

/// Starts a [`Trial`] on the connection: used to find out what a change would come to, by making it
/// and asking, without leaving it made.
pub async fn begin_trial(conn: &mut Conn) -> Result<Trial<'_>, DbError> {
    use sqlx::Connection;
    Ok(conn.begin().await?)
}

/// Ends the trial and undoes everything written in it.
pub async fn end_trial(trial: Trial<'_>) -> Result<(), DbError> {
    Ok(trial.rollback().await?)
}

/// Rows written directly, for tests of what stands on them. Not part of
/// any request: a real membership or role is made by the request that
/// makes it.
pub mod testing {
    use super::{NewEntry, PageKey, insert_entry};
    use crate::{Conn, DbError};
    use chrono::{DateTime, Utc};
    use orch_types::{Id, Json, new_id};

    /// Makes `user` a system admin.
    pub async fn make_system_admin(conn: &mut Conn, user: Id) -> Result<(), DbError> {
        sqlx::query("UPDATE users SET is_system_admin = true WHERE id = $1")
            .bind(user)
            .execute(conn)
            .await?;
        Ok(())
    }

    /// Makes `user` a member of the root org `root`, with `domain` as their
    /// domain.
    pub async fn insert_member(
        conn: &mut Conn,
        root: Id,
        user: Id,
        domain: Id,
    ) -> Result<(), DbError> {
        sqlx::query("INSERT INTO org_members (org_id, user_id, domain_id) VALUES ($1, $2, $3)")
            .bind(root)
            .bind(user)
            .bind(domain)
            .execute(conn)
            .await?;
        Ok(())
    }

    /// Ends the membership of `user` in the root org `root`.
    pub async fn remove_member(conn: &mut Conn, root: Id, user: Id) -> Result<(), DbError> {
        sqlx::query("DELETE FROM org_members WHERE org_id = $1 AND user_id = $2")
            .bind(root)
            .bind(user)
            .execute(conn)
            .await?;
        Ok(())
    }

    /// Gives `role` in the place `place` to a subject of `subject_kind`
    /// (`user`, `group` or `members`), by `by`.
    pub async fn give_role(
        conn: &mut Conn,
        place: Id,
        subject_kind: &str,
        subject: Id,
        role: &str,
        by: Id,
        expires_at: Option<DateTime<Utc>>,
    ) -> Result<Id, DbError> {
        let id = new_id();
        sqlx::query(
            "INSERT INTO node_roles (id, node_id, subject_kind, subject_id, role, expires_at, created_by)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(id)
        .bind(place)
        .bind(subject_kind)
        .bind(subject)
        .bind(role)
        .bind(expires_at)
        .bind(by)
        .execute(conn)
        .await?;
        Ok(id)
    }

    /// Takes the giving `role` away.
    pub async fn take_role_away(
        conn: &mut Conn,
        role: Id,
        by: Id,
        at: DateTime<Utc>,
    ) -> Result<(), DbError> {
        sqlx::query("UPDATE node_roles SET removed_at = $3, removed_by = $2 WHERE id = $1")
            .bind(role)
            .bind(by)
            .bind(at)
            .execute(conn)
            .await?;
        Ok(())
    }

    /// A group named `name` kept by the org `org`.
    pub async fn insert_group(conn: &mut Conn, org: Id, name: &str, by: Id) -> Result<Id, DbError> {
        let id = new_id();
        sqlx::query("INSERT INTO groups (id, org_id, name, created_by) VALUES ($1, $2, $3, $4)")
            .bind(id)
            .bind(org)
            .bind(name)
            .bind(by)
            .execute(conn)
            .await?;
        Ok(id)
    }

    /// Puts `user` into `group`.
    pub async fn add_to_group(conn: &mut Conn, group: Id, user: Id) -> Result<(), DbError> {
        sqlx::query("INSERT INTO group_members (group_id, user_id) VALUES ($1, $2)")
            .bind(group)
            .bind(user)
            .execute(conn)
            .await?;
        Ok(())
    }

    /// Takes `user` out of `group`.
    pub async fn remove_from_group(conn: &mut Conn, group: Id, user: Id) -> Result<(), DbError> {
        sqlx::query("DELETE FROM group_members WHERE group_id = $1 AND user_id = $2")
            .bind(group)
            .bind(user)
            .execute(conn)
            .await?;
        Ok(())
    }

    /// Suspends `user` in the org `org`, by `by`.
    pub async fn suspend_in(conn: &mut Conn, org: Id, user: Id, by: Id) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO org_suspensions (org_id, user_id, suspended_by) VALUES ($1, $2, $3)",
        )
        .bind(org)
        .bind(user)
        .bind(by)
        .execute(conn)
        .await?;
        Ok(())
    }

    /// Lifts the suspension of `user` in `org`.
    pub async fn lift_suspension(conn: &mut Conn, org: Id, user: Id) -> Result<(), DbError> {
        sqlx::query("DELETE FROM org_suspensions WHERE org_id = $1 AND user_id = $2")
            .bind(org)
            .bind(user)
            .execute(conn)
            .await?;
        Ok(())
    }

    /// Names a holder, `(kind, id, role)` as an entry keeps it, in the row
    /// `permission` / `profile` of `page`, and returns the entry.
    pub async fn add_entry(
        conn: &mut Conn,
        page: &PageKey,
        profile: &str,
        permission: &str,
        holder: (&str, Option<Id>, Option<&str>),
        expires_at: Option<DateTime<Utc>>,
        at: DateTime<Utc>,
    ) -> Result<Id, DbError> {
        let id = new_id();
        let options = Json::Object(serde_json::Map::new());
        insert_entry(
            conn,
            &NewEntry {
                id,
                page,
                profile,
                permission,
                holder_kind: holder.0,
                holder_id: holder.1,
                holder_role: holder.2,
                options: &options,
                expires_at,
                note: "",
                source: "page",
                created_by: None,
                at,
            },
        )
        .await?;
        Ok(id)
    }

    /// Names `holder` (`user` or `group`, and its identifier) among the
    /// owners of the node `node`.
    pub async fn name_owner(
        conn: &mut Conn,
        node: Id,
        holder: (&str, Id),
        at: DateTime<Utc>,
    ) -> Result<Id, DbError> {
        add_entry(
            conn,
            &PageKey::new("node", node, "live"),
            "any",
            "access.own",
            (holder.0, Some(holder.1), None),
            None,
            at,
        )
        .await
    }

    /// Stores the row `permission` / `profile` of `page` as it is given.
    pub async fn set_row(
        conn: &mut Conn,
        page: &PageKey,
        profile: &str,
        permission: &str,
        inherit: bool,
        locked: bool,
    ) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO permission_rows (object_kind, object_id, page, profile, permission, inherit, locked)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             ON CONFLICT (object_kind, object_id, page, profile, permission)
             DO UPDATE SET inherit = EXCLUDED.inherit, locked = EXCLUDED.locked, revision = permission_rows.revision + 1",
        )
        .bind(&page.object_kind)
        .bind(page.object_id)
        .bind(&page.page)
        .bind(profile)
        .bind(permission)
        .bind(inherit)
        .bind(locked)
        .execute(conn)
        .await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// The pages of everything: what the requests that are the same for every kind of thing read.
//
// One block, kept together here: the history of a resource, an environment or a document by what
// its lines are about, the people who may know one another, and the resources that have a place
// as their home. Nothing here decides anything.

/// What the requests that are the same for every kind of thing read.
pub mod everything {
    use crate::queries::hierarchy::{AuditCursor, AuditRow};
    use crate::{Conn, DbError};
    use orch_types::Id;

    /// What narrows the history of a thing to some of its lines.
    #[derive(Debug, Clone, Copy, Default)]
    pub struct HistoryFilter<'a> {
        /// Only what this person did.
        pub actor: Option<Id>,
        /// Only lines of these actions, written out in full; none for every line.
        pub actions: Option<&'a [String]>,
        /// Only lines about this thread.
        pub thread: Option<Id>,
        /// Only lines about this file or about anything under this directory.
        pub path: Option<&'a str>,
    }

    /// The lines of the history of a resource, a document or an environment, newest first: the
    /// audit rows that are lines of history and are about the thing `id`, or, for an environment
    /// (`environment` is set), done to it or in it; at most `limit`.
    ///
    /// A row is about the thing when the thing is its object, or when its facts name the thing
    /// as the object: the lines of a page's change are about the entry that was named or taken
    /// out, and say whose page it was in their facts.
    pub async fn history_of(
        conn: &mut Conn,
        id: Id,
        environment: bool,
        filter: &HistoryFilter<'_>,
        after: Option<AuditCursor>,
        limit: i64,
    ) -> Result<Vec<AuditRow>, DbError> {
        let actions: Option<Vec<String>> = filter.actions.map(<[String]>::to_vec);
        Ok(sqlx::query_as(
            "SELECT a.id, a.at, a.actor_kind, a.actor_id, a.action, a.history, a.override, a.object_kind,
                    a.object_id, a.node_id, a.summary, a.details, host(a.ip) AS ip
               FROM audit_log a
              WHERE a.history
                AND (a.object_id = $1 OR a.details -> 'object' ->> 'id' = $1::text
                     OR ($2 AND a.environment_id = $1))
                AND ($3::uuid IS NULL OR a.actor_id = $3)
                AND ($4::text[] IS NULL OR a.action = ANY($4))
                AND ($5::uuid IS NULL OR (a.object_kind = 'thread' AND a.object_id = $5)
                     OR a.details ->> 'thread_id' = $5::text OR a.details ->> 'thread' = $5::text)
                AND ($6::text IS NULL OR a.details ->> 'path' = $6
                     OR starts_with(COALESCE(a.details ->> 'path', ''), $6 || '/')
                     OR EXISTS (SELECT 1 FROM jsonb_array_elements_text(
                                    CASE WHEN jsonb_typeof(a.details -> 'paths') = 'array'
                                         THEN a.details -> 'paths' ELSE '[]'::jsonb END) p
                                 WHERE p = $6 OR starts_with(p, $6 || '/')))
                AND ($7::uuid IS NULL OR (a.at, a.id) < ($8, $7))
              ORDER BY a.at DESC, a.id DESC
              LIMIT $9",
        )
        .bind(id)
        .bind(environment)
        .bind(filter.actor)
        .bind(actions)
        .bind(filter.thread)
        .bind(filter.path)
        .bind(after.map(|cursor| cursor.id))
        .bind(after.map(|cursor| cursor.at))
        .bind(limit)
        .fetch_all(conn)
        .await?)
    }

    /// The pages inside a place, found by what they lie in.
    #[derive(Debug, Clone, Default, PartialEq, Eq)]
    pub struct Inside {
        /// The nodes inside it (it among them), not deleted.
        pub nodes: Vec<Id>,
        /// The resources whose home is a node inside it, deleted ones among them.
        pub resources: Vec<Id>,
        /// The documents beside a node inside it, not deleted.
        pub documents: Vec<Id>,
    }

    /// The nodes, resources and documents that lie inside the place `place` (a node, or the whole
    /// system when none), by identifier, at most `limit` of each kind.
    pub async fn inside(conn: &mut Conn, place: Option<Id>, limit: i64) -> Result<Inside, DbError> {
        let nodes: Vec<Id> = sqlx::query_scalar(
            "SELECT id FROM nodes WHERE deleted_at IS NULL AND ($1::uuid IS NULL OR ($1 = ANY(path) AND id <> $1))
              ORDER BY id LIMIT $2",
        )
        .bind(place)
        .bind(limit)
        .fetch_all(&mut *conn)
        .await?;
        let resources: Vec<Id> = sqlx::query_scalar(
            "SELECT r.id FROM (
                SELECT id, home_id FROM providers UNION ALL SELECT id, home_id FROM agents
                UNION ALL SELECT id, home_node_id FROM environments UNION ALL SELECT id, home_id FROM hosts
                UNION ALL SELECT id, home_id FROM env_templates UNION ALL SELECT id, home_id FROM mcp_servers
                UNION ALL SELECT id, home_id FROM mounts UNION ALL SELECT id, home_id FROM skills) r
              WHERE $1::uuid IS NULL
                 OR r.home_id IN (SELECT id FROM nodes WHERE $1 = ANY(path))
              ORDER BY r.id LIMIT $2",
        )
        .bind(place)
        .bind(limit)
        .fetch_all(&mut *conn)
        .await?;
        let documents: Vec<Id> = sqlx::query_scalar(
            "SELECT id FROM documents
              WHERE deleted_at IS NULL
                AND ($1::uuid IS NULL OR node_id IN (SELECT id FROM nodes WHERE $1 = ANY(path)))
              ORDER BY id LIMIT $2",
        )
        .bind(place)
        .bind(limit)
        .fetch_all(conn)
        .await?;
        Ok(Inside {
            nodes,
            resources,
            documents,
        })
    }

    /// Those of `candidates` who are members of a root org that `person` is a member of: the
    /// people a person may be told of by name, as the search for people offers them.
    pub async fn sharing_a_tree_with(
        conn: &mut Conn,
        person: Id,
        candidates: &[Id],
    ) -> Result<Vec<Id>, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT DISTINCT a.user_id
               FROM org_members a JOIN org_members b ON b.org_id = a.org_id
              WHERE b.user_id = $1 AND a.user_id = ANY($2)
              ORDER BY 1",
        )
        .bind(person)
        .bind(candidates)
        .fetch_all(conn)
        .await?)
    }
}

// ---------------------------------------------------------------- how many people a role stands for (giving as a file or a folder is given)

/// One indexed holder source in a grouped audience read. Explicit Everyone-at-place uses `visible`;
/// a role used by shipped defaults uses `role`, so page-only viewing never becomes an inherited role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HolderPeopleQuery {
    /// `user`, `group`, `members`, `role` or `visible`.
    pub kind: &'static str,
    /// The named identity/place; none means roles anywhere, for system-default chips only.
    pub id: Option<Id>,
    /// The minimum role for `role`, empty for other sources.
    pub role: &'static str,
}

/// All source/person pairs in one recursive query, never a function/statement per chip.
/// The first member is the source's one-based ordinal; every audience is complete before its caller counts or pages it.
pub async fn holder_people_many(
    conn: &mut Conn,
    sources: &[HolderPeopleQuery],
    now: DateTime<Utc>,
) -> Result<Vec<(i64, Id)>, DbError> {
    if sources.is_empty() {
        return Ok(Vec::new());
    }
    let kinds: Vec<_> = sources.iter().map(|source| source.kind).collect();
    let ids: Vec<_> = sources.iter().map(|source| source.id).collect();
    let roles: Vec<_> = sources.iter().map(|source| source.role).collect();
    Ok(sqlx::query_as("SELECT wanted, user_id FROM access_holder_people_many($1,$2,$3,$4,true) ORDER BY wanted,user_id")
        .bind(&kinds).bind(&ids).bind(&roles).bind(now).fetch_all(conn).await?)
}

/// One source of a resource/thread audience; owners additionally have to belong to its root tree.
#[derive(Debug, Clone, Copy)]
pub struct HolderAudienceSource {
    /// The object whose audience this source contributes to.
    pub object: Id,
    /// The distinct holder expanded once for the complete batch.
    pub holder: HolderPeopleQuery,
    /// Required membership for an owner's source, absent for an ordinary page entry.
    pub owner_root: Option<Id>,
}

/// The org path whose suspensions keep a person out of this object's audience.
#[derive(Debug, Clone, Copy)]
pub struct HolderAudiencePath<'a> {
    /// The object.
    pub object: Id,
    /// Its root org and departments.
    pub orgs: &'a [Id],
}

/// Expands all distinct holders once, applies owner membership and per-object suspensions, and
/// returns distinct object/person pairs. One statement for a whole resource list, including its threads.
pub async fn holder_audiences(
    conn: &mut Conn,
    sources: &[HolderAudienceSource],
    paths: &[HolderAudiencePath<'_>],
    now: DateTime<Utc>,
) -> Result<Vec<(Id, Id)>, DbError> {
    if sources.is_empty() {
        return Ok(Vec::new());
    }
    let mut unique = Vec::new();
    let mut slots = std::collections::HashMap::new();
    let mut indices = Vec::with_capacity(sources.len());
    for source in sources {
        let index = *slots.entry(source.holder).or_insert_with(|| {
            unique.push(source.holder);
            i64::try_from(unique.len()).unwrap_or(i64::MAX)
        });
        indices.push(index);
    }
    let kinds: Vec<_> = unique.iter().map(|source| source.kind).collect();
    let ids: Vec<_> = unique.iter().map(|source| source.id).collect();
    let roles: Vec<_> = unique.iter().map(|source| source.role).collect();
    let objects: Vec<_> = sources.iter().map(|source| source.object).collect();
    let roots: Vec<_> = sources.iter().map(|source| source.owner_root).collect();
    let path_objects: Vec<_> = paths
        .iter()
        .flat_map(|path| path.orgs.iter().map(|_| path.object))
        .collect();
    let orgs: Vec<_> = paths
        .iter()
        .flat_map(|path| path.orgs.iter().copied())
        .collect();
    Ok(sqlx::query_as(
        "WITH people AS MATERIALIZED (
           SELECT * FROM access_holder_people_many($1,$2,$3,$9,true)
         ), sources AS (
           SELECT * FROM unnest($4::bigint[],$5::uuid[],$6::uuid[]) AS s(wanted,object,root)
         ), paths AS (
           SELECT * FROM unnest($7::uuid[],$8::uuid[]) AS p(object,org)
         ) SELECT DISTINCT s.object,h.user_id FROM sources s JOIN people h ON h.wanted=s.wanted
            WHERE (s.root IS NULL OR EXISTS (SELECT 1 FROM org_members m WHERE m.org_id=s.root AND m.user_id=h.user_id))
              AND NOT EXISTS (SELECT 1 FROM paths p JOIN org_suspensions x ON x.org_id=p.org
                               WHERE p.object=s.object AND x.user_id=h.user_id)
            ORDER BY s.object,h.user_id",
    ).bind(&kinds).bind(&ids).bind(&roles).bind(&indices).bind(&objects).bind(&roots)
        .bind(&path_objects).bind(&orgs).bind(now).fetch_all(conn).await?)
}

/// Counts the people behind role chips: it yields each chip's index and head-count. A chip is the people who
/// hold its role or a higher one there or above: given it, through a group or as a member of an
/// org it was given to; the owners of those places who are members of the root org, who count as
/// Admins; and the system's admins, who are Admin above every org. Only people who are not
/// suspended altogether are counted, each once.
macro_rules! head_counts {
    () => {
            "WITH chips AS MATERIALIZED (
                SELECT c.role, c.place, c.viewers, c.idx
                  FROM unnest($1::text[], $2::uuid[], $4::boolean[]) WITH ORDINALITY AS c(role, place, viewers, idx)
            ), people AS MATERIALIZED (
                SELECT h.wanted AS idx, h.user_id FROM access_holder_people_many(
                  ARRAY(SELECT CASE WHEN role='viewer' AND viewers THEN 'visible' ELSE 'role' END FROM chips ORDER BY idx),
                  ARRAY(SELECT place FROM chips ORDER BY idx), ARRAY(SELECT role FROM chips ORDER BY idx), $3, true) h
            )
            SELECT c.idx, count(DISTINCT p.user_id) FILTER (WHERE u.status = 'active')
              FROM chips c
              LEFT JOIN people p ON p.idx = c.idx
              LEFT JOIN users u ON u.id = p.user_id
             GROUP BY c.idx ORDER BY c.idx"
    };
}

/// How many people hold each of the roles `chips` names, in the order they are given: a chip is a
/// role and the place it is held in, so that the people are those who hold that role or a higher
/// one there or in a place above it; a chip with no place (on the defaults of the system) counts
/// the people who hold the role anywhere. One grouped recursion/statement for every chip of the list.
pub async fn role_head_counts(
    conn: &mut Conn,
    chips: &[(String, Option<Id>, bool)],
    now: DateTime<Utc>,
) -> Result<Vec<i64>, DbError> {
    let mut counts = vec![0_i64; chips.len()];
    if !chips.is_empty() {
        let roles: Vec<_> = chips.iter().map(|chip| chip.0.as_str()).collect();
        let places: Vec<_> = chips.iter().map(|chip| chip.1).collect();
        let found: Vec<(i64, i64)> = sqlx::query_as(head_counts!())
            .bind(&roles)
            .bind(&places)
            .bind(now)
            .bind(chips.iter().map(|chip| chip.2).collect::<Vec<_>>())
            .fetch_all(&mut *conn)
            .await?;
        for (idx, count) in found {
            if let Some(at) = usize::try_from(idx - 1)
                .ok()
                .and_then(|at| counts.get_mut(at))
            {
                *at = count;
            }
        }
    }
    Ok(counts)
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;
    use crate::queries::hierarchy::testing::insert_node;
    use crate::queries::identity::testing::{insert_user, suspend};
    use chrono::Duration;
    use orch_testkit::{clock, database::TestDb};
    use orch_types::new_id;

    struct World {
        ada: Id,
        bo: Id,
        root: Id,
        department: Id,
        project: Id,
    }

    async fn world(conn: &mut Conn) -> World {
        let (ada, bo) = (new_id(), new_id());
        insert_user(conn, ada, "ada").await.unwrap();
        insert_user(conn, bo, "bob").await.unwrap();
        let (root, department, project) = (new_id(), new_id(), new_id());
        insert_node(conn, root, "org", None, "Acme", ada)
            .await
            .unwrap();
        insert_node(conn, department, "org", Some(root), "Research", ada)
            .await
            .unwrap();
        insert_node(conn, project, "project", Some(department), "Web", ada)
            .await
            .unwrap();
        World {
            ada,
            bo,
            root,
            department,
            project,
        }
    }

    fn live(node: Id) -> PageKey {
        PageKey::new("node", node, "live")
    }

    async fn holder_graph_matches_initial_function(conn: &mut Conn, at: DateTime<Utc>) {
        // Keep the original function as an independent oracle on this fixture's database.
        // Only its name changes; its first definition is kept beside this file.
        let initial = include_str!("access_oracles/access_holders_initial.sql");
        let definition = initial
            .split_once("CREATE FUNCTION access_holders(")
            .unwrap()
            .1
            .split_once("\n$$;")
            .unwrap()
            .0;
        let baseline = sqlx::AssertSqlSafe(format!(
            "CREATE OR REPLACE FUNCTION access_holders_initial({definition}\n$$;"
        ));
        sqlx::raw_sql(baseline).execute(&mut *conn).await.unwrap();
        let people: Vec<Id> = sqlx::query_scalar("SELECT id FROM users ORDER BY id")
            .fetch_all(&mut *conn)
            .await
            .unwrap();
        for person in people {
            for system in [false, true] {
                let expected: Vec<(String, Id, String)> = sqlx::query_as(
                    "SELECT kind,id,role FROM access_holders_initial($1,$2,$3) ORDER BY kind,id,role",
                )
                .bind(person)
                .bind(at)
                .bind(system)
                .fetch_all(&mut *conn)
                .await
                .unwrap();
                let actual: Vec<(String, Id, String)> = sqlx::query_as(
                    "SELECT kind,id,role FROM access_holders($1,$2,$3) ORDER BY kind,id,role",
                )
                .bind(person)
                .bind(at)
                .bind(system)
                .fetch_all(&mut *conn)
                .await
                .unwrap();
                assert_eq!(
                    actual, expected,
                    "the optimized holder graph preserves every identity: person={person}, system={system}, at={at}"
                );
                let groups: Vec<_> = expected
                    .iter()
                    .filter(|(kind, _, _)| kind == "group")
                    .map(|(_, id, _)| *id)
                    .collect();
                assert_eq!(
                    effective_groups(conn, person, at, system).await.unwrap(),
                    groups,
                    "the public group lookup preserves identifier order"
                );
                let sets: Vec<_> = expected
                    .iter()
                    .filter(|(kind, _, _)| matches!(kind.as_str(), "group" | "visible"))
                    .map(|(kind, id, _)| (kind.clone(), *id))
                    .collect();
                assert_eq!(
                    effective_holder_sets(conn, person, at, system)
                        .await
                        .unwrap(),
                    sets,
                    "the public holder lookup preserves kind and identifier order"
                );
            }
        }
    }

    async fn install_previous_holder_people(conn: &mut Conn) {
        let previous = include_str!("access_oracles/access_holder_people_previous.sql");
        // Preserve the installed reverse graph as a second independent oracle.
        // Its historical definition changes only its name in this fixture.
        let previous = previous.replacen(
            "FUNCTION access_holder_people_many(",
            "FUNCTION access_holder_people_many_previous(",
            1,
        );
        sqlx::raw_sql(sqlx::AssertSqlSafe(previous))
            .execute(&mut *conn)
            .await
            .unwrap();
    }

    async fn reverse_matches_forward(
        conn: &mut Conn,
        cases: &[(&str, Id, &str)],
        at: DateTime<Utc>,
    ) {
        holder_graph_matches_initial_function(conn, at).await;
        install_previous_holder_people(conn).await;
        for system in [false, true] {
            let kinds: Vec<_> = cases.iter().map(|case| case.0).collect();
            let ids: Vec<_> = cases.iter().map(|case| Some(case.1)).collect();
            let roles: Vec<_> = cases.iter().map(|case| case.2).collect();
            let grouped: Vec<(i64, Id)> = sqlx::query_as("SELECT wanted,user_id FROM access_holder_people_many($1,$2,$3,$4,$5) ORDER BY wanted,user_id")
                .bind(&kinds).bind(&ids).bind(&roles).bind(at).bind(system).fetch_all(&mut *conn).await.unwrap();
            let previous: Vec<(i64, Id)> = sqlx::query_as("SELECT wanted,user_id FROM access_holder_people_many_previous($1,$2,$3,$4,$5) ORDER BY wanted,user_id")
                .bind(&kinds).bind(&ids).bind(&roles).bind(at).bind(system).fetch_all(&mut *conn).await.unwrap();
            assert_eq!(
                grouped, previous,
                "the replacement preserves every requested slot and person, system={system}, at={at}"
            );
            let people: Vec<Id> = sqlx::query_scalar("SELECT id FROM users ORDER BY id")
                .fetch_all(&mut *conn)
                .await
                .unwrap();
            // Expanding only some people gives exactly what the whole expansion gives for them: every other person,
            // every second person, and nobody.
            for chosen in [
                people.iter().copied().step_by(2).collect::<Vec<_>>(),
                people
                    .iter()
                    .copied()
                    .skip(1)
                    .step_by(2)
                    .collect::<Vec<_>>(),
                Vec::new(),
            ] {
                let among: Vec<(i64, Id)> = sqlx::query_as("SELECT wanted,user_id FROM access_holder_people_many($1,$2,$3,$4,$5,$6) ORDER BY wanted,user_id")
                    .bind(&kinds).bind(&ids).bind(&roles).bind(at).bind(system).bind(&chosen).fetch_all(&mut *conn).await.unwrap();
                assert_eq!(
                    among,
                    grouped
                        .iter()
                        .copied()
                        .filter(|(_, person)| chosen.contains(person))
                        .collect::<Vec<_>>(),
                    "a candidate list narrows the people and changes nothing else, system={system}, at={at}"
                );
            }
            let mut extra = vec![
                ("role", None, "viewer"),
                ("role", None, "interactor"),
                ("role", None, "developer"),
                ("role", None, "admin"),
                ("role", None, "invalid"),
                ("visible", None, ""),
                ("group", Some(new_id()), ""),
                ("user", Some(new_id()), ""),
                ("all", None, ""),
                ("all", Some(new_id()), ""),
            ];
            extra.extend(people.into_iter().map(|person| ("user", Some(person), "")));
            if let Some(&(kind, id, role)) = cases.first() {
                extra.extend([(kind, Some(id), role), (kind, Some(id), role)]);
            }
            let kinds: Vec<_> = extra.iter().map(|case| case.0).collect();
            let ids: Vec<_> = extra.iter().map(|case| case.1).collect();
            let roles: Vec<_> = extra.iter().map(|case| case.2).collect();
            for (kinds, ids, roles) in [(kinds, ids, roles), (vec![], vec![], vec![])] {
                let previous: Vec<(i64, Id)> = sqlx::query_as("SELECT wanted,user_id FROM access_holder_people_many_previous($1,$2,$3,$4,$5) ORDER BY wanted,user_id")
                    .bind(&kinds).bind(&ids).bind(&roles).bind(at).bind(system).fetch_all(&mut *conn).await.unwrap();
                let actual: Vec<(i64, Id)> = sqlx::query_as("SELECT wanted,user_id FROM access_holder_people_many($1,$2,$3,$4,$5) ORDER BY wanted,user_id")
                    .bind(&kinds).bind(&ids).bind(&roles).bind(at).bind(system).fetch_all(&mut *conn).await.unwrap();
                assert_eq!(
                    actual, previous,
                    "unplaced, direct, repeated, missing and empty requests preserve every person and slot"
                );
            }
            for (index, &(kind, id, role)) in cases.iter().enumerate() {
                // The old per-person direction is an oracle only in this small
                // correctness fixture, never in a production audience query.
                let expected: Vec<Id> = sqlx::query_scalar(
                    "SELECT u.id FROM users u WHERE EXISTS (
                       SELECT 1 FROM access_holders(u.id, $4, $5) h
                        WHERE h.kind = $1 AND h.id = $2 AND ($1 <> 'role' OR
                          array_position(ARRAY['viewer','interactor','developer','admin'], h.role)
                          >= array_position(ARRAY['viewer','interactor','developer','admin'], $3)))
                     ORDER BY u.id",
                )
                .bind(kind)
                .bind(id)
                .bind(role)
                .bind(at)
                .bind(system)
                .fetch_all(&mut *conn)
                .await
                .unwrap();
                let actual: Vec<Id> = sqlx::query_scalar(
                    "SELECT user_id FROM access_holder_people($1, $2, $3, $4, $5) ORDER BY user_id",
                )
                .bind(kind)
                .bind(id)
                .bind(role)
                .bind(at)
                .bind(system)
                .fetch_all(&mut *conn)
                .await
                .unwrap();
                assert_eq!(
                    actual, expected,
                    "{kind} {id} {role}, system={system}, at={at}"
                );
                let ordinal = i64::try_from(index + 1).unwrap();
                let batch: Vec<_> = grouped
                    .iter()
                    .filter(|(slot, _)| *slot == ordinal)
                    .map(|(_, user)| *user)
                    .collect();
                assert_eq!(
                    batch, expected,
                    "a grouped source has exactly the same active audience, in its own slot"
                );
            }
        }
    }

    #[tokio::test]
    async fn placed_unplaced_and_empty_role_chips_keep_their_own_counts_in_one_batch() {
        let db = TestDb::fresh().await;
        let mut conn = db.pool.acquire().await.unwrap();
        let w = world(&mut conn).await;
        let at = clock::start();
        insert_member(&mut conn, w.root, w.ada, w.root)
            .await
            .unwrap();
        insert_member(&mut conn, w.root, w.bo, w.department)
            .await
            .unwrap();
        give_role(
            &mut conn,
            w.department,
            "user",
            w.bo,
            "developer",
            w.ada,
            None,
        )
        .await
        .unwrap();
        let missing = new_id();
        let chips = vec![
            ("developer".to_owned(), Some(w.project), true),
            ("viewer".to_owned(), Some(w.project), false),
            ("admin".to_owned(), None, false),
            ("viewer".to_owned(), Some(missing), true),
        ];
        let counts = role_head_counts(&mut conn, &chips, at).await.unwrap();
        for (index, (role, place, viewers)) in chips.iter().enumerate() {
            let kind = if role == "viewer" && *viewers {
                "visible"
            } else {
                "role"
            };
            let expected: i64 = sqlx::query_scalar(
                "SELECT count(DISTINCT user_id) FROM access_holder_people($1,$2,$3,$4,true)",
            )
            .bind(kind)
            .bind(place)
            .bind(role)
            .bind(at)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
            assert_eq!(counts[index], expected);
        }
        assert_eq!(counts[3], 0);
    }

    #[tokio::test]
    async fn expanding_from_a_holder_preserves_domains_nested_groups_roles_cuts_and_expiry() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let w = world(&mut conn).await;
        let at = clock::start();
        let outside = new_id();
        insert_user(&mut conn, outside, "outside").await.unwrap();
        let (topic, thread) = (new_id(), new_id());
        insert_node(&mut conn, topic, "topic", Some(w.project), "Topic", w.ada)
            .await
            .unwrap();
        insert_node(&mut conn, thread, "thread", Some(topic), "Thread", w.ada)
            .await
            .unwrap();
        // A thread's own page is deliberately outside the place holder graph.
        name_owner(&mut conn, thread, ("user", outside), at)
            .await
            .unwrap();
        add_entry(
            &mut conn,
            &live(thread),
            "any",
            "thread.view",
            ("user", Some(outside), None),
            None,
            at,
        )
        .await
        .unwrap();
        insert_member(&mut conn, w.root, w.ada, w.root)
            .await
            .unwrap();
        insert_member(&mut conn, w.root, w.bo, w.department)
            .await
            .unwrap();
        make_system_admin(&mut conn, w.ada).await.unwrap();
        let inner = insert_group(&mut conn, w.department, "Inside", w.ada)
            .await
            .unwrap();
        let outer = insert_group(&mut conn, w.root, "Outer", w.ada)
            .await
            .unwrap();
        let cycle = insert_group(&mut conn, w.root, "Circular role", w.ada)
            .await
            .unwrap();
        add_to_group(&mut conn, inner, w.bo).await.unwrap();
        sqlx::query("INSERT INTO group_members (group_id, member_group_id) VALUES ($1, $2)")
            .bind(outer)
            .bind(inner)
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query("INSERT INTO group_members (group_id, member_node_id, member_role) VALUES ($1, $2, 'developer')")
            .bind(cycle).bind(w.department).execute(&mut *conn).await.unwrap();
        give_role(
            &mut conn,
            w.department,
            "group",
            cycle,
            "developer",
            w.ada,
            None,
        )
        .await
        .unwrap();
        give_role(
            &mut conn,
            w.department,
            "group",
            outer,
            "developer",
            w.ada,
            Some(at + Duration::hours(1)),
        )
        .await
        .unwrap();
        name_owner(&mut conn, w.project, ("group", outer), at)
            .await
            .unwrap();
        let system = PageKey::new("system", SYSTEM_ID, "defaults");
        for permission in ["org.view", "project.view"] {
            add_entry(
                &mut conn,
                &system,
                "any",
                permission,
                ("role", None, Some("viewer")),
                None,
                at,
            )
            .await
            .unwrap();
        }
        add_entry(
            &mut conn,
            &live(w.root),
            "any",
            "chat.view",
            ("user", Some(outside), None),
            None,
            at,
        )
        .await
        .unwrap();
        let creation = add_entry(
            &mut conn,
            &live(w.department),
            "any",
            "org.view",
            ("user", Some(outside), None),
            None,
            at,
        )
        .await
        .unwrap();
        sqlx::query("UPDATE permission_entries SET source = 'creation' WHERE id = $1")
            .bind(creation)
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query("INSERT INTO node_roles (id, node_id, subject_kind, subject_id, subject_role, role, created_by)
                     VALUES ($1, $2, 'role', $3, 'viewer', 'interactor', $4)")
            .bind(new_id()).bind(w.project).bind(w.department).bind(w.ada).execute(&mut *conn).await.unwrap();
        let by_domain = insert_group(&mut conn, w.root, "Department members", w.ada)
            .await
            .unwrap();
        sqlx::query("INSERT INTO group_members (group_id, member_org_id) VALUES ($1, $2)")
            .bind(by_domain)
            .bind(w.department)
            .execute(&mut *conn)
            .await
            .unwrap();
        let cases = [
            ("group", outer, ""),
            ("group", cycle, ""),
            ("group", by_domain, ""),
            ("members", w.department, ""),
            ("members", w.root, ""),
            ("role", w.project, "viewer"),
            ("role", w.project, "developer"),
            ("role", w.project, "admin"),
            ("visible", w.department, ""),
            ("visible", w.project, ""),
            ("reachable", w.department, ""),
        ];
        assert_eq!(
            group_people(&mut conn, &[outer], 20, at).await.unwrap(),
            [(outer, w.bo)]
        );
        reverse_matches_forward(&mut conn, &cases, at).await;
        for roles in [
            vec!["developer".to_owned()],
            vec!["viewer".to_owned(), "admin".to_owned()],
            vec![
                "interactor".to_owned(),
                "developer".to_owned(),
                "admin".to_owned(),
            ],
        ] {
            let expected: Vec<(Id, Id)> = sqlx::query_as(
                "SELECT h.id, u.id FROM users u CROSS JOIN LATERAL access_holders(u.id, $2, true) h
                  WHERE h.kind = 'role' AND h.id = $1 AND h.role = ANY($3) ORDER BY h.id, u.id",
            )
            .bind(w.project)
            .bind(at)
            .bind(&roles)
            .fetch_all(&mut *conn)
            .await
            .unwrap();
            assert_eq!(
                people_in_roles(&mut conn, &[w.project], &roles, at, 20, false)
                    .await
                    .unwrap(),
                expected,
                "highest actual role, including gaps in the requested role list: {roles:?}"
            );
        }

        set_domain(&mut conn, w.root, w.bo, w.root, w.ada, at)
            .await
            .unwrap();
        suspend_in(&mut conn, w.department, w.bo, w.ada)
            .await
            .unwrap();
        set_row(
            &mut conn,
            &live(w.department),
            "any",
            "org.view",
            false,
            false,
        )
        .await
        .unwrap();
        reverse_matches_forward(&mut conn, &cases, at).await;
        lift_suspension(&mut conn, w.department, w.bo)
            .await
            .unwrap();
        suspend(&mut conn, outside).await.unwrap();
        set_row(
            &mut conn,
            &PageKey::new("node", w.root, "defaults"),
            "any",
            "org.view",
            true,
            true,
        )
        .await
        .unwrap();
        reverse_matches_forward(&mut conn, &cases, at + Duration::hours(2)).await;
        sqlx::query("UPDATE nodes SET deleted_at = $2 WHERE id = $1")
            .bind(w.department)
            .bind(at)
            .execute(&mut *conn)
            .await
            .unwrap();
        reverse_matches_forward(&mut conn, &cases, at + Duration::hours(2)).await;
        sqlx::query("UPDATE nodes SET deleted_at = NULL WHERE id = $1")
            .bind(w.department)
            .execute(&mut *conn)
            .await
            .unwrap();
        reverse_matches_forward(&mut conn, &cases, at + Duration::hours(2)).await;
    }

    fn under_a_hundred_ms(elapsed: std::time::Duration, query: &str) {
        eprintln!("{query}: {elapsed:?}; budget 100 ms");
        assert!(
            elapsed < std::time::Duration::from_millis(100),
            "{query}: {elapsed:?}; the 1,000-person latency budget is 100 ms"
        );
    }

    // Opt-in diagnosis on this exact live fixture, after its real query was timed.
    // The SQL files are immutable QA inputs; no migration/function body is installed.
    async fn audience_role_plans(
        conn: &mut Conn,
        project: Id,
        at: DateTime<Utc>,
        elapsed: std::time::Duration,
        rows: usize,
    ) {
        use sqlx::Row;
        let Some(output) = std::env::var_os("ORCH_AUDIENCE_PLAN_OUT") else {
            return;
        };
        let output = std::path::PathBuf::from(output);
        let inputs = std::path::PathBuf::from(
            std::env::var_os("ORCH_AUDIENCE_PLAN_INPUTS")
                .expect("an opt-in audience diagnostic names its frozen SQL input directory"),
        );
        std::fs::create_dir_all(&output).unwrap();
        let measurement = serde_json::json!({"query":"people_in_roles","elapsed_ns":elapsed.as_nanos(),
            "budget_ns":100_000_000,"rows":rows,"sampled_at":at,"project_id":project,
            "plan_order":["canonical","candidate"],"timing_precedes_every_explain":true});
        std::fs::write(
            output.join("measurement.json"),
            serde_json::to_vec_pretty(&measurement).unwrap(),
        )
        .unwrap();
        for name in ["canonical", "candidate"] {
            let sql = std::fs::read_to_string(inputs.join(format!("{name}-role-walk-explain.sql")))
                .unwrap();
            assert!(sql.contains("BEGIN READ ONLY;"));
            assert!(sql.trim_end().ends_with("ROLLBACK;"));
            assert_eq!(sql.matches(":'project_id'").count(), 1);
            assert_eq!(sql.matches(":'sampled_at'").count(), 1);
            // These are typed fixture UUID/time literals, never shell text or arbitrary user values.
            let sql = sql
                .replace(":'project_id'", &format!("'{project}'"))
                .replace(":'sampled_at'", &format!("'{}'", at.to_rfc3339()));
            let plans = sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
                .fetch_all(&mut *conn)
                .await
                .unwrap();
            assert_eq!(plans.len(), 1, "one JSON plan from the read-only script");
            let plan: Json = plans[0].try_get(0).unwrap();
            std::fs::write(
                output.join(format!("{name}-plan.json")),
                serde_json::to_vec_pretty(&plan).unwrap(),
            )
            .unwrap();
        }
    }

    // Opt-in evidence comes after the real timed call; the latency assertion below still
    // checks that original interval. Additional reads are diagnostics, never acceptance.
    async fn role_head_profile(
        conn: &mut Conn,
        pool: &sqlx::PgPool,
        label: &str,
        chips: &[(String, Option<Id>, bool)],
        at: DateTime<Utc>,
        first_elapsed: std::time::Duration,
        counts: &[i64],
    ) {
        use sqlx::Connection as _;
        let Some(output) = std::env::var_os("ORCH_ROLE_HEAD_PROFILE_OUT") else {
            return;
        };
        let output = std::path::PathBuf::from(output).join(label);
        std::fs::create_dir_all(&output).unwrap();
        let roles: Vec<_> = chips.iter().map(|chip| chip.0.as_str()).collect();
        let places: Vec<_> = chips.iter().map(|chip| chip.1).collect();
        let viewers: Vec<_> = chips.iter().map(|chip| chip.2).collect();
        let kinds: Vec<_> = chips
            .iter()
            .map(|chip| {
                if chip.0 == "viewer" && chip.2 {
                    "visible"
                } else {
                    "role"
                }
            })
            .collect();
        let options = pool.connect_options();
        let mut fresh = sqlx::PgConnection::connect_with(&options).await.unwrap();
        let cold_start = std::time::Instant::now();
        let cold_counts = role_head_counts(&mut fresh, chips, at).await.unwrap();
        let cold_elapsed = cold_start.elapsed();
        assert_eq!(cold_counts, counts);
        // Five repetitions expose scheduling variance without turning a diagnostic into load.
        const WARM_REPETITIONS: usize = 5;
        let mut warm_ns = Vec::new();
        for _ in 0..WARM_REPETITIONS {
            let start = std::time::Instant::now();
            let actual = role_head_counts(&mut fresh, chips, at).await.unwrap();
            warm_ns.push(start.elapsed().as_nanos());
            assert_eq!(actual, counts);
        }
        let measurement = serde_json::json!({
            "query": "role_head_counts", "original_elapsed_ns": first_elapsed.as_nanos(),
            "fresh_backend_elapsed_ns": cold_elapsed.as_nanos(), "warm_elapsed_ns": warm_ns,
            "counts": counts, "project_id": chips[0].1, "sampled_at": at,
            "cold_definition": "first query and function plan in a newly connected backend; fixture buffers may already be warm",
            "loadavg": std::fs::read_to_string("/proc/loadavg").unwrap_or_default(),
            "timing_precedes_every_explain": true
        });
        std::fs::write(
            output.join("measurement.json"),
            serde_json::to_vec_pretty(&measurement).unwrap(),
        )
        .unwrap();
        let outer: Json = sqlx::query_scalar(concat!(
            "EXPLAIN (ANALYZE, BUFFERS, SETTINGS, FORMAT JSON) ",
            head_counts!()
        ))
        .bind(&roles)
        .bind(&places)
        .bind(at)
        .bind(&viewers)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        std::fs::write(
            output.join("outer-plan.json"),
            serde_json::to_vec_pretty(&outer).unwrap(),
        )
        .unwrap();
        let definition: String = sqlx::query_scalar(
            "SELECT pg_get_functiondef('access_holder_people_many(text[],uuid[],text[],timestamptz,boolean,uuid[])'::regprocedure)",
        ).fetch_one(&mut *conn).await.unwrap();
        let body = definition
            .split_once("RETURN QUERY\n")
            .unwrap()
            .1
            .rsplit_once("\nEND")
            .unwrap()
            .0
            .trim()
            .trim_end_matches(';')
            .replace("wanted_kinds", "$1")
            .replace("wanted_ids", "$2")
            .replace("minimum_roles", "$3")
            .replace("sampled_at", "$4")
            .replace("AND system_admin AND", "AND $5 AND");
        std::fs::write(output.join("inner-query.sql"), &body).unwrap();
        sqlx::query("BEGIN READ ONLY")
            .execute(&mut fresh)
            .await
            .unwrap();
        sqlx::query("SET LOCAL jit = off")
            .execute(&mut fresh)
            .await
            .unwrap();
        sqlx::query("SET LOCAL plan_cache_mode = force_generic_plan")
            .execute(&mut fresh)
            .await
            .unwrap();
        // The function's stored body is the trusted schema definition on this fixture only.
        let explain = sqlx::AssertSqlSafe(format!(
            "EXPLAIN (ANALYZE, BUFFERS, SETTINGS, FORMAT JSON) {body}"
        ));
        let inner: Json = sqlx::query_scalar(explain)
            .bind(&kinds)
            .bind(&places)
            .bind(&roles)
            .bind(at)
            .bind(true)
            .fetch_one(&mut fresh)
            .await
            .unwrap();
        sqlx::query("ROLLBACK").execute(&mut fresh).await.unwrap();
        std::fs::write(
            output.join("inner-plan.json"),
            serde_json::to_vec_pretty(&inner).unwrap(),
        )
        .unwrap();
        install_previous_holder_people(conn).await;
        let previous_sql = head_counts!().replace(
            "access_holder_people_many(",
            "access_holder_people_many_previous(",
        );
        let mut previous_conn = sqlx::PgConnection::connect_with(&options).await.unwrap();
        let expected: Vec<_> = counts
            .iter()
            .enumerate()
            .map(|(index, count)| (i64::try_from(index + 1).unwrap(), *count))
            .collect();
        let mut previous_ns = Vec::new();
        for _ in 0..=WARM_REPETITIONS {
            let start = std::time::Instant::now();
            let actual: Vec<(i64, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(previous_sql.clone()))
                .bind(&roles)
                .bind(&places)
                .bind(at)
                .bind(&viewers)
                .fetch_all(&mut previous_conn)
                .await
                .unwrap();
            previous_ns.push(start.elapsed().as_nanos());
            assert_eq!(actual, expected);
        }
        let previous_measurement = serde_json::json!({
            "query": "role_head_counts", "counts": counts,
            "fresh_backend_elapsed_ns": previous_ns[0], "warm_elapsed_ns": previous_ns[1..],
            "sampled_at": at, "project_id": chips[0].1,
            "cold_definition": "first query and function plan in a newly connected backend; fixture buffers may already be warm",
            "loadavg": std::fs::read_to_string("/proc/loadavg").unwrap_or_default(),
            "definition": "unchanged historical reverse graph under a fixture-only name"
        });
        std::fs::write(
            output.join("previous-measurement.json"),
            serde_json::to_vec_pretty(&previous_measurement).unwrap(),
        )
        .unwrap();
        let previous_explain = sqlx::AssertSqlSafe(format!(
            "EXPLAIN (ANALYZE, BUFFERS, SETTINGS, FORMAT JSON) {previous_sql}"
        ));
        let previous_outer: Json = sqlx::query_scalar(previous_explain)
            .bind(&roles)
            .bind(&places)
            .bind(at)
            .bind(&viewers)
            .fetch_one(&mut previous_conn)
            .await
            .unwrap();
        std::fs::write(
            output.join("previous-outer-plan.json"),
            serde_json::to_vec_pretty(&previous_outer).unwrap(),
        )
        .unwrap();
        eprintln!("role-head diagnostic: {measurement}");
    }

    #[tokio::test]
    async fn all_audience_queries_stay_below_a_hundred_ms_with_a_thousand_people() {
        use crate::queries::chats;
        use crate::usage::caps;
        use std::time::Instant;
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let w = world(&mut conn).await;
        let at = clock::start();
        // The owner's measured size, not a page limit: every source must return
        // or count all 1,000 people before any cursor/limit is applied.
        let people: Vec<Id> = (0..1_000).map(|_| new_id()).collect();
        sqlx::query(
            "INSERT INTO users (id, username, display_name, avatar_color)
                     SELECT id, 'load' || ord, 'Person ' || ord, 0
                       FROM unnest($1::uuid[]) WITH ORDINALITY AS p(id, ord)",
        )
        .bind(&people)
        .execute(&mut *conn)
        .await
        .unwrap();
        // Include the unrelated personal trees a real installation also has:
        // a holder lookup in this team's tree must not traverse all of them.
        let personal: Vec<Id> = people.iter().map(|_| new_id()).collect();
        sqlx::query(
            "INSERT INTO nodes (id, kind, org_id, path, name, owner_id, created_by)
                     SELECT id, 'org', id, ARRAY[id], 'Personal ' || ord, owner, owner
                       FROM unnest($1::uuid[], $2::uuid[]) WITH ORDINALITY AS p(id, owner, ord)",
        )
        .bind(&personal)
        .bind(&people)
        .execute(&mut *conn)
        .await
        .unwrap();
        // As in create_user, associate each personal root before claiming the
        // account's public name for its organization side row.
        sqlx::query(
            "UPDATE users u SET personal_org_id = p.org
                      FROM unnest($1::uuid[], $2::uuid[]) AS p(org, person) WHERE u.id = p.person",
        )
        .bind(&personal)
        .bind(&people)
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO orgs (node_id, slug, org_kind, closed, flat)
                     SELECT id, 'load' || ord, 'personal', true, true
                       FROM unnest($1::uuid[]) WITH ORDINALITY AS p(id, ord)",
        )
        .bind(&personal)
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO org_members (org_id, user_id, domain_id)
                     SELECT org, person, org FROM unnest($1::uuid[], $2::uuid[]) AS p(org, person)",
        )
        .bind(&personal)
        .bind(&people)
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query("INSERT INTO permission_entries (id, object_kind, object_id, page, permission, holder_kind, holder_id, created_by, source)
                     SELECT gen_random_uuid(), 'node', org, 'live', 'access.own', 'user', person, person, 'creation'
                       FROM unnest($1::uuid[], $2::uuid[]) AS p(org, person)")
            .bind(&personal).bind(&people).execute(&mut *conn).await.unwrap();
        sqlx::query(
            "INSERT INTO chats (id, kind, node_id, org_id, created_by)
                     SELECT gen_random_uuid(), 'node', org, org, person
                       FROM unnest($1::uuid[], $2::uuid[]) AS p(org, person)",
        )
        .bind(&personal)
        .bind(&people)
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query("INSERT INTO org_members (org_id, user_id, domain_id) SELECT $1, id, $2 FROM unnest($3::uuid[]) AS p(id)")
            .bind(w.root).bind(w.department).bind(&people).execute(&mut *conn).await.unwrap();
        let inner = insert_group(&mut conn, w.root, "Thousand people", w.ada)
            .await
            .unwrap();
        let outer = insert_group(&mut conn, w.root, "Nested thousand", w.ada)
            .await
            .unwrap();
        sqlx::query("INSERT INTO group_members (group_id, user_id) SELECT $1, id FROM unnest($2::uuid[]) AS p(id)")
            .bind(inner).bind(&people).execute(&mut *conn).await.unwrap();
        sqlx::query("INSERT INTO group_members (group_id, member_group_id) VALUES ($1, $2)")
            .bind(outer)
            .bind(inner)
            .execute(&mut *conn)
            .await
            .unwrap();
        give_role(
            &mut conn,
            w.department,
            "group",
            outer,
            "developer",
            w.ada,
            None,
        )
        .await
        .unwrap();
        name_owner(&mut conn, w.project, ("group", outer), at)
            .await
            .unwrap();
        let system = PageKey::new("system", SYSTEM_ID, "defaults");
        add_entry(
            &mut conn,
            &system,
            "any",
            "project.view",
            ("role", None, Some("viewer")),
            None,
            at,
        )
        .await
        .unwrap();
        let topic = new_id();
        insert_node(
            &mut conn,
            topic,
            "topic",
            Some(w.project),
            "Load topic",
            w.ada,
        )
        .await
        .unwrap();
        let thread = new_id();
        insert_node(
            &mut conn,
            thread,
            "thread",
            Some(topic),
            "Load thread",
            w.ada,
        )
        .await
        .unwrap();
        name_owner(&mut conn, thread, ("group", outer), at)
            .await
            .unwrap();
        let chat = chats::chat_of_node(&mut conn, thread)
            .await
            .unwrap()
            .unwrap()
            .id;
        let provider = new_id();
        sqlx::query("INSERT INTO usage_daily (day, org_id, project_id, topic_id, thread_id, provider_id, user_id, model, total_tokens)
                     SELECT $1, $2, $3, $4, $5, $6, id, 'fixture', 1 FROM unnest($7::uuid[]) AS p(id)")
            .bind(at.date_naive()).bind(w.root).bind(w.project).bind(topic).bind(thread).bind(provider).bind(&people)
            .execute(&mut *conn).await.unwrap();

        let start = Instant::now();
        let groups = group_people(&mut conn, &[outer], 2_000, at).await.unwrap();
        under_a_hundred_ms(start.elapsed(), "group_people");
        assert_eq!(groups.len(), 1_000);
        let roles = vec![
            "viewer".to_owned(),
            "interactor".to_owned(),
            "developer".to_owned(),
            "admin".to_owned(),
        ];
        let start = Instant::now();
        let named = people_in_roles(&mut conn, &[w.project], &roles, at, 2_000, false)
            .await
            .unwrap();
        let elapsed = start.elapsed();
        audience_role_plans(&mut conn, w.project, at, elapsed, named.len()).await;
        under_a_hundred_ms(elapsed, "people_in_roles");
        assert_eq!(named.len(), 1_000);
        let start = Instant::now();
        let admins = admin_people(&mut conn, &[w.project], at, false)
            .await
            .unwrap();
        under_a_hundred_ms(start.elapsed(), "admin_people");
        assert_eq!(admins.len(), 1_000);
        let chips = [
            ("viewer".to_owned(), Some(w.project), true),
            ("admin".to_owned(), Some(w.project), false),
        ];
        let start = Instant::now();
        let counts = role_head_counts(&mut conn, &chips, at).await.unwrap();
        let elapsed = start.elapsed();
        role_head_profile(
            &mut conn, &test.pool, "placed", &chips, at, elapsed, &counts,
        )
        .await;
        under_a_hundred_ms(elapsed, "role_head_counts");
        assert_eq!(counts, [1_000, 1_000]);
        let start = Instant::now();
        let unplaced = role_head_counts(&mut conn, &[("admin".to_owned(), None, false)], at)
            .await
            .unwrap();
        let elapsed = start.elapsed();
        role_head_profile(
            &mut conn,
            &test.pool,
            "unplaced",
            &[("admin".to_owned(), None, false)],
            at,
            elapsed,
            &unplaced,
        )
        .await;
        under_a_hundred_ms(elapsed, "unplaced role_head_counts");
        assert_eq!(unplaced, [1_000]);
        let role_places = [w.project];
        for source in [
            NotificationPeopleSource {
                group: Some(outer),
                ..Default::default()
            },
            NotificationPeopleSource {
                members_org: Some(w.department),
                ..Default::default()
            },
            NotificationPeopleSource {
                role_places: &role_places,
                roles: &roles,
                ..Default::default()
            },
        ] {
            let start = Instant::now();
            let notified = notification_people_page(&mut conn, &source, None, 2_000, at)
                .await
                .unwrap();
            under_a_hundred_ms(start.elapsed(), "notification_people_page");
            assert_eq!(notified.len(), 1_000);
        }
        // A source names one holder; the expansions it leaves out are not run, and find nobody.
        let start = Instant::now();
        let alone = notification_people_page(
            &mut conn,
            &NotificationPeopleSource {
                user: Some(people[0]),
                ..Default::default()
            },
            None,
            2_000,
            at,
        )
        .await
        .unwrap();
        under_a_hundred_ms(start.elapsed(), "notification_people_page of one person");
        assert_eq!(alone, [people[0]]);
        let nobody = notification_people_page(
            &mut conn,
            &NotificationPeopleSource::default(),
            None,
            2_000,
            at,
        )
        .await
        .unwrap();
        assert!(nobody.is_empty());
        let both = NotificationPeopleSource {
            user: Some(w.ada),
            group: Some(outer),
            ..Default::default()
        };
        assert_eq!(
            notification_people_page(&mut conn, &both, None, 2_000, at)
                .await
                .unwrap()
                .len(),
            1_001
        );
        let start = Instant::now();
        let followers = chats::following::derived(&mut conn, None, Some(&[chat]), at)
            .await
            .unwrap();
        under_a_hundred_ms(start.elapsed(), "chat owners");
        assert_eq!(followers.len(), 1_000);
        assert!(
            followers
                .iter()
                .all(|person| person.owner && !person.following)
        );
        for (kind, subject) in [
            (caps::subject::GROUP, outer),
            (caps::subject::MEMBERS, w.department),
        ] {
            let cap = caps::insert(
                &mut conn,
                &caps::NewCap {
                    id: new_id(),
                    node_id: Some(w.root),
                    provider_id: None,
                    subject_kind: kind,
                    subject_id: Some(subject),
                    subject_role: None,
                    split: caps::split::SHARED,
                    models: &[],
                    tiers: &[],
                    harness_kinds: &[],
                    meter: caps::meter::TOKENS,
                    period: Some(caps::period::TOTAL),
                    counted_from: None,
                    amount: None,
                    at_limit: caps::at_limit::STOP,
                    warn_percent: None,
                    uncounted: caps::uncounted::ALLOW,
                    note: "",
                    created_by: w.ada,
                    at,
                },
            )
            .await
            .unwrap()
            .unwrap();
            let start = Instant::now();
            let used = caps::used(&mut conn, &cap, None, None, at).await.unwrap();
            under_a_hundred_ms(start.elapsed(), "shared cap audience");
            assert_eq!(used.len(), 1);
            assert_eq!(used[0].total_tokens, 1_000);
        }
    }

    #[tokio::test]
    async fn what_is_known_of_a_person_is_their_groups_suspensions_and_flags() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let w = world(&mut conn).await;
        let group = insert_group(&mut conn, w.root, "Backend", w.ada)
            .await
            .unwrap();
        add_to_group(&mut conn, group, w.bo).await.unwrap();
        suspend_in(&mut conn, w.department, w.bo, w.ada)
            .await
            .unwrap();
        make_system_admin(&mut conn, w.ada).await.unwrap();
        let bo = principal(&mut conn, w.bo).await.unwrap().unwrap();
        assert_eq!(
            (
                bo.status.as_str(),
                bo.is_system_admin,
                bo.groups,
                bo.suspended_in
            ),
            ("active", false, vec![group], vec![w.department])
        );
        let ada = principal(&mut conn, w.ada).await.unwrap().unwrap();
        assert!(ada.is_system_admin && ada.groups.is_empty() && ada.suspended_in.is_empty());
        suspend(&mut conn, w.bo).await.unwrap();
        assert_eq!(
            principal(&mut conn, w.bo).await.unwrap().unwrap().status,
            "suspended"
        );
        assert!(principal(&mut conn, new_id()).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn a_member_comes_with_the_orgs_from_the_root_down_to_their_domain() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let w = world(&mut conn).await;
        insert_member(&mut conn, w.root, w.bo, w.department)
            .await
            .unwrap();
        let found = memberships(&mut conn, w.bo).await.unwrap();
        assert_eq!(
            found,
            vec![MembershipRow {
                org_id: w.root,
                domain_id: w.department,
                domain_path: vec![w.root, w.department],
            }]
        );
        assert!(memberships(&mut conn, w.ada).await.unwrap().is_empty());
        assert_eq!(
            members_among(&mut conn, w.root, &[w.bo, w.ada])
                .await
                .unwrap(),
            vec![w.bo]
        );
        remove_member(&mut conn, w.root, w.bo).await.unwrap();
        assert!(memberships(&mut conn, w.bo).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn the_roles_that_reach_a_person_are_found_by_user_group_and_org_and_only_while_they_count()
     {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let w = world(&mut conn).await;
        let now = clock::start();
        let group = insert_group(&mut conn, w.root, "Backend", w.ada)
            .await
            .unwrap();
        let places = [w.root, w.department, w.project];
        let direct = give_role(&mut conn, w.project, "user", w.bo, "viewer", w.ada, None)
            .await
            .unwrap();
        let by_group = give_role(
            &mut conn,
            w.department,
            "group",
            group,
            "developer",
            w.ada,
            None,
        )
        .await
        .unwrap();
        let by_org = give_role(
            &mut conn,
            w.root,
            "members",
            w.root,
            "interactor",
            w.ada,
            None,
        )
        .await
        .unwrap();
        let ended = give_role(
            &mut conn,
            w.root,
            "user",
            w.bo,
            "admin",
            w.ada,
            Some(now - Duration::hours(1)),
        )
        .await
        .unwrap();
        let removed = give_role(&mut conn, w.department, "user", w.bo, "admin", w.ada, None)
            .await
            .unwrap();
        take_role_away(&mut conn, removed, w.ada, now)
            .await
            .unwrap();
        let someone_else = give_role(&mut conn, w.root, "user", w.ada, "admin", w.ada, None)
            .await
            .unwrap();
        let found = roles_reaching(&mut conn, &places, w.bo, &[group], &[w.root], now)
            .await
            .unwrap();
        let mut ids: Vec<Id> = found.iter().map(|role| role.id).collect();
        ids.sort();
        let mut expected = vec![direct, by_group, by_org];
        expected.sort();
        assert_eq!(ids, expected);
        assert!(!ids.contains(&ended) && !ids.contains(&removed) && !ids.contains(&someone_else));
        let narrow = roles_reaching(&mut conn, &[w.project], w.bo, &[group], &[w.root], now)
            .await
            .unwrap();
        assert_eq!(narrow.len(), 1, "only the places asked about");
        let listed = roles_in(&mut conn, w.root, now).await.unwrap();
        assert_eq!(listed.len(), 2, "every giving that counts, to whomever");
    }

    #[tokio::test]
    async fn rows_and_entries_are_read_by_page_and_permission_and_a_removed_entry_is_gone() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let w = world(&mut conn).await;
        let now = clock::start();
        let page = live(w.project);
        let defaults = PageKey::new("node", w.root, "defaults");
        let system = PageKey::new("system", SYSTEM_ID, "defaults");
        let kept = add_entry(
            &mut conn,
            &page,
            "any",
            "project.view",
            ("user", Some(w.bo), None),
            None,
            now,
        )
        .await
        .unwrap();
        let gone = add_entry(
            &mut conn,
            &page,
            "any",
            "project.view",
            ("user", Some(w.ada), None),
            None,
            now,
        )
        .await
        .unwrap();
        add_entry(
            &mut conn,
            &page,
            "any",
            "chat.view",
            ("everyone", None, None),
            None,
            now,
        )
        .await
        .unwrap();
        add_entry(
            &mut conn,
            &defaults,
            "any",
            "project.view",
            ("role", None, Some("viewer")),
            None,
            now,
        )
        .await
        .unwrap();
        add_entry(
            &mut conn,
            &system,
            "managed",
            "thread.view",
            ("role", None, Some("viewer")),
            None,
            now,
        )
        .await
        .unwrap();
        set_row(&mut conn, &defaults, "any", "project.view", false, true)
            .await
            .unwrap();
        set_row(&mut conn, &page, "any", "chat.view", false, false)
            .await
            .unwrap();
        assert!(remove_entry(&mut conn, gone, w.ada, now).await.unwrap());
        assert!(
            !remove_entry(&mut conn, gone, w.ada, now).await.unwrap(),
            "once"
        );

        let wanted = ["project.view".to_owned()];
        let found = entries(
            &mut conn,
            &[page.clone(), defaults.clone(), system.clone()],
            &wanted,
        )
        .await
        .unwrap();
        let mut seen: Vec<(String, Id)> = found.iter().map(|e| (e.page.clone(), e.id)).collect();
        seen.sort();
        assert_eq!(
            seen.len(),
            2,
            "the removed entry and the other permissions are left out"
        );
        assert!(
            found
                .iter()
                .any(|e| e.id == kept && e.holder_kind == "user")
        );
        let rows = stored_rows(&mut conn, &[page.clone(), defaults.clone()], &wanted)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].locked && !rows[0].inherit && rows[0].page == "defaults");
        assert_eq!(stored_rows_of(&mut conn, &page).await.unwrap().len(), 1);
        assert_eq!(entries_of(&mut conn, &page).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn the_owners_row_is_read_with_the_people_and_groups_in_the_order_they_were_named() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let w = world(&mut conn).await;
        let now = clock::start();
        let group = insert_group(&mut conn, w.root, "Leads", w.ada)
            .await
            .unwrap();
        let first = name_owner(&mut conn, w.project, ("user", w.ada), now)
            .await
            .unwrap();
        let second = name_owner(
            &mut conn,
            w.project,
            ("group", group),
            now + Duration::seconds(1),
        )
        .await
        .unwrap();
        add_entry(
            &mut conn,
            &live(w.project),
            "any",
            "project.view",
            ("user", Some(w.bo), None),
            None,
            now,
        )
        .await
        .unwrap();
        name_owner(&mut conn, w.department, ("user", w.bo), now)
            .await
            .unwrap();
        let owners = owner_entries(&mut conn, &[live(w.project)]).await.unwrap();
        assert_eq!(
            owners.iter().map(|e| e.id).collect::<Vec<_>>(),
            [first, second]
        );
        assert_eq!(
            owner_entries(&mut conn, &[live(w.project), live(w.department)])
                .await
                .unwrap()
                .len(),
            3
        );
    }

    #[tokio::test]
    async fn a_page_counts_its_changes_in_one_row_and_a_reset_puts_it_back() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let w = world(&mut conn).await;
        let now = clock::start();
        let page = live(w.project);
        assert_eq!(page_revision(&mut conn, &page).await.unwrap(), 0);
        assert_eq!(
            count_page_change(&mut conn, &page, Some(w.ada), now)
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            count_page_change(&mut conn, &page, Some(w.ada), now)
                .await
                .unwrap(),
            2
        );
        assert_eq!(page_revision(&mut conn, &page).await.unwrap(), 2);
        let cut = RowWrite {
            page: &page,
            profile: "any",
            permission: "project.view",
            inherit: false,
            locked: false,
        };
        write_row(&mut conn, &cut, Some(w.ada), now).await.unwrap();
        write_row(&mut conn, &cut, Some(w.ada), now).await.unwrap();
        let entry = add_entry(
            &mut conn,
            &page,
            "any",
            "project.view",
            ("user", Some(w.bo), None),
            None,
            now,
        )
        .await
        .unwrap();
        let owner = name_owner(&mut conn, w.project, ("user", w.ada), now)
            .await
            .unwrap();
        let removed = reset_page(&mut conn, &page, w.ada, now).await.unwrap();
        assert_eq!(removed, vec![entry], "the owners are not removed");
        let rows = stored_rows_of(&mut conn, &page).await.unwrap();
        let view = rows
            .iter()
            .find(|row| row.permission == "project.view")
            .unwrap();
        assert!(view.inherit && !view.locked && view.revision == 3);
        let left = entries_of(&mut conn, &page).await.unwrap();
        assert_eq!(left.iter().map(|e| e.id).collect::<Vec<_>>(), [owner]);
        // The page counter is a row of its own and the rest are not touched by counting.
        assert_eq!(page_revision(&mut conn, &page).await.unwrap(), 2);
    }

    #[tokio::test]
    async fn a_holder_is_named_once_in_a_row_and_an_entry_can_be_rewritten() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let w = world(&mut conn).await;
        let now = clock::start();
        let page = live(w.project);
        let id = add_entry(
            &mut conn,
            &page,
            "any",
            "project.view",
            ("user", Some(w.bo), None),
            None,
            now,
        )
        .await
        .unwrap();
        let again = add_entry(
            &mut conn,
            &page,
            "any",
            "project.view",
            ("user", Some(w.bo), None),
            None,
            now,
        )
        .await;
        assert!(matches!(again, Err(DbError::Unique { .. })));
        let found = find_entry(
            &mut conn,
            &page,
            "any",
            "project.view",
            "user",
            Some(w.bo),
            None,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(found.id, id);
        assert!(
            find_entry(
                &mut conn,
                &page,
                "any",
                "project.view",
                "user",
                Some(w.ada),
                None
            )
            .await
            .unwrap()
            .is_none()
        );
        let options = serde_json::json!({"models": ["m"]});
        let until = now + Duration::days(7);
        assert!(
            rewrite_entry(&mut conn, id, &options, Some(until), "for a week")
                .await
                .unwrap()
        );
        let found = find_entry(
            &mut conn,
            &page,
            "any",
            "project.view",
            "user",
            Some(w.bo),
            None,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            (found.options, found.expires_at, found.note, found.revision),
            (options, Some(until), "for a week".to_owned(), 2)
        );
        remove_entry(&mut conn, id, w.ada, now).await.unwrap();
        assert!(
            !rewrite_entry(&mut conn, id, &serde_json::json!({}), None, "")
                .await
                .unwrap()
        );
        // Once removed, the holder can be named again.
        add_entry(
            &mut conn,
            &page,
            "any",
            "project.view",
            ("user", Some(w.bo), None),
            None,
            now,
        )
        .await
        .unwrap();
        // A role is named by its role, wherever it is held.
        add_entry(
            &mut conn,
            &page,
            "any",
            "project.view",
            ("role", None, Some("viewer")),
            None,
            now,
        )
        .await
        .unwrap();
        let elsewhere = add_entry(
            &mut conn,
            &page,
            "any",
            "project.view",
            ("role", Some(w.root), Some("viewer")),
            None,
            now,
        )
        .await
        .unwrap();
        let found = find_entry(
            &mut conn,
            &page,
            "any",
            "project.view",
            "role",
            Some(w.root),
            Some("viewer"),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(found.id, elsewhere);
    }

    #[tokio::test]
    async fn the_epoch_starts_at_nothing_and_every_bump_moves_it_by_one() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        assert_eq!(epoch(&mut conn).await.unwrap(), 0);
        assert_eq!(bump_epoch(&mut conn).await.unwrap(), 1);
        assert_eq!(bump_epoch(&mut conn).await.unwrap(), 2);
        assert_eq!(epoch(&mut conn).await.unwrap(), 2);
        lock_system_defaults(&mut conn).await.unwrap();
        assert_eq!(
            epoch(&mut conn).await.unwrap(),
            2,
            "locking changes nothing"
        );
    }

    #[tokio::test]
    async fn the_people_behind_a_role_are_those_given_it_in_person_by_group_or_as_members() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let w = world(&mut conn).await;
        let now = clock::start();
        let (cy, di, ed) = (new_id(), new_id(), new_id());
        for (id, name) in [(cy, "cyd"), (di, "dia"), (ed, "eddie")] {
            insert_user(&mut conn, id, name).await.unwrap();
        }
        let group = insert_group(&mut conn, w.root, "Ops", w.ada).await.unwrap();
        add_to_group(&mut conn, group, di).await.unwrap();
        insert_member(&mut conn, w.root, ed, w.department)
            .await
            .unwrap();
        give_role(&mut conn, w.project, "user", cy, "admin", w.ada, None)
            .await
            .unwrap();
        give_role(
            &mut conn,
            w.project,
            "group",
            group,
            "developer",
            w.ada,
            None,
        )
        .await
        .unwrap();
        give_role(
            &mut conn,
            w.project,
            "members",
            w.department,
            "developer",
            w.ada,
            None,
        )
        .await
        .unwrap();
        give_role(&mut conn, w.project, "user", w.bo, "viewer", w.ada, None)
            .await
            .unwrap();
        let wanted = ["developer".to_owned(), "admin".to_owned()];
        let found = people_given_role(&mut conn, &[w.project], &wanted, now, 100)
            .await
            .unwrap();
        let mut people: Vec<Id> = found.iter().map(|(_, user)| *user).collect();
        people.sort();
        let mut expected = vec![cy, di, ed];
        expected.sort();
        assert_eq!(people, expected);
        suspend(&mut conn, cy).await.unwrap();
        let found = people_given_role(&mut conn, &[w.project], &wanted, now, 2)
            .await
            .unwrap();
        assert_eq!(
            found.len(),
            2,
            "limited, and the suspended person is left out"
        );
        assert!(
            found
                .iter()
                .all(|(place, user)| *place == w.project && *user != cy)
        );
    }

    #[tokio::test]
    async fn group_people_system_admins_and_the_active_are_found() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let w = world(&mut conn).await;
        let group = insert_group(&mut conn, w.root, "Ops", w.ada).await.unwrap();
        add_to_group(&mut conn, group, w.bo).await.unwrap();
        add_to_group(&mut conn, group, w.ada).await.unwrap();
        let now = clock::start();
        let pairs = group_people(&mut conn, &[group], 10, now).await.unwrap();
        assert_eq!(pairs.len(), 2);
        assert_eq!(
            group_people(&mut conn, &[group], 1, now)
                .await
                .unwrap()
                .len(),
            1
        );
        make_system_admin(&mut conn, w.ada).await.unwrap();
        assert_eq!(
            system_admin_people(&mut conn, 10).await.unwrap(),
            vec![w.ada]
        );
        suspend_in(&mut conn, w.department, w.bo, w.ada)
            .await
            .unwrap();
        let active = active_among(&mut conn, &[w.ada, w.bo], &[w.root, w.department])
            .await
            .unwrap();
        assert_eq!(
            active,
            vec![w.ada],
            "suspended in one of the orgs asked about"
        );
        let elsewhere = active_among(&mut conn, &[w.ada, w.bo], &[w.root])
            .await
            .unwrap();
        assert_eq!(
            elsewhere.len(),
            2,
            "a suspension in another org counts for nothing"
        );
        suspend(&mut conn, w.ada).await.unwrap();
        assert!(
            system_admin_people(&mut conn, 10).await.unwrap().is_empty(),
            "suspended altogether"
        );
        assert!(
            active_among(&mut conn, &[w.ada], &[])
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn the_roles_of_several_places_and_the_rows_of_many_nodes_are_read_at_once() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let w = world(&mut conn).await;
        let now = clock::start();
        let first = give_role(&mut conn, w.root, "user", w.bo, "viewer", w.ada, None)
            .await
            .unwrap();
        let second = give_role(&mut conn, w.project, "user", w.bo, "admin", w.ada, None)
            .await
            .unwrap();
        give_role(
            &mut conn,
            w.department,
            "user",
            w.bo,
            "developer",
            w.ada,
            Some(now - Duration::hours(1)),
        )
        .await
        .unwrap();
        let found = roles_in_places(&mut conn, &[w.root, w.department, w.project], now)
            .await
            .unwrap();
        let mut ids: Vec<Id> = found.iter().map(|role| role.id).collect();
        ids.sort();
        let mut expected = vec![first, second];
        expected.sort();
        assert_eq!(ids, expected, "what ended is left out");
        set_row(
            &mut conn,
            &live(w.project),
            "any",
            "project.view",
            false,
            false,
        )
        .await
        .unwrap();
        set_row(
            &mut conn,
            &PageKey::new("node", w.department, "defaults"),
            "any",
            "project.view",
            true,
            true,
        )
        .await
        .unwrap();
        set_row(
            &mut conn,
            &live(w.project),
            "any",
            "chat.view",
            false,
            false,
        )
        .await
        .unwrap();
        let rows = stored_rows_of_nodes(
            &mut conn,
            &[w.project, w.department, w.root],
            &["project.view".to_owned()],
        )
        .await
        .unwrap();
        assert_eq!(
            rows.len(),
            2,
            "one permission, on the own page and the defaults page"
        );
        assert!(rows.iter().any(|row| row.page == "defaults" && row.locked));
    }

    #[tokio::test]
    async fn names_and_groups_are_found_with_the_root_of_the_org_that_keeps_them() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let w = world(&mut conn).await;
        let group = insert_group(&mut conn, w.department, "Ops", w.ada)
            .await
            .unwrap();
        let rows = groups(&mut conn, &[group]).await.unwrap();
        assert_eq!(
            rows,
            vec![GroupRow {
                id: group,
                org_id: w.department,
                root_id: w.root,
                name: "Ops".into()
            }]
        );
        let users = user_names(&mut conn, &[w.ada, w.bo]).await.unwrap();
        assert_eq!(users.len(), 2);
        assert_eq!(
            group_names(&mut conn, &[group]).await.unwrap()[0].name,
            "Ops"
        );
        let roots = roots_of(&mut conn, &[w.project, w.department])
            .await
            .unwrap();
        assert!(roots.iter().all(|(_, root)| *root == w.root));
    }

    #[tokio::test]
    async fn the_holder_expansion_keeps_one_plan_a_connection_and_is_never_compiled() {
        // Replanning at every call cost 4-14 ms for 1 ms of work; its generic plan's
        // estimate is past JIT's threshold, so it never compiles whatever the session says.
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let (language, config): (String, Vec<String>) = sqlx::query_as(
            "SELECT l.lanname::text, COALESCE(p.proconfig, '{}') FROM pg_proc p JOIN pg_language l ON l.oid = p.prolang
              WHERE p.proname = 'access_holder_people_many'",
        )
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        assert_eq!(language, "plpgsql");
        assert!(config.iter().any(|setting| setting == "jit=off"));
        assert!(
            config
                .iter()
                .any(|setting| setting == "plan_cache_mode=force_generic_plan")
        );
        let w = world(&mut conn).await;
        let at = clock::start();
        insert_member(&mut conn, w.root, w.bo, w.department)
            .await
            .unwrap();
        give_role(
            &mut conn,
            w.department,
            "user",
            w.bo,
            "developer",
            w.ada,
            None,
        )
        .await
        .unwrap();
        for _ in 0..3 {
            let people: Vec<Id> = sqlx::query_scalar(
                "SELECT user_id FROM access_holder_people('role', $1, 'developer', $2, false) ORDER BY user_id",
            )
            .bind(w.project)
            .bind(at)
            .fetch_all(&mut *conn)
            .await
            .unwrap();
            assert!(
                people.contains(&w.bo),
                "the same people at every call of the connection"
            );
        }
    }
}
