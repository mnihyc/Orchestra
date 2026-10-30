//! People, channels, routes and notifications on a real database, with a clock that stands still.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![allow(dead_code, reason = "each test file uses some of what is here")]

use chrono::{DateTime, Duration, Utc};
use orch_db::Db;
use orch_db::queries::hierarchy::testing::insert_node;
use orch_db::queries::identity::testing::insert_user;
use orch_db::queries::notifications::channels::{self, ChannelRow, NewChannel, owner};
use orch_db::queries::notifications::outbox::{self, OutboxRow, Payload, Snapshot};
use orch_db::queries::notifications::people::{self, Announced, NewNotification};
use orch_db::queries::notifications::routes::{self, RouteChange, RouteRow};
use orch_db::queries::settings;
use orch_db::secrets::MasterKey;
use orch_notifier_contract::delivery::DEFAULT_RETRY_SCHEDULE;
use orch_testkit::{clock, database::TestDb};
use orch_types::{Clock, Id, Json, ManualClock, new_id};
use orch_worker_core::channels::{Notifiers, Surroundings, seal_secret};
use orch_worker_core::heartbeat::Metrics;
use orch_worker_core::outbox::{Outbox, OutboxParts};
use serde_json::json;
use std::sync::Arc;

/// Whose a channel is.
#[derive(Debug, Clone, Copy)]
pub enum Of {
    /// The system's.
    System,
    /// A root org's.
    Org(Id),
    /// A person's.
    User(Id),
}

/// The world of a test.
pub struct World {
    /// The database.
    pub test: TestDb,
    /// The clock, which stands still.
    pub clock: ManualClock,
    /// The key that seals secrets.
    pub master: MasterKey,
    /// The person who sets things up.
    pub admin: Id,
    /// The notifiers there are.
    pub notifiers: Notifiers,
    /// What is counted.
    pub metrics: Arc<Metrics>,
}

impl World {
    /// A world with these notifiers and a person who sets things up.
    pub async fn new(notifiers: Notifiers) -> Self {
        let test = TestDb::fresh().await;
        let admin = new_id();
        {
            let mut conn = test.pool.acquire().await.unwrap();
            insert_user(&mut conn, admin, "admin").await.unwrap();
        }
        Self {
            test,
            clock: clock::manual(),
            master: MasterKey::new([7; 32]),
            admin,
            notifiers,
            metrics: Arc::new(Metrics::new()),
        }
    }

    pub fn db(&self) -> Db {
        Db::from_pool(self.test.pool.clone())
    }

    pub fn now(&self) -> DateTime<Utc> {
        self.clock.now()
    }

    pub fn surroundings(&self) -> Surroundings {
        Surroundings {
            system_name: "Acme Orch".to_owned(),
            public_url: "https://orch.example".to_owned(),
        }
    }

    /// The outbox of a worker.
    pub fn outbox(&self) -> Outbox {
        Outbox::new(OutboxParts {
            db: self.db(),
            clock: Arc::new(self.clock.clone()),
            notifiers: self.notifiers.clone(),
            master: self.master.clone(),
            surroundings: self.surroundings(),
            retry: DEFAULT_RETRY_SCHEDULE.to_vec(),
            metrics: Arc::clone(&self.metrics),
        })
    }

    /// A person with an address they confirmed, in English and UTC.
    pub async fn person(&self, username: &str) -> Id {
        self.person_with(
            username,
            "en",
            "UTC",
            Some(&format!("{username}@example.org")),
        )
        .await
    }

    /// A person with the language, time zone and (confirmed) address given.
    pub async fn person_with(
        &self,
        username: &str,
        locale: &str,
        timezone: &str,
        email: Option<&str>,
    ) -> Id {
        let id = new_id();
        let mut conn = self.test.pool.acquire().await.unwrap();
        insert_user(&mut conn, id, username).await.unwrap();
        sqlx::query(
            "UPDATE users SET locale = $2, timezone = $3, email = $4,
                    email_verified_at = CASE WHEN $4::text IS NULL THEN NULL ELSE $5 END,
                    display_name = $6
              WHERE id = $1",
        )
        .bind(id)
        .bind(locale)
        .bind(timezone)
        .bind(email)
        .bind(self.now())
        .bind(format!(
            "{}{}",
            username[..1].to_uppercase(),
            &username[1..]
        ))
        .execute(&mut *conn)
        .await
        .unwrap();
        id
    }

    /// Quiet hours for a person, in minutes after local midnight.
    pub async fn quiet_hours(&self, user: Id, start: i32, end: i32) {
        sqlx::query(
            "INSERT INTO notification_prefs (user_id, quiet_start_minute, quiet_end_minute) VALUES ($1, $2, $3)
             ON CONFLICT (user_id) DO UPDATE SET quiet_start_minute = $2, quiet_end_minute = $3",
        )
        .bind(user)
        .bind(start)
        .bind(end)
        .execute(&self.test.pool)
        .await
        .unwrap();
    }

    /// A root org with a name.
    pub async fn org(&self, name: &str) -> Id {
        let id = new_id();
        let mut conn = self.test.pool.acquire().await.unwrap();
        insert_node(&mut conn, id, "org", None, name, self.admin)
            .await
            .unwrap();
        id
    }

    /// A node of a kind inside another.
    pub async fn node(&self, kind: &str, parent: Id, name: &str) -> Id {
        let id = new_id();
        let mut conn = self.test.pool.acquire().await.unwrap();
        insert_node(&mut conn, id, kind, Some(parent), name, self.admin)
            .await
            .unwrap();
        id
    }

    /// A channel of a kind with these values and secrets.
    pub async fn channel(
        &self,
        of: Of,
        kind: &str,
        config: Json,
        secrets: &[(&str, &str)],
    ) -> ChannelRow {
        let mut conn = self.test.pool.acquire().await.unwrap();
        let (org_id, user_id) = match of {
            Of::System => (None, None),
            Of::Org(org) => (Some(org), None),
            Of::User(user) => (None, Some(user)),
        };
        let id = new_id();
        let row = channels::insert_channel(
            &mut conn,
            &NewChannel {
                id,
                org_id,
                user_id,
                kind,
                name: &format!("{kind} channel"),
                config: &config,
                templates: &json!({}),
                enabled: true,
                created_by: self.admin,
                at: self.now(),
            },
        )
        .await
        .unwrap();
        for (name, value) in secrets {
            let sealed = seal_secret(&self.master, owner::CHANNEL, id, name, value).unwrap();
            settings::put_secret(
                &mut conn,
                owner::CHANNEL,
                id,
                name,
                &sealed,
                None,
                self.now(),
            )
            .await
            .unwrap();
        }
        row
    }

    /// A route of a person through a channel.
    pub async fn route(&self, user: Id, channel: &ChannelRow, change: RouteChange<'_>) -> RouteRow {
        let mut conn = self.test.pool.acquire().await.unwrap();
        routes::put_route(&mut conn, new_id(), user, channel.id, &change, self.now())
            .await
            .unwrap()
    }

    /// A route that is on.
    pub async fn route_on(&self, user: Id, channel: &ChannelRow) -> RouteRow {
        self.route(
            user,
            channel,
            RouteChange {
                enabled: Some(true),
                ..RouteChange::default()
            },
        )
        .await
    }

    /// A notification in the Inbox of a person, `age` ago.
    pub async fn notification(
        &self,
        user: Id,
        kind: &str,
        title: &str,
        node: Option<Id>,
        age: Duration,
    ) -> Announced {
        let mut conn = self.test.pool.acquire().await.unwrap();
        people::announce(
            &mut conn,
            &NewNotification {
                user_id: user,
                kind,
                node_id: node,
                thread_id: None,
                approval_id: None,
                chat_id: None,
                title,
                body: "",
                data: &json!({}),
                dedupe_key: &format!("{kind}:{}", new_id()),
                at: self.now() - age,
            },
        )
        .await
        .unwrap()
        .unwrap()
    }

    /// A snapshot of a notification as the engine writes it into the outbox.
    pub fn snapshot(&self, kind: &str, title: &str) -> Snapshot {
        serde_json::from_value(json!({
            "id": new_id(), "kind": kind, "at": self.now(), "title": title, "body": "", "data": {}
        }))
        .unwrap()
    }

    /// Puts a delivery for a route into the outbox, due now.
    pub async fn queue_for_route(
        &self,
        route: &RouteRow,
        kind: &str,
        notifications: &[Snapshot],
    ) -> Id {
        let mut conn = self.test.pool.acquire().await.unwrap();
        let id = new_id();
        outbox::insert(
            &mut conn,
            &outbox::NewRow {
                id,
                channel: kind,
                route_id: Some(route.id),
                destination_id: None,
                channel_id: None,
                payload: &Payload::of(notifications).to_json(),
                at: self.now(),
            },
        )
        .await
        .unwrap();
        id
    }

    /// Every row of the outbox, the oldest first.
    pub async fn rows(&self) -> Vec<OutboxRow> {
        sqlx::query_as(
            "SELECT id, channel, route_id, destination_id, channel_id, payload, state, attempts, next_attempt_at, last_error, created_at
               FROM outbox ORDER BY created_at, id",
        )
        .fetch_all(&self.test.pool)
        .await
        .unwrap()
    }

    /// One row of the outbox.
    pub async fn row(&self, id: Id) -> OutboxRow {
        self.rows()
            .await
            .into_iter()
            .find(|row| row.id == id)
            .unwrap()
    }

    /// A channel as it is now.
    pub async fn channel_now(&self, id: Id) -> ChannelRow {
        let mut conn = self.test.pool.acquire().await.unwrap();
        channels::channel(&mut conn, id).await.unwrap().unwrap()
    }

    /// A route as it is now.
    pub async fn route_now(&self, id: Id) -> RouteRow {
        let mut conn = self.test.pool.acquire().await.unwrap();
        routes::route(&mut conn, id).await.unwrap().unwrap()
    }
}
