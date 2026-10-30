//! A small world on a real database for the tests of settings.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![allow(dead_code, reason = "each test file uses some of what is here")]

use chrono::{DateTime, Utc};
use orch_db::audit::{Actor, Origin};
use orch_db::queries::hierarchy::testing::{insert_node, put_thread_in_environment};
use orch_db::queries::identity::testing::insert_user;
use orch_db::queries::settings::testing::{
    set_environment_config, set_template_spec_of_environment,
};
use orch_db::secrets::MasterKey;
use orch_settings::write::{Clear, Write, WriteError, Written, clear, write};
use orch_settings::{Place, SettingView, Target, resolve};
use orch_testkit::{clock, database::TestDb};
use orch_types::{Id, Json, new_id};
use sqlx::Postgres;
use sqlx::pool::PoolConnection;

/// A tree of one root org with a department inside it, a project in the
/// department, a topic in the project and a thread in the topic; and a person,
/// `ada`, who made them all and writes everything.
pub struct World {
    pub db: TestDb,
    pub ada: Id,
    pub root: Id,
    pub department: Id,
    pub project: Id,
    pub topic: Id,
    pub thread: Id,
}

impl World {
    pub async fn new() -> Self {
        let db = TestDb::fresh().await;
        let mut conn = db.pool.acquire().await.unwrap();
        let ada = new_id();
        insert_user(&mut conn, ada, "ada").await.unwrap();
        let (root, department, project, topic, thread) =
            (new_id(), new_id(), new_id(), new_id(), new_id());
        insert_node(&mut conn, root, "org", None, "Acme", ada)
            .await
            .unwrap();
        insert_node(&mut conn, department, "org", Some(root), "Research", ada)
            .await
            .unwrap();
        insert_node(&mut conn, project, "project", Some(department), "Web", ada)
            .await
            .unwrap();
        insert_node(&mut conn, topic, "topic", Some(project), "Login", ada)
            .await
            .unwrap();
        insert_node(&mut conn, thread, "thread", Some(topic), "Fix", ada)
            .await
            .unwrap();
        drop(conn);
        Self {
            db,
            ada,
            root,
            department,
            project,
            topic,
            thread,
        }
    }

    pub async fn conn(&self) -> PoolConnection<Postgres> {
        self.db.pool.acquire().await.unwrap()
    }

    pub fn now() -> DateTime<Utc> {
        clock::start()
    }

    /// A node of `kind` inside `parent`, made by `ada`.
    pub async fn node(&self, kind: &str, parent: Id, name: &str) -> Id {
        let id = new_id();
        let mut conn = self.conn().await;
        insert_node(&mut conn, id, kind, Some(parent), name, self.ada)
            .await
            .unwrap();
        id
    }

    /// Writes as `ada`, who may do everything: no lock, no revision expected.
    pub async fn set(&self, target: Target, key: &str, value: Json) -> Written {
        self.try_set(target, key, value, None, None, true)
            .await
            .unwrap()
    }

    /// Writes and locks the row, as `ada`.
    pub async fn lock(&self, target: Target, key: &str, value: Json) -> Written {
        self.try_set(target, key, value, Some(true), None, true)
            .await
            .unwrap()
    }

    pub async fn try_set(
        &self,
        target: Target,
        key: &str,
        value: Json,
        locked: Option<bool>,
        expected_revision: Option<i64>,
        may_lock: bool,
    ) -> Result<Written, WriteError> {
        let origin = Origin::default();
        let request = Write {
            actor: Actor::User(self.ada),
            target,
            key,
            value,
            locked,
            expected_revision,
            may_lock,
            origin: &origin,
        };
        let mut conn = self.conn().await;
        write(&mut conn, &request, Self::now()).await
    }

    pub async fn try_clear(
        &self,
        target: Target,
        key: &str,
        expected_revision: Option<i64>,
        may_lock: bool,
    ) -> Result<Written, WriteError> {
        let origin = Origin::default();
        let request = Clear {
            actor: Actor::User(self.ada),
            target,
            key,
            expected_revision,
            may_lock,
            origin: &origin,
        };
        let mut conn = self.conn().await;
        clear(&mut conn, &request, Self::now()).await
    }

    pub async fn view(&self, place: Place, key: &str) -> SettingView {
        let mut conn = self.conn().await;
        resolve(&mut conn, place, key).await.unwrap()
    }

    /// The value that holds, if any.
    pub async fn value(&self, place: Place, key: &str) -> Option<Json> {
        self.view(place, key).await.resolved.value
    }

    /// Puts `thread` in a managed environment whose home is `home`, made from
    /// a template with this `spec`, with these own values.
    pub async fn managed_environment(
        &self,
        thread: Id,
        home: Id,
        spec: &Json,
        config: &Json,
    ) -> Id {
        let mut conn = self.conn().await;
        let environment = put_thread_in_environment(&mut conn, thread, "managed", self.ada, home)
            .await
            .unwrap();
        set_template_spec_of_environment(&mut conn, environment, spec)
            .await
            .unwrap();
        set_environment_config(&mut conn, environment, config)
            .await
            .unwrap();
        environment
    }

    /// The audit rows so far: action, whether it is a line of history, the
    /// summary and the details.
    pub async fn audit(&self) -> Vec<(String, bool, String, Json)> {
        sqlx::query_as("SELECT action, history, summary, details FROM audit_log ORDER BY at, id")
            .fetch_all(&self.db.pool)
            .await
            .unwrap()
    }
}

/// A master key for the tests.
pub fn master() -> MasterKey {
    MasterKey::new([9; 32])
}

/// Counts the statements that run on this thread: sqlx tells of each one as an
/// event. One subscriber is installed for the whole test binary, and it counts
/// per thread, so that tests that run side by side count their own.
pub struct Statements;

thread_local! {
    static COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

impl Statements {
    /// Makes sure the subscriber is installed, and returns the count so far on
    /// this thread.
    pub fn now() -> usize {
        static INSTALL: std::sync::Once = std::sync::Once::new();
        INSTALL.call_once(|| {
            let _ = tracing::subscriber::set_global_default(Statements);
        });
        COUNT.with(std::cell::Cell::get)
    }
}

impl tracing::Subscriber for Statements {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        metadata.target() == "sqlx::query"
    }

    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        if event.metadata().target() == "sqlx::query" {
            COUNT.with(|count| count.set(count.get() + 1));
        }
    }

    fn enter(&self, _: &tracing::span::Id) {}

    fn exit(&self, _: &tracing::span::Id) {}
}
