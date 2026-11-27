//! Database connections and transaction entry points.
//!
//! [`Db`] is the pool. Queries take `&mut Conn`: pass `&mut *tx` inside a
//! transaction, or a connection taken with [`Db::acquire`]. Errors are
//! sorted into the few kinds a caller acts on: a unique value that is
//! taken, a missing row that is referred to, a broken rule of the schema,
//! a transaction that lost a race and may be run again, and the rest.

use orch_api_contract::common::DatabaseWaitPhase;
use sqlx::pool::PoolConnection;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions, PgTransactionManager};
use sqlx::{ConnectOptions as _, Postgres, SqlSafeStr as _};
use sqlx_core::transaction::TransactionManager as _;
use std::cell::Cell;
use std::future::Future;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use std::{str::FromStr, time::Duration};
use tracing::instrument::WithSubscriber as _;

/// One connection, alone or inside a transaction.
pub type Conn = sqlx::PgConnection;

/// The default warning boundary for a statement or a successful pool acquisition: one second.
const SLOW_DEFAULT: Duration = Duration::from_secs(1);

/// What the database refuses to let a connection of a pool do, so that no session can
/// hold rows, a pooled connection or the vacuum horizon for long, and how many statements
/// each connection keeps prepared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// How long acquiring a pool connection, including opening it, may take.
    pub acquire_timeout: Duration,
    /// How long a session may sit inside a transaction doing nothing before PostgreSQL ends it.
    pub idle_in_transaction: Duration,
    /// How long one statement may run before PostgreSQL cancels it; none for no bound.
    pub statement: Option<Duration>,
    /// How many statements a connection keeps prepared, the least recently used given up first. A
    /// statement that is not kept costs a round trip to prepare it again, and PostgreSQL parses
    /// and plans it again, each time it runs.
    pub prepared_statements: usize,
}

impl Limits {
    /// API limits. A transaction idle for 10 s is ended to free its rows and connection.
    /// Requests should spend less than 1 s idle; the tenfold margin accommodates debug
    /// builds and loaded machines. Statements are cancelled after 15 s, fifteen times the
    /// one-second statement budget and longer than the 10 s pool acquisition timeout,
    /// so a statement blocked on a lock fails instead of retaining a connection indefinitely.
    pub const REQUESTS: Self = Self::milliseconds(
        orch_config::server::DATABASE_ACQUIRE_TIMEOUT_MS,
        orch_config::server::DATABASE_REQUEST_STATEMENT_TIMEOUT_MS,
        orch_config::server::DATABASE_IDLE_TRANSACTION_TIMEOUT_MS,
        orch_config::server::DATABASE_PREPARED_STATEMENTS,
    );
    /// A background service's (the worker): the same 10 s for a transaction left idle, and no statement bound,
    /// for the rare long statement of a purge, a rollup or a migration of data that it runs.
    pub const BACKGROUND: Self = Self::milliseconds(
        orch_config::server::DATABASE_ACQUIRE_TIMEOUT_MS,
        orch_config::server::DATABASE_BACKGROUND_STATEMENT_TIMEOUT_MS,
        orch_config::server::DATABASE_IDLE_TRANSACTION_TIMEOUT_MS,
        orch_config::server::DATABASE_PREPARED_STATEMENTS,
    );

    /// Request limits from the configuration already loaded before opening the database.
    #[must_use]
    pub fn requests(config: &orch_config::server::Database) -> Self {
        Self::configured(config, config.request_statement_timeout_ms)
    }

    /// Worker and maintenance limits from that process's startup configuration.
    #[must_use]
    pub fn background(config: &orch_config::server::Database) -> Self {
        Self::configured(config, config.background_statement_timeout_ms)
    }

    fn configured(config: &orch_config::server::Database, statement_ms: u64) -> Self {
        Self::milliseconds(
            config.acquire_timeout_ms,
            statement_ms,
            config.idle_transaction_timeout_ms,
            config.prepared_statements,
        )
    }

    const fn milliseconds(
        acquire_ms: u64,
        statement_ms: u64,
        idle_ms: u64,
        prepared_statements: usize,
    ) -> Self {
        Self {
            acquire_timeout: Duration::from_millis(acquire_ms),
            idle_in_transaction: Duration::from_millis(idle_ms),
            statement: if statement_ms == 0 {
                None
            } else {
                Some(Duration::from_millis(statement_ms))
            },
            prepared_statements,
        }
    }

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
        settings.push((
            "statement_timeout",
            millis(self.statement.unwrap_or_default()),
        ));
        settings.push(("jit", "off".to_owned()));
        settings
    }

    fn apply(self, options: PgConnectOptions) -> PgConnectOptions {
        options
            .options(self.settings())
            .statement_cache_capacity(self.prepared_statements)
    }
}

/// How many of a pool's connections are kept open and ready while its service is idle, and how long a connection
/// serves. PostgreSQL answers a connection it has served for a while faster than a new one: the connection keeps the
/// statements its pool prepared, and the catalogue, plans and functions they use. On the team fixture a page's
/// first opening on new connections took 231 ms where the same opening took 148-168 ms on used ones, and a pool
/// whose idle connections closed after ten minutes met every opening after a pause with new ones (D-3146).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Warm {
    /// Connections kept open while the service is idle. A connection beyond them that sits idle is closed, one
    /// each [`Warm::pass`]; zero keeps none, and an idle connection closes after ten minutes.
    pub connections: u32,
    /// How long a connection serves before it is replaced; none for no bound.
    pub lifetime: Option<Duration>,
    /// How many of the statements its siblings ran most a new connection prepares (and has PostgreSQL plan once)
    /// before it serves.
    pub statements: usize,
    /// How often the pool's keeper looks at its idle connections.
    pub pass: Duration,
}

impl Warm {
    /// No connection kept open: the pool of a tool or of a test.
    pub const NONE: Self = Self {
        connections: 0,
        lifetime: Some(Duration::from_secs(30 * 60)),
        statements: 0,
        pass: Duration::from_secs(60),
    };

    /// The API's: `connections` kept open, each serving for `lifetime`, preparing half the statements a connection
    /// keeps prepared (the other half is room for the rest of what it runs).
    #[must_use]
    pub const fn requests(connections: u32, lifetime: Option<Duration>, limits: Limits) -> Self {
        Self {
            connections,
            lifetime,
            statements: limits.prepared_statements / 2,
            pass: Duration::from_secs(60),
        }
    }
}

/// A statement the pool's connections ran, as a new connection prepares it.
struct Hot {
    sql: String,
    types: Vec<u32>,
    runs: i64,
}

/// What a pool's keeper shares with the pool: when the pool opens a connection, and the statements it learned.
struct Keeper {
    warm: Warm,
    opened: AtomicU64,
    wake: tokio::sync::Notify,
    hot: std::sync::Mutex<Vec<Hot>>,
}

/// A connection that has prepared this many statements has served: what it ran is worth preparing elsewhere.
const LEARN_FROM: usize = 32;

impl Keeper {
    /// Learns what a served idle connection ran most, from PostgreSQL's own list of its prepared statements. Says
    /// whether the list changed.
    async fn learn(&self, conn: &mut Conn) -> bool {
        let rows: Result<Vec<(String, Vec<i64>, i64)>, sqlx::Error> = sqlx::query_as(
            "SELECT statement, parameter_types::oid[]::int8[], generic_plans + custom_plans FROM pg_prepared_statements
              WHERE NOT from_sql ORDER BY generic_plans + custom_plans DESC LIMIT $1",
        )
        .bind(i64::try_from(self.warm.statements).unwrap_or(i64::MAX))
        .persistent(false)
        .fetch_all(&mut *conn)
        .await;
        let Ok(rows) = rows else {
            return false;
        };
        let Ok(mut hot) = self.hot.lock() else {
            return false;
        };
        let before: Vec<String> = hot.iter().map(|each| each.sql.clone()).collect();
        // What this connection ran counts with what the others ran; older counts weigh half, so that what is no
        // longer run gives way.
        for each in hot.iter_mut() {
            each.runs /= 2;
        }
        for (sql, types, runs) in rows {
            match hot.iter_mut().find(|each| each.sql == sql) {
                Some(each) => each.runs = each.runs.saturating_add(runs),
                None => hot.push(Hot {
                    sql,
                    types: types
                        .into_iter()
                        .filter_map(|oid| u32::try_from(oid).ok())
                        .collect(),
                    runs,
                }),
            }
        }
        hot.sort_by(|one, other| other.runs.cmp(&one.runs));
        hot.truncate(self.warm.statements);
        hot.iter().map(|each| &each.sql).ne(before.iter())
    }

    /// Prepares the learned statements on a connection that has not served, and has PostgreSQL plan each once
    /// (`EXPLAIN (GENERIC_PLAN)`, nothing run), so that its first requests find them and what they use ready. A
    /// statement that does not prepare here is left for its first use.
    async fn prepare(&self, conn: &mut Conn) {
        use sqlx::Executor as _;
        let hot: Vec<(String, Vec<u32>)> = match self.hot.lock() {
            Ok(hot) => hot
                .iter()
                .map(|each| (each.sql.clone(), each.types.clone()))
                .collect(),
            Err(_) => return,
        };
        for (sql, types) in hot {
            let types: Vec<_> = types
                .into_iter()
                .map(|oid| sqlx::postgres::PgTypeInfo::with_oid(sqlx::postgres::types::Oid(oid)))
                .collect();
            if (&mut *conn)
                .prepare_with(sqlx::AssertSqlSafe(sql.clone()).into_sql_str(), &types)
                .await
                .is_err()
            {
                continue;
            }
            let _ = sqlx::raw_sql(sqlx::AssertSqlSafe(format!("EXPLAIN (GENERIC_PLAN) {sql}")))
                .execute(&mut *conn)
                .await;
        }
    }
}

/// Keeps a pool's connections warm: prepares what its served connections ran on each connection it opens (in the
/// background, never on a request's way), learns that list from them, and closes, one each pass, an idle
/// connection beyond the warm ones. It takes one idle connection at a time and gives it back at once, so that a
/// request is never without one. Ends when the pool is closed.
async fn keep_warm(pool: sqlx::PgPool, keeper: std::sync::Arc<Keeper>) {
    let mut warmed = 0_u64;
    let mut again = false;
    loop {
        tokio::select! {
            () = keeper.wake.notified() => {}
            () = tokio::time::sleep(keeper.warm.pass) => {}
        }
        if pool.is_closed() {
            return;
        }
        let opened = keeper.opened.load(Ordering::Relaxed);
        let look = opened != warmed || again;
        warmed = opened;
        again = false;
        let mut learned = false;
        let idle = pool.num_idle();
        // The one idle longest is taken first.
        let mut beyond = u32::try_from(idle).unwrap_or(u32::MAX) > keeper.warm.connections;
        for _ in 0..idle {
            let Some(mut conn) = pool.try_acquire() else {
                break;
            };
            if beyond {
                beyond = false;
                // A connection that does not close cleanly is gone all the same.
                let _ = conn.close().await;
                continue;
            }
            let served = sqlx::Connection::cached_statements_size(&*conn);
            if !learned && served >= LEARN_FROM {
                learned = true;
                // The connections looked at before it this pass are looked at again on the next.
                again = keeper.learn(&mut conn).await;
            }
            let known = keeper.hot.lock().map_or(0, |hot| hot.len());
            if (look || again) && known > 0 && served < known / 2 {
                keeper.prepare(&mut conn).await;
            }
            conn.return_to_pool().await;
        }
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

const BEGIN_OPENING: &str = "BEGIN ISOLATION LEVEL REPEATABLE READ, READ ONLY";

static HELD_REPORTS: AtomicU64 = AtomicU64::new(0);
static NESTED_REPORTS: AtomicU64 = AtomicU64::new(0);
/// How many reports of a nested acquisition carry a backtrace in one process.
const NESTED_BACKTRACES: u64 = 20;

tokio::task_local! {
    /// The request a task serves, and how many transactions of [`Db`] it holds open now.
    static SERVING: Serving;
    static READ_FACTS_UNCHANGED: Cell<bool>;
}

/// One effect-free snapshot's fact-reuse interval; a trial write ends reuse for this attempt.
pub async fn reading_facts<'a, T, F: Future<Output = T>>(
    tx: &'a mut Tx,
    read: impl FnOnce(&'a mut Conn) -> F,
) -> T {
    if !tx.repeatable_read() {
        return read(tx).await;
    }
    let connection = std::ptr::from_ref::<Conn>(tx).addr();
    let read_only = tx.read_only();
    let work = READ_FACTS_UNCHANGED.scope(Cell::new(true), read(tx));
    crate::queries::read_facts::sharing(connection, read_only, work).await
}

/// A preview may be externally effect-free while making rollbackable permission changes.
/// Once a trial begins, neither its old facts nor its tentative facts are reused in this attempt.
pub(crate) fn discard_read_facts() {
    let _ = READ_FACTS_UNCHANGED.try_with(|unchanged| unchanged.set(false));
}

/// Whether the current read attempt has made no tentative dependency changes.
#[must_use]
pub fn read_facts_unchanged() -> bool {
    READ_FACTS_UNCHANGED.try_with(Cell::get).unwrap_or(false)
}

struct Serving {
    route: &'static str,
    open: Cell<u32>,
    refused: Cell<bool>,
    limits: Cell<Option<Limits>>,
    limits_seen: Cell<bool>,
    text_replacements: u64,
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
    serving_watched_with_text(route, source_text_replacements(), work).await
}

/// Carries the immutable request's repair receipt through its accepted write, including retries.
pub async fn serving_watched_with_text<T>(
    route: &'static str,
    replacements: u64,
    work: impl Future<Output = T>,
) -> (T, bool) {
    SERVING
        .scope(
            Serving {
                route,
                open: Cell::new(0),
                refused: Cell::new(false),
                limits: Cell::new(None),
                limits_seen: Cell::new(false),
                text_replacements: replacements,
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

/// Replacements in the source request of this task, never inferred from an existing replacement glyph.
pub fn source_text_replacements() -> u64 {
    SERVING
        .try_with(|serving| serving.text_replacements)
        .unwrap_or_default()
}

/// Notes that a commit of the work being served was refused because an authorization epoch it decided
/// under moved (`orch_commit_work`'s `authz_epoch_moved`).
fn note_refusal() {
    let _ = SERVING.try_with(|serving| serving.refused.set(true));
}

/// A mixed or unknown pool profile cannot certify the deadline of a later query error.
fn note_limits(limits: Option<Limits>) {
    let _ = SERVING.try_with(|serving| {
        if !serving.limits_seen.replace(true) {
            serving.limits.set(limits);
        } else if serving.limits.get() != limits {
            serving.limits.set(None);
        }
    });
}

fn captured_limit(phase: DatabaseWaitPhase) -> Option<Duration> {
    let limits = SERVING
        .try_with(|serving| serving.limits.get())
        .ok()
        .flatten()?;
    let limit = match phase {
        DatabaseWaitPhase::Acquire => return None,
        DatabaseWaitPhase::Statement => limits.statement?,
        DatabaseWaitPhase::IdleTransaction => limits.idle_in_transaction,
    };
    (!limit.is_zero()).then_some(limit)
}

/// PostgreSQL's cancellation SQLSTATE alone also describes manual cancellation.
fn timeout_phase(error: &sqlx::Error) -> Option<DatabaseWaitPhase> {
    if matches!(error, sqlx::Error::PoolTimedOut) {
        return Some(DatabaseWaitPhase::Acquire);
    }
    let native = error
        .as_database_error()?
        .try_downcast_ref::<sqlx::postgres::PgDatabaseError>()?;
    if native.file() != Some("postgres.c") || native.routine() != Some("ProcessInterrupts") {
        return None;
    }
    match native.code() {
        "25P03" => Some(DatabaseWaitPhase::IdleTransaction),
        "57014" if native.message() == "canceling statement due to statement timeout" => {
            Some(DatabaseWaitPhase::Statement)
        }
        _ => None,
    }
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

/// The work this task serves ([`serving`]), if any: what a measurement of a statement attributes it to.
#[must_use]
pub fn serving_route() -> Option<&'static str> {
    route_now()
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

// Keep the checkout until commit or rollback has finished and the next request can use it.
struct PooledTransaction {
    connection: PoolConnection<Postgres>,
    open: bool,
    read_only: bool,
}

impl PooledTransaction {
    async fn begin(
        connection: PoolConnection<Postgres>,
        read_only: bool,
    ) -> Result<Self, sqlx::Error> {
        let mut tx = Self {
            connection,
            open: true,
            read_only,
        };
        let statement = read_only.then(|| BEGIN_OPENING.into_sql_str());
        PgTransactionManager::begin(&mut tx.connection, statement).await?;
        Ok(tx)
    }

    async fn return_finished(mut self) {
        self.open = false;
        self.connection.return_to_pool().await;
    }
}

impl Drop for PooledTransaction {
    fn drop(&mut self) {
        if self.open {
            PgTransactionManager::start_rollback(&mut self.connection);
        }
    }
}

enum Transaction {
    Ordinary(sqlx::Transaction<'static, Postgres>),
    Pooled(PooledTransaction),
}

/// A transaction. Dropped without `commit`, it is rolled back. It is the connection it runs on
/// (`&mut *tx` is a [`Conn`]), and the development guard knows how long it is open.
pub struct Tx {
    inner: Transaction,
    held: Held,
    repeatable: bool,
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
        match &self.inner {
            Transaction::Ordinary(tx) => tx,
            Transaction::Pooled(tx) => &tx.connection,
        }
    }
}

impl DerefMut for Tx {
    fn deref_mut(&mut self) -> &mut Conn {
        match &mut self.inner {
            Transaction::Ordinary(tx) => tx,
            Transaction::Pooled(tx) => &mut tx.connection,
        }
    }
}

impl AsMut<Conn> for Tx {
    fn as_mut(&mut self) -> &mut Conn {
        self
    }
}

impl Tx {
    /// Begin a guarded write on an existing checkout. Once committed, the connection
    /// is back in the pool for the request that immediately follows the write.
    pub async fn begin_on(connection: PoolConnection<Postgres>) -> Result<Self, DbError> {
        check_nested("begin");
        let inner = finish_boundary(PooledTransaction::begin(connection, false)).await?;
        let mut tx = Self {
            inner: Transaction::Pooled(inner),
            held: Held::begin(),
            repeatable: false,
        };
        crate::queries::transaction_guard::begin_scope(&mut tx).await?;
        Ok(tx)
    }

    /// Whether PostgreSQL enforces a read-only snapshot on this transaction.
    #[must_use]
    pub fn read_only(&self) -> bool {
        matches!(&self.inner, Transaction::Pooled(inner) if inner.read_only)
    }

    /// Whether this entry point began a repeatable snapshot before any dependency was read.
    #[must_use]
    pub const fn repeatable_read(&self) -> bool {
        self.repeatable
    }

    /// Commits. The commit runs what the transaction left for it (`orch_commit_work`): a change
    /// of authority it decided under refuses it as [`DbError::Retry`].
    pub async fn commit(self) -> Result<(), sqlx::Error> {
        let Self { inner, held, .. } = self;
        let done = finish_boundary(async move {
            match inner {
                Transaction::Ordinary(mut tx) => {
                    // Run deferred checks while a rejected transaction still needs its
                    // rollback. A failed PostgreSQL COMMIT has already ended it.
                    sqlx::query("SET CONSTRAINTS ALL IMMEDIATE")
                        .execute(&mut *tx)
                        .await?;
                    tx.commit().await
                }
                Transaction::Pooled(mut tx) => {
                    if !tx.read_only {
                        sqlx::query("SET CONSTRAINTS ALL IMMEDIATE")
                            .execute(&mut *tx.connection)
                            .await?;
                    }
                    PgTransactionManager::commit(&mut tx.connection).await?;
                    tx.return_finished().await;
                    Ok(())
                }
            }
        })
        .await;
        drop(held);
        done
    }

    /// Rolls back.
    pub async fn rollback(self) -> Result<(), sqlx::Error> {
        let Self { inner, held, .. } = self;
        let done = finish_boundary(async move {
            match inner {
                Transaction::Ordinary(tx) => tx.rollback().await,
                Transaction::Pooled(mut tx) => {
                    PgTransactionManager::rollback(&mut tx.connection).await?;
                    tx.return_finished().await;
                    Ok(())
                }
            }
        })
        .await;
        drop(held);
        done
    }
}

/// Once BEGIN, COMMIT or ROLLBACK is sent, one task owns it through the
/// server's answer. Cancelling its waiter cannot return a half-finished boundary
/// to the pool. A BEGIN whose caller disappeared drops its completed transaction
/// normally; a COMMIT already begun finishes even if the HTTP request goes away.
async fn finish_boundary<T: Send + 'static>(
    work: impl Future<Output = Result<T, sqlx::Error>> + Send + 'static,
) -> Result<T, sqlx::Error> {
    tokio::spawn(work.with_current_subscriber())
        .await
        .map_err(|error| sqlx::Error::Io(std::io::Error::other(error)))?
}

/// A timeout's facts remain in the source chain when an engine error wraps it transparently.
#[derive(Debug, thiserror::Error)]
#[error("a native database deadline expired in {phase:?}")]
pub struct DatabaseTimeout {
    /// The native phase that expired.
    pub phase: DatabaseWaitPhase,
    /// The original applied limit, if that profile was known.
    pub applied_limit: Option<Duration>,
    /// The original cause remains available to the service's diagnostics only.
    #[source]
    pub source: sqlx::Error,
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
    /// A known native deadline, captured before later work can replace the pool profile.
    #[error("{source}")]
    Timeout {
        /// The original native cause and the facts captured with it.
        #[source]
        source: DatabaseTimeout,
    },
    /// This representation is invalid or exceeds a database program limit; retrying it cannot help.
    #[error("the value cannot be stored ({code})")]
    Data {
        /// PostgreSQL SQLSTATE, without an offending value or statement.
        code: String,
    },
    /// Anything else.
    #[error(transparent)]
    Other(sqlx::Error),
}

impl From<sqlx::Error> for DbError {
    fn from(error: sqlx::Error) -> Self {
        if let Some(phase) = timeout_phase(&error) {
            return Self::Timeout {
                source: DatabaseTimeout {
                    phase,
                    applied_limit: captured_limit(phase),
                    source: error,
                },
            };
        }
        if matches!(&error, sqlx::Error::ColumnDecode {source,..} | sqlx::Error::Decode(source) if source.downcast_ref::<orch_types::storable::InvalidEncoding>().is_some())
        {
            return Self::Data {
                code: "22000".to_owned(),
            };
        }
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
            Some(code) if code.starts_with("22") || code.starts_with("54") => Self::Data {
                code: code.to_owned(),
            },
            _ => Self::Other(error),
        }
    }
}

impl From<orch_types::storable::InvalidEncoding> for DbError {
    fn from(_: orch_types::storable::InvalidEncoding) -> Self {
        Self::Data {
            code: "22000".to_owned(),
        }
    }
}

impl DbError {
    /// A deterministic data/program-limit refusal, with no secret-bearing database message.
    #[must_use]
    pub fn data_code(&self) -> Option<&str> {
        match self {
            Self::Data { code } => Some(code),
            _ => None,
        }
    }
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
    configured_limits: Option<Limits>,
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
        Self::connect_kept_warm(
            url,
            connections,
            slow_statement,
            slow_acquire,
            limits,
            Warm::NONE,
        )
        .await
    }

    /// [`Db::connect_with_limits`], keeping `warm` connections open and ready ([`Warm`]): they are opened before
    /// this returns, and a keeper task looks after them until the pool is closed.
    pub async fn connect_kept_warm(
        url: &str,
        connections: u32,
        slow_statement: Duration,
        slow_acquire: Duration,
        limits: Limits,
        warm: Warm,
    ) -> Result<Self, DbError> {
        let options = limits.apply(
            PgConnectOptions::from_str(url)?
                .application_name("orch")
                .log_slow_statements(log::LevelFilter::Warn, slow_statement),
        );
        let keeper = std::sync::Arc::new(Keeper {
            warm,
            opened: AtomicU64::new(0),
            wake: tokio::sync::Notify::new(),
            hot: std::sync::Mutex::new(Vec::new()),
        });
        let told = std::sync::Arc::clone(&keeper);
        let pool = PgPoolOptions::new()
            .max_connections(connections)
            .min_connections(warm.connections.min(connections))
            // The keeper closes the idle connections beyond the warm ones; without warm ones they close as before.
            .idle_timeout((warm.connections == 0).then_some(Duration::from_secs(10 * 60)))
            .max_lifetime(warm.lifetime)
            .after_connect(move |_, _| {
                told.opened.fetch_add(1, Ordering::Relaxed);
                told.wake.notify_one();
                Box::pin(async { Ok(()) })
            })
            .acquire_timeout(limits.acquire_timeout)
            .acquire_slow_level(log::LevelFilter::Warn)
            .acquire_slow_threshold(slow_acquire)
            .connect_with(options)
            .await?;
        if warm.connections > 0 {
            tokio::spawn(keep_warm(pool.clone(), keeper).with_current_subscriber());
        }
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
        Ok(Self {
            pool,
            configured_limits: Some(limits),
        })
    }

    /// Notes that a caller asks for a connection, before it waits for one: a pool whose connections are all
    /// held shows nowhere else, not to the database, which sees a statement only once it has a connection.
    fn asking(&self, what: &'static str) {
        tracing::debug!(
            target: "orch_db::pool",
            phase = what,
            pool_size = self.pool.size(),
            pool_idle = self.pool.num_idle(),
            "database connection asked for"
        );
    }

    /// Notes how long a caller waited for a connection.
    fn measure(&self, waited: Duration) {
        tracing::debug!(
            target: "orch_db::pool",
            phase = "acquire",
            elapsed_secs = waited.as_secs_f64(),
            pool_size = self.pool.size(),
            pool_idle = self.pool.num_idle(),
            "database connection acquired"
        );
        metrics::histogram!(metrics_names::ACQUIRE_SECONDS).record(waited.as_secs_f64());
        if waited > ACQUIRE_BAR {
            metrics::counter!(metrics_names::ACQUIRE_OVER_BAR).increment(1);
        }
    }

    /// The database behind an existing pool.
    #[must_use]
    pub fn from_pool(pool: sqlx::PgPool) -> Self {
        Self {
            pool,
            configured_limits: None,
        }
    }

    /// Limits applied when this pool was opened; an externally supplied pool is unreported.
    #[must_use]
    pub fn configured_limits(&self) -> Option<Limits> {
        self.configured_limits
    }

    /// The pool, for the few things that need it as such (a listener).
    #[must_use]
    pub fn pool(&self) -> &sqlx::PgPool {
        &self.pool
    }

    /// A connection for statements that need no transaction.
    pub async fn acquire(&self) -> Result<sqlx::pool::PoolConnection<sqlx::Postgres>, DbError> {
        note_limits(self.configured_limits);
        check_nested("acquire");
        self.asking("acquire");
        let asked = Instant::now();
        let conn = self
            .pool
            .acquire()
            .await
            .map_err(|error| self.failure(error))?;
        self.measure(asked.elapsed());
        Ok(conn)
    }

    /// Begins a transaction.
    pub async fn begin(&self) -> Result<Tx, DbError> {
        self.begin_with_snapshot(false, false).await
    }

    /// Begins a repeatable snapshot before the authorization guard records its epochs in this transaction.
    pub async fn begin_snapshot(&self) -> Result<Tx, DbError> {
        self.begin_with_snapshot(true, false).await
    }

    /// Opens a read-only repeatable snapshot for the first screen unless the enclosing
    /// request has a lock class whose epoch bookkeeping requires a writable transaction.
    pub async fn begin_opening_snapshot(&self) -> Result<Tx, DbError> {
        self.begin_with_snapshot(
            true,
            crate::queries::transaction_guard::permits_read_only_snapshot(),
        )
        .await
    }

    async fn begin_with_snapshot(&self, snapshot: bool, read_only: bool) -> Result<Tx, DbError> {
        note_limits(self.configured_limits);
        check_nested("begin");
        self.asking("begin");
        let asked = Instant::now();
        let pool = self.pool.clone();
        let inner = finish_boundary(async move {
            if read_only {
                PooledTransaction::begin(pool.acquire().await?, true)
                    .await
                    .map(Transaction::Pooled)
            } else {
                pool.begin().await.map(Transaction::Ordinary)
            }
        })
        .await
        .map_err(|error| self.failure(error))?;
        self.measure(asked.elapsed());
        let mut tx = Tx {
            inner,
            held: Held::begin(),
            repeatable: snapshot,
        };
        if snapshot && !read_only {
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
        self.begin_isolated_locked(isolation, None).await
    }

    /// A utility lock can protect partition layout before the first snapshot or epoch query.
    pub(crate) async fn begin_isolated_locked(
        &self,
        isolation: &'static str,
        lock: Option<&'static str>,
    ) -> Result<Tx, DbError> {
        note_limits(self.configured_limits);
        check_nested("begin");
        self.asking("begin");
        let asked = Instant::now();
        let pool = self.pool.clone();
        let inner = finish_boundary(async move { pool.begin().await })
            .await
            .map_err(|error| self.failure(error))?;
        self.measure(asked.elapsed());
        let mut tx = Tx {
            inner: Transaction::Ordinary(inner),
            held: Held::begin(),
            repeatable: isolation == "SET TRANSACTION ISOLATION LEVEL REPEATABLE READ",
        };
        sqlx::query(isolation).execute(&mut *tx).await?;
        if let Some(lock) = lock {
            sqlx::query(lock).execute(&mut *tx).await?;
        }
        crate::queries::transaction_guard::begin_scope(&mut tx).await?;
        Ok(tx)
    }

    /// Keeps an entry-point failure tied to the pool whose deadline actually expired.
    fn failure(&self, error: sqlx::Error) -> DbError {
        let mut error = DbError::from(error);
        if let DbError::Timeout { source: timeout } = &mut error {
            timeout.applied_limit = match timeout.phase {
                DatabaseWaitPhase::Acquire => Some(self.pool.options().get_acquire_timeout()),
                DatabaseWaitPhase::Statement => {
                    self.configured_limits.and_then(|limits| limits.statement)
                }
                DatabaseWaitPhase::IdleTransaction => self
                    .configured_limits
                    .map(|limits| limits.idle_in_transaction),
            }
            .filter(|limit| !limit.is_zero());
        }
        error
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
    async fn finishing_an_opening_makes_its_connection_ready_for_the_next_permission_check() {
        let test = TestDb::fresh().await;
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .after_release(|_, _| {
                Box::pin(async {
                    tokio::task::yield_now().await;
                    Ok(true)
                })
            })
            .connect_lazy_with((*test.pool.connect_options()).clone());
        let db = Db::from_pool(pool.clone());

        for commit in [true, false] {
            let mut tx = db.begin_opening_snapshot().await.unwrap();
            let (isolation, read_only): (String, bool) = sqlx::query_as(
                "SELECT current_setting('transaction_isolation'), current_setting('transaction_read_only')::bool",
            )
            .fetch_one(&mut *tx)
            .await
            .unwrap();
            assert_eq!(isolation, "repeatable read");
            assert!(read_only);
            if commit {
                tx.commit().await.unwrap();
            } else {
                tx.rollback().await.unwrap();
            }
            assert_eq!(
                pool.num_idle(),
                1,
                "the next check can take the same session"
            );
        }

        let tx = db.begin_opening_snapshot().await.unwrap();
        drop(tx);
        let mut conn = pool.acquire().await.unwrap();
        let writable: bool =
            sqlx::query_scalar("SELECT NOT current_setting('transaction_read_only')::bool")
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        assert!(
            writable,
            "abandoning the projection rolled back its transaction"
        );
    }

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
            (Limits::BACKGROUND, Some("0")),
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
    async fn a_pool_applies_the_configured_deadlines_and_reports_only_known_limits() {
        let test = TestDb::fresh().await;
        let limits = Limits {
            acquire_timeout: Duration::from_millis(31_000),
            idle_in_transaction: Duration::from_millis(9_876),
            statement: Some(Duration::from_millis(23_456)),
            ..Limits::REQUESTS
        };
        let db = Db::connect_with_limits(&test.url, 1, SLOW_DEFAULT, SLOW_DEFAULT, limits)
            .await
            .unwrap();
        assert_eq!(
            db.pool.options().get_acquire_timeout(),
            limits.acquire_timeout
        );
        for (name, expected) in [
            ("statement_timeout", 23_456_i64),
            ("idle_in_transaction_session_timeout", 9_876),
        ] {
            let applied: i64 =
                sqlx::query_scalar("SELECT setting::bigint FROM pg_settings WHERE name=$1")
                    .bind(name)
                    .fetch_one(db.pool())
                    .await
                    .unwrap();
            assert_eq!(applied, expected);
        }
        assert_eq!(db.configured_limits(), Some(limits));
        assert_eq!(Db::from_pool(test.pool.clone()).configured_limits(), None);
    }

    #[test]
    fn each_service_profile_uses_its_loaded_deadlines_instead_of_another_profiles_defaults() {
        let config = orch_config::server::Database {
            acquire_timeout_ms: 70_000,
            request_statement_timeout_ms: 0,
            background_statement_timeout_ms: 90_000,
            idle_transaction_timeout_ms: 0,
            prepared_statements: 64,
            ..Default::default()
        };
        let request = Limits::requests(&config);
        let background = Limits::background(&config);
        assert_eq!(request.acquire_timeout, Duration::from_secs(70));
        assert_eq!(request.prepared_statements, 64);
        assert_eq!(background.prepared_statements, 64);
        assert_eq!(background.acquire_timeout, request.acquire_timeout);
        assert_eq!(request.idle_in_transaction, Duration::ZERO);
        assert_eq!(request.statement, None);
        assert_eq!(background.statement, Some(Duration::from_secs(90)));
        assert_eq!(Limits::requests(&Default::default()), Limits::REQUESTS);
        assert_eq!(Limits::background(&Default::default()), Limits::BACKGROUND);
    }

    #[test]
    fn an_explicitly_unbounded_pool_clears_the_databases_own_statement_default() {
        let limits = Limits {
            idle_in_transaction: Duration::ZERO,
            ..Limits::BACKGROUND
        };
        let settings = limits.settings();
        assert!(
            settings
                .iter()
                .any(|(key, value)| *key == "statement_timeout" && value == "0")
        );
        assert!(
            settings
                .iter()
                .any(|(key, value)| *key == "idle_in_transaction_session_timeout" && value == "0")
        );
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

    /// A turn's start alone runs a few hundred distinct statements. A connection that kept only the last hundred
    /// would parse and plan most of them again on every turn, each a round trip more and the planner's work again.
    #[tokio::test]
    async fn a_connection_keeps_every_statement_of_a_busy_path_prepared() {
        let test = TestDb::fresh().await;
        let db =
            Db::connect_with_limits(&test.url, 1, SLOW_DEFAULT, SLOW_DEFAULT, Limits::REQUESTS)
                .await
                .unwrap();
        let distinct = 300_i64;
        for round in 0..2_i64 {
            let mut conn = db.acquire().await.unwrap();
            for statement in 0..distinct {
                let sql = format!("SELECT {statement}::bigint + $1");
                let _: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
                    .bind(round)
                    .fetch_one(&mut *conn)
                    .await
                    .unwrap();
            }
        }
        let mut conn = db.acquire().await.unwrap();
        let prepared: i64 =
            sqlx::query_scalar("SELECT count(*) FROM pg_prepared_statements WHERE NOT from_sql")
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        assert!(
            prepared > distinct,
            "the connection keeps {prepared} statements prepared after running {distinct} distinct ones twice"
        );
    }

    /// How many of the statements `SELECT <n>::bigint + $1` each of the pool's `connections` holds prepared, as
    /// PostgreSQL lists them for each session.
    async fn prepared_on_each(db: &Db, connections: usize) -> Vec<i64> {
        let mut held = Vec::new();
        for _ in 0..connections {
            held.push(db.acquire().await.unwrap());
        }
        let mut counts = Vec::new();
        for conn in &mut held {
            counts.push(
                sqlx::query_scalar(
                    "SELECT count(*) FROM pg_prepared_statements WHERE statement LIKE 'SELECT %::bigint + $1'",
                )
                .persistent(false)
                .fetch_one(&mut **conn)
                .await
                .unwrap(),
            );
        }
        counts
    }

    #[tokio::test]
    async fn every_warm_connection_comes_to_hold_what_another_ran_without_serving_it() {
        let test = TestDb::fresh().await;
        let warm = Warm {
            connections: 4,
            lifetime: None,
            statements: 64,
            pass: Duration::from_millis(50),
        };
        let db = Db::connect_kept_warm(
            &test.url,
            4,
            SLOW_DEFAULT,
            SLOW_DEFAULT,
            Limits::REQUESTS,
            warm,
        )
        .await
        .unwrap();
        assert_eq!(
            db.pool().size(),
            4,
            "the warm connections are open before the pool serves"
        );
        let distinct = 40_i64;
        {
            let mut conn = db.acquire().await.unwrap();
            for statement in 0..distinct {
                let _: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
                    "SELECT {statement}::bigint + $1"
                )))
                .bind(1_i64)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
            }
        }
        let started = Instant::now();
        let mut counts = prepared_on_each(&db, 4).await;
        while counts.iter().any(|count| *count < distinct)
            && started.elapsed() < Duration::from_secs(20)
        {
            tokio::time::sleep(Duration::from_millis(100)).await;
            counts = prepared_on_each(&db, 4).await;
        }
        assert_eq!(
            counts,
            vec![distinct; 4],
            "each connection holds the statements another ran"
        );
        db.pool().close().await;
    }

    #[tokio::test]
    async fn idle_connections_beyond_the_warm_ones_close_and_the_warm_ones_stay() {
        let test = TestDb::fresh().await;
        let warm = Warm {
            connections: 1,
            lifetime: None,
            statements: 64,
            pass: Duration::from_millis(50),
        };
        let db = Db::connect_kept_warm(
            &test.url,
            3,
            SLOW_DEFAULT,
            SLOW_DEFAULT,
            Limits::REQUESTS,
            warm,
        )
        .await
        .unwrap();
        let mut held = Vec::new();
        for _ in 0..3 {
            held.push(db.acquire().await.unwrap());
        }
        assert_eq!(db.pool().size(), 3);
        drop(held);
        let started = Instant::now();
        while db.pool().size() > 1 && started.elapsed() < Duration::from_secs(20) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(
            db.pool().size(),
            1,
            "the idle connections beyond the warm one were closed"
        );
        tokio::time::sleep(warm.pass * 4).await;
        assert_eq!(db.pool().size(), 1, "the warm one stays open while idle");
        db.pool().close().await;
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
