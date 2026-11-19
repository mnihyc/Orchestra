//! Database connections and transaction entry points.
//!
//! [`Db`] is the pool. Queries take `&mut Conn`: pass `&mut *tx` inside a
//! transaction, or a connection taken with [`Db::acquire`]. Errors are
//! sorted into the few kinds a caller acts on: a unique value that is
//! taken, a missing row that is referred to, a broken rule of the schema,
//! a transaction that lost a race and may be run again, and the rest.

use sqlx::ConnectOptions as _;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::cell::Cell;
use std::future::Future;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use std::{str::FromStr, time::Duration};

/// One connection, alone or inside a transaction.
pub type Conn = sqlx::PgConnection;

/// How long a caller waits for a free connection before the request
/// fails: long enough to ride out a burst, short enough that a stuck
/// database shows as errors and not as hanging requests.
const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(10);
/// The default warning boundary for a statement or a successful pool acquisition: one second.
const SLOW_DEFAULT: Duration = Duration::from_secs(1);

/// What the database refuses to let a connection of a pool do, so that no session can
/// hold rows, a pooled connection or the vacuum horizon for long.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// How long a session may sit inside a transaction doing nothing before PostgreSQL ends it.
    pub idle_in_transaction: Duration,
    /// How long one statement may run before PostgreSQL cancels it; none for no bound.
    pub statement: Option<Duration>,
}

impl Limits {
    /// API limits. A transaction idle for 10 s is ended to free its rows and connection.
    /// Requests should spend less than 1 s idle; the tenfold margin accommodates debug
    /// builds and loaded machines. Statements are cancelled after 15 s, fifteen times the
    /// one-second statement budget and longer than the 10 s pool acquisition timeout,
    /// so a statement blocked on a lock fails instead of retaining a connection indefinitely.
    pub const REQUESTS: Self = Self {
        idle_in_transaction: Duration::from_secs(10),
        statement: Some(Duration::from_secs(15)),
    };
    /// A background service's (the worker): the same 10 s for a transaction left idle, and no statement bound,
    /// for the rare long statement of a purge, a rollup or a migration of data that it runs.
    pub const BACKGROUND: Self = Self {
        idle_in_transaction: Duration::from_secs(10),
        statement: None,
    };

    /// The settings a connection of the pool starts with: these limits, and no JIT compilation.
    /// PostgreSQL compiles a statement with LLVM once the planner's estimate passes
    /// `jit_above_cost` (100 000), and inlines and optimises past 500 000. Orch's indexed reads
    /// finish in milliseconds. On small or newly populated tables, stale statistics can raise
    /// cost estimates enough to trigger compilation whose overhead exceeds execution time.
    /// These queries process too few rows to benefit from compiled expressions, so disabling
    /// JIT avoids that overhead.
    fn settings(self) -> Vec<(&'static str, String)> {
        let millis = |duration: Duration| duration.as_millis().to_string();
        let mut settings = vec![(
            "idle_in_transaction_session_timeout",
            millis(self.idle_in_transaction),
        )];
        if let Some(statement) = self.statement {
            settings.push(("statement_timeout", millis(statement)));
        }
        settings.push(("jit", "off".to_owned()));
        settings
    }

    fn apply(self, options: PgConnectOptions) -> PgConnectOptions {
        options.options(self.settings())
    }
}

/// The pool's figures, exported by every service that holds one: its size, its connections in use and
/// idle, the wait for a connection, and what the development guard reported.
pub mod metrics_names {
    /// The pool's largest size, as configured (beside the heartbeat's `orch_db_pool_in_use`).
    pub const MAX: &str = "orch_db_pool_max_connections";
    /// Seconds a caller waited for a connection, from asking to having it.
    pub const ACQUIRE_SECONDS: &str = "orch_db_pool_acquire_seconds";
    /// Callers that waited longer than the 100 ms pool acquisition budget.
    pub const ACQUIRE_OVER_BAR: &str = "orch_db_pool_acquire_over_100ms_total";
    /// Transactions open longer than the development guard's 200 ms (counted in every build).
    pub const HELD_OVER_GUARD: &str = "orch_db_transactions_held_over_200ms_total";
}

/// The warning threshold for acquiring a pooled connection.
const ACQUIRE_BAR: Duration = Duration::from_millis(100);
/// The development guard's bound of a transaction: what a request's transaction may take on a debug build
/// before it is reported with its route; a request normally needs only a few statements.
pub const HELD_REPORT: Duration = Duration::from_millis(200);

static HELD_REPORTS: AtomicU64 = AtomicU64::new(0);
static NESTED_REPORTS: AtomicU64 = AtomicU64::new(0);
/// How many reports of a nested acquisition carry a backtrace in one process.
const NESTED_BACKTRACES: u64 = 20;

tokio::task_local! {
    /// The request a task serves, and how many transactions of [`Db`] it holds open now.
    static SERVING: Serving;
}

struct Serving {
    route: &'static str,
    open: Cell<u32>,
    refused: Cell<bool>,
}

/// Runs a request's work so that the development guard can name it: a transaction of this task open
/// longer than [`HELD_REPORT`], and a pooled connection asked for while this task holds a transaction,
/// are reported with `route` (logged in a debug build or a test; counted by [`guard_reports`]).
pub async fn serving<T>(route: &'static str, work: impl Future<Output = T>) -> T {
    serving_watched(route, work).await.0
}

/// [`serving`], also saying whether a commit of the work was refused because who may do what
/// changed while it ran: a request that may be served once more.
pub async fn serving_watched<T>(route: &'static str, work: impl Future<Output = T>) -> (T, bool) {
    SERVING
        .scope(
            Serving {
                route,
                open: Cell::new(0),
                refused: Cell::new(false),
            },
            async move {
                let out = work.await;
                (
                    out,
                    SERVING
                        .try_with(|serving| serving.refused.get())
                        .unwrap_or(false),
                )
            },
        )
        .await
}

/// Notes that a commit of the work being served was refused because an authorization epoch it decided
/// under moved (`orch_commit_work`'s `authz_epoch_moved`).
fn note_refusal() {
    let _ = SERVING.try_with(|serving| serving.refused.set(true));
}

/// How many times the development guard reported a transaction held too long, and a pooled acquisition
/// made while the same task held a transaction, since the process started.
#[must_use]
pub fn guard_reports() -> (u64, u64) {
    (
        HELD_REPORTS.load(Ordering::Relaxed),
        NESTED_REPORTS.load(Ordering::Relaxed),
    )
}

fn route_now() -> Option<&'static str> {
    SERVING.try_with(|serving| serving.route).ok()
}

/// Reports a pooled acquisition made while this task holds a transaction of the pool: the second
/// connection waits on the same pool the first one holds, which a burst turns into a pool-wide stall
/// that the database cannot see.
fn check_nested(what: &'static str) {
    let open = SERVING.try_with(|serving| serving.open.get()).unwrap_or(0);
    if open == 0 {
        return;
    }
    let earlier = NESTED_REPORTS.fetch_add(1, Ordering::Relaxed);
    if cfg!(debug_assertions) {
        // Where it came from, for the first reports of a process: a backtrace costs a debug build tens of
        // milliseconds, which a frequent fault must not add to every request it reports.
        let backtrace = if earlier < NESTED_BACKTRACES {
            std::backtrace::Backtrace::force_capture().to_string()
        } else {
            Default::default()
        };
        tracing::error!(
            route = route_now().unwrap_or("-"),
            what,
            open,
            backtrace,
            "a pooled connection was asked for while this task holds a transaction"
        );
    }
}

/// What a transaction of [`Db`] keeps beside its connection: when it began, and for which request.
struct Held {
    at: Instant,
    route: Option<&'static str>,
    counted: bool,
}

impl Held {
    fn begin() -> Self {
        let route = route_now();
        let counted = SERVING
            .try_with(|serving| serving.open.set(serving.open.get().saturating_add(1)))
            .is_ok();
        Self {
            at: Instant::now(),
            route,
            counted,
        }
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        if self.counted {
            let _ =
                SERVING.try_with(|serving| serving.open.set(serving.open.get().saturating_sub(1)));
        }
        let held = self.at.elapsed();
        if held > HELD_REPORT {
            HELD_REPORTS.fetch_add(1, Ordering::Relaxed);
            metrics::counter!(metrics_names::HELD_OVER_GUARD).increment(1);
            if cfg!(debug_assertions) {
                tracing::warn!(
                    route = self.route.unwrap_or("-"),
                    held_ms = u64::try_from(held.as_millis()).unwrap_or(u64::MAX),
                    "a transaction was open longer than 200 ms"
                );
            }
        }
    }
}

/// A transaction. Dropped without `commit`, it is rolled back. It is the connection it runs on
/// (`&mut *tx` is a [`Conn`]), and the development guard knows how long it is open.
pub struct Tx {
    inner: sqlx::Transaction<'static, sqlx::Postgres>,
    held: Held,
}

impl std::fmt::Debug for Tx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tx")
            .field("route", &self.held.route)
            .finish_non_exhaustive()
    }
}

impl Deref for Tx {
    type Target = Conn;
    fn deref(&self) -> &Conn {
        &self.inner
    }
}

impl DerefMut for Tx {
    fn deref_mut(&mut self) -> &mut Conn {
        &mut self.inner
    }
}

impl AsMut<Conn> for Tx {
    fn as_mut(&mut self) -> &mut Conn {
        &mut self.inner
    }
}

impl Tx {
    /// Commits. The commit runs what the transaction left for it (`orch_commit_work`): a change
    /// of authority it decided under refuses it as [`DbError::Retry`].
    pub async fn commit(self) -> Result<(), sqlx::Error> {
        let Self { inner, held } = self;
        let done = inner.commit().await;
        drop(held);
        done
    }

    /// Rolls back.
    pub async fn rollback(self) -> Result<(), sqlx::Error> {
        let Self { inner, held } = self;
        let done = inner.rollback().await;
        drop(held);
        done
    }
}

/// What went wrong in the database, by what a caller can do about it.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    /// A value that must be unique is taken.
    #[error("a unique value is taken ({constraint})")]
    Unique {
        /// The constraint or index that refused.
        constraint: String,
    },
    /// A row that this one refers to is not there, or one that is referred
    /// to cannot go.
    #[error("a row this depends on is missing or still in use ({constraint})")]
    Reference {
        /// The constraint that refused.
        constraint: String,
    },
    /// A value breaks a rule the schema states.
    #[error("a value breaks a rule of the schema ({constraint})")]
    Rule {
        /// The constraint that refused.
        constraint: String,
    },
    /// A value written to a column breaks a check the schema states on it.
    #[error("a value breaks a check of the schema ({constraint})")]
    Check {
        /// The table the value was written to.
        table: String,
        /// The check that refused: `<table>_<column>_check` where the schema did not name it.
        constraint: String,
    },
    /// The transaction lost a race with another and can be run again.
    #[error("the transaction lost a race and can be run again")]
    Retry,
    /// Anything else.
    #[error(transparent)]
    Other(sqlx::Error),
}

impl From<sqlx::Error> for DbError {
    fn from(error: sqlx::Error) -> Self {
        let Some(database) = error.as_database_error() else {
            return Self::Other(error);
        };
        let constraint = || database.constraint().unwrap_or_default().to_owned();
        // SQLSTATE classes of PostgreSQL: 23 is an integrity violation, 40 a rollback.
        match database.code().as_deref() {
            Some("23505") => Self::Unique {
                constraint: constraint(),
            },
            Some("23503") => Self::Reference {
                constraint: constraint(),
            },
            Some("23514") => Self::Check {
                table: database
                    .try_downcast_ref::<sqlx::postgres::PgDatabaseError>()
                    .and_then(sqlx::postgres::PgDatabaseError::table)
                    .unwrap_or_default()
                    .to_owned(),
                constraint: constraint(),
            },
            Some("23502" | "23P01") => Self::Rule {
                constraint: constraint(),
            },
            Some("40001" | "40P01") => {
                if database.constraint() == Some("authz_epoch_moved") {
                    note_refusal();
                }
                Self::Retry
            }
            _ => Self::Other(error),
        }
    }
}

impl DbError {
    /// Whether a unique value was taken, on the constraint or index named
    /// `constraint`.
    #[must_use]
    pub fn is_unique(&self, constraint: &str) -> bool {
        matches!(self, Self::Unique { constraint: found } if found == constraint)
    }
}

/// The database.
#[derive(Debug, Clone)]
pub struct Db {
    pool: sqlx::PgPool,
}

impl Db {
    /// Connects to the database at `url` with at most `connections` at once.
    pub async fn connect(url: &str, connections: u32) -> Result<Self, DbError> {
        Self::connect_with_slow_thresholds(url, connections, SLOW_DEFAULT, SLOW_DEFAULT).await
    }

    /// Connects with the operator's warning thresholds; SQLx logs statement text, never bound values.
    /// A background service's [`Limits`].
    pub async fn connect_with_slow_thresholds(
        url: &str,
        connections: u32,
        slow_statement: Duration,
        slow_acquire: Duration,
    ) -> Result<Self, DbError> {
        Self::connect_with_limits(
            url,
            connections,
            slow_statement,
            slow_acquire,
            Limits::BACKGROUND,
        )
        .await
    }

    /// Connects with the operator's warning thresholds and what the database refuses to let a
    /// connection do (`limits`).
    pub async fn connect_with_limits(
        url: &str,
        connections: u32,
        slow_statement: Duration,
        slow_acquire: Duration,
        limits: Limits,
    ) -> Result<Self, DbError> {
        let options = limits.apply(
            PgConnectOptions::from_str(url)?
                .application_name("orch")
                .log_slow_statements(log::LevelFilter::Warn, slow_statement),
        );
        let pool = PgPoolOptions::new()
            .max_connections(connections)
            .acquire_timeout(ACQUIRE_TIMEOUT)
            .acquire_slow_level(log::LevelFilter::Warn)
            .acquire_slow_threshold(slow_acquire)
            .connect_with(options)
            .await?;
        metrics::describe_gauge!(metrics_names::MAX, "The largest size of the database pool");
        metrics::describe_histogram!(
            metrics_names::ACQUIRE_SECONDS,
            "Seconds waited for a pooled connection"
        );
        metrics::describe_counter!(
            metrics_names::ACQUIRE_OVER_BAR,
            "Waits for a pooled connection over 100 ms"
        );
        metrics::describe_counter!(
            metrics_names::HELD_OVER_GUARD,
            "Transactions open longer than 200 ms"
        );
        metrics::gauge!(metrics_names::MAX).set(f64::from(connections));
        metrics::counter!(metrics_names::ACQUIRE_OVER_BAR).increment(0);
        metrics::counter!(metrics_names::HELD_OVER_GUARD).increment(0);
        Ok(Self { pool })
    }

    /// Notes how long a caller waited for a connection.
    fn measure(&self, waited: Duration) {
        metrics::histogram!(metrics_names::ACQUIRE_SECONDS).record(waited.as_secs_f64());
        if waited > ACQUIRE_BAR {
            metrics::counter!(metrics_names::ACQUIRE_OVER_BAR).increment(1);
        }
    }

    /// The database behind an existing pool.
    #[must_use]
    pub fn from_pool(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }

    /// The pool, for the few things that need it as such (a listener).
    #[must_use]
    pub fn pool(&self) -> &sqlx::PgPool {
        &self.pool
    }

    /// A connection for statements that need no transaction.
    pub async fn acquire(&self) -> Result<sqlx::pool::PoolConnection<sqlx::Postgres>, DbError> {
        check_nested("acquire");
        let asked = Instant::now();
        let conn = self.pool.acquire().await?;
        self.measure(asked.elapsed());
        Ok(conn)
    }

    /// Begins a transaction.
    pub async fn begin(&self) -> Result<Tx, DbError> {
        self.begin_with_snapshot(false).await
    }

    /// Begins a repeatable snapshot before the authorization guard records its epochs in this transaction.
    pub async fn begin_snapshot(&self) -> Result<Tx, DbError> {
        self.begin_with_snapshot(true).await
    }

    async fn begin_with_snapshot(&self, snapshot: bool) -> Result<Tx, DbError> {
        check_nested("begin");
        let asked = Instant::now();
        let inner = self.pool.begin().await?;
        self.measure(asked.elapsed());
        let mut tx = Tx {
            inner,
            held: Held::begin(),
        };
        if snapshot {
            // Epoch recording arms commit bookkeeping, which needs a write even for a content read.
            sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
                .execute(&mut *tx)
                .await?;
        }
        crate::queries::transaction_guard::begin_scope(&mut tx).await?;
        Ok(tx)
    }

    /// Begins a transaction whose first statement is `isolation`, a `SET TRANSACTION` statement:
    /// PostgreSQL takes one only before any query, so it comes before what the request's lock class
    /// runs when a transaction begins (a write's request reads its answer after its commit).
    pub(crate) async fn begin_isolated(&self, isolation: &'static str) -> Result<Tx, DbError> {
        check_nested("begin");
        let asked = Instant::now();
        let mut inner = self.pool.begin().await?;
        self.measure(asked.elapsed());
        sqlx::query(isolation).execute(&mut *inner).await?;
        let mut tx = Tx {
            inner,
            held: Held::begin(),
        };
        crate::queries::transaction_guard::begin_scope(&mut tx).await?;
        Ok(tx)
    }

    /// Whether the database answers.
    pub async fn ping(&self) -> bool {
        sqlx::query_scalar::<_, i32>("SELECT 1")
            .fetch_one(&self.pool)
            .await
            .is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_testkit::database::TestDb;

    #[tokio::test]
    async fn a_taken_unique_value_is_told_apart_from_other_failures() {
        let test = TestDb::fresh().await;
        let db = Db::from_pool(test.pool.clone());
        let mut tx = db.begin().await.unwrap();
        let insert = "INSERT INTO users (id, username, display_name, avatar_color) VALUES ($1, 'ada', 'Ada', 1)";
        sqlx::query(insert)
            .bind(orch_types::new_id())
            .execute(&mut *tx)
            .await
            .unwrap();
        let again = sqlx::query(insert)
            .bind(orch_types::new_id())
            .execute(&mut *tx)
            .await
            .unwrap_err();
        assert!(DbError::from(again).is_unique("users_username_key"));
    }

    #[tokio::test]
    async fn a_value_that_breaks_a_columns_check_names_its_table_and_check() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        sqlx::query("CREATE TEMP TABLE waits (minutes int CHECK (minutes BETWEEN 1 AND 1440))")
            .execute(&mut *conn)
            .await
            .unwrap();
        let refused = sqlx::query("INSERT INTO waits (minutes) VALUES (0)")
            .execute(&mut *conn)
            .await
            .unwrap_err();
        match DbError::from(refused) {
            DbError::Check { table, constraint } => {
                assert_eq!(
                    (table.as_str(), constraint.as_str()),
                    ("waits", "waits_minutes_check")
                );
            }
            other => panic!("a check violation is told as one: {other:?}"),
        }
    }

    #[test]
    fn every_pool_starts_its_connections_without_jit_and_within_its_limits() {
        for (limits, statement) in [
            (Limits::REQUESTS, Some("15000")),
            (Limits::BACKGROUND, None),
        ] {
            let settings = limits.settings();
            let value = |name: &str| {
                settings
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| value.as_str())
            };
            assert_eq!(value("jit"), Some("off"));
            assert_eq!(value("idle_in_transaction_session_timeout"), Some("10000"));
            assert_eq!(value("statement_timeout"), statement);
            let options = limits.apply(PgConnectOptions::new());
            assert!(
                options
                    .get_options()
                    .is_some_and(|text| text.contains("-c jit=off"))
            );
        }
    }

    #[tokio::test]
    async fn a_connection_of_a_pool_runs_without_jit() {
        let test = TestDb::fresh().await;
        let db =
            Db::connect_with_limits(&test.url, 1, SLOW_DEFAULT, SLOW_DEFAULT, Limits::REQUESTS)
                .await
                .unwrap();
        let jit: String = sqlx::query_scalar("SELECT current_setting('jit')")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(jit, "off");
    }

    #[tokio::test]
    async fn a_transaction_that_is_dropped_leaves_nothing() {
        let test = TestDb::fresh().await;
        let db = Db::from_pool(test.pool.clone());
        {
            let mut tx = db.begin().await.unwrap();
            sqlx::query("INSERT INTO users (id, username, display_name, avatar_color) VALUES ($1, 'ada', 'Ada', 1)")
                .bind(orch_types::new_id())
                .execute(&mut *tx)
                .await
                .unwrap();
        }
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(count, 0);
        assert!(db.ping().await);
    }
}
