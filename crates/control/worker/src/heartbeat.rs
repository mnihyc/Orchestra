//! Worker health, delivery metrics, and the worker's own samples of them.
//!
//! The worker reports that it is alive and which build it is in a row of its
//! own every few seconds, so that nothing has to reach it to know. What it
//! delivers is counted as it happens (`orch_route_messages_total`), the depth of
//! its queue and the number of people's routes are read from the database
//! (`orch_outbox_pending`, `orch_routes`), and once a minute the worker writes
//! what it holds, as a value for the minute, into the measurements the server
//! stores: a counter as it grew in that minute, a gauge as it stood.

use crate::channels::Notifiers;
use chrono::{DateTime, DurationRound, Utc};
use orch_db::queries::notifications::upkeep::Measure;
use orch_db::queries::notifications::{outbox, routes, upkeep};
use orch_db::{Db, DbError, heartbeat};
use orch_notifier_contract::templates::Form;
use orch_types::{Clock, Id};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

/// The name this service reports under.
pub const COMPONENT: &str = "worker";

/// A series of `orch_route_messages_total`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Series {
    channel: String,
    form: &'static str,
    result: &'static str,
}

/// The series of gauges that have been reported, so that one that is no longer
/// there is reported as 0 and not left at the value it last had.
#[derive(Debug, Default)]
struct Reported {
    /// Channels that had rows waiting in the outbox.
    pending: BTreeSet<String>,
    /// Channels and the states their routes were in.
    routes: BTreeSet<(String, &'static str)>,
}

/// What the worker has counted.
#[derive(Debug, Default)]
pub struct Metrics {
    /// How many messages left, by channel, form and result.
    messages: Mutex<BTreeMap<Series, u64>>,
    /// The counts at the last sample.
    sampled: Mutex<BTreeMap<Series, u64>>,
    /// The gauges set so far.
    reported: Mutex<Reported>,
}

fn locked<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Metrics {
    /// Nothing counted yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Counts a message that left the application for a person or a node:
    /// `result` is `sent`, `failed` or `gone`.
    pub fn route_message(&self, channel: &str, form: Form, result: &'static str) {
        let form = match form {
            Form::Single => "single",
            Form::Digest => "digest",
        };
        metrics::counter!(
            "orch_route_messages_total",
            "channel" => channel.to_owned(), "form" => form, "result" => result
        )
        .increment(1);
        *locked(&self.messages)
            .entry(Series {
                channel: channel.to_owned(),
                form,
                result,
            })
            .or_default() += 1;
    }

    /// How many messages of each series there were since the last time this
    /// was asked.
    fn grown(&self) -> Vec<(Series, u64)> {
        let now = locked(&self.messages).clone();
        let mut before = locked(&self.sampled);
        let grown = now
            .iter()
            .filter_map(|(series, count)| {
                let grew = count - before.get(series).copied().unwrap_or(0);
                (grew > 0).then(|| (series.clone(), grew))
            })
            .collect();
        *before = now;
        grown
    }
}

/// Who this process is: drawn when it started, so that a new one means a
/// restart.
#[derive(Debug, Clone)]
pub struct Identity {
    /// A value drawn when the process started.
    pub run: Id,
    /// The release it is.
    pub version: String,
    /// When it started.
    pub started_at: DateTime<Utc>,
}

/// Stores this process's actual recorder once a minute; histograms retain only an interval mean beside the full exporter.
pub async fn keep_measurements(
    db: Db,
    clock: Arc<dyn Clock>,
    readings: orch_telemetry::Readings,
    component: &'static str,
    store: bool,
    stop: tokio_util::sync::CancellationToken,
) {
    let process = orch_types::new_id();
    let mut every = tokio::time::interval(std::time::Duration::from_secs(60));
    every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {()=stop.cancelled()=>return,_=every.tick()=>{}}
        let pool = db.pool();
        metrics::gauge!("orch_db_pool_in_use","component"=>component).set(f64::from(
            pool.size()
                .saturating_sub(u32::try_from(pool.num_idle()).unwrap_or(0)),
        ));
        readings.upkeep();
        let mut samples = readings.take_samples();
        if component == "worker" {
            // These four already have their existing exact worker sample writer below.
            samples.retain(|sample| {
                !matches!(
                    sample.name.as_str(),
                    "orch_outbox_pending"
                        | "orch_routes"
                        | "orch_db_pool_in_use"
                        | "orch_route_messages_total"
                )
            });
        }
        if !store {
            continue;
        }
        let result = async {
            let mut conn = db.acquire().await?;
            orch_db::queries::metrics::write_process_samples(
                &mut conn,
                clock.now(),
                component,
                process,
                &samples,
            )
            .await
        }
        .await;
        if let Err(error) = result {
            tracing::debug!(%error,component,"the process measurements could not be stored");
        }
    }
}

impl Identity {
    /// The identity of a process that starts now.
    #[must_use]
    pub fn begin(version: &str, clock: &dyn Clock) -> Self {
        Self {
            run: orch_types::new_id(),
            version: version.to_owned(),
            started_at: clock.now(),
        }
    }
}

/// Reports that the worker is alive.
pub async fn beat(db: &Db, clock: &dyn Clock, who: &Identity) -> Result<(), DbError> {
    let mut conn = db.acquire().await?;
    heartbeat::beat(
        &mut conn,
        COMPONENT,
        who.run,
        &who.version,
        who.started_at,
        clock.now(),
    )
    .await?;
    crate::gauges::transactions(&mut conn).await?;
    crate::gauges::operational(&mut conn, clock.now()).await
}

/// The states a route is counted in.
fn route_state(links_people: bool, enabled: bool, linked: bool) -> &'static str {
    match (enabled, links_people, linked) {
        (true, _, _) => "on",
        (false, true, false) => "unlinked",
        (false, _, _) => "off",
    }
}

/// Reads what the database holds that is a measurement, sets the gauges that
/// the metrics listener serves, and, where the system stores measurements,
/// writes one value for the minute `now` is in for every series the worker
/// holds. Says how many values it wrote.
pub async fn sample(
    db: &Db,
    clock: &dyn Clock,
    metrics: &Metrics,
    notifiers: &Notifiers,
    store: bool,
) -> Result<usize, DbError> {
    let now = clock.now();
    let minute = now
        .duration_trunc(chrono::Duration::minutes(1))
        .unwrap_or(now);
    let mut conn = db.acquire().await?;
    let mut values: Vec<(&'static str, serde_json::Value, f64, Measure)> = Vec::new();

    let mut pending: BTreeMap<String, i64> = outbox::pending_by_channel(&mut conn)
        .await?
        .into_iter()
        .collect();
    let mut states: BTreeMap<(String, &'static str), i64> = BTreeMap::new();
    for each in routes::route_counts(&mut conn).await? {
        let links = notifiers
            .get(&each.channel_kind)
            .is_some_and(|notifier| notifier.describe().reaches.links_people());
        *states
            .entry((
                each.channel_kind,
                route_state(links, each.enabled, each.linked),
            ))
            .or_default() += each.routes;
    }
    {
        // A series that was there and is not now stands at 0.
        let mut reported = locked(&metrics.reported);
        for channel in &reported.pending {
            pending.entry(channel.clone()).or_insert(0);
        }
        for key in &reported.routes {
            states.entry(key.clone()).or_insert(0);
        }
        reported.pending.extend(pending.keys().cloned());
        reported.routes.extend(states.keys().cloned());
    }
    for (channel, count) in pending {
        let count = u32::try_from(count).map_or(f64::MAX, f64::from);
        metrics::gauge!("orch_outbox_pending", "channel" => channel.clone()).set(count);
        values.push((
            "outbox_pending",
            serde_json::json!({ "channel": channel }),
            count,
            Measure::Gauge,
        ));
    }
    for ((channel, state), count) in states {
        let count = u32::try_from(count).map_or(f64::MAX, f64::from);
        metrics::gauge!("orch_routes", "channel" => channel.clone(), "state" => state).set(count);
        values.push((
            "routes",
            serde_json::json!({ "channel": channel, "state": state }),
            count,
            Measure::Gauge,
        ));
    }

    // The operator's gauges the database answers; one that cannot be read leaves the others as they are.
    if let Err(error) = crate::gauges::sample(&mut conn, now).await {
        tracing::debug!(%error, "the gauges the database answers were not read");
    }

    let pool = db.pool();
    let in_use = f64::from(
        pool.size()
            .saturating_sub(u32::try_from(pool.num_idle()).unwrap_or(0)),
    );
    metrics::gauge!("orch_db_pool_in_use", "component" => COMPONENT).set(in_use);
    values.push((
        "db_pool_in_use",
        serde_json::json!({ "component": COMPONENT }),
        in_use,
        Measure::Gauge,
    ));

    for (series, grew) in metrics.grown() {
        values.push((
            "route_messages_total",
            serde_json::json!({ "channel": series.channel, "form": series.form, "result": series.result }),
            f64::from(u32::try_from(grew).unwrap_or(u32::MAX)),
            Measure::Counter,
        ));
    }

    if !store {
        return Ok(0);
    }
    let mut written = 0;
    for (name, labels, value, measure) in values {
        upkeep::write_sample(&mut conn, minute, name, &labels, value, measure).await?;
        written += 1;
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_was_counted_since_the_last_sample_is_what_grew() {
        let metrics = Metrics::new();
        metrics.route_message("email", Form::Single, "sent");
        metrics.route_message("email", Form::Single, "sent");
        metrics.route_message("telegram", Form::Digest, "gone");
        let first: Vec<_> = metrics
            .grown()
            .into_iter()
            .map(|(series, grew)| (series.channel, series.form, series.result, grew))
            .collect();
        assert_eq!(
            first,
            [
                ("email".to_owned(), "single", "sent", 2),
                ("telegram".to_owned(), "digest", "gone", 1)
            ]
        );
        assert!(metrics.grown().is_empty(), "nothing grew since");
        metrics.route_message("email", Form::Single, "sent");
        assert_eq!(metrics.grown().len(), 1);
    }

    #[test]
    fn a_route_is_on_off_or_waiting_for_a_link_that_was_never_followed() {
        assert_eq!(route_state(false, true, false), "on");
        assert_eq!(route_state(true, true, true), "on");
        assert_eq!(route_state(true, false, false), "unlinked");
        assert_eq!(route_state(true, false, true), "off");
        assert_eq!(route_state(false, false, false), "off");
    }
}
