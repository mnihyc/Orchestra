//! Templates of both kinds, runtimes of both pools with their successors, environments with their owners, homes, what each exec node offers, when each was last used and its scheduled removal, the threads in them, mounts with their locations, and MCP servers: presets, and those imported on a machine.
//!
//! What is here so far is what the access engine reads of environments and mounts: the threads
//! that run in an environment, which are the doors into it; how many environments were made
//! through the entries of a host; the machine a mount's location is on and whose it is; and the
//! storages of a host. Nothing here decides anything.

use crate::{Conn, DbError};
use orch_types::Id;

/// Activity counts for one requested environment, without naming its users or threads.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Activity {
    /// The requested environment.
    pub environment_id: Id,
    /// Primary threads currently running.
    pub running_threads: i64,
    /// Primary threads currently idle.
    pub idle_threads: i64,
    /// Primary threads currently blocked.
    pub blocked_threads: i64,
    /// Turns currently starting or running, including side threads.
    pub active_turns: i64,
    /// Shells not yet closed.
    pub open_shells: i64,
}

/// Reads counts in three grouped scans for any number of environments, including observed zeroes.
pub async fn activity(conn: &mut Conn, environments: &[Id]) -> Result<Vec<Activity>, DbError> {
    Ok(sqlx::query_as(
        "WITH wanted AS (SELECT unnest($1::uuid[]) AS id),
         thread_counts AS (
           SELECT t.environment_id,
             count(*) FILTER (WHERE t.state = 'running') AS running_threads,
             count(*) FILTER (WHERE t.state = 'idle') AS idle_threads,
             count(*) FILTER (WHERE t.state = 'blocked') AS blocked_threads
           FROM threads t JOIN nodes n ON n.id = t.node_id
           WHERE t.environment_id = ANY($1) AND t.side_of IS NULL AND n.deleted_at IS NULL
           GROUP BY t.environment_id
         ), turn_counts AS (
           SELECT t.environment_id, count(*) AS active_turns
           FROM turns u JOIN threads t ON t.node_id = u.thread_id
           WHERE t.environment_id = ANY($1) AND u.state IN ('starting', 'running')
           GROUP BY t.environment_id
         ), shell_counts AS (
           SELECT environment_id, count(*) AS open_shells FROM terminal_sessions
           WHERE environment_id = ANY($1) AND state <> 'closed' GROUP BY environment_id
         )
         SELECT w.id AS environment_id,
           coalesce(t.running_threads, 0) AS running_threads,
           coalesce(t.idle_threads, 0) AS idle_threads,
           coalesce(t.blocked_threads, 0) AS blocked_threads,
           coalesce(u.active_turns, 0) AS active_turns,
           coalesce(s.open_shells, 0) AS open_shells
         FROM wanted w LEFT JOIN thread_counts t ON t.environment_id = w.id
         LEFT JOIN turn_counts u ON u.environment_id = w.id
         LEFT JOIN shell_counts s ON s.environment_id = w.id",
    )
    .bind(environments)
    .fetch_all(conn)
    .await?)
}

// ---- What the access engine reads of environments and mounts -------------------------------------

/// A thread that runs in an environment: the way in of whoever may talk to the agent there, and no
/// door of the environment's, whose four doors are rows of its own page.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct ThreadInEnvironment {
    /// The environment.
    pub environment_id: Id,
    /// The thread's node.
    pub thread_id: Id,
    /// The topic it lies in.
    pub topic_id: Id,
    /// The root org of its tree.
    pub org_id: Id,
    /// Its name.
    pub name: String,
    /// Its first owner.
    pub owner_id: Id,
    /// Its directory in the environment's workspace.
    pub workspace_dir: String,
}

/// The threads that run in the environments `environments`, in the order they were made, not
/// deleted. A side thread runs where its thread does and is no door of its own, so it is not
/// among them.
pub async fn threads_in(
    conn: &mut Conn,
    environments: &[Id],
) -> Result<Vec<ThreadInEnvironment>, DbError> {
    Ok(sqlx::query_as(
        "SELECT t.environment_id, n.id AS thread_id, t.topic_id, n.org_id, n.name, n.owner_id, t.workspace_dir
           FROM threads t JOIN nodes n ON n.id = t.node_id
          WHERE t.environment_id = ANY($1) AND t.side_of IS NULL AND n.deleted_at IS NULL
          ORDER BY t.environment_id, n.id",
    )
    .bind(environments)
    .fetch_all(conn)
    .await?)
}

/// The environment each of the threads `threads` runs in, for those that run in one.
pub async fn environments_of_threads(
    conn: &mut Conn,
    threads: &[Id],
) -> Result<Vec<(Id, Id)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT node_id, environment_id FROM threads
          WHERE node_id = ANY($1) AND environment_id IS NOT NULL",
    )
    .bind(threads)
    .fetch_all(conn)
    .await?)
}

/// Serializes only creation attempts whose bounded physical-Use entries overlap.
/// The caller must retain its transaction through the environment insert. The
/// following count is a new READ COMMITTED statement after competing commits.
pub async fn lock_environment_entries(conn: &mut Conn, entries: &[Id]) -> Result<(), DbError> {
    if !crate::queries::transaction_guard::in_transaction(conn) {
        return Err(DbError::Rule {
            constraint: "environment_capacity_requires_transaction".into(),
        });
    }
    let _: Vec<Id> = sqlx::query_scalar(
        "SELECT id FROM permission_entries WHERE id=ANY($1) ORDER BY id FOR NO KEY UPDATE",
    )
    .bind(entries)
    .fetch_all(conn)
    .await?;
    Ok(())
}

/// How many undeleted environments were made through physical host or machine Use entries:
/// what an entry's cap on environments is counted against.
pub async fn environments_made_through(conn: &mut Conn, entries: &[Id]) -> Result<i64, DbError> {
    other_environments_made_through(conn, entries, None).await
}

/// Counts other reservations while an already authorized environment changes placement.
/// Its own persisted entry is excluded only when it belongs to the counted set.
pub async fn other_environments_made_through(
    conn: &mut Conn,
    entries: &[Id],
    existing: Option<Id>,
) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM environments WHERE host_entry_id = ANY($1) AND deleted_at IS NULL
         AND ($2::uuid IS NULL OR id <> $2)",
    )
    .bind(entries)
    .bind(existing)
    .fetch_one(conn)
    .await?)
}

/// A machine that holds a location of a mount, with whose it is and which host it is.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct LocationMachine {
    /// The machine.
    pub machine_id: Id,
    /// The person it belongs to; none for a machine the system made.
    pub owner_id: Option<Id>,
    /// The host it is the node of, if it is one.
    pub host_id: Option<Id>,
}

/// The machine `machine`, with the host it is the node of.
pub async fn location_machine(
    conn: &mut Conn,
    machine: Id,
) -> Result<Option<LocationMachine>, DbError> {
    Ok(sqlx::query_as(
        "SELECT m.id AS machine_id, m.owner_id, h.id AS host_id
           FROM machines m LEFT JOIN hosts h ON h.machine_id = m.id
          WHERE m.id = $1 AND m.deleted_at IS NULL",
    )
    .bind(machine)
    .fetch_optional(conn)
    .await?)
}

/// A named place on a host where volumes are made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostStorage {
    /// Its name.
    pub name: String,
    /// Whether it is where a volume goes when none is named.
    pub default: bool,
}

/// The storages the host `host` reports.
pub async fn host_storages(conn: &mut Conn, host: Id) -> Result<Vec<HostStorage>, DbError> {
    let reported: Option<serde_json::Value> =
        sqlx::query_scalar("SELECT storages FROM hosts WHERE id = $1")
            .bind(host)
            .fetch_optional(conn)
            .await?;
    let items = reported
        .as_ref()
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(items
        .iter()
        .filter_map(|item| {
            Some(HostStorage {
                name: item.get("name")?.as_str()?.to_owned(),
                default: item
                    .get("default")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
            })
        })
        .collect())
}

#[cfg(test)]
mod access_reads_tests {
    use super::*;
    use crate::queries::access::testing::insert_member;
    use crate::queries::hierarchy::testing::insert_node;
    use crate::queries::identity::testing::insert_user;
    use crate::queries::resources::access_fixtures::*;
    use orch_testkit::database::TestDb;
    use orch_types::new_id;

    #[tokio::test]
    async fn the_threads_of_an_environment_are_its_doors_and_a_side_thread_is_none() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let ada = new_id();
        insert_user(&mut conn, ada, "ada").await.unwrap();
        let (acme, project, topic, one, two, side) =
            (new_id(), new_id(), new_id(), new_id(), new_id(), new_id());
        insert_node(&mut conn, acme, "org", None, "Acme", ada)
            .await
            .unwrap();
        insert_member(&mut conn, acme, ada, acme).await.unwrap();
        insert_node(&mut conn, project, "project", Some(acme), "Web", ada)
            .await
            .unwrap();
        insert_node(&mut conn, topic, "topic", Some(project), "Login", ada)
            .await
            .unwrap();
        insert_node(&mut conn, one, "thread", Some(topic), "One", ada)
            .await
            .unwrap();
        insert_node(&mut conn, two, "thread", Some(topic), "Two", ada)
            .await
            .unwrap();
        insert_node(&mut conn, side, "thread", Some(one), "Side", ada)
            .await
            .unwrap();
        let environment = insert_environment(&mut conn, ada, Some(topic), "main", "managed")
            .await
            .unwrap();
        for thread in [one, two, side] {
            put_thread_into(&mut conn, thread, environment)
                .await
                .unwrap();
        }
        sqlx::query("UPDATE threads SET side_of = $2 WHERE node_id = $1")
            .bind(side)
            .bind(one)
            .execute(&mut *conn)
            .await
            .unwrap();
        let found = threads_in(&mut conn, &[environment]).await.unwrap();
        assert_eq!(
            found.iter().map(|t| t.thread_id).collect::<Vec<_>>(),
            [one, two]
        );
        assert!(
            found
                .iter()
                .all(|t| t.org_id == acme && t.topic_id == topic)
        );
        assert!(threads_in(&mut conn, &[new_id()]).await.unwrap().is_empty());
        assert_eq!(
            environments_made_through(&mut conn, &[new_id()])
                .await
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn a_hosts_machine_is_known_by_the_host_and_its_storages_by_name() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let ada = new_id();
        insert_user(&mut conn, ada, "ada").await.unwrap();
        let host = insert_resource(&mut conn, "host", Some(ada), None, "gpu")
            .await
            .unwrap();
        sqlx::query("UPDATE hosts SET storages = $2 WHERE id = $1")
            .bind(host)
            .bind(serde_json::json!([
                { "name": "fast", "default": true },
                { "name": "bulk" }
            ]))
            .execute(&mut *conn)
            .await
            .unwrap();
        let machine: Id = sqlx::query_scalar("SELECT machine_id FROM hosts WHERE id = $1")
            .bind(host)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        let found = location_machine(&mut conn, machine).await.unwrap().unwrap();
        assert_eq!(found.host_id, Some(host));
        assert!(
            location_machine(&mut conn, new_id())
                .await
                .unwrap()
                .is_none()
        );
        let storages = host_storages(&mut conn, host).await.unwrap();
        assert_eq!(
            storages,
            [
                HostStorage {
                    name: "fast".into(),
                    default: true
                },
                HostStorage {
                    name: "bulk".into(),
                    default: false
                },
            ]
        );
    }
}

/// What the thread engine reads of environments and of the runtimes under
/// them. A module of its own, so that it stands beside whatever else is
/// written about environments without sharing a name with it.
pub mod thread_engine {
    use crate::{Conn, DbError};
    use chrono::{DateTime, Utc};
    use orch_types::{Id, Json};

    /// An environment, as far as a turn needs it.
    #[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
    pub struct EnvironmentRow {
        /// Its identifier.
        pub id: Id,
        /// Its name.
        pub name: String,
        /// Its first owner.
        pub owner_id: Id,
        /// The root org of its home.
        pub org_id: Id,
        /// `managed` or `connected`.
        pub placement: String,
        /// The node it takes its settings from.
        pub home_node_id: Id,
        /// The absolute path of its workspace.
        pub workspace_root: String,
        /// Its own values of the settings of its scope.
        pub config: Json,
        /// The machine it is on now.
        pub machine_id: Option<Id>,
        /// How many times it was made anew.
        pub generation: i32,
        /// Where it stands.
        pub status: String,
        /// A sentence about that.
        pub status_detail: Option<String>,
        /// What it reports of itself.
        pub facts: Json,
        /// When its removal is due.
        pub remove_at: Option<DateTime<Utc>>,
        /// When it was deleted.
        pub deleted_at: Option<DateTime<Utc>>,
    }

    /// The environment `id`.
    pub async fn environment(conn: &mut Conn, id: Id) -> Result<Option<EnvironmentRow>, DbError> {
        let mut found: Option<EnvironmentRow> = sqlx::query_as(
            "SELECT id, name, owner_id, org_id, placement, home_node_id, workspace_root, config, machine_id,
                    generation, status, status_detail, facts, remove_at, deleted_at
               FROM environments WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?;
        if let Some(row) = &mut found
            && let Some(machine) = crate::queries::environment_facts::of(conn, id).await?
        {
            if !row.facts.is_object() {
                row.facts = serde_json::json!({});
            }
            row.facts["environment_descriptor"] = machine.descriptor.unwrap_or(Json::Null);
        }
        Ok(found)
    }

    /// Records that something was done in an environment: the end of a turn.
    ///
    /// Every turn of every thread in the environment marks the same activity row. While another
    /// transaction holds it, that one's mark, of about the same moment, stands for this one, and
    /// nothing waits for it.
    pub async fn touch_activity(conn: &mut Conn, id: Id, at: DateTime<Utc>) -> Result<(), DbError> {
        sqlx::query(
            "UPDATE environments e SET last_activity_at = $2
               FROM (SELECT id FROM environments WHERE id = $1 AND (last_activity_at IS NULL OR last_activity_at < $2)
                     FOR NO KEY UPDATE SKIP LOCKED) due
              WHERE e.id = due.id",
        )
        .bind(id)
        .bind(at)
        .execute(conn)
        .await?;
        Ok(())
    }

    /// The spec of the template a managed environment was made from.
    pub async fn template_spec(conn: &mut Conn, environment: Id) -> Result<Option<Json>, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT t.spec FROM environments e JOIN env_templates t ON t.id = e.template_id WHERE e.id = $1",
        )
        .bind(environment)
        .fetch_optional(conn)
        .await?)
    }

    /// What a runtime is, as far as a blocked thread says it.
    #[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
    pub struct RuntimeRow {
        /// Its identifier.
        pub id: Id,
        /// `provider` or `environment`.
        pub kind: String,
        /// `creating`, `running`, `stopping`, `stopped`, `failed`, `retired` or `removed`.
        pub state: String,
        /// A sentence about that.
        pub state_detail: Option<String>,
        /// The machine of its connector, once it has enrolled.
        pub machine_id: Option<Id>,
    }

    /// The runtime that serves an environment now.
    pub async fn runtime_of_environment(
        conn: &mut Conn,
        environment: Id,
    ) -> Result<Option<RuntimeRow>, DbError> {
        Ok(sqlx::query_as(
            "SELECT id, kind, state, state_detail, machine_id FROM runtimes
              WHERE environment_id = $1 AND kind = 'environment' AND successor_of IS NULL
                AND state NOT IN ('retired', 'removed')",
        )
        .bind(environment)
        .fetch_optional(conn)
        .await?)
    }

    /// The runtime that serves a provider now.
    pub async fn runtime_of_provider(
        conn: &mut Conn,
        provider: Id,
    ) -> Result<Option<RuntimeRow>, DbError> {
        Ok(sqlx::query_as(
            "SELECT id, kind, state, state_detail, machine_id FROM runtimes
              WHERE provider_id = $1 AND kind = 'provider' AND successor_of IS NULL
                AND state NOT IN ('retired', 'removed')",
        )
        .bind(provider)
        .fetch_optional(conn)
        .await?)
    }
}

// ---- What the HTTP routes of environments read and write -----------------------------------------
//
// One block, kept together at the end of the file: the rows the requests of environments list,
// read and change. Nothing here decides anything.

/// What the routes of environments read and write.
pub mod routes {
    use crate::{Conn, DbError};
    use chrono::{DateTime, Utc};
    use orch_types::{Id, Json};

    /// An environment, as stored.
    #[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
    pub struct EnvironmentFull {
        /// Its identifier.
        pub id: Id,
        /// Its name.
        pub name: String,
        /// Its first owner.
        pub owner_id: Id,
        /// The root org of its home node.
        pub org_id: Id,
        /// `managed` or `connected`.
        pub placement: String,
        /// The node it takes its settings from.
        pub home_node_id: Id,
        /// The absolute path of its workspace.
        pub workspace_root: String,
        /// Its own values of the settings of its scope.
        pub config: Json,
        /// The machine it is a directory of, or the one it is on now.
        pub machine_id: Option<Id>,
        /// The template it was made from, for a managed one.
        pub template_id: Option<Id>,
        /// How many times it was made anew.
        pub generation: i32,
        /// Whether files may be written from outside.
        pub remote_write: bool,
        /// Whether shells may be opened in it.
        pub terminal: bool,
        /// Whether browsers may be shown, started and driven in it.
        pub browser: bool,
        /// Whether displays may be shown, started and worked in in it.
        pub vnc: bool,
        /// When something last happened in it.
        pub last_activity_at: Option<DateTime<Utc>>,
        /// The capabilities granted to it.
        pub granted_capabilities: Vec<String>,
        /// Where it stands.
        pub status: String,
        /// A sentence about that.
        pub status_detail: Option<String>,
        /// What it reports of itself.
        pub facts: Json,
        /// When its manifest was taken.
        pub manifest_at: Option<DateTime<Utc>>,
        /// The sequence of its feed.
        pub env_seq: i64,
        /// Since when no thread is bound to it.
        pub unused_since: Option<DateTime<Utc>>,
        /// When its removal is due.
        pub remove_at: Option<DateTime<Utc>>,
        /// The revision a change names.
        pub revision: i64,
        /// When it was made.
        pub created_at: DateTime<Utc>,
        /// When it was last changed.
        pub updated_at: DateTime<Utc>,
    }

    const COLUMNS: &str = "id, name, owner_id, org_id, placement, home_node_id, workspace_root, config, machine_id,
        template_id, generation, COALESCE(remote_write,true) AS remote_write, COALESCE(terminal,true) AS terminal, COALESCE(browser,true) AS browser, COALESCE(vnc,true) AS vnc, last_activity_at, granted_capabilities,
        status, status_detail, facts, manifest_at, env_seq, unused_since, remove_at, revision, created_at,
        updated_at";

    /// The environments `ids` that are not deleted, in the order they were made.
    pub async fn environments(
        conn: &mut Conn,
        ids: &[Id],
    ) -> Result<Vec<EnvironmentFull>, DbError> {
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {COLUMNS} FROM environments WHERE id = ANY($1) AND deleted_at IS NULL
              ORDER BY created_at, id"
        )))
        .bind(ids)
        .fetch_all(conn)
        .await?)
    }

    /// The same stored metadata reader, including retained deleted registrations.
    pub async fn environments_including_deleted(
        conn: &mut Conn,
        ids: &[Id],
    ) -> Result<Vec<EnvironmentFull>, DbError> {
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {COLUMNS} FROM environments WHERE id=ANY($1) ORDER BY created_at,id"
        )))
        .bind(ids)
        .fetch_all(conn)
        .await?)
    }

    /// A directory of a person's machine to register as an environment.
    #[derive(Debug, Clone, PartialEq)]
    pub struct NewConnected<'a> {
        /// Its identifier.
        pub id: Id,
        /// Its name.
        pub name: &'a str,
        /// Its creator and canonical first owner.
        pub owner_id: Id,
        /// The machine.
        pub machine_id: Id,
        /// The node whose settings it takes.
        pub home_node_id: Id,
        /// The directory that is its workspace root.
        pub workspace_root: &'a str,
        /// Whether files may be written from outside.
        pub remote_write: Option<bool>,
        /// Whether shells may be opened in it.
        pub terminal: Option<bool>,
    }

    /// Registers a directory of a machine as an environment, in status `none`; its root org is
    /// read from the hierarchy. Its browsers and its displays are shut, as everything of a
    /// person's machine is until its owner says otherwise (the columns start off). Returns whether
    /// the home node is there. A clash of the name among the owner's environments of that root org
    /// is a [`DbError::Unique`].
    pub async fn insert_connected(
        conn: &mut Conn,
        made: &NewConnected<'_>,
        at: DateTime<Utc>,
    ) -> Result<bool, DbError> {
        let inserted = sqlx::query(
            "INSERT INTO environments (id, name, owner_id, org_id, placement, home_node_id, workspace_root,
                                       machine_id, remote_write, terminal, created_at, updated_at)
             SELECT $1, $2, $3, n.org_id, 'connected', n.id, $5, $4, $6, $7, $8, $8
               FROM nodes n WHERE n.id = $9 AND n.deleted_at IS NULL",
        )
        .bind(made.id)
        .bind(made.name)
        .bind(made.owner_id)
        .bind(made.machine_id)
        .bind(made.workspace_root)
        .bind(made.remote_write)
        .bind(made.terminal)
        .bind(at)
        .bind(made.home_node_id)
        .execute(conn)
        .await?
        .rows_affected();
        Ok(inserted > 0)
    }

    /// What a change of an environment sets; what is left out stays.
    #[derive(Debug, Clone, Copy, Default, PartialEq)]
    pub struct Change<'a> {
        /// A new name.
        pub name: Option<&'a str>,
        /// New own values.
        pub config: Option<&'a Json>,
        /// Files may be written from outside, or not.
        pub remote_write: Option<bool>,
        /// Shells may be opened, or not.
        pub terminal: Option<bool>,
        /// Browsers may be shown, started and driven, or not.
        pub browser: Option<bool>,
        /// Displays may be shown, started and worked in, or not.
        pub vnc: Option<bool>,
        /// A new home node, which must lie in the same root org.
        pub home_node_id: Option<Id>,
    }

    /// Changes an environment that is at revision `expected`. Returns whether it was. A clash of
    /// the name is a [`DbError::Unique`].
    pub async fn change(
        conn: &mut Conn,
        id: Id,
        expected: i64,
        change: &Change<'_>,
        at: DateTime<Utc>,
    ) -> Result<bool, DbError> {
        let changed = sqlx::query(
            "UPDATE environments SET name = COALESCE($3, name), config = COALESCE($4, config),
                    remote_write = COALESCE($5, remote_write), terminal = COALESCE($6, terminal),
                    browser = COALESCE($7, browser), vnc = COALESCE($8, vnc),
                    home_node_id = COALESCE($9, home_node_id), revision = revision + 1, updated_at = $10
              WHERE id = $1 AND revision = $2 AND deleted_at IS NULL",
        )
        .bind(id)
        .bind(expected)
        .bind(change.name)
        .bind(change.config)
        .bind(change.remote_write)
        .bind(change.terminal)
        .bind(change.browser)
        .bind(change.vnc)
        .bind(change.home_node_id)
        .bind(at)
        .execute(conn)
        .await?
        .rows_affected();
        Ok(changed > 0)
    }

    /// Switches an environment off, or on again: it stands where it did before it was switched off.
    pub async fn set_disabled(
        conn: &mut Conn,
        id: Id,
        disabled: bool,
        at: DateTime<Utc>,
    ) -> Result<(), DbError> {
        sqlx::query(
            "UPDATE environments SET status = CASE WHEN $2 THEN 'disabled'
                                                   WHEN status = 'disabled' THEN CASE WHEN EXISTS(SELECT 1 FROM runtimes r
                                                       WHERE r.environment_id=$1 AND r.successor_of IS NULL AND r.state NOT IN ('retired','removed'))
                                                       THEN 'stopped' ELSE 'none' END ELSE status END,
                    revision = revision + 1, updated_at = $3
              WHERE id = $1 AND deleted_at IS NULL AND ((status = 'disabled') <> $2)",
        )
        .bind(id)
        .bind(disabled)
        .bind(at)
        .execute(conn)
        .await?;
        Ok(())
    }

    /// Dates the removal of an environment that is made from a template: the reaper does it.
    pub async fn schedule_removal(
        conn: &mut Conn,
        id: Id,
        at: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<(), DbError> {
        sqlx::query(
            "UPDATE environments SET remove_at = $2, revision = revision + 1, updated_at = $3
              WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(id)
        .bind(at)
        .bind(now)
        .execute(conn)
        .await?;
        Ok(())
    }

    /// The local policy each of the machines `ids` last reported, for those that reported one.
    pub async fn machine_policies(conn: &mut Conn, ids: &[Id]) -> Result<Vec<(Id, Json)>, DbError> {
        Ok(sqlx::query_as(
            "SELECT id, facts -> 'policy' FROM machines
              WHERE id = ANY($1) AND jsonb_typeof(facts -> 'policy') = 'object'",
        )
        .bind(ids)
        .fetch_all(conn)
        .await?)
    }

    /// Removes the registration of a directory of a machine: the environment is deleted and its
    /// removal dated.
    pub async fn remove_registration(
        conn: &mut Conn,
        id: Id,
        at: DateTime<Utc>,
    ) -> Result<(), DbError> {
        sqlx::query(
            "UPDATE environments SET deleted_at = $2, remove_at = $2, revision = revision + 1, updated_at = $2
              WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(id)
        .bind(at)
        .execute(conn)
        .await?;
        Ok(())
    }

    /// The environment of `owner` in the tree of the root org `org` that goes by `name`, whatever its
    /// case, that is not deleted: names are taken per owner and per root org.
    pub async fn named(
        conn: &mut Conn,
        owner: Id,
        org: Id,
        name: &str,
    ) -> Result<Option<Id>, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT id FROM environments
              WHERE owner_id = $1 AND org_id = $2 AND lower(name) = lower($3) AND deleted_at IS NULL LIMIT 1",
        )
        .bind(owner)
        .bind(org)
        .bind(name)
        .fetch_optional(conn)
        .await?)
    }

    /// How many threads that are not deleted are bound to an environment.
    pub async fn threads_bound(conn: &mut Conn, id: Id) -> Result<i64, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT count(*) FROM threads t JOIN nodes n ON n.id = t.node_id
              WHERE t.environment_id = $1 AND n.deleted_at IS NULL",
        )
        .bind(id)
        .fetch_one(conn)
        .await?)
    }

    /// The threads that are not deleted and are bound to an environment, each with its first owner.
    pub async fn bound_threads(conn: &mut Conn, id: Id) -> Result<Vec<(Id, Id)>, DbError> {
        Ok(sqlx::query_as(
            "SELECT t.node_id, n.owner_id FROM threads t JOIN nodes n ON n.id = t.node_id
              WHERE t.environment_id = $1 AND n.deleted_at IS NULL ORDER BY t.node_id",
        )
        .bind(id)
        .fetch_all(conn)
        .await?)
    }

    /// The places that explicitly choose each environment as their inherited default.
    pub async fn default_for(conn: &mut Conn, ids: &[Id]) -> Result<Vec<(Id, Id)>, DbError> {
        Ok(sqlx::query_as(
            "SELECT (s.value #>> '{}')::uuid, s.node_id FROM settings s JOIN nodes n ON n.id = s.node_id
              WHERE s.key = 'env.default_id' AND s.value #>> '{}' = ANY(SELECT id::text FROM unnest($1::uuid[]) id)
                AND n.deleted_at IS NULL",
        )
        .bind(ids)
        .fetch_all(conn)
        .await?)
    }

    /// The templates `ids`, with the driver that makes units from each.
    pub async fn template_names(
        conn: &mut Conn,
        ids: &[Id],
    ) -> Result<Vec<(Id, String, String)>, DbError> {
        Ok(
            sqlx::query_as("SELECT id, name, driver FROM env_templates WHERE id = ANY($1)")
                .bind(ids)
                .fetch_all(conn)
                .await?,
        )
    }

    /// Whether the node `inner` lies in the same root org as the node `outer` and is not deleted:
    /// what a new home of an environment must do.
    pub async fn same_tree(conn: &mut Conn, inner: Id, outer: Id) -> Result<bool, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM nodes a JOIN nodes b ON a.org_id = b.org_id
                             WHERE a.id = $1 AND b.id = $2 AND a.deleted_at IS NULL AND b.deleted_at IS NULL)",
        )
        .bind(inner)
        .bind(outer)
        .fetch_one(conn)
        .await?)
    }
}

// ---- What the workspace of an environment reads and writes ---------------------------------------
//
// One block, kept together at the end of the file, for the requests about an environment's files,
// snapshots and processes: the environment as they need it, where a thread is, the list of the tree
// that a machine last uploaded, and the changes agents made. Nothing here decides anything.

/// What the requests about an environment's workspace read and write.
pub mod workspace {
    use crate::{Conn, DbError};
    use chrono::{DateTime, Utc};
    use orch_types::{Hash, Id, Json};

    /// An environment, as far as its workspace is concerned.
    #[derive(Debug, Clone, PartialEq)]
    pub struct EnvironmentRow {
        /// Its identifier.
        pub id: Id,
        /// Its name.
        pub name: String,
        /// Its first owner.
        pub owner_id: Id,
        /// The root org of its home node.
        pub org_id: Id,
        /// The node it takes its settings from.
        pub home_node_id: Id,
        /// `managed` or `connected`.
        pub placement: String,
        /// Where it stands.
        pub status: String,
        /// The absolute path of its workspace.
        pub workspace_root: String,
        /// The machine it is on now.
        pub machine_id: Option<Id>,
        /// Whether files may be written from outside.
        pub remote_write: bool,
        /// Whether shells may be opened in it, and its processes signalled.
        pub terminal: bool,
        /// How many times it was made anew.
        pub generation: i32,
        /// What it reports of itself.
        pub facts: Json,
        /// The list of its tree that a machine last uploaded.
        pub manifest_blob: Option<Hash>,
        /// When that list was taken.
        pub manifest_at: Option<DateTime<Utc>>,
    }

    #[derive(sqlx::FromRow)]
    struct Raw {
        id: Id,
        name: String,
        owner_id: Id,
        org_id: Id,
        home_node_id: Id,
        placement: String,
        status: String,
        workspace_root: String,
        machine_id: Option<Id>,
        remote_write: bool,
        terminal: bool,
        generation: i32,
        facts: Json,
        manifest_blob: Option<Vec<u8>>,
        manifest_at: Option<DateTime<Utc>>,
    }

    fn hash_of(raw: &[u8]) -> Result<Hash, DbError> {
        Hash::from_slice(raw)
            .map_err(|_| DbError::Other(sqlx::Error::Decode("a hash that is not 32 bytes".into())))
    }

    /// The environment `id`, if it is there and not deleted.
    pub async fn environment(conn: &mut Conn, id: Id) -> Result<Option<EnvironmentRow>, DbError> {
        let raw: Option<Raw> = sqlx::query_as(
            "SELECT id, name, owner_id, org_id, home_node_id, placement, status, workspace_root, machine_id,
                    COALESCE(remote_write,true) AS remote_write, COALESCE(terminal,true) AS terminal, generation, facts, manifest_blob, manifest_at
               FROM environments WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(id)
        .fetch_optional(conn)
        .await?;
        raw.map(|raw| {
            Ok(EnvironmentRow {
                id: raw.id,
                name: raw.name,
                owner_id: raw.owner_id,
                org_id: raw.org_id,
                home_node_id: raw.home_node_id,
                placement: raw.placement,
                status: raw.status,
                workspace_root: raw.workspace_root,
                machine_id: raw.machine_id,
                remote_write: raw.remote_write,
                terminal: raw.terminal,
                generation: raw.generation,
                facts: raw.facts,
                manifest_blob: raw.manifest_blob.as_deref().map(hash_of).transpose()?,
                manifest_at: raw.manifest_at,
            })
        })
        .transpose()
    }

    /// Where a thread is.
    #[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
    pub struct ThreadPlace {
        /// The environment it runs in, if it runs in one.
        pub environment_id: Option<Id>,
        /// Its directory there, relative to the root of the workspace.
        pub workspace_dir: String,
        /// The topic it lies in.
        pub topic_id: Id,
        /// The thread it is a side of, for a side thread.
        pub side_of: Option<Id>,
    }

    /// Where the thread `thread` is, if it is a thread that is not deleted.
    pub async fn thread_place(conn: &mut Conn, thread: Id) -> Result<Option<ThreadPlace>, DbError> {
        Ok(sqlx::query_as(
            "SELECT t.environment_id, t.workspace_dir, t.topic_id, t.side_of
               FROM threads t JOIN nodes n ON n.id = t.node_id
              WHERE t.node_id = $1 AND n.deleted_at IS NULL",
        )
        .bind(thread)
        .fetch_optional(conn)
        .await?)
    }

    /// Keeps the list of the tree that a machine uploaded as the environment's latest, and says
    /// which list it replaces: the one the caller then unpins.
    pub async fn keep_manifest(
        conn: &mut Conn,
        environment: Id,
        manifest: &Hash,
        taken_at: DateTime<Utc>,
    ) -> Result<Option<Option<Hash>>, DbError> {
        let before: Option<Option<Vec<u8>>> = sqlx::query_scalar(
            "SELECT manifest_blob FROM environments WHERE id = $1 AND deleted_at IS NULL FOR UPDATE",
        )
        .bind(environment)
        .fetch_optional(&mut *conn)
        .await?;
        let Some(before) = before else {
            return Ok(None);
        };
        sqlx::query("UPDATE environments SET manifest_blob = $2, manifest_at = $3 WHERE id = $1")
            .bind(environment)
            .bind(manifest.as_bytes().as_slice())
            .bind(taken_at)
            .execute(conn)
            .await?;
        Ok(Some(before.as_deref().map(hash_of).transpose()?))
    }

    /// A change an agent made to a file, with where it came from.
    #[derive(Debug, Clone, PartialEq)]
    pub struct ChangeRow {
        /// The item of the thread whose tool call made it.
        pub item_id: Id,
        /// The thread.
        pub thread_id: Id,
        /// When the item was last written.
        pub at: DateTime<Utc>,
        /// The change, as the item holds it (`FileChange`).
        pub change: Json,
    }

    /// The latest change to each path that the file-change tool calls of the threads in an
    /// environment reported: those of one thread, of one turn, or of every thread there.
    pub async fn latest_changes(
        conn: &mut Conn,
        environment: Id,
        thread: Option<Id>,
        turn: Option<Id>,
    ) -> Result<Vec<ChangeRow>, DbError> {
        let rows: Vec<(Id, Id, DateTime<Utc>, Json)> = sqlx::query_as(
            "SELECT DISTINCT ON (c.change ->> 'path') i.id, i.thread_id, i.updated_at, c.change
               FROM items i
               JOIN threads t ON t.node_id = i.thread_id
              CROSS JOIN LATERAL jsonb_array_elements(i.payload #> '{tool,changes}') WITH ORDINALITY AS c(change, n)
              WHERE t.environment_id = $1
                AND i.kind = 'tool_call' AND i.payload #>> '{tool,kind}' = 'file_change'
                AND jsonb_typeof(i.payload #> '{tool,changes}') = 'array'
                AND ($2::uuid IS NULL OR i.thread_id = $2)
                AND ($3::uuid IS NULL OR i.turn_id = $3)
              ORDER BY c.change ->> 'path', i.seq_created DESC, c.n DESC",
        )
        .bind(environment)
        .bind(thread)
        .bind(turn)
        .fetch_all(conn)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(item_id, thread_id, at, change)| ChangeRow {
                item_id,
                thread_id,
                at,
                change,
            })
            .collect())
    }
}

// ---- What choosing an environment for a thread reads ------------------------------------------------
//
// One block, kept together at the end of the file: the host an environment made from a template runs
// on, and whether a person has a machine of their own to make one on. Nothing here decides anything.

/// What the dialog that puts a thread into an environment reads.
pub mod binding {
    use crate::{Conn, DbError};
    use orch_types::Id;

    /// The host each of the environments `environments` runs on, for those made from a template whose
    /// unit is there: the environment, the host and its name.
    pub async fn hosts_of(
        conn: &mut Conn,
        environments: &[Id],
    ) -> Result<Vec<(Id, Id, String)>, DbError> {
        Ok(sqlx::query_as(
            "SELECT r.environment_id, h.id, h.name
               FROM runtimes r JOIN hosts h ON h.id = r.host_id
              WHERE r.kind = 'environment' AND r.environment_id = ANY($1)
                AND r.state NOT IN ('retired', 'removed') AND r.successor_of IS NULL",
        )
        .bind(environments)
        .fetch_all(conn)
        .await?)
    }

    /// Of the templates `ids`, those environments are made from and that are switched on.
    pub async fn environment_templates(conn: &mut Conn, ids: &[Id]) -> Result<Vec<Id>, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT id FROM env_templates
              WHERE id = ANY($1) AND kind = 'environment' AND enabled AND deleted_at IS NULL",
        )
        .bind(ids)
        .fetch_all(conn)
        .await?)
    }

    /// Whether the person owns a machine that is registered and not revoked: one an environment of
    /// their own can be a directory of.
    pub async fn owns_a_machine(conn: &mut Conn, person: Id) -> Result<bool, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM machines
                             WHERE owner_id = $1 AND placement = 'connected' AND status = 'active'
                               AND deleted_at IS NULL)",
        )
        .bind(person)
        .fetch_one(conn)
        .await?)
    }
}

// ---- What making, starting, stopping and removing environments reads and writes -------------------
//
// (the environments part) One block, kept together at the end of the file: the rows an environment
// and its runtimes are made of, changed by compare-and-set on their state, and every change of an
// environment stamped on its feed. Nothing here decides anything.

/// What the jobs and the schedule of the environments part read and write.
pub mod hosting {
    use crate::queries::jobs::stamp_environment;
    use crate::{Conn, DbError};
    use chrono::{DateTime, Utc};
    use orch_types::{Id, Json};

    /// The words of `environments.status`.
    pub mod status {
        /// A record, never made.
        pub const NONE: &str = "none";
        /// Being made, or started.
        pub const PROVISIONING: &str = "provisioning";
        /// Up.
        pub const RUNNING: &str = "running";
        /// Stopped, its unit kept.
        pub const STOPPED: &str = "stopped";
        /// Being given a new unit.
        pub const RECREATING: &str = "recreating";
        /// Being taken to another host.
        pub const MOVING: &str = "moving";
        /// Failed, with the reason.
        pub const FAILED: &str = "failed";
        /// Its machine is away.
        pub const OFFLINE: &str = "offline";
        /// Switched off.
        pub const DISABLED: &str = "disabled";
    }

    /// The words of `runtimes.state`.
    pub mod runtime_state {
        /// Being made, or started.
        pub const CREATING: &str = "creating";
        /// Up.
        pub const RUNNING: &str = "running";
        /// Being stopped.
        pub const STOPPING: &str = "stopping";
        /// Stopped, its unit kept.
        pub const STOPPED: &str = "stopped";
        /// Gone wrong.
        pub const FAILED: &str = "failed";
        /// Replaced, waiting for the reaper.
        pub const RETIRED: &str = "retired";
        /// Gone.
        pub const REMOVED: &str = "removed";
        /// The states of a runtime that is somebody's current one.
        pub const LIVE: [&str; 5] = [CREATING, RUNNING, STOPPING, STOPPED, FAILED];
    }

    /// An environment, as the jobs read it.
    #[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
    pub struct EnvironmentState {
        /// Its identifier.
        pub id: Id,
        /// Its name.
        pub name: String,
        /// Its first owner.
        pub owner_id: Id,
        /// The root org of its home.
        pub org_id: Id,
        /// `managed` or `connected`.
        pub placement: String,
        /// The node it takes its settings from.
        pub home_node_id: Id,
        /// The absolute path of its workspace.
        pub workspace_root: String,
        /// Its own values of the settings of its scope.
        pub config: Json,
        /// The machine it is on now.
        pub machine_id: Option<Id>,
        /// The template it was made from.
        pub template_id: Option<Id>,
        /// The entry its creator used on the host.
        pub host_entry_id: Option<Id>,
        /// The entry its creator used on the template.
        pub template_entry_id: Option<Id>,
        /// How many times it was made anew.
        pub generation: i32,
        /// Whether files may be written from outside.
        pub remote_write: bool,
        /// Whether shells may be opened in it.
        pub terminal: bool,
        /// When something last happened in it.
        pub last_activity_at: Option<DateTime<Utc>>,
        /// The capabilities granted to it.
        pub granted_capabilities: Vec<String>,
        /// Where it stands.
        pub status: String,
        /// A sentence about that.
        pub status_detail: Option<String>,
        /// What it reports of itself.
        pub facts: Json,
        /// The number of its feed.
        pub env_seq: i64,
        /// Since when no thread is bound to it.
        pub unused_since: Option<DateTime<Utc>>,
        /// When its removal is due.
        pub remove_at: Option<DateTime<Utc>>,
        /// The revision a change names.
        pub revision: i64,
        /// When it was made.
        pub created_at: DateTime<Utc>,
        /// When it was deleted.
        pub deleted_at: Option<DateTime<Utc>>,
    }

    const STATE_COLUMNS: &str = "id, name, owner_id, org_id, placement, home_node_id, workspace_root, config, machine_id,
        template_id, host_entry_id, template_entry_id, generation, COALESCE(remote_write,true) AS remote_write, COALESCE(terminal,true) AS terminal, last_activity_at,
        granted_capabilities, status, status_detail, facts, env_seq, unused_since, remove_at, revision, created_at,
        deleted_at";

    /// The environment `id`, deleted or not.
    pub async fn environment(conn: &mut Conn, id: Id) -> Result<Option<EnvironmentState>, DbError> {
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {STATE_COLUMNS} FROM environments WHERE id = $1"
        )))
        .bind(id)
        .fetch_optional(conn)
        .await?)
    }

    /// The environment `id`, locked until the transaction ends.
    pub async fn lock_environment(
        conn: &mut Conn,
        id: Id,
    ) -> Result<Option<EnvironmentState>, DbError> {
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {STATE_COLUMNS} FROM environments WHERE id = $1 FOR UPDATE"
        )))
        .bind(id)
        .fetch_optional(conn)
        .await?)
    }

    /// A managed environment to make.
    #[derive(Debug, Clone, PartialEq)]
    pub struct NewManaged<'a> {
        /// Its identifier.
        pub id: Id,
        /// Its name.
        pub name: &'a str,
        /// Its first owner: its maker.
        pub owner_id: Id,
        /// The node whose settings it takes.
        pub home_node_id: Id,
        /// The template it is made from.
        pub template_id: Id,
        /// Its own values: what its maker chose and what it inherited.
        pub config: &'a Json,
        /// The entry its maker used on the host, once a host is named.
        pub host_entry_id: Option<Id>,
        /// The entry its maker used on the template.
        pub template_entry_id: Option<Id>,
    }

    /// Makes a managed environment, in status `none`: files may be written through Orch, shells
    /// opened, browsers shown and displays worked in, as for everything made from a template.
    /// Returns whether the home node is there. A clash of the name among the owner's environments
    /// of that root org is a [`DbError::Unique`].
    pub async fn insert_managed(
        conn: &mut Conn,
        made: &NewManaged<'_>,
        at: DateTime<Utc>,
    ) -> Result<bool, DbError> {
        let inserted = sqlx::query(
            "INSERT INTO environments (id, name, owner_id, org_id, placement, home_node_id, config, template_id,
                                       host_entry_id, template_entry_id,
                                       status, created_at, updated_at)
             SELECT $1, $2, $3, n.org_id, 'managed', n.id, $5, $6, $7, $8, 'none', $4, $4
               FROM nodes n WHERE n.id = $9 AND n.deleted_at IS NULL",
        )
        .bind(made.id)
        .bind(made.name)
        .bind(made.owner_id)
        .bind(at)
        .bind(made.config)
        .bind(made.template_id)
        .bind(made.host_entry_id)
        .bind(made.template_entry_id)
        .bind(made.home_node_id)
        .execute(conn)
        .await?
        .rows_affected();
        Ok(inserted > 0)
    }

    /// Moves an environment from one of the statuses `from` to `to`, with a sentence about it, and
    /// stamps its feed. Returns whether it was in one of them: a status is never taken for granted.
    pub async fn set_status(
        conn: &mut Conn,
        id: Id,
        from: &[&str],
        to: &str,
        detail: Option<&str>,
        at: DateTime<Utc>,
    ) -> Result<bool, DbError> {
        let moved = sqlx::query(
            "UPDATE environments SET status = $3, status_detail = $4, updated_at = $5
              WHERE id = $1 AND status = ANY($2) AND deleted_at IS NULL",
        )
        .bind(id)
        .bind(
            from.iter()
                .map(|word| (*word).to_owned())
                .collect::<Vec<_>>(),
        )
        .bind(to)
        .bind(detail)
        .bind(at)
        .execute(&mut *conn)
        .await?
        .rows_affected();
        if moved > 0 {
            stamp_environment(conn, id, at).await?;
        }
        Ok(moved > 0)
    }

    /// What an environment is given when it is up.
    #[derive(Debug, Clone, PartialEq)]
    pub struct Up<'a> {
        /// The machine of its connector.
        pub machine: Id,
        /// Where its workspace is, as its connector reported it.
        pub workspace_root: Option<&'a str>,
        /// What its connector reported of it.
        pub facts: Option<&'a Json>,
        /// Whether this is a unit made anew: its generation goes up.
        pub new_unit: bool,
    }

    /// Makes an environment `running` on a machine, from the statuses `from`. Returns whether it
    /// was in one of them.
    pub async fn make_running(
        conn: &mut Conn,
        id: Id,
        from: &[&str],
        up: &Up<'_>,
        at: DateTime<Utc>,
    ) -> Result<bool, DbError> {
        let moved = sqlx::query(
            "UPDATE environments
                SET loss_missing=false,status = 'running', status_detail = NULL, machine_id = $3,
                    workspace_root = COALESCE($4, workspace_root), facts = COALESCE($5, facts),
                    generation = generation + CASE WHEN $6 THEN 1 ELSE 0 END,
                    last_activity_at = GREATEST(COALESCE(last_activity_at, $7), $7), updated_at = $7
              WHERE id = $1 AND status = ANY($2) AND deleted_at IS NULL",
        )
        .bind(id)
        .bind(
            from.iter()
                .map(|word| (*word).to_owned())
                .collect::<Vec<_>>(),
        )
        .bind(up.machine)
        .bind(up.workspace_root)
        .bind(up.facts)
        .bind(up.new_unit)
        .bind(at)
        .execute(&mut *conn)
        .await?
        .rows_affected();
        if moved > 0 {
            stamp_environment(conn, id, at).await?;
        }
        Ok(moved > 0)
    }

    /// Notes that something was done in an environment at `at`, whatever it was: the clock of the
    /// idle stop starts over. Unlike the end of a turn this may also set the time back.
    pub async fn keep_awake(conn: &mut Conn, id: Id, at: DateTime<Utc>) -> Result<bool, DbError> {
        let kept = sqlx::query(
            "UPDATE environments SET last_activity_at = $2, updated_at = $2
              WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(id)
        .bind(at)
        .execute(&mut *conn)
        .await?
        .rows_affected();
        if kept > 0 {
            stamp_environment(conn, id, at).await?;
        }
        Ok(kept > 0)
    }

    /// Records what an environment's connector reported of it: its facts, and where its workspace
    /// is. Returns whether the environment is there.
    pub async fn record_facts(
        conn: &mut Conn,
        id: Id,
        facts: &Json,
        workspace_root: Option<&str>,
        at: DateTime<Utc>,
    ) -> Result<bool, DbError> {
        let recorded = sqlx::query(
            "UPDATE environments SET facts = $2, workspace_root = COALESCE($3, workspace_root), updated_at = $4
              WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(id)
        .bind(facts)
        .bind(workspace_root)
        .bind(at)
        .execute(&mut *conn)
        .await?
        .rows_affected();
        if recorded > 0 {
            stamp_environment(conn, id, at).await?;
        }
        Ok(recorded > 0)
    }

    /// Withdraws a scheduled removal that has not begun, and starts the count of unused days over.
    /// Returns whether there was one.
    pub async fn keep(conn: &mut Conn, id: Id, at: DateTime<Utc>) -> Result<bool, DbError> {
        let kept = sqlx::query(
            "UPDATE environments
                SET remove_at = NULL, unused_since = CASE WHEN unused_since IS NULL THEN NULL ELSE $2 END,
                    removal_phase = NULL, removed_keep_days = NULL, retained_volumes = '[]',
                    deleted_by = NULL, removal_revision = removal_revision + 1,
                    revision = revision + 1, updated_at = $2
              WHERE id = $1 AND deleted_at IS NULL AND remove_at IS NOT NULL
                AND (removal_phase IS NULL OR removal_phase = 'scheduled')",
        )
        .bind(id)
        .bind(at)
        .execute(&mut *conn)
        .await?
        .rows_affected();
        if kept > 0 {
            stamp_environment(conn, id, at).await?;
        }
        Ok(kept > 0)
    }

    /// Marks an environment deleted once its unit and its volumes are gone.
    pub async fn mark_deleted(conn: &mut Conn, id: Id, at: DateTime<Utc>) -> Result<bool, DbError> {
        let gone = sqlx::query(
            "UPDATE environments SET deleted_at = $2, machine_id = NULL, status = 'none',
                    status_detail = NULL, updated_at = $2, revision = revision + 1
              WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(id)
        .bind(at)
        .execute(&mut *conn)
        .await?
        .rows_affected();
        Ok(gone > 0)
    }

    /// A runtime, as stored.
    #[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
    pub struct RuntimeFull {
        /// Exact owned installed-files source, retained even when its archived unit is absent.
        #[sqlx(default)]
        pub kept_image: Option<Json>,
        /// Its identifier.
        pub id: Id,
        /// `environment` or `provider`.
        pub kind: String,
        /// The host it is on.
        pub host_id: Id,
        /// The machine of its connector, once that has enrolled.
        pub machine_id: Option<Id>,
        /// The name of its unit.
        pub unit_name: String,
        /// The runtime it is preparing to take over from.
        pub successor_of: Option<Id>,
        /// The provider it is for.
        pub provider_id: Option<Id>,
        /// The environment it is for.
        pub environment_id: Option<Id>,
        /// The driver that runs it.
        pub driver: String,
        /// The template it was made from.
        pub template_id: Id,
        /// The hash of what it was made with.
        pub spec_hash: Vec<u8>,
        /// How many times it was made.
        pub generation: i32,
        /// Where it stands.
        pub state: String,
        /// A sentence about that.
        pub state_detail: Option<String>,
        /// When its unit may be removed.
        pub remove_at: Option<DateTime<Utc>>,
        /// Its volumes, by what they are for.
        pub volumes: Json,
        /// When it last had a session or a turn.
        pub last_active_at: Option<DateTime<Utc>>,
        /// When it was made.
        pub created_at: DateTime<Utc>,
        /// When it last changed.
        pub updated_at: DateTime<Utc>,
    }

    const RUNTIME_COLUMNS: &str = "id, kind, host_id, machine_id, unit_name, successor_of, provider_id, environment_id,
        driver, template_id, spec_hash, generation, state, state_detail, remove_at, volumes, kept_image, last_active_at,
        created_at, updated_at";

    /// The exact runtime rows a host or fleet page is showing, read together.
    pub async fn runtimes_by(conn: &mut Conn, ids: &[Id]) -> Result<Vec<RuntimeFull>, DbError> {
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {RUNTIME_COLUMNS} FROM runtimes WHERE id = ANY($1) ORDER BY id"
        )))
        .bind(ids)
        .fetch_all(conn)
        .await?)
    }

    /// Current units of the exact environment and provider lists being projected.
    pub async fn live_runtimes_of(
        conn: &mut Conn,
        environments: &[Id],
        providers: &[Id],
    ) -> Result<Vec<RuntimeFull>, DbError> {
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {RUNTIME_COLUMNS} FROM runtimes WHERE successor_of IS NULL AND state NOT IN ('retired','removed')
                AND ((kind='environment' AND environment_id=ANY($1)) OR (kind='provider' AND provider_id=ANY($2))) ORDER BY id"
        ))).bind(environments).bind(providers).fetch_all(conn).await?)
    }

    /// The runtime that serves an environment now: the one that is neither retired nor removed and
    /// is nobody's successor in the making.
    pub async fn live_runtime(
        conn: &mut Conn,
        environment: Id,
    ) -> Result<Option<RuntimeFull>, DbError> {
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {RUNTIME_COLUMNS} FROM runtimes
              WHERE environment_id = $1 AND kind = 'environment' AND successor_of IS NULL
                AND state NOT IN ('retired', 'removed')"
        )))
        .bind(environment)
        .fetch_optional(conn)
        .await?)
    }

    /// The runtime `id`.
    pub async fn runtime(conn: &mut Conn, id: Id) -> Result<Option<RuntimeFull>, DbError> {
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {RUNTIME_COLUMNS} FROM runtimes WHERE id = $1"
        )))
        .bind(id)
        .fetch_optional(conn)
        .await?)
    }

    /// The runtimes of a host, whoever's they are, newest first.
    pub async fn runtimes_on(
        conn: &mut Conn,
        host: Id,
        kind: Option<&str>,
    ) -> Result<Vec<RuntimeFull>, DbError> {
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {RUNTIME_COLUMNS} FROM runtimes
              WHERE host_id = $1 AND ($2::text IS NULL OR kind = $2) AND state <> 'removed'
              ORDER BY created_at DESC, id"
        )))
        .bind(host)
        .bind(kind)
        .fetch_all(conn)
        .await?)
    }

    /// A runtime to make.
    #[derive(Debug, Clone, PartialEq)]
    pub struct NewRuntime<'a> {
        /// Its identifier.
        pub id: Id,
        /// The host it is on.
        pub host_id: Id,
        /// The name of its unit.
        pub unit_name: &'a str,
        /// The environment it is for.
        pub environment_id: Id,
        /// The driver that runs it.
        pub driver: &'a str,
        /// The template it is made from.
        pub template_id: Id,
        /// The hash of what it is made with.
        pub spec_hash: &'a [u8],
        /// Which unit it is of its environment.
        pub generation: i32,
        /// Its volumes, by what they are for.
        pub volumes: &'a Json,
    }

    /// Makes the runtime of an environment, `creating`. The one live runtime of an environment is a
    /// rule of the schema: a second is a [`DbError::Unique`].
    pub async fn insert_runtime(
        conn: &mut Conn,
        made: &NewRuntime<'_>,
        at: DateTime<Utc>,
    ) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO runtimes (id, kind, host_id, unit_name, environment_id, driver, template_id, spec_hash,
                                   generation, state, volumes, created_at, updated_at)
             VALUES ($1, 'environment', $2, $3, $4, $5, $6, $7, $8, 'creating', $9, $10, $10)",
        )
        .bind(made.id)
        .bind(made.host_id)
        .bind(made.unit_name)
        .bind(made.environment_id)
        .bind(made.driver)
        .bind(made.template_id)
        .bind(made.spec_hash)
        .bind(made.generation)
        .bind(made.volumes)
        .bind(at)
        .execute(conn)
        .await?;
        Ok(())
    }

    /// Moves a runtime from one of the states `from` to `to`, with a sentence about it. Returns
    /// whether it was in one of them.
    pub async fn set_runtime_state(
        conn: &mut Conn,
        id: Id,
        from: &[&str],
        to: &str,
        detail: Option<&str>,
        at: DateTime<Utc>,
    ) -> Result<bool, DbError> {
        let moved = sqlx::query(
            "UPDATE runtimes SET state = $3, state_detail = $4, updated_at = $5
              WHERE id = $1 AND state = ANY($2)",
        )
        .bind(id)
        .bind(
            from.iter()
                .map(|word| (*word).to_owned())
                .collect::<Vec<_>>(),
        )
        .bind(to)
        .bind(detail)
        .bind(at)
        .execute(conn)
        .await?
        .rows_affected();
        Ok(moved > 0)
    }

    /// Says that a runtime was active at `at`: it had a session or a turn.
    pub async fn touch_runtime(conn: &mut Conn, id: Id, at: DateTime<Utc>) -> Result<(), DbError> {
        sqlx::query(
            "UPDATE runtimes SET last_active_at = GREATEST(COALESCE(last_active_at, $2), $2) WHERE id = $1",
        )
        .bind(id)
        .bind(at)
        .execute(conn)
        .await?;
        Ok(())
    }

    /// Retires a runtime: replaced, or its environment removed, and waiting for the reaper from `at`.
    pub async fn retire_runtime(
        conn: &mut Conn,
        id: Id,
        detail: Option<&str>,
        at: DateTime<Utc>,
    ) -> Result<bool, DbError> {
        let retired = sqlx::query(
            "UPDATE runtimes SET state = 'retired', state_detail = $2, remove_at = $3, updated_at = $3
              WHERE id = $1 AND state NOT IN ('retired', 'removed')",
        )
        .bind(id)
        .bind(detail)
        .bind(at)
        .execute(conn)
        .await?
        .rows_affected();
        Ok(retired > 0)
    }

    /// Notes that a runtime's removal failed, and when it is tried again: the pause grows with the
    /// attempts, and the reason is shown on the host.
    pub async fn postpone_removal(
        conn: &mut Conn,
        id: Id,
        detail: &str,
        again_at: DateTime<Utc>,
        at: DateTime<Utc>,
    ) -> Result<(), DbError> {
        sqlx::query(
            "UPDATE runtimes SET remove_at = $3, state_detail = $2, updated_at = $4 WHERE id = $1",
        )
        .bind(id)
        .bind(detail)
        .bind(again_at)
        .bind(at)
        .execute(conn)
        .await?;
        Ok(())
    }

    /// The retired runtimes whose time has come, and the runtimes of environments whose removal is
    /// due (for those the runtime is the environment's current one).
    pub async fn runtimes_due(
        conn: &mut Conn,
        now: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<RuntimeFull>, DbError> {
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {RUNTIME_COLUMNS} FROM runtimes
              WHERE state = 'retired' AND lost_at IS NULL AND remove_at IS NOT NULL AND remove_at <= $1
              ORDER BY remove_at, id LIMIT $2"
        )))
        .bind(now)
        .bind(limit)
        .fetch_all(conn)
        .await?)
    }

    /// The environments whose removal is due and that are not deleted yet.
    pub async fn environments_due(
        conn: &mut Conn,
        now: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<Id>, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT e.id FROM environments e
              WHERE e.deleted_at IS NULL AND e.remove_at IS NOT NULL AND e.remove_at <= $1
                AND NOT EXISTS (SELECT 1 FROM jobs j WHERE j.environment_id=e.id
                    AND j.kind='environment_remove' AND j.state IN ('queued','running'))
              ORDER BY e.remove_at, e.id LIMIT $2",
        )
        .bind(now)
        .bind(limit)
        .fetch_all(conn)
        .await?)
    }

    /// Managed environments that are running and idle: nothing happened in them since `before`, with
    /// the threads bound to them counted, and which have none of the turns that keep them up. The
    /// caller applies the setting of each.
    pub async fn running_environments(
        conn: &mut Conn,
        limit: i64,
    ) -> Result<Vec<EnvironmentState>, DbError> {
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {STATE_COLUMNS} FROM environments
              WHERE placement = 'managed' AND status = 'running' AND deleted_at IS NULL
              ORDER BY COALESCE(last_activity_at, created_at), id LIMIT $1"
        )))
        .bind(limit)
        .fetch_all(conn)
        .await?)
    }

    /// How many turns of threads bound to an environment are not over.
    pub async fn active_turns(conn: &mut Conn, environment: Id) -> Result<i64, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT count(*) FROM turns u JOIN threads t ON t.node_id = u.thread_id
              WHERE t.environment_id = $1 AND u.state IN ('starting', 'running')",
        )
        .bind(environment)
        .fetch_one(conn)
        .await?)
    }

    /// How many shells of an environment are open.
    pub async fn open_shells(conn: &mut Conn, environment: Id) -> Result<i64, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT count(*) FROM terminal_sessions WHERE environment_id = $1 AND state <> 'closed'",
        )
        .bind(environment)
        .fetch_one(conn)
        .await?)
    }

    /// Sets, for the managed environments `ids`, the instant from which no thread has been bound to
    /// them (none while one is), and returns those whose count of unused days has run out.
    pub async fn refresh_unused(conn: &mut Conn, at: DateTime<Utc>) -> Result<(), DbError> {
        // Newly empty ones start counting; ones with a thread again stop. Archived threads do not
        // count as bound.
        sqlx::query(
            "UPDATE environments e SET unused_since = $1
              WHERE e.placement = 'managed' AND e.deleted_at IS NULL AND e.unused_since IS NULL
                AND NOT EXISTS (SELECT 1 FROM threads t JOIN nodes n ON n.id = t.node_id
                                 WHERE t.environment_id = e.id AND n.deleted_at IS NULL AND n.archived_at IS NULL)",
        )
        .bind(at)
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "UPDATE environments e SET unused_since = NULL
              WHERE e.placement = 'managed' AND e.deleted_at IS NULL AND e.unused_since IS NOT NULL
                AND EXISTS (SELECT 1 FROM threads t JOIN nodes n ON n.id = t.node_id
                             WHERE t.environment_id = e.id AND n.deleted_at IS NULL AND n.archived_at IS NULL)",
        )
        .execute(conn)
        .await?;
        Ok(())
    }

    /// The managed environments that have been unused since `unused_since` and have no removal
    /// scheduled: candidates for the removal of an environment unused for too long.
    pub async fn unused_candidates(
        conn: &mut Conn,
        limit: i64,
    ) -> Result<Vec<EnvironmentState>, DbError> {
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {STATE_COLUMNS} FROM environments
              WHERE placement = 'managed' AND deleted_at IS NULL AND remove_at IS NULL AND unused_since IS NOT NULL
              ORDER BY unused_since, id LIMIT $1"
        )))
        .bind(limit)
        .fetch_all(conn)
        .await?)
    }

    /// Schedules the removal of an environment that has none scheduled. Returns whether it did.
    pub async fn schedule_removal(
        conn: &mut Conn,
        id: Id,
        remove_at: DateTime<Utc>,
        at: DateTime<Utc>,
    ) -> Result<bool, DbError> {
        let scheduled = sqlx::query(
            "UPDATE environments SET remove_at = $2, revision = revision + 1, updated_at = $3
              WHERE id = $1 AND deleted_at IS NULL AND remove_at IS NULL",
        )
        .bind(id)
        .bind(remove_at)
        .bind(at)
        .execute(&mut *conn)
        .await?
        .rows_affected();
        if scheduled > 0 {
            stamp_environment(conn, id, at).await?;
        }
        Ok(scheduled > 0)
    }

    /// The host of the newest runtime an environment has had, whatever became of it: where its volumes
    /// are, once it has been made.
    pub async fn last_host_of(conn: &mut Conn, environment: Id) -> Result<Option<Id>, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT host_id FROM runtimes WHERE environment_id = $1 AND kind = 'environment'
              ORDER BY created_at DESC, id DESC LIMIT 1",
        )
        .bind(environment)
        .fetch_optional(conn)
        .await?)
    }

    /// The host of the environment's published generation and current template.
    /// An unpublished or failed move candidate is not its admitted own placement.
    pub async fn published_host_of(
        conn: &mut Conn,
        environment: Id,
    ) -> Result<Option<Id>, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT r.host_id FROM environments e JOIN runtimes r ON r.environment_id=e.id
             AND r.generation=e.generation AND r.template_id=e.template_id
             WHERE e.id=$1 AND e.generation>0 AND e.deleted_at IS NULL AND r.kind='environment'
             ORDER BY r.created_at DESC,r.id DESC LIMIT 1",
        )
        .bind(environment)
        .fetch_optional(conn)
        .await?)
    }

    /// The executors that the threads bound to an environment work through.
    pub async fn executors_of(conn: &mut Conn, environment: Id) -> Result<Vec<String>, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT DISTINCT t.executor FROM threads t JOIN nodes n ON n.id = t.node_id
              WHERE t.environment_id = $1 AND t.executor IS NOT NULL AND n.deleted_at IS NULL
              ORDER BY 1",
        )
        .bind(environment)
        .fetch_all(conn)
        .await?)
    }

    /// Closes the shells of an environment whose unit stops or goes: nothing outlives it.
    pub async fn close_shells(
        conn: &mut Conn,
        environment: Id,
        at: DateTime<Utc>,
    ) -> Result<u64, DbError> {
        Ok(sqlx::query(
            "UPDATE terminal_sessions SET state = 'closed', closed_at = $2, close_reason = 'environment_stopped'
              WHERE environment_id = $1 AND state <> 'closed'",
        )
        .bind(environment)
        .bind(at)
        .execute(conn)
        .await?
        .rows_affected())
    }

    /// The interactive handles whose processes ended with a confirmed same-unit restart stop.
    /// Announced endpoints lose this environment's session too; their outside servers are untouched.
    pub async fn close_restart_sessions(
        conn: &mut Conn,
        environment: Id,
        at: DateTime<Utc>,
    ) -> Result<(), DbError> {
        close_shells(conn, environment, at).await?;
        sqlx::query("UPDATE browser_sessions SET state='closed',closed_at=$2,close_reason='environment_stopped' WHERE environment_id=$1 AND state<>'closed'")
            .bind(environment).bind(at).execute(&mut *conn).await?;
        sqlx::query("UPDATE display_sessions SET state='closed',closed_at=$2,close_reason='environment_stopped' WHERE environment_id=$1 AND state<>'closed'")
            .bind(environment).bind(at).execute(conn).await?;
        Ok(())
    }

    /// How many removals of an environment (or, for a retired runtime, of that runtime) have failed:
    /// what the pause before the next attempt grows with.
    pub async fn failed_removals(
        conn: &mut Conn,
        environment: Id,
        runtime: Option<Id>,
    ) -> Result<i64, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT count(*) FROM jobs
              WHERE environment_id = $1 AND kind = 'environment_remove' AND state = 'failed'
                AND ($2::text IS NULL OR plan ->> 'runtime' = $2)",
        )
        .bind(environment)
        .bind(runtime.map(|id| id.to_string()))
        .fetch_one(conn)
        .await?)
    }

    /// Whether a volume of the host is named by a runtime other than `except` that is still somebody's:
    /// a retired runtime removes only what its successor does not use.
    pub async fn volume_in_use(
        conn: &mut Conn,
        host: Id,
        volume: &str,
        except: Id,
    ) -> Result<bool, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS (
               SELECT 1 FROM runtimes r, jsonb_each(r.volumes) AS v(role, body)
                WHERE r.host_id = $1 AND r.id <> $3 AND r.state <> 'removed' AND v.body ->> 'name' = $2)",
        )
        .bind(host)
        .bind(volume)
        .bind(except)
        .fetch_one(conn)
        .await?)
    }

    /// Notes the entries the creator used on the host and on the template when the host was chosen.
    pub async fn set_entries(
        conn: &mut Conn,
        id: Id,
        host_entry: Option<Id>,
        template_entry: Option<Id>,
    ) -> Result<(), DbError> {
        sqlx::query(
            "UPDATE environments SET host_entry_id = $2, template_entry_id = $3 WHERE id = $1",
        )
        .bind(id)
        .bind(host_entry)
        .bind(template_entry)
        .execute(conn)
        .await?;
        Ok(())
    }

    /// Dates the next attempt to remove an environment after one failed, and says why where the
    /// environment is shown.
    pub async fn postpone_environment_removal(
        conn: &mut Conn,
        id: Id,
        detail: &str,
        again_at: DateTime<Utc>,
        at: DateTime<Utc>,
    ) -> Result<(), DbError> {
        sqlx::query(
            "UPDATE environments SET remove_at = $2, status_detail = $3, updated_at = $4
              WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(id)
        .bind(again_at)
        .bind(detail)
        .bind(at)
        .execute(conn)
        .await?;
        Ok(())
    }

    /// The root org of a node that is not deleted.
    pub async fn root_org_of(conn: &mut Conn, node: Id) -> Result<Option<Id>, DbError> {
        Ok(
            sqlx::query_scalar("SELECT org_id FROM nodes WHERE id = $1 AND deleted_at IS NULL")
                .bind(node)
                .fetch_optional(conn)
                .await?,
        )
    }

    /// The providers whose credential is handed over to an environment.
    pub async fn providers_holding(conn: &mut Conn, environment: Id) -> Result<Vec<Id>, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT id FROM providers WHERE held_by_environment_id = $1 AND deleted_at IS NULL",
        )
        .bind(environment)
        .fetch_all(conn)
        .await?)
    }

    /// The topic a thread lies in, for the feed of the topic.
    pub async fn threads_of(conn: &mut Conn, environment: Id) -> Result<Vec<Id>, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT t.node_id FROM threads t JOIN nodes n ON n.id = t.node_id
              WHERE t.environment_id = $1 AND n.deleted_at IS NULL ORDER BY t.node_id",
        )
        .bind(environment)
        .fetch_all(conn)
        .await?)
    }

    /// The runtimes that stand `stopping`, whose stop was begun before `before`: a stop that a process
    /// that died did not finish.
    pub async fn stale_stops(
        conn: &mut Conn,
        before: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<RuntimeFull>, DbError> {
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {RUNTIME_COLUMNS} FROM runtimes
              WHERE kind = 'environment' AND state = 'stopping' AND updated_at <= $1
                AND NOT EXISTS(SELECT 1 FROM jobs j WHERE j.environment_id=runtimes.environment_id AND j.kind='environment_restart' AND j.state IN ('queued','running'))
              ORDER BY updated_at, id LIMIT $2"
        )))
        .bind(before)
        .bind(limit)
        .fetch_all(conn)
        .await?)
    }

    /// The people who are told of what happens to an environment: its owner, and every person its
    /// page names for `environment.manage` or as an owner.
    pub async fn managers_of(conn: &mut Conn, id: Id) -> Result<Vec<Id>, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT owner_id FROM environments WHERE id = $1
             UNION
             SELECT holder_id FROM permission_entries
              WHERE object_kind = 'environment' AND object_id = $1 AND page = 'live'
                AND permission IN ('environment.manage', 'access.own')
                AND holder_kind = 'user' AND holder_id IS NOT NULL",
        )
        .bind(id)
        .fetch_all(conn)
        .await?)
    }

    // ------------------------------------------------------------ what the requests read of them

    /// The host each of the environments `ids` is on, by its unit that is not retired or removed: the
    /// environment, the host and the host's name.
    pub async fn hosts_of(conn: &mut Conn, ids: &[Id]) -> Result<Vec<(Id, Id, String)>, DbError> {
        Ok(sqlx::query_as(
            "SELECT DISTINCT ON (r.environment_id) r.environment_id, h.id, h.name
               FROM runtimes r JOIN hosts h ON h.id = r.host_id
              WHERE r.environment_id = ANY($1) AND r.kind = 'environment'
                AND r.state NOT IN ('retired', 'removed')
              ORDER BY r.environment_id, r.created_at DESC",
        )
        .bind(ids)
        .fetch_all(conn)
        .await?)
    }

    /// Why the removal of each of the environments `ids` that has one was scheduled: the word the
    /// schedule wrote when it did it itself, else `request`.
    pub async fn removal_reasons(
        conn: &mut Conn,
        ids: &[Id],
    ) -> Result<Vec<(Id, String)>, DbError> {
        Ok(sqlx::query_as(
            "SELECT DISTINCT ON (object_id) object_id, COALESCE(details ->> 'reason', 'request')
               FROM audit_log
              WHERE action = 'environment.removal_scheduled' AND object_kind = 'environment'
                AND object_id = ANY($1)
              ORDER BY object_id, at DESC",
        )
        .bind(ids)
        .fetch_all(conn)
        .await?)
    }

    /// The environments that threads under a node run in, and those whose home is under it.
    pub async fn environments_under(conn: &mut Conn, node: Id) -> Result<Vec<Id>, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT e.id FROM environments e JOIN nodes n ON n.id = e.home_node_id
              WHERE e.deleted_at IS NULL AND n.deleted_at IS NULL AND $1 = ANY(n.path)
             UNION
             SELECT e.id FROM threads t
               JOIN nodes n ON n.id = t.node_id
               JOIN environments e ON e.id = t.environment_id
              WHERE e.deleted_at IS NULL AND n.deleted_at IS NULL AND $1 = ANY(n.path)
             ORDER BY 1",
        )
        .bind(node)
        .fetch_all(conn)
        .await?)
    }

    /// How many threads that are not archived or deleted are bound to each of the environments `ids`.
    /// The same home and binding set for several nodes, read together for topic summaries and cards.
    pub async fn environments_under_many(
        conn: &mut Conn,
        nodes: &[Id],
    ) -> Result<Vec<(Id, Id)>, DbError> {
        Ok(sqlx::query_as(
            "SELECT place.id, e.id FROM unnest($1::uuid[]) place(id)
               JOIN nodes n ON place.id = ANY(n.path) JOIN environments e ON e.home_node_id = n.id
              WHERE e.deleted_at IS NULL AND n.deleted_at IS NULL
             UNION
             SELECT place.id, e.id FROM unnest($1::uuid[]) place(id)
               JOIN nodes n ON place.id = ANY(n.path) JOIN threads t ON t.node_id = n.id
               JOIN environments e ON e.id = t.environment_id
              WHERE e.deleted_at IS NULL AND n.deleted_at IS NULL ORDER BY 1,2",
        )
        .bind(nodes)
        .fetch_all(conn)
        .await?)
    }

    /// How many threads that are not archived or deleted are bound to each of the environments `ids`.
    pub async fn thread_counts(conn: &mut Conn, ids: &[Id]) -> Result<Vec<(Id, i64)>, DbError> {
        Ok(sqlx::query_as(
            "SELECT t.environment_id, count(*) FROM threads t JOIN nodes n ON n.id = t.node_id
              WHERE t.environment_id = ANY($1) AND n.deleted_at IS NULL AND n.archived_at IS NULL
              GROUP BY t.environment_id",
        )
        .bind(ids)
        .fetch_all(conn)
        .await?)
    }

    // ------------------------------------------------------------ directories on people's machines

    /// The statuses of an environment on a person's machine that its machine's session moves between.
    const CONNECTED_STATUSES: [&str; 5] = ["none", "offline", "running", "stopped", "failed"];

    /// A directory on a person's machine, with where its machine stands: what the schedule needs to
    /// bring the status of the environment in step with the session of the machine.
    #[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
    pub struct Connected {
        /// The environment.
        pub id: Id,
        /// Its machine.
        pub machine_id: Id,
        /// The directory, as it was registered.
        pub workspace_root: String,
        /// Where the environment stands.
        pub status: String,
        /// A sentence about that.
        pub status_detail: Option<String>,
        /// When the environment last changed.
        pub updated_at: DateTime<Utc>,
        /// When its removal is due, if it is scheduled.
        pub remove_at: Option<DateTime<Utc>>,
        /// The session of its machine that `env.attach` was last done in.
        pub attached_session: Option<DateTime<Utc>>,
        /// Whether its machine has a session now, and the system accepts it.
        pub here: bool,
        /// Whether its machine takes the role of an environment.
        pub serves: bool,
        /// When the session of its machine began: the moment of the last word about its presence.
        pub session: Option<DateTime<Utc>>,
    }

    impl Connected {
        /// Whether the environment stands attached in the session its machine has now: it is
        /// `running`, and the attach was done in this session and not in an earlier one.
        #[must_use]
        pub fn attached(&self) -> bool {
            self.here
                && matches!(self.status.as_str(), "running" | "stopped")
                && self.attached_session == self.session
        }
    }

    const CONNECTED_SELECT: &str =
        "SELECT e.id, e.machine_id, e.workspace_root, e.status, e.status_detail, e.updated_at,
               e.remove_at, e.attached_session,
               (m.online AND m.status = 'active' AND m.deleted_at IS NULL) AS here,
               ('environment' = ANY(m.roles)) AS serves,
               COALESCE(m.session_started_at,m.last_seen_at) AS session
          FROM environments e JOIN machines m ON m.id = e.machine_id";

    /// The environment `id` on a person's machine, with where its machine stands.
    pub async fn connected(conn: &mut Conn, id: Id) -> Result<Option<Connected>, DbError> {
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "{CONNECTED_SELECT} WHERE e.id = $1 AND e.placement = 'connected' AND e.deleted_at IS NULL"
        )))
        .bind(id)
        .fetch_optional(conn)
        .await?)
    }

    /// The environments on people's machines whose status does not follow their machine: one that
    /// says it is up while its machine has no session, and one whose machine has a session that the
    /// attach was not tried in (or that stands `none` or `offline` in one). One that failed to attach
    /// in the session its machine has is only listed again once `retry_before` has passed since it
    /// did; in a later session it is listed at once. An environment whose removal is scheduled is
    /// left alone.
    pub async fn connected_out_of_step(
        conn: &mut Conn,
        retry_before: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<Connected>, DbError> {
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "{CONNECTED_SELECT}
              WHERE e.placement = 'connected' AND e.deleted_at IS NULL AND e.remove_at IS NULL
                AND e.status = ANY($3)
                AND (
                      (e.status = 'running' AND NOT (m.online AND m.status = 'active' AND m.deleted_at IS NULL))
                   OR (m.online AND m.status = 'active' AND m.deleted_at IS NULL
                       AND (e.attached_session IS DISTINCT FROM COALESCE(m.session_started_at,m.last_seen_at)
                            OR e.status IN ('none', 'offline')
                            OR (e.status = 'failed' AND e.updated_at <= $1)))
                    )
              ORDER BY e.id LIMIT $2"
        )))
        .bind(retry_before)
        .bind(limit)
        .bind(CONNECTED_STATUSES.iter().map(|word| (*word).to_owned()).collect::<Vec<_>>())
        .fetch_all(conn)
        .await?)
    }

    /// Records that `env.attach` was done for an environment on a person's machine in the session of
    /// the machine that began at `session`: it is `running`, with the facts its connector gave. Returns
    /// whether it was still one that the session moves (not removed, not switched off).
    pub async fn connected_attached(
        conn: &mut Conn,
        id: Id,
        session: Option<DateTime<Utc>>,
        facts: &Json,
        at: DateTime<Utc>,
    ) -> Result<bool, DbError> {
        let moved = sqlx::query(
            "UPDATE environments
                SET status = CASE WHEN status='stopped' THEN 'stopped' ELSE 'running' END, status_detail = NULL, attached_session = $2, facts = $3, updated_at = $4
              WHERE id = $1 AND placement = 'connected' AND deleted_at IS NULL AND remove_at IS NULL
                AND status = ANY($5)",
        )
        .bind(id)
        .bind(session)
        .bind(facts)
        .bind(at)
        .bind(CONNECTED_STATUSES.iter().map(|word| (*word).to_owned()).collect::<Vec<_>>())
        .execute(&mut *conn)
        .await?
        .rows_affected();
        if moved > 0 {
            stamp_environment(conn, id, at).await?;
        }
        Ok(moved > 0)
    }

    /// Records that an environment on a person's machine could not be attached in the session of its
    /// machine that began at `session`: it is `failed`, with why, and is tried again once a while has
    /// passed or when the machine has another session. Returns whether it was still one that the
    /// session moves.
    pub async fn connected_attach_failed(
        conn: &mut Conn,
        id: Id,
        session: Option<DateTime<Utc>>,
        detail: &str,
        at: DateTime<Utc>,
    ) -> Result<bool, DbError> {
        let moved = sqlx::query(
            "UPDATE environments
                SET status = 'failed', status_detail = $2, attached_session = $5, updated_at = $3
              WHERE id = $1 AND placement = 'connected' AND deleted_at IS NULL AND remove_at IS NULL
                AND status = ANY($4)",
        )
        .bind(id)
        .bind(detail)
        .bind(at)
        .bind(
            CONNECTED_STATUSES
                .iter()
                .map(|word| (*word).to_owned())
                .collect::<Vec<_>>(),
        )
        .bind(session)
        .execute(&mut *conn)
        .await?
        .rows_affected();
        if moved > 0 {
            stamp_environment(conn, id, at).await?;
        }
        Ok(moved > 0)
    }
}

// ---------------------------------------------------------------- what the capabilities of an environment read and write
//
// One block, kept together at the end of the file: the definitions of the capabilities, what an environment has and
// what it was made from, the keys it is given and has taken back, and the job that makes its unit anew. Nothing here
// decides anything.

/// The capabilities an environment has and can be given.
pub mod capabilities {
    use crate::{Conn, DbError};
    use chrono::{DateTime, Utc};
    use orch_types::{Id, Json};

    /// A capability as the catalogue defines it.
    #[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
    pub struct DefinitionRow {
        /// Its key.
        pub key: String,
        /// What it is called.
        pub label: String,
        /// What it is.
        pub description: String,
        /// `low`, `medium`, `high` or `critical`.
        pub risk: String,
        /// A patch for each driver that can apply it.
        pub driver_patch: Json,
        /// What happens to a request for it unless a policy says otherwise.
        pub default_decision: String,
        /// Whose approvers decide it unless a policy says otherwise.
        pub default_approver_scope: String,
    }

    /// The capability `key`.
    pub async fn definition(conn: &mut Conn, key: &str) -> Result<Option<DefinitionRow>, DbError> {
        Ok(sqlx::query_as(
            "SELECT key, label, description, risk, driver_patch, default_decision, default_approver_scope
               FROM capability_defs WHERE key = $1",
        )
        .bind(key)
        .fetch_optional(conn)
        .await?)
    }

    /// Every capability, by key.
    pub async fn definitions(conn: &mut Conn) -> Result<Vec<DefinitionRow>, DbError> {
        Ok(sqlx::query_as(
            "SELECT key, label, description, risk, driver_patch, default_decision, default_approver_scope
               FROM capability_defs ORDER BY key",
        )
        .fetch_all(conn)
        .await?)
    }

    /// What an environment has and what it was made from, as far as its capabilities go.
    #[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
    pub struct EnvironmentFacts {
        /// The environment.
        pub id: Id,
        /// Its name.
        pub name: String,
        /// `managed` or `connected`.
        pub placement: String,
        /// The node it takes its settings from.
        pub home_node_id: Id,
        /// The root org of its home.
        pub org_id: Id,
        /// The keys it was given.
        pub granted_capabilities: Vec<String>,
        /// The driver of the template it was made from.
        pub template_driver: Option<String>,
        /// The template's spec.
        pub template_spec: Option<Json>,
        /// What the entry its creator used on the template allows; none where the creator owned the template.
        pub entry_options: Option<Json>,
        /// How many times it was made anew.
        pub generation: i32,
        /// Where it stands.
        pub status: String,
        /// When it was deleted.
        pub deleted_at: Option<DateTime<Utc>>,
    }

    /// The environment `id`.
    pub async fn environment_facts(
        conn: &mut Conn,
        id: Id,
    ) -> Result<Option<EnvironmentFacts>, DbError> {
        Ok(sqlx::query_as(
            "SELECT e.id, e.name, e.placement, e.home_node_id, e.org_id, e.granted_capabilities,
                    t.driver AS template_driver, t.spec AS template_spec,
                    (SELECT p.options FROM permission_entries p
                      WHERE p.id = e.template_entry_id AND p.removed_at IS NULL) AS entry_options,
                    e.generation, e.status, e.deleted_at
               FROM environments e LEFT JOIN env_templates t ON t.id = e.template_id
              WHERE e.id = $1",
        )
        .bind(id)
        .fetch_optional(conn)
        .await?)
    }

    /// The host the unit of an environment is on and what that host reports of its drivers.
    pub async fn host_of(conn: &mut Conn, environment: Id) -> Result<Option<(Id, Json)>, DbError> {
        Ok(sqlx::query_as(
            "SELECT h.id, h.drivers FROM runtimes r JOIN hosts h ON h.id = r.host_id
              WHERE r.kind = 'environment' AND r.environment_id = $1
                AND r.state NOT IN ('retired', 'removed') AND r.successor_of IS NULL
              LIMIT 1",
        )
        .bind(environment)
        .fetch_optional(conn)
        .await?)
    }

    /// Adds a key to what an environment was given. The answer says whether this call did.
    pub async fn grant(
        conn: &mut Conn,
        environment: Id,
        key: &str,
        at: DateTime<Utc>,
    ) -> Result<bool, DbError> {
        let done = sqlx::query(
            "UPDATE environments
                SET granted_capabilities = array_append(granted_capabilities, $2),
                    revision = revision + 1, updated_at = $3
              WHERE id = $1 AND NOT ($2 = ANY(granted_capabilities))",
        )
        .bind(environment)
        .bind(key)
        .bind(at)
        .execute(conn)
        .await?;
        Ok(done.rows_affected() == 1)
    }

    /// Takes a key out of what an environment was given. The answer says whether this call did.
    pub async fn revoke(
        conn: &mut Conn,
        environment: Id,
        key: &str,
        at: DateTime<Utc>,
    ) -> Result<bool, DbError> {
        let done = sqlx::query(
            "UPDATE environments
                SET granted_capabilities = array_remove(granted_capabilities, $2),
                    revision = revision + 1, updated_at = $3
              WHERE id = $1 AND $2 = ANY(granted_capabilities)",
        )
        .bind(environment)
        .bind(key)
        .bind(at)
        .execute(conn)
        .await?;
        Ok(done.rows_affected() == 1)
    }

    /// A job that makes the unit of an environment anew, as it stands.
    #[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
    pub struct RecreateJob {
        /// The job.
        pub id: Id,
        /// The environment.
        pub environment_id: Id,
        /// `queued` until something runs it.
        pub state: String,
        /// What it was asked for.
        pub plan: Json,
        /// Who asked, where a person did.
        pub created_by: Option<Id>,
        /// When.
        pub created_at: DateTime<Utc>,
    }

    /// Queues the job that makes the unit of an environment anew, for the reason the plan says. Whatever runs
    /// the jobs of environments takes it from there.
    pub async fn queue_recreate(
        conn: &mut Conn,
        id: Id,
        environment: Id,
        plan: &Json,
        by: Option<Id>,
        at: DateTime<Utc>,
    ) -> Result<RecreateJob, DbError> {
        Ok(sqlx::query_as(
            "INSERT INTO jobs (id, kind, environment_id, state, plan, created_by, created_at, updated_at)
             VALUES ($1, 'environment_recreate', $2, 'queued', $3, $4, $5, $5)
             ON CONFLICT (id) DO UPDATE SET updated_at = jobs.updated_at
             RETURNING id, environment_id, state, plan, created_by, created_at",
        )
        .bind(id)
        .bind(environment)
        .bind(plan)
        .bind(by)
        .bind(at)
        .fetch_one(conn)
        .await?)
    }
}

// Runtime updates and moves: successors stay candidates until their checks pass.

/// Runtime replacement and host maintenance queries.
pub mod changes {
    use super::hosting::{self, RuntimeFull};
    use crate::{Conn, DbError};
    use chrono::{DateTime, Utc};
    use orch_types::{Id, Json};

    /// Connector versions of the named machines, read together for resource lists.
    pub async fn connector_versions(
        conn: &mut Conn,
        machines: &[Id],
    ) -> Result<Vec<(Id, String)>, DbError> {
        Ok(
            sqlx::query_as("SELECT id, connector_version FROM machines WHERE id = ANY($1)")
                .bind(machines)
                .fetch_all(conn)
                .await?,
        )
    }

    /// The runtimes and volume mounts that keep a host in service.
    pub async fn host_dependencies(
        conn: &mut Conn,
        host: Id,
        machine: Id,
    ) -> Result<(Vec<Id>, Vec<Id>), DbError> {
        let runtimes = sqlx::query_scalar(
            "SELECT id FROM runtimes WHERE host_id = $1 AND state <> 'removed' AND lost_at IS NULL ORDER BY id",
        )
        .bind(host)
        .fetch_all(&mut *conn)
        .await?;
        let mounts=sqlx::query_scalar("SELECT mount_id FROM mount_locations WHERE machine_id = $1 AND kind = 'volume' ORDER BY mount_id").bind(machine).fetch_all(conn).await?;
        Ok((runtimes, mounts))
    }

    /// Retires an empty host's registration while preserving references in runtime history.
    pub async fn forget_host(
        conn: &mut Conn,
        host: Id,
        machine: Id,
        now: DateTime<Utc>,
    ) -> Result<(), DbError> {
        sqlx::query("UPDATE hosts SET schedulable = false, revision = revision + 1, updated_at = $2 WHERE id = $1")
            .bind(host)
            .bind(now)
            .execute(&mut *conn)
            .await?;
        super::super::resources::revoke_machine(conn, machine, now).await?;
        sqlx::query("UPDATE machines SET deleted_at = $2 WHERE id = $1")
            .bind(machine)
            .bind(now)
            .execute(&mut *conn)
            .await?;
        sqlx::query("DELETE FROM mount_locations WHERE machine_id = $1 AND kind = 'path'")
            .bind(machine)
            .execute(conn)
            .await?;
        Ok(())
    }

    /// The shells closed when an environment move begins.
    pub async fn open_shells(conn: &mut Conn, environment: Id) -> Result<Vec<Id>, DbError> {
        Ok(sqlx::query_scalar("SELECT id FROM terminal_sessions WHERE environment_id = $1 AND state <> 'closed' ORDER BY id").bind(environment).fetch_all(conn).await?)
    }

    /// Moves a turn waiting for its unit to the segment opened by an update on wake.
    pub async fn starting_segment(
        conn: &mut Conn,
        turn: Id,
        segment: Id,
        seq: i64,
    ) -> Result<(), DbError> {
        sqlx::query("UPDATE turns SET segment_id = $2, seq_updated = $3 WHERE id = $1 AND state = 'starting'")
            .bind(turn).bind(segment).bind(seq).execute(conn).await?;
        Ok(())
    }

    /// Records the requested placement, preserving a previous runtime when explicitly requested.
    pub async fn moved_settings(
        conn: &mut Conn,
        environment: Id,
        host: Option<Id>,
        old: Id,
        keep: bool,
        now: DateTime<Utc>,
    ) -> Result<(), DbError> {
        sqlx::query("UPDATE environments SET config = jsonb_set(config,'{env.host_id}',COALESCE(to_jsonb($2::uuid),'null'::jsonb)),updated_at = $3 WHERE id = $1")
            .bind(environment).bind(host).bind(now).execute(&mut *conn).await?;
        if keep {
            sqlx::query("UPDATE runtimes SET remove_at = NULL WHERE id = $1")
                .bind(old)
                .execute(conn)
                .await?;
        }
        Ok(())
    }

    /// Creates a successor on the given volumes, without retiring the current runtime.
    pub async fn successor(
        conn: &mut Conn,
        old: &RuntimeFull,
        id: Id,
        name: &str,
        template: Id,
        hash: &[u8],
        now: DateTime<Utc>,
    ) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO runtimes (id, kind, host_id, machine_id, unit_name, successor_of,
              provider_id, environment_id, driver, template_id, spec_hash, generation, state, volumes, created_at, updated_at)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,'creating',$13,$14,$14)",
        ).bind(id).bind(&old.kind).bind(old.host_id).bind(old.machine_id).bind(name).bind(old.id)
            .bind(old.provider_id).bind(old.environment_id).bind(&old.driver).bind(template).bind(hash)
            .bind(old.generation.saturating_add(1)).bind(&old.volumes).bind(now).execute(conn).await?;
        Ok(())
    }

    /// Records the complete definition hash once the limits and exact program builds are known.
    pub async fn save_hash(
        conn: &mut Conn,
        runtime: Id,
        hash: &[u8],
        now: DateTime<Utc>,
    ) -> Result<(), DbError> {
        sqlx::query("UPDATE runtimes SET spec_hash = $2, updated_at = $3 WHERE id = $1 AND state <> 'removed'")
            .bind(runtime).bind(hash).bind(now).execute(conn).await?;
        Ok(())
    }

    /// Publishes a checked successor and retires only the old unit; shared volumes remain in use.
    pub async fn commit(
        conn: &mut Conn,
        old: Id,
        next: Id,
        facts: Option<&Json>,
        now: DateTime<Utc>,
    ) -> Result<bool, DbError> {
        let Some(candidate) = hosting::runtime(conn, next).await? else {
            return Ok(false);
        };
        if candidate.successor_of != Some(old) {
            return Ok(false);
        }
        sqlx::query("UPDATE runtimes SET state = 'retired', remove_at = $2, machine_id = CASE WHEN machine_id = $3 THEN NULL ELSE machine_id END, updated_at = $2 WHERE id = $1")
            .bind(old).bind(now).bind(candidate.machine_id).execute(&mut *conn).await?;
        sqlx::query("UPDATE runtimes SET successor_of = NULL, state = 'running', updated_at = $2 WHERE id = $1")
            .bind(next).bind(now).execute(&mut *conn).await?;
        if let Some(machine) = candidate.machine_id {
            sqlx::query(
                "UPDATE machines SET runtime_id = $2 WHERE id = $1 AND placement = 'managed'",
            )
            .bind(machine)
            .bind(next)
            .execute(&mut *conn)
            .await?;
        }
        if let Some(environment) = candidate.environment_id {
            sqlx::query("UPDATE environments SET loss_missing=false,machine_id = $2, generation = $3, status = 'running', status_detail = NULL,
                facts = COALESCE($4,facts), workspace_root = COALESCE($4 ->> 'workspace_root',workspace_root), last_activity_at = $5, updated_at = $5,template_id = $6 WHERE id = $1")
                .bind(environment).bind(candidate.machine_id).bind(candidate.generation).bind(facts).bind(now).bind(candidate.template_id).execute(&mut *conn).await?;
            super::super::jobs::stamp_environment(conn, environment, now).await?;
        }
        if let Some(provider) = candidate.provider_id {
            super::super::resources::provider_runtime::set_machine(
                conn,
                provider,
                candidate.machine_id,
                now,
            )
            .await?;
        }
        Ok(true)
    }

    /// Discards a candidate's identity association without revoking a machine shared with the old unit.
    pub async fn discard(conn: &mut Conn, next: Id, now: DateTime<Utc>) -> Result<(), DbError> {
        sqlx::query("UPDATE runtimes SET state = 'removed', machine_id = NULL, updated_at = $2 WHERE id = $1 AND successor_of IS NOT NULL")
            .bind(next).bind(now).execute(conn).await?;
        Ok(())
    }

    /// Environments currently placed on a host, including stopped units with their volumes.
    pub async fn on_host(conn: &mut Conn, host: Id) -> Result<Vec<Id>, DbError> {
        Ok(sqlx::query_scalar("SELECT DISTINCT environment_id FROM runtimes WHERE host_id = $1 AND kind = 'environment' AND state NOT IN ('retired','removed') AND successor_of IS NULL ORDER BY environment_id")
            .bind(host).fetch_all(conn).await?)
    }

    /// A stable notice identifier lets a restarted phase deduplicate it and the next turn consume it.
    pub fn notice_id(notice: &str) -> String {
        format!("runtime:{}", orch_types::Hash::of(notice.as_bytes()))
    }

    /// Retains a short notice in the shape the next turn reads and consumes, without altering conversations.
    pub async fn notice(
        conn: &mut Conn,
        thread: Id,
        notice: &str,
        now: DateTime<Utc>,
    ) -> Result<(), DbError> {
        crate::queries::threads::notices::append(
            conn,
            thread,
            &serde_json::json!({
                "id":notice_id(notice),"kind":"runtime","at":now,"from":"Orch",
                "from_thread":false,"title":"Runtime change","body":notice
            }),
        )
        .await?;
        Ok(())
    }
}

/// Frozen references and the separate lifetimes of removed units and their files.
pub mod retained;
/// Temporary environment deadlines, their creating thread, and the owner's warning window.
pub mod temporary;
