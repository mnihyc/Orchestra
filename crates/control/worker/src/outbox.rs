//! Claim transactional outbox rows and deliver each through its notifier, with retry, and a route or a destination switched off when its target is gone.
//!
//! **One attempt in three steps.** A worker finds the delivery that is due and takes the locks in
//! their order: the channel (shared), the person's choices (shared), the route, then the delivery
//! itself, passing over one that another worker holds. In that short transaction it reads all
//! the message needs and reserves the delivery for one attempt: `sending`, with an attempt
//! identifier of its own, the worker's identity and a deadline, which is the longest a notifier
//! is given ([`DELIVERY_DEADLINE`]) and a margin. The transaction ends before the notifier is
//! called: no connection and no lock is held while the far side answers. What the call came to
//! is recorded in a second short transaction, and only where the delivery is still `sending`
//! under that very attempt, so that a late answer changes nothing that moved on (a retry, a
//! cancellation, a removed channel). A reservation whose deadline passed, because its worker
//! ended, goes back to the retry path with its try counted: a send that was accepted but whose
//! answer was lost may go twice, never not at all.
//!
//! What a try came to is decided by the contract: taken; to be tried again after the pause of the
//! schedule or the one the far side named; failed for good; or the target is gone, which
//! switches off the route or the destination, or removes the browser. A route keeps how it
//! fared: when its notifier last accepted (no proof that a device showed anything), why it
//! failed, and the failure episode it is in until it fully recovers. The first terminal outcome
//! of an episode tells its person once, in the Inbox only. Push that reaches some browsers and
//! not others is `partial`, and names the browsers it lost.
//!
//! There are three kinds of row. **A notification for a person** names a route: while quiet hours
//! or a pause hold the person, it waits unreserved, marked as held and with no try counted, and
//! what was held for one route goes out as one message when both holds are over. Every try looks
//! again at whether the route and its channel are on, and leaves out what the person has read
//! since. **A notification for a node** names a destination. **Mail that is no notification**
//! names a channel and carries the message signing in wrote; once delivered, or failed for good,
//! what it holds that is secret is blanked.

use crate::channels::{
    self, ChannelError, DELIVERY_DEADLINE, Notifiers, Surroundings, config_of_channel,
    config_of_destination, guarded,
};
use crate::heartbeat::Metrics;
use crate::quiet_hours;
use crate::render::{self, Rendering};
use crate::spans;
use crate::system_mail::{self, MailRow};
use chrono::{DateTime, Utc};
use orch_db::queries::notifications::channels::{self as rows, ChannelRow};
use orch_db::queries::notifications::outbox::{
    self as db_outbox, OutboxRow, Payload, Settle, Snapshot,
};
use orch_db::queries::notifications::pages;
use orch_db::queries::notifications::people::{self, PushSubscriptionRow, Recipient};
use orch_db::queries::notifications::routes::{self, health};
use orch_db::secrets::MasterKey;
use orch_db::{Conn, Db, DbError};
use orch_notifier_contract::delivery::{Config, DeliverError, Delivery, Next, after_delivery};
use orch_notifier_contract::descriptor::{NotifierDescriptor, Reaches};
use orch_notifier_contract::notifier::Notifier;
use orch_notifier_contract::templates::Form;
use orch_types::{Clock, Id, Json};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

/// How many deliveries one worker makes at once: enough that a slow mail server
/// does not hold up a bot, few enough to leave connections of the pool for the
/// other jobs.
pub const CONCURRENCY: usize = 4;

/// The most deliveries one run of the job takes before it ends, so that a long
/// backlog does not keep the job's next turn waiting for ever.
pub const DELIVERIES_PER_RUN: usize = 200;

/// The longest text of an error kept on a row or a channel: enough for the
/// words of a server, not enough for a page.
pub const ERROR_CHARS: usize = 500;

/// How many failures in a row take a browser off the list that push is sent
/// to.
pub const PUSH_FAILURES_BEFORE_REMOVAL: i32 = 5;

/// How long past the notifier's deadline a reservation is still its worker's: what recording
/// the answer of a call that took all its time needs.
const RESERVATION_MARGIN: Duration = Duration::from_secs(60);

/// How many deliveries one try looks at before it ends for now, when other workers hold those it
/// finds: enough that several workers at once each find one, few enough that it ends soon.
const CANDIDATES_PER_TRY: usize = 16;

/// Why a run of the outbox stopped.
#[derive(Debug, thiserror::Error)]
pub enum OutboxError {
    /// The database refused.
    #[error(transparent)]
    Db(#[from] DbError),
    /// A channel could not be loaded for a reason of the database's.
    #[error(transparent)]
    Channel(#[from] ChannelError),
    /// Current recipient access could not be checked.
    #[error(transparent)]
    Access(#[from] orch_authz::Error),
}

/// What a run delivered.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Run {
    /// Messages the far side took.
    pub sent: u32,
    /// Messages that will be tried again.
    pub retried: u32,
    /// Messages that failed for good.
    pub failed: u32,
    /// Messages whose target was gone.
    pub gone: u32,
    /// Messages that quiet hours or a pause held back.
    pub held: u32,
}

impl Run {
    /// The number of messages this run dealt with.
    #[must_use]
    pub fn total(&self) -> u32 {
        self.sent + self.retried + self.failed + self.gone + self.held
    }

    fn add(&mut self, other: Self) {
        self.sent += other.sent;
        self.retried += other.retried;
        self.failed += other.failed;
        self.gone += other.gone;
        self.held += other.held;
    }

    fn one(how: Done) -> Self {
        let mut run = Self::default();
        match how {
            Done::Sent => run.sent = 1,
            Done::Retried => run.retried = 1,
            Done::Failed => run.failed = 1,
            Done::Gone => run.gone = 1,
            Done::Held => run.held = 1,
        }
        run
    }
}

/// What became of one unit of work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Done {
    Sent,
    Retried,
    Failed,
    Gone,
    Held,
}

/// What the outbox is made of.
pub struct OutboxParts {
    /// The database.
    pub db: Db,
    /// Where time comes from.
    pub clock: Arc<dyn Clock>,
    /// The notifiers there are.
    pub notifiers: Notifiers,
    /// The key that opens sealed secrets.
    pub master: MasterKey,
    /// What a message is written with.
    pub surroundings: Surroundings,
    /// The pause before each further try of a delivery that failed, as
    /// `[worker] outbox_retry_secs` has it.
    pub retry: Vec<Duration>,
    /// What is counted.
    pub metrics: Arc<Metrics>,
}

struct Inner {
    db: Db,
    clock: Arc<dyn Clock>,
    notifiers: Notifiers,
    master: MasterKey,
    surroundings: Surroundings,
    retry: Vec<Duration>,
    metrics: Arc<Metrics>,
    /// This worker's identity on the deliveries it reserves.
    owner: Id,
}

/// The delivery of the outbox's rows.
#[derive(Clone)]
pub struct Outbox {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Outbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Outbox").finish_non_exhaustive()
    }
}

/// Keeps an error to the length it may have on a row or a channel.
fn kept(text: &str) -> String {
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= ERROR_CHARS {
        one_line
    } else {
        let mut cut: String = one_line.chars().take(ERROR_CHARS - 1).collect();
        cut.push('…');
        cut
    }
}

/// What a failed try is kept as on a row and on a channel: the code and the
/// words of the far side, on one line and no longer than [`ERROR_CHARS`].
#[must_use]
pub fn error_line(error: &DeliverError) -> String {
    kept(&format!("{}: {}", error.code, error.message))
}

/// Where an attempt for a person goes.
enum Aim {
    /// One address, chat or hook.
    One(Json),
    /// Each browser of the person that the message has not reached yet.
    Browsers(Vec<PushSubscriptionRow>),
    /// Nowhere: the person lacks what the channel reaches them by.
    Nowhere(DeliverError),
}

/// What an attempt is for.
enum Work {
    /// A person, through a route.
    Route {
        route: Id,
        channel: Id,
        person: Box<Recipient>,
        config: Config,
        message: render::Message,
        aim: Aim,
    },
    /// A node's destination.
    Destination { destination: Id, delivery: Delivery },
    /// Mail that is no notification.
    Mail {
        channel: Id,
        delivery: Delivery,
        scrubbed: Json,
    },
}

/// One attempt, reserved, with everything its call needs.
struct Attempt {
    unit: Vec<OutboxRow>,
    dispatch: Id,
    notifier: Arc<dyn Notifier>,
    kind: String,
    form: Form,
    work: Work,
}

/// What the call of an attempt came to.
enum Called {
    /// One delivery's answer.
    One(Result<(), DeliverError>),
    /// Each browser's answer: the browser, its name, and what it answered.
    Browsers(Vec<(Id, String, Result<(), DeliverError>)>),
}

/// What the first step came to.
enum Step {
    /// Another worker has the delivery, or it is no longer due.
    Passed,
    /// It was settled without a call: held, cancelled or failed before it could go.
    Settled(Run),
    /// It is reserved and goes.
    Go(Box<Attempt>),
}

impl Outbox {
    /// An outbox of these parts.
    #[must_use]
    pub fn new(parts: OutboxParts) -> Self {
        Self {
            inner: Arc::new(Inner {
                db: parts.db,
                clock: parts.clock,
                notifiers: parts.notifiers,
                master: parts.master,
                surroundings: parts.surroundings,
                retry: parts.retry,
                metrics: parts.metrics,
                owner: orch_types::new_id(),
            }),
        }
    }

    /// Takes and delivers every row that is due, a few at once, until none is
    /// or the run has made [`DELIVERIES_PER_RUN`] deliveries. Reservations that
    /// outlived their deadline are given back to the retry path first.
    pub async fn run(&self) -> Result<Run, OutboxError> {
        self.run_until(&CancellationToken::new()).await
    }

    /// The same, but a stop that is asked for ends it between two deliveries: a
    /// delivery under way is finished, so that its row is settled.
    pub async fn run_until(&self, stop: &CancellationToken) -> Result<Run, OutboxError> {
        self.recover().await?;
        let budget = Arc::new(AtomicUsize::new(DELIVERIES_PER_RUN));
        let mut workers = JoinSet::new();
        for _ in 0..CONCURRENCY {
            let (outbox, budget, stop) = (self.clone(), Arc::clone(&budget), stop.clone());
            workers.spawn(async move {
                let mut tally = Run::default();
                while !stop.is_cancelled()
                    && budget
                        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                            left.checked_sub(1)
                        })
                        .is_ok()
                {
                    match outbox.deliver_next().await? {
                        Some(done) => tally.add(done),
                        None => break,
                    }
                }
                Ok::<Run, OutboxError>(tally)
            });
        }
        let mut total = Run::default();
        let mut failure = None;
        while let Some(joined) = workers.join_next().await {
            match joined {
                Ok(Ok(tally)) => total.add(tally),
                Ok(Err(error)) => failure = Some(error),
                Err(error) => tracing::error!(%error, "a delivery task ended abnormally"),
            }
        }
        match failure {
            Some(error) if total.total() == 0 => Err(error),
            Some(error) => {
                tracing::warn!(%error, "the outbox stopped early");
                Ok(total)
            }
            None => Ok(total),
        }
    }

    /// Gives back to the retry path the reservations whose deadline has passed: the worker that
    /// made them ended, or its call never came back.
    pub async fn recover(&self) -> Result<u64, OutboxError> {
        let mut conn = self.inner.db.acquire().await?;
        let recovered = db_outbox::recover_abandoned(&mut conn, self.inner.clock.now()).await?;
        if recovered > 0 {
            tracing::warn!(
                recovered,
                "deliveries whose attempt was never confirmed go back to their retries"
            );
        }
        Ok(recovered)
    }

    /// Finds the next delivery that is due, reserves it and delivers it, or settles it at once
    /// where it is not to go now. None where nothing is due, or everything that is due is held
    /// by other workers.
    pub async fn deliver_next(&self) -> Result<Option<Run>, OutboxError> {
        let inner = &self.inner;
        let mut passed: Vec<Id> = Vec::new();
        let mut passed_routes: Vec<Id> = Vec::new();
        for _ in 0..CANDIDATES_PER_TRY {
            // Current read authority is held before any outbox row, as a request does: a
            // withdrawal of access has one order with this delivery.
            let mut tx =
                orch_db::queries::transaction_guard::scope(false, inner.db.begin()).await?;
            orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
            let now = inner.clock.now();
            let step = match db_outbox::next_due(&mut tx, now, &passed).await? {
                Some(due) => {
                    let step = self.first_step(&mut tx, now, due).await?;
                    if matches!(step, Step::Passed) {
                        passed.push(due.id);
                    }
                    step
                }
                None => match db_outbox::next_held_route(&mut tx, now, &passed_routes).await? {
                    Some(route) => {
                        let step = self.first_for_a_route(&mut tx, now, route, None).await?;
                        if matches!(step, Step::Passed) {
                            passed_routes.push(route);
                        }
                        step
                    }
                    None => {
                        tx.rollback().await.map_err(DbError::from)?;
                        return Ok(None);
                    }
                },
            };
            match step {
                Step::Passed => tx.rollback().await.map_err(DbError::from)?,
                Step::Settled(done) => {
                    tx.commit().await.map_err(DbError::from)?;
                    return Ok(Some(done));
                }
                Step::Go(attempt) => {
                    tx.commit().await.map_err(DbError::from)?;
                    return Ok(Some(self.go(*attempt).await?));
                }
            }
        }
        Ok(None)
    }

    /// The first step for a delivery that was found due.
    async fn first_step(
        &self,
        conn: &mut Conn,
        now: DateTime<Utc>,
        due: db_outbox::Due,
    ) -> Result<Step, OutboxError> {
        if let Some(route) = due.route_id {
            return self.first_for_a_route(conn, now, route, Some(due.id)).await;
        }
        let Some(row) = db_outbox::take_due(conn, due.id, now).await? else {
            return Ok(Step::Passed);
        };
        match (row.destination_id, row.channel_id) {
            (Some(destination), _) => {
                self.first_for_a_destination(conn, now, row, destination)
                    .await
            }
            (None, Some(channel)) => self.first_for_mail(conn, now, row, channel).await,
            (None, None) => Ok(Step::Settled(
                fail_all(
                    conn,
                    std::slice::from_ref(&row),
                    "the row names neither a route, a destination nor a channel",
                    Also::Nothing,
                )
                .await?,
            )),
        }
    }

    /// The pause before a try, given how many were made.
    fn next(&self, result: &Result<(), DeliverError>, attempts: u32) -> Next {
        after_delivery(result, attempts, &self.inner.retry)
    }

    /// How long a reservation made now lasts.
    fn reservation_end(now: DateTime<Utc>) -> DateTime<Utc> {
        now + spans::seconds((DELIVERY_DEADLINE + RESERVATION_MARGIN).as_secs())
    }

    // ----- a notification for a person -------------------------------------------------------

    /// The first step for a person's route: one delivery that is due (`one`), or, without one,
    /// what a hold kept back for the route, which goes together.
    async fn first_for_a_route(
        &self,
        conn: &mut Conn,
        now: DateTime<Utc>,
        route_id: Id,
        one: Option<Id>,
    ) -> Result<Step, OutboxError> {
        let inner = &self.inner;
        let Some(seen) = routes::route(conn, route_id).await? else {
            return Ok(Step::Passed);
        };
        // The locks in their order: the channel, the person's choices, the route, the delivery.
        let channel = match seen.channel_id {
            Some(channel) => rows::lock_channel_shared(conn, channel).await?,
            None => None,
        };
        people::lock_prefs_shared(conn, seen.user_id).await?;
        let Some(route) = routes::lock_route(conn, route_id).await? else {
            return Ok(Step::Passed);
        };
        let unit: Vec<OutboxRow> = match one {
            Some(id) => match db_outbox::take_due(conn, id, now).await? {
                Some(row) => vec![row],
                None => return Ok(Step::Passed),
            },
            None => db_outbox::take_held_of_route(conn, route.id, now).await?,
        };
        if unit.is_empty() {
            return Ok(Step::Passed);
        }
        let ids: Vec<Id> = unit.iter().map(|row| row.id).collect();
        let channel = match (channel, route.channel_id) {
            (Some(channel), Some(current)) if channel.id == current => channel,
            _ => {
                for id in &ids {
                    db_outbox::cancel(conn, *id, "channel_removed").await?;
                }
                return Ok(Step::Settled(Run::default()));
            }
        };
        let Some(person) = people::recipient(conn, route.user_id).await? else {
            return Ok(Step::Settled(
                fail_all(conn, &unit, "the person is gone", Also::Nothing).await?,
            ));
        };
        if !route.enabled {
            for id in &ids {
                db_outbox::cancel(conn, *id, "route_off").await?;
            }
            return Ok(Step::Settled(Run::default()));
        }
        if !channel.enabled {
            return Ok(Step::Settled(
                fail_all(conn, &unit, "the channel is switched off", Also::Nothing).await?,
            ));
        }
        if let Some(until) = quiet_hours::outward_held_until(
            &person.timezone,
            person.quiet_window(),
            person.pause_until,
            now,
        ) {
            db_outbox::hold_unreserved(conn, &ids, until).await?;
            routes::set_held(conn, route.id, true).await?;
            return Ok(Step::Settled(Run::one(Done::Held)));
        }
        let Some(notifier) = inner.notifiers.get(&channel.kind).cloned() else {
            return Ok(Step::Settled(
                fail_all(
                    conn,
                    &unit,
                    &format!("there is no notifier named {}", channel.kind),
                    Also::Channel(&channel),
                )
                .await?,
            ));
        };
        let declared = notifier.describe();
        let (snapshots, read) = self.still_to_tell(conn, now, route.user_id, &unit).await?;
        if snapshots.is_empty() {
            let reason = if read { "read" } else { "lost_access" };
            for id in &ids {
                db_outbox::cancel(conn, *id, reason).await?;
            }
            routes::set_held(conn, route.id, false).await?;
            return Ok(Step::Settled(Run::default()));
        }
        let config = match config_of_channel(conn, &inner.master, &channel).await {
            Ok(config) => config,
            Err(error @ ChannelError::Sealed { .. }) => {
                return Ok(Step::Settled(
                    fail_all(conn, &unit, &error.to_string(), Also::Channel(&channel)).await?,
                ));
            }
            Err(error) => return Err(error.into()),
        };
        let message = match self
            .render_for(
                conn,
                unit[0].id,
                &declared,
                &channel.templates,
                Some(&person),
                &snapshots,
            )
            .await?
        {
            Ok(message) => message,
            Err(fault) => {
                return Ok(Step::Settled(
                    fail_all(conn, &unit, &fault.to_string(), Also::Channel(&channel)).await?,
                ));
            }
        };
        let address_fault = |text: &str| DeliverError::gone("no_address", text);
        let aim = match declared.reaches {
            Reaches::Devices => {
                Aim::Browsers(people::push_subscriptions_of(conn, person.id).await?)
            }
            Reaches::Email => match person.confirmed_email() {
                None => Aim::Nowhere(address_fault("the person has no confirmed address")),
                Some(address) => Aim::One(serde_json::json!({ "email": address })),
            },
            Reaches::Link => {
                let linked = route
                    .address
                    .as_object()
                    .is_some_and(|members| !members.is_empty());
                if linked {
                    Aim::One(route.address.clone())
                } else {
                    Aim::Nowhere(address_fault("the person's route is not linked"))
                }
            }
            Reaches::Channel => Aim::One(serde_json::json!({})),
        };
        let dispatch = orch_types::new_id();
        let ends = Self::reservation_end(now);
        for id in &ids {
            if !db_outbox::reserve_as(conn, *id, dispatch, inner.owner, ends, now).await? {
                return Ok(Step::Passed);
            }
        }
        routes::set_held(conn, route.id, false).await?;
        let attempt = Attempt {
            unit,
            dispatch,
            notifier,
            kind: channel.kind.clone(),
            form: message.form,
            work: Work::Route {
                route: route.id,
                channel: channel.id,
                person: Box::new(person),
                config,
                message,
                aim,
            },
        };
        Ok(Step::Go(Box::new(attempt)))
    }

    /// The notifications of a unit that are still to be told: what the person read since is left
    /// out, and so is a notice about work they may no longer read. Says too whether anything was
    /// left out for having been read.
    async fn still_to_tell(
        &self,
        conn: &mut Conn,
        now: DateTime<Utc>,
        user: Id,
        unit: &[OutboxRow],
    ) -> Result<(Vec<Snapshot>, bool), OutboxError> {
        let merged = merged_notifications(unit);
        let ids: Vec<Id> = merged.iter().filter_map(|each| each.id).collect();
        let unread: Vec<Id> = if ids.is_empty() {
            Vec::new()
        } else {
            people::unread_among(conn, user, &ids).await?
        };
        let known: Vec<Id> = if ids.is_empty() {
            Vec::new()
        } else {
            people::known_among(conn, user, &ids).await?
        };
        let mut read = false;
        let mut snapshots = Vec::new();
        for snapshot in merged {
            // A notification the Inbox still has and that was read since is not told.
            if let Some(id) = snapshot.id
                && known.contains(&id)
                && !unread.contains(&id)
            {
                read = true;
                continue;
            }
            if matches!(
                snapshot.kind.as_str(),
                "doc.comment" | "doc.mention" | "person.notify" | "mention" | "chat.message"
            ) {
                let principal = match orch_authz::Principal::load(
                    conn,
                    orch_authz::Caller::session(user),
                    orch_types::Time::new(now),
                )
                .await
                {
                    Ok(principal) => principal,
                    Err(orch_authz::Error::NoSuchUser(_) | orch_authz::Error::Suspended { .. }) => {
                        continue;
                    }
                    Err(error) => return Err(error.into()),
                };
                if !orch_authz::documents::notice_readable(
                    conn,
                    &principal,
                    &snapshot.kind,
                    &snapshot.data,
                    snapshot.chat_id,
                    snapshot.thread_id,
                )
                .await?
                {
                    continue;
                }
            }
            snapshots.push(snapshot);
        }
        Ok((snapshots, read))
    }

    // ----- a notification for a node -----------------------------------------------------------

    async fn first_for_a_destination(
        &self,
        conn: &mut Conn,
        now: DateTime<Utc>,
        row: OutboxRow,
        destination_id: Id,
    ) -> Result<Step, OutboxError> {
        let inner = &self.inner;
        let unit = vec![row];
        let Some(destination) = rows::destination(conn, destination_id).await? else {
            return Ok(Step::Settled(
                fail_all(conn, &unit, "the destination is gone", Also::Nothing).await?,
            ));
        };
        if !destination.enabled {
            return Ok(Step::Settled(
                fail_all(
                    conn,
                    &unit,
                    "the destination is switched off",
                    Also::Nothing,
                )
                .await?,
            ));
        }
        let Some(notifier) = inner.notifiers.get(&destination.channel).cloned() else {
            return Ok(Step::Settled(
                fail_all(
                    conn,
                    &unit,
                    &format!("there is no notifier named {}", destination.channel),
                    Also::Nothing,
                )
                .await?,
            ));
        };
        let declared = notifier.describe();
        let snapshots = merged_notifications(&unit);
        if snapshots.is_empty() {
            return Ok(Step::Settled(
                fail_all(conn, &unit, "the row tells nothing", Also::Nothing).await?,
            ));
        }
        let config = match config_of_destination(conn, &inner.master, &destination).await {
            Ok(config) => config,
            Err(error @ ChannelError::Sealed { .. }) => {
                return Ok(Step::Settled(
                    fail_all(conn, &unit, &error.to_string(), Also::Nothing).await?,
                ));
            }
            Err(error) => return Err(error.into()),
        };
        let message = match self
            .render_for(
                conn,
                unit[0].id,
                &declared,
                &destination.templates,
                None,
                &snapshots,
            )
            .await?
        {
            Ok(message) => message,
            Err(fault) => {
                return Ok(Step::Settled(
                    fail_all(conn, &unit, &fault.to_string(), Also::Nothing).await?,
                ));
            }
        };
        let attempts = attempts_of(&unit);
        let delivery = delivery_of(
            unit[0].id,
            attempts,
            config,
            serde_json::json!({}),
            None,
            &message,
        );
        let Some(dispatch) = db_outbox::reserve(
            conn,
            unit[0].id,
            inner.owner,
            Self::reservation_end(now),
            now,
        )
        .await?
        else {
            return Ok(Step::Passed);
        };
        Ok(Step::Go(Box::new(Attempt {
            unit,
            dispatch,
            notifier,
            kind: destination.channel.clone(),
            form: message.form,
            work: Work::Destination {
                destination: destination.id,
                delivery,
            },
        })))
    }

    // ----- mail that is no notification ---------------------------------------------------------

    async fn first_for_mail(
        &self,
        conn: &mut Conn,
        now: DateTime<Utc>,
        row: OutboxRow,
        channel_id: Id,
    ) -> Result<Step, OutboxError> {
        let inner = &self.inner;
        let unit = vec![row];
        let Some(mail) = MailRow::read(&unit[0].payload) else {
            return Ok(Step::Settled(
                fail_all(
                    conn,
                    &unit,
                    "the row is no message the worker can write",
                    Also::Nothing,
                )
                .await?,
            ));
        };
        let scrubbed = system_mail::scrubbed(&unit[0].payload);
        let Some(channel) = rows::channel(conn, channel_id).await? else {
            return Ok(Step::Settled(
                fail_all(
                    conn,
                    &unit,
                    "the mail server is gone",
                    Also::Payload(&scrubbed),
                )
                .await?,
            ));
        };
        if !channel.enabled {
            return Ok(Step::Settled(
                fail_all(
                    conn,
                    &unit,
                    "the mail server is switched off",
                    Also::Payload(&scrubbed),
                )
                .await?,
            ));
        }
        let Some(notifier) = inner.notifiers.get(&channel.kind).cloned() else {
            return Ok(Step::Settled(
                fail_all(
                    conn,
                    &unit,
                    &format!("there is no notifier named {}", channel.kind),
                    Also::Payload(&scrubbed),
                )
                .await?,
            ));
        };
        let person = match mail.user_id {
            Some(user) => people::recipient(conn, user).await?,
            None => None,
        };
        let language = if mail.language.is_empty() {
            person
                .as_ref()
                .map_or("en", |person| person.locale.as_str())
                .to_owned()
        } else {
            mail.language.clone()
        };
        let config = match config_of_channel(conn, &inner.master, &channel).await {
            Ok(config) => config,
            Err(error @ ChannelError::Sealed { .. }) => {
                return Ok(Step::Settled(
                    fail_all(conn, &unit, &error.to_string(), Also::Payload(&scrubbed)).await?,
                ));
            }
            Err(error) => return Err(error.into()),
        };
        let text = system_mail::text_of(&mail, &language, &inner.surroundings.system_name);
        let attempts = attempts_of(&unit);
        let delivery = Delivery {
            id: unit[0].id,
            attempt: attempts,
            config,
            to: serde_json::json!({ "email": mail.to }),
            recipient: person.as_ref().map(render::user_ref),
            language,
            subject: Some(text.subject),
            body: text.body,
            notifications: Vec::new(),
        };
        let Some(dispatch) = db_outbox::reserve(
            conn,
            unit[0].id,
            inner.owner,
            Self::reservation_end(now),
            now,
        )
        .await?
        else {
            return Ok(Step::Passed);
        };
        Ok(Step::Go(Box::new(Attempt {
            unit,
            dispatch,
            notifier,
            kind: channel.kind.clone(),
            form: Form::Single,
            work: Work::Mail {
                channel: channel.id,
                delivery,
                scrubbed,
            },
        })))
    }

    // ----- the call, with nothing held ----------------------------------------------------------

    /// Makes the call of a reserved attempt, then records what it came to.
    async fn go(&self, attempt: Attempt) -> Result<Run, OutboxError> {
        let called = self.call(&attempt).await;
        let mut tx =
            orch_db::queries::transaction_guard::scope(false, self.inner.db.begin()).await?;
        orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
        let now = self.inner.clock.now();
        let done = self.conclude(&mut tx, now, &attempt, called).await?;
        tx.commit().await.map_err(DbError::from)?;
        self.count(&attempt.kind, attempt.form, done);
        Ok(done)
    }

    /// The remote call of an attempt. No connection and no lock is held while it runs.
    async fn call(&self, attempt: &Attempt) -> Called {
        let attempts = attempts_of(&attempt.unit);
        match &attempt.work {
            Work::Route {
                person,
                config,
                message,
                aim,
                ..
            } => match aim {
                Aim::Nowhere(fault) => Called::One(Err(fault.clone())),
                Aim::One(to) => Called::One(
                    self.send(
                        &attempt.notifier,
                        delivery_of(
                            attempt.unit[0].id,
                            attempts,
                            config.clone(),
                            to.clone(),
                            Some(person.as_ref()),
                            message,
                        ),
                    )
                    .await,
                ),
                Aim::Browsers(browsers) => {
                    let already: Vec<Id> = attempt
                        .unit
                        .iter()
                        .filter_map(|row| Payload::read(&row.payload))
                        .flat_map(|payload| payload.delivered_to)
                        .collect();
                    let mut answers = Vec::new();
                    for browser in browsers.iter().filter(|each| !already.contains(&each.id)) {
                        let delivery = delivery_of(
                            attempt.unit[0].id,
                            attempts,
                            config.clone(),
                            channels::device_address(browser),
                            Some(person.as_ref()),
                            message,
                        );
                        let answer = self.send(&attempt.notifier, delivery).await;
                        answers.push((
                            browser.id,
                            pages::device_label(&browser.user_agent),
                            answer,
                        ));
                    }
                    Called::Browsers(answers)
                }
            },
            Work::Destination { delivery, .. } | Work::Mail { delivery, .. } => {
                Called::One(self.send(&attempt.notifier, delivery.clone()).await)
            }
        }
    }

    /// Records what an attempt came to, under the attempt it was.
    async fn conclude(
        &self,
        conn: &mut Conn,
        now: DateTime<Utc>,
        attempt: &Attempt,
        called: Called,
    ) -> Result<Run, OutboxError> {
        let attempts = attempts_of(&attempt.unit);
        match (&attempt.work, called) {
            (Work::Route { route, channel, .. }, Called::One(outcome)) => {
                let next = self.next(&outcome, attempts);
                let done = self
                    .record_unit(conn, now, attempt, &outcome, next, None)
                    .await?;
                self.route_outcome(conn, now, (*route, *channel), &outcome, next, &[])
                    .await?;
                Ok(done)
            }
            (
                Work::Route {
                    route,
                    channel,
                    person,
                    ..
                },
                Called::Browsers(answers),
            ) => {
                self.conclude_browsers(conn, now, attempt, (*route, *channel, person.id), answers)
                    .await
            }
            (Work::Destination { destination, .. }, Called::One(outcome)) => {
                let next = self.next(&outcome, attempts);
                let done = self
                    .record_unit(conn, now, attempt, &outcome, next, None)
                    .await?;
                match next {
                    Next::Taken => rows::record_destination_ok(conn, *destination, now).await?,
                    Next::Gone => {
                        rows::switch_off_destination(conn, *destination, now).await?;
                    }
                    Next::RetryAfter(_) | Next::Failed => {
                        if let Err(error) = &outcome {
                            rows::record_destination_error(conn, *destination, &error_line(error))
                                .await?;
                        }
                    }
                }
                Ok(done)
            }
            (
                Work::Mail {
                    channel, scrubbed, ..
                },
                Called::One(outcome),
            ) => {
                let next = self.next(&outcome, attempts);
                // The secret is kept for as long as the message may still be sent.
                let after = (!matches!(next, Next::RetryAfter(_))).then_some(scrubbed);
                let done = self
                    .record_unit(conn, now, attempt, &outcome, next, after)
                    .await?;
                match next {
                    Next::Taken => rows::record_channel_ok(conn, *channel, now).await?,
                    Next::RetryAfter(_) | Next::Failed => {
                        if let Err(error) = &outcome {
                            rows::record_channel_error(conn, *channel, &error_line(error)).await?;
                        }
                    }
                    // A mailbox that does not exist is nothing the mail server is to
                    // blame for, and there is no route to switch off.
                    Next::Gone => {}
                }
                Ok(done)
            }
            (Work::Destination { .. } | Work::Mail { .. }, Called::Browsers(_)) => {
                Ok(Run::default())
            }
        }
    }

    /// Push: each browser's answer is kept with the browser, a browser that is gone or failed too
    /// often is taken off the list, and the route is `partial` where some were reached and others
    /// lost. A person who has no browser left is no longer reached: the route is off, and says it
    /// needs one.
    async fn conclude_browsers(
        &self,
        conn: &mut Conn,
        now: DateTime<Utc>,
        attempt: &Attempt,
        (route, channel, user): (Id, Id, Id),
        answers: Vec<(Id, String, Result<(), DeliverError>)>,
    ) -> Result<Run, OutboxError> {
        let attempts = attempts_of(&attempt.unit);
        let mut reached: Vec<Id> = attempt
            .unit
            .iter()
            .filter_map(|row| Payload::read(&row.payload))
            .flat_map(|payload| payload.delivered_to)
            .collect();
        let mut worst: Option<DeliverError> = None;
        let mut waiting = 0_usize;
        let mut lost: Vec<String> = Vec::new();
        for (browser, label, answer) in answers {
            match answer {
                Ok(()) => {
                    reached.push(browser);
                    people::push_succeeded(conn, browser, now).await?;
                }
                Err(error) if error.gone => {
                    people::remove_push_subscription(conn, browser).await?;
                    lost.push(label);
                }
                Err(error) => {
                    let failures = people::push_failed(conn, browser).await?;
                    if failures.is_some_and(|count| count >= PUSH_FAILURES_BEFORE_REMOVAL) {
                        people::remove_push_subscription(conn, browser).await?;
                        lost.push(label);
                    } else {
                        waiting += 1;
                    }
                    worst.get_or_insert(error);
                }
            }
        }
        let left = people::push_subscriptions_of(conn, user).await?;
        if reached.is_empty() && waiting == 0 {
            // Nothing took the message and nothing is left to try: the person has no browser
            // that takes push. The route is off until they add one.
            let done = self
                .record_unit(
                    conn,
                    now,
                    attempt,
                    &Err(DeliverError::gone(
                        "no_device",
                        "the person has no browser that takes push",
                    )),
                    Next::Failed,
                    None,
                )
                .await?;
            if left.is_empty() {
                routes::switch_off(conn, route, false, now).await?;
                let failure = routes::failure_json(
                    health::TARGET_GONE,
                    "No browser of yours takes push any longer.",
                    &lost,
                );
                let episode =
                    routes::record_failure(conn, route, Some(health::TARGET_GONE), &failure, now)
                        .await?;
                self.tell_attention(conn, now, route, episode).await?;
            }
            return Ok(done);
        }
        let result = match (&worst, waiting) {
            (Some(error), still) if still > 0 => Err(error.clone()),
            _ => Ok(()),
        };
        let next = self.next(&result, attempts);
        let payload = marked_reached(&attempt.unit[0].payload, &reached);
        let done = self
            .record_unit(conn, now, attempt, &result, next, Some(&payload))
            .await?;
        self.route_outcome(conn, now, (route, channel), &result, next, &lost)
            .await?;
        Ok(done)
    }

    /// What a route keeps of an outcome: when its notifier accepted, or why it failed, with the
    /// one notice of a failure episode's first terminal outcome.
    async fn route_outcome(
        &self,
        conn: &mut Conn,
        now: DateTime<Utc>,
        (route, channel): (Id, Id),
        outcome: &Result<(), DeliverError>,
        next: Next,
        lost: &[String],
    ) -> Result<(), OutboxError> {
        match next {
            Next::Taken => {
                let partial = (!lost.is_empty()).then(|| {
                    routes::failure_json(
                        health::PARTIAL,
                        "Push no longer reaches some of your browsers.",
                        lost,
                    )
                });
                let episode = routes::record_accepted(conn, route, partial.as_ref(), now).await?;
                rows::record_channel_ok(conn, channel, now).await?;
                self.tell_attention(conn, now, route, episode).await?;
            }
            Next::RetryAfter(_) => {
                if let Err(error) = outcome {
                    let failure = routes::failure_json(&error.code, &kept(&error.message), lost);
                    routes::record_failure(conn, route, None, &failure, now).await?;
                    rows::record_channel_error(conn, channel, &error_line(error)).await?;
                }
            }
            Next::Failed => {
                if let Err(error) = outcome {
                    let failure = routes::failure_json("exhausted", &kept(&error.message), lost);
                    let episode =
                        routes::record_failure(conn, route, Some(health::FAILED), &failure, now)
                            .await?;
                    rows::record_channel_error(conn, channel, &error_line(error)).await?;
                    self.tell_attention(conn, now, route, episode).await?;
                }
            }
            Next::Gone => {
                routes::switch_off(conn, route, true, now).await?;
                let message = outcome
                    .as_ref()
                    .err()
                    .map(|error| kept(&error.message))
                    .unwrap_or_default();
                let failure = routes::failure_json(health::TARGET_GONE, &message, lost);
                let episode =
                    routes::record_failure(conn, route, Some(health::TARGET_GONE), &failure, now)
                        .await?;
                self.tell_attention(conn, now, route, episode).await?;
            }
        }
        Ok(())
    }

    /// Tells the person of a route's failure episode once, in the Inbox only.
    async fn tell_attention(
        &self,
        conn: &mut Conn,
        now: DateTime<Utc>,
        route: Id,
        episode: Option<Id>,
    ) -> Result<(), OutboxError> {
        let Some(episode) = episode else {
            return Ok(());
        };
        let Some(row) = routes::route(conn, route).await? else {
            return Ok(());
        };
        let waiting = routes::waiting_count(conn, route).await?;
        people::tell_route_attention(conn, &row, episode, waiting, now).await?;
        Ok(())
    }

    // ----- the parts --------------------------------------------------------------------------

    /// Renders the message of notifications for a person or for a node.
    async fn render_for(
        &self,
        conn: &mut Conn,
        row_id: Id,
        declared: &NotifierDescriptor,
        own_texts: &Json,
        person: Option<&Recipient>,
        snapshots: &[Snapshot],
    ) -> Result<Result<render::Message, render::RenderFault>, OutboxError> {
        let surroundings = &self.inner.surroundings;
        let node_ids: Vec<Id> = snapshots.iter().filter_map(|each| each.node_id).collect();
        let places = people::places(conn, &node_ids).await?;
        let org_name = match snapshots.last() {
            Some(newest) => match (newest.node_id, newest.chat_id) {
                (Some(node), _) => places
                    .iter()
                    .find(|place| place.id == node)
                    .map(|place| place.org_name.clone()),
                (None, Some(chat)) => people::org_of_chat(conn, chat).await?.map(|(_, name)| name),
                (None, None) => None,
            },
            None => None,
        }
        .unwrap_or_else(|| surroundings.system_name.clone());
        let language = person.map_or(render::DESTINATION_LANGUAGE, |person| {
            person.locale.as_str()
        });
        Ok(render::render(
            row_id,
            &Rendering {
                descriptor: declared,
                own_texts,
                language,
                recipient: person,
                notifications: snapshots,
                places: &places,
                org_name: &org_name,
                public_url: &self.inner.surroundings.public_url,
            },
        ))
    }

    async fn send(
        &self,
        notifier: &Arc<dyn Notifier>,
        delivery: Delivery,
    ) -> Result<(), DeliverError> {
        guarded(notifier.deliver(delivery)).await
    }

    /// Writes what a try came to into every row of the attempt, under that attempt.
    async fn record_unit(
        &self,
        conn: &mut Conn,
        now: DateTime<Utc>,
        attempt: &Attempt,
        outcome: &Result<(), DeliverError>,
        next: Next,
        payload: Option<&Json>,
    ) -> Result<Run, OutboxError> {
        let error = outcome.as_ref().err().map(error_line).unwrap_or_default();
        let how = match next {
            Next::Taken => Settle::Sent { payload },
            Next::RetryAfter(pause) => Settle::Retry {
                at: now + spans::seconds(pause.as_secs()),
                error: &error,
                payload,
            },
            Next::Failed | Next::Gone => Settle::Failed {
                error: &error,
                payload,
            },
        };
        let mut recorded = false;
        for row in &attempt.unit {
            recorded |= db_outbox::record(conn, row.id, attempt.dispatch, &how).await?;
        }
        if !recorded {
            // The attempt was given up, retried or cancelled meanwhile: its answer changes nothing.
            return Ok(Run::default());
        }
        Ok(Run::one(match next {
            Next::Taken => Done::Sent,
            Next::RetryAfter(_) => Done::Retried,
            Next::Failed => Done::Failed,
            Next::Gone => Done::Gone,
        }))
    }

    fn count(&self, channel: &str, form: Form, done: Run) {
        let result = if done.sent > 0 {
            "sent"
        } else if done.gone > 0 {
            "gone"
        } else if done.failed > 0 {
            "failed"
        } else {
            return;
        };
        self.inner.metrics.route_message(channel, form, result);
    }
}

/// The delivery of a message to one place.
fn delivery_of(
    id: Id,
    attempt: u32,
    config: Config,
    to: Json,
    person: Option<&Recipient>,
    message: &render::Message,
) -> Delivery {
    Delivery {
        id,
        attempt,
        config,
        to,
        recipient: person.map(render::user_ref),
        language: person.map_or_else(
            || render::DESTINATION_LANGUAGE.to_owned(),
            |p| p.locale.clone(),
        ),
        subject: message.subject.clone(),
        body: message.body.clone(),
        notifications: message.notifications.clone(),
    }
}

/// The try that is being made: one more than the most that any row of the unit
/// has had.
fn attempts_of(unit: &[OutboxRow]) -> u32 {
    let most = unit.iter().map(|row| row.attempts).max().unwrap_or(0);
    u32::try_from(most).unwrap_or(0) + 1
}

/// The notifications of every row of a unit, oldest first.
fn merged_notifications(unit: &[OutboxRow]) -> Vec<Snapshot> {
    let mut all: Vec<Snapshot> = unit
        .iter()
        .filter_map(|row| Payload::read(&row.payload))
        .flat_map(|payload| payload.notifications)
        .collect();
    all.sort_by_key(|each| each.at);
    all
}

/// The payload of the first row, with the browsers a try reached noted in it.
fn marked_reached(payload: &Json, reached: &[Id]) -> Json {
    let mut marked = Payload::read(payload).unwrap_or_default();
    marked.delivered_to = reached.to_vec();
    marked.to_json()
}

/// What a failure also touches besides the rows.
#[derive(Clone, Copy)]
enum Also<'a> {
    /// Nothing.
    Nothing,
    /// The payload the rows keep from now on: the same, with what is secret
    /// blanked.
    Payload(&'a Json),
    /// The channel that is to blame, which keeps the reason as its last error.
    Channel(&'a ChannelRow),
}

/// Fails every row of a unit that is not reserved, for the reason given.
async fn fail_all(
    conn: &mut Conn,
    unit: &[OutboxRow],
    reason: &str,
    also: Also<'_>,
) -> Result<Run, OutboxError> {
    let reason = kept(reason);
    let payload = match also {
        Also::Payload(payload) => Some(payload),
        _ => None,
    };
    for row in unit {
        db_outbox::settle(
            conn,
            row.id,
            &Settle::Failed {
                error: &reason,
                payload,
            },
        )
        .await?;
    }
    if let Also::Channel(channel) = also {
        rows::record_channel_error(conn, channel.id, &reason).await?;
    }
    Ok(Run::one(Done::Failed))
}
