//! Settings and secret metadata.
//!
//! A setting is a row of `settings`: a key with a value at a node, or at the
//! system when the node is none. The scope is not stored; it is the kind of
//! the node. Reading is one statement for the system and every node of a
//! chain, whatever the number of keys. Writing is a compare-and-set on the
//! row's revision, so that two writers never silently overwrite each other.
//!
//! A secret is a row of `secrets`, sealed by the caller; this module stores and
//! finds the sealed bytes and never opens them. The remaining queries are the
//! few facts about other tables that a setting is resolved from: the own
//! values of an environment and of a provider's runtime, and the little that
//! supplies a default (a thread's environment and its template, a model's
//! parameters, the capability definitions, whether the system has a mail
//! server).

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Id, Json};

/// One setting-reach read's subtree and ancestors, or every tree for a system read.
/// Deleted ancestors are retained so the caller can exclude their entire branch without another query.
pub async fn reach_nodes(
    conn: &mut Conn,
    place: Option<Id>,
) -> Result<Vec<super::hierarchy::NodeRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT n.id, n.kind, n.parent_id, n.org_id, n.path, n.name, n.owner_id, n.archived_at, n.deleted_at,
                n.revision, NULL::text AS placement, t.environment_id
           FROM nodes n LEFT JOIN threads t ON t.node_id=n.id
          WHERE $1::uuid IS NULL OR n.path @> ARRAY[$1]::uuid[]
             OR n.id IN (SELECT unnest(path) FROM nodes WHERE id=$1)
          ORDER BY n.id",
    ).bind(place).fetch_all(conn).await?)
}

/// How many times a write goes round before it gives up. A write goes round
/// again only when a row appeared or vanished between its two statements,
/// which takes a writer at exactly the same row; three rounds is far more than
/// any real contention needs and still ends.
const PUT_ROUNDS: usize = 3;

macro_rules! setting_columns {
    () => {
        "node_id, key, value, locked, revision, updated_by, updated_at"
    };
}

/// One stored value of a key, at a node or at the system.
#[derive(Debug, Clone, PartialEq, serde::Deserialize, sqlx::FromRow)]
pub struct SettingRow {
    /// The node it is written at; none for the system.
    pub node_id: Option<Id>,
    /// The key.
    pub key: String,
    /// The value. `null` is a value: it sets the key to nothing.
    pub value: Json,
    /// Whether it decides the key for everything below.
    pub locked: bool,
    /// How many times the row was written; the first is 1.
    pub revision: i64,
    /// Who wrote it last; none for what was seeded.
    pub updated_by: Option<Id>,
    /// When it was written last.
    pub updated_at: DateTime<Utc>,
}

/// The rows of the system and of every node in `nodes`, for the keys in
/// `keys` or for all keys when that is none. One statement, whatever their
/// number.
pub async fn rows_on_chain(
    conn: &mut Conn,
    nodes: &[Id],
    keys: Option<&[String]>,
) -> Result<Vec<SettingRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        setting_columns!(),
        " FROM settings
          WHERE (node_id IS NULL OR node_id = ANY($1)) AND ($2::text[] IS NULL OR key = ANY($2))"
    ))
    .bind(nodes)
    .bind(keys)
    .fetch_all(conn)
    .await?)
}

/// The row of `key` at `node` (the system when none).
pub async fn row(
    conn: &mut Conn,
    node: Option<Id>,
    key: &str,
) -> Result<Option<SettingRow>, DbError> {
    Ok(match node {
        Some(node) => {
            sqlx::query_as(concat!(
                "SELECT ",
                setting_columns!(),
                " FROM settings WHERE node_id = $1 AND key = $2"
            ))
            .bind(node)
            .bind(key)
            .fetch_optional(conn)
            .await?
        }
        None => {
            sqlx::query_as(concat!(
                "SELECT ",
                setting_columns!(),
                " FROM settings WHERE node_id IS NULL AND key = $1"
            ))
            .bind(key)
            .fetch_optional(conn)
            .await?
        }
    })
}

/// Active orgs and projects with a possible nonshipped choice, before effective settings account for ancestors and locks.
pub async fn harness_version_places(conn: &mut Conn, key: &str) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT DISTINCT n.id FROM settings s JOIN nodes n ON n.id = s.node_id
          WHERE s.key = $1 AND n.kind IN ('org','project') AND n.deleted_at IS NULL AND n.archived_at IS NULL
            AND NOT EXISTS (SELECT 1 FROM nodes a WHERE a.id = ANY(n.path)
                              AND (a.deleted_at IS NOT NULL OR a.archived_at IS NOT NULL))
            AND jsonb_path_exists(s.value, '$.*.use ? (@ == \"newest\" || @ == \"newest_checked\" || @ == \"exact\")')",
    ).bind(key).fetch_all(conn).await?)
}

/// What putting a value came to.
#[derive(Debug, Clone, PartialEq)]
pub enum Put {
    /// The row as it now stands.
    Written(SettingRow),
    /// The writer expected a revision that the row does not have, or a row
    /// that is not there.
    Stale,
}

/// What a write puts into a row.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stored<'a> {
    /// The value.
    pub value: &'a Json,
    /// Whether it decides the key for everything below.
    pub locked: bool,
    /// Who writes it; none for the system itself.
    pub by: Option<Id>,
    /// When.
    pub at: DateTime<Utc>,
}

/// Writes `stored` at `node` (the system when none) for `key`. With
/// `expected`, the row must be at that revision, or absent when it is zero; without, the write is
/// unconditional and makes the row when it is not there. A row that is
/// written counts one more revision.
pub async fn put(
    conn: &mut Conn,
    node: Option<Id>,
    key: &str,
    stored: &Stored<'_>,
    expected: Option<i64>,
) -> Result<Put, DbError> {
    if expected == Some(0) {
        return Ok(match insert(conn, node, key, stored).await? {
            Some(row) => Put::Written(row),
            None => Put::Stale,
        });
    }
    for _ in 0..PUT_ROUNDS {
        if let Some(row) = update(conn, node, key, stored, expected).await? {
            return Ok(Put::Written(row));
        }
        if expected.is_some() {
            return Ok(Put::Stale);
        }
        if let Some(row) = insert(conn, node, key, stored).await? {
            return Ok(Put::Written(row));
        }
    }
    Err(DbError::Retry)
}

async fn update(
    conn: &mut Conn,
    node: Option<Id>,
    key: &str,
    stored: &Stored<'_>,
    expected: Option<i64>,
) -> Result<Option<SettingRow>, DbError> {
    Ok(match node {
        Some(node) => {
            sqlx::query_as(concat!(
                "UPDATE settings SET value = $3, locked = $4, revision = revision + 1, updated_by = $5, updated_at = $6
                  WHERE node_id = $1 AND key = $2 AND ($7::bigint IS NULL OR revision = $7)
              RETURNING ",
                setting_columns!()
            ))
            .bind(node)
            .bind(key)
            .bind(stored.value)
            .bind(stored.locked)
            .bind(stored.by)
            .bind(stored.at)
            .bind(expected)
            .fetch_optional(conn)
            .await?
        }
        None => {
            sqlx::query_as(concat!(
                "UPDATE settings SET value = $2, locked = $3, revision = revision + 1, updated_by = $4, updated_at = $5
                  WHERE node_id IS NULL AND key = $1 AND ($6::bigint IS NULL OR revision = $6)
              RETURNING ",
                setting_columns!()
            ))
            .bind(key)
            .bind(stored.value)
            .bind(stored.locked)
            .bind(stored.by)
            .bind(stored.at)
            .bind(expected)
            .fetch_optional(conn)
            .await?
        }
    })
}

async fn insert(
    conn: &mut Conn,
    node: Option<Id>,
    key: &str,
    stored: &Stored<'_>,
) -> Result<Option<SettingRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "INSERT INTO settings (node_id, key, value, locked, revision, updated_by, updated_at)
         VALUES ($1, $2, $3, $4, 1, $5, $6)
         ON CONFLICT DO NOTHING
         RETURNING ",
        setting_columns!()
    ))
    .bind(node)
    .bind(key)
    .bind(stored.value)
    .bind(stored.locked)
    .bind(stored.by)
    .bind(stored.at)
    .fetch_optional(conn)
    .await?)
}

/// What removing a row came to.
#[derive(Debug, Clone, PartialEq)]
pub enum Removed {
    /// The row that was removed, as it stood.
    Gone(SettingRow),
    /// There was no row, and none was expected.
    Absent,
    /// The writer expected a revision that the row does not have, or a row
    /// that is not there.
    Stale,
}

/// Removes the row of `key` at `node` (the system when none), so that the key
/// is inherited again. With `expected`, only a row at that revision goes.
pub async fn remove(
    conn: &mut Conn,
    node: Option<Id>,
    key: &str,
    expected: Option<i64>,
) -> Result<Removed, DbError> {
    let gone: Option<SettingRow> = match node {
        Some(node) => {
            sqlx::query_as(concat!(
                "DELETE FROM settings
                  WHERE node_id = $1 AND key = $2 AND ($3::bigint IS NULL OR revision = $3)
              RETURNING ",
                setting_columns!()
            ))
            .bind(node)
            .bind(key)
            .bind(expected)
            .fetch_optional(conn)
            .await?
        }
        None => {
            sqlx::query_as(concat!(
                "DELETE FROM settings
                  WHERE node_id IS NULL AND key = $1 AND ($2::bigint IS NULL OR revision = $2)
              RETURNING ",
                setting_columns!()
            ))
            .bind(key)
            .bind(expected)
            .fetch_optional(conn)
            .await?
        }
    };
    Ok(match (gone, expected) {
        (Some(row), _) => Removed::Gone(row),
        (None, Some(_)) => Removed::Stale,
        (None, None) => Removed::Absent,
    })
}

/// The kinds of thing a secret can belong to, as `secrets.owner_kind` has them.
pub mod owner {
    /// A node: the secret is a setting's, given to the environments below it.
    pub const SETTING: &str = "setting";
    /// An environment: its own secret.
    pub const ENVIRONMENT: &str = "environment";
}

/// A secret as stored, with its sealed value.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct SecretRow {
    /// Its identifier.
    pub id: Id,
    /// What owns it: see [`owner`].
    pub owner_kind: String,
    /// The node or the environment.
    pub owner_id: Id,
    /// Its name.
    pub name: String,
    /// The value, sealed.
    pub sealed: Vec<u8>,
    /// Who first wrote it; none for what was seeded.
    pub created_by: Option<Id>,
    /// When it was first written.
    pub created_at: DateTime<Utc>,
    /// When its value was last replaced; none until then.
    pub rotated_at: Option<DateTime<Utc>>,
}

/// A secret without its value: what a list shows.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct SecretName {
    /// Its identifier.
    pub id: Id,
    /// What owns it: see [`owner`].
    pub owner_kind: String,
    /// The node or the environment.
    pub owner_id: Id,
    /// Its name.
    pub name: String,
    /// When it was first written.
    pub created_at: DateTime<Utc>,
    /// When its value was last replaced; none until then.
    pub rotated_at: Option<DateTime<Utc>>,
}

/// The secret `name` of the owner, with its sealed value.
pub async fn secret(
    conn: &mut Conn,
    owner_kind: &str,
    owner_id: Id,
    name: &str,
) -> Result<Option<SecretRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, owner_kind, owner_id, name, sealed, created_by, created_at, rotated_at
           FROM secrets WHERE owner_kind = $1 AND owner_id = $2 AND name = $3",
    )
    .bind(owner_kind)
    .bind(owner_id)
    .bind(name)
    .fetch_optional(conn)
    .await?)
}

/// The names, without their values, of every secret of the owners given,
/// each a pair of a kind (see [`owner`]) and an identifier. One statement.
pub async fn secret_names_of(
    conn: &mut Conn,
    owners: &[(&str, Id)],
) -> Result<Vec<SecretName>, DbError> {
    let (kinds, ids): (Vec<String>, Vec<Id>) = owners
        .iter()
        .map(|(kind, id)| ((*kind).to_owned(), *id))
        .unzip();
    Ok(sqlx::query_as(
        "SELECT s.id, s.owner_kind, s.owner_id, s.name, s.created_at, s.rotated_at
           FROM secrets s
           JOIN unnest($1::text[], $2::uuid[]) AS o(kind, id) ON s.owner_kind = o.kind AND s.owner_id = o.id",
    )
    .bind(kinds)
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

/// The same with the sealed values, for whoever opens them.
pub async fn secrets_of(conn: &mut Conn, owners: &[(&str, Id)]) -> Result<Vec<SecretRow>, DbError> {
    let (kinds, ids): (Vec<String>, Vec<Id>) = owners
        .iter()
        .map(|(kind, id)| ((*kind).to_owned(), *id))
        .unzip();
    Ok(sqlx::query_as(
        "SELECT s.id, s.owner_kind, s.owner_id, s.name, s.sealed, s.created_by, s.created_at, s.rotated_at
           FROM secrets s
           JOIN unnest($1::text[], $2::uuid[]) AS o(kind, id) ON s.owner_kind = o.kind AND s.owner_id = o.id",
    )
    .bind(kinds)
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

/// Stores `sealed` as the value of the secret `name` of the owner: a new row,
/// or the value of the row there is, which then counts as rotated at `at`.
pub async fn put_secret(
    conn: &mut Conn,
    owner_kind: &str,
    owner_id: Id,
    name: &str,
    sealed: &[u8],
    by: Option<Id>,
    at: DateTime<Utc>,
) -> Result<SecretName, DbError> {
    Ok(sqlx::query_as(
        "INSERT INTO secrets (id, owner_kind, owner_id, name, sealed, created_by, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         ON CONFLICT (owner_kind, owner_id, name)
         DO UPDATE SET sealed = EXCLUDED.sealed, rotated_at = EXCLUDED.created_at
         RETURNING id, owner_kind, owner_id, name, created_at, rotated_at",
    )
    .bind(orch_types::new_id())
    .bind(owner_kind)
    .bind(owner_id)
    .bind(name)
    .bind(sealed)
    .bind(by)
    .bind(at)
    .fetch_one(conn)
    .await?)
}

/// Deletes the secret `name` of the owner and returns what was deleted, or
/// none when there was no such secret.
pub async fn delete_secret(
    conn: &mut Conn,
    owner_kind: &str,
    owner_id: Id,
    name: &str,
) -> Result<Option<SecretName>, DbError> {
    Ok(sqlx::query_as(
        "DELETE FROM secrets WHERE owner_kind = $1 AND owner_id = $2 AND name = $3
     RETURNING id, owner_kind, owner_id, name, created_at, rotated_at",
    )
    .bind(owner_kind)
    .bind(owner_id)
    .bind(name)
    .fetch_optional(conn)
    .await?)
}

/// What a setting needs of an environment: where it takes its settings from,
/// its own values, and the template it was made from.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct EnvironmentRow {
    /// Its identifier.
    pub id: Id,
    /// The node it takes its settings from.
    pub home_node_id: Id,
    /// Its own values for the keys of scope environment: an object, by key.
    pub config: Json,
    /// `managed` or `connected`.
    pub placement: String,
    /// How many times it was changed.
    pub revision: i64,
    /// The spec of the template it was made from; none for a connected one.
    pub template_spec: Option<Json>,
}

/// The live environment, or exact retained data admitted by a guarded restoration job.
pub async fn environment(conn: &mut Conn, id: Id) -> Result<Option<EnvironmentRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT e.id, e.home_node_id, e.config, e.placement, e.revision, t.spec AS template_spec
           FROM environments e LEFT JOIN env_templates t ON t.id = e.template_id
          WHERE e.id = $1 AND (e.deleted_at IS NULL OR $2)",
    )
    .bind(id)
    .bind(crate::queries::trace::retained_read(id))
    .fetch_optional(conn)
    .await?)
}

/// What a setting needs of a provider: its home and its runtime's own values.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct ProviderRuntimeRow {
    /// Its identifier.
    pub id: Id,
    /// The org it belongs to.
    pub home_id: Id,
    /// Its runtime's own values for the keys of scope provider: an object,
    /// by key.
    pub runtime_config: Json,
    /// How many times it was changed.
    pub revision: i64,
}

/// The provider `id`, if it exists and was not deleted.
pub async fn provider_runtime(
    conn: &mut Conn,
    id: Id,
) -> Result<Option<ProviderRuntimeRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, home_id, runtime_config, revision FROM providers WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

/// The environment a thread runs in, with what supplies defaults.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct ThreadEnvironment {
    /// The environment.
    pub environment_id: Id,
    /// `managed` or `connected`.
    pub placement: String,
    /// The spec of its template; none for a connected one.
    pub template_spec: Option<Json>,
}

/// The environment of the thread `thread`, none while it is in no environment.
pub async fn thread_environment(
    conn: &mut Conn,
    thread: Id,
) -> Result<Option<ThreadEnvironment>, DbError> {
    Ok(sqlx::query_as(
        "SELECT e.id AS environment_id, e.placement, t.spec AS template_spec
           FROM threads th
           JOIN environments e ON e.id = th.environment_id
           LEFT JOIN env_templates t ON t.id = e.template_id
          WHERE th.node_id = $1",
    )
    .bind(thread)
    .fetch_optional(conn)
    .await?)
}

/// What the machine a connected environment is registered on lets an exec
/// node there offer: the `exec.offers` of the local policy the machine last
/// reported. None while it reported none, and for an environment on no
/// machine.
pub async fn machine_exec_offers(
    conn: &mut Conn,
    environment: Id,
) -> Result<Option<Json>, DbError> {
    Ok(sqlx::query_scalar::<_, Option<Json>>(
        "SELECT COALESCE(hm.facts,m.facts) -> 'policy' -> 'exec.offers'
           FROM environments e
           LEFT JOIN machines m ON m.id = e.machine_id
           LEFT JOIN runtimes r ON r.environment_id=e.id AND r.state NOT IN ('retired','removed') AND r.successor_of IS NULL
           LEFT JOIN hosts h ON h.id=r.host_id LEFT JOIN machines hm ON hm.id=h.machine_id
          WHERE e.id = $1 LIMIT 1",
    )
    .bind(environment)
    .fetch_optional(conn)
    .await?
    .flatten())
}

/// What the provider of the thread `thread` lists of the model `model`: its
/// parameters (efforts, default effort, …) as a manager entered them. None
/// while the thread has no provider or the provider does not list the model.
pub async fn thread_model_parameters(
    conn: &mut Conn,
    thread: Id,
    model: &str,
) -> Result<Option<Json>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT pm.parameters
           FROM threads th
           JOIN provider_models pm ON pm.provider_id = th.provider_id AND pm.model_name = $2
          WHERE th.node_id = $1",
    )
    .bind(thread)
    .bind(model)
    .fetch_optional(conn)
    .await?)
}

/// Whether the system has a mail server of its own that is switched on.
pub async fn system_has_mail_server(conn: &mut Conn) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM notification_channels
                         WHERE org_id IS NULL AND user_id IS NULL AND kind = 'email' AND enabled)",
    )
    .fetch_one(conn)
    .await?)
}

/// What a capability definition says a request for it comes to unless a
/// policy says otherwise.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct CapabilityDefault {
    /// The capability.
    pub key: String,
    /// `allow`, `ask` or `deny`.
    pub default_decision: String,
    /// `host`, `org` or `system`.
    pub default_approver_scope: String,
}

/// The defaults of every capability definition, by key.
pub async fn capability_defaults(conn: &mut Conn) -> Result<Vec<CapabilityDefault>, DbError> {
    Ok(sqlx::query_as(
        "SELECT key, default_decision, default_approver_scope FROM capability_defs ORDER BY key",
    )
    .fetch_all(conn)
    .await?)
}

/// Rows written directly, for tests of what stands on them. Not part of any
/// request: a real environment or provider is made by the request that makes
/// it.
pub mod testing {
    use crate::{Conn, DbError};
    use orch_types::{Id, Json};

    /// Sets the own values of an environment.
    pub async fn set_environment_config(
        conn: &mut Conn,
        environment: Id,
        config: &Json,
    ) -> Result<(), DbError> {
        sqlx::query("UPDATE environments SET config = $2 WHERE id = $1")
            .bind(environment)
            .bind(config)
            .execute(conn)
            .await?;
        Ok(())
    }

    /// Sets the spec of the template an environment was made from.
    pub async fn set_template_spec_of_environment(
        conn: &mut Conn,
        environment: Id,
        spec: &Json,
    ) -> Result<(), DbError> {
        sqlx::query(
            "UPDATE env_templates SET spec = $2 WHERE id = (SELECT template_id FROM environments WHERE id = $1)",
        )
        .bind(environment)
        .bind(spec)
        .execute(conn)
        .await?;
        Ok(())
    }

    /// An API provider named `name`, first owned by `owner`, at home in the
    /// org `home`, with one endpoint and `runtime_config` as its runtime's
    /// own values.
    pub async fn insert_provider(
        conn: &mut Conn,
        id: Id,
        name: &str,
        owner: Id,
        home: Id,
        runtime_config: &Json,
    ) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO providers (id, name, owner_id, org_id, home_id, kind, upstreams, arrangements, runtime_config)
             SELECT $1, $2, $3, h.org_id, h.id, 'api',
                    '[{\"protocol\":\"openai-responses\",\"base_url\":\"https://api.example/v1\"}]'::jsonb,
                    ARRAY['separated'], $5
               FROM nodes h WHERE h.id = $4",
        )
        .bind(id)
        .bind(name)
        .bind(owner)
        .bind(home)
        .bind(runtime_config)
        .execute(conn)
        .await?;
        Ok(())
    }

    /// Lists `model` on `provider` with these parameters, made by `by`.
    pub async fn insert_provider_model(
        conn: &mut Conn,
        provider: Id,
        model: &str,
        parameters: &Json,
        by: Id,
    ) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO provider_models (id, provider_id, model_name, parameters, source, created_by)
             VALUES ($1, $2, $3, $4, 'manual', $5)",
        )
        .bind(orch_types::new_id())
        .bind(provider)
        .bind(model)
        .bind(parameters)
        .bind(by)
        .execute(conn)
        .await?;
        Ok(())
    }

    /// Binds `thread` to `provider`.
    pub async fn bind_thread_to_provider(
        conn: &mut Conn,
        thread: Id,
        provider: Id,
    ) -> Result<(), DbError> {
        sqlx::query("UPDATE threads SET provider_id = $2 WHERE node_id = $1")
            .bind(thread)
            .bind(provider)
            .execute(conn)
            .await?;
        Ok(())
    }

    /// A mail server of the system, switched on or off.
    pub async fn insert_system_mail_server(
        conn: &mut Conn,
        by: Id,
        enabled: bool,
    ) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO notification_channels (id, kind, name, enabled, created_by) VALUES ($1, 'email', 'Mail', $2, $3)",
        )
        .bind(orch_types::new_id())
        .bind(enabled)
        .bind(by)
        .execute(conn)
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;
    use crate::queries::hierarchy::testing::{insert_node, put_thread_in_environment};
    use crate::queries::identity::testing::insert_user;
    use orch_testkit::{clock, database::TestDb};
    use orch_types::new_id;
    use serde_json::json;

    async fn org(conn: &mut Conn) -> (Id, Id) {
        let (ada, org) = (new_id(), new_id());
        insert_user(conn, ada, "ada").await.unwrap();
        insert_node(conn, org, "org", None, "Acme", ada)
            .await
            .unwrap();
        (ada, org)
    }

    async fn written(put: Result<Put, DbError>) -> SettingRow {
        match put.unwrap() {
            Put::Written(row) => row,
            Put::Stale => panic!("the write was stale"),
        }
    }

    /// Puts `value` at `node` the way a writer who expects `expected` does.
    async fn set(
        conn: &mut Conn,
        node: Option<Id>,
        key: &str,
        value: Json,
        locked: bool,
        expected: Option<i64>,
    ) -> Result<Put, DbError> {
        let stored = Stored {
            value: &value,
            locked,
            by: None,
            at: clock::start(),
        };
        put(conn, node, key, &stored, expected).await
    }

    #[tokio::test]
    async fn a_row_is_made_by_the_first_write_and_counted_by_each_later_one() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let (_, org) = org(&mut conn).await;
        let first =
            written(set(&mut conn, Some(org), "agent.model", json!("a"), false, None).await).await;
        assert_eq!((first.revision, first.value.clone()), (1, json!("a")));
        let second =
            written(set(&mut conn, Some(org), "agent.model", json!("b"), true, None).await).await;
        assert_eq!((second.revision, second.locked), (2, true));
        let system = written(
            set(
                &mut conn,
                None,
                "terminal.enabled",
                json!(false),
                false,
                None,
            )
            .await,
        )
        .await;
        assert_eq!((system.node_id, system.revision), (None, 1));
        let again = written(
            set(
                &mut conn,
                None,
                "terminal.enabled",
                json!(true),
                false,
                None,
            )
            .await,
        )
        .await;
        assert_eq!(
            again.revision, 2,
            "the system's row is found by its null node as well"
        );
        assert_eq!(
            row(&mut conn, None, "terminal.enabled")
                .await
                .unwrap()
                .unwrap()
                .value,
            json!(true)
        );
    }

    #[tokio::test]
    async fn a_write_that_expects_another_revision_changes_nothing() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let (_, org) = org(&mut conn).await;
        written(set(&mut conn, Some(org), "agent.model", json!("a"), false, None).await).await;
        let stale = set(
            &mut conn,
            Some(org),
            "agent.model",
            json!("b"),
            false,
            Some(7),
        )
        .await
        .unwrap();
        assert_eq!(stale, Put::Stale);
        let kept = row(&mut conn, Some(org), "agent.model")
            .await
            .unwrap()
            .unwrap();
        assert_eq!((kept.value, kept.revision), (json!("a"), 1));
        let current = written(
            set(
                &mut conn,
                Some(org),
                "agent.model",
                json!("b"),
                false,
                Some(1),
            )
            .await,
        )
        .await;
        assert_eq!(current.revision, 2);
        let gone = set(
            &mut conn,
            Some(org),
            "agent.effort",
            json!("b"),
            false,
            Some(1),
        )
        .await
        .unwrap();
        assert_eq!(
            gone,
            Put::Stale,
            "a row that is expected and not there is stale as well"
        );
    }

    #[tokio::test]
    async fn removing_a_row_says_whether_it_was_there_and_whether_it_was_the_one_expected() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let (_, org) = org(&mut conn).await;
        written(set(&mut conn, Some(org), "agent.model", json!("a"), false, None).await).await;
        assert_eq!(
            remove(&mut conn, Some(org), "agent.model", Some(9))
                .await
                .unwrap(),
            Removed::Stale
        );
        assert!(matches!(
            remove(&mut conn, Some(org), "agent.model", Some(1)).await.unwrap(),
            Removed::Gone(row) if row.value == json!("a")
        ));
        assert_eq!(
            remove(&mut conn, Some(org), "agent.model", None)
                .await
                .unwrap(),
            Removed::Absent
        );
        assert_eq!(
            remove(&mut conn, Some(org), "agent.model", Some(1))
                .await
                .unwrap(),
            Removed::Stale
        );
    }

    #[tokio::test]
    async fn the_rows_of_a_chain_come_in_one_statement_with_the_systems() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let (ada, root) = org(&mut conn).await;
        let (other, project) = (new_id(), new_id());
        insert_node(&mut conn, other, "org", None, "Other", ada)
            .await
            .unwrap();
        insert_node(&mut conn, project, "project", Some(root), "Web", ada)
            .await
            .unwrap();
        for (node, key) in [
            (Some(root), "agent.model"),
            (Some(project), "agent.model"),
            (Some(other), "agent.model"),
            (None, "chat.enabled"),
        ] {
            written(set(&mut conn, node, key, json!(1), false, None).await).await;
        }
        let rows = rows_on_chain(&mut conn, &[root, project], None)
            .await
            .unwrap();
        let mut found: Vec<_> = rows
            .iter()
            .map(|row| (row.node_id, row.key.as_str()))
            .collect();
        found.sort();
        assert_eq!(found.len(), 3, "the other tree's row is left out");
        assert!(found.contains(&(None, "chat.enabled")));
        let only = rows_on_chain(
            &mut conn,
            &[root, project],
            Some(&["chat.enabled".to_owned()]),
        )
        .await
        .unwrap();
        assert_eq!(only.len(), 1);
    }

    #[tokio::test]
    async fn a_secret_is_replaced_by_its_name_and_listed_without_its_value() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let (ada, root) = org(&mut conn).await;
        let at = clock::start();
        let first = put_secret(
            &mut conn,
            owner::SETTING,
            root,
            "API_KEY",
            b"sealed-1",
            Some(ada),
            at,
        )
        .await
        .unwrap();
        assert_eq!(first.rotated_at, None);
        let later = at + chrono::Duration::hours(1);
        let second = put_secret(
            &mut conn,
            owner::SETTING,
            root,
            "API_KEY",
            b"sealed-2",
            Some(ada),
            later,
        )
        .await
        .unwrap();
        assert_eq!((second.id, second.rotated_at), (first.id, Some(later)));
        let stored = secret(&mut conn, owner::SETTING, root, "API_KEY")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.sealed, b"sealed-2");
        let names = secret_names_of(&mut conn, &[(owner::SETTING, root)])
            .await
            .unwrap();
        assert_eq!(names.len(), 1);
        assert!(
            secret_names_of(&mut conn, &[(owner::ENVIRONMENT, root)])
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            delete_secret(&mut conn, owner::SETTING, root, "API_KEY")
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            delete_secret(&mut conn, owner::SETTING, root, "API_KEY")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn an_environment_is_found_with_its_own_values_its_home_and_its_template() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let (ada, root) = org(&mut conn).await;
        let (topic, thread, project) = (new_id(), new_id(), new_id());
        insert_node(&mut conn, project, "project", Some(root), "Web", ada)
            .await
            .unwrap();
        insert_node(&mut conn, topic, "topic", Some(project), "Login", ada)
            .await
            .unwrap();
        insert_node(&mut conn, thread, "thread", Some(topic), "Fix", ada)
            .await
            .unwrap();
        assert_eq!(thread_environment(&mut conn, thread).await.unwrap(), None);
        let env = put_thread_in_environment(&mut conn, thread, "managed", ada, topic)
            .await
            .unwrap();
        set_environment_config(&mut conn, env, &json!({"env.update": "on_start"}))
            .await
            .unwrap();
        set_template_spec_of_environment(
            &mut conn,
            env,
            &json!({"agent_defaults": {"approval.mode": "auto"}}),
        )
        .await
        .unwrap();
        let found = environment(&mut conn, env).await.unwrap().unwrap();
        assert_eq!(
            (found.home_node_id, found.placement.as_str()),
            (topic, "managed")
        );
        assert_eq!(found.config, json!({"env.update": "on_start"}));
        let of_thread = thread_environment(&mut conn, thread)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(of_thread.environment_id, env);
        assert_eq!(
            of_thread.template_spec,
            Some(json!({"agent_defaults": {"approval.mode": "auto"}}))
        );
        assert!(environment(&mut conn, new_id()).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn the_parameters_of_a_threads_model_are_those_its_provider_lists() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let (ada, root) = org(&mut conn).await;
        let (project, topic, thread, provider) = (new_id(), new_id(), new_id(), new_id());
        insert_node(&mut conn, project, "project", Some(root), "Web", ada)
            .await
            .unwrap();
        insert_node(&mut conn, topic, "topic", Some(project), "Login", ada)
            .await
            .unwrap();
        insert_node(&mut conn, thread, "thread", Some(topic), "Fix", ada)
            .await
            .unwrap();
        insert_provider(
            &mut conn,
            provider,
            "Key",
            ada,
            root,
            &json!({"runtime.renew_days": 3}),
        )
        .await
        .unwrap();
        insert_provider_model(
            &mut conn,
            provider,
            "gpt-x",
            &json!({"efforts": ["low", "high"], "default_effort": "low"}),
            ada,
        )
        .await
        .unwrap();
        assert_eq!(
            thread_model_parameters(&mut conn, thread, "gpt-x")
                .await
                .unwrap(),
            None,
            "no provider yet"
        );
        bind_thread_to_provider(&mut conn, thread, provider)
            .await
            .unwrap();
        let listed = thread_model_parameters(&mut conn, thread, "gpt-x")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(listed["default_effort"], "low");
        assert_eq!(
            thread_model_parameters(&mut conn, thread, "other")
                .await
                .unwrap(),
            None
        );
        let runtime = provider_runtime(&mut conn, provider)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (runtime.home_id, runtime.runtime_config),
            (root, json!({"runtime.renew_days": 3}))
        );
    }

    #[tokio::test]
    async fn a_mail_server_counts_only_when_it_is_the_systems_and_switched_on() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let (ada, _) = org(&mut conn).await;
        assert!(!system_has_mail_server(&mut conn).await.unwrap());
        insert_system_mail_server(&mut conn, ada, false)
            .await
            .unwrap();
        assert!(!system_has_mail_server(&mut conn).await.unwrap());
        sqlx::query("UPDATE notification_channels SET enabled = true")
            .execute(&mut *conn)
            .await
            .unwrap();
        assert!(system_has_mail_server(&mut conn).await.unwrap());
    }
}
